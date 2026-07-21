use crate::ids::new_task_id;
use crate::memory::{KernelIntentSink, MemoryIntentSink, MemoryKernel};
use crate::mission::MissionSpec;
use crate::mission_event::{MissionEventKind, TaskDescriptor};
use crate::mission_runtime::CancelToken;
use crate::mission_runtime::MissionEventBus;
use crate::mission_runtime::MissionRuntimeError;
use crate::mission_worker_dispatch::{
    run_real_worker_controlled, WorkerEventStream, WorkerSubmission, WorkerVendor,
    DEFAULT_WORKER_TIMEOUT,
};
use crate::mock_worker::{MockWorkerKind, MockWorkerVariant};
use crate::parser::WorkerEventSink;
use crate::vendor_profile::{profile_for_vendor, DeclaredSideEffectKind};
use adapter_core::Adapter;
use antigravity_adapter::AntigravityAdapter;
use claude_adapter::ClaudeAdapter;
use codex_adapter::CodexAdapter;
use copilot_adapter::CopilotAdapter;
use event_schema::{Event, EventKind, LogLevel, LogStream};
use gemini_adapter::GeminiAdapter;
use kiro_adapter::KiroAdapter;
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use tokio::process::Command;

/// Which worker implementation a mission uses. Derived from
/// `MissionSpec.worker_model` by [`select_worker_backend`].
///
/// `Mock` keeps the deterministic demo/test path. `AutoReal` means
/// "pick a real CLI for each task role". `RealCli(_)` pins every
/// task to a concrete vendor. `Roster(_)` pins each task index to
/// an independently selected worker CLI, cycling if follow-up split
/// tasks outnumber the original roster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerBackend {
    Mock,
    /// L1-only deterministic quota exercise. The first pass emits a
    /// Claude adapter quota signal, then the resumed pass falls through
    /// to the regular mock worker so the mission can complete.
    L1ClaudeQuotaExhausted,
    AutoReal,
    RealCli(WorkerVendor),
    Roster(WorkerRoster),
}

pub const L1_CLAUDE_QUOTA_EXHAUSTED_WORKER_MODEL: &str = "claude_quota_exhausted";
const DEFAULT_AUTO_WORKER_VENDOR: WorkerVendor = WorkerVendor::Claude;
const MAX_WORKER_ROSTER_LEN: usize = 10;
const L1_QUOTA_MARKER_PATH: &str = ".vigla/l1-claude-quota-exhausted.seen";
const DEFAULT_L1_QUOTA_RESET_MS: u64 = 90_000;
const MIN_L1_QUOTA_RESET_MS: u64 = 5_000;
const MAX_L1_QUOTA_RESET_MS: u64 = 10 * 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerRoster {
    vendors: [WorkerVendor; MAX_WORKER_ROSTER_LEN],
    len: u8,
}

impl WorkerRoster {
    pub fn parse(model: &str) -> Option<Self> {
        let mut vendors = [DEFAULT_AUTO_WORKER_VENDOR; MAX_WORKER_ROSTER_LEN];
        let mut len = 0usize;
        for part in model.split(',') {
            let vendor = parse_worker_selection(part)?.vendor;
            if len >= MAX_WORKER_ROSTER_LEN {
                return None;
            }
            vendors[len] = vendor;
            len += 1;
        }
        if len <= 1 {
            return None;
        }
        Some(Self {
            vendors,
            len: len as u8,
        })
    }

    pub fn vendor_for_task_index(self, task_index: u32) -> WorkerVendor {
        let len = usize::from(self.len).max(1);
        self.vendors[task_index as usize % len]
    }

    pub fn fallback_vendor_for_task_index(
        self,
        task_index: u32,
        current: WorkerVendor,
    ) -> Option<WorkerVendor> {
        let len = usize::from(self.len);
        if len <= 1 {
            return None;
        }
        let start = task_index as usize % len;
        (1..len)
            .map(|offset| self.vendors[(start + offset) % len])
            .find(|vendor| *vendor != current)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkerSelection {
    vendor: WorkerVendor,
    model: Option<String>,
}

fn parse_worker_selection(raw: &str) -> Option<WorkerSelection> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (vendor_raw, model_raw) = trimmed
        .split_once(':')
        .map_or((trimmed, None), |(vendor, model)| (vendor, Some(model)));
    let vendor = WorkerVendor::parse(vendor_raw)?;
    let model = model_raw
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned);
    // Reject a model fragment that would be parsed as a CLI flag (e.g.
    // `claude:-x`). render_command_args passes the model as its own argument
    // slot, so a leading dash would be interpreted by the vendor CLI as an
    // option rather than a model name. An invalid selection makes the host
    // IPC reject the value and select_worker_backend fall back to Mock — it
    // never reaches the spawned command (F-20).
    if model.as_deref().is_some_and(|m| m.starts_with('-')) {
        return None;
    }
    Some(WorkerSelection { vendor, model })
}

