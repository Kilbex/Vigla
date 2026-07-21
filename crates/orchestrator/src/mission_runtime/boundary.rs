#[cfg(test)]
use super::support::emit;
#[cfg(test)]
use super::MissionEventBus;
#[cfg(test)]
use crate::mission_event::MissionEventKind;
#[cfg(test)]
use std::sync::atomic::AtomicU64;
#[cfg(test)]
use std::sync::Arc;

/// Phase 1 (decisions.md entry 6 — Single supervisor per mission,
/// permanent boundary). Roles a supervisor can request when asking
/// the orchestrator to spawn a worker. The team metaphor stops at
/// one level of supervision: requests for [`WorkerRole::Supervisor`]
/// are refused and surfaced as `boundary.sub_supervisor_refused`.
///
/// The current supervisor adapter cannot request another supervisor; its spawn
/// action names only a task index. This vocabulary is also used by handoff
/// records and retained event decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRole {
    Supervisor,
    Employee,
}

/// Enforce the "single supervisor per mission" boundary at the
/// orchestrator's spawn entry point. Returns `Ok(())` when the role
/// is permitted; on a sub-supervisor request, emits a
/// `boundary.sub_supervisor_refused` mission event and returns
/// `Err(())` so the caller drops the spawn attempt.
#[cfg(test)]
pub(crate) fn enforce_single_supervisor(
    event_bus: &MissionEventBus,
    mission_id: &str,
    seq: &Arc<AtomicU64>,
    supervisor_id: &str,
    requested_worker_id: &str,
    role: WorkerRole,
) -> Result<(), ()> {
    if matches!(role, WorkerRole::Supervisor) {
        emit(
            event_bus,
            mission_id,
            seq,
            MissionEventKind::SubSupervisorRefused {
                requested_by_supervisor_id: supervisor_id.to_string(),
                requested_worker_id: requested_worker_id.to_string(),
            },
        );
        return Err(());
    }
    Ok(())
}
