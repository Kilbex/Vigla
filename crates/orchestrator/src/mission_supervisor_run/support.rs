use crate::mission::MissionState;
use crate::mission_event::MissionEventKind;
use crate::mission_runtime::{MissionEventBus, MissionRuntimeError};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use supervisor_adapter::{SupervisorIntent, SupervisorOutput};
use tokio::sync::watch;

/// Find the first intent in the outputs, ignoring logs/no-intent.
pub(super) fn first_intent(outputs: &[SupervisorOutput]) -> Option<&SupervisorIntent> {
    outputs.iter().find_map(|o| match o {
        SupervisorOutput::Intent(i) => Some(i),
        _ => None,
    })
}

pub(super) async fn abort(
    event_bus: &MissionEventBus,
    mission_id: &str,
    seq: &Arc<AtomicU64>,
    state_tx: &Arc<watch::Sender<MissionState>>,
    reason: &str,
) -> Result<(), MissionRuntimeError> {
    state_tx.send(MissionState::Aborted).ok();
    emit(
        event_bus,
        mission_id,
        seq,
        MissionEventKind::Aborted {
            reason: reason.into(),
        },
    );
    Ok(())
}

pub(super) fn emit(
    bus: &MissionEventBus,
    mission_id: &str,
    seq: &Arc<AtomicU64>,
    kind: MissionEventKind,
) {
    bus.emit_kind(mission_id, seq, kind);
}
