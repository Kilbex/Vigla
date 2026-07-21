//! Supervisor-driven mission loop (MSV U3.4).
//!
//! The parallel of `mission_runtime::run_mock_mission`, but each
//! decision is requested from a real (or scripted) supervisor instead
//! of a hardcoded timeline. The orchestrator still owns all real
//! actions (branch creation, worktree provisioning, integration); the
//! supervisor only produces intents.
//!
//! Two drivers ship today:
//!
//! - [`SupervisorDriver::RealClaude`] spawns `claude -p` with the
//!   bundled [`supervisor_adapter::PLAYBOOK`] as the system prompt and
//!   a read-only `--tools Read,Glob,LS` surface so the supervisor can
//!   inspect the repo before emitting assistant prose + a fenced JSON
//!   envelope.
//! - [`SupervisorDriver::Scripted`] returns pre-canned outputs per
//!   turn, used by tests so the mission loop can be exercised end-to-
//!   end without spawning Claude.

mod driver;
mod mission_loop;
mod prompts;
mod run_task;
mod support;
mod worker_pass;

pub use driver::{RealClaudeConfig, ScriptedSupervisor, SupervisorDriver, SupervisorTurnResult};
pub use worker_pass::{
    select_worker_backend, worker_model_selection_is_valid, WorkerBackend, WorkerRoster,
    L1_CLAUDE_QUOTA_EXHAUSTED_WORKER_MODEL,
};

pub(crate) use worker_pass::PassSignalSink;

pub(crate) use mission_loop::run_supervisor_mission;

#[cfg(test)]
use driver::{collect_supervisor_turn, SUPERVISOR_DISALLOWED_TOOLS, SUPERVISOR_MAX_TURNS};
#[cfg(test)]
use prompts::format_decompose_prompt;
#[cfg(test)]
use worker_pass::side_effect_events_for_submission;

#[cfg(test)]
mod scripted_fixtures;

#[cfg(test)]
mod tests;