fn worker_selection_for_task_index(
    spec_worker_model: Option<&str>,
    task_index: u32,
) -> Option<WorkerSelection> {
    let model = spec_worker_model?;
    if model.contains(',') {
        let parts = model.split(',').collect::<Vec<_>>();
        if parts.is_empty() {
            return None;
        }
        parse_worker_selection(parts[task_index as usize % parts.len()])
    } else {
        parse_worker_selection(model)
    }
}

pub(super) fn resolve_worker_model_for_task(
    backend: WorkerBackend,
    task: &TaskDescriptor,
    spec_worker_model: Option<&str>,
) -> Option<String> {
    let WorkerBackend::RealCli(vendor) = backend else {
        return None;
    };
    let selection = worker_selection_for_task_index(spec_worker_model, task.index)?;
    (selection.vendor == vendor)
        .then_some(selection.model)
        .flatten()
}

/// Returns true when a model string is a syntactically valid
/// worker selection, including optional per-worker model overrides.
pub fn worker_model_selection_is_valid(model: &str) -> bool {
    match model.trim() {
        "auto" | "Auto" => true,
        other if other.contains(',') => WorkerRoster::parse(other).is_some(),
        other => parse_worker_selection(other).is_some(),
    }
}

/// Pick the backend for a mission from its spec. Unrecognized
/// `worker_model` strings fall back to Mock (so a stray value can't
/// crash the mission); the host IPC validates input upstream and
/// rejects bad values pre-spawn, so this fallback only matters for
/// tests that hand-build specs.
pub fn select_worker_backend(spec: &MissionSpec) -> WorkerBackend {
    match spec.worker_model.as_deref() {
        None | Some("auto") | Some("Auto") => WorkerBackend::AutoReal,
        Some(L1_CLAUDE_QUOTA_EXHAUSTED_WORKER_MODEL) => WorkerBackend::L1ClaudeQuotaExhausted,
        Some(other) if other.contains(',') => WorkerRoster::parse(other)
            .map(WorkerBackend::Roster)
            .unwrap_or(WorkerBackend::Mock),
        Some(other) => parse_worker_selection(other)
            .map(|selection| WorkerBackend::RealCli(selection.vendor))
            .unwrap_or(WorkerBackend::Mock),
    }
}

/// Resolve a mission-level backend into the concrete backend for a
/// single task. Auto routing uses the task role heuristic, with
/// Claude as the fallback for implementation tasks whose role does
/// not name a more specialized worker.
pub(super) fn resolve_worker_backend_for_task(
    backend: WorkerBackend,
    task: &TaskDescriptor,
    spec_worker_model: Option<&str>,
) -> WorkerBackend {
    match backend {
        WorkerBackend::AutoReal => {
            let vendor = crate::task_graph::select_vendor_for_role(task.role, spec_worker_model)
                .unwrap_or(DEFAULT_AUTO_WORKER_VENDOR);
            WorkerBackend::RealCli(vendor)
        }
        WorkerBackend::Roster(roster) => {
            WorkerBackend::RealCli(roster.vendor_for_task_index(task.index))
        }
        other => other,
    }
}

/// Inputs needed to wire the real-CLI worker's stdout into the
/// mission's [`MissionEventBus`] as user-visible
/// [`MissionEventKind::WorkerProgress`] notes. Mock passes ignore this
/// entirely; only [`WorkerBackend::RealCli`] uses it to build the
/// [`WorkerEventStream`] handed to
/// [`crate::mission_worker_dispatch::run_real_worker`].
#[derive(Clone)]
pub(crate) struct WorkerPassObservability {
    pub(crate) mission_id: String,
    pub(crate) worker_id: String,
    pub(crate) event_bus: MissionEventBus,
    pub(crate) seq: Arc<AtomicU64>,
    pub(crate) memory: Option<Arc<MemoryKernel>>,
    pub(crate) signal_sink: Arc<PassSignalSink>,
}

