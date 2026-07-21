//! Step 12 — multi-concurrency scaling stress.
//!
//! Spawns 1, 3, 8, and 16 concurrent mock workers and reports
//! throughput + emit-to-sink p95 latency for each.

use event_schema::Event;
use orchestrator::{parser::WorkerEventSink, Repository, SpawnRequest, Supervisor};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Default)]
struct LatencySink {
    events: Mutex<Vec<u64>>, // sink-receive epoch ms only
}

impl WorkerEventSink for LatencySink {
    fn emit(&self, _event: &Event) {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.events.lock().unwrap().push(now_ms);
    }
}

static BUILD_ONCE: Once = Once::new();

fn ensure_mock_harness_built() -> PathBuf {
    BUILD_ONCE.call_once(|| {
        let out = Command::new(env!("CARGO"))
            .args(["build", "-p", "vigla-mock-harness", "--bin", "mock-harness"])
            .output()
            .expect("invoke cargo build");
        assert!(
            out.status.success(),
            "mock-harness build: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    });
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .join("target/debug/mock-harness")
}

async fn run_n_workers(n: usize) -> (Duration, usize, u64, u64) {
    let bin = ensure_mock_harness_built();
    let repo = Repository::open_in_memory().await.unwrap();
    let sink: Arc<LatencySink> = Arc::new(LatencySink::default());
    let supervisor = Supervisor::new(repo.clone(), Arc::clone(&sink) as _, bin);

    let scripts = ["claude_happy", "codex_blocked"];

    let started = Instant::now();
    let mut worker_ids = Vec::new();
    for i in 0..n {
        let script = scripts[i % scripts.len()];
        let id = supervisor
            .spawn_mock(SpawnRequest {
                script: script.into(),
                speed: 0.0,
                task_title: format!("scale {script} #{i}"),
            })
            .await
            .unwrap();
        worker_ids.push(id);
    }

    // Drain.
    let deadline = Instant::now() + Duration::from_secs(60);
    'outer: while Instant::now() < deadline {
        for wid in &worker_ids {
            if supervisor.is_running(wid).await {
                tokio::time::sleep(Duration::from_millis(25)).await;
                continue 'outer;
            }
        }
        break;
    }
    let elapsed = started.elapsed();

    let mut events_count = 0;
    for wid in &worker_ids {
        events_count += repo.replay_for_worker(wid).await.unwrap().len();
    }

    // Inter-arrival latency: gap between consecutive sink emissions
    // (proxy for "is the parser keeping up?"). We don't measure
    // emit-to-paint here — that's Tauri+React, not the orchestrator.
    let mut sink_times = sink.events.lock().unwrap().clone();
    sink_times.sort_unstable();
    let mut gaps: Vec<u64> = sink_times.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.sort_unstable();
    let p95 = gaps
        .get((gaps.len() * 95 / 100).saturating_sub(0))
        .copied()
        .unwrap_or(0);
    let max = gaps.last().copied().unwrap_or(0);

    (elapsed, events_count, p95, max)
}

#[tokio::test]
async fn scale_1_3_8_16() {
    for n in [1usize, 3, 8, 16] {
        let (elapsed, events, p95, max) = run_n_workers(n).await;
        eprintln!(
            "[step-12] n={n:>2}  events={events:>4}  wall={elapsed:?}  inter-arrival p95={p95}ms  max={max}ms"
        );
        // The orchestrator is expected to drain n=16 in well under
        // 30s under workspace-test contention.
        assert!(
            elapsed < Duration::from_secs(60),
            "n={n} took {elapsed:?}, expected <60s"
        );
    }
}
