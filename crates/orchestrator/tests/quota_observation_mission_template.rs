//! Verifies that the quota-observation `MissionSpec` JSON fixture
//! (`tests/fixtures/quota-observation-mission.json`) parses cleanly
//! under `serde_json` against the current `MissionSpec` shape and
//! passes `MissionSpec::validate()`. The fixture drives a reproducible
//! quota-observation session via `scripts/observe-quota.sh`; keeping it
//! in lock-step with the production struct prevents that script from
//! silently rotting on a `MissionSpec` field rename.

use orchestrator::mission::MissionSpec;

#[test]
fn quota_observation_mission_template_parses_and_validates() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/quota-observation-mission.json");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let spec: MissionSpec =
        serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("parse quota mission JSON: {e}"));
    spec.validate().expect("MissionSpec::validate");

    assert_eq!(spec.title, "L1 row-4 quota observation");
    assert_eq!(spec.target_ref, "main");
    assert_eq!(spec.supervisor_model.as_deref(), Some("claude"));
    assert_eq!(
        spec.worker_model.as_deref(),
        Some(orchestrator::mission_supervisor_run::L1_CLAUDE_QUOTA_EXHAUSTED_WORKER_MODEL)
    );
    assert_eq!(spec.worker_count, Some(1));
    assert_eq!(
        spec.scope_paths.len(),
        1,
        "scope is intentionally a single file so repeated replays don't drift"
    );
}
