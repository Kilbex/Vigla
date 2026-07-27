//! Golden-transcript conformance suite for [`GeminiAdapter`].
//!
//! Every transcript line is a shape `GeminiAdapter::ingest_line` already
//! dispatches on (`init`, `message`, `tool_use`, `tool_result`,
//! `result`), reusing the synthetic session id, model name and fixed
//! timestamps documented in `tests/fixtures/README.md` — no data from a
//! real Gemini session. The goldens pin full event ordering and seq
//! numbering, the session id drained from `init`, and the two terminal
//! paths that differ from every other vendor: `result` carries its own
//! terminal state, so `finalize` must stay silent, while a
//! `RESOURCE_EXHAUSTED` result must produce a non-retryable RateLimit
//! failure *and* a drainable quota signal.
//!
//! Regenerate goldens after an *intentional* adapter change with:
//!   UPDATE_GOLDEN=1 cargo test -p vigla-adapter-gemini --test conformance

use adapter_conformance::assert_conformance;
use gemini_adapter::GeminiAdapter;

fn run_case(name: &str) {
    let mut adapter = GeminiAdapter::new("w-conf", Some("t-conf".into()));
    assert_conformance(env!("CARGO_MANIFEST_DIR"), name, &mut adapter);
}

#[test]
fn conformance_happy_path() {
    run_case("happy_path");
}

#[test]
fn conformance_quota_exhaustion() {
    run_case("quota_exhaustion");
}

#[test]
fn conformance_killed_midrun() {
    run_case("killed_midrun");
}

#[test]
fn conformance_interleaved_stderr() {
    run_case("interleaved_stderr");
}
