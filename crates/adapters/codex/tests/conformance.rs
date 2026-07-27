//! Golden-transcript conformance suite for [`CodexAdapter`].
//!
//! Every transcript line is a shape `CodexAdapter::ingest_line` already
//! dispatches on (`thread.started`, `turn.started`, `item.started`,
//! `item.completed`, `turn.completed`) with the same synthetic thread
//! and item ids as `tests/fixtures/codex_simple.jsonl` — no session,
//! account, or path data from a real run. The goldens pin the parts
//! `from_fixture.rs` only spot-checks: full event ordering and seq
//! numbering, the session id drained from `thread.started`, the quota
//! signal drained from a `usage_limit_exceeded` turn, and the terminal
//! event Codex only ever gets from `finalize` (it emits no "done" line).
//!
//! Regenerate goldens after an *intentional* adapter change with:
//!   UPDATE_GOLDEN=1 cargo test -p vigla-adapter-codex --test conformance

use adapter_conformance::assert_conformance;
use codex_adapter::CodexAdapter;

fn run_case(name: &str) {
    let mut adapter = CodexAdapter::new("w-conf", Some("t-conf".into()));
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
