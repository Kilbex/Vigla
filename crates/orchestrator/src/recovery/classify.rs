//! Pure mapper: failure surface → [`FailureClass`].
//!
//! Inputs:
//! - The [`WorkerDispatchError`] returned by
//!   [`crate::mission_worker_dispatch::run_real_worker`].
//! - The drained set of typed signals observed during the run
//!   (quota signals from adapters; context requests). These take
//!   priority over the raw dispatch error.
//! - The [`ClassifyContext`] carrying mission-level data needed for
//!   disambiguation (e.g. the configured scope_paths for drift).
//!
//! Output: a single [`FailureClass`]. The mapper is total — every
//! input combination yields a class. Unknown cases collapse onto
//! `CommandError { kind: Persistent }`, which the policy escalates.

use std::path::PathBuf;

use event_schema::Vendor;

use crate::mission_worker_dispatch::WorkerDispatchError;
use crate::recovery::types::{CommandErrorKind, ContextRequest, FailureClass};

/// Per-pass context the classifier needs. Built by `mission_loop`
/// from the worker's task descriptor and the mission spec.
#[derive(Debug, Clone)]
pub struct ClassifyContext {
    /// Vendor running the failing worker. Used to construct
    /// `VendorCrash` and `QuotaExhausted` variants.
    pub vendor: Vendor,
    /// Files the worker touched before the failure (best-effort —
    /// may be empty if the worker died before producing any output).
    pub touched_files: Vec<String>,
    /// Declared scope from the mission spec.
    pub declared_scope: Vec<PathBuf>,
    /// Quota signals drained from the worker stream during this
    /// pass. Non-empty → quota wins regardless of dispatch error.
    pub quota_signals: Vec<QuotaSignal>,
    /// Context requests drained from the worker stream during this
    /// pass.
    pub context_requests: Vec<ContextRequest>,
}

/// Quota signal as drained from an adapter. The adapter has already
/// parsed the vendor-specific format; the supervisor sees only this
/// canonical shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaSignal {
    pub vendor: Vendor,
    /// Adapter's best estimate. `None` means "fall back to the
    /// vendor's configured default window".
    pub estimated_reset_at_ms: Option<u64>,
}

/// Total mapping. Quota signals take precedence; otherwise context
/// requests; otherwise the raw dispatch error.
pub fn classify_failure(
    error: Option<&WorkerDispatchError>,
    ctx: &ClassifyContext,
    fallback_window_ms: u64,
    now_unix_ms: u64,
) -> FailureClass {
    if let Some(sig) = ctx.quota_signals.first() {
        let reset = sig
            .estimated_reset_at_ms
            .unwrap_or_else(|| now_unix_ms.saturating_add(fallback_window_ms));
        return FailureClass::QuotaExhausted {
            vendor: sig.vendor,
            estimated_reset_at_ms: reset,
        };
    }

    if let Some(req) = ctx.context_requests.first() {
        return FailureClass::InadequateContext {
            request: req.clone(),
        };
    }

    let Some(err) = error else {
        // No dispatch error AND no signals AND no quota — should
        // not happen at the classifier's call site (it is only
        // entered on failure paths). Collapse onto a Persistent
        // command error so the policy escalates rather than spinning.
        return FailureClass::CommandError {
            exit_code: -1,
            kind: CommandErrorKind::Persistent,
        };
    };

    match err {
        WorkerDispatchError::Spawn(msg) => classify_spawn(msg, ctx.vendor),
        WorkerDispatchError::Exit(msg) => classify_exit(msg, ctx.vendor),
        WorkerDispatchError::Timeout(_) => FailureClass::VendorCrash {
            vendor: ctx.vendor,
            last_exit_code: None,
            signal: true,
        },
        WorkerDispatchError::Git(msg) => classify_git(msg),
        WorkerDispatchError::Io(msg) => classify_io(msg),
        WorkerDispatchError::Cancelled => FailureClass::VendorCrash {
            vendor: ctx.vendor,
            last_exit_code: None,
            signal: true,
        },
    }
}

