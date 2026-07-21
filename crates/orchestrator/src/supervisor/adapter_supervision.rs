use super::Supervisor;
use crate::ids::rfc3339_now;
use crate::parser::{
    persist_and_emit, read_line_capped, LineRead, WorkerEventSink, MAX_LINE_BYTES,
};
use crate::repository::Repository;
use adapter_core::{Adapter, AdapterExit};
use event_schema::LogStream;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::BufReader;
use tokio::process::Child;
use tokio::sync::oneshot;

impl Supervisor {
    /// Drive the adapter line-pump for one worker: consume stdout +
    /// stderr through the adapter, persist + emit each event, and
    /// drain on EOF / cancel / child exit. Public so integration tests
    /// can exercise the real-CLI coordination path without depending on
    /// a real CLI binary; production callers reach this via
    /// [`Self::spawn_claude`] / [`Self::spawn_codex`].
    pub async fn supervise_with_adapter(
        self: Arc<Self>,
        mut child: Child,
        stdout: tokio::process::ChildStdout,
        stderr: tokio::process::ChildStderr,
        mut adapter: Box<dyn Adapter>,
        worker_id: String,
        mut cancel: oneshot::Receiver<()>,
    ) {
        // Single-threaded line-pump that feeds both stdout and
        // stderr into ONE adapter instance (no Mutex needed). The
        // adapter handles the LogStream parameter to differentiate
        // sources internally. Lines are capped at MAX_LINE_BYTES so
        // a malformed binary blob from a real CLI cannot OOM the host.
        let mut stdout_buf = BufReader::new(stdout);
        let mut stderr_buf = BufReader::new(stderr);
        let mut stdout_line = String::new();
        let mut stderr_line = String::new();
        // Coordinating wrap mirrors what `supervise` does for the mock
        // path; without it, downstream tasks waiting on a real-CLI
        // upstream never unblock.
        let sink = self.coordinating_sink();
        // Once stderr EOFs, disable that select arm with `if !stderr_eof`
        // — otherwise `read_line_capped` returns Ok(LineRead::Eof)
        // immediately on every poll and the select! tight-loops on the
        // closed stream, starving stdout reads and the cancel branch.
        let mut stderr_eof = false;
        let mut stdout_eof = false;
        // Set when the user (or another internal stop) fires the cancel
        // channel. Threaded into adapter.finalize() so adapters can
        // emit `Failed` instead of `Done` when the run was killed.
        let mut cancelled = false;
        // Step 25 — capture the session_id from the adapter exactly once.
        let mut session_id_captured = false;

        loop {
            tokio::select! {
                outcome = read_line_capped(&mut stdout_buf, &mut stdout_line, MAX_LINE_BYTES),
                    if !stdout_eof => {
                    match outcome {
                        Ok(LineRead::Line { truncated }) => {
                            if truncated {
                                tracing::warn!(
                                    "orchestrator: dropped overlong stdout line for worker {worker_id}"
                                );
                                continue;
                            }
                            let text = stdout_line.trim_end_matches(['\r', '\n']);
                            for event in adapter.ingest_line(text, LogStream::Stdout) {
                                persist_and_emit(&event, &self.repo, sink.as_ref()).await;
                            }
                            // Step 25 — capture session_id from adapter once.
                            self.capture_session_id_once(
                                adapter.as_mut(),
                                &worker_id,
                                &mut session_id_captured,
                            )
                            .await;
                        }
                        // A CLI may close stdout while continuing work or
                        // while descendants still hold stderr. Disable only
                        // this arm; cancellation and child exit remain live.
                        _ => stdout_eof = true,
                    }
                }
                outcome = read_line_capped(&mut stderr_buf, &mut stderr_line, MAX_LINE_BYTES),
                    if !stderr_eof =>
                {
                    match outcome {
                        Ok(LineRead::Line { truncated }) => {
                            if truncated {
                                tracing::warn!(
                                    "orchestrator: dropped overlong stderr line for worker {worker_id}"
                                );
                                continue;
                            }
                            let text = stderr_line.trim_end_matches(['\r', '\n']);
                            for event in adapter.ingest_line(text, LogStream::Stderr) {
                                persist_and_emit(&event, &self.repo, sink.as_ref()).await;
                            }
                            // Step 25 — capture session_id from adapter once.
                            self.capture_session_id_once(
                                adapter.as_mut(),
                                &worker_id,
                                &mut session_id_captured,
                            )
                            .await;
                        }
                        // EOF or error on stderr — disable this arm so we
                        // keep responsive on stdout/cancel/child.wait.
                        _ => stderr_eof = true,
                    }
                }
                _ = &mut cancel => {
                    crate::process_tree::terminate_and_reap(&mut child).await;
                    cancelled = true;
                    break;
                }
                _ = child.wait() => break,
            }
        }

        // Drain remaining lines after select breaks (if EOF on
        // stdout, stderr may still have data). Skip the drain entirely
        // when cancelled — after `child.kill()` there is no useful
        // post-mortem output to capture, and a grandchild that
        // inherited the pipe fd (didn't set O_CLOEXEC) keeps the
        // stream open indefinitely, hanging this task forever and
        // making `stop()` block until the orphan process exits.
        // Audit r5: real Claude/Codex spawns are now wired to a UI
        // Stop button; without this guard, clicking Stop on a worker
        // that shells out (e.g., a bash tool call) freezes the
        // command. Belt-and-suspenders: even on natural exit, bound
        // each per-line read with a short timeout so a pathological
        // pipe holder cannot wedge the supervise task.
        if !cancelled {
            drain_with_timeout(
                &mut stderr_buf,
                &mut stderr_line,
                LogStream::Stderr,
                adapter.as_mut(),
                &self.repo,
                sink.as_ref(),
                &worker_id,
            )
            .await;
            drain_with_timeout(
                &mut stdout_buf,
                &mut stdout_line,
                LogStream::Stdout,
                adapter.as_mut(),
                &self.repo,
                sink.as_ref(),
                &worker_id,
            )
            .await;
        }

        // The init line carrying the session_id may only have been read
        // during the post-exit drain above — if the child won the select!
        // race and broke the loop before the first in-loop read, the
        // in-loop capture never ran. Attempt capture once more so resume
        // isn't permanently broken with SessionIdMissing. Mirrors the
        // supervisor driver's post-drain read; idempotent via the flag.
        self.capture_session_id_once(adapter.as_mut(), &worker_id, &mut session_id_captured)
            .await;

        // Reap the child and translate the exit status into the
        // adapter-facing AdapterExit so finalize() can emit the right
        // terminal event.
        let exit_status = if cancelled {
            None
        } else {
            Some(child.wait().await)
        };
        let adapter_exit = match exit_status {
            Some(status) => classify_exit(status),
            None => AdapterExit::Killed,
        };

        // adapter.finalize for any trailing partial state.
        for event in adapter.finalize(adapter_exit) {
            persist_and_emit(&event, &self.repo, sink.as_ref()).await;
        }

        let ended_at = rfc3339_now();
        if let Err(e) = self.repo.mark_worker_ended(&worker_id, &ended_at).await {
            tracing::error!("orchestrator: mark_worker_ended failed for {worker_id}: {e}");
        }
        self.workers.lock().await.remove(&worker_id);
    }

