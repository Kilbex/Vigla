//! Real CLI worker dispatch (MSV U4.2).
//!
//! Spawns a real, profile-backed vendor CLI process inside a
//! mission's worker worktree to do actual work on a task, then
//! commits whatever changes the worker produced. Returns a
//! [`WorkerSubmission`] the supervisor mission loop reads as the
//! review prompt (same path as mock submissions).
//!
//! When the caller supplies a [`WorkerEventStream`], stdout/stderr are
//! also streamed line-by-line through the supplied [`Adapter`] and
//! each produced [`Event`] is persisted (when a repository is
//! attached) and forwarded through the sink — the same pipeline the
//! standalone supervisor's
//! [`crate::supervisor::Supervisor::supervise_with_adapter`] uses for
//! its own workers. This keeps mission-spawned real CLI workers and
//! standalone real CLI workers observable in exactly the same way.
//!
//! ## Why we commit on the worker's behalf
//!
//! The worker playbook
//! ([`supervisor_adapter::WORKER_PLAYBOOK`]) explicitly forbids
//! workers running `git` commands. Concentrating the commit in the
//! orchestrator gives us:
//!
//! - **Atomicity:** one commit per submission, always.
//! - **Boundary clarity:** "worker done" is unambiguous (process
//!   exit), not "worker decided it's done."
//! - **Safety:** the worker can't accidentally push, switch
//!   branches, or commit partial state.
//!
//! ## Routing
//!
//! - `worker_model = None | "auto"` is resolved by the caller into a
//!   concrete real CLI per task role before routing here.
//! - Any registered vendor id (`claude`, `codex`, `antigravity`,
//!   `kiro`, `copilot`, or legacy `gemini`) routes here directly.
//! - A comma-separated vendor roster is resolved by the
//!   caller into an independently selected CLI per task index.
//! - Anything else is rejected pre-spawn by the host IPC.

use crate::mission_runtime::CancelToken;
use crate::parser::{read_line_capped, LineRead, WorkerEventSink, MAX_LINE_BYTES};
use crate::repository::{InsertOutcome, Repository};
pub use crate::vendor_profile::WorkerVendor;
use crate::vendor_profile::{profile_for_vendor, render_command_args, CommandRole, CommandVars};
use adapter_core::{Adapter, AdapterExit};
use event_schema::{Event, LogStream};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncBufRead, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::time::timeout;

/// Default per-worker wall-clock timeout. Generous — real CLI workers
/// can spend several minutes on a non-trivial task. The mission-level
/// timeout (10 min for U3) bounds the whole run.
pub const DEFAULT_WORKER_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_CAPTURED_OUTPUT_BYTES: usize = 64 * 1024;
const CAPTURE_TRUNCATED_MARKER: &str = "\n[vigla: captured output truncated]\n";
const POST_EXIT_DRAIN_TIMEOUT: Duration = Duration::from_millis(250);

/// What the orchestrator gets back after a worker pass completes.
/// Shaped to feed the supervisor's review prompt verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerSubmission {
    /// Files changed in the worker's commit (relative to worktree).
    pub files: Vec<String>,
    /// One-line summary suitable for the supervisor's review prompt.
    /// Concise; the supervisor's playbook reads short summaries best.
    pub summary: String,
    /// The full final commit message that the orchestrator wrote on
    /// the worker's behalf.
    pub commit_message: String,
    /// Did the worker produce any committable changes? `false` means
    /// `git diff` was empty after the worker exited — the supervisor
    /// will receive a submission summary saying so.
    pub produced_changes: bool,
}

/// Optional event-streaming attached to a [`run_real_worker`] call.
///
/// When supplied, every line a worker writes to stdout/stderr is
/// passed through `adapter` into canonical [`Event`]s. Each event is
/// forwarded through `sink` for live observability; if `repo` is also
/// `Some`, the event is first persisted (duplicates by `(worker_id,
/// seq)` are skipped silently before reaching the sink — matching the
/// supervisor's `persist_and_emit` semantics).
///
/// Mirrors
/// [`crate::supervisor::Supervisor::supervise_with_adapter`] so the
/// mission's real-CLI worker path is observable identically to the
/// standalone real-CLI worker path. Pass `None` from callers that do
/// not need observability — the dispatcher then behaves exactly as
/// the pre-event-surface implementation (capture stdout for the
/// commit message tail; no parsing, no emission).
pub struct WorkerEventStream {
    /// Worker identifier. Used only for diagnostic logging here; the
    /// adapter stamps it on every event it emits.
    pub worker_id: String,
    /// Canonical vendor identity for the adapter behind `adapter`. The
    /// line-dispatch loop stamps quota signals with this so the
    /// supervisor's recovery engine knows which vendor exhausted.
    /// Adapters surface `adapter_core::QuotaSignal` (no vendor field —
    /// the adapter trait is vendor-agnostic), and the supervisor needs
    /// `recovery::classify::QuotaSignal { vendor, ... }`; this field
    /// is the bridge.
    pub vendor: event_schema::Vendor,
    /// Vendor-specific adapter converting raw lines into canonical
    /// events.
    pub adapter: Box<dyn Adapter>,
    /// Optional persistence target. When `Some`, every event is
    /// inserted via [`Repository::insert_event`] before being emitted
    /// through `sink`; duplicate-seq inserts are skipped (no emit).
    /// When `None`, events flow straight to `sink`.
    pub repo: Option<Repository>,
    /// Live observability sink — typically forwards to the frontend
    /// (Tauri `AppHandle::emit`) or a test capture buffer.
    pub sink: Arc<dyn WorkerEventSink>,
    /// Optional memory sink. When installed, memory intents extracted
    /// by the vendor adapter are drained after each raw line and
    /// routed into the Memory Kernel. Keeping this on the event stream
    /// makes proposal capture opt-in and preserves byte-identical
    /// behaviour for callers that do not install memory.
    pub memory_sink: Option<Arc<dyn crate::memory::MemoryIntentSink>>,
    /// S5: adapter-side quota/context signals drained per line.
    pub(crate) signal_sink: Option<Arc<crate::mission_supervisor_run::PassSignalSink>>,
}

impl std::fmt::Debug for WorkerEventStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerEventStream")
            .field("worker_id", &self.worker_id)
            .field("vendor", &self.vendor)
            .field("adapter", &self.adapter)
            .field("repo", &self.repo.as_ref().map(|_| "<Repository>"))
            .field("sink", &"<dyn WorkerEventSink>")
            .field(
                "memory_sink",
                &self.memory_sink.as_ref().map(|_| "<dyn MemoryIntentSink>"),
            )
            .field(
                "signal_sink",
                &self.signal_sink.as_ref().map(|_| "<PassSignalSink>"),
            )
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum WorkerDispatchError {
    #[error("worker spawn failed: {0}")]
    Spawn(String),
    #[error("worker process exited with error: {0}")]
    Exit(String),
    #[error("worker exceeded timeout: {0:?}")]
    Timeout(Duration),
    #[error("git error: {0}")]
    Git(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("worker cancelled")]
    Cancelled,
}

