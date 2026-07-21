//! Worker supervision: spawn a child process, plumb its stdout
//! through the parser into the repository and the frontend, manage
//! lifecycle (cancellation, exit detection), and keep a per-worker
//! handle so the host can stop in flight.
//!
//! Supports spawn/stop, dependency-aware dispatch, bounded retry, and
//! resumable sessions where the vendor adapter exposes a session id.

mod adapter_supervision;
pub mod coalescing;
pub mod coordination;
mod dispatch;
mod real_workers;
mod resume;

use crate::error::RepositoryError;
use crate::ids::new_task_id;
use crate::parser::WorkerEventSink;
use crate::repository::Repository;
use coordination::CoordinatingSink;
use event_schema::Vendor;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{oneshot, watch, Mutex};
use tokio::task::JoinHandle;

/// Error type for the supervisor surface.
#[derive(Debug, Error)]
pub enum SupervisorError {
    #[error("unknown script {0:?}; expected one of: claude_happy, codex_blocked, gemini_happy, gemini_blocked, gemini_failed, gemini_terminal")]
    UnknownScript(String),

    #[error("mock-harness binary not found at {0}")]
    MockHarnessMissing(PathBuf),

    #[error("worker not found: {0}")]
    WorkerNotFound(String),

    #[error("worker is still running; cannot resume until it completes")]
    WorkerStillRunning,

    #[error("resume not supported for {0:?}")]
    ResumeUnsupported(Vendor),

    #[error("worker has no session id (check vendor capabilities)")]
    SessionIdMissing,

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("repository: {0}")]
    Repository(#[from] RepositoryError),
}

/// Inputs for the simple "run this mock script now" call site.
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub script: String,
    pub speed: f64,
    pub task_title: String,
}

impl SpawnRequest {
    pub fn realtime(script: impl Into<String>) -> Self {
        let script = script.into();
        let task_title = default_task_title(&script);
        Self {
            script,
            speed: 1.0,
            task_title,
        }
    }
}

/// Inputs for dependency-aware [`Supervisor::dispatch`].
#[derive(Debug, Clone)]
pub struct DispatchRequest {
    pub script: String,
    pub speed: f64,
    pub task_title: String,
    /// Pre-allocated task id. Use [`new_task_id()`] if the caller
    /// doesn't have one. Setting it explicitly lets a caller declare
    /// downstream dependencies before the upstream task spawns.
    pub task_id: String,
    /// Other tasks that must reach `done` (or any
    /// [`Repository::insert_event`] of type `completion`) before this
    /// task is allowed to spawn.
    pub depends_on: Vec<String>,
    pub retry: RetryPolicy,
}

impl DispatchRequest {
    /// Convenience: bare "run this script, no deps, no retry" request.
    pub fn from_script(script: impl Into<String>) -> Self {
        let script = script.into();
        Self {
            script: script.clone(),
            speed: 1.0,
            task_title: default_task_title(&script),
            task_id: new_task_id(),
            depends_on: Vec::new(),
            retry: RetryPolicy::Never,
        }
    }
}

/// Retry policy attached to a dispatched task. Immediate spawn paths default
/// to `Never`; coordination dispatches can opt in.
#[derive(Debug, Clone, Copy)]
pub enum RetryPolicy {
    /// One attempt; failure is terminal.
    Never,
    /// Up to `max_attempts` total attempts on retryable failures, with
    /// exponential backoff `base_ms · 2^(attempt-1)`.
    OnFailure { max_attempts: u32, base_ms: u64 },
}

fn default_task_title(script: &str) -> String {
    match script {
        "claude_happy" => "Add retry to fetcher".into(),
        "codex_blocked" => "Apply schema migration".into(),
        "gemini_happy" => "Refactor auth middleware".into(),
        "gemini_blocked" => "Update API docs after spec".into(),
        "gemini_failed" => "Add expired-token test".into(),
        "gemini_terminal" => "Audit r5 retry gate".into(),
        other => format!("Mock task ({other})"),
    }
}

fn vendor_for_script(script: &str) -> Result<Vendor, SupervisorError> {
    match script {
        "claude_happy" => Ok(Vendor::Claude),
        "codex_blocked" => Ok(Vendor::Codex),
        "gemini_happy" | "gemini_blocked" | "gemini_failed" | "gemini_terminal" => {
            Ok(Vendor::Gemini)
        }
        other => Err(SupervisorError::UnknownScript(other.to_owned())),
    }
}

