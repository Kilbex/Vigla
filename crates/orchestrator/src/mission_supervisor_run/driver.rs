use crate::mission_runtime::CancelToken;
use crate::mission_worker_dispatch::WorkerVendor;
use crate::parser::{read_line_resumable, LineRead, MAX_LINE_BYTES};
use crate::vendor_profile::{profile_for_vendor, render_command_args, CommandRole, CommandVars};
use std::collections::VecDeque;
use std::path::Path;
use std::process::ExitStatus;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use supervisor_adapter::{SupervisorAdapter, SupervisorOutput};
use tokio::io::{AsyncBufRead, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::sync::Mutex;

/// Hard wall-clock cap per supervisor turn. Claude in `-p` mode with
/// `--max-turns 4` typically completes in 1–15 seconds; 90s is a wide
/// margin without letting a hung subprocess pin the mission.
const DEFAULT_TURN_TIMEOUT: Duration = Duration::from_secs(90);
const SUPERVISOR_POST_EXIT_DRAIN_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_SUPERVISOR_STDOUT_LOGS: usize = 64;
const MAX_SUPERVISOR_STDERR_LOGS: usize = 64;

/// What [`SupervisorDriver::run_turn`] returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorTurnResult {
    pub outputs: Vec<SupervisorOutput>,
    /// `Some` if this turn observed a `system/init` line carrying a
    /// session id. The orchestrator forwards it into the next
    /// `run_turn` call as the conversation-resumption handle.
    pub session_id: Option<String>,
}

/// Strategy for getting a supervisor decision out of a prompt.
///
/// `RealClaude` shells out; `Scripted` returns pre-canned outputs
/// (used by tests and by callers that want a deterministic mission
/// for demos).
#[derive(Debug)]
pub enum SupervisorDriver {
    RealClaude(RealClaudeConfig),
    Scripted(ScriptedSupervisor),
}

/// Configuration for the real-Claude driver.
#[derive(Debug, Clone)]
pub struct RealClaudeConfig {
    /// Path or name of the `claude` binary. Defaults to `"claude"`
    /// (resolved via the user's PATH per `resolve_user_path`).
    pub binary: String,
    /// Optional `--model` override.
    pub model: Option<String>,
    /// Per-turn wall-clock timeout.
    pub turn_timeout: Duration,
}

impl Default for RealClaudeConfig {
    fn default() -> Self {
        Self {
            binary: "claude".into(),
            model: None,
            turn_timeout: DEFAULT_TURN_TIMEOUT,
        }
    }
}

/// Scripted supervisor: returns pre-loaded outputs per turn.
///
/// Useful for:
/// - Unit tests of `run_supervisor_mission` end-to-end without
///   spawning Claude.
/// - Demo "replay" missions that deterministically exercise every
///   branch (accept / revise / reject) for documentation captures.
#[derive(Debug, Clone)]
pub struct ScriptedSupervisor {
    /// Each `Vec<SupervisorOutput>` is one turn's outputs. The first
    /// element is consumed on the first `run_turn`, etc.
    turns: Arc<Mutex<VecDeque<Vec<SupervisorOutput>>>>,
    /// Optional fixed session id returned by every turn.
    session_id: Option<String>,
    /// Prompts captured for test assertions.
    pub(super) captured_prompts: Arc<Mutex<Vec<String>>>,
    /// Match the production supervisor contract by requiring one semantic
    /// review turn per successful worker pass. Off by default so small tests
    /// can use the automated arbiter without scripting conversational turns.
    semantic_reviews: bool,
}

impl ScriptedSupervisor {
    pub fn new(turns: Vec<Vec<SupervisorOutput>>) -> Self {
        Self {
            turns: Arc::new(Mutex::new(VecDeque::from(turns))),
            session_id: Some("scripted-session".into()),
            captured_prompts: Arc::new(Mutex::new(Vec::new())),
            semantic_reviews: false,
        }
    }

    /// Require the same per-submission semantic-review turns as the real
    /// supervisor. Intended for end-to-end tests of review/rework behavior.
    pub fn with_semantic_reviews(mut self) -> Self {
        self.semantic_reviews = true;
        self
    }

    /// Drain the prompts the scripted supervisor was handed so far.
    /// Used by tests that want to assert the orchestrator phrased
    /// review prompts correctly.
    pub async fn captured_prompts(&self) -> Vec<String> {
        self.captured_prompts.lock().await.clone()
    }
}

impl SupervisorDriver {
    pub(super) fn requires_semantic_review(&self) -> bool {
        match self {
            Self::RealClaude(_) => true,
            Self::Scripted(scripted) => scripted.semantic_reviews,
        }
    }

