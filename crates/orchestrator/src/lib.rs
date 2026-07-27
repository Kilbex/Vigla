//! Vigla orchestrator: process supervision and event normalization.
//!
//! Thin spine that supervises external CLI worker processes, normalizes
//! their output into the canonical event schema (the `event-schema`
//! crate), and feeds the macOS app via Tauri IPC.
//!
//! The public surface covers worker supervision, durable event storage,
//! mission execution and recovery, audit, judgment, and reversible integration.

// R3: several long-lived supervisor APIs intentionally take ~8
// arguments — splitting into a config struct just to pacify clippy
// would obscure the call sites that already pass cohesive batches of
// the same fields. Allow workspace-wide for the orchestrator crate.
#![allow(clippy::too_many_arguments)]

pub mod acl;
pub mod arbiter;
pub mod audit;
pub mod endurance;
mod ephemeral_context;
mod error;
pub mod escalation;
pub mod host_services;
pub mod ids;
pub mod judgment;
pub mod memory;
pub mod mission;
pub mod mission_event;
pub mod mission_runtime;
pub mod mission_supervisor_run;
pub mod mission_worker_dispatch;
pub mod mission_workspace;
pub mod mock_worker;
pub mod parser;
mod process_tree;
pub mod recovery;
mod repository;
pub mod retention;
mod skills;
mod supervisor;
pub mod task_graph;
pub mod vendor_profile;

// ── Mission lifecycle ────────────────────────────────────────
// Consumed by app/src-tauri/src/{lib.rs, mission_history_command.rs,
// memory_commands.rs} and orchestrator/tests/*.
pub use mission::{MissionSpec, MissionState, ResolveAction};
pub use mission_event::{MergeResolution, MissionEvent, MissionEventKind, TaskDescriptor};
pub use mission_runtime::{MissionEventReceiver, MissionRuntime};
pub use mission_workspace::MissionWorkspace;

// ── Persistence & retention ──────────────────────────────────
// Consumed by app/src-tauri/src/lib.rs and the orchestrator's own
// integration tests in orchestrator/tests/.
pub use error::RepositoryError;
pub use repository::{
    DispositionAction, DispositionIntentDto, InsertOutcome, MissionHistoryDto,
    MissionHistoryStatus, MissionOutcomeDto, MissionOutcomeState, Repository,
};
pub use retention::RetentionGuard;

// ── Host services / IPC entrypoints ──────────────────────────
// Consumed by app/src-tauri/src/lib.rs (Tauri command bridge).
pub use host_services::{
    cleanup_aborted_mission_artifacts, continue_worker, get_worker_diff,
    reconcile_disposition_journal, start_claude_worker, start_codex_worker, start_gemini_worker,
    validate_working_dir, MissionController,
};

// ── Worker supervision ───────────────────────────────────────
// Consumed by app/src-tauri/src/lib.rs (Supervisor + SpawnRequest +
// SupervisorError) and by orchestrator integration tests.
pub use supervisor::coalescing::CoalescingSink;
pub use supervisor::{DispatchRequest, RetryPolicy, SpawnRequest, Supervisor, SupervisorError};
pub use vendor_profile::WorkerVendor;

// ── Judgment / completion verdicts ───────────────────────────
// Consumed by app/src-tauri/src/lib.rs for the inbox-card payloads.
pub use judgment::{CompletionVerdict, RiskBand, UnresolvedIssue};

// ── Endurance (U10 Pillar A — all-day fleet) ─────────────────
// Durable liveness heartbeat + stall detection + the endurance gate.
// Consumed by the `orchestrator_endurance` bin and integration tests;
// the live mission loop integration is the documented next step.
pub use endurance::{
    BeatStatus, Clock, EnduranceConfig, EnduranceGate, EnduranceMonitor, EnduranceReport,
    GateOutcome, Heartbeat, Liveness, SystemClock,
};

// Symbols defined further inside the crate (acl::*, arbiter::*,
// audit::*, escalation::*, memory::*, mission_supervisor_run::*,
// mission_worker_dispatch::*, mock_worker::*, recovery::*,
// task_graph::*, vendor_profile::*) are reachable via their deep
// module paths and not re-exported flat here — those are
// implementation details rather than the host-facing surface.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

