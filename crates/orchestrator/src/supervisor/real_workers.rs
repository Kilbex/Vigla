use super::{vendor_short_name, Supervisor, SupervisorError};
use crate::ids::{new_task_id, new_worker_id, rfc3339_now};
use crate::vendor_profile::WorkerVendor;
use crate::vendor_profile::{profile_for_vendor, render_command_args, CommandRole, CommandVars};
use adapter_core::Adapter;
use claude_adapter::ClaudeAdapter;
use codex_adapter::CodexAdapter;
use event_schema::{TaskInfo, Vendor, WorkerInfo};
use gemini_adapter::GeminiAdapter;
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::oneshot;

impl Supervisor {
    /// Step 11 surface: spawn the real Claude Code CLI against a
    /// working directory and a prompt. Stdout/stderr stream through
    /// [`ClaudeAdapter`] into the canonical event pipeline so the
    /// rest of Vigla sees the run identically to a mock worker.
    pub async fn spawn_claude(
        self: &Arc<Self>,
        prompt: String,
        working_dir: std::path::PathBuf,
        max_turns: u32,
    ) -> Result<String, SupervisorError> {
        self.spawn_real_cli(WorkerVendor::Claude, prompt, working_dir, Some(max_turns))
            .await
    }

    /// Step 13 surface: spawn the real Codex CLI against a working
    /// directory and a prompt. `codex exec --json` emits structured
    /// turn events that [`CodexAdapter`] translates into canonical
    /// events.
    pub async fn spawn_codex(
        self: &Arc<Self>,
        prompt: String,
        working_dir: std::path::PathBuf,
    ) -> Result<String, SupervisorError> {
        self.spawn_real_cli(WorkerVendor::Codex, prompt, working_dir, None)
            .await
    }

    /// Spawn the real Gemini CLI against a working directory and a
    /// prompt. `gemini -p PROMPT --output-format stream-json
    /// --approval-mode yolo --skip-trust` emits NDJSON events that
    /// [`GeminiAdapter`] translates into canonical events. The
    /// `--skip-trust` flag is required for headless use in untrusted
    /// directories (see `gemini --help`).
    pub async fn spawn_gemini(
        self: &Arc<Self>,
        prompt: String,
        working_dir: std::path::PathBuf,
    ) -> Result<String, SupervisorError> {
        self.spawn_real_cli(WorkerVendor::Gemini, prompt, working_dir, None)
            .await
    }

    async fn spawn_real_cli(
        self: &Arc<Self>,
        worker_vendor: WorkerVendor,
        prompt: String,
        working_dir: std::path::PathBuf,
        max_turns: Option<u32>,
    ) -> Result<String, SupervisorError> {
        let worker_id = new_worker_id();
        let task_id = new_task_id();
        let reservation = self.reserve_worker_slot(&worker_id).await?;
        let now = rfc3339_now();
        let counter = self.name_counter.fetch_add(1, Ordering::Relaxed);
        let vendor = match worker_vendor {
            WorkerVendor::Claude => Vendor::Claude,
            WorkerVendor::Codex => Vendor::Codex,
            WorkerVendor::Gemini => Vendor::Gemini,
            _ => unreachable!("standalone real worker supports three adapters"),
        };
        let profile = profile_for_vendor(worker_vendor);
        let setup: Result<_, SupervisorError> = async {
            self.repo
                .insert_worker_and_task(
                    &WorkerInfo {
                        id: worker_id.clone(),
                        name: format!("{}-{}", vendor_short_name(vendor), counter + 1),
                        vendor,
                        cli_binary: profile.cli_binary.clone(),
                        cli_version: None,
                        cwd: working_dir.to_string_lossy().into_owned(),
                        model: None,
                        spawned_at: now.clone(),
                        ended_at: None,
                    },
                    &TaskInfo {
                        id: task_id.clone(),
                        parent_id: None,
                        title: prompt.chars().take(80).collect(),
                        depends_on: Vec::new(),
                        created_at: now,
                    },
                )
                .await?;
            self.repo.set_last_prompt(&worker_id, &prompt).await?;

            let mut vars = CommandVars::new(&prompt).cwd(&working_dir);
            if let Some(max_turns) = max_turns {
                vars = vars.max_turns(max_turns);
            }
            let args = render_command_args(profile, CommandRole::StandaloneWorker, vars)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let mut command = Command::new(&profile.cli_binary);
            command
                .args(args)
                .current_dir(&working_dir)
                .env("PATH", crate::resolve_user_path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            crate::process_tree::configure(&mut command);
            let mut child = command.spawn()?;
            let stdout = child.stdout.take().expect("piped stdout");
            let stderr = child.stderr.take().expect("piped stderr");
            Ok((child, stdout, stderr))
        }
        .await;

        let (mut child, stdout, stderr) = match setup {
            Ok(parts) => parts,
            Err(error) => {
                if let Err(rollback) = self
                    .repo
                    .rollback_worker_spawn(&worker_id, Some(&task_id), true)
                    .await
                {
                    tracing::error!(
                        "orchestrator: failed to roll back real-worker rows: {rollback}"
                    );
                }
                self.fail_worker_slot(&worker_id, reservation.generation)
                    .await;
                return Err(error);
            }
        };

        let supervisor = Arc::clone(self);
        let worker_id_for_task = worker_id.clone();
        let task_id_for_adapter = task_id.clone();
        let adapter: Box<dyn Adapter> = match worker_vendor {
            WorkerVendor::Claude => Box::new(ClaudeAdapter::new(
                worker_id_for_task.clone(),
                Some(task_id_for_adapter),
            )),
            WorkerVendor::Codex => Box::new(CodexAdapter::new(
                worker_id_for_task.clone(),
                Some(task_id_for_adapter),
            )),
            WorkerVendor::Gemini => Box::new(GeminiAdapter::new(
                worker_id_for_task.clone(),
                Some(task_id_for_adapter),
            )),
            _ => unreachable!(),
        };
        let (start_tx, start_rx) = oneshot::channel();
        let cancel_rx = reservation.cancel_rx;
        let join = tokio::spawn(async move {
            if start_rx.await.is_err() {
                crate::process_tree::terminate_and_reap(&mut child).await;
                return;
            }
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

        if let Err(error) = self
            .activate_worker_slot(&worker_id, reservation.generation, join, start_tx)
            .await
        {
            if let Err(rollback) = self
                .repo
                .rollback_worker_spawn(&worker_id, Some(&task_id), true)
                .await
            {
                tracing::error!(
                    "orchestrator: failed to roll back inactive real-worker rows: {rollback}"
                );
            }
            return Err(error);
        }

        Ok(worker_id)
    }
}
