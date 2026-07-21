//! Failure Recovery Engine.
//!
//! Converts worker-level failures into typed [`FailureClass`] values
//! and routes each class through a single recovery policy that
//! produces a [`RecoveryAction`]. Recovery is the *failure* path of
//! the per-task loop in
//! [`crate::mission_supervisor_run::mission_loop`]; the success path
//! (audit + arbiter) is untouched.
//!
//! Per the supervisor-final-arbiter roadmap §7 S5 the engine covers
//! eight cases: 7 non-trivial failure classes plus vendor quota
//! exhaustion (handled as a *planned pause*, not a failure). Quota
//! state is tracked per-vendor in [`quota::VendorQuotaTracker`]; a
//! tokio wake-up task in [`wakeup`] re-dispatches paused missions
//! at the vendor's reset time.
//!
//! Public entry points:
//! - [`classify_failure`] — `WorkerDispatchError` + observed event
//!   stream → `FailureClass`.
//! - [`recover`] — `FailureClass` + `RecoveryHistory` →
//!   `RecoveryAction`.
//! - [`quota::VendorQuotaTracker`] — host-level state shared across
//!   missions.

pub mod classify;
pub mod history;
pub mod policy;
pub mod quota;
pub mod scope_drift;
pub mod types;
pub mod wakeup;

pub use classify::{classify_failure, ClassifyContext, QuotaSignal};
pub use history::RecoveryHistory;
pub use policy::{recover, RecoveryPolicy};
pub use quota::{default_window_ms, QuotaSignalSource, VendorQuotaState, VendorQuotaTracker};
pub use scope_drift::{drift_to_failure_class, ScopeDriftHeuristic, ScopeDriftVerdict};
pub use types::{
    CommandErrorKind, ContextRequest, ContextRequestKind, FailureClass, PauseReason,
    RecoveryAction, SupervisorRequestKind,
};
pub use wakeup::{spawn_quota_wakeup_task, QuotaWakeupHandle, WakeupEvent};