static START: OnceLock<Instant> = OnceLock::new();
static PANIC_HOOK: OnceLock<()> = OnceLock::new();
const LOGIN_SHELL_PATH_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_LOGIN_SHELL_OUTPUT_BYTES: usize = 64 * 1024;

#[cfg(unix)]
fn capture_login_shell_path(shell: &str, timeout: Duration) -> Option<String> {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::process::Stdio;

    let mut command = std::process::Command::new(shell);
    command
        .arg("-l")
        .arg("-c")
        .arg("echo $PATH")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    let mut child = command.spawn().ok()?;
    let mut stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            terminate_login_shell(&mut child);
            return None;
        }
    };
    let descriptor = stdout.as_raw_fd();
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        terminate_login_shell(&mut child);
        return None;
    }

    let started = Instant::now();
    let mut output = Vec::new();
    let mut stdout_closed = false;
    let mut chunk = [0_u8; 4096];
    loop {
        if !stdout_closed {
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) => {
                        stdout_closed = true;
                        break;
                    }
                    Ok(read)
                        if output.len().saturating_add(read) <= MAX_LOGIN_SHELL_OUTPUT_BYTES =>
                    {
                        output.extend_from_slice(&chunk[..read]);
                    }
                    Ok(_) => {
                        terminate_login_shell(&mut child);
                        return None;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        terminate_login_shell(&mut child);
                        return None;
                    }
                }
            }
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let path = String::from_utf8_lossy(&output).trim().to_string();
                return (!path.is_empty()).then_some(path);
            }
            Ok(None) => {}
            Err(_) => {
                terminate_login_shell(&mut child);
                return None;
            }
        }

        if started.elapsed() >= timeout {
            terminate_login_shell(&mut child);
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(not(unix))]
fn capture_login_shell_path(_shell: &str, _timeout: Duration) -> Option<String> {
    // GUI PATH recovery is a macOS concern. On non-Unix targets, retain the
    // inherited PATH rather than start a blocking reader that cannot be
    // interrupted portably when a descendant inherits the capture pipe.
    None
}

fn terminate_login_shell(child: &mut std::process::Child) {
    #[cfg(unix)]
    if unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) } != 0 {
        let _ = child.kill();
    }
    #[cfg(not(unix))]
    let _ = child.kill();
    let _ = child.wait();
}

/// Mark the orchestrator's start time. Idempotent — safe to call from
/// the host's `setup` hook on every launch.
pub fn init() {
    START.get_or_init(Instant::now);
    install_panic_hook();
}

/// R2 — install a global panic hook that records a payload-free panic
/// diagnostic via `tracing::error!`. Without this, panics inside
/// `tokio::spawn`ed tasks are silently dropped
/// by the runtime and panics on the main thread surface only as a
/// macOS crash dialog with no actionable signal.
///
/// The hook deliberately does not chain to the previous/default hook: those
/// hooks format the raw panic payload, which may contain a prompt, token, or
/// path and can be captured by persistent host logs. Unwinding/abort behavior
/// is independent of hook chaining, so callers still receive the panic while
/// diagnostics retain the categorical payload kind and source location.
fn install_panic_hook() {
    PANIC_HOOK.get_or_init(|| {
        std::panic::set_hook(Box::new(move |info| {
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "<unknown>".into());
            let payload_kind = panic_payload_kind(info.payload());
            write_panic_stderr_diagnostic(&location, payload_kind);
            tracing::error!(
                target: "vigla::panic",
                location = %location,
                payload_kind,
                "panic captured"
            );
        }));
    });
}

/// R2 — spawn a tokio task with panic visibility. The returned
/// `JoinHandle` resolves when the wrapped future completes; if it
/// panics, a payload-free diagnostic is logged via `tracing::error!`
/// with the task `name` for grep-ability, then re-raised so callers that
/// `.await?` still see `JoinError::is_panic()`. Callers that detach
/// the handle (the common case for long-running background loops)
/// still get the log line because the wrapper runs `catch_unwind`
/// in-place rather than relying on the parent ever joining.
///
/// Use this in place of bare `tokio::spawn` for any long-running
/// loop whose panic would otherwise vanish — coordination flush,
/// retention sweepers, supervision loops.
pub fn spawn_supervised<F>(name: &'static str, fut: F) -> tokio::task::JoinHandle<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    use futures::FutureExt;
    tokio::spawn(async move {
        let outcome = std::panic::AssertUnwindSafe(fut).catch_unwind().await;
        if let Err(panic) = outcome {
            let payload_kind = panic_payload_kind(panic.as_ref());
            tracing::error!(
                target: "vigla::panic",
                task = name,
                payload_kind,
                "spawned task panicked"
            );
            // Re-raise so test harnesses and tokio's own JoinError
            // path still see the panic; production callers that
            // detach the handle simply drop the join receiver.
            std::panic::resume_unwind(panic);
        }
    })
}