fn classify_spawn(msg: &str, vendor: Vendor) -> FailureClass {
    // Spawn errors that mention permission/access are Permissions
    // (the binary itself wasn't executable, or the cwd was locked).
    if msg.contains("Permission denied") || msg.contains("permission denied") {
        return FailureClass::Permissions {
            path: PathBuf::from(extract_path(msg).unwrap_or_default()),
        };
    }
    if msg.contains("No such file") || msg.contains("not found") {
        // Vendor binary itself missing → vendor crash (the orchestra
        // will surface "claude not found" as a Risk-bound escalation
        // because retry won't help and the user needs to install).
        return FailureClass::VendorCrash {
            vendor,
            last_exit_code: None,
            signal: false,
        };
    }
    FailureClass::CommandError {
        exit_code: -1,
        kind: CommandErrorKind::Persistent,
    }
}

fn classify_exit(msg: &str, vendor: Vendor) -> FailureClass {
    // The Exit string the dispatcher builds includes the underlying
    // error text. SIGSEGV / SIGKILL / SIGABRT → vendor crash.
    let signal = msg.contains("signal")
        || msg.contains("SIGSEGV")
        || msg.contains("SIGKILL")
        || msg.contains("SIGABRT")
        || msg.contains("killed");
    let exit_code = extract_exit_code(msg);
    if signal {
        return FailureClass::VendorCrash {
            vendor,
            last_exit_code: exit_code,
            signal: true,
        };
    }
    if let Some(code) = exit_code {
        if is_transient_exit_code(code) {
            return FailureClass::CommandError {
                exit_code: code,
                kind: CommandErrorKind::Transient,
            };
        }
        return FailureClass::CommandError {
            exit_code: code,
            kind: CommandErrorKind::Persistent,
        };
    }
    FailureClass::VendorCrash {
        vendor,
        last_exit_code: None,
        signal: false,
    }
}

fn classify_git(msg: &str) -> FailureClass {
    if msg.contains("CONFLICT") || msg.contains("Merge conflict") {
        return FailureClass::MergeConflict {
            against_ref: "HEAD".into(),
        };
    }
    if msg.contains("Permission denied") {
        return FailureClass::Permissions {
            path: PathBuf::from(extract_path(msg).unwrap_or_default()),
        };
    }
    FailureClass::CommandError {
        exit_code: 128,
        kind: CommandErrorKind::Persistent,
    }
}

fn classify_io(msg: &str) -> FailureClass {
    if msg.contains("Permission denied") || msg.contains("EACCES") {
        return FailureClass::Permissions {
            path: PathBuf::from(extract_path(msg).unwrap_or_default()),
        };
    }
    if msg.contains("No such file or directory") || msg.contains("ENOENT") {
        return FailureClass::MissingFile {
            path: PathBuf::from(extract_path(msg).unwrap_or_default()),
        };
    }
    if is_transient_io_phrase(msg) {
        return FailureClass::CommandError {
            exit_code: 0,
            kind: CommandErrorKind::Transient,
        };
    }
    FailureClass::CommandError {
        exit_code: 0,
        kind: CommandErrorKind::Persistent,
    }
}

fn is_transient_exit_code(code: i32) -> bool {
    // 124 = GNU timeout binary's "killed" code; 75 = EX_TEMPFAIL.
    // 1 alone is too generic to call transient. 7 / 6 are curl's
    // network exit codes; cargo's network errors often produce 101
    // but with a "Resolving" / "Failed to resolve" string — those
    // are caught in the message-based path, not here.
    matches!(code, 75 | 124)
}

fn is_transient_io_phrase(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("connection reset")
        || lower.contains("temporarily unavailable")
        || lower.contains("network is unreachable")
        || lower.contains("dns resolution")
        || lower.contains("timeout")
}

fn extract_exit_code(msg: &str) -> Option<i32> {
    // Dispatcher error strings include "code N" or "exited with
    // code N" or "(exit N)".
    for needle in ["code ", "exit ", "(exit "] {
        if let Some(idx) = msg.find(needle) {
            let tail = &msg[idx + needle.len()..];
            let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse::<i32>() {
                return Some(n);
            }
        }
    }
    None
}

