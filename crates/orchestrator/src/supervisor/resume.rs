use super::{vendor_short_name, Supervisor, SupervisorError};
use crate::vendor_profile::WorkerVendor;
use crate::vendor_profile::{profile_for_vendor, render_command_args, CommandRole, CommandVars};
use claude_adapter::ClaudeAdapter;
use event_schema::Vendor;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

impl Supervisor {
    /// Step 25 — continue a worker with a follow-up prompt. Requires:
    /// 1. Worker exists and is not currently running.
    /// 2. Worker has a saved session_id from a previous run.
    /// 3. Vendor supports resume flag.
    pub async fn continue_worker(
        self: &Arc<Self>,
        worker_id: &str,
        prompt: &str,
    ) -> Result<(), SupervisorError> {
        // Check if worker is still running.
        if self.is_running(worker_id).await {
            return Err(SupervisorError::WorkerStillRunning);
        }

        // Get resume metadata from repository.
        let metadata = self.repo.get_resume_metadata(worker_id).await?;

        // Check if vendor supports resume.
        match metadata.vendor {
            Vendor::Claude => {
                // Claude supports resume.
            }
            Vendor::Codex => {
                // No verified session-resume contract is available for this adapter.
                return Err(SupervisorError::ResumeUnsupported(Vendor::Codex));
            }
            Vendor::Gemini => {
                // The maintained legacy adapter does not expose session resume.
                return Err(SupervisorError::ResumeUnsupported(Vendor::Gemini));
            }
            Vendor::Antigravity | Vendor::Kiro | Vendor::Copilot => {
                // Stub adapters do not expose a resume primitive yet.
                return Err(SupervisorError::ResumeUnsupported(metadata.vendor));
            }
            Vendor::Opencode | Vendor::Mock => {
                return Err(SupervisorError::ResumeUnsupported(metadata.vendor));
            }
        }

        // Check if we have a session_id.
        let session_id = metadata
            .session_id
            .ok_or(SupervisorError::SessionIdMissing)?;

        // Persist the new prompt as last_prompt.
        self.repo.set_last_prompt(worker_id, prompt).await?;

        // Create a new worker for this continuation (same task_id).
        let counter = self.name_counter.fetch_add(1, Ordering::Relaxed);
        let _name = format!(
            "{}-continue-{}",
            vendor_short_name(metadata.vendor),
            counter + 1
        );

        // For now, we'll just spawn the worker with resume. In a more
        // complete implementation, we'd track this as a separate worker
        // entity. For the MVP, we reuse the worker_id and rely on state
        // transitions to make it clear this is a continuation.

        // Spawn the appropriate vendor CLI with resume.
        match metadata.vendor {
            Vendor::Claude => {
                self.spawn_claude_resume(worker_id, prompt, &session_id, metadata.model.as_deref())
                    .await?;
            }
            _ => {
                // Already checked above, this shouldn't happen.
                return Err(SupervisorError::ResumeUnsupported(metadata.vendor));
            }
        }

        Ok(())
    }

    /// Step 25 — retry a worker with its last prompt. Requires worker
    /// exists and is not currently running.
    pub async fn retry_worker(self: &Arc<Self>, worker_id: &str) -> Result<(), SupervisorError> {
        // Check if worker is still running.
        if self.is_running(worker_id).await {
            return Err(SupervisorError::WorkerStillRunning);
        }

        // Get resume metadata from repository.
        let metadata = self.repo.get_resume_metadata(worker_id).await?;

        // Get the last prompt.
        let prompt = metadata.last_prompt.clone().ok_or_else(|| {
            SupervisorError::Repository(crate::error::RepositoryError::RowCorrupt(
                "no last_prompt saved for retry".into(),
            ))
        })?;

        // Check if vendor supports resume.
        match metadata.vendor {
            Vendor::Claude => {
                // Claude supports resume.
            }
            Vendor::Codex => {
                return Err(SupervisorError::ResumeUnsupported(Vendor::Codex));
            }
            Vendor::Gemini => {
                return Err(SupervisorError::ResumeUnsupported(Vendor::Gemini));
            }
            Vendor::Antigravity | Vendor::Kiro | Vendor::Copilot => {
                return Err(SupervisorError::ResumeUnsupported(metadata.vendor));
            }
            Vendor::Opencode | Vendor::Mock => {
                return Err(SupervisorError::ResumeUnsupported(metadata.vendor));
            }
        }

        // Check if we have a session_id.
        let session_id = metadata
            .session_id
            .ok_or(SupervisorError::SessionIdMissing)?;

        // Spawn the retry with resume.
        match metadata.vendor {
            Vendor::Claude => {
                self.spawn_claude_resume(
                    worker_id,
                    &prompt,
                    &session_id,
                    metadata.model.as_deref(),
                )
                .await?;
            }
            _ => {
                return Err(SupervisorError::ResumeUnsupported(metadata.vendor));
            }
        }

        Ok(())
    }

