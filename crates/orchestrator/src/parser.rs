//! Pure ingestion path: read JSONL lines from any [`AsyncBufRead`],
//! persist each event via [`Repository`], and emit it through a
//! [`WorkerEventSink`].
//!
//! Decoupled from process spawning so it can be unit-tested with an
//! in-memory byte stream (`Cursor`-backed). The supervision module
//! plugs in `tokio::process::ChildStdout` at runtime.

use crate::repository::{InsertOutcome, Repository};
use adapter_core::{Adapter, AdapterExit};
use event_schema::{Event, LogStream};
use std::sync::Arc;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};

/// Per-line memory cap for any worker stream we read. Lines exceeding
/// this are truncated and the remainder of the physical line is
/// drained chunk-by-chunk so the next read starts at the next line.
///
/// Without a cap, a real CLI emitting a single huge line (binary
/// blob, runaway JSON, missing newline) would grow the buffer until
/// the host runs out of memory. Step 17 lit up this path by routing
/// real Claude/Codex through `supervise_with_adapter`.
pub(crate) const MAX_LINE_BYTES: usize = 1_048_576; // 1 MiB

/// Outcome of a single capped line read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LineRead {
    /// A line ending in `\n` was read, OR EOF closed a partial line.
    /// `truncated` = true means the physical line exceeded
    /// `MAX_LINE_BYTES` and the trailing bytes were discarded.
    Line { truncated: bool },
    /// The reader yielded no bytes at all on this call.
    Eof,
}

/// Read the next line from `reader` into `out`, capped at `max` bytes.
///
/// `out` is cleared first. Returns `LineRead::Line { truncated: true }`
/// only when the line's content genuinely exceeded `max` (i.e. `max + 1`
/// bytes were read without reaching a newline); in that case `out`
/// contains the first `max` bytes (UTF-8 may be split — `from_utf8_lossy`
/// inserts replacement chars at the truncation boundary) and the rest of
/// the physical line has been drained, so the next call starts at the
/// next line. A line whose content is *exactly* `max` bytes is complete,
/// not truncated: it is returned with `truncated: false` and the drain is
/// skipped, so the following line is never consumed.
pub(crate) async fn read_line_capped<R>(
    reader: &mut R,
    out: &mut String,
    max: usize,
) -> std::io::Result<LineRead>
where
    R: AsyncBufRead + Unpin,
{
    out.clear();
    let mut buf: Vec<u8> = Vec::new();
    // take(max + 1) lets us see one byte past the cap so we can detect
    // overflow without reading unboundedly.
    let n = (&mut *reader)
        .take((max as u64).saturating_add(1))
        .read_until(b'\n', &mut buf)
        .await?;
    if n == 0 {
        return Ok(LineRead::Eof);
    }
    // `read_until` stopped either at the cap (`max + 1` bytes) or at a
    // newline. A line whose content is exactly `max` bytes arrives here as
    // `max + 1` bytes *including* its trailing '\n' — that line is complete,
    // so treat it as normal and, critically, skip the drain below (draining
    // here would consume and silently drop the *next* physical line). Only a
    // buffer that hit the cap with no trailing newline is genuinely overlong.
    let overflowed = buf.len() > max && buf.last() != Some(&b'\n');
    let truncated = if overflowed {
        buf.truncate(max);
        // Drain the rest of the physical line, capped per chunk so
        // the drain itself can't OOM either.
        let mut chunk: Vec<u8> = Vec::with_capacity(4096);
        loop {
            chunk.clear();
            let m = (&mut *reader)
                .take(4096)
                .read_until(b'\n', &mut chunk)
                .await?;
            if m == 0 || chunk.last() == Some(&b'\n') {
                break;
            }
        }
        true
    } else {
        false
    };
    *out = String::from_utf8_lossy(&buf).into_owned();
    Ok(LineRead::Line { truncated })
}

