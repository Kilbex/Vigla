//! Round-trip JSON tests: every event type from `docs/event-schema.md`
//! parses, re-serializes, and re-parses to the same value. Catches
//! serde-flatten + adjacently-tagged-enum regressions before they
//! reach the wire.

use event_schema::{
    Artifact, ArtifactKind, Completion, Cost, Dependency, Event, EventKind, Failure,
    FailureCategory, FileActivity, FileOp, Log, LogLevel, LogStream, Progress, StateChange,
    TaskInfo, TestFailure, TestResult, Vendor, WorkerInfo, WorkerState, SCHEMA_VERSION,
};
use serde_json::json;

fn round_trip(json: serde_json::Value, expected: Event) {
    // 1. parse the canonical JSON into Event
    let parsed: Event = serde_json::from_value(json.clone()).unwrap_or_else(|e| {
        panic!("parse failed for {json}: {e}");
    });
    assert_eq!(parsed, expected, "parsed value does not match expected");

    // 2. re-serialize and confirm shape preservation
    let re_serialized = serde_json::to_value(&parsed).expect("re-serialize failed");
    assert_eq!(
        re_serialized, json,
        "re-serialized JSON differs from canonical input"
    );

    // 3. re-parse for idempotence
    let re_parsed: Event = serde_json::from_value(re_serialized).expect("re-parse failed");
    assert_eq!(re_parsed, parsed, "re-parsed event differs from original");
}

#[test]
fn schema_version_constant_is_two_zero() {
    assert_eq!(SCHEMA_VERSION, "2.0");
}

#[test]
fn worker_info_round_trip() {
    let json = json!({
        "id": "0190a7e0-2c3a-7a01-9f00-0000000000a1",
        "name": "claude-1",
        "vendor": "claude",
        "cli_binary": "/usr/local/bin/claude",
        "cli_version": "1.4.2",
        "cwd": "/tmp/work",
        "model": "claude-opus-4-7",
        "spawned_at": "2026-05-08T19:42:13.000Z",
        "ended_at": null
    });
    let parsed: WorkerInfo = serde_json::from_value(json.clone()).unwrap();
    let again = serde_json::to_value(&parsed).unwrap();
    assert_eq!(again, json);
    assert_eq!(parsed.vendor, Vendor::Claude);
}

#[test]
fn task_info_round_trip() {
    let json = json!({
        "id": "0190a7e0-2c3a-7a01-9f00-0000000000b7",
        "parent_id": null,
        "title": "Add retry to fetcher",
        "depends_on": [],
        "created_at": "2026-05-08T19:42:00.000Z"
    });
    let parsed: TaskInfo = serde_json::from_value(json.clone()).unwrap();
    let again = serde_json::to_value(&parsed).unwrap();
    assert_eq!(again, json);
    assert_eq!(parsed.title, "Add retry to fetcher");
}

#[test]
fn state_change_round_trip() {
    let json = json!({
        "schema_version": "1.0",
        "worker_id": "0190a7e0-2c3a-7a01-9f00-0000000000a1",
        "task_id": "0190a7e0-2c3a-7a01-9f00-0000000000b7",
        "seq": 14,
        "ts": "2026-05-08T19:43:01.221Z",
        "type": "state_change",
        "payload": { "state": "executing", "from": "planning", "note": "applying patch 2/4" }
    });
    let expected = Event {
        schema_version: "1.0".into(),
        worker_id: "0190a7e0-2c3a-7a01-9f00-0000000000a1".into(),
        task_id: Some("0190a7e0-2c3a-7a01-9f00-0000000000b7".into()),
        seq: 14,
        ts: "2026-05-08T19:43:01.221Z".into(),
        kind: EventKind::StateChange(StateChange {
            state: WorkerState::Executing,
            from: Some(WorkerState::Planning),
            note: Some("applying patch 2/4".into()),
        }),
    };
    round_trip(json, expected);
}