fn panic_payload_kind(payload: &(dyn std::any::Any + Send)) -> &'static str {
    if payload.is::<&str>() || payload.is::<String>() {
        "string"
    } else {
        "non-string"
    }
}

fn write_panic_stderr_diagnostic(location: &str, payload_kind: &str) {
    use std::io::Write;

    // A tracing subscriber is installed by the desktop host, but panics can
    // happen before host setup or in library-only consumers. Keep a minimal
    // categorical stderr signal without ever formatting the panic payload.
    let _ = writeln!(
        std::io::stderr().lock(),
        "vigla: panic captured payload_kind={payload_kind} location={location}"
    );
}

/// Snapshot of orchestrator liveness. Plain data; serde lives in the
/// host crate so the orchestrator stays serde-light internally (the
/// only serde use is JSON shuttling for event payloads in
/// [`Repository`]).
#[derive(Debug, Clone, Copy)]
pub struct HealthStatus {
    pub version: &'static str,
    pub uptime_ms: u64,
}

/// Return a current liveness snapshot. If [`init`] was never called
/// (e.g. a unit test that pulls the function in isolation), the start
/// time is set lazily on first call.
pub fn health_check() -> HealthStatus {
    let start = START.get_or_init(Instant::now);
    HealthStatus {
        version: VERSION,
        uptime_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

/// Resolve the user's shell `PATH` once and cache it. macOS GUI-launched
/// apps inherit a minimal `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`), so
/// without this every node-managed binary (claude / codex / gemini,
/// nvm-installed and otherwise) reports as "not detected" even when the
/// user can run them fine in a terminal. We invoke the user's login
/// shell to capture its `PATH`; if that fails (or returns empty), we
/// fall back to the inherited `PATH` plus a curated list of common
/// shell-managed locations (homebrew, nvm, cargo, asdf, etc.).
pub fn resolve_user_path() -> &'static str {
    static USER_PATH: OnceLock<String> = OnceLock::new();
    USER_PATH.get_or_init(|| {
        let inherited = std::env::var("PATH").unwrap_or_default();
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let home = std::env::var("HOME").unwrap_or_default();
        resolve_user_path_uncached(&shell, &inherited, &home, LOGIN_SHELL_PATH_TIMEOUT)
    })
}

fn resolve_user_path_uncached(
    shell: &str,
    inherited: &str,
    home: &str,
    timeout: Duration,
) -> String {
    // Login shell + `-c` so user's rc files run and PATH is the same
    // one their terminal sees. Capture stdout only; rc files often
    // print warnings to stderr we don't care about.
    if let Some(captured) = capture_login_shell_path(shell, timeout) {
        return if inherited.is_empty() {
            captured
        } else {
            format!("{captured}:{inherited}")
        };
    }

    let mut paths: Vec<String> = vec![
        "/opt/homebrew/bin".into(),
        "/opt/homebrew/sbin".into(),
        "/usr/local/bin".into(),
        "/usr/local/sbin".into(),
    ];
    if !home.is_empty() {
        paths.push(format!("{home}/.local/bin"));
        paths.push(format!("{home}/.cargo/bin"));
        let nvm_dir = format!("{home}/.nvm/versions/node");
        if let Ok(entries) = std::fs::read_dir(&nvm_dir) {
            for entry in entries.flatten() {
                let path = entry.path().join("bin");
                if path.exists() {
                    paths.push(path.to_string_lossy().into_owned());
                }
            }
        }
    }
    if !inherited.is_empty() {
        paths.push(inherited.to_owned());
    }
    paths.join(":")
}

/// Default location for the Vigla SQLite database, following each
/// platform's own application-data convention:
///
/// | Target | Path |
/// |---|---|
/// | macOS | `~/Library/Application Support/Vigla/vigla.sqlite` |
/// | Windows | `%APPDATA%\Vigla\vigla.sqlite` |
/// | Linux and every other Unix | `$XDG_DATA_HOME/vigla/vigla.sqlite`, else `~/.local/share/vigla/vigla.sqlite` |
///
/// XDG is the fallback for anything that is neither macOS nor Windows;
/// there is no fourth shape.
///
/// The `VIGLA_DB_PATH` env var overrides all of them (used by tests
/// and developer setups).
///
/// This is deliberately hand-rolled rather than pulled from a
/// directories crate: the orchestrator is a portable crate with a tight
/// dependency budget, and the whole rule is three branches. The macOS
/// branch must keep producing the exact historical path — changing it
/// would orphan every existing local database.
pub fn default_db_path() -> PathBuf {
    if let Ok(override_path) = std::env::var("VIGLA_DB_PATH") {
        return PathBuf::from(override_path);
    }
    app_data_root().join(DB_FILE_NAME)
}

/// File name of the SQLite database inside the application-data root.
const DB_FILE_NAME: &str = "vigla.sqlite";

/// Which application-data convention a host follows. Deliberately a
/// runtime value rather than a `#[cfg]` block: every arm then compiles
/// and is unit-testable on every host, so the Linux rule is covered by
/// the macOS developer loop and the macOS rule by the Linux CI job.
/// The `cfg!` in [`AppDataConvention::host`] is the only place the
/// target actually matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppDataConvention {
    /// `~/Library/Application Support/Vigla`.
    AppleAppSupport,
    /// `%APPDATA%\Vigla`.
    WindowsRoamingAppData,
    /// XDG Base Directory: `$XDG_DATA_HOME/vigla`.
    XdgDataHome,
}