impl std::fmt::Debug for WorkerPassObservability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerPassObservability")
            .field("mission_id", &self.mission_id)
            .field("worker_id", &self.worker_id)
            .finish()
    }
}

impl WorkerPassObservability {
    /// Drain the adapter-reported quota/context signals. Called by
    /// `run_worker_pass` after the worker pass completes. The
    /// signals are populated by the adapter through the
    /// `signal_sink` channel set up in `build_event_stream`.
    pub(crate) async fn drain_signals(
        &self,
    ) -> (
        Vec<crate::recovery::classify::QuotaSignal>,
        Vec<crate::recovery::types::ContextRequest>,
    ) {
        let mut quota = self.signal_sink.quota.lock().await;
        let mut ctx = self.signal_sink.context.lock().await;
        (std::mem::take(&mut *quota), std::mem::take(&mut *ctx))
    }
}

/// Per-pass collector for adapter signals. Cloned into the
/// `WorkerEventStream` so the line-by-line ingest path can append.
#[derive(Debug, Default)]
pub(crate) struct PassSignalSink {
    pub quota: tokio::sync::Mutex<Vec<crate::recovery::classify::QuotaSignal>>,
    pub context: tokio::sync::Mutex<Vec<crate::recovery::types::ContextRequest>>,
}

/// What `run_worker_pass` returns. The submission Result is the
/// primary signal; the two signal vectors are auxiliary and
/// populated by adapter drain calls that happen during the run.
#[derive(Debug)]
pub(crate) struct WorkerPassOutcome {
    pub submission: Result<WorkerSubmission, crate::mission_worker_dispatch::WorkerDispatchError>,
    pub quota_signals: Vec<crate::recovery::classify::QuotaSignal>,
    pub context_requests: Vec<crate::recovery::types::ContextRequest>,
}

/// Dispatch one worker pass on the given backend. Returns a
/// [`WorkerPassOutcome`] carrying either a [`WorkerSubmission`] or
/// the dispatch error, plus any quota / context signals drained
/// from the adapter mid-run.
///
/// - **Mock:** construct a `MockWorkerVariant` at `(kind_for_index,
///   pass_number)`, write the variant's file, `git add`, `git commit`.
///   The mock backend never populates quota / context signals.
/// - **RealCli:** call [`run_real_worker`] which spawns the vendor CLI
///   in the worktree, streams its stdout through the vendor adapter
///   onto the mission event bus as user-visible WorkerProgress notes
///   (so the run is observable identically to a standalone real-CLI
///   worker), then commits whatever the worker changed (`git add -A
///   && git commit`). The worker playbook forbids workers running git
///   themselves. After the run, drains the adapter signals collected
///   during the pass into the outcome.
pub(super) async fn run_worker_pass(
    backend: WorkerBackend,
    worktree: &Path,
    objective: &str,
    task: &TaskDescriptor,
    pass_number: u32,
    model: Option<&str>,
    revision_directive: Option<&str>,
    observability: WorkerPassObservability,
    cancel: Arc<CancelToken>,
    ephemeral_context: crate::ephemeral_context::EphemeralContextSnapshot,
) -> WorkerPassOutcome {
    let backend = resolve_worker_backend_for_task(backend, task, None);
    // Mock path doesn't speak to a real adapter; signals are empty.
    let backend_for_signals = backend;
    let submission: Result<WorkerSubmission, crate::mission_worker_dispatch::WorkerDispatchError> =
        match backend {
            WorkerBackend::Mock => mock_run(worktree, task, pass_number).await.map_err(|e| {
                crate::mission_worker_dispatch::WorkerDispatchError::Io(e.to_string())
            }),
            WorkerBackend::L1ClaudeQuotaExhausted => {
                l1_claude_quota_exhausted_run(worktree, task, pass_number, &observability).await
            }
            WorkerBackend::AutoReal | WorkerBackend::Roster(_) => {
                // Should be impossible: resolve_worker_backend_for_task maps
                // AutoReal/Roster to a concrete backend above. Surface a
                // dispatch error rather than panicking if that ever changes.
                Err(crate::mission_worker_dispatch::WorkerDispatchError::Io(
                    "unresolved worker backend (AutoReal/Roster) reached dispatch".to_string(),
                ))
            }
            WorkerBackend::RealCli(vendor) => {
                let path_env = crate::resolve_user_path();
                let event_stream = Some(build_event_stream(vendor, &observability));
                run_real_worker_controlled(
                    vendor,
                    worktree.to_path_buf(),
                    objective,
                    &task.title,
                    task.description.as_deref(),
                    revision_directive,
                    pass_number,
                    model,
                    path_env,
                    DEFAULT_WORKER_TIMEOUT,
                    event_stream,
                    Some(cancel),
                    Some(ephemeral_context),
                )
                .await
            }
        };

    // Drain signals from the observability sink (populated by the
    // adapter during the run). The mock backend never populates
    // these; real-CLI runs feed them through the MissionProgressSink
    // path extended in mission_worker_dispatch.rs.
    let (quota_signals, context_requests) = match backend_for_signals {
        WorkerBackend::Mock => (Vec::new(), Vec::new()),
        WorkerBackend::AutoReal | WorkerBackend::Roster(_) => (Vec::new(), Vec::new()),
        WorkerBackend::L1ClaudeQuotaExhausted | WorkerBackend::RealCli(_) => {
            observability.drain_signals().await
        }
    };

    WorkerPassOutcome {
        submission,
        quota_signals,
        context_requests,
    }
}