/// Spawn a real worker for one task pass.
///
/// `revision_directive` is `Some(...)` on revision passes (the
/// supervisor's directive from the previous review). The dispatcher
/// composes the worker prompt accordingly.
///
/// `event_stream` is the optional observability surface — see
/// [`WorkerEventStream`]. When `Some`, every stdout/stderr line is
/// fed through the supplied adapter and each event reaches the
/// repository (if attached) and sink. When `None`, no parsing happens
/// — the function falls back to the pre-event-surface behavior
/// (capture only, for the commit-message tail). Either way, the
/// returned [`WorkerSubmission`] is identical for a given worker run.
///
/// The function:
///
/// 1. Builds the vendor-specific command with the worker playbook
///    appended/prepended as appropriate for the vendor's CLI shape.
/// 2. Spawns the process in `worktree`, streams stdout into a buffer
///    (so a future drawer can render it), optionally also through the
///    event stream, waits with the configured timeout.
/// 3. On exit / timeout, calls `adapter.finalize` with the
///    appropriate [`AdapterExit`] so the adapter can emit terminal
///    events even on kill / non-zero exit.
/// 4. Stages and commits whatever the worker changed. If nothing
///    changed, marks `produced_changes: false` and skips the commit.
/// 5. Returns the submission shape the supervisor reviews.
#[allow(clippy::too_many_arguments)]
pub async fn run_real_worker(
    vendor: WorkerVendor,
    worktree: PathBuf,
    objective: &str,
    task_title: &str,
    task_description: Option<&str>,
    revision_directive: Option<&str>,
    pass_number: u32,
    model: Option<&str>,
    path_env: &str,
    worker_timeout: Duration,
    event_stream: Option<WorkerEventStream>,
) -> Result<WorkerSubmission, WorkerDispatchError> {
    run_real_worker_controlled(
        vendor,
        worktree,
        objective,
        task_title,
        task_description,
        revision_directive,
        pass_number,
        model,
        path_env,
        worker_timeout,
        event_stream,
        None,
        None,
    )
    .await
}

pub(crate) async fn run_real_worker_controlled(
    vendor: WorkerVendor,
    worktree: PathBuf,
    objective: &str,
    task_title: &str,
    task_description: Option<&str>,
    revision_directive: Option<&str>,
    pass_number: u32,
    model: Option<&str>,
    path_env: &str,
    worker_timeout: Duration,
    mut event_stream: Option<WorkerEventStream>,
    cancel: Option<Arc<CancelToken>>,
    ephemeral_context: Option<crate::ephemeral_context::EphemeralContextSnapshot>,
) -> Result<WorkerSubmission, WorkerDispatchError> {
    let prompt = compose_prompt(objective, task_title, task_description, revision_directive);
    let mut cmd = build_command(vendor, &prompt, &worktree, path_env, model);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| WorkerDispatchError::Spawn(e.to_string()))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    let (captured, status) = match collect_child_output_and_status_controlled(
        child,
        stdout,
        stderr,
        &mut event_stream,
        Some(worker_timeout),
        cancel.as_deref(),
    )
    .await
    {
        Ok((c, s)) => {
            let exit = classify_adapter_exit(&s);
            drain_finalize(&mut event_stream, exit).await;
            (c, s)
        }
        Err(ControlledCollectError::Io(e)) => {
            restore_ephemeral_context(ephemeral_context.as_ref(), &worktree).await?;
            drain_finalize(&mut event_stream, AdapterExit::Failed { code: None }).await;
            return Err(WorkerDispatchError::Exit(e));
        }
        Err(ControlledCollectError::Timeout) => {
            restore_ephemeral_context(ephemeral_context.as_ref(), &worktree).await?;
            drain_finalize(&mut event_stream, AdapterExit::Killed).await;
            return Err(WorkerDispatchError::Timeout(worker_timeout));
        }
        Err(ControlledCollectError::Cancelled) => {
            restore_ephemeral_context(ephemeral_context.as_ref(), &worktree).await?;
            drain_finalize(&mut event_stream, AdapterExit::Killed).await;
            return Err(WorkerDispatchError::Cancelled);
        }
    };

    // Context supplied through CLAUDE.md / AGENTS.md / GEMINI.md is an
    // ephemeral input. Remove Vigla-owned regions before any diff, ACL audit,
    // or `git add -A`, while preserving worker edits outside those regions.
    restore_ephemeral_context(ephemeral_context.as_ref(), &worktree).await?;

    // NOTE: we deliberately do NOT write captured stdout to a file
    // inside `worktree` here. An earlier draft wrote it to
    // `<worktree>/.vigla/worker-output.log`, but `git add -A` then
    // staged it into the worker's commit and contaminated the
    // integration with debug noise. The captured string is in memory;
    // a future drawer surface can render it from there.
    let tail = tail_summary(&captured, 400);
    let exit_code = if status.success() {
        None
    } else {
        // Fall through to commit anyway — some CLIs exit non-zero but
        // still produced useful changes. Let git be the source of
        // truth via the empty-diff check inside commit_and_synthesize.
        Some(status.code().unwrap_or(-1))
    };
    let submission =
        commit_and_synthesize(&worktree, vendor, task_title, pass_number, &tail, exit_code).await?;
    Ok(submission)
}

async fn restore_ephemeral_context(
    snapshot: Option<&crate::ephemeral_context::EphemeralContextSnapshot>,
    worktree: &Path,
) -> Result<(), WorkerDispatchError> {
    match snapshot {
        Some(snapshot) => snapshot.restore(worktree).await.map_err(|error| {
            WorkerDispatchError::Io(format!("ephemeral context cleanup failed: {error}"))
        }),
        None => Ok(()),
    }
}

/// Mirror of `supervisor::adapter_supervision::classify_exit`. Kept
/// inline so this module stays independent of the supervisor module's
/// privacy boundary.
fn classify_adapter_exit(status: &ExitStatus) -> AdapterExit {
    if status.success() {
        return AdapterExit::Clean;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if status.signal().is_some() {
            return AdapterExit::Killed;
        }
    }
    AdapterExit::Failed {
        code: status.code(),
    }
}