fn extract_path(msg: &str) -> Option<String> {
    // Best-effort: look for a quoted path. Dispatcher Io strings
    // include the path via std::io::Error's Display.
    if let Some(start) = msg.find('"') {
        if let Some(end) = msg[start + 1..].find('"') {
            return Some(msg[start + 1..start + 1 + end].to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::types::ContextRequestKind;
    use std::time::Duration;

    fn ctx(vendor: Vendor) -> ClassifyContext {
        ClassifyContext {
            vendor,
            touched_files: vec![],
            declared_scope: vec![],
            quota_signals: vec![],
            context_requests: vec![],
        }
    }

    #[test]
    fn quota_signal_beats_dispatch_error() {
        let mut c = ctx(Vendor::Claude);
        c.quota_signals.push(QuotaSignal {
            vendor: Vendor::Claude,
            estimated_reset_at_ms: Some(2_000),
        });
        let err = WorkerDispatchError::Exit("died with code 1".into());
        let class = classify_failure(Some(&err), &c, 5 * 3600 * 1000, 1_000);
        assert_eq!(
            class,
            FailureClass::QuotaExhausted {
                vendor: Vendor::Claude,
                estimated_reset_at_ms: 2_000,
            }
        );
    }

    #[test]
    fn quota_signal_without_reset_falls_back_to_window() {
        let mut c = ctx(Vendor::Claude);
        c.quota_signals.push(QuotaSignal {
            vendor: Vendor::Claude,
            estimated_reset_at_ms: None,
        });
        let class = classify_failure(None, &c, 5 * 3600 * 1000, 1_000);
        assert_eq!(
            class,
            FailureClass::QuotaExhausted {
                vendor: Vendor::Claude,
                estimated_reset_at_ms: 1_000 + 5 * 3600 * 1000,
            }
        );
    }

    #[test]
    fn context_request_classifies_inadequate_context() {
        let mut c = ctx(Vendor::Codex);
        let req = ContextRequest {
            kind: ContextRequestKind::Documentation,
            detail: "rust async-trait".into(),
        };
        c.context_requests.push(req.clone());
        let class = classify_failure(None, &c, 0, 0);
        assert_eq!(class, FailureClass::InadequateContext { request: req });
    }

    #[test]
    fn timeout_dispatch_error_is_vendor_crash_signal() {
        let c = ctx(Vendor::Gemini);
        let err = WorkerDispatchError::Timeout(Duration::from_secs(900));
        let class = classify_failure(Some(&err), &c, 0, 0);
        assert_eq!(
            class,
            FailureClass::VendorCrash {
                vendor: Vendor::Gemini,
                last_exit_code: None,
                signal: true,
            }
        );
    }

    #[test]
    fn exit_with_sigsegv_is_vendor_crash() {
        let c = ctx(Vendor::Claude);
        let err = WorkerDispatchError::Exit("died with signal SIGSEGV".into());
        let class = classify_failure(Some(&err), &c, 0, 0);
        match class {
            FailureClass::VendorCrash { signal, .. } => assert!(signal),
            other => panic!("expected VendorCrash, got {other:?}"),
        }
    }

    #[test]
    fn exit_code_75_is_transient_command_error() {
        let c = ctx(Vendor::Codex);
        let err = WorkerDispatchError::Exit("exited with code 75".into());
        let class = classify_failure(Some(&err), &c, 0, 0);
        assert_eq!(
            class,
            FailureClass::CommandError {
                exit_code: 75,
                kind: CommandErrorKind::Transient,
            }
        );
    }

    #[test]
    fn io_eacces_is_permissions() {
        let c = ctx(Vendor::Claude);
        let err = WorkerDispatchError::Io("EACCES on \"/etc/passwd\"".into());
        let class = classify_failure(Some(&err), &c, 0, 0);
        assert_eq!(
            class,
            FailureClass::Permissions {
                path: PathBuf::from("/etc/passwd"),
            }
        );
    }

    #[test]
    fn io_enoent_is_missing_file() {
        let c = ctx(Vendor::Claude);
        let err = WorkerDispatchError::Io("ENOENT \"src/lib.rs\"".into());
        let class = classify_failure(Some(&err), &c, 0, 0);
        assert_eq!(
            class,
            FailureClass::MissingFile {
                path: PathBuf::from("src/lib.rs"),
            }
        );
    }

    #[test]
    fn git_conflict_marker_is_merge_conflict() {
        let c = ctx(Vendor::Claude);
        let err =
            WorkerDispatchError::Git("CONFLICT (content): Merge conflict in src/lib.rs".into());
        let class = classify_failure(Some(&err), &c, 0, 0);
        assert_eq!(
            class,
            FailureClass::MergeConflict {
                against_ref: "HEAD".into(),
            }
        );
    }

    #[test]
    fn no_error_no_signal_collapses_to_persistent_command_error() {
        let c = ctx(Vendor::Claude);
        let class = classify_failure(None, &c, 0, 0);
        assert!(matches!(
            class,
            FailureClass::CommandError {
                kind: CommandErrorKind::Persistent,
                ..
            }
        ));
    }
}
