//! Types describing recovery decisions. Pure data — no behavior.
//!
//! The eight [`FailureClass`] variants and four [`RecoveryAction`]
//! variants are the closed sets the rest of the recovery module
//! pattern-matches against. Adding a new failure class is a typed
//! refactor: every match arm in [`crate::recovery::policy`] must be
//! updated, and the compiler enforces it.

use event_schema::Vendor;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::arbiter::AuthorityBound;
use crate::arbiter::EscalationEvidence;

// PauseReason is the canonical mission-level pause carrier; recovery
// re-exports it so RecoveryAction::Pause and MissionState::Paused
// agree on the wire and at the type level. The historical duplicate
// in this module diverged manually at run_task.rs's recovery→mission
// bridge — removing the duplicate removes the drift surface.
pub use crate::mission::PauseReason;

/// Closed set of failure shapes the recovery engine recognises.
///
/// Variants cover the 7 non-trivial failure classes from roadmap §7
/// S5 plus the eighth case — [`Self::QuotaExhausted`] — which is
/// treated as a *planned pause* rather than a failure.
///
/// Wire format: externally-tagged (default) — variant name keys the
/// outer object. An internal tag of `"kind"` collides with the
/// in-variant `kind` field (e.g. `CommandError.kind`), so we keep
/// serde's default representation here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// §4.1 — worker referenced a path that does not exist in the
    /// worktree. Today: falls through silently. New behavior: emit a
    /// `RequestContext { path }` directive so the supervisor can
    /// supply the file or escalate as Scope.
    MissingFile { path: PathBuf },
    /// §4.3 — a tool/process invocation exited non-zero.
    /// `kind` distinguishes transient (network / DNS / temp file)
    /// from persistent (unrecognised flag, missing binary).
    CommandError {
        exit_code: i32,
        kind: CommandErrorKind,
    },
    /// §4.4 — the worker's submission cannot be merged because of a
    /// real git conflict. S4 owns rebase / snapshot semantics; S5
    /// only ensures the conflict does not trigger a spurious retry
    /// — it routes to `Escalate(Quality)` if S4's auto-rebase has
    /// already failed.
    MergeConflict { against_ref: String },
    /// §4.5 — `Permission denied` trapped on a file IO. Always
    /// escalates as Risk bound (user co-sign required).
    Permissions { path: PathBuf },
    /// §4.6 — worker explicitly signalled it lacks context (e.g.
    /// emitted `RequestContext { kind: Documentation }`). Not a
    /// hard failure: the worker continues with what it has and the
    /// supervisor catches up async.
    InadequateContext { request: ContextRequest },
    /// §4.7 — touched-files vs declared-scope ratio crossed a
    /// threshold. The worker is still running coherently; the
    /// supervisor sends a "stay in scope" rework directive.
    TaskDrift {
        observed_files: Vec<String>,
        declared_scope: Vec<PathBuf>,
    },
    /// §4.9 — the CLI binary itself crashed (segfault, panic, OOM
    /// kill). Bounded auto-restart (default 2 retries). Escalates
    /// as `Risk` after the budget is exhausted.
    VendorCrash {
        vendor: Vendor,
        last_exit_code: Option<i32>,
        signal: bool,
    },
    /// §2 pause condition — the vendor's quota window closed. Never
    /// a failure: the worker pauses, the mission lifecycle enters
    /// [`crate::mission::MissionState::Paused`], the wake-up task
    /// resumes it at `estimated_reset_at`.
    QuotaExhausted {
        vendor: Vendor,
        /// Unix milliseconds since epoch. Stored as `u64` rather
        /// than `String` so policy comparison is a single integer
        /// compare; the wire format converts to RFC 3339 at the
        /// event boundary.
        estimated_reset_at_ms: u64,
    },
}

/// Distinguishes transient command errors (retryable once) from
/// persistent ones (escalate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandErrorKind {
    /// Network / DNS / temp-file / "another process holds the lock"
    /// — likely to succeed on a single retry.
    Transient,
    /// Unrecognised flag, missing binary, syntax error — retrying is
    /// not going to help.
    Persistent,
}

/// Worker-side informational signal: "I need X to do my job well."
/// Not blocking — the worker continues with what it has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextRequest {
    pub kind: ContextRequestKind,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextRequestKind {
    /// Worker wants to read a specific file's content.
    FileContent,
    /// Worker wants documentation / API reference text.
    Documentation,
    /// Worker wants a prior architectural decision or memory entry.
    PriorDecision,
}