impl AppDataConvention {
    /// The convention of the platform this binary was built for.
    fn host() -> Self {
        if cfg!(target_os = "macos") {
            Self::AppleAppSupport
        } else if cfg!(target_os = "windows") {
            Self::WindowsRoamingAppData
        } else {
            Self::XdgDataHome
        }
    }

    /// Environment variable naming the user's home directory.
    fn home_var(self) -> &'static str {
        match self {
            Self::WindowsRoamingAppData => "USERPROFILE",
            Self::AppleAppSupport | Self::XdgDataHome => "HOME",
        }
    }
}

/// Directory that holds the database (and any future sibling state)
/// on this host.
fn app_data_root() -> PathBuf {
    app_data_root_for(AppDataConvention::host(), |key| std::env::var_os(key))
}

/// [`app_data_root`] with the convention and the environment injected,
/// so every platform's rule is testable without mutating process state
/// (which would race with every other test in this binary).
fn app_data_root_for(
    convention: AppDataConvention,
    var: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> PathBuf {
    match convention {
        AppDataConvention::AppleAppSupport => {
            home_dir_for(convention, &var).join("Library/Application Support/Vigla")
        }
        AppDataConvention::WindowsRoamingAppData => {
            // %APPDATA% is the roaming application-data directory and
            // is set for every interactive session. Fall back to a
            // path under the profile directory rather than inventing a
            // relative one.
            match var("APPDATA") {
                Some(appdata) if !appdata.is_empty() => PathBuf::from(appdata).join("Vigla"),
                _ => home_dir_for(convention, &var).join("AppData/Roaming/Vigla"),
            }
        }
        AppDataConvention::XdgDataHome => {
            // $XDG_DATA_HOME wins, but only when it is an absolute
            // path — the spec says a relative value must be treated as
            // unset.
            match var("XDG_DATA_HOME").map(PathBuf::from) {
                Some(data_home) if data_home.is_absolute() => data_home.join("vigla"),
                _ => home_dir_for(convention, &var).join(".local/share/vigla"),
            }
        }
    }
}

/// The user's home directory, or `.` when the platform did not give us
/// one. An unset home must not silently root state at `/`.
fn home_dir_for(
    convention: AppDataConvention,
    var: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> PathBuf {
    match var(convention.home_var()) {
        Some(home) if !home.is_empty() => PathBuf::from(home),
        _ => PathBuf::from("."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn write_test_shell(path: &std::path::Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;

        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn login_shell_path_capture_uses_successful_shell_output() {
        let temp = tempfile::tempdir().unwrap();
        let shell = temp.path().join("successful-shell");
        write_test_shell(&shell, "printf '/test/login/bin\\n'");

        let resolved = resolve_user_path_uncached(
            shell.to_str().unwrap(),
            "/test/inherited/bin",
            temp.path().to_str().unwrap(),
            LOGIN_SHELL_PATH_TIMEOUT,
        );

        assert_eq!(resolved, "/test/login/bin:/test/inherited/bin");
    }

    #[cfg(unix)]
    #[test]
    fn login_shell_path_capture_rejects_output_over_the_cap() {
        let temp = tempfile::tempdir().unwrap();
        let shell = temp.path().join("overproducing-shell");
        write_test_shell(&shell, "yes x | head -c 70000");

        let resolved = resolve_user_path_uncached(
            shell.to_str().unwrap(),
            "/test/inherited/bin",
            temp.path().to_str().unwrap(),
            Duration::from_secs(10),
        );

        assert!(resolved.contains("/opt/homebrew/bin"));
        assert!(resolved.contains("/test/inherited/bin"));
        assert!(!resolved.contains("x\nx\nx\n"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn login_shell_path_capture_times_out_and_reaps_a_blocking_shell() {
        let temp = tempfile::tempdir().unwrap();
        let shell = temp.path().join("blocking-shell");
        let pid_file = temp.path().join("shell.pid");
        write_test_shell(
            &shell,
            &format!(
                "echo $$ > \"{}\"\nwhile :; do sleep 1; done",
                pid_file.display()
            ),
        );
        let home = temp.path().to_string_lossy().into_owned();

        let mut capture = tokio::task::spawn_blocking(move || {
            resolve_user_path_uncached(
                shell.to_str().unwrap(),
                "/test/inherited/bin",
                &home,
                Duration::from_millis(500),
            )
        });
        let result = tokio::time::timeout(Duration::from_secs(2), &mut capture).await;
        if result.is_err() {
            if let Ok(raw) = std::fs::read_to_string(&pid_file) {
                if let Ok(pid) = raw.trim().parse() {
                    unsafe {
                        libc::kill(pid, libc::SIGKILL);
                    }
                }
            }
            let _ = tokio::time::timeout(Duration::from_secs(2), capture).await;
            panic!("login shell PATH capture ignored its configured timeout");
        }
        let resolved = result.unwrap().unwrap();
        assert!(resolved.contains("/opt/homebrew/bin"));
        assert!(resolved.contains("/test/inherited/bin"));
        // Under a saturated parallel test run the timeout can expire before
        // the spawned script gets its first timeslice. If it did start, prove
        // the owned process was reaped; otherwise the bounded return itself is
        // the applicable postcondition and there is no PID to inspect.
        if let Ok(raw) = std::fs::read_to_string(pid_file) {
            let pid: i32 = raw.trim().parse().unwrap();
            assert!(unsafe { libc::kill(pid, 0) } != 0);
        }
    }

    #[test]
    fn health_check_reports_version_and_nondecreasing_uptime() {
        init();
        let a = health_check();
        assert_eq!(a.version, VERSION);
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = health_check();
        assert!(b.uptime_ms >= a.uptime_ms);
    }

    #[test]
    fn default_db_path_honors_override() {
        // Use a temp path that doesn't have to exist — we're only
        // verifying the env-var override is consulted.
        // SAFETY: tests in this crate run sequentially within this
        // module file because they share the env var; we set it for
        // this test and clear it on exit.
        unsafe {
            std::env::set_var("VIGLA_DB_PATH", "/tmp/explicit-test-path.sqlite");
        }
        let p = default_db_path();
        assert_eq!(p.to_string_lossy(), "/tmp/explicit-test-path.sqlite");
        unsafe {
            std::env::remove_var("VIGLA_DB_PATH");
        }
    }

    /// Build an environment lookup from a fixed table. Nothing here
    /// touches the real process environment, so these tests are safe
    /// to run in parallel with everything else in this binary.
    fn fake_env<'a>(
        pairs: &'a [(&'a str, &'a str)],
    ) -> impl Fn(&str) -> Option<std::ffi::OsString> + use<'a> {
        move |key: &str| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| std::ffi::OsString::from(*value))
        }
    }

    /// A full environment covering every convention, so each case
    /// below differs only in which convention it asks for.
    const EVERY_HOME: &[(&str, &str)] = &[
        ("HOME", "/home/tester"),
        ("USERPROFILE", r"C:\Users\tester"),
        ("APPDATA", r"C:\Users\tester\AppData\Roaming"),
        ("XDG_DATA_HOME", "/srv/state"),
    ];

    /// macOS installs already hold a database at the historical path.
    /// Pin it exactly: changing this string orphans every existing
    /// local database.
    #[test]
    fn apple_convention_keeps_the_shipped_application_support_path() {
        let root = app_data_root_for(
            AppDataConvention::AppleAppSupport,
            fake_env(&[("HOME", "/Users/op")]),
        );
        assert_eq!(
            root.join(DB_FILE_NAME).to_string_lossy(),
            "/Users/op/Library/Application Support/Vigla/vigla.sqlite"
        );
    }

    /// The XDG rule resolves against `$XDG_DATA_HOME`. Target-independent
    /// by construction — the convention is passed in — so it runs
    /// everywhere; what ties a *target* to this rule is
    /// [`tests::default_db_path_matches_the_build_target_convention`].
    #[test]
    fn xdg_convention_uses_data_home() {
        let root = app_data_root_for(AppDataConvention::XdgDataHome, fake_env(EVERY_HOME));
        assert_eq!(root, PathBuf::from("/srv/state/vigla"));
    }

    /// XDG says a relative `XDG_DATA_HOME` must be treated as unset.
    #[test]
    fn xdg_data_home_is_honoured_only_when_absolute() {
        let relative = app_data_root_for(
            AppDataConvention::XdgDataHome,
            fake_env(&[
                ("HOME", "/home/tester"),
                ("XDG_DATA_HOME", "relative/state"),
            ]),
        );
        assert_eq!(relative, PathBuf::from("/home/tester/.local/share/vigla"));

        let unset = app_data_root_for(
            AppDataConvention::XdgDataHome,
            fake_env(&[("HOME", "/home/tester")]),
        );
        assert_eq!(unset, PathBuf::from("/home/tester/.local/share/vigla"));
    }

    /// Windows resolves against `%APPDATA%`, and falls back under the
    /// user profile — never to a relative path.
    #[test]
    fn windows_convention_prefers_appdata_then_the_user_profile() {
        let roaming = app_data_root_for(
            AppDataConvention::WindowsRoamingAppData,
            fake_env(EVERY_HOME),
        );
        assert_eq!(
            roaming,
            PathBuf::from(r"C:\Users\tester\AppData\Roaming").join("Vigla")
        );

        // An empty %APPDATA% is as good as unset.
        let fallback = app_data_root_for(
            AppDataConvention::WindowsRoamingAppData,
            fake_env(&[("USERPROFILE", r"C:\Users\tester"), ("APPDATA", "")]),
        );
        assert_eq!(
            fallback,
            PathBuf::from(r"C:\Users\tester").join("AppData/Roaming/Vigla")
        );
    }

    /// Windows reads `USERPROFILE`; the other conventions read `HOME`.
    /// Getting this backwards would silently root state in `.`.
    #[test]
    fn each_convention_reads_its_own_home_variable() {
        let env = fake_env(EVERY_HOME);
        assert_eq!(
            home_dir_for(AppDataConvention::AppleAppSupport, &env),
            PathBuf::from("/home/tester")
        );
        assert_eq!(
            home_dir_for(AppDataConvention::XdgDataHome, &env),
            PathBuf::from("/home/tester")
        );
        assert_eq!(
            home_dir_for(AppDataConvention::WindowsRoamingAppData, &env),
            PathBuf::from(r"C:\Users\tester")
        );
    }

    /// An empty or missing home variable must not produce a path
    /// rooted at `/` — fall back to the working directory instead.
    #[test]
    fn missing_home_falls_back_to_the_working_directory() {
        for convention in [
            AppDataConvention::AppleAppSupport,
            AppDataConvention::WindowsRoamingAppData,
            AppDataConvention::XdgDataHome,
        ] {
            assert_eq!(home_dir_for(convention, fake_env(&[])), PathBuf::from("."));
            assert_eq!(
                home_dir_for(convention, fake_env(&[("HOME", ""), ("USERPROFILE", "")])),
                PathBuf::from(".")
            );
        }
    }

    /// Each convention produces its whole documented path, file name
    /// included. Asserting the full path rather than just
    /// `file_name()` is the point: `join(DB_FILE_NAME).file_name()` is
    /// `DB_FILE_NAME` for *any* receiver, so that weaker form would
    /// pass even if the root collapsed to an empty path.
    #[test]
    fn every_convention_produces_its_documented_path() {
        let expected = [
            (
                AppDataConvention::AppleAppSupport,
                PathBuf::from("/home/tester/Library/Application Support/Vigla/vigla.sqlite"),
            ),
            (
                AppDataConvention::WindowsRoamingAppData,
                PathBuf::from(r"C:\Users\tester\AppData\Roaming").join("Vigla/vigla.sqlite"),
            ),
            (
                AppDataConvention::XdgDataHome,
                PathBuf::from("/srv/state/vigla/vigla.sqlite"),
            ),
        ];
        for (convention, want) in expected {
            let got = app_data_root_for(convention, fake_env(EVERY_HOME)).join(DB_FILE_NAME);
            assert_eq!(got, want, "{convention:?}");
        }
    }

    /// The one assertion that binds a *build target* to a *produced
    /// path*, and the only one that can fail if the wiring in
    /// [`app_data_root`] regresses to a single hardcoded convention.
    ///
    /// Every other test in this group injects the convention by hand,
    /// so all of them would stay green if `app_data_root` went back to
    /// always returning `Library/Application Support` — which is
    /// exactly the defect this split was made to fix, and exactly what
    /// the "Portable Rust (Linux)" CI job must catch.
    #[test]
    fn default_db_path_matches_the_build_target_convention() {
        // `default_db_path` short-circuits on VIGLA_DB_PATH, and other
        // tests in this binary set it. Assert on the composition
        // underneath instead, which no test mutates.
        let path = app_data_root().join(DB_FILE_NAME);
        let shown = path.to_string_lossy();

        assert_eq!(path.file_name().unwrap(), "vigla.sqlite", "{shown}");
        if cfg!(target_os = "macos") {
            assert!(
                shown.contains("Library/Application Support/Vigla"),
                "macOS must keep the historical path, got {shown}"
            );
        } else {
            assert!(
                !shown.contains("Application Support"),
                "non-Apple targets must not produce a macOS path, got {shown}"
            );
            if cfg!(target_os = "windows") {
                assert!(shown.contains("Vigla"), "{shown}");
            } else {
                assert!(
                    shown.contains(".local/share/vigla") || shown.contains("/vigla/"),
                    "expected an XDG data path, got {shown}"
                );
            }
        }
    }

    #[tokio::test]
    async fn spawn_supervised_logs_and_propagates_panic() {
        // R2 regression: a panic inside a `spawn_supervised` body
        // must surface via JoinError::is_panic so callers that
        // await still see it. The tracing::error! log line is
        // verified via the panic-hook install side-effect — the
        // test's stderr capture shows "spawned task panicked"
        // when run with --nocapture; here we assert the contract
        // `is_panic()` so detach-then-log callers still get the
        // log line even without an awaiter.
        install_panic_hook();
        let handle = super::spawn_supervised("test::panic", async {
            panic!("kaboom");
        });
        let err = handle.await.expect_err("must surface panic");
        assert!(err.is_panic(), "expected JoinError::is_panic, got {err:?}");
    }

    #[test]
    fn panic_payload_diagnostic_omits_sensitive_content() {
        let sensitive = String::from("secret-bearing panic detail");

        let label = panic_payload_kind(&sensitive);

        assert_eq!(label, "string");
        assert!(!label.contains("secret-bearing panic detail"));
    }

    #[test]
    fn panic_hook_does_not_chain_to_a_payload_printing_hook() {
        const CHILD_MARKER: &str = "VIGLA_PANIC_HOOK_LEAK_TEST_CHILD";
        const SENSITIVE: &str = "panic-secret-that-must-not-reach-stderr";

        if std::env::var_os(CHILD_MARKER).is_some() {
            std::panic::set_hook(Box::new(|info| {
                if let Some(payload) = info.payload().downcast_ref::<&str>() {
                    eprintln!("previous panic hook: {payload}");
                } else if let Some(payload) = info.payload().downcast_ref::<String>() {
                    eprintln!("previous panic hook: {payload}");
                }
            }));
            install_panic_hook();
            let _ = std::panic::catch_unwind(|| panic!("{SENSITIVE}"));
            return;
        }

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::panic_hook_does_not_chain_to_a_payload_printing_hook",
                "--nocapture",
            ])
            .env(CHILD_MARKER, "1")
            .output()
            .expect("spawn isolated panic-hook test");
        assert!(output.status.success(), "child test failed: {output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        let diagnostics = format!("{}{stderr}", String::from_utf8_lossy(&output.stdout));
        assert!(
            !diagnostics.contains(SENSITIVE),
            "panic payload leaked through the previous hook: {diagnostics}"
        );
        assert!(
            stderr.contains("panic captured") && stderr.contains("payload_kind=string"),
            "payload-free categorical panic diagnostics were missing from stderr: {diagnostics}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn login_shell_timeout_does_not_leave_a_detached_pipe_reader() {
        const CHILD_MARKER: &str = "VIGLA_LOGIN_SHELL_PIPE_TEST_CHILD";
        const RESULT_PATH: &str = "VIGLA_LOGIN_SHELL_PIPE_TEST_RESULT";
        const SESSION_PATH: &str = "VIGLA_LOGIN_SHELL_PIPE_TEST_SESSION";
        const WRITE_STATUS_PATH: &str = "VIGLA_LOGIN_SHELL_PIPE_TEST_WRITE_STATUS";

        if std::env::var_os(CHILD_MARKER).is_some() {
            let session = unsafe { libc::setsid() };
            unsafe {
                libc::signal(libc::SIGHUP, libc::SIG_IGN);
                libc::signal(libc::SIGTERM, libc::SIG_IGN);
            }
            std::fs::write(std::env::var_os(SESSION_PATH).unwrap(), session.to_string()).unwrap();
            std::thread::sleep(Duration::from_millis(2_500));
            let probe = b"reader-probe\n";
            let wrote =
                unsafe { libc::write(libc::STDOUT_FILENO, probe.as_ptr().cast(), probe.len()) };
            std::fs::write(
                std::env::var_os(WRITE_STATUS_PATH).unwrap(),
                wrote.to_string(),
            )
            .unwrap();
            if wrote == probe.len() as isize {
                std::fs::write(std::env::var_os(RESULT_PATH).unwrap(), "reader-still-open")
                    .unwrap();
            }
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let shell = temp.path().join("escaping-shell");
        let result_path = temp.path().join("reader-result");
        let session_path = temp.path().join("session-result");
        let write_status_path = temp.path().join("write-status");
        let current_exe = std::env::current_exe().unwrap();
        write_test_shell(
            &shell,
            &format!(
                "{CHILD_MARKER}=1 {RESULT_PATH}=\"{}\" {SESSION_PATH}=\"{}\" \
                 {WRITE_STATUS_PATH}=\"{}\" \"{}\" --exact \
                 tests::login_shell_timeout_does_not_leave_a_detached_pipe_reader --nocapture\n:",
                result_path.display(),
                session_path.display(),
                write_status_path.display(),
                current_exe.display(),
            ),
        );

        let started = Instant::now();
        let captured = capture_login_shell_path(shell.to_str().unwrap(), Duration::from_secs(2));
        assert!(captured.is_none());
        assert!(started.elapsed() < Duration::from_millis(2_500));
        let Ok(session_raw) = std::fs::read_to_string(&session_path) else {
            // A saturated parallel test run can exhaust the capture timeout
            // before the shell schedules its child at all. The shell process
            // group has been killed and reaped in that case, so there is no
            // escaped writer (or detached reader) left to probe.
            assert!(!write_status_path.exists());
            assert!(!result_path.exists());
            return;
        };
        let session: i32 = session_raw.parse().unwrap();
        assert!(session > 0, "setsid failed with {session}");

        let probe_deadline = Instant::now() + Duration::from_secs(4);
        while !write_status_path.exists() && Instant::now() < probe_deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let wrote: isize = std::fs::read_to_string(&write_status_path)
            .expect("escaped child completed its pipe probe")
            .parse()
            .unwrap();
        assert!(
            wrote < 0 && !result_path.exists(),
            "a detached reader kept the timed-out shell pipe open (write returned {wrote})"
        );
    }
}
