//! Tauri commands backing the S3 escalation surface.
//!
//! - `mission_event_visibility` exposes
//!   [`orchestrator::escalation::visibility_for`] to the
//!   frontend so TS ingest consults the canonical mapping (single
//!   source of truth — see plan §S3 Task 6).
//! - `surface_inbox_notification` fires a macOS native banner via
//!   the `tauri-plugin-notification` 2.x crate. Called from
//!   the frontend when an `ActionRequired` Inbox card lands and
//!   the window is not focused.

use orchestrator::escalation::{visibility_for, EventVisibility};
use orchestrator::mission_event::MissionEventKind;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

/// Thin DTO wrapper — `MissionEventKind` is `#[serde(tag = "type",
/// content = "payload")]`, so this struct passes through unchanged.
/// Specta picks up the inner enum's bindings.
#[derive(Serialize, Deserialize, Debug, Clone, Type)]
#[serde(transparent)]
pub struct MissionEventKindDto(pub MissionEventKind);

#[tauri::command]
#[specta::specta]
pub fn mission_event_visibility(event: MissionEventKindDto) -> EventVisibility {
    visibility_for(&event.0)
}

#[tauri::command]
#[specta::specta]
pub async fn surface_inbox_notification(
    app: AppHandle,
    title: String,
    body: String,
) -> Result<(), String> {
    app.notification()
        .builder()
        .title(&title)
        .body(&body)
        .show()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchestrator::mission::MissionSpec;

    #[test]
    fn visibility_command_returns_internal_for_created() {
        let spec = MissionSpec {
            title: "T".into(),
            objective: "O".into(),
            target_ref: "main".into(),
            tests: None,
            supervisor_model: None,
            worker_model: None,
            worker_count: None,
            confirm_plan: None,
            scope_paths: vec![],
        };
        let v = mission_event_visibility(MissionEventKindDto(MissionEventKind::Created { spec }));
        assert_eq!(v, EventVisibility::Internal);
    }

    #[test]
    fn visibility_command_returns_inbox_for_completed() {
        let v = mission_event_visibility(MissionEventKindDto(MissionEventKind::Completed {
            summary: "done".into(),
            files_changed: 1,
        }));
        assert!(matches!(v, EventVisibility::Inbox { .. }));
    }
}