/// Sink receiving canonical events as the orchestrator parses them
/// from a worker's stdout. Implementors typically forward to the
/// frontend via Tauri (`AppHandle::emit`).
pub trait WorkerEventSink: Send + Sync + 'static {
    fn emit(&self, event: &Event);
}

/// Stats returned by [`process_event_stream`] when the stream ends.
/// Useful for tests and supervisor-level logging.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StreamStats {
    pub typed_events: u64,
    pub raw_events: u64,
    pub skipped_lines: u64,
}

/// Read JSONL events from `reader` until EOF.
///
/// For each non-empty line:
/// 1. Try to parse as a typed [`event_schema::Event`]. On success,
///    persist via `repo.insert_event` and emit through `sink`.
/// 2. On typed-parse failure, fall back to envelope extraction +
///    `repo.insert_event_raw` so unknown event types and unknown
///    fields still survive replay (the event-log contract). The
///    line is **not** emitted in this case (the frontend can render
///    unknown events from the persisted log when replay lands in
///    Step 14).
/// 3. If the line is not JSON at all, count as a skip and continue
///    (defensive — mock-harness will never emit such lines, but real
///    adapters might mix human-readable diagnostics into stdout).
///
/// Returns when the reader yields EOF.
pub async fn process_event_stream<R, S>(
    mut reader: R,
    repo: &Repository,
    sink: Arc<S>,
) -> StreamStats
where
    R: AsyncBufRead + Unpin,
    S: WorkerEventSink + ?Sized,
{
    let mut stats = StreamStats::default();
    let mut line = String::new();

    loop {
        match read_line_capped(&mut reader, &mut line, MAX_LINE_BYTES).await {
            Ok(LineRead::Eof) => break,
            Ok(LineRead::Line { truncated }) => {
                if truncated {
                    tracing::warn!(
                        "orchestrator: dropped overlong stdout line (>{MAX_LINE_BYTES} bytes)"
                    );
                    stats.skipped_lines += 1;
                    continue;
                }
            }
            Err(e) => {
                tracing::warn!("orchestrator: read_line error: {e}");
                break;
            }
        }

        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            continue;
        }

        match handle_line(trimmed, repo, sink.as_ref()).await {
            LineOutcome::Typed => stats.typed_events += 1,
            LineOutcome::Raw => stats.raw_events += 1,
            LineOutcome::Skipped => stats.skipped_lines += 1,
        }
    }

    stats
}

enum LineOutcome {
    Typed,
    Raw,
    Skipped,
}

/// Step 10 surface: read raw lines from a worker's stdout (or stderr)
/// and route them through an [`Adapter`] which produces canonical
/// events. Each emitted event is persisted via `repo.insert_event`
/// and forwarded through `sink`. Returns when the reader EOFs and
/// after `adapter.finalize()` has been drained.
pub async fn process_with_adapter<R, A, S>(
    reader: R,
    adapter: &mut A,
    stream: LogStream,
    repo: &Repository,
    sink: Arc<S>,
) -> StreamStats
where
    R: AsyncBufRead + Unpin,
    A: Adapter + ?Sized,
    S: WorkerEventSink + ?Sized,
{
    process_with_adapter_and_memory(reader, adapter, stream, repo, sink, None).await
}