/// Drain `adapter.finalize` events into the supplied stream, mirroring
/// `process_with_adapter`'s end-of-stream behaviour. No-op when no
/// stream is attached.
async fn drain_finalize(stream: &mut Option<WorkerEventStream>, exit: AdapterExit) {
    let Some(stream) = stream.as_mut() else {
        return;
    };
    let events = stream.adapter.finalize(exit);
    for event in events {
        forward_event(stream, &event).await;
    }
    drain_memory_intents(stream);
    // S5 / C1: a quota error reported in the final newline-less chunk
    // (e.g. a vendor that prints "Quota exceeded" then exits non-zero
    // without flushing) lands in the adapter's `pending_*` slots
    // before `finalize` returns. Drain those into the supervisor's
    // PassSignalSink so the recovery engine sees them on the first
    // pass — not just on a subsequent one.
    drain_adapter_signals(stream).await;
}

/// Persist (if a repo is attached) and emit a single event. Mirrors
/// `parser::persist_and_emit` semantics: duplicate-seq inserts are
/// logged inside the repository and skipped here so the live sink
/// never sees the same event twice.
async fn forward_event(stream: &WorkerEventStream, event: &Event) {
    if let Some(repo) = stream.repo.as_ref() {
        match repo.insert_event(event).await {
            Ok(InsertOutcome::Inserted) => stream.sink.emit(event),
            Ok(InsertOutcome::DuplicateSkipped) => {}
            Err(e) => tracing::error!(
                "orchestrator: insert_event failed for worker {}: {e}",
                stream.worker_id
            ),
        }
    } else {
        stream.sink.emit(event);
    }
}

/// Feed one captured line through `stream.adapter` and forward every
/// resulting event. Line endings are trimmed before ingest so adapter
/// authors see the same shape they would from a JSONL stream.
async fn forward_line_to_adapter(
    stream: &mut WorkerEventStream,
    line: &str,
    log_stream: LogStream,
) {
    let text = line.trim_end_matches(['\r', '\n']);
    let events = stream.adapter.ingest_line(text, log_stream);
    for event in events {
        forward_event(stream, &event).await;
    }
    drain_memory_intents(stream);
    // S5 / C1: route adapter-side quota / context signals into the
    // supervisor's PassSignalSink so the recovery engine catches a
    // mid-pass quota hit on the very first occurrence. Without this
    // the only check is the pre-pass `tracker.is_exhausted` guard,
    // which can only see state from a prior run.
    drain_adapter_signals(stream).await;
}

fn drain_memory_intents(stream: &mut WorkerEventStream) {
    if stream.memory_sink.is_none() {
        return;
    }
    let intents = stream.adapter.take_memory_intents();
    if intents.is_empty() {
        return;
    }
    if let Some(sink) = stream.memory_sink.as_ref() {
        for intent in intents {
            sink.emit(intent);
        }
    }
}

/// Drain adapter quota / context-request signals into the supervisor's
/// PassSignalSink. No-op when no `signal_sink` is installed (mock
/// callers and standalone-supervisor callers run without one).
///
/// Conversion notes:
///
/// - `adapter_core::QuotaSignal` carries only `estimated_reset_at_ms`
///   because the adapter trait is vendor-agnostic. We stamp the
///   canonical vendor identity from `stream.vendor` so the supervisor
///   recovery engine knows which vendor is exhausted.
/// - `adapter_core::ContextRequestSignal` and
///   `recovery::types::ContextRequest` are the same shape modulo
///   kind enum names — we translate variant-by-variant. Adding a
///   new `ContextRequestSignalKind` variant will fail this match
///   on purpose so we update the recovery shape in lockstep.
async fn drain_adapter_signals(stream: &mut WorkerEventStream) {
    let Some(sink) = stream.signal_sink.clone() else {
        return;
    };

    if let Some(adapter_sig) = stream.adapter.take_quota_signal() {
        let canonical = crate::recovery::classify::QuotaSignal {
            vendor: stream.vendor,
            estimated_reset_at_ms: adapter_sig.estimated_reset_at_ms,
        };
        sink.quota.lock().await.push(canonical);
    }

    let context_signals = stream.adapter.take_context_requests();
    if !context_signals.is_empty() {
        let mut guard = sink.context.lock().await;
        for sig in context_signals {
            guard.push(adapter_context_request_to_recovery(sig));
        }
    }
}

fn adapter_context_request_to_recovery(
    sig: adapter_core::ContextRequestSignal,
) -> crate::recovery::types::ContextRequest {
    use crate::recovery::types::ContextRequestKind as RK;
    use adapter_core::ContextRequestSignalKind as AK;
    let kind = match sig.kind {
        AK::FileContent => RK::FileContent,
        AK::Documentation => RK::Documentation,
        AK::PriorDecision => RK::PriorDecision,
    };
    crate::recovery::types::ContextRequest {
        kind,
        detail: sig.detail,
    }
}

#[cfg(test)]
async fn collect_child_output_and_status(
    child: Child,
    stdout: ChildStdout,
    stderr: ChildStderr,
    event_stream: &mut Option<WorkerEventStream>,
) -> Result<(String, ExitStatus), String> {
    collect_child_output_and_status_controlled(child, stdout, stderr, event_stream, None, None)
        .await
        .map_err(|error| match error {
            ControlledCollectError::Io(error) => error,
            ControlledCollectError::Timeout | ControlledCollectError::Cancelled => {
                "unexpected controlled process termination".to_string()
            }
        })
}

#[derive(Debug)]
enum ControlledCollectError {
    Io(String),
    Timeout,
    Cancelled,
}