    /// Capture the adapter's `session_id` exactly once and persist it so a
    /// later `continue_worker` / `retry_worker` can resume. Idempotent via
    /// `captured`; safe to call from the read loop AND after the post-exit
    /// drain, since only the first successful `take_session_id` flips the
    /// flag and does the insert + persist.
    async fn capture_session_id_once(
        &self,
        adapter: &mut dyn Adapter,
        worker_id: &str,
        captured: &mut bool,
    ) {
        if *captured {
            return;
        }
        if let Some(sid) = adapter.take_session_id() {
            *captured = true;
            self.session_ids
                .lock()
                .await
                .insert(worker_id.to_string(), sid.clone());
            if let Err(e) = self.repo.set_session_id(worker_id, &sid).await {
                tracing::warn!(
                    worker_id = %worker_id,
                    error = %e,
                    "failed to persist session_id — resume will not work after app restart"
                );
            }
        }
    }
}

/// Audit r5 — bounded post-loop drain. Reads remaining lines from a
/// stream after the supervise select! has exited, but caps each
/// per-line read with a short timeout so a pathological pipe holder
/// (e.g. a grandchild that inherited the fd) cannot wedge the
/// supervise task indefinitely. Exits cleanly on EOF, IO error, or
/// timeout. Mirrors the in-loop read+process+persist+emit shape.
#[allow(clippy::too_many_arguments)]
async fn drain_with_timeout(
    buf: &mut BufReader<impl tokio::io::AsyncRead + Unpin>,
    line: &mut String,
    stream: LogStream,
    adapter: &mut dyn Adapter,
    repo: &Repository,
    sink: &dyn WorkerEventSink,
    worker_id: &str,
) {
    // 500 ms per line is generous: pipes that already EOF'd return
    // immediately; pipes still held open by an orphan return Elapsed
    // and the loop terminates, freeing the supervise task. Tuned to
    // stay below typical IPC timeouts so `stop()` returns promptly.
    const PER_LINE_TIMEOUT: Duration = Duration::from_millis(500);
    loop {
        let outcome = tokio::time::timeout(
            PER_LINE_TIMEOUT,
            read_line_capped(buf, line, MAX_LINE_BYTES),
        )
        .await;
        let Ok(read) = outcome else {
            // Timeout — give up on the drain. Log once at the call
            // site; further lines are unreachable from here without
            // risking the same wedge.
            tracing::warn!(
                "orchestrator: drain timed out for worker {worker_id} on {stream:?} stream"
            );
            return;
        };
        match read {
            Ok(LineRead::Line { truncated }) => {
                if truncated {
                    continue;
                }
                let text = line.trim_end_matches(['\r', '\n']);
                for event in adapter.ingest_line(text, stream) {
                    persist_and_emit(&event, repo, sink).await;
                }
            }
            // EOF or IO error — done with this stream.
            _ => return,
        }
    }
}

/// Classify a `child.wait()` outcome for the adapter. Signaled
/// children land in `Killed` so the adapter emits a `Failed` terminal
/// state instead of a clean `Done` after a SIGKILL / SIGTERM.
fn classify_exit(status: std::io::Result<std::process::ExitStatus>) -> AdapterExit {
    let Ok(status) = status else {
        return AdapterExit::Failed { code: None };
    };
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