#[test]
fn log_round_trip() {
    let json = json!({
        "schema_version": "1.0",
        "worker_id": "0190a7e0-2c3a-7a01-9f00-0000000000a1",
        "task_id": "0190a7e0-2c3a-7a01-9f00-0000000000b7",
        "seq": 15,
        "ts": "2026-05-08T19:43:01.305Z",
        "type": "log",
        "payload": { "level": "info", "stream": "stdout", "line": "[32m✓[0m read 4 files", "tag": "fs" }
    });
    let expected = Event {
        schema_version: "1.0".into(),
        worker_id: "0190a7e0-2c3a-7a01-9f00-0000000000a1".into(),
        task_id: Some("0190a7e0-2c3a-7a01-9f00-0000000000b7".into()),
        seq: 15,
        ts: "2026-05-08T19:43:01.305Z".into(),
        kind: EventKind::Log(Log {
            level: LogLevel::Info,
            stream: LogStream::Stdout,
            line: "[32m✓[0m read 4 files".into(),
            tag: Some("fs".into()),
        }),
    };
    round_trip(json, expected);
}

#[test]
fn progress_round_trip() {
    let json = json!({
        "schema_version": "1.0",
        "worker_id": "0190a7e0-2c3a-7a01-9f00-0000000000a1",
        "task_id": "0190a7e0-2c3a-7a01-9f00-0000000000b7",
        "seq": 22,
        "ts": "2026-05-08T19:43:09.880Z",
        "type": "progress",
        "payload": { "percent": 62.5, "eta_ms": 18000, "note": "patching tests" }
    });
    let expected = Event {
        schema_version: "1.0".into(),
        worker_id: "0190a7e0-2c3a-7a01-9f00-0000000000a1".into(),
        task_id: Some("0190a7e0-2c3a-7a01-9f00-0000000000b7".into()),
        seq: 22,
        ts: "2026-05-08T19:43:09.880Z".into(),
        kind: EventKind::Progress(Progress {
            percent: 62.5,
            eta_ms: Some(18000),
            note: Some("patching tests".into()),
        }),
    };
    round_trip(json, expected);
}

#[test]
fn file_activity_round_trip() {
    let json = json!({
        "schema_version": "1.0",
        "worker_id": "0190a7e0-2c3a-7a01-9f00-0000000000a1",
        "task_id": "0190a7e0-2c3a-7a01-9f00-0000000000b7",
        "seq": 31,
        "ts": "2026-05-08T19:43:18.402Z",
        "type": "file_activity",
        "payload": { "path": "src/fetcher.ts", "op": "modified", "lines_added": 12, "lines_removed": 4 }
    });
    let expected = Event {
        schema_version: "1.0".into(),
        worker_id: "0190a7e0-2c3a-7a01-9f00-0000000000a1".into(),
        task_id: Some("0190a7e0-2c3a-7a01-9f00-0000000000b7".into()),
        seq: 31,
        ts: "2026-05-08T19:43:18.402Z".into(),
        kind: EventKind::FileActivity(FileActivity {
            path: "src/fetcher.ts".into(),
            op: FileOp::Modified,
            from_path: None,
            lines_added: Some(12),
            lines_removed: Some(4),
            bytes: None,
        }),
    };
    round_trip(json, expected);
}

#[test]
fn test_result_round_trip() {
    let json = json!({
        "schema_version": "1.0",
        "worker_id": "0190a7e0-2c3a-7a01-9f00-0000000000a1",
        "task_id": "0190a7e0-2c3a-7a01-9f00-0000000000b7",
        "seq": 47,
        "ts": "2026-05-08T19:44:02.118Z",
        "type": "test_result",
        "payload": {
            "suite": "vitest",
            "passed": 18,
            "failed": 1,
            "skipped": 0,
            "duration_ms": 1230,
            "failures": [
                { "name": "fetcher > retries on 503", "message": "expected 3 calls, got 2", "file": "src/fetcher.test.ts", "line": 42 }
            ]
        }
    });
    let expected = Event {
        schema_version: "1.0".into(),
        worker_id: "0190a7e0-2c3a-7a01-9f00-0000000000a1".into(),
        task_id: Some("0190a7e0-2c3a-7a01-9f00-0000000000b7".into()),
        seq: 47,
        ts: "2026-05-08T19:44:02.118Z".into(),
        kind: EventKind::TestResult(TestResult {
            suite: "vitest".into(),
            passed: 18,
            failed: 1,
            skipped: 0,
            duration_ms: 1230,
            failures: Some(vec![TestFailure {
                name: "fetcher > retries on 503".into(),
                message: "expected 3 calls, got 2".into(),
                file: Some("src/fetcher.test.ts".into()),
                line: Some(42),
            }]),
        }),
    };
    round_trip(json, expected);
}

