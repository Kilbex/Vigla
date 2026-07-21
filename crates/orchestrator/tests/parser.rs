//! Parser unit tests. Use a Cursor-backed byte stream as the
//! "stdout" so we can assert the persistence + emission contract
//! without actually spawning a worker.

use event_schema::Event;
use orchestrator::parser::{process_event_stream, WorkerEventSink};
use orchestrator::Repository;
use std::sync::{Arc, Mutex};
use tokio::io::BufReader;

#[derive(Default)]
struct CapturingSink {
    events: Mutex<Vec<Event>>,
}

impl WorkerEventSink for CapturingSink {
    fn emit(&self, event: &Event) {
        self.events.lock().unwrap().push(event.clone());
    }
}

#[tokio::test]
async fn parses_persists_and_emits_typed_events() {
    let repo = Repository::open_in_memory().await.unwrap();
    let sink = Arc::new(CapturingSink::default());

    let lines = [
        r#"{"schema_version":"1.0","worker_id":"w1","task_id":null,"seq":0,"ts":"2026-05-08T19:43:00.000Z","type":"state_change","payload":{"state":"idle"}}"#,
        r#"{"schema_version":"1.0","worker_id":"w1","task_id":"t1","seq":1,"ts":"2026-05-08T19:43:01.000Z","type":"state_change","payload":{"state":"executing","from":"idle"}}"#,
        r#"{"schema_version":"1.0","worker_id":"w1","task_id":"t1","seq":2,"ts":"2026-05-08T19:43:02.000Z","type":"log","payload":{"level":"info","stream":"stdout","line":"hello"}}"#,
    ]
    .join("\n")
        + "\n";

    let reader = BufReader::new(lines.as_bytes());
    let stats =
        process_event_stream(reader, &repo, Arc::clone(&sink) as Arc<dyn WorkerEventSink>).await;

    assert_eq!(stats.typed_events, 3);
    assert_eq!(stats.raw_events, 0);
    assert_eq!(stats.skipped_lines, 0);

    // Persisted in seq order.
    let replay = repo.replay_for_worker("w1").await.unwrap();
    assert_eq!(replay.len(), 3);
    for (i, e) in replay.iter().enumerate() {
        assert_eq!(e.seq, i as u64);
    }

    // Emitted in arrival order, identical content.
    let emitted = sink.events.lock().unwrap().clone();
    assert_eq!(emitted, replay);
}

#[tokio::test]
async fn skips_blank_lines_and_garbage_but_processes_around_them() {
    let repo = Repository::open_in_memory().await.unwrap();
    let sink = Arc::new(CapturingSink::default());

    let body = "\n\
        not json at all\n\
        \n\
        {\"schema_version\":\"1.0\",\"worker_id\":\"w1\",\"task_id\":null,\"seq\":0,\"ts\":\"2026-05-08T00:00:00.000Z\",\"type\":\"state_change\",\"payload\":{\"state\":\"idle\"}}\n\
        \n";

    let reader = BufReader::new(body.as_bytes());
    let stats =
        process_event_stream(reader, &repo, Arc::clone(&sink) as Arc<dyn WorkerEventSink>).await;

    assert_eq!(stats.typed_events, 1);
    assert_eq!(stats.raw_events, 0);
    assert_eq!(
        stats.skipped_lines, 1,
        "the `not json at all` line should count as a skip"
    );

    assert_eq!(sink.events.lock().unwrap().len(), 1);
    assert_eq!(repo.replay_for_worker("w1").await.unwrap().len(), 1);
}

#[tokio::test]
async fn unknown_event_type_falls_back_to_raw_persistence() {
    // Forward-compat: a future schema version may add new event types.
    // We must persist them verbatim (event-schema.md §6) without
    // crashing or emitting through the typed sink.
    let repo = Repository::open_in_memory().await.unwrap();
    let sink = Arc::new(CapturingSink::default());

    let line = r#"{"schema_version":"1.1","worker_id":"w1","task_id":null,"seq":0,"ts":"2026-05-08T00:00:00.000Z","type":"future_event","payload":{"foo":42}}"#;
    let body = format!("{line}\n");
    let reader = BufReader::new(body.as_bytes());

    let stats =
        process_event_stream(reader, &repo, Arc::clone(&sink) as Arc<dyn WorkerEventSink>).await;

    assert_eq!(stats.typed_events, 0);
    assert_eq!(stats.raw_events, 1);
    assert_eq!(stats.skipped_lines, 0);

    // Sink does NOT receive unknown types — frontend can render them
    // from the persisted log when replay lands in Step 14.
    assert_eq!(sink.events.lock().unwrap().len(), 0);
}

#[tokio::test]
async fn malformed_envelope_is_skipped_not_persisted() {
    // Missing required envelope fields should not crash and should
    // not persist a half-row.
    let repo = Repository::open_in_memory().await.unwrap();
    let sink = Arc::new(CapturingSink::default());

    let body = "\
        {\"schema_version\":\"1.0\",\"task_id\":null,\"type\":\"unknown\",\"payload\":{}}\n\
        {\"worker_id\":\"w1\",\"seq\":0}\n\
        []\n\
        true\n";

    let reader = BufReader::new(body.as_bytes());
    let stats =
        process_event_stream(reader, &repo, Arc::clone(&sink) as Arc<dyn WorkerEventSink>).await;

    assert_eq!(stats.typed_events, 0);
    assert_eq!(stats.raw_events, 0);
    assert_eq!(stats.skipped_lines, 4);
}
