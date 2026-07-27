//! Golden-transcript conformance suite for [`CopilotAdapter`].
//!
//! The Copilot vendor profile asks the CLI for JSON output, but this
//! adapter deliberately does not parse it: every non-empty line becomes
//! a `Log` and the whole lifecycle comes from the opening pair plus the
//! child's exit status. The transcript text below is therefore
//! synthetic — it is not a recording of a real CLI session and does not
//! claim to pin a Copilot output schema. The JSON-looking line in
//! `happy_path` is there for the opposite reason: to pin that a
//! structured line is passed through verbatim as one log rather than
//! being decoded into events. The rest of the goldens pin the
//! event-schema §5 `idle`→`executing` opening pair, the shared quota
//! detector firing on a rate-limit line, and the terminal event
//! synthesised for each `AdapterExit`.
//!
//! Regenerate goldens after an *intentional* adapter change with:
//!   UPDATE_GOLDEN=1 cargo test -p vigla-adapter-copilot --test conformance

use adapter_conformance::assert_conformance;
use copilot_adapter::CopilotAdapter;

fn run_case(name: &str) {
    let mut adapter = CopilotAdapter::new("w-conf", Some("t-conf".into()));
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

/// A worker that exits without printing anything still has to open its
/// stream on `idle` before the terminal event, or the orchestrator sees
/// a stream whose first `state_change` has no `from` and is terminal.
#[test]
fn conformance_silent_worker() {
    run_case("silent_worker");
}
