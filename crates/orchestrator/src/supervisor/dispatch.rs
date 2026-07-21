use super::{
    vendor_for_script, vendor_short_name, DispatchRequest, PendingDispatch, RetryPolicy,
    SpawnRequest, Supervisor, SupervisorError,
};
use crate::ids::{new_task_id, new_worker_id, rfc3339_now};
use crate::parser::{process_event_stream, read_line_capped, LineRead, MAX_LINE_BYTES};
use event_schema::{TaskInfo, WorkerInfo};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::BufReader;
use tokio::process::{Child, Command};
use tokio::sync::oneshot;

impl Supervisor {
    /// Spawn a worker immediately with no dependencies and no
    /// retry. Equivalent to `dispatch(DispatchRequest::from_script(...))`
    /// with `retry=Never` and `depends_on=[]`.
    pub async fn spawn_mock(
        self: &Arc<Self>,
        req: SpawnRequest,
    ) -> Result<String, SupervisorError> {
        let dispatch = DispatchRequest {
            script: req.script,
            speed: req.speed,
            task_title: req.task_title,
            task_id: new_task_id(),
            depends_on: Vec::new(),
            retry: RetryPolicy::Never,
        };
        self.dispatch(dispatch).await
    }

    /// Dispatch a task. If `depends_on` is satisfied,
    /// spawn immediately and return the worker_id. Otherwise queue the
    /// dispatch and return `req.task_id` (no worker spawned yet).
    /// In the queued case, the supervisor will spawn the worker once
    /// every `depends_on` task has reached completion.
    ///
    /// The deps check + push must run under the `pending` lock to avoid
    /// a TOCTOU race against [`Self::drain_pending`]: if dep `A`
    /// completed between an earlier "deps not satisfied" read and the
    /// push, the simultaneous drain saw an empty queue and the entry
    /// would have stayed pending forever. Acquiring `pending` first
    /// matches `drain_pending`'s order (`pending` → `completed_tasks`)
    /// so the two paths serialize without deadlock.
    pub async fn dispatch(
        self: &Arc<Self>,
        req: DispatchRequest,
    ) -> Result<String, SupervisorError> {
        let mut pending = self.pending.lock().await;
        let completed = self.completed_tasks.lock().await;
        let mut failed = self.failed_tasks.lock().await;
        let blocked = req.depends_on.iter().any(|d| failed.contains(d));
        let satisfied = req.depends_on.iter().all(|d| completed.contains(d));
        if blocked {
            let task_id = req.task_id;
            failed.insert(task_id.clone());
            drop(failed);
            drop(completed);
            drop(pending);
            self.drain_pending().await;
            return Ok(task_id);
        }
        if satisfied {
            drop(failed);
            drop(completed);
            drop(pending);
            return self.spawn_now(req, 1).await;
        }
        let task_id = req.task_id.clone();
        pending.push(PendingDispatch {
            request: req,
            attempt: 1,
        });
        Ok(task_id)
    }

    /// Check if a worker is still being supervised.
    pub async fn is_running(&self, worker_id: &str) -> bool {
        self.workers.lock().await.contains_key(worker_id)
    }

    /// Has the given task reached completion (saw a `Completion` event
    /// or a `state_change → done`)? Used by callers that want to wait
    /// for downstream-dispatch readiness, and by tests verifying the
    /// coordination wrap fires on every supervise path.
    pub async fn task_completed(&self, task_id: &str) -> bool {
        self.completed_tasks.lock().await.contains(task_id)
    }

    /// Has the task failed terminally or been canceled because an upstream
    /// dependency failed? Canceled tasks never spawn a worker.
    pub async fn task_failed(&self, task_id: &str) -> bool {
        self.failed_tasks.lock().await.contains(task_id)
    }

    /// Check if any worker is currently running OR if any task is
    /// queued for dispatch (deps-pending or retry-backoff). Used by
    /// tests to wait for full drain of a dispatched DAG.
    pub async fn is_quiescent(&self) -> bool {
        self.workers.lock().await.is_empty()
            && self.pending.lock().await.is_empty()
            && self.pending_retries.load(Ordering::SeqCst) == 0
    }