    /// Batch 2 — get worker information for display/review purposes.
    /// Returns the full WorkerInfo record from the database.
    pub async fn get_worker_info(
        &self,
        worker_id: &str,
    ) -> Result<event_schema::WorkerInfo, SupervisorError> {
        self.repo
            .get_worker_info_by_id(worker_id)
            .await
            .map_err(SupervisorError::from)
    }

    /// Internal: spawn Claude with resume flag. Called by continue_worker
    /// and retry_worker.
    ///
    /// Reserves the worker slot atomically *before* doing any spawn work so
    /// two concurrent calls for the same worker_id can't both pass the
    /// running-check and double-spawn — the second insert would overwrite
    /// the first's cancel handle and orphan an unkillable child. On spawn
    /// failure the reservation is rolled back (F-7).
    async fn spawn_claude_resume(
        self: &Arc<Self>,
        worker_id: &str,
        prompt: &str,
        session_id: &str,
        model: Option<&str>,
    ) -> Result<(), SupervisorError> {
        let reservation = self.reserve_worker_slot(worker_id).await?;
        match self
            .spawn_claude_resume_child(worker_id, prompt, session_id, model, reservation.cancel_rx)
            .await
        {
            Ok((join, start)) => {
                self.activate_worker_slot(worker_id, reservation.generation, join, start)
                    .await
            }
            Err(e) => {
                // Roll back the reservation so a failed spawn doesn't leave
                // a phantom "running" worker blocking all future resume.
                self.fail_worker_slot(worker_id, reservation.generation)
                    .await;
                Err(e)
            }
        }
    }

    /// Spawn the resumed Claude child + supervise task and return its
    /// cancel/join handles. Does NOT touch the workers map — the caller
    /// (`spawn_claude_resume`) owns the reservation lifecycle.
    async fn spawn_claude_resume_child(
        self: &Arc<Self>,
        worker_id: &str,
        prompt: &str,
        session_id: &str,
        model: Option<&str>,
        cancel_rx: oneshot::Receiver<()>,
    ) -> Result<(JoinHandle<()>, oneshot::Sender<()>), SupervisorError> {
        // Get the cwd for the worker.
        let metadata = self.repo.get_resume_metadata(worker_id).await?;
        let cwd = PathBuf::from(&metadata.cwd);

        // Seed the resumed adapter past the original run's max seq so
        // its events don't collide on the `(worker_id, seq)` PRIMARY
        // KEY and vanish silently inside `insert_event_raw`.
        let starting_seq = self
            .repo
            .max_seq_for_worker(worker_id)
            .await?
            .map(|m| m + 1)
            .unwrap_or(0);

        // Render the same standalone-worker command profile as
        // `spawn_claude`, with `--resume <session_id>` inserted by the
        // profile renderer. Hardcode `--max-turns 8` to match the host
        // command's default — resume re-prompts; the original
        // turn-budget is already spent.
        let profile = profile_for_vendor(WorkerVendor::Claude);
        let args = render_command_args(
            profile,
            CommandRole::StandaloneWorker,
            CommandVars::new(prompt)
                .cwd(&cwd)
                .max_turns(8)
                .resume_session(Some(session_id))
                .model(model),
        )
        .map_err(|e| std::io::Error::other(e.to_string()))?;

        let mut command = Command::new(&profile.cli_binary);
        command
            .args(args)
            .current_dir(&cwd)
            .env("PATH", crate::resolve_user_path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::process_tree::configure(&mut command);
        let mut child = command.spawn()?;

        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let supervisor = Arc::clone(self);
        let worker_id_for_task = worker_id.to_owned();
        let (start_tx, start_rx) = oneshot::channel();

        let join = tokio::spawn(async move {
            if start_rx.await.is_err() {
                crate::process_tree::terminate_and_reap(&mut child).await;
                return;
            }
            let adapter = Box::new(ClaudeAdapter::with_starting_seq(
                worker_id_for_task.clone(),
                None, // No task_id for resumed workers.
                starting_seq,
            ));

            supervisor
                .supervise_with_adapter(
                    child,
                    stdout,
                    stderr,
                    adapter,
                    worker_id_for_task,
                    cancel_rx,
                )
                .await;
        });

        Ok((join, start_tx))
    }
}