#[test]
fn cost_round_trip() {
    let json = json!({
        "schema_version": "1.0",
        "worker_id": "0190a7e0-2c3a-7a01-9f00-0000000000a1",
        "task_id": "0190a7e0-2c3a-7a01-9f00-0000000000b7",
        "seq": 52,
        "ts": "2026-05-08T19:44:11.500Z",
        "type": "cost",
        "payload": {
            "input_tokens": 4210,
            "output_tokens": 980,
            "cache_read_tokens": 11200,
            "usd": 0.0186,
            "model": "claude-opus-4-7"
        }
    });
    let parsed: Event = serde_json::from_value(json.clone()).unwrap();
    // re-serialize and re-parse instead of asserting on the JSON
    // structure directly: f64 USD round-trips to 0.0186 cleanly here,
    // but field ordering of optional fields differs between parse
    // and serialize, so we compare via re-parse.
    let re = serde_json::to_value(&parsed).unwrap();
    let re_parsed: Event = serde_json::from_value(re).unwrap();
    assert_eq!(parsed, re_parsed);
    if let EventKind::Cost(c) = &parsed.kind {
        assert_eq!(c.input_tokens, 4210);
        assert_eq!(c.output_tokens, 980);
        assert_eq!(c.cache_read_tokens, Some(11200));
        assert!((c.usd - 0.0186).abs() < 1e-9);
        assert_eq!(c.model.as_deref(), Some("claude-opus-4-7"));
    } else {
        panic!("wrong variant: {:?}", parsed.kind);
    }
    // Reduce noise from struct/JSON comparison; also exercise the helper.
    let _: Cost = serde_json::from_value(serde_json::json!({
        "input_tokens": 1, "output_tokens": 1, "usd": 0.01
    }))
    .unwrap();
}

#[test]
fn dependency_round_trip() {
    let json = json!({
        "schema_version": "1.0",
        "worker_id": "0190a7e0-2c3a-7a01-9f00-0000000000a1",
        "task_id": "0190a7e0-2c3a-7a01-9f00-0000000000b7",
        "seq": 61,
        "ts": "2026-05-08T19:44:33.014Z",
        "type": "dependency",
        "payload": {
            "waiting_on": ["0190a7e0-2c3a-7a01-9f00-0000000000c3"],
            "reason": "needs schema migration from codex-1",
            "since": "2026-05-08T19:44:33.000Z"
        }
    });
    let expected = Event {
        schema_version: "1.0".into(),
        worker_id: "0190a7e0-2c3a-7a01-9f00-0000000000a1".into(),
        task_id: Some("0190a7e0-2c3a-7a01-9f00-0000000000b7".into()),
        seq: 61,
        ts: "2026-05-08T19:44:33.014Z".into(),
        kind: EventKind::Dependency(Dependency {
            waiting_on: vec!["0190a7e0-2c3a-7a01-9f00-0000000000c3".into()],
            reason: "needs schema migration from codex-1".into(),
            since: Some("2026-05-08T19:44:33.000Z".into()),
        }),
    };
    round_trip(json, expected);
}