    /// Stop a worker in flight. The entry remains in the workers map
    /// until the supervise task finishes its cleanup (drains stdio,
    /// marks ended, self-removes), so `is_running()` stays truthful
    /// through the cancel + join window — callers polling that signal
    /// observe a coherent view of the worker's lifecycle.
    pub async fn stop(&self, worker_id: &str) -> Result<(), SupervisorError> {
        let (cancel_tx, mut join, mut phase, generation) = {
            let mut workers = self.workers.lock().await;
            let entry = workers
                .get_mut(worker_id)
                .ok_or_else(|| SupervisorError::WorkerNotFound(worker_id.to_owned()))?;
            (
                entry.cancel.take(),
                entry.join.take(),
                entry.phase.subscribe(),
                entry.generation,
            )
        };
        if let Some(tx) = cancel_tx {
            let _ = tx.send(());
        }
        while join.is_none() && *phase.borrow_and_update() == super::WorkerSlotPhase::Preparing {
            if phase.changed().await.is_err() {
                break;
            }
            let mut workers = self.workers.lock().await;
            let Some(entry) = workers.get_mut(worker_id) else {
                break;
            };
            if entry.generation != generation {
                break;
            }
            join = entry.join.take();
        }
        if let Some(j) = join {
            let _ = j.await;
        }
        Ok(())
    }