struct RunningWorker {
    cancel: Option<oneshot::Sender<()>>,
    /// `Option` so [`Supervisor::stop`] can take the JoinHandle out via
    /// `Option::take` without removing the entry from the workers map —
    /// keeps `is_running()` truthful through the cancel + drain window.
    /// The supervise task itself removes the entry once cleanup is done.
    join: Option<JoinHandle<()>>,
    generation: u64,
    phase: watch::Sender<WorkerSlotPhase>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerSlotPhase {
    Preparing,
    Running,
    Failed,
}

struct WorkerReservation {
    generation: u64,
    cancel_rx: oneshot::Receiver<()>,
}

struct PendingDispatch {
    request: DispatchRequest,
    /// Attempt number for this *task* (not for the whole pending
    /// queue). 1 = first attempt; ≥2 = scheduled retry of an earlier
    /// failed attempt of the same task_id.
    attempt: u32,
}

pub struct Supervisor {
    repo: Repository,
    sink: Arc<dyn WorkerEventSink>,
    mock_harness: PathBuf,
    workers: Mutex<HashMap<String, RunningWorker>>,
    name_counter: AtomicU32,
    reservation_counter: AtomicU64,

    /// Task IDs whose worker has reached `done` (or has emitted a
    /// `completion` event). Drives downstream dispatch.
    completed_tasks: Mutex<HashSet<String>>,
    /// Task IDs that have exhausted retries (or are not retryable).
    /// Downstream tasks blocked on a failed task stay blocked.
    failed_tasks: Mutex<HashSet<String>>,
    /// Tasks whose deps are not yet satisfied. Drained by
    /// [`Supervisor::on_task_completed`].
    pending: Mutex<Vec<PendingDispatch>>,
    /// task_id → number of attempts made so far.
    attempts: Mutex<HashMap<String, u32>>,
    /// Number of retry tasks scheduled but not yet spawned. `is_quiescent`
    /// stays false while > 0 so tests can wait for in-flight backoff
    /// before asserting drain.
    pending_retries: AtomicU32,
    /// Step 25 — worker_id → session_id for ongoing or recently-completed
    /// sessions. Populated by the supervision loop when adapters emit
    /// session_id via `take_session_id()`.
    session_ids: Mutex<HashMap<String, String>>,
}

impl Supervisor {
    /// Build a Supervisor wrapped in `Arc`. `user_sink` is the raw
    /// frontend-facing sink; every supervise path wraps it in a
    /// [`CoordinatingSink`] at run time via [`Self::coordinating_sink`]
    /// so the supervisor sees the same events the frontend sees.
    pub fn new(
        repo: Repository,
        user_sink: Arc<dyn WorkerEventSink>,
        mock_harness: PathBuf,
    ) -> Arc<Self> {
        Arc::new(Self {
            repo,
            sink: user_sink,
            mock_harness,
            workers: Mutex::new(HashMap::new()),
            name_counter: AtomicU32::new(0),
            reservation_counter: AtomicU64::new(0),
            completed_tasks: Mutex::new(HashSet::new()),
            failed_tasks: Mutex::new(HashSet::new()),
            pending: Mutex::new(Vec::new()),
            attempts: Mutex::new(HashMap::new()),
            pending_retries: AtomicU32::new(0),
            session_ids: Mutex::new(HashMap::new()),
        })
    }

    async fn reserve_worker_slot(
        &self,
        worker_id: &str,
    ) -> Result<WorkerReservation, SupervisorError> {
        let mut workers = self.workers.lock().await;
        if workers.contains_key(worker_id) {
            return Err(SupervisorError::WorkerStillRunning);
        }
        let generation = self.reservation_counter.fetch_add(1, Ordering::Relaxed) + 1;
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (phase, _) = watch::channel(WorkerSlotPhase::Preparing);
        workers.insert(
            worker_id.to_owned(),
            RunningWorker {
                cancel: Some(cancel_tx),
                join: None,
                generation,
                phase,
            },
        );
        Ok(WorkerReservation {
            generation,
            cancel_rx,
        })
    }

    async fn activate_worker_slot(
        &self,
        worker_id: &str,
        generation: u64,
        join: JoinHandle<()>,
        start: oneshot::Sender<()>,
    ) -> Result<(), SupervisorError> {
        let mut workers = self.workers.lock().await;
        let Some(entry) = workers.get_mut(worker_id) else {
            join.abort();
            return Err(SupervisorError::WorkerNotFound(worker_id.to_owned()));
        };
        if entry.generation != generation {
            join.abort();
            return Err(SupervisorError::WorkerStillRunning);
        }
        entry.join = Some(join);
        entry.phase.send_replace(WorkerSlotPhase::Running);
        let _ = start.send(());
        Ok(())
    }

    async fn fail_worker_slot(&self, worker_id: &str, generation: u64) {
        let mut workers = self.workers.lock().await;
        if workers
            .get(worker_id)
            .is_some_and(|entry| entry.generation == generation)
        {
            if let Some(entry) = workers.get(worker_id) {
                entry.phase.send_replace(WorkerSlotPhase::Failed);
            }
            workers.remove(worker_id);
        }
    }

