//! P4 — Async startup state container.
//!
//! On boot the Tauri `setup` callback used to `block_on` the full
//! repository / playbook / memory registry / mission controller
//! initialisation sequence. On a fresh install with a schema bump
//! that meant the window did not paint for several hundred
//! milliseconds while sqlx replayed migrations.
//!
//! Now `setup()` returns immediately after registering an uninitialized
//! [`RuntimeHandle`] and the heavy init runs on a background task.
//! Commands extract `State<'_, RuntimeHandle>` and call
//! [`RuntimeHandle::ready`] to retrieve the real state. While the
//! background task is still working, that helper returns a
//! human-readable "initializing" error; the frontend gates IPC by
//! showing an "Initializing Vigla…" splash until it sees the
//! `vigla://startup-complete` event (or polls
//! [`startup_status`](crate::startup_status) and observes `phase=ready`).
//!
//! Origin: backlog brief P4 "async migrations at startup" (resolved
//! 2026-05-21; the brief is preserved in git history).
use std::sync::Arc;
use std::sync::OnceLock;

use orchestrator::memory::MemoryRegistry;
use orchestrator::{MissionController, Repository, Supervisor};

use crate::playbook_store::PlaybookStore;

/// The set of long-lived runtime objects the host installs once
/// migrations + memory + supervisor are ready. Wrapped in `Arc`s
/// where the underlying type is not cheaply `Clone`.
pub struct RuntimeState {
    pub supervisor: Arc<Supervisor>,
    pub repository: Repository,
    pub playbook_store: Arc<PlaybookStore>,
    pub mission_controller: Arc<MissionController>,
    pub memory_registry: Arc<MemoryRegistry>,
}

/// Tauri-managed handle that wraps a `OnceLock<RuntimeState>`. The
/// handle itself is registered synchronously in `setup()` so the
/// State extractor never panics; the contents are populated by the
/// background init task.
#[derive(Default)]
pub struct RuntimeHandle {
    inner: OnceLock<RuntimeState>,
    failure: OnceLock<String>,
}

impl RuntimeHandle {
    pub fn new() -> Self {
        Self {
            inner: OnceLock::new(),
            failure: OnceLock::new(),
        }
    }

    /// Borrow the installed runtime, or return a human-readable
    /// error if init hasn't completed yet. The string is surfaced
    /// directly to the frontend as the command's `Err` value.
    pub fn ready(&self) -> Result<&RuntimeState, String> {
        if let Some(state) = self.inner.get() {
            return Ok(state);
        }
        if let Some(error) = self.failure.get() {
            return Err(format!("vigla runtime failed to initialize: {error}"));
        }
        Err("vigla runtime is still initializing; please retry".to_string())
    }

    /// Install the runtime. Returns `Err` if called twice.
    pub fn install(&self, state: RuntimeState) -> Result<(), &'static str> {
        if self.failure.get().is_some() {
            return Err("vigla runtime initialization already failed");
        }
        self.inner
            .set(state)
            .map_err(|_| "vigla runtime already initialized")
    }

    /// Persist a startup failure so polling clients cannot remain on an
    /// initializing splash when the one-shot event was emitted too early.
    pub fn fail(&self, error: String) -> Result<(), &'static str> {
        if self.inner.get().is_some() {
            return Err("vigla runtime already initialized");
        }
        self.failure
            .set(error)
            .map_err(|_| "vigla runtime failure already recorded")
    }

    pub fn is_ready(&self) -> bool {
        self.inner.get().is_some()
    }

    pub fn failure(&self) -> Option<&str> {
        self.failure.get().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_is_durable_and_ready_reports_it() {
        let handle = RuntimeHandle::new();
        assert!(!handle.is_ready());
        assert!(handle.ready().err().unwrap().contains("still initializing"));

        handle.fail("migration checksum mismatch".into()).unwrap();

        assert_eq!(handle.failure(), Some("migration checksum mismatch"));
        assert!(handle
            .ready()
            .err()
            .unwrap()
            .contains("migration checksum mismatch"));
        assert!(handle.fail("second".into()).is_err());
    }
}