/// As [`process_with_adapter`] but also drains
/// [`Adapter::take_memory_intents`] after each `ingest_line` and after
/// `finalize`, forwarding the intents to the supplied
/// [`crate::memory::MemoryIntentSink`] (Tier-2D).
///
/// `memory_sink == None` produces behaviour byte-identical to
/// `process_with_adapter` — existing call sites keep working without
/// change. When the memory kernel is installed, the worker dispatch
/// path passes `Some(KernelIntentSink::new(...))` so worker proposals
/// flow into the kernel in the same turn they were emitted.
pub async fn process_with_adapter_and_memory<R, A, S>(
    mut reader: R,
    adapter: &mut A,
    stream: LogStream,
    repo: &Repository,
    sink: Arc<S>,
    memory_sink: Option<Arc<dyn crate::memory::MemoryIntentSink>>,
) -> StreamStats
where
    R: AsyncBufRead + Unpin,
    A: Adapter + ?Sized,
    S: WorkerEventSink + ?Sized,
{
    let mut stats = StreamStats::default();
    let mut line = String::new();
    loop {
        match read_line_capped(&mut reader, &mut line, MAX_LINE_BYTES).await {
            Ok(LineRead::Eof) => break,
            Ok(LineRead::Line { truncated }) => {
                if truncated {
                    tracing::warn!(
                        "orchestrator: dropped overlong adapter line (>{MAX_LINE_BYTES} bytes)"
                    );
                    stats.skipped_lines += 1;
                    continue;
                }
            }
            Err(e) => {
                tracing::warn!("orchestrator: read_line error: {e}");
                break;
            }
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            continue;
        }
        let events = adapter.ingest_line(trimmed, stream);
        for event in events {
            persist_and_emit(&event, repo, sink.as_ref()).await;
            stats.typed_events += 1;
        }
        // Drain accumulated memory intents and route them to the
        // sink. The default adapter impl returns empty, so this is a
        // single pointer-comparison + early return for adapters that
        // don't speak `vigla_memory`.
        if let Some(ms) = memory_sink.as_ref() {
            for intent in adapter.take_memory_intents() {
                ms.emit(intent);
            }
        }
    }
    // No process is being supervised here (we're a pure stream
    // consumer), so report a Clean exit — the producer ended via EOF.
    for event in adapter.finalize(AdapterExit::Clean) {
        persist_and_emit(&event, repo, sink.as_ref()).await;
        stats.typed_events += 1;
    }
    if let Some(ms) = memory_sink.as_ref() {
        for intent in adapter.take_memory_intents() {
            ms.emit(intent);
        }
    }
    stats
}

/// Persist `event` and only emit downstream if a fresh row was added.
/// The event-log contract makes the persisted log the source of
/// truth; emitting an event we couldn't persist would desync the live
/// UI from SQLite and from any later replay. Duplicates (seq
/// regressions) are logged inside the repo and skipped here so the UI
/// doesn't render the same event twice.
pub(crate) async fn persist_and_emit<S>(event: &Event, repo: &Repository, sink: &S)
where
    S: WorkerEventSink + ?Sized,
{
    match repo.insert_event(event).await {
        Ok(InsertOutcome::Inserted) => sink.emit(event),
        Ok(InsertOutcome::DuplicateSkipped) => {
            // Already-persisted seq; the repository logged the
            // regression. Skip the duplicate emit.
        }
        Err(e) => tracing::error!("orchestrator: insert_event failed: {e}"),
    }
}