    /// Wrap `self.sink` in a [`CoalescingSink`] (UI/IPC throughput
    /// shaping) and then in a [`CoordinatingSink`] (supervisor
    /// coordination side effects). Both `supervise` (mock path) and
    /// `supervise_with_adapter` (real-CLI path) MUST call this — the
    /// CoordinatingSink wrap is required so downstream tasks unblock,
    /// the CoalescingSink wrap is required so a chatty worker can't
    /// flood the IPC bus.
    fn coordinating_sink(self: &Arc<Self>) -> Arc<dyn WorkerEventSink> {
        let coalesced: Arc<dyn WorkerEventSink> =
            Arc::new(coalescing::CoalescingSink::new(Arc::clone(&self.sink)));
        Arc::new(CoordinatingSink::new(Arc::downgrade(self), coalesced))
    }

    /// Test-only public accessor for the coordination sink. The
    /// production `coordinating_sink` is private — only used inside
    /// the supervisor module. Integration tests need a way to construct
    /// one. Marked `#[doc(hidden)]` so it isn't surfaced in the
    /// crate's public docs.
    #[doc(hidden)]
    pub fn coordinating_sink_for_test(self: &Arc<Self>) -> Arc<coordination::CoordinatingSink> {
        Arc::new(coordination::CoordinatingSink::new(
            Arc::downgrade(self),
            Arc::clone(&self.sink),
        ))
    }

    /// Test-only — full production sink composition
    /// (CoordinatingSink wrapping CoalescingSink wrapping the user
    /// sink). Used by the storm test to verify rate-limiting at the
    /// composed-pipeline level. The plain
    /// `coordinating_sink_for_test` skips the CoalescingSink layer
    /// so layer-1 tests (Task 1) stay pure.
    #[doc(hidden)]
    pub fn coordinating_sink_for_test_full(self: &Arc<Self>) -> Arc<dyn WorkerEventSink> {
        self.coordinating_sink()
    }

    /// Test-only access to the underlying repository for fixture
    /// setup (inserting workers/tasks before exercising the sink).
    #[doc(hidden)]
    pub fn repo_for_test(&self) -> &Repository {
        &self.repo
    }

    /// Locate the mock-harness binary. Resolution order:
    /// 1. `VIGLA_MOCK_HARNESS` env var.
    /// 2. Sibling of the host binary (dev mode:
    ///    `target/{debug,release}/mock-harness` next to `vigla-host`).
    /// 3. macOS bundled `.app/Contents/Resources/mock-harness` (set up
    ///    by `bundle.resources` in `app/src-tauri/tauri.conf.json`).
    ///
    /// The bundled-Resources path matters because the .app's host
    /// binary lives at `Contents/MacOS/vigla-host`, which means
    /// step 2 would resolve to `Contents/MacOS/mock-harness` — a path
    /// that doesn't exist in a standard tauri bundle. Without step 3
    /// the packaged app cannot spawn mock workers, breaking the
    /// mock-first first-open experience.
    pub fn locate_mock_harness() -> Result<PathBuf, SupervisorError> {
        if let Ok(p) = std::env::var("VIGLA_MOCK_HARNESS") {
            let p = PathBuf::from(p);
            if p.exists() {
                return Ok(p);
            }
            return Err(SupervisorError::MockHarnessMissing(p));
        }
        let exe = std::env::current_exe()?;
        locate_mock_harness_from_exe(&exe)
    }
}

/// Pure function form of [`Supervisor::locate_mock_harness`] taking the
/// host executable path as input — split out so the macOS .app
/// resolution logic is testable without faking `current_exe()`.
fn locate_mock_harness_from_exe(exe: &Path) -> Result<PathBuf, SupervisorError> {
    let parent = exe
        .parent()
        .ok_or_else(|| SupervisorError::MockHarnessMissing(PathBuf::from("<no exe parent>")))?;

    // Dev / target build: mock-harness sits next to vigla-host.
    let sibling = parent.join("mock-harness");
    if sibling.exists() {
        return Ok(sibling);
    }

    // Bundled macOS .app: parent is Contents/MacOS, so the resources
    // dir is ../Resources relative to the host binary.
    if let Some(grandparent) = parent.parent() {
        let in_resources = grandparent.join("Resources").join("mock-harness");
        if in_resources.exists() {
            return Ok(in_resources);
        }
    }

    Err(SupervisorError::MockHarnessMissing(sibling))
}

fn vendor_short_name(v: Vendor) -> &'static str {
    match v {
        Vendor::Claude => "claude",
        Vendor::Codex => "codex",
        Vendor::Gemini => "gemini",
        Vendor::Antigravity => "antigravity",
        Vendor::Kiro => "kiro",
        Vendor::Copilot => "copilot",
        Vendor::Opencode => "opencode",
        Vendor::Mock => "mock",
    }
}

#[cfg(test)]
mod tests;