async fn l1_claude_quota_exhausted_run(
    worktree: &Path,
    task: &TaskDescriptor,
    pass_number: u32,
    observability: &WorkerPassObservability,
) -> Result<WorkerSubmission, crate::mission_worker_dispatch::WorkerDispatchError> {
    let marker = worktree.join(L1_QUOTA_MARKER_PATH);
    if tokio::fs::metadata(&marker).await.is_ok() {
        return mock_run(worktree, task, pass_number)
            .await
            .map_err(|e| crate::mission_worker_dispatch::WorkerDispatchError::Io(e.to_string()));
    }

    if let Some(parent) = marker.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| crate::mission_worker_dispatch::WorkerDispatchError::Io(e.to_string()))?;
    }
    tokio::fs::write(&marker, b"seen\n")
        .await
        .map_err(|e| crate::mission_worker_dispatch::WorkerDispatchError::Io(e.to_string()))?;

    let mut adapter = ClaudeAdapter::new(observability.worker_id.clone(), Some(new_task_id()));
    let events = adapter.ingest_line(
        r#"{"type":"rate_limit_event","rate_limit_info":{"status":"exceeded"}}"#,
        LogStream::Stdout,
    );
    let sink = MissionProgressSink {
        mission_id: observability.mission_id.clone(),
        worker_id: observability.worker_id.clone(),
        event_bus: observability.event_bus.clone(),
        seq: Arc::clone(&observability.seq),
    };
    for event in events {
        sink.emit(&event);
    }

    let adapter_signal = adapter.take_quota_signal();
    let reset_at_ms = adapter_signal
        .and_then(|sig| sig.estimated_reset_at_ms)
        .unwrap_or_else(|| now_unix_ms().saturating_add(l1_quota_reset_delay_ms()));
    observability
        .signal_sink
        .quota
        .lock()
        .await
        .push(crate::recovery::classify::QuotaSignal {
            vendor: event_schema::Vendor::Claude,
            estimated_reset_at_ms: Some(reset_at_ms),
        });

    Err(crate::mission_worker_dispatch::WorkerDispatchError::Exit(
        "claude_quota_exhausted mock reported Claude quota exhaustion".into(),
    ))
}

