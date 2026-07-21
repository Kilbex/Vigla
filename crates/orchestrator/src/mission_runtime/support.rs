use super::{MissionEventBus, MissionRuntimeError};
use crate::mission::MissionState;
use crate::mission_event::MissionEventKind;
use crate::mission_workspace::MissionGitError;
use std::path::Path;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::watch;

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

/// Called when the spawned mission task returns `Err`. Flips the state
/// machine to `Aborted` and emits a final `Aborted` event so callers
/// blocked on `await_complete_or_terminal` / `resolve` unblock instead
/// of hanging forever. No-op if the mission has already reached a
/// terminal state.
pub(super) async fn finalize_failure(
    state_tx: &Arc<watch::Sender<MissionState>>,
    event_bus: &MissionEventBus,
    seq: &Arc<AtomicU64>,
    mission_id: &str,
    err: MissionRuntimeError,
) {
    // `MissionState` is no longer `Copy` (the S5 `Paused` variant
    // carries a typed payload). Match on a borrow so we don't move out
    // of the watch channel's `Ref` guard.
    let current = state_tx.borrow().clone();
    if matches!(
        current,
        MissionState::Merged | MissionState::Discarded | MissionState::Aborted
    ) {
        return;
    }
    state_tx.send(MissionState::Aborted).ok();
    emit(
        event_bus,
        mission_id,
        seq,
        MissionEventKind::Aborted {
            reason: format!("mission task failed: {err}"),
        },
    );
}

pub(super) fn emit(
    bus: &MissionEventBus,
    mission_id: &str,
    seq: &Arc<AtomicU64>,
    kind: MissionEventKind,
) {
    bus.emit_kind(mission_id, seq, kind);
}

pub(super) async fn run_git_in(cwd: &Path, args: &[&str]) -> Result<(), MissionRuntimeError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .map_err(|e| MissionRuntimeError::Io(e.to_string()))?;
    if !output.status.success() {
        return Err(MissionRuntimeError::Git(MissionGitError::Git {
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        }));
    }
    Ok(())
}