async fn collect_child_output_and_status_controlled(
    mut child: Child,
    stdout: ChildStdout,
    stderr: ChildStderr,
    event_stream: &mut Option<WorkerEventStream>,
    wall_timeout: Option<Duration>,
    cancel: Option<&CancelToken>,
) -> Result<(String, ExitStatus), ControlledCollectError> {
    let mut stdout_buf = BufReader::new(stdout);
    let mut stderr_buf = BufReader::new(stderr);
    let mut stdout_line = String::new();
    let mut stderr_line = String::new();
    let mut stdout_eof = false;
    let mut stderr_eof = false;
    let mut captured = String::new();
    let mut capture_truncated = false;

    let deadline =
        tokio::time::sleep(wall_timeout.unwrap_or_else(|| Duration::from_secs(365 * 24 * 60 * 60)));
    tokio::pin!(deadline);

    let status = loop {
        if stdout_eof && stderr_eof {
            break child
                .wait()
                .await
                .map_err(|e| ControlledCollectError::Io(e.to_string()))?;
        }
        tokio::select! {
            biased;
            _ = wait_for_optional_cancel(cancel) => {
                crate::process_tree::terminate_and_reap(&mut child).await;
                return Err(ControlledCollectError::Cancelled);
            }
            _ = &mut deadline => {
                crate::process_tree::terminate_and_reap(&mut child).await;
                return Err(ControlledCollectError::Timeout);
            }
            line = read_line_capped(&mut stdout_buf, &mut stdout_line, MAX_LINE_BYTES),
                if !stdout_eof =>
            {
                match line {
                    Ok(LineRead::Line { truncated }) => {
                        append_captured_line(
                            &mut captured,
                            &stdout_line,
                            truncated,
                            &mut capture_truncated,
                        );
                        if !truncated {
                            if let Some(es) = event_stream.as_mut() {
                                forward_line_to_adapter(es, &stdout_line, LogStream::Stdout).await;
                            }
                        }
                    }
                    Ok(LineRead::Eof) | Err(_) => stdout_eof = true,
                }
            },
            line = read_line_capped(&mut stderr_buf, &mut stderr_line, MAX_LINE_BYTES),
                if !stderr_eof =>
            {
                match line {
                    Ok(LineRead::Line { truncated }) => {
                        append_captured_line(
                            &mut captured,
                            &stderr_line,
                            truncated,
                            &mut capture_truncated,
                        );
                        if !truncated {
                            if let Some(es) = event_stream.as_mut() {
                                forward_line_to_adapter(es, &stderr_line, LogStream::Stderr).await;
                            }
                        }
                    }
                    Ok(LineRead::Eof) | Err(_) => stderr_eof = true,
                }
            },
            status = child.wait() => break status.map_err(|e| ControlledCollectError::Io(e.to_string()))?,
        }
    };

    if !stdout_eof {
        drain_after_child_exit(
            &mut stdout_buf,
            &mut stdout_line,
            &mut captured,
            &mut capture_truncated,
            event_stream,
            LogStream::Stdout,
        )
        .await;
    }
    if !stderr_eof {
        drain_after_child_exit(
            &mut stderr_buf,
            &mut stderr_line,
            &mut captured,
            &mut capture_truncated,
            event_stream,
            LogStream::Stderr,
        )
        .await;
    }

    Ok((captured, status))
}

async fn wait_for_optional_cancel(cancel: Option<&CancelToken>) {
    match cancel {
        Some(cancel) => cancel.notified().await,
        None => std::future::pending::<()>().await,
    }
}

async fn drain_after_child_exit<R>(
    reader: &mut R,
    line: &mut String,
    captured: &mut String,
    capture_truncated: &mut bool,
    event_stream: &mut Option<WorkerEventStream>,
    log_stream: LogStream,
) where
    R: AsyncBufRead + Unpin,
{
    loop {
        let read = timeout(
            POST_EXIT_DRAIN_TIMEOUT,
            read_line_capped(reader, line, MAX_LINE_BYTES),
        )
        .await;
        match read {
            Ok(Ok(LineRead::Line { truncated })) => {
                append_captured_line(captured, line, truncated, capture_truncated);
                if !truncated {
                    if let Some(es) = event_stream.as_mut() {
                        forward_line_to_adapter(es, line, log_stream).await;
                    }
                }
            }
            Ok(Ok(LineRead::Eof)) | Ok(Err(_)) | Err(_) => return,
        }
    }
}

fn append_captured_line(
    captured: &mut String,
    line: &str,
    source_truncated: bool,
    capture_truncated: &mut bool,
) {
    if *capture_truncated {
        return;
    }

    let text = line.trim_end_matches(['\r', '\n']);
    let needed = text.len().saturating_add(1);
    if captured.len().saturating_add(needed) <= MAX_CAPTURED_OUTPUT_BYTES {
        captured.push_str(text);
        captured.push('\n');
        if source_truncated {
            append_capture_marker(captured, capture_truncated);
        }
        return;
    }

    let remaining = MAX_CAPTURED_OUTPUT_BYTES.saturating_sub(captured.len());
    if remaining > 0 {
        let prefix_len = safe_prefix_len(text, remaining.saturating_sub(1));
        if prefix_len > 0 {
            captured.push_str(&text[..prefix_len]);
            captured.push('\n');
        }
    }
    append_capture_marker(captured, capture_truncated);
}

fn append_capture_marker(captured: &mut String, capture_truncated: &mut bool) {
    if !*capture_truncated {
        captured.push_str(CAPTURE_TRUNCATED_MARKER);
        *capture_truncated = true;
    }
}

fn safe_prefix_len(s: &str, max_bytes: usize) -> usize {
    if s.len() <= max_bytes {
        return s.len();
    }
    s.char_indices()
        .map(|(idx, _)| idx)
        .take_while(|idx| *idx <= max_bytes)
        .last()
        .unwrap_or(0)
}

/// Build the vendor-specific [`Command`] for the worker run. Splits
/// the playbook injection by vendor:
///
/// - **Claude:** `--append-system-prompt $WORKER_PLAYBOOK` (the CLI
///   supports a dedicated system-prompt slot — keeps the user prompt
///   focused on the task).
/// - **Codex / Gemini:** no equivalent flag; the playbook is
///   prepended to the user prompt with a separator. Less clean but
///   still scopes the worker's behavior.
///
/// Optional per-worker model selection is rendered by the vendor
/// profile's `${model_args}` placeholder (`-m` for Codex, `--model`
/// for Claude/Gemini).
fn build_command(
    vendor: WorkerVendor,
    prompt: &str,
    cwd: &Path,
    path_env: &str,
    model: Option<&str>,
) -> Command {
    let profile = profile_for_vendor(vendor);
    let args = render_command_args(
        profile,
        CommandRole::MissionWorker,
        CommandVars::new(prompt).cwd(cwd).model(model),
    )
    .expect("bundled mission-worker profile command must render");
    let mut cmd = Command::new(&profile.cli_binary);
    cmd.args(args);
    cmd.current_dir(cwd).env("PATH", path_env);
    crate::process_tree::configure(&mut cmd);
    cmd
}

fn compose_prompt(
    objective: &str,
    task_title: &str,
    task_description: Option<&str>,
    revision_directive: Option<&str>,
) -> String {
    let mut out = String::new();
    out.push_str("You are implementing one task as part of a larger mission.\n\n");
    out.push_str("Mission objective:\n");
    out.push_str(objective.trim());
    out.push_str("\n\nYour task:\n");
    out.push_str(task_title.trim());
    if let Some(desc) = task_description {
        let trimmed = desc.trim();
        if !trimmed.is_empty() {
            out.push_str("\n\n");
            out.push_str(trimmed);
        }
    }
    if let Some(directive) = revision_directive {
        out.push_str("\n\nRevision directive: ");
        out.push_str(directive.trim());
        out.push_str(
            "\n\nFocus on this directive. Adjust the parts of your previous submission \
             that the directive calls out; don't rewrite the whole thing.",
        );
    }
    out.push_str(
        "\n\nWhen you're done, exit cleanly. Your final response should be a short \
         (2–4 sentence) plain-prose summary of what you actually did. Vigla will \
         commit your changes on your behalf — do not run git commands yourself.",
    );
    out
}