    /// Drive one supervisor turn. Returns whatever the supervisor
    /// produced; the caller decides how to respond.
    pub async fn run_turn(
        &mut self,
        prompt: &str,
        session_id: Option<&str>,
        cwd: &Path,
    ) -> SupervisorTurnResult {
        self.run_turn_cancellable(prompt, session_id, cwd, None)
            .await
    }

    pub(super) async fn run_turn_cancellable(
        &mut self,
        prompt: &str,
        session_id: Option<&str>,
        cwd: &Path,
        cancel: Option<&CancelToken>,
    ) -> SupervisorTurnResult {
        match self {
            Self::RealClaude(cfg) => {
                run_real_claude_turn(cfg, prompt, session_id, cwd, cancel).await
            }
            Self::Scripted(s) => {
                if cancel.is_some_and(CancelToken::is_cancelled) {
                    return SupervisorTurnResult {
                        outputs: vec![SupervisorOutput::Error("supervisor turn cancelled".into())],
                        session_id: None,
                    };
                }
                s.captured_prompts.lock().await.push(prompt.to_owned());
                let outputs = s
                    .turns
                    .lock()
                    .await
                    .pop_front()
                    .unwrap_or_else(|| vec![SupervisorOutput::NoIntent]);
                SupervisorTurnResult {
                    outputs,
                    session_id: s.session_id.clone(),
                }
            }
        }
    }
}

/// Tools disabled for the supervisor process. Bash / Edit / Write /
/// MultiEdit stay off because the supervisor is judgment-only — the
/// orchestrator does all I/O. Read / Glob / LS are deliberately
/// allowed so the supervisor can survey the user's codebase before
/// decomposing (see `Codebase discovery` in the supervisor playbook
/// and the `supervisor_disallowed_tools_*` unit tests). WebFetch /
/// WebSearch stay off so the supervisor stays in-repo.
pub(super) const SUPERVISOR_DISALLOWED_TOOLS: &str =
    "Bash,Edit,Write,MultiEdit,Grep,WebFetch,WebSearch,Task,TodoWrite,NotebookEdit";

/// Per-turn max-turns ceiling for the supervisor process. Each tool
/// call consumes a turn; the discover-then-decompose flow on turn 1
/// needs ~4–6 reads/lists plus the final JSON. 8 is the comfortable
/// upper bound. Bumped from 4 in QC-1.
pub(super) const SUPERVISOR_MAX_TURNS: u32 = 8;

/// Spawn `claude -p` once for a single supervisor turn. Returns
/// whatever the adapter produced. The caller composes the prompt;
/// this function knows nothing about mission semantics.
async fn run_real_claude_turn(
    cfg: &RealClaudeConfig,
    prompt: &str,
    session_id: Option<&str>,
    cwd: &Path,
    cancel: Option<&CancelToken>,
) -> SupervisorTurnResult {
    let profile = profile_for_vendor(WorkerVendor::Claude);
    let args = match render_command_args(
        profile,
        CommandRole::Supervisor,
        CommandVars::new(prompt).supervisor(
            SUPERVISOR_DISALLOWED_TOOLS,
            SUPERVISOR_MAX_TURNS,
            session_id,
            cfg.model.as_deref(),
        ),
    ) {
        Ok(args) => args,
        Err(e) => {
            return SupervisorTurnResult {
                outputs: vec![SupervisorOutput::Error(format!(
                    "failed to render supervisor command profile: {e}"
                ))],
                session_id: None,
            };
        }
    };

    let binary = if cfg.binary == WorkerVendor::Claude.binary() {
        profile.cli_binary.as_str()
    } else {
        cfg.binary.as_str()
    };
    let mut cmd = Command::new(binary);
    cmd.args(args)
        .current_dir(cwd)
        .env("PATH", crate::resolve_user_path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::process_tree::configure(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return SupervisorTurnResult {
                outputs: vec![SupervisorOutput::Error(format!(
                    "failed to spawn supervisor process: {e}"
                ))],
                session_id: None,
            };
        }
    };

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    collect_supervisor_turn_cancellable(child, stdout, stderr, cfg.turn_timeout, cancel).await
}

#[cfg(test)]
pub(super) async fn collect_supervisor_turn(
    child: Child,
    stdout: ChildStdout,
    stderr: ChildStderr,
    turn_timeout: Duration,
) -> SupervisorTurnResult {
    collect_supervisor_turn_cancellable(child, stdout, stderr, turn_timeout, None).await
}