async fn handle_line<S>(line: &str, repo: &Repository, sink: &S) -> LineOutcome
where
    S: WorkerEventSink + ?Sized,
{
    // Step 1: typed parse.
    if let Ok(event) = serde_json::from_str::<Event>(line) {
        return match repo.insert_event(&event).await {
            Ok(InsertOutcome::Inserted) => {
                sink.emit(&event);
                LineOutcome::Typed
            }
            Ok(InsertOutcome::DuplicateSkipped) => LineOutcome::Skipped,
            Err(e) => {
                tracing::error!("orchestrator: insert_event failed: {e}");
                LineOutcome::Skipped
            }
        };
    }

    // Step 2: raw fallback. Parse to a Value, pull out envelope fields.
    let value: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return LineOutcome::Skipped,
    };
    let object = match value.as_object() {
        Some(o) => o,
        None => return LineOutcome::Skipped,
    };
    let worker_id = object
        .get("worker_id")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let task_id = object
        .get("task_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let seq = object.get("seq").and_then(|v| v.as_u64());
    let ts = object.get("ts").and_then(|v| v.as_str()).map(String::from);
    let event_type = object
        .get("type")
        .and_then(|v| v.as_str())
        .map(String::from);
    let schema_version = object
        .get("schema_version")
        .and_then(|v| v.as_str())
        .map(String::from);
    let payload_value = object
        .get("payload")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let (Some(worker_id), Some(seq), Some(ts), Some(event_type), Some(schema_version)) =
        (worker_id, seq, ts, event_type, schema_version)
    else {
        // Envelope incomplete — refuse to persist a malformed row.
        return LineOutcome::Skipped;
    };

    let payload_json = match serde_json::to_string(&payload_value) {
        Ok(s) => s,
        Err(_) => return LineOutcome::Skipped,
    };

    match repo
        .insert_event_raw(
            &worker_id,
            task_id.as_deref(),
            seq,
            &ts,
            &event_type,
            &payload_json,
            &schema_version,
        )
        .await
    {
        Ok(InsertOutcome::Inserted) => LineOutcome::Raw,
        Ok(InsertOutcome::DuplicateSkipped) => LineOutcome::Skipped,
        Err(e) => {
            tracing::error!("orchestrator: insert_event_raw failed: {e}");
            LineOutcome::Skipped
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tokio::io::BufReader;

    #[tokio::test]
    async fn capped_reads_normal_line() {
        let data = b"hello\n";
        let mut reader = BufReader::new(Cursor::new(data));
        let mut out = String::new();
        let r = read_line_capped(&mut reader, &mut out, 1024).await.unwrap();
        assert_eq!(r, LineRead::Line { truncated: false });
        assert_eq!(out, "hello\n");
    }

    #[tokio::test]
    async fn capped_returns_eof_on_empty_stream() {
        let mut reader = BufReader::new(Cursor::new(&b""[..]));
        let mut out = String::new();
        let r = read_line_capped(&mut reader, &mut out, 1024).await.unwrap();
        assert_eq!(r, LineRead::Eof);
    }

    #[tokio::test]
    async fn capped_handles_partial_final_line() {
        let mut reader = BufReader::new(Cursor::new(&b"no-newline-then-eof"[..]));
        let mut out = String::new();
        let r = read_line_capped(&mut reader, &mut out, 1024).await.unwrap();
        assert_eq!(r, LineRead::Line { truncated: false });
        assert_eq!(out, "no-newline-then-eof");

        let r2 = read_line_capped(&mut reader, &mut out, 1024).await.unwrap();
        assert_eq!(r2, LineRead::Eof);
    }

    #[tokio::test]
    async fn capped_truncates_overlong_line() {
        // 2000-byte line with a 100-byte cap.
        let mut data = vec![b'x'; 2000];
        data.push(b'\n');
        let mut reader = BufReader::new(Cursor::new(data));
        let mut out = String::new();
        let r = read_line_capped(&mut reader, &mut out, 100).await.unwrap();
        assert_eq!(r, LineRead::Line { truncated: true });
        assert_eq!(out.len(), 100);
        // Next call sees EOF — the rest of the physical line was drained.
        let r2 = read_line_capped(&mut reader, &mut out, 100).await.unwrap();
        assert_eq!(r2, LineRead::Eof);
    }

    #[tokio::test]
    async fn capped_resyncs_to_next_line_after_overlong() {
        // Overlong line followed by a normal line.
        let mut data = vec![b'x'; 5000];
        data.push(b'\n');
        data.extend_from_slice(b"ok\n");
        let mut reader = BufReader::new(Cursor::new(data));
        let mut out = String::new();

        let r1 = read_line_capped(&mut reader, &mut out, 100).await.unwrap();
        assert_eq!(r1, LineRead::Line { truncated: true });
        assert_eq!(out.len(), 100);

        let r2 = read_line_capped(&mut reader, &mut out, 100).await.unwrap();
        assert_eq!(r2, LineRead::Line { truncated: false });
        assert_eq!(out, "ok\n");
    }

    #[tokio::test]
    async fn capped_line_at_exact_cap_is_complete_not_truncated() {
        // A line whose content is exactly the cap (100 bytes) followed by a
        // newline arrives as 101 bytes *ending in* '\n'. The line is
        // complete, so it must be returned intact and NOT flagged truncated
        // — otherwise the caller would drop a valid event and the drain path
        // would eat the following line (see the regression test below).
        let mut data = vec![b'x'; 100];
        data.push(b'\n');
        let mut reader = BufReader::new(Cursor::new(data));
        let mut out = String::new();
        let r = read_line_capped(&mut reader, &mut out, 100).await.unwrap();
        assert_eq!(r, LineRead::Line { truncated: false });
        // out preserves the full 100-byte content plus its trailing '\n',
        // exactly like any normal line.
        assert_eq!(out.len(), 101);
        assert!(out.starts_with(&"x".repeat(100)));
        assert!(out.ends_with('\n'));
    }

    #[tokio::test]
    async fn capped_exact_cap_line_preserves_following_line() {
        // Regression: an exactly-`max`-byte line immediately followed by a
        // normal line. The exact-cap line must be returned complete, and the
        // FOLLOWING line must survive. Previously the exact-cap line was
        // misclassified as overlong and the drain silently consumed "next".
        let mut data = vec![b'x'; 100];
        data.push(b'\n');
        data.extend_from_slice(b"next\n");
        let mut reader = BufReader::new(Cursor::new(data));
        let mut out = String::new();

        let r1 = read_line_capped(&mut reader, &mut out, 100).await.unwrap();
        assert_eq!(r1, LineRead::Line { truncated: false });
        assert_eq!(out.len(), 101);

        let r2 = read_line_capped(&mut reader, &mut out, 100).await.unwrap();
        assert_eq!(r2, LineRead::Line { truncated: false });
        assert_eq!(out, "next\n");

        let r3 = read_line_capped(&mut reader, &mut out, 100).await.unwrap();
        assert_eq!(r3, LineRead::Eof);
    }

    // Shared test scaffolding for Tier-2D adapter tests.
    #[derive(Debug)]
    struct DropSink;
    impl WorkerEventSink for DropSink {
        fn emit(&self, _event: &Event) {}
    }

    #[derive(Debug)]
    struct CapturingIntentSink(std::sync::Mutex<Vec<adapter_core::MemoryIntent>>);
    impl crate::memory::MemoryIntentSink for CapturingIntentSink {
        fn emit(&self, intent: adapter_core::MemoryIntent) {
            self.0.lock().unwrap().push(intent);
        }
    }

    /// Tier-2D end-to-end: a synthetic Claude stream-json line whose
    /// assistant text contains an `vigla_memory` block flows
    /// through the adapter, the parser drains intents after
    /// `ingest_line`, and a memory sink we own captures them.
    #[tokio::test]
    async fn process_with_adapter_routes_memory_intents_to_sink() {
        use claude_adapter::ClaudeAdapter;
        use std::sync::{Arc, Mutex};

        // Claude stream-json wraps assistant text in a system→assistant→result
        // sequence. We hand it just enough shape to exercise the
        // assistant text handler — the parser is what we're testing.
        let propose_json = r#"{"vigla_memory":{"type":"propose","kind":"hazard","scope":{"kind":"repo"},"body":"x","derived_from":[],"evidence_event_ids":[]}}"#;
        let assistant_line = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": [
                    { "type": "text", "text": format!("Here's my proposal:\n{propose_json}") }
                ]
            }
        })
        .to_string();
        let stream_bytes = format!("{assistant_line}\n");

        let repo = Repository::open_in_memory().await.unwrap();
        let event_sink: Arc<DropSink> = Arc::new(DropSink);
        let intents = Arc::new(CapturingIntentSink(Mutex::new(Vec::new())));
        let intent_sink: Arc<dyn crate::memory::MemoryIntentSink> = intents.clone();
        let mut adapter = ClaudeAdapter::new("w1", Some("t1".to_string()));

        let reader = BufReader::new(Cursor::new(stream_bytes.into_bytes()));
        process_with_adapter_and_memory(
            reader,
            &mut adapter,
            LogStream::Stdout,
            &repo,
            event_sink,
            Some(intent_sink),
        )
        .await;

        let captured = intents.0.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let adapter_core::MemoryIntent::Propose(p) = &captured[0];
        assert_eq!(p.kind, "hazard");
        assert_eq!(p.scope.kind, "repo");
    }

    /// Same end-to-end shape but for the Codex adapter. Codex's
    /// `agent_message` item is the parallel of Claude's
    /// `assistant`/`text` block.
    #[tokio::test]
    async fn process_with_adapter_routes_codex_memory_intents() {
        use codex_adapter::CodexAdapter;
        use std::sync::{Arc, Mutex};

        let propose_json = r#"{"vigla_memory":{"type":"propose","kind":"fact","scope":{"kind":"repo"},"body":"y"}}"#;
        let thread_started = r#"{"type":"thread.started","thread_id":"t-1"}"#.to_string() + "\n";
        let turn_started = r#"{"type":"turn.started"}"#.to_string() + "\n";
        let agent_msg_line = serde_json::json!({
            "type": "item.completed",
            "item": {
                "type": "agent_message",
                "text": format!("Note for ops:\n{propose_json}")
            }
        })
        .to_string();
        let stream_bytes = format!("{thread_started}{turn_started}{agent_msg_line}\n");

        let repo = Repository::open_in_memory().await.unwrap();
        let event_sink: Arc<DropSink> = Arc::new(DropSink);
        let intents = Arc::new(CapturingIntentSink(Mutex::new(Vec::new())));
        let intent_sink: Arc<dyn crate::memory::MemoryIntentSink> = intents.clone();
        let mut adapter = CodexAdapter::new("w1", Some("t1".to_string()));

        let reader = BufReader::new(Cursor::new(stream_bytes.into_bytes()));
        process_with_adapter_and_memory(
            reader,
            &mut adapter,
            LogStream::Stdout,
            &repo,
            event_sink,
            Some(intent_sink),
        )
        .await;

        let captured = intents.0.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let adapter_core::MemoryIntent::Propose(p) = &captured[0];
        assert_eq!(p.kind, "fact");
    }

    /// Memory sink is fully optional: when `None`, intents are
    /// silently dropped. Adapters that don't emit intents pay zero
    /// cost — the per-line `take_memory_intents` call returns an
    /// empty vec by trait default.
    #[tokio::test]
    async fn process_with_adapter_no_memory_sink_drops_intents() {
        use claude_adapter::ClaudeAdapter;
        use std::sync::Arc;

        let propose_json = r#"{"vigla_memory":{"type":"propose","kind":"hazard","scope":{"kind":"repo"},"body":"x"}}"#;
        let assistant_line = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": [{ "type": "text", "text": propose_json }]
            }
        })
        .to_string();
        let stream_bytes = format!("{assistant_line}\n");

        let repo = Repository::open_in_memory().await.unwrap();
        let event_sink: Arc<DropSink> = Arc::new(DropSink);
        let mut adapter = ClaudeAdapter::new("w1", Some("t1".to_string()));

        let reader = BufReader::new(Cursor::new(stream_bytes.into_bytes()));
        // Use the no-memory path on purpose — proves the legacy entry
        // point keeps working byte-for-byte.
        let stats =
            process_with_adapter(reader, &mut adapter, LogStream::Stdout, &repo, event_sink).await;
        // The adapter still ran and emitted canonical events; the
        // intents were simply not routed anywhere.
        assert!(stats.typed_events > 0);
    }
}
