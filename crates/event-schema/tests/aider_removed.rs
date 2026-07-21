//! Schema 2.0 — Aider removal regression locks.
//!
//! - SCHEMA_VERSION must be bumped to 2.0.
//! - `vendor: "aider"` must no longer deserialize.

use event_schema::{Vendor, WorkerInfo, SCHEMA_VERSION};

#[test]
fn schema_version_is_two_zero_after_aider_removal() {
    assert_eq!(SCHEMA_VERSION, "2.0");
}

#[test]
fn aider_vendor_no_longer_deserializes() {
    let json = r#"{
        "id": "x", "name": "x", "vendor": "aider",
        "cli_binary": "x", "cli_version": null, "cwd": ".",
        "model": null, "spawned_at": "2026-05-10T00:00:00.000Z",
        "ended_at": null
    }"#;
    let r: Result<WorkerInfo, _> = serde_json::from_str(json);
    assert!(
        r.is_err(),
        "expected aider deserialization to fail post-removal, got {r:?}"
    );
}

#[test]
fn gemini_vendor_still_deserializes() {
    let json = r#"{
        "id": "x", "name": "x", "vendor": "gemini",
        "cli_binary": "gemini", "cli_version": null, "cwd": ".",
        "model": null, "spawned_at": "2026-05-10T00:00:00.000Z",
        "ended_at": null
    }"#;
    let w: WorkerInfo = serde_json::from_str(json).expect("gemini must deserialize");
    assert_eq!(w.vendor, Vendor::Gemini);
}