/// After the worker exits, stage and commit whatever changed. Returns
/// a `WorkerSubmission` shaped for the supervisor's review prompt.
async fn commit_and_synthesize(
    worktree: &Path,
    vendor: WorkerVendor,
    task_title: &str,
    pass_number: u32,
    worker_summary_tail: &str,
    exit_code: Option<i32>,
) -> Result<WorkerSubmission, WorkerDispatchError> {
    // Stage any worker-produced changes.
    git_in(worktree, &["add", "-A"]).await?;

    // Check whether there's anything to commit. Exit code 0 from
    // `git diff --cached --quiet` means no staged diff.
    let diff_quiet_status = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(worktree)
        .status()
        .await
        .map_err(|e| WorkerDispatchError::Io(e.to_string()))?;
    let has_changes = !diff_quiet_status.success();

    if !has_changes {
        let why = match exit_code {
            Some(code) if code != 0 => {
                format!("worker exited with code {code} and produced no committable changes")
            }
            _ => "worker exited cleanly but produced no committable changes".into(),
        };
        return Ok(WorkerSubmission {
            files: Vec::new(),
            summary: why.clone(),
            commit_message: why,
            produced_changes: false,
        });
    }

    let title_suffix = if pass_number == 0 {
        String::new()
    } else {
        format!(" (revision pass {pass_number})")
    };
    let commit_subject = format!("{} [{}]{}", task_title, vendor.binary(), title_suffix);
    let commit_body = if worker_summary_tail.is_empty() {
        commit_subject.clone()
    } else {
        format!("{commit_subject}\n\n{worker_summary_tail}")
    };
    git_in(worktree, &["commit", "-m", &commit_body]).await?;

    // Collect the file list from HEAD~1..HEAD. The branch was at
    // `parent_ref` before this commit; using HEAD~1 is more robust
    // than threading the parent ref through the API.
    let files = git_files_in_head_commit(worktree).await?;
    let summary = format!(
        "{} {} {} ({} {} changed)",
        task_title,
        if pass_number == 0 {
            "submitted"
        } else {
            "revised"
        },
        if let Some(code) = exit_code {
            format!("(exit {code})")
        } else {
            String::new()
        },
        files.len(),
        if files.len() == 1 { "file" } else { "files" },
    );

    Ok(WorkerSubmission {
        files,
        summary: format!(
            "{}: {}",
            summary.trim(),
            if worker_summary_tail.is_empty() {
                "(no worker summary captured)"
            } else {
                worker_summary_tail
            }
        ),
        commit_message: commit_body,
        produced_changes: true,
    })
}