fn l1_quota_reset_delay_ms() -> u64 {
    std::env::var("VIGLA_L1_QUOTA_RESET_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .map(|ms| ms.clamp(MIN_L1_QUOTA_RESET_MS, MAX_L1_QUOTA_RESET_MS))
        .unwrap_or(DEFAULT_L1_QUOTA_RESET_MS)
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

async fn mock_run(
    worktree: &Path,
    task: &TaskDescriptor,
    pass_number: u32,
) -> Result<WorkerSubmission, MissionRuntimeError> {
    let variant = MockWorkerVariant {
        kind: MockWorkerKind::for_task_index(task.index),
        pass: pass_number,
    };
    let pass = variant.run_pass(task);
    tokio::fs::write(worktree.join(&pass.file_name), &pass.file_content)
        .await
        .map_err(|e| MissionRuntimeError::Io(e.to_string()))?;
    run_git_in(worktree, &["add", &pass.file_name]).await?;
    run_git_in(worktree, &["commit", "-m", &pass.commit_message]).await?;
    Ok(WorkerSubmission {
        files: vec![pass.file_name],
        summary: pass.submission_summary,
        commit_message: pass.commit_message,
        produced_changes: true,
    })
}

/// Construct a [`WorkerEventStream`] whose sink translates the
/// canonical event flow into mission-level WorkerProgress notes. The
/// adapter is the vendor-specific one used elsewhere in the
/// orchestrator (same instance type the standalone supervisor builds
/// in `supervisor::real_workers`).
fn build_event_stream(vendor: WorkerVendor, obs: &WorkerPassObservability) -> WorkerEventStream {
    let task_id = new_task_id();
    let adapter: Box<dyn Adapter> = match vendor {
        WorkerVendor::Claude => Box::new(ClaudeAdapter::new(obs.worker_id.clone(), Some(task_id))),
        WorkerVendor::Codex => Box::new(CodexAdapter::new(obs.worker_id.clone(), Some(task_id))),
        WorkerVendor::Gemini => Box::new(GeminiAdapter::new(obs.worker_id.clone(), Some(task_id))),
        WorkerVendor::Antigravity => Box::new(AntigravityAdapter::new(
            obs.worker_id.clone(),
            Some(task_id),
        )),
        WorkerVendor::Kiro => Box::new(KiroAdapter::new(obs.worker_id.clone(), Some(task_id))),
        WorkerVendor::Copilot => {
            Box::new(CopilotAdapter::new(obs.worker_id.clone(), Some(task_id)))
        }
    };
    let sink: Arc<dyn WorkerEventSink> = Arc::new(MissionProgressSink {
        mission_id: obs.mission_id.clone(),
        worker_id: obs.worker_id.clone(),
        event_bus: obs.event_bus.clone(),
        seq: Arc::clone(&obs.seq),
    });
    let memory_sink: Option<Arc<dyn MemoryIntentSink>> = obs.memory.as_ref().map(|kernel| {
        Arc::new(KernelIntentSink::new(
            kernel.clone(),
            obs.mission_id.clone(),
            obs.worker_id.clone(),
        )) as Arc<dyn MemoryIntentSink>
    });
    WorkerEventStream {
        worker_id: obs.worker_id.clone(),
        vendor: vendor.event_schema_vendor(),
        adapter,
        // Mission events live on the in-memory bus today (see
        // `crate::mission_event` module docs). When mission-scoped
        // persistence lands, plumb the Repository through here.
        repo: None,
        sink,
        memory_sink,
        signal_sink: Some(obs.signal_sink.clone()),
    }
}

/// Bridge from canonical [`Event`]s to mission-level
/// [`MissionEventKind::WorkerProgress`]. Filters down to events the
/// user actually wants in their mission timeline — see
/// [`event_to_progress_note`] for the curated mapping.
#[derive(Debug)]
struct MissionProgressSink {
    mission_id: String,
    worker_id: String,
    event_bus: MissionEventBus,
    seq: Arc<AtomicU64>,
}

impl WorkerEventSink for MissionProgressSink {
    fn emit(&self, event: &Event) {
        let Some(note) = event_to_progress_note(event) else {
            return;
        };
        self.event_bus.emit_kind(
            &self.mission_id,
            &self.seq,
            MissionEventKind::WorkerProgress {
                worker_id: self.worker_id.clone(),
                note,
            },
        );
    }
}

/// Render a canonical event as a short progress note. Returns `None`
/// for event kinds that would be noisy on the mission timeline
/// (info-level logs, per-token cost ticks, dependency churn) — the
/// MissionEventBus is a user-facing channel, not a raw stream.
fn event_to_progress_note(event: &Event) -> Option<String> {
    match &event.kind {
        EventKind::StateChange(sc) => {
            let state = serialize_snake_case(&sc.state)?;
            Some(match &sc.note {
                Some(note) if !note.is_empty() => format!("state: {state} ({note})"),
                _ => format!("state: {state}"),
            })
        }
        EventKind::Progress(p) => match &p.note {
            Some(note) if !note.is_empty() => Some(note.clone()),
            _ if p.percent.is_finite() => Some(format!("progress: {:.0}%", p.percent)),
            _ => None,
        },
        EventKind::FileActivity(fa) => {
            let op = serialize_snake_case(&fa.op)?;
            Some(format!("{op}: {}", fa.path))
        }
        EventKind::TestResult(t) => Some(format!(
            "tests {}: {} passed / {} failed / {} skipped",
            t.suite, t.passed, t.failed, t.skipped
        )),
        EventKind::Completion(c) => Some(format!("completion: {}", c.summary)),
        EventKind::Failure(f) => Some(format!("failure: {}", f.error)),
        EventKind::Dependency(d) => Some(format!(
            "blocked on {} — {}",
            d.waiting_on.join(", "),
            d.reason
        )),
        EventKind::Log(log) => match log.level {
            LogLevel::Warn | LogLevel::Error => {
                let level = serialize_snake_case(&log.level)?;
                Some(format!("[{level}] {}", log.line))
            }
            // Info/debug/trace logs would flood the mission timeline.
            // The drawer surface (Step 14) can render them from the
            // persisted log if a repo gets wired in later.
            _ => None,
        },
        // Cost events feed the budget meter elsewhere; surfacing
        // per-tick cost as user progress would just be noise.
        EventKind::Cost(_) => None,
    }
}

fn serialize_snake_case<T: serde::Serialize>(value: &T) -> Option<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
}

pub(super) fn side_effect_events_for_submission(
    worker_id: &str,
    backend: WorkerBackend,
    summary: &str,
) -> Vec<MissionEventKind> {
    let Some(package_install) = detect_package_install(summary) else {
        return Vec::new();
    };
    let kind = DeclaredSideEffectKind::PackageInstall;
    vec![MissionEventKind::SideEffectLogged {
        worker_id: worker_id.to_owned(),
        kind,
        summary: format!("package install observed: {package_install}"),
        declared: side_effect_declared_by_backend(backend, kind),
    }]
}

fn detect_package_install(summary: &str) -> Option<&'static str> {
    let lower = summary.to_ascii_lowercase();
    [
        "python -m pip install",
        "python3 -m pip install",
        "pip install",
        "pip3 install",
        "npm install",
        "pnpm add",
        "yarn add",
        "cargo add",
        "brew install",
    ]
    .into_iter()
    .find(|needle| lower.contains(needle))
}

