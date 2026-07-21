//! Regression test for the stderr-EOF spin in `supervise_with_adapter`.
//!
//! Before the fix, the select! arm reading from stderr did not disable
//! itself once the stream closed. `read_line_capped` returned
//! `LineRead::Eof` immediately on every poll, so the loop tight-cycled
//! on the closed branch — burning CPU and starving stdout reads / the
//! cancel branch. The fix tracks `stderr_eof` and gates the arm with
//! `if !stderr_eof` so it stops competing once exhausted.
//!
//! This test spawns `/bin/sh -c 'exec 2>&-; printf "..."'` which closes
//! stderr immediately, then writes a known number of stdout lines.
//! After supervise completes, every stdout line must be observed AND
//! the test must finish well under any normal CI budget. With the bug
//! the test could still pass (busy-loops still yield in tokio) but
//! the line count verifies that stdout reads weren't starved.

use adapter_core::Adapter;
use event_schema::{Event, EventKind, Log, LogLevel, LogStream};
use orchestrator::{parser::WorkerEventSink, Repository, Supervisor};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::Instant;
use tokio::process::Command;
use tokio::sync::oneshot;

#[derive(Default)]
struct CountingSink {
    events: Mutex<Vec<Event>>,
}

impl WorkerEventSink for CountingSink {
    fn emit(&self, event: &Event) {
        self.events.lock().unwrap().push(event.clone());
    }
}

/// Adapter that turns every stdout line into a single Log event so we
/// can count lines through the sink. Stderr lines are ignored (the
/// real stream is closed in this test anyway).
#[derive(Debug)]
struct CountingAdapter {
    worker_id: String,
    seq: u64,
}

impl Adapter for CountingAdapter {
    fn ingest_line(&mut self, line: &str, stream: LogStream) -> Vec<Event> {
        if !matches!(stream, LogStream::Stdout) {
            return vec![];
        }
        self.seq += 1;
        vec![Event {
            schema_version: "1.0".into(),
            worker_id: self.worker_id.clone(),
            task_id: None,
            seq: self.seq,
            ts: "2026-05-09T00:00:00.000Z".into(),
            kind: EventKind::Log(Log {
                level: LogLevel::Info,
                stream: LogStream::Stdout,
                line: line.to_owned(),
                tag: None,
            }),
        }]
    }
}

#[tokio::test]
async fn supervise_handles_early_stderr_eof_without_starving_stdout() {
    let repo = Repository::open_in_memory().await.unwrap();
    let sink: Arc<CountingSink> = Arc::new(CountingSink::default());
    let supervisor = Supervisor::new(
        repo.clone(),
        Arc::clone(&sink) as Arc<dyn WorkerEventSink>,
        PathBuf::new(),
    );

    let worker_id = "stderr-eof-test-worker".to_string();
    let now = "2026-05-09T00:00:00.000Z".to_string();
    repo.insert_worker(&event_schema::WorkerInfo {
        id: worker_id.clone(),
        name: "stderr-eof-test".into(),
        vendor: event_schema::Vendor::Mock,
        cli_binary: "sh".into(),
        cli_version: None,
        cwd: ".".into(),
        model: None,
        spawned_at: now,
        ended_at: None,
    })
    .await
    .unwrap();

    // Close stderr (`exec 2>&-`) then emit 5 stdout lines back-to-back.
    // No sleeps — we want stdout to be the only thing the loop has to
    // do, so any spin on the closed stderr would visibly degrade the
    // stdout pump.
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg("exec 2>&-; printf 'a\\nb\\nc\\nd\\ne\\n'")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn /bin/sh");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    let (_cancel_tx, cancel_rx) = oneshot::channel();
    let started = Instant::now();
    Arc::clone(&supervisor)
        .supervise_with_adapter(
            child,
            stdout,
            stderr,
            Box::new(CountingAdapter {
                worker_id: worker_id.clone(),
                seq: 0,
            }),
            worker_id,
            cancel_rx,
        )
        .await;
    let elapsed = started.elapsed();

    let events = sink.events.lock().unwrap();
    assert_eq!(
        events.len(),
        5,
        "all 5 stdout lines must be observed; got {}",
        events.len()
    );
    // Generous bound — the actual work is microseconds. If the test
    // ever exceeds this on CI it's a sign the busy-loop is back.
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "supervise took {elapsed:?}, expected well under 5s"
    );
}

/// Closing stdout is not process completion. Some CLIs hand work to a child,
/// close their output stream, and remain alive. Cancellation must stay live
/// instead of entering a blocking post-exit drain/wait path.
#[tokio::test]
async fn supervise_stays_cancellable_after_stdout_eof() {
    let repo = Repository::open_in_memory().await.unwrap();
    let sink: Arc<CountingSink> = Arc::new(CountingSink::default());
    let supervisor = Supervisor::new(
        repo.clone(),
        Arc::clone(&sink) as Arc<dyn WorkerEventSink>,
        PathBuf::new(),
    );

    let worker_id = "stdout-eof-test-worker".to_string();
    repo.insert_worker(&event_schema::WorkerInfo {
        id: worker_id.clone(),
        name: "stdout-eof-test".into(),
        vendor: event_schema::Vendor::Mock,
        cli_binary: "sh".into(),
        cli_version: None,
        cwd: ".".into(),
        model: None,
        spawned_at: "2026-05-09T00:00:00.000Z".into(),
        ended_at: None,
    })
    .await
    .unwrap();

    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("exec 1>&-; exec /bin/sleep 30")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().expect("spawn /bin/sh");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    let (cancel_tx, cancel_rx) = oneshot::channel();
    let cancel = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = cancel_tx.send(());
    });
    let started = Instant::now();
    Arc::clone(&supervisor)
        .supervise_with_adapter(
            child,
            stdout,
            stderr,
            Box::new(CountingAdapter {
                worker_id: worker_id.clone(),
                seq: 0,
            }),
            worker_id,
            cancel_rx,
        )
        .await;
    cancel.await.unwrap();

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "stdout EOF must not disable cancellation"
    );
}