async fn collect_supervisor_turn_cancellable(
    mut child: Child,
    stdout: ChildStdout,
    stderr: ChildStderr,
    turn_timeout: Duration,
    cancel: Option<&CancelToken>,
) -> SupervisorTurnResult {
    let mut adapter = SupervisorAdapter::new();
    let mut outputs = Vec::new();

    enum WaitResult {
        Finished(Option<String>),
        Cancelled,
        TimedOut,
    }
    let result = {
        let turn =
            collect_supervisor_turn_inner(&mut child, stdout, stderr, &mut adapter, &mut outputs);
        tokio::pin!(turn);
        tokio::select! {
            biased;
            _ = wait_for_optional_cancel(cancel) => WaitResult::Cancelled,
            sid = &mut turn => WaitResult::Finished(sid),
            _ = tokio::time::sleep(turn_timeout) => WaitResult::TimedOut,
        }
    };
    let sid = match result {
        WaitResult::Finished(sid) => sid,
        WaitResult::Cancelled => {
            crate::process_tree::terminate_and_reap(&mut child).await;
            outputs.push(SupervisorOutput::Error("supervisor turn cancelled".into()));
            None
        }
        WaitResult::TimedOut => {
            crate::process_tree::terminate_and_reap(&mut child).await;
            outputs.push(SupervisorOutput::Error(format!(
                "supervisor turn exceeded timeout {turn_timeout:?}"
            )));
            None
        }
    };

    SupervisorTurnResult {
        outputs,
        session_id: sid,
    }
}

async fn wait_for_optional_cancel(cancel: Option<&CancelToken>) {
    match cancel {
        Some(cancel) => cancel.notified().await,
        None => std::future::pending::<()>().await,
    }
}

pub(super) async fn collect_supervisor_turn_inner(
    child: &mut Child,
    stdout: ChildStdout,
    stderr: ChildStderr,
    adapter: &mut SupervisorAdapter,
    outputs: &mut Vec<SupervisorOutput>,
) -> Option<String> {
    let mut stdout_buf = BufReader::new(stdout);
    let mut stderr_buf = BufReader::new(stderr);
    let mut stdout_line = String::new();
    let mut stderr_line = String::new();
    // Persistent per-stream accumulators keep the select! reads cancel-safe:
    // a read arm dropped mid-line (because another arm won) leaves its
    // consumed bytes here for the next read to resume from.
    let mut stdout_partial: Vec<u8> = Vec::new();
    let mut stderr_partial: Vec<u8> = Vec::new();
    let mut stdout_eof = false;
    let mut stderr_eof = false;
    let mut stdout_logs = 0usize;
    let mut stderr_logs = 0usize;

    loop {
        if stdout_eof && stderr_eof {
            match child.wait().await {
                Ok(status) => push_supervisor_exit_status(outputs, status),
                Err(e) => {
                    outputs.push(SupervisorOutput::Error(format!(
                        "supervisor process wait failed: {e}"
                    )));
                }
            }
            break;
        }

        tokio::select! {
            line = read_line_resumable(&mut stdout_buf, &mut stdout_partial, &mut stdout_line, MAX_LINE_BYTES),
                if !stdout_eof =>
            {
                match line {
                    Ok(LineRead::Line { truncated }) => {
                        if truncated {
                            push_supervisor_stdout_outputs(
                                outputs,
                                vec![SupervisorOutput::Log(
                                    "supervisor stdout line exceeded Vigla cap and was dropped".into(),
                                )],
                                &mut stdout_logs,
                            );
                        } else {
                            let line = stdout_line.trim_end_matches(['\r', '\n']);
                            push_supervisor_stdout_outputs(
                                outputs,
                                adapter.ingest_line(line),
                                &mut stdout_logs,
                            );
                        }
                    }
                    Ok(LineRead::Eof) | Err(_) => stdout_eof = true,
                }
            }
            line = read_line_resumable(&mut stderr_buf, &mut stderr_partial, &mut stderr_line, MAX_LINE_BYTES),
                if !stderr_eof =>
            {
                match line {
                    Ok(LineRead::Line { truncated }) => push_supervisor_stderr_log(
                        outputs,
                        &stderr_line,
                        truncated,
                        &mut stderr_logs,
                    ),
                    Ok(LineRead::Eof) | Err(_) => stderr_eof = true,
                }
            }
            status = child.wait() => {
                match status {
                    Ok(status) => push_supervisor_exit_status(outputs, status),
                    Err(e) => {
                        outputs.push(SupervisorOutput::Error(format!(
                            "supervisor process wait failed: {e}"
                        )));
                    }
                }
                break;
            }
        }
    }

    if !stdout_eof {
        drain_supervisor_stdout(
            &mut stdout_buf,
            &mut stdout_partial,
            &mut stdout_line,
            adapter,
            outputs,
            &mut stdout_logs,
        )
        .await;
    }
    if !stderr_eof {
        drain_supervisor_stderr(
            &mut stderr_buf,
            &mut stderr_partial,
            &mut stderr_line,
            outputs,
            &mut stderr_logs,
        )
        .await;
    }
    push_supervisor_stdout_outputs(outputs, adapter.finalize(), &mut stdout_logs);
    adapter.session_id().map(str::to_owned)
}

