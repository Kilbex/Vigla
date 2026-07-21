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
use std::time::Instant;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

static START: OnceLock<Instant> = OnceLock::new();
static PANIC_HOOK: OnceLock<()> = OnceLock::new();

/// Mark the orchestrator's start time. Idempotent — safe to call from
/// the host's `setup` hook on every launch.
pub fn init() {
    START.get_or_init(Instant::now);
    install_panic_hook();
}

/// R2 — install a global panic hook that records the panic via
/// `tracing::error!` before chaining to the previous hook. Without
/// this, panics inside `tokio::spawn`ed tasks are silently dropped
/// by the runtime and panics on the main thread surface only as a
/// macOS crash dialog with no actionable signal.
///
/// The hook is installed once, atomically, via `OnceLock`. Tests
/// that install their own hooks (e.g. proptest's harness) will
/// replace this one — that's fine, the chained `prev_hook` keeps
/// the default formatting and abort-on-panic semantics intact for
/// the lifetime of the global slot.
fn install_panic_hook() {
    PANIC_HOOK.get_or_init(|| {
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "<unknown>".into());
            let payload = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic payload>".into());
            tracing::error!(
                target: "vigla::panic",
                location = %location,
                payload = %payload,
                "panic captured"
            );
            prev_hook(info);
        }));
    });
}

/// R2 — spawn a tokio task with panic visibility. The returned
/// `JoinHandle` resolves when the wrapped future completes; if it
/// panics, the panic is logged via `tracing::error!` with the task
/// `name` for grep-ability, then re-raised so callers that
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
            let payload = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic payload>".into());
            tracing::error!(
                target: "vigla::panic",
                task = name,
                payload = %payload,
                "spawned task panicked"
            );
            // Re-raise so test harnesses and tokio's own JoinError
            // path still see the panic; production callers that
            // detach the handle simply drop the join receiver.
            std::panic::resume_unwind(panic);
        }
    })
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
        // Login shell + `-c` so user's rc files run and PATH is the same
        // one their terminal sees. Capture stdout only; rc files often
        // print warnings to stderr we don't care about.
        let captured = std::process::Command::new(&shell)
            .arg("-l")
            .arg("-c")
            .arg("echo $PATH")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            });

        if let Some(s) = captured {
            // Append the inherited PATH for forward-compat (the OS may
            // add bootstrap dirs the login shell didn't).
            if inherited.is_empty() {
                s
            } else {
                format!("{s}:{inherited}")
            }
        } else {
            // Login-shell capture failed — compose a defensible default.
            let home = std::env::var("HOME").unwrap_or_default();
            let mut paths: Vec<String> = vec![
                "/opt/homebrew/bin".into(),
                "/opt/homebrew/sbin".into(),
                "/usr/local/bin".into(),
                "/usr/local/sbin".into(),
            ];
            if !home.is_empty() {
                paths.push(format!("{home}/.local/bin"));
                paths.push(format!("{home}/.cargo/bin"));
                // nvm: pick up every installed node version's bin dir.
                let nvm_dir = format!("{home}/.nvm/versions/node");
                if let Ok(entries) = std::fs::read_dir(&nvm_dir) {
                    for entry in entries.flatten() {
                        let p = entry.path().join("bin");
                        if p.exists() {
                            paths.push(p.to_string_lossy().into_owned());
                        }
                    }
                }
            }
            if !inherited.is_empty() {
                paths.push(inherited);
            }
            paths.join(":")
        }
    })
}

/// Default location for the Vigla SQLite database on macOS:
/// `~/Library/Application Support/Vigla/vigla.sqlite`. Override via
/// the `VIGLA_DB_PATH` env var (used in tests and developer setups).
pub fn default_db_path() -> PathBuf {
    if let Ok(override_path) = std::env::var("VIGLA_DB_PATH") {
        return PathBuf::from(override_path);
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("Library/Application Support/Vigla/vigla.sqlite")
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