    /// Internal: actually spawn the child process for a dispatch.
    /// `attempt` is 1 for first attempts, 2+ for retries.
    async fn spawn_now(
        self: &Arc<Self>,
        req: DispatchRequest,
        attempt: u32,
    ) -> Result<String, SupervisorError> {
        let vendor = vendor_for_script(&req.script)?;
        let worker_id = new_worker_id();
        let reservation = self.reserve_worker_slot(&worker_id).await?;
        let now = rfc3339_now();
        let counter = self.name_counter.fetch_add(1, Ordering::Relaxed);
        let suffix = if attempt > 1 {
            format!("-{}-r{}", counter + 1, attempt)
        } else {
            format!("-{}", counter + 1)
        };
        let name = format!("{}{suffix}", vendor_short_name(vendor));

        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        let worker_info = WorkerInfo {
            id: worker_id.clone(),
            name,
            vendor,
            cli_binary: self.mock_harness.to_string_lossy().into_owned(),
            cli_version: None,
            cwd: cwd.to_string_lossy().into_owned(),
            model: None,
            spawned_at: now.clone(),
            ended_at: None,
        };
        // First attempt: worker + task atomically. Retries re-use the
        // existing task_id, so they only insert a fresh worker row.
        let persistence = if attempt == 1 {
            self.repo
                .insert_worker_and_task(
                    &worker_info,
                    &TaskInfo {
                        id: req.task_id.clone(),
                        parent_id: None,
                        title: req.task_title.clone(),
                        depends_on: req.depends_on.clone(),
                        created_at: now,
                    },
                )
                .await
        } else {
            self.repo.insert_worker(&worker_info).await
        };
        if let Err(error) = persistence {
            self.fail_worker_slot(&worker_id, reservation.generation)
                .await;
            return Err(error.into());
        }

        let mut command = Command::new(&self.mock_harness);
        command
            .arg("--script")
            .arg(&req.script)
            .arg("--speed")
            .arg(format!("{:.2}", req.speed))
            .arg("--worker-id")
            .arg(&worker_id)
            .arg("--task-id")
            .arg(&req.task_id)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::process_tree::configure(&mut command);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                if let Err(rollback) = self
                    .repo
                    .rollback_worker_spawn(&worker_id, Some(&req.task_id), attempt == 1)
                    .await
                {
                    tracing::error!("orchestrator: failed to roll back mock spawn row: {rollback}");
                }
                self.fail_worker_slot(&worker_id, reservation.generation)
                    .await;
                return Err(error.into());
            }
        };

        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        // Record the attempt count so retries can be bounded.
        self.attempts
            .lock()
            .await
            .insert(req.task_id.clone(), attempt);

        let supervisor = Arc::clone(self);
        let worker_id_for_task = worker_id.clone();
        let req_for_task = req.clone();
        let (start_tx, start_rx) = oneshot::channel();
        let cancel_rx = reservation.cancel_rx;
        let join = tokio::spawn(async move {
            if start_rx.await.is_err() {
                crate::process_tree::terminate_and_reap(&mut child).await;
                return;
            }
            supervisor
                .supervise(
                    child,
                    stdout,
                    stderr,
                    worker_id_for_task,
                    req_for_task,
                    attempt,
                    cancel_rx,
                )
                .await;
        });

        if let Err(error) = self
            .activate_worker_slot(&worker_id, reservation.generation, join, start_tx)
            .await
        {
            if let Err(rollback) = self
                .repo
                .rollback_worker_spawn(&worker_id, Some(&req.task_id), attempt == 1)
                .await
            {
                tracing::error!(
                    "orchestrator: failed to roll back inactive mock spawn row: {rollback}"
                );
            }
            return Err(error);
        }

        Ok(worker_id)
    }

    async fn supervise(
        self: Arc<Self>,
        mut child: Child,
        stdout: tokio::process::ChildStdout,
        stderr: tokio::process::ChildStderr,
        worker_id: String,
        req: DispatchRequest,
        attempt: u32,
        mut cancel: oneshot::Receiver<()>,
    ) {
        let stdout_buf = BufReader::new(stdout);
        let mut stderr_buf = BufReader::new(stderr);

        let mut parser_task = {
            let repo = self.repo.clone();
            // Same coordinating wrap used by supervise_with_adapter.
            let sink = self.coordinating_sink();
            tokio::spawn(async move { process_event_stream(stdout_buf, &repo, sink).await })
        };

        let mut stderr_task = tokio::spawn(async move {
            let mut buf = String::new();
            while let Ok(LineRead::Line { truncated }) =
                read_line_capped(&mut stderr_buf, &mut buf, MAX_LINE_BYTES).await
            {
                if truncated {
                    tracing::warn!("worker stderr: <dropped overlong line>");
                    continue;
                }
                let text = buf.trim_end_matches(['\r', '\n']);
                tracing::warn!("worker stderr: {text}");
            }
        });

        tokio::select! {
            _ = &mut cancel => {
                crate::process_tree::terminate_and_reap(&mut child).await;
            }
            _ = child.wait() => {}
        }

        let _ = child.wait().await;
        if tokio::time::timeout(Duration::from_millis(500), &mut parser_task)
            .await
            .is_err()
        {
            tracing::warn!("orchestrator: mock stdout drain timed out for {worker_id}");
            parser_task.abort();
            let _ = parser_task.await;
        }
        if tokio::time::timeout(Duration::from_millis(500), &mut stderr_task)
            .await
            .is_err()
        {
            tracing::warn!("orchestrator: mock stderr drain timed out for {worker_id}");
            stderr_task.abort();
            let _ = stderr_task.await;
        }

        let ended_at = rfc3339_now();
        if let Err(e) = self.repo.mark_worker_ended(&worker_id, &ended_at).await {
            tracing::error!("orchestrator: mark_worker_ended failed for {worker_id}: {e}");
        }

        self.workers.lock().await.remove(&worker_id);

        // After the worker finishes, decide whether to retry. The
        // coordinating sink already updated `attempts` based on
        // observed `failure` events; we only need to act on retry.
        // Spawn the retry decision in a separate task so this
        // supervise task can return promptly (and the worker is
        // visibly drained by `is_running`). We bump `pending_retries`
        // before the spawn so tests can detect "retry in flight" via
        // `is_quiescent`.
        if matches!(req.retry, RetryPolicy::OnFailure { .. }) {
            self.pending_retries.fetch_add(1, Ordering::SeqCst);
            // Use spawn_supervised (not bare tokio::spawn) so a panic in
            // the retry path is logged and propagated rather than silently
            // swallowed — a swallowed panic here would strand
            // `pending_retries` and stall quiescence (F-13).
            crate::spawn_supervised(
                "supervisor::schedule_retry",
                Arc::clone(&self).schedule_retry(req, attempt),
            );
        }
    }

    /// Mark a task complete and drain any pending dispatches whose
    /// dependencies are now satisfied. Called from
    /// [`CoordinatingSink::emit`] when a `completion` event lands.
    pub(super) async fn on_task_completed(self: &Arc<Self>, task_id: &str) {
        {
            let mut done = self.completed_tasks.lock().await;
            done.insert(task_id.to_owned());
        }
        self.drain_pending().await;
    }

    /// Mark a task as terminally failed and cancel every pending task that
    /// depends on it, directly or transitively.
    pub(super) async fn on_task_failed_terminal(self: &Arc<Self>, task_id: &str) {
        self.failed_tasks.lock().await.insert(task_id.to_owned());
        self.drain_pending().await;
    }

    async fn drain_pending(self: &Arc<Self>) {
        // Two-pass: first identify ready entries, then spawn outside
        // the lock.
        let mut ready: Vec<PendingDispatch> = Vec::new();
        {
            let mut pending = self.pending.lock().await;
            let completed = self.completed_tasks.lock().await;
            let mut failed = self.failed_tasks.lock().await;
            loop {
                let mut keep: Vec<PendingDispatch> = Vec::new();
                let mut canceled_any = false;
                for entry in pending.drain(..) {
                    let any_failed = entry.request.depends_on.iter().any(|d| failed.contains(d));
                    if any_failed {
                        canceled_any |= failed.insert(entry.request.task_id);
                        continue;
                    }
                    let all_done = entry
                        .request
                        .depends_on
                        .iter()
                        .all(|d| completed.contains(d));
                    if all_done {
                        ready.push(entry);
                    } else {
                        keep.push(entry);
                    }
                }
                *pending = keep;
                if !canceled_any {
                    break;
                }
            }
        }
        for entry in ready {
            if let Err(e) = self.spawn_now(entry.request.clone(), entry.attempt).await {
                tracing::error!(
                    "orchestrator: failed to spawn pending dispatch for task {}: {e}",
                    entry.request.task_id
                );
            }
        }
    }

    /// Boxed-future helper that schedules a retry. Returning an
    /// `impl Future + Send` (rather than letting the compiler infer
    /// an opaque type) breaks the supervise → maybe_retry → spawn_now
    /// → supervise recursion that Rust's opaque-type analysis can't
    /// resolve.
    fn schedule_retry(
        self: Arc<Self>,
        req: DispatchRequest,
        attempt: u32,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(async move {
            let _result = (async {
                let RetryPolicy::OnFailure {
                    max_attempts,
                    base_ms,
                } = req.retry
                else {
                    return;
                };
                let completed = self.completed_tasks.lock().await.contains(&req.task_id);
                if completed {
                    return;
                }
                // Audit r5 polish — honor the worker-emitted Failure
                // event's `retryable: false` flag.
                // `apply_coordination_side_effects` populates
                // `failed_tasks` when a non-retryable Failure arrives;
                // without this gate the dispatch's `RetryPolicy::OnFailure`
                // would fire spawn after spawn up to `max_attempts`,
                // ignoring the worker's clear "don't retry me" signal.
                let failed_terminal = self.failed_tasks.lock().await.contains(&req.task_id);
                if failed_terminal {
                    return;
                }
                let next_attempt = attempt + 1;
                if next_attempt > max_attempts {
                    self.on_task_failed_terminal(&req.task_id).await;
                    return;
                }
                let backoff = retry_backoff(base_ms, attempt);
                tokio::time::sleep(backoff).await;
                if let Err(e) = self.spawn_now(req.clone(), next_attempt).await {
                    tracing::error!(
                        "orchestrator: retry spawn failed for task {}: {e}",
                        req.task_id
                    );
                    self.on_task_failed_terminal(&req.task_id).await;
                }
            })
            .await;
            // Always decrement, regardless of outcome.
            self.pending_retries.fetch_sub(1, Ordering::SeqCst);
            // `result` is `()`; no further use.
        })
    }
}

/// Backoff for retry attempt N: `base_ms · 2^(attempt-1)`, with the
/// shift clamped to 63 and the multiplication saturated. Without the
/// clamp `1u64 << 64` panics in debug at any caller-supplied
/// `max_attempts ≥ 66` (DispatchRequest.retry is public input).
pub(super) fn retry_backoff(base_ms: u64, attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(63);
    Duration::from_millis(base_ms.saturating_mul(1u64 << shift))
}