async fn git_files_in_head_commit(worktree: &Path) -> Result<Vec<String>, WorkerDispatchError> {
    let out = Command::new("git")
        .args(["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"])
        .current_dir(worktree)
        .output()
        .await
        .map_err(|e| WorkerDispatchError::Io(e.to_string()))?;
    if !out.status.success() {
        return Err(WorkerDispatchError::Git(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect())
}

async fn git_in(cwd: &Path, args: &[&str]) -> Result<(), WorkerDispatchError> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .map_err(|e| WorkerDispatchError::Io(e.to_string()))?;
    if !out.status.success() {
        return Err(WorkerDispatchError::Git(format!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// Take the last `max_bytes` of `text`, trimming whitespace, suitable
/// for embedding in a commit message body or supervisor review
/// prompt. CLI workers tend to print their final summary near the
/// end of stdout.
fn tail_summary(text: &str, max_bytes: usize) -> String {
    let trimmed = text.trim();
    if trimmed.len() <= max_bytes {
        return trimmed.to_owned();
    }
    let start = trimmed.len() - max_bytes;
    // Don't cut a unicode char in half.
    let safe_start = trimmed
        .char_indices()
        .find(|(i, _)| *i >= start)
        .map(|(i, _)| i)
        .unwrap_or(start);
    format!("…{}", &trimmed[safe_start..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::WorkerEventSink;
    use crate::repository::Repository;
    use adapter_core::{AdapterExit, MemoryIntent, ProposeIntent, ScopeIntent};
    use event_schema::{
        Event, EventKind, Log, LogLevel, LogStream as EventLogStream, SCHEMA_VERSION,
    };
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;

    /// Test adapter: emits one Log event per line; finalize records the
    /// exit signal and emits a terminal log so the test can assert the
    /// supervise loop calls finalize at the right moment with the right
    /// classification.
    #[derive(Debug)]
    struct LineCountingAdapter {
        worker_id: String,
        seq: u64,
        finalize_called: Arc<StdMutex<Option<AdapterExit>>>,
    }

    impl adapter_core::Adapter for LineCountingAdapter {
        fn ingest_line(&mut self, line: &str, stream: EventLogStream) -> Vec<Event> {
            self.seq += 1;
            vec![Event {
                schema_version: SCHEMA_VERSION.into(),
                worker_id: self.worker_id.clone(),
                task_id: None,
                seq: self.seq,
                ts: format!("2026-05-15T00:00:00.{:03}Z", self.seq.min(999)),
                kind: EventKind::Log(Log {
                    level: LogLevel::Info,
                    stream,
                    line: line.to_owned(),
                    tag: None,
                }),
            }]
        }

        fn finalize(&mut self, exit: AdapterExit) -> Vec<Event> {
            *self.finalize_called.lock().unwrap() = Some(exit);
            self.seq += 1;
            vec![Event {
                schema_version: SCHEMA_VERSION.into(),
                worker_id: self.worker_id.clone(),
                task_id: None,
                seq: self.seq,
                ts: format!("2026-05-15T00:01:00.{:03}Z", self.seq.min(999)),
                kind: EventKind::Log(Log {
                    level: LogLevel::Info,
                    stream: EventLogStream::Stdout,
                    line: format!("finalize:{exit:?}"),
                    tag: Some("finalize".into()),
                }),
            }]
        }
    }

    #[derive(Debug, Default)]
    struct CapturingSink {
        events: StdMutex<Vec<Event>>,
    }

    impl WorkerEventSink for CapturingSink {
        fn emit(&self, event: &Event) {
            self.events.lock().unwrap().push(event.clone());
        }
    }

    #[derive(Debug, Default)]
    struct CapturingMemorySink {
        intents: StdMutex<Vec<MemoryIntent>>,
    }

    impl crate::memory::MemoryIntentSink for CapturingMemorySink {
        fn emit(&self, intent: MemoryIntent) {
            self.intents.lock().unwrap().push(intent);
        }
    }

    #[derive(Debug)]
    struct IntentOnlyAdapter {
        pending: Vec<MemoryIntent>,
    }

    impl adapter_core::Adapter for IntentOnlyAdapter {
        fn ingest_line(&mut self, line: &str, _stream: EventLogStream) -> Vec<Event> {
            self.pending.push(MemoryIntent::Propose(ProposeIntent {
                kind: "hazard".into(),
                scope: ScopeIntent {
                    kind: "repo".into(),
                    value: None,
                },
                body: line.to_owned(),
                derived_from: vec!["worktree:README.md:1".into()],
                evidence_event_ids: vec![],
            }));
            Vec::new()
        }

        fn take_memory_intents(&mut self) -> Vec<MemoryIntent> {
            std::mem::take(&mut self.pending)
        }
    }

    fn make_stream(
        worker_id: &str,
        sink: Arc<CapturingSink>,
        repo: Option<Repository>,
        finalize_called: Arc<StdMutex<Option<AdapterExit>>>,
    ) -> WorkerEventStream {
        WorkerEventStream {
            worker_id: worker_id.to_owned(),
            vendor: event_schema::Vendor::Claude,
            adapter: Box::new(LineCountingAdapter {
                worker_id: worker_id.to_owned(),
                seq: 0,
                finalize_called,
            }),
            repo,
            sink: sink as Arc<dyn WorkerEventSink>,
            memory_sink: None,
            signal_sink: None,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn event_stream_forwards_each_stdout_line_through_adapter() {
        let sink: Arc<CapturingSink> = Arc::new(CapturingSink::default());
        let finalize_called = Arc::new(StdMutex::new(None));
        let mut stream = Some(make_stream(
            "test-worker-stdout",
            sink.clone(),
            None,
            finalize_called.clone(),
        ));

        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("printf 'alpha\\nbeta\\ngamma\\n'")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn shell");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");

        let (captured, status) =
            collect_child_output_and_status(child, stdout, stderr, &mut stream)
                .await
                .expect("collect output");

        assert!(status.success());
        assert!(captured.contains("alpha"));
        let lines: Vec<String> = sink
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::Log(log) => Some(log.line.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(lines, vec!["alpha", "beta", "gamma"]);
        assert!(
            finalize_called.lock().unwrap().is_none(),
            "collect should not invoke finalize itself; run_real_worker drives that"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn event_stream_forwards_stderr_lines_through_adapter() {
        let sink: Arc<CapturingSink> = Arc::new(CapturingSink::default());
        let finalize_called = Arc::new(StdMutex::new(None));
        let mut stream = Some(make_stream(
            "test-worker-stderr",
            sink.clone(),
            None,
            finalize_called.clone(),
        ));

        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("printf 'oops\\n' 1>&2; printf 'ok\\n'")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn shell");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");

        let (_captured, status) =
            collect_child_output_and_status(child, stdout, stderr, &mut stream)
                .await
                .expect("collect output");

        assert!(status.success());
        let stderr_lines: Vec<String> = sink
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::Log(log) if log.stream == EventLogStream::Stderr => {
                    Some(log.line.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(stderr_lines, vec!["oops"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn event_stream_persists_events_through_repository_when_provided() {
        let repo = Repository::open_in_memory().await.expect("open repo");
        let sink: Arc<CapturingSink> = Arc::new(CapturingSink::default());
        let finalize_called = Arc::new(StdMutex::new(None));
        let worker_id = "test-worker-persist";
        let mut stream = Some(make_stream(
            worker_id,
            sink.clone(),
            Some(repo.clone()),
            finalize_called.clone(),
        ));

        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("printf 'one\\ntwo\\n'")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn shell");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");

        let (_captured, status) =
            collect_child_output_and_status(child, stdout, stderr, &mut stream)
                .await
                .expect("collect output");

        assert!(status.success());
        let persisted = repo.replay_for_worker(worker_id).await.expect("replay");
        assert_eq!(persisted.len(), 2);
        assert_eq!(sink.events.lock().unwrap().len(), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn event_stream_drains_memory_intents_after_adapter_ingest() {
        let sink: Arc<CapturingSink> = Arc::new(CapturingSink::default());
        let memory_sink: Arc<CapturingMemorySink> = Arc::new(CapturingMemorySink::default());
        let mut stream = Some(WorkerEventStream {
            worker_id: "test-worker-memory".into(),
            vendor: event_schema::Vendor::Claude,
            adapter: Box::new(IntentOnlyAdapter {
                pending: Vec::new(),
            }),
            repo: None,
            sink: sink as Arc<dyn WorkerEventSink>,
            memory_sink: Some(memory_sink.clone() as Arc<dyn crate::memory::MemoryIntentSink>),
            signal_sink: None,
        });

        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("printf 'remember host-bound sessions\\n'")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn shell");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");

        let (_captured, status) =
            collect_child_output_and_status(child, stdout, stderr, &mut stream)
                .await
                .expect("collect output");

        assert!(status.success());
        let intents = memory_sink.intents.lock().unwrap();
        assert_eq!(intents.len(), 1);
        let MemoryIntent::Propose(p) = &intents[0];
        assert_eq!(p.body, "remember host-bound sessions");
    }

    /// Test adapter that emits a `QuotaSignal` after seeing the magic
    /// line "QUOTA_HIT". Mirrors the way real adapters stash a signal
    /// in `pending_quota_signal` after matching a vendor error pattern.
    #[derive(Debug, Default)]
    struct QuotaEmittingAdapter {
        pending_quota: Option<adapter_core::QuotaSignal>,
        pending_context: Vec<adapter_core::ContextRequestSignal>,
    }

    impl adapter_core::Adapter for QuotaEmittingAdapter {
        fn ingest_line(&mut self, line: &str, _stream: EventLogStream) -> Vec<Event> {
            if line.contains("QUOTA_HIT") {
                self.pending_quota = Some(adapter_core::QuotaSignal {
                    estimated_reset_at_ms: Some(1_716_000_000_000),
                });
            }
            if line.contains("NEEDS_FILE") {
                self.pending_context
                    .push(adapter_core::ContextRequestSignal {
                        kind: adapter_core::ContextRequestSignalKind::FileContent,
                        detail: "src/foo.rs".into(),
                    });
            }
            Vec::new()
        }

        fn take_quota_signal(&mut self) -> Option<adapter_core::QuotaSignal> {
            self.pending_quota.take()
        }

        fn take_context_requests(&mut self) -> Vec<adapter_core::ContextRequestSignal> {
            std::mem::take(&mut self.pending_context)
        }
    }

    /// C1 regression: a quota signal raised mid-pass by the adapter
    /// must reach the supervisor's `PassSignalSink` rather than
    /// dying in the adapter's `pending_quota_signal` slot. The
    /// pre-pass `tracker.is_exhausted` guard only catches state from
    /// prior runs; this test pins the per-line drain that catches the
    /// first occurrence.
    #[tokio::test(flavor = "current_thread")]
    async fn event_stream_drains_quota_signal_into_pass_signal_sink() {
        let sink: Arc<CapturingSink> = Arc::new(CapturingSink::default());
        let signal_sink = Arc::new(crate::mission_supervisor_run::PassSignalSink::default());
        let mut stream = Some(WorkerEventStream {
            worker_id: "test-worker-quota".into(),
            vendor: event_schema::Vendor::Codex,
            adapter: Box::new(QuotaEmittingAdapter::default()),
            repo: None,
            sink: sink as Arc<dyn WorkerEventSink>,
            memory_sink: None,
            signal_sink: Some(signal_sink.clone()),
        });

        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("printf 'starting\\nQUOTA_HIT vendor rejected\\n'")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn shell");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");

        let (_captured, status) =
            collect_child_output_and_status(child, stdout, stderr, &mut stream)
                .await
                .expect("collect output");
        assert!(status.success());

        let drained = signal_sink.quota.lock().await;
        assert_eq!(
            drained.len(),
            1,
            "quota signal must be drained into the PassSignalSink"
        );
        assert_eq!(
            drained[0].vendor,
            event_schema::Vendor::Codex,
            "drained signal must inherit vendor from WorkerEventStream"
        );
        assert_eq!(drained[0].estimated_reset_at_ms, Some(1_716_000_000_000));
    }

    /// C1 regression: same idea, for context-request signals. The
    /// adapter's pending vector must drain into the PassSignalSink so
    /// the supervisor's recovery engine can issue a
    /// `RequestSupervisor::NeedsContext`.
    #[tokio::test(flavor = "current_thread")]
    async fn event_stream_drains_context_requests_into_pass_signal_sink() {
        let sink: Arc<CapturingSink> = Arc::new(CapturingSink::default());
        let signal_sink = Arc::new(crate::mission_supervisor_run::PassSignalSink::default());
        let mut stream = Some(WorkerEventStream {
            worker_id: "test-worker-context".into(),
            vendor: event_schema::Vendor::Gemini,
            adapter: Box::new(QuotaEmittingAdapter::default()),
            repo: None,
            sink: sink as Arc<dyn WorkerEventSink>,
            memory_sink: None,
            signal_sink: Some(signal_sink.clone()),
        });

        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("printf 'NEEDS_FILE src/foo.rs\\n'")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn shell");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");

        let (_captured, status) =
            collect_child_output_and_status(child, stdout, stderr, &mut stream)
                .await
                .expect("collect output");
        assert!(status.success());

        let drained = signal_sink.context.lock().await;
        assert_eq!(drained.len(), 1);
        assert_eq!(
            drained[0].kind,
            crate::recovery::types::ContextRequestKind::FileContent
        );
        assert_eq!(drained[0].detail, "src/foo.rs");
    }

    /// C1: signal drain is a no-op when no `signal_sink` is installed,
    /// preserving existing behaviour for standalone supervisor callers
    /// that don't run the mission recovery pipeline.
    #[tokio::test(flavor = "current_thread")]
    async fn event_stream_signal_drain_is_noop_without_sink() {
        let sink: Arc<CapturingSink> = Arc::new(CapturingSink::default());
        let mut stream = Some(WorkerEventStream {
            worker_id: "test-worker-no-sink".into(),
            vendor: event_schema::Vendor::Claude,
            adapter: Box::new(QuotaEmittingAdapter::default()),
            repo: None,
            sink: sink as Arc<dyn WorkerEventSink>,
            memory_sink: None,
            signal_sink: None,
        });

        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("printf 'QUOTA_HIT\\n'")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn shell");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");

        // Just assert this does not panic / hang.
        let (_captured, status) =
            collect_child_output_and_status(child, stdout, stderr, &mut stream)
                .await
                .expect("collect output");
        assert!(status.success());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn collect_without_event_stream_preserves_capture_only_behavior() {
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("printf 'hello world\\n'")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn shell");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");

        let mut no_stream: Option<WorkerEventStream> = None;
        let (captured, status) =
            collect_child_output_and_status(child, stdout, stderr, &mut no_stream)
                .await
                .expect("collect output");

        assert!(status.success());
        assert!(captured.contains("hello world"));
    }

    #[test]
    fn parse_vendor_recognizes_each_name_case_insensitively() {
        assert_eq!(WorkerVendor::parse("claude"), Some(WorkerVendor::Claude));
        assert_eq!(WorkerVendor::parse("CLAUDE"), Some(WorkerVendor::Claude));
        assert_eq!(WorkerVendor::parse(" codex "), Some(WorkerVendor::Codex));
        assert_eq!(WorkerVendor::parse("Gemini"), Some(WorkerVendor::Gemini));
        assert_eq!(WorkerVendor::parse("auto"), None);
        assert_eq!(WorkerVendor::parse(""), None);
        assert_eq!(WorkerVendor::parse("opencode"), None);
    }

    #[test]
    fn parse_vendor_returns_none_for_unknown() {
        assert_eq!(WorkerVendor::parse("nope"), None);
    }

    #[test]
    fn compose_prompt_includes_objective_task_and_directive_when_present() {
        let p = compose_prompt(
            "Add a logout endpoint",
            "Implement /api/logout",
            Some("Invalidate the session token on logout"),
            Some("Cover the case where the session is already expired"),
        );
        assert!(p.contains("Mission objective"));
        assert!(p.contains("Add a logout endpoint"));
        assert!(p.contains("Implement /api/logout"));
        assert!(p.contains("Invalidate the session token on logout"));
        assert!(p.contains("Revision directive"));
        assert!(p.contains("already expired"));
        assert!(p.contains("do not run git commands"));
    }

    #[test]
    fn compose_prompt_omits_description_and_directive_when_absent() {
        let p = compose_prompt("Obj", "Task", None, None);
        assert!(p.contains("Obj"));
        assert!(p.contains("Task"));
        assert!(!p.contains("Revision directive"));
    }

    #[test]
    fn tail_summary_handles_short_text_idempotently() {
        let s = tail_summary("short text", 100);
        assert_eq!(s, "short text");
    }

    #[test]
    fn tail_summary_truncates_long_text_with_leading_marker() {
        let long = "a".repeat(500);
        let s = tail_summary(&long, 100);
        assert!(s.starts_with('…'));
        assert!(s.len() <= 105); // 100 ascii + the '…' marker
    }

    #[test]
    fn tail_summary_respects_unicode_boundaries() {
        // String with non-ASCII characters near the cut point.
        let mut s = "x".repeat(200);
        s.push_str("é字é字é字");
        let out = tail_summary(&s, 8);
        // Must be valid UTF-8; no panic on slicing.
        assert!(out.starts_with('…'));
    }

    #[test]
    fn worker_vendor_binary_matches_cli_name() {
        assert_eq!(WorkerVendor::Claude.binary(), "claude");
        assert_eq!(WorkerVendor::Codex.binary(), "codex");
        assert_eq!(WorkerVendor::Gemini.binary(), "gemini");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn collect_child_output_handles_stderr_eof_without_spinning() {
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("exec 2>&-; printf 'stdout survives\\n'")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn shell");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");

        let mut no_stream: Option<WorkerEventStream> = None;
        let (captured, status) = timeout(
            Duration::from_secs(1),
            collect_child_output_and_status(child, stdout, stderr, &mut no_stream),
        )
        .await
        .expect("stderr EOF must not spin until timeout")
        .expect("collect output");

        assert!(status.success(), "shell should exit successfully");
        assert!(captured.contains("stdout survives"), "{captured}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn collect_child_output_returns_after_child_exits_even_if_grandchild_holds_pipes() {
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("sleep 3 & echo pid:$!; printf 'child done\\n'")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn shell");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");

        let mut no_stream: Option<WorkerEventStream> = None;
        let (captured, status) = timeout(
            Duration::from_secs(1),
            collect_child_output_and_status(child, stdout, stderr, &mut no_stream),
        )
        .await
        .expect("grandchild-held pipe must not stall until worker timeout")
        .expect("collect output");

        if let Some(pid) = captured.lines().find_map(|line| line.strip_prefix("pid:")) {
            let _ = Command::new("kill").arg(pid).status().await;
        }

        assert!(status.success(), "shell should exit successfully");
        assert!(captured.contains("child done"), "{captured}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn collect_child_output_caps_noisy_worker_capture() {
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg("i=0; while [ $i -lt 12000 ]; do printf 'abcdefghijabcdefghijabcdefghijabcdefghij\\n'; i=$((i+1)); done")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn shell");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");

        let mut no_stream: Option<WorkerEventStream> = None;
        let (captured, status) =
            collect_child_output_and_status(child, stdout, stderr, &mut no_stream)
                .await
                .expect("collect output");

        assert!(status.success(), "shell should exit successfully");
        assert!(captured.contains(CAPTURE_TRUNCATED_MARKER), "{captured}");
        assert!(
            captured.len() <= MAX_CAPTURED_OUTPUT_BYTES + CAPTURE_TRUNCATED_MARKER.len(),
            "captured output should stay bounded; len={}",
            captured.len()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn real_dispatch_never_commits_injected_context_but_keeps_worker_edits() {
        use std::os::unix::fs::PermissionsExt;

        for existing_native in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let git = |args: &[&str]| {
                let output = std::process::Command::new("git")
                    .args(args)
                    .current_dir(root)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "git {args:?}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            };
            git(&["init", "--initial-branch=main"]);
            git(&["config", "user.name", "Vigla Test"]);
            git(&["config", "user.email", "test@vigla.local"]);
            git(&["config", "commit.gpgsign", "false"]);
            std::fs::write(root.join("README.md"), "fixture\n").unwrap();
            if existing_native {
                std::fs::write(root.join("CLAUDE.md"), "project instructions\n").unwrap();
            }
            git(&["add", "."]);
            git(&["commit", "-m", "initial"]);

            let mut snapshot = crate::ephemeral_context::EphemeralContextSnapshot::capture(root)
                .await
                .unwrap();
            crate::memory::write_anchor_block(
                &root.join("CLAUDE.md"),
                crate::memory::MEMORY_ANCHOR_OPEN,
                crate::memory::MEMORY_ANCHOR_CLOSE,
                "private memory",
            )
            .await
            .unwrap();
            crate::memory::write_anchor_block(
                &root.join("CLAUDE.md"),
                crate::skills::SKILLS_ANCHOR_OPEN,
                crate::skills::SKILLS_ANCHOR_CLOSE,
                "private skill",
            )
            .await
            .unwrap();
            snapshot.seal(root).await.unwrap();

            let bin_dir = root.join("bin");
            std::fs::create_dir_all(&bin_dir).unwrap();
            let script = bin_dir.join("claude");
            let native_edit = if existing_native {
                "printf 'worker-authored note\\n' >> CLAUDE.md\n"
            } else {
                ""
            };
            std::fs::write(
                &script,
                format!("#!/bin/sh\n{native_edit}printf 'real work\\n' > src.txt\nprintf done\n"),
            )
            .unwrap();
            let mut permissions = std::fs::metadata(&script).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&script, permissions).unwrap();
            let path_env = format!(
                "{}:{}",
                bin_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            );

            let submission = run_real_worker_controlled(
                WorkerVendor::Claude,
                root.to_path_buf(),
                "objective",
                "task",
                None,
                None,
                0,
                None,
                &path_env,
                Duration::from_secs(5),
                None,
                None,
                Some(snapshot),
            )
            .await
            .unwrap();
            assert!(submission.files.contains(&"src.txt".to_string()));
            assert_eq!(
                submission.files.contains(&"CLAUDE.md".to_string()),
                existing_native,
                "only a genuine worker edit may put the native file in the submission"
            );

            let show = std::process::Command::new("git")
                .args(["show", "--format=", "--stat", "HEAD"])
                .current_dir(root)
                .output()
                .unwrap();
            let committed = String::from_utf8_lossy(&show.stdout);
            assert!(!committed.contains("vigla:memory"));
            assert!(!committed.contains("vigla:skills"));
            assert!(!committed.contains("private memory"));
            assert!(!committed.contains("private skill"));
            if existing_native {
                let native = std::fs::read_to_string(root.join("CLAUDE.md")).unwrap();
                assert!(native.contains("project instructions"));
                assert!(native.contains("worker-authored note"));
                assert!(!native.contains("vigla:"));
            } else {
                assert!(!root.join("CLAUDE.md").exists());
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_kills_worker_process_group_before_returning() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let descendant_pid_file = dir.path().join("descendant-pid");
        let script = bin_dir.join("claude");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nsleep 30 &\nprintf '%s' \"$!\" > '{}'\nwait\n",
                descendant_pid_file.display(),
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).unwrap();

        let cancel = CancelToken::new();
        let run_cancel = Arc::clone(&cancel);
        let worktree = dir.path().to_path_buf();
        let path_env = format!(
            "{}:{}",
            bin_dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let run = tokio::spawn(async move {
            run_real_worker_controlled(
                WorkerVendor::Claude,
                worktree,
                "objective",
                "task",
                None,
                None,
                0,
                None,
                &path_env,
                Duration::from_secs(30),
                None,
                Some(run_cancel),
                None,
            )
            .await
        });

        let descendant_pid = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Ok(contents) = std::fs::read_to_string(&descendant_pid_file) {
                    if let Ok(pid) = contents.parse::<i32>() {
                        break pid;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fake CLI did not publish its descendant PID");
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(2), run)
            .await
            .expect("worker cancellation was not prompt")
            .expect("worker task panicked");
        assert!(matches!(result, Err(WorkerDispatchError::Cancelled)));

        tokio::time::timeout(Duration::from_secs(2), async {
            while unsafe { libc::kill(descendant_pid, 0) } == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancelled worker left a descendant alive");
    }
}