fn side_effect_declared_by_backend(backend: WorkerBackend, kind: DeclaredSideEffectKind) -> bool {
    match backend {
        WorkerBackend::Mock | WorkerBackend::L1ClaudeQuotaExhausted => false,
        WorkerBackend::AutoReal | WorkerBackend::Roster(_) => [
            WorkerVendor::Claude,
            WorkerVendor::Codex,
            WorkerVendor::Gemini,
        ]
        .into_iter()
        .any(|vendor| {
            profile_for_vendor(vendor)
                .declared_side_effects
                .iter()
                .any(|effect| effect.kind == kind)
        }),
        WorkerBackend::RealCli(vendor) => profile_for_vendor(vendor)
            .declared_side_effects
            .iter()
            .any(|effect| effect.kind == kind),
    }
}

async fn run_git_in(cwd: &Path, args: &[&str]) -> Result<(), MissionRuntimeError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .map_err(|e| MissionRuntimeError::Io(e.to_string()))?;
    if !output.status.success() {
        return Err(MissionRuntimeError::Git(
            crate::mission_workspace::MissionGitError::Git {
                code: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            },
        ));
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────
// S6 — ReviewIntent → ReworkKind translation
// ─────────────────────────────────────────────────────────────────

/// Convert a [`supervisor_adapter::ReviewIntent`] into the arbiter's
/// [`crate::arbiter::ReworkKind`], when the decision is one of the
/// six rework kinds.
///
/// Returns `None` if the decision is terminal (Accept) — the
/// caller's normal Accept path runs. Returns `Some(_)` for any
/// non-terminal decision; legacy `Reject` maps to
/// `MarkUnachievable` to preserve the deprecation policy from
/// Task 4 (back-compat).
pub(crate) fn rework_kind_from_review_intent(
    intent: &supervisor_adapter::ReviewIntent,
) -> Option<crate::arbiter::ReworkKind> {
    use supervisor_adapter::ReviewDecisionTag as Tag;
    match intent.decision {
        Tag::Accept => None,
        Tag::Revise => Some(crate::arbiter::ReworkKind::Revise {
            directive: intent.directive.clone().unwrap_or_default(),
        }),
        Tag::Reject | Tag::MarkUnachievable => Some(crate::arbiter::ReworkKind::MarkUnachievable {
            rationale: intent
                .resolved_rationale()
                .map(str::to_string)
                .unwrap_or_default(),
        }),
        Tag::Reassign => Some(crate::arbiter::ReworkKind::Reassign {
            from_worker: intent.resolved_from_worker().to_string(),
            to_vendor: intent.to_vendor,
        }),
        Tag::Split => {
            let descs = intent
                .sub_tasks
                .as_ref()
                .map(|subs| {
                    subs.iter()
                        .enumerate()
                        .map(|(i, s)| crate::mission_event::TaskDescriptor {
                            index: i as u32,
                            title: s.title.clone(),
                            description: s.description.clone(),
                            depends_on: s.depends_on.clone(),
                            scope_paths: s.scope_paths.clone(),
                            ..Default::default()
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Some(crate::arbiter::ReworkKind::Split { sub_tasks: descs })
        }
        Tag::Narrow => Some(crate::arbiter::ReworkKind::Narrow {
            reduced_scope: intent
                .reduced_scope
                .as_ref()
                .map(|paths| paths.iter().map(std::path::PathBuf::from).collect())
                .unwrap_or_default(),
        }),
        Tag::Rebrief => Some(crate::arbiter::ReworkKind::Rebrief {
            new_brief: intent.new_brief.clone().unwrap_or_default(),
        }),
    }
}

#[cfg(test)]
mod outcome_tests {
    use super::*;

    #[test]
    fn worker_pass_outcome_ok_with_no_signals() {
        let outcome = WorkerPassOutcome {
            submission: Ok(WorkerSubmission {
                files: vec!["a.rs".into()],
                summary: "ok".into(),
                commit_message: "ok".into(),
                produced_changes: true,
            }),
            quota_signals: vec![],
            context_requests: vec![],
        };
        assert!(outcome.submission.is_ok());
        assert!(outcome.quota_signals.is_empty());
        assert!(outcome.context_requests.is_empty());
    }

    #[tokio::test]
    async fn l1_claude_quota_backend_emits_quota_signal_and_marker() {
        let dir = tempfile::tempdir().expect("tempdir");
        let signal_sink = Arc::new(PassSignalSink::default());
        let obs = WorkerPassObservability {
            mission_id: "mission-l1".into(),
            worker_id: "worker-l1".into(),
            event_bus: crate::mission_runtime::MissionEventBus::new(16),
            seq: Arc::new(AtomicU64::new(0)),
            memory: None,
            signal_sink: Arc::clone(&signal_sink),
        };
        let task = TaskDescriptor {
            index: 0,
            title: "Trigger quota".into(),
            ..Default::default()
        };

        let err = l1_claude_quota_exhausted_run(dir.path(), &task, 0, &obs)
            .await
            .expect_err("first pass should report quota exhaustion");

        assert!(err.to_string().contains("quota"), "unexpected error: {err}");
        assert!(dir.path().join(L1_QUOTA_MARKER_PATH).is_file());
        let (quota, context) = obs.drain_signals().await;
        assert!(context.is_empty());
        assert_eq!(quota.len(), 1);
        assert_eq!(quota[0].vendor, event_schema::Vendor::Claude);
        assert!(quota[0]
            .estimated_reset_at_ms
            .is_some_and(|reset| reset > now_unix_ms()));
    }
}

#[cfg(test)]
mod worker_selection_tests {
    use super::*;

    #[test]
    fn valid_vendor_and_model_parse() {
        let s = parse_worker_selection("claude:claude-opus-4-7").expect("valid");
        assert_eq!(s.vendor, WorkerVendor::Claude);
        assert_eq!(s.model.as_deref(), Some("claude-opus-4-7"));
    }

    #[test]
    fn bare_vendor_has_no_model() {
        let s = parse_worker_selection("claude").expect("valid");
        assert!(s.model.is_none());
    }

    #[test]
    fn model_starting_with_dash_is_rejected() {
        // F-20: a `-`-prefixed model would be passed to the vendor CLI as a
        // flag. Reject the whole selection so it can't reach the command.
        assert!(parse_worker_selection("claude:-x").is_none());
        assert!(parse_worker_selection("claude:--dangerously-skip").is_none());
        assert!(!worker_model_selection_is_valid("claude:-x"));
    }
}

#[cfg(test)]
mod rework_kind_tests {
    use super::*;
    use supervisor_adapter::{ReviewDecisionTag, ReviewIntent};

    fn intent(decision: ReviewDecisionTag) -> ReviewIntent {
        ReviewIntent {
            worker_id: "mock-1".into(),
            decision,
            summary: None,
            directive: None,
            reason: None,
            from_worker: None,
            to_vendor: None,
            sub_tasks: None,
            reduced_scope: None,
            new_brief: None,
            rationale: None,
        }
    }

    #[test]
    fn accept_returns_none() {
        assert!(rework_kind_from_review_intent(&intent(ReviewDecisionTag::Accept)).is_none());
    }

    #[test]
    fn revise_passes_directive_through() {
        let mut i = intent(ReviewDecisionTag::Revise);
        i.directive = Some("fix the parser".into());
        match rework_kind_from_review_intent(&i).unwrap() {
            crate::arbiter::ReworkKind::Revise { directive } => {
                assert_eq!(directive, "fix the parser");
            }
            other => panic!("expected Revise, got {other:?}"),
        }
    }

    #[test]
    fn revise_without_directive_yields_empty_string() {
        let i = intent(ReviewDecisionTag::Revise);
        match rework_kind_from_review_intent(&i).unwrap() {
            crate::arbiter::ReworkKind::Revise { directive } => {
                assert_eq!(directive, "");
            }
            other => panic!("expected Revise, got {other:?}"),
        }
    }

    #[test]
    fn legacy_reject_maps_to_mark_unachievable_via_reason() {
        let mut i = intent(ReviewDecisionTag::Reject);
        i.reason = Some("legacy rejection".into());
        match rework_kind_from_review_intent(&i).unwrap() {
            crate::arbiter::ReworkKind::MarkUnachievable { rationale } => {
                assert_eq!(rationale, "legacy rejection");
            }
            other => panic!("expected MarkUnachievable, got {other:?}"),
        }
    }

    #[test]
    fn reassign_falls_back_from_worker_to_worker_id() {
        let mut i = intent(ReviewDecisionTag::Reassign);
        i.to_vendor = Some(event_schema::Vendor::Codex);
        match rework_kind_from_review_intent(&i).unwrap() {
            crate::arbiter::ReworkKind::Reassign {
                from_worker,
                to_vendor,
            } => {
                assert_eq!(from_worker, "mock-1");
                assert_eq!(to_vendor, Some(event_schema::Vendor::Codex));
            }
            other => panic!("expected Reassign, got {other:?}"),
        }
    }

    #[test]
    fn split_translates_sub_tasks_with_reindex() {
        let mut i = intent(ReviewDecisionTag::Split);
        i.sub_tasks = Some(vec![
            supervisor_adapter::SupervisorTaskDescriptor {
                title: "Add parser".into(),
                description: None,
                ..Default::default()
            },
            supervisor_adapter::SupervisorTaskDescriptor {
                title: "Add tests".into(),
                description: Some("unit + integration".into()),
                ..Default::default()
            },
        ]);
        match rework_kind_from_review_intent(&i).unwrap() {
            crate::arbiter::ReworkKind::Split { sub_tasks } => {
                assert_eq!(sub_tasks.len(), 2);
                assert_eq!(sub_tasks[0].index, 0);
                assert_eq!(sub_tasks[1].index, 1);
            }
            other => panic!("expected Split, got {other:?}"),
        }
    }

    #[test]
    fn narrow_translates_string_paths_to_pathbufs() {
        let mut i = intent(ReviewDecisionTag::Narrow);
        i.reduced_scope = Some(vec!["src/lib.rs".into()]);
        match rework_kind_from_review_intent(&i).unwrap() {
            crate::arbiter::ReworkKind::Narrow { reduced_scope } => {
                assert!(reduced_scope[0].ends_with("lib.rs"));
            }
            other => panic!("expected Narrow, got {other:?}"),
        }
    }
}
