//! Regression test for the missing `CoordinatingSink` wrap on the
//! real-CLI supervise path (`supervise_with_adapter`).
//!
//! Before the fix, `supervise()` (mock path) wrapped the user sink in a
//! `CoordinatingSink` per-call but `supervise_with_adapter()` (Claude /
//! Codex path) used the raw user sink, so a downstream task waiting on
//! a real-CLI upstream never unblocked. This test feeds a fake adapter
//! that emits a single `Completion` event through `supervise_with_adapter`
//! and asserts that the supervisor records the task as completed —
//! exercise of the wrap, not of any real CLI.

use adapter_core::Adapter;
use event_schema::{Completion, Event, EventKind, LogStream};
use orchestrator::{parser::WorkerEventSink, Repository, Supervisor};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::oneshot;

#[derive(Default)]
struct CapturingSink {
    events: Mutex<Vec<Event>>,
}

impl WorkerEventSink for CapturingSink {
    fn emit(&self, event: &Event) {
        self.events.lock().unwrap().push(event.clone());
    }
}

/// Adapter that ignores child output and emits exactly one Completion
/// event on the first stdout line, then nothing.
#[derive(Debug)]
struct CompletionEmittingAdapter {
    worker_id: String,
    task_id: String,
    emitted: bool,
}

impl Adapter for CompletionEmittingAdapter {
    fn ingest_line(&mut self, _line: &str, _stream: LogStream) -> Vec<Event> {
        if self.emitted {
            return vec![];
        }
        self.emitted = true;
        vec![Event {
            schema_version: "1.0".into(),
            worker_id: self.worker_id.clone(),
            task_id: Some(self.task_id.clone()),
            seq: 1,
            ts: "2026-05-09T00:00:00.000Z".into(),
            kind: EventKind::Completion(Completion {
                summary: "fake-real-cli completion".into(),
                artifacts: None,
                duration_ms: None,
            }),
        }]
    }
}

#[tokio::test]
async fn supervise_with_adapter_wraps_sink_for_coordination() {
    let repo = Repository::open_in_memory().await.unwrap();
    let sink: Arc<CapturingSink> = Arc::new(CapturingSink::default());
    let supervisor = Supervisor::new(
        repo.clone(),
        Arc::clone(&sink) as Arc<dyn WorkerEventSink>,
        // mock_harness path is irrelevant for this test (we don't go
        // through spawn_mock); use an empty placeholder.
        PathBuf::new(),
    );

    // Insert worker + task records so update_worker_state has a row to
    // touch. The IDs are fixed strings; UUIDs aren't required for this
    // exercise of the coordination wrap.
    let worker_id = "test-worker-real-cli".to_string();
    let task_id = "test-task-real-cli".to_string();
    let now = "2026-05-09T00:00:00.000Z".to_string();

    repo.insert_worker(&event_schema::WorkerInfo {
        id: worker_id.clone(),
        name: "fake-claude".into(),
        vendor: event_schema::Vendor::Claude,
        cli_binary: "fake".into(),
        cli_version: None,
        cwd: ".".into(),
        model: None,
        spawned_at: now.clone(),
        ended_at: None,
    })
    .await
    .unwrap();
    repo.insert_task(&event_schema::TaskInfo {
        id: task_id.clone(),
        parent_id: None,
        title: "fake task".into(),
        depends_on: vec![],
        created_at: now,
    })
    .await
    .unwrap();

    // Tiny child process: emit one line then exit. Any short-lived
    // command that pipes one line is fine; we only need stdout EOF to
    // arrive quickly.
    let mut child = Command::new("printf")
        .arg("x\n")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn printf");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    let (_cancel_tx, cancel_rx) = oneshot::channel();
    Arc::clone(&supervisor)
        .supervise_with_adapter(
            child,
            stdout,
            stderr,
            Box::new(CompletionEmittingAdapter {
                worker_id: worker_id.clone(),
                task_id: task_id.clone(),
                emitted: false,
            }),
            worker_id,
            cancel_rx,
        )
        .await;

    // CoordinatingSink::emit spawns the side-effect task on tokio; give
    // it a moment to run so the completed_tasks set is updated. (The
    // coordinating-sink ordering fix in H1 will turn this poll into a
    // direct await; for now, the small delay is enough.)
    let mut completed = false;
    for _ in 0..40 {
        if supervisor.task_completed(&task_id).await {
            completed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        completed,
        "task_completed({task_id}) should be true after \
         supervise_with_adapter saw a Completion event — \
         the coordinating-sink wrap on the real-CLI path is missing",
    );

    // Also assert the sink saw the event (sanity: the user sink is the
    // inner of the coordinating sink and must receive every event).
    let events = sink.events.lock().unwrap();
    assert_eq!(
        events.len(),
        1,
        "expected exactly the one Completion event to reach the user sink"
    );
    assert!(matches!(events[0].kind, EventKind::Completion(_)));
}