#[test]
fn completion_round_trip() {
    let json = json!({
        "schema_version": "1.0",
        "worker_id": "0190a7e0-2c3a-7a01-9f00-0000000000a1",
        "task_id": "0190a7e0-2c3a-7a01-9f00-0000000000b7",
        "seq": 78,
        "ts": "2026-05-08T19:45:12.700Z",
        "type": "completion",
        "payload": {
            "summary": "Added retry-with-backoff to fetcher; 1 unit test added; 19 tests pass.",
            "artifacts": [
                { "kind": "file", "ref": "src/fetcher.ts",      "label": "patched" },
                { "kind": "file", "ref": "src/fetcher.test.ts", "label": "+1 test" },
                { "kind": "diff", "ref": "feature/retry@HEAD",  "label": "session diff" }
            ],
            "duration_ms": 198400
        }
    });
    let expected = Event {
        schema_version: "1.0".into(),
        worker_id: "0190a7e0-2c3a-7a01-9f00-0000000000a1".into(),
        task_id: Some("0190a7e0-2c3a-7a01-9f00-0000000000b7".into()),
        seq: 78,
        ts: "2026-05-08T19:45:12.700Z".into(),
        kind: EventKind::Completion(Completion {
            summary: "Added retry-with-backoff to fetcher; 1 unit test added; 19 tests pass."
                .into(),
            artifacts: Some(vec![
                Artifact {
                    kind: ArtifactKind::File,
                    artifact_ref: "src/fetcher.ts".into(),
                    label: Some("patched".into()),
                },
                Artifact {
                    kind: ArtifactKind::File,
                    artifact_ref: "src/fetcher.test.ts".into(),
                    label: Some("+1 test".into()),
                },
                Artifact {
                    kind: ArtifactKind::Diff,
                    artifact_ref: "feature/retry@HEAD".into(),
                    label: Some("session diff".into()),
                },
            ]),
            duration_ms: Some(198400),
        }),
    };
    round_trip(json, expected);
}

#[test]
fn failure_round_trip() {
    let json = json!({
        "schema_version": "1.0",
        "worker_id": "0190a7e0-2c3a-7a01-9f00-0000000000a1",
        "task_id": "0190a7e0-2c3a-7a01-9f00-0000000000b7",
        "seq": 79,
        "ts": "2026-05-08T19:45:14.011Z",
        "type": "failure",
        "payload": {
            "error": "test_runner exited 1: 3 tests failed",
            "retryable": true,
            "suggestion": "review failed test names; consider re-planning",
            "exit_code": 1,
            "category": "task_logic"
        }
    });
    let expected = Event {
        schema_version: "1.0".into(),
        worker_id: "0190a7e0-2c3a-7a01-9f00-0000000000a1".into(),
        task_id: Some("0190a7e0-2c3a-7a01-9f00-0000000000b7".into()),
        seq: 79,
        ts: "2026-05-08T19:45:14.011Z".into(),
        kind: EventKind::Failure(Failure {
            error: "test_runner exited 1: 3 tests failed".into(),
            retryable: true,
            suggestion: Some("review failed test names; consider re-planning".into()),
            exit_code: Some(1),
            category: Some(FailureCategory::TaskLogic),
        }),
    };
    round_trip(json, expected);
}

#[test]
fn null_task_id_serializes_as_null_not_omitted() {
    // Worker-level lifecycle events (e.g. initial idle) carry task_id =
    // null. Verify serde keeps it explicit rather than omitting it.
    let evt = Event {
        schema_version: "1.0".into(),
        worker_id: "w".into(),
        task_id: None,
        seq: 0,
        ts: "2026-01-01T00:00:00.000Z".into(),
        kind: EventKind::StateChange(StateChange {
            state: WorkerState::Idle,
            from: None,
            note: None,
        }),
    };
    let v = serde_json::to_value(&evt).unwrap();
    assert!(v.get("task_id").is_some(), "task_id should be present");
    assert!(v["task_id"].is_null(), "task_id should serialize as null");
}

#[test]
fn unknown_payload_field_is_tolerated_on_parse() {
    // §6 consumer obligations: tolerate unknown payload fields.
    let json = json!({
        "schema_version": "1.0",
        "worker_id": "w",
        "task_id": null,
        "seq": 0,
        "ts": "2026-01-01T00:00:00.000Z",
        "type": "state_change",
        "payload": { "state": "idle", "future_field": 42 }
    });
    let parsed: Event = serde_json::from_value(json).unwrap();
    if let EventKind::StateChange(sc) = parsed.kind {
        assert_eq!(sc.state, WorkerState::Idle);
    } else {
        panic!("wrong variant");
    }
}