async fn drain_supervisor_stdout<R>(
    reader: &mut R,
    partial: &mut Vec<u8>,
    line: &mut String,
    adapter: &mut SupervisorAdapter,
    outputs: &mut Vec<SupervisorOutput>,
    stdout_logs: &mut usize,
) where
    R: AsyncBufRead + Unpin,
{
    loop {
        let read = tokio::time::timeout(
            SUPERVISOR_POST_EXIT_DRAIN_TIMEOUT,
            read_line_resumable(reader, partial, line, MAX_LINE_BYTES),
        )
        .await;
        match read {
            Ok(Ok(LineRead::Line { truncated })) => {
                if truncated {
                    push_supervisor_stdout_outputs(
                        outputs,
                        vec![SupervisorOutput::Log(
                            "supervisor stdout line exceeded Vigla cap and was dropped".into(),
                        )],
                        stdout_logs,
                    );
                } else {
                    let text = line.trim_end_matches(['\r', '\n']);
                    push_supervisor_stdout_outputs(outputs, adapter.ingest_line(text), stdout_logs);
                }
            }
            Ok(Ok(LineRead::Eof)) | Ok(Err(_)) | Err(_) => return,
        }
    }
}

fn push_supervisor_stdout_outputs(
    outputs: &mut Vec<SupervisorOutput>,
    incoming: Vec<SupervisorOutput>,
    stdout_logs: &mut usize,
) {
    for output in incoming {
        if matches!(output, SupervisorOutput::Log(_)) {
            if *stdout_logs > MAX_SUPERVISOR_STDOUT_LOGS {
                continue;
            }
            if *stdout_logs == MAX_SUPERVISOR_STDOUT_LOGS {
                outputs.push(SupervisorOutput::Log(
                    "supervisor stdout output truncated by Vigla".into(),
                ));
                *stdout_logs += 1;
                continue;
            }
            *stdout_logs += 1;
        }
        // Intent, error, and no-intent outputs are structural. Never discard
        // them merely because diagnostic logs filled their allowance.
        outputs.push(output);
    }
}

async fn drain_supervisor_stderr<R>(
    reader: &mut R,
    partial: &mut Vec<u8>,
    line: &mut String,
    outputs: &mut Vec<SupervisorOutput>,
    stderr_logs: &mut usize,
) where
    R: AsyncBufRead + Unpin,
{
    loop {
        let read = tokio::time::timeout(
            SUPERVISOR_POST_EXIT_DRAIN_TIMEOUT,
            read_line_resumable(reader, partial, line, MAX_LINE_BYTES),
        )
        .await;
        match read {
            Ok(Ok(LineRead::Line { truncated })) => {
                push_supervisor_stderr_log(outputs, line, truncated, stderr_logs);
            }
            Ok(Ok(LineRead::Eof)) | Ok(Err(_)) | Err(_) => return,
        }
    }
}

fn push_supervisor_stderr_log(
    outputs: &mut Vec<SupervisorOutput>,
    line: &str,
    source_truncated: bool,
    stderr_logs: &mut usize,
) {
    if *stderr_logs > MAX_SUPERVISOR_STDERR_LOGS {
        return;
    }
    if *stderr_logs == MAX_SUPERVISOR_STDERR_LOGS {
        outputs.push(SupervisorOutput::Log(
            "supervisor stderr output truncated by Vigla".into(),
        ));
        *stderr_logs += 1;
        return;
    }
    let text = if source_truncated {
        "supervisor stderr line exceeded Vigla cap and was dropped".to_string()
    } else {
        format!("supervisor stderr: {}", line.trim_end_matches(['\r', '\n']))
    };
    outputs.push(SupervisorOutput::Log(text));
    *stderr_logs += 1;
}

fn push_supervisor_exit_status(outputs: &mut Vec<SupervisorOutput>, status: ExitStatus) {
    if status.success() {
        return;
    }
    outputs.push(SupervisorOutput::Error(format!(
        "supervisor process exited unsuccessfully: {status}"
    )));
}