/// What the recovery engine tells `mission_loop.rs` to do next.
///
/// Wire format: externally-tagged (default) — variant name keys the
/// outer object. An internal tag of `"kind"` collides with the
/// in-variant `kind` field on `RequestSupervisor`, so we keep
/// serde's default representation here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    /// Re-dispatch the worker. `attempt` is 1-based for the current
    /// task; `max` is the per-class retry budget.
    Retry { attempt: u8, max: u8 },
    /// Pause the mission until `until_unix_ms` (or sooner if the
    /// quota state changes). Workers on unaffected vendors keep
    /// running.
    Pause {
        until_unix_ms: u64,
        reason: PauseReason,
    },
    /// Halt and surface to the user with the existing four-bound
    /// arbiter channel. Recovery never invents new bounds.
    Escalate {
        bound: AuthorityBound,
        evidence: EscalationEvidence,
    },
    /// Send the supervisor an informational signal that does NOT
    /// block worker progress. The supervisor catches up async on
    /// its next decision turn (the worker is still running).
    RequestSupervisor { kind: SupervisorRequestKind },
}

/// Informational supervisor messages emitted by recovery — never
/// blocking. Worker keeps running; supervisor consumes async.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SupervisorRequestKind {
    /// Worker wants context. The supervisor either supplies it from
    /// memory or escalates as Scope.
    NeedsContext { request: ContextRequest },
    /// Drift detector tripped on the worker's touched-files set.
    /// Supervisor sends a "stay in scope" rework directive on the
    /// next turn.
    DriftDetected {
        observed_files: Vec<String>,
        declared_scope: Vec<PathBuf>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vendor_claude() -> Vendor {
        Vendor::Claude
    }

    #[test]
    fn failure_class_missing_file_round_trip() {
        let f = FailureClass::MissingFile {
            path: PathBuf::from("src/lib.rs"),
        };
        let s = serde_json::to_string(&f).unwrap();
        let back: FailureClass = serde_json::from_str(&s).unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn failure_class_command_error_round_trip() {
        let f = FailureClass::CommandError {
            exit_code: 7,
            kind: CommandErrorKind::Transient,
        };
        let s = serde_json::to_string(&f).unwrap();
        let back: FailureClass = serde_json::from_str(&s).unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn failure_class_quota_exhausted_round_trip() {
        let f = FailureClass::QuotaExhausted {
            vendor: vendor_claude(),
            estimated_reset_at_ms: 1_716_000_000_000,
        };
        let s = serde_json::to_string(&f).unwrap();
        let back: FailureClass = serde_json::from_str(&s).unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn recovery_action_retry_round_trip() {
        let a = RecoveryAction::Retry { attempt: 1, max: 2 };
        let s = serde_json::to_string(&a).unwrap();
        let back: RecoveryAction = serde_json::from_str(&s).unwrap();
        assert_eq!(a, back);
    }

    #[test]
    fn recovery_action_pause_round_trip() {
        let a = RecoveryAction::Pause {
            until_unix_ms: 1_716_000_000_000,
            reason: PauseReason::WaitingForQuota {
                vendor: vendor_claude(),
            },
        };
        let s = serde_json::to_string(&a).unwrap();
        let back: RecoveryAction = serde_json::from_str(&s).unwrap();
        assert_eq!(a, back);
    }

    #[test]
    fn recovery_action_escalate_round_trip() {
        let a = RecoveryAction::Escalate {
            bound: AuthorityBound::Risk,
            evidence: EscalationEvidence {
                summary: "permission denied on /etc/passwd".into(),
                payload_json: None,
            },
        };
        let s = serde_json::to_string(&a).unwrap();
        let back: RecoveryAction = serde_json::from_str(&s).unwrap();
        assert_eq!(a, back);
    }

    #[test]
    fn recovery_action_request_supervisor_round_trip() {
        let a = RecoveryAction::RequestSupervisor {
            kind: SupervisorRequestKind::NeedsContext {
                request: ContextRequest {
                    kind: ContextRequestKind::Documentation,
                    detail: "rust async-trait conventions".into(),
                },
            },
        };
        let s = serde_json::to_string(&a).unwrap();
        let back: RecoveryAction = serde_json::from_str(&s).unwrap();
        assert_eq!(a, back);
    }
}
