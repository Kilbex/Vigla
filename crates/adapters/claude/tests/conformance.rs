//! Golden-transcript conformance suite for ClaudeAdapter (WS-A).
//!
//! Each test runs a recorded transcript through a fresh adapter and
//! compares the full snapshot (events + drained signals) to a committed
//! golden. Regenerate goldens after an *intentional* adapter change with:
//!   UPDATE_GOLDEN=1 cargo test -p vigla-adapter-claude --test conformance

use adapter_conformance::assert_conformance;
use claude_adapter::ClaudeAdapter;

fn run_case(name: &str) {
    let mut adapter = ClaudeAdapter::new("w-conf", Some("t-conf".into()));
    assert_conformance(env!("CARGO_MANIFEST_DIR"), name, &mut adapter);
}

#[test]
fn conformance_happy_path() {
    run_case("happy_path");
}

#[test]
fn conformance_result_error() {
    run_case("result_error");
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

#[test]
fn conformance_truncated_no_result() {
    run_case("truncated_no_result");
}
