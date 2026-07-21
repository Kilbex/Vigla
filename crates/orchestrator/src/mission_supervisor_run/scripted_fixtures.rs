//! QC-3 scripted-supervisor builders.
//!
//! Each function returns a complete `Vec<Vec<SupervisorOutput>>`
//! (turns × outputs) suitable for
//! `ScriptedSupervisor::new(turns)`. Together they cover the four
//! Mission Pre-Planning scenarios driven by the envelope-fit gate
//! in `mission_loop`:
//!
//! - `plan_happy_envelope_within` — all four bounds Within. In
//!   Direct mode the loop auto-proceeds; in Review mode the user
//!   sees the standard plan-approval card.
//! - `plan_exceeds_risk` — `risk.fit == Exceeds`. The loop forces
//!   `PendingPlanApproval` even when `confirm_plan == None`.
//! - `plan_regenerate_then_clean` — first decompose returns
//!   `quality.fit == NearLimit` and a weak task list; the next
//!   turn returns a clean envelope. Drives the regenerate path.
//! - `plan_no_envelope` — legacy-shape decompose (`envelope_fit ==
//!   None`). Back-compat check: the gate collapses to QC-2
//!   semantics — only `confirm_plan == Some(true)` pauses.

use supervisor_adapter::{
    BoundFit, BoundFitKind, EnvelopeFit, SupervisorIntent, SupervisorOutput,
    SupervisorTaskDescriptor, TechChoice,
};

pub(crate) fn bf(fit: BoundFitKind, note: &str) -> BoundFit {
    BoundFit {
        fit,
        note: note.into(),
    }
}

pub(crate) fn all_within_envelope() -> EnvelopeFit {
    EnvelopeFit {
        scope: bf(BoundFitKind::Within, "all under src/auth/"),
        reversibility: bf(BoundFitKind::Within, "no schema changes"),
        risk: bf(BoundFitKind::Within, "no secrets"),
        quality: bf(BoundFitKind::Within, "tests included"),
    }
}

fn simple_task(idx: u32, title: &str) -> SupervisorTaskDescriptor {
    SupervisorTaskDescriptor {
        title: title.into(),
        description: Some(format!("(scripted fixture task #{idx})")),
        depends_on: vec![],
        scope_paths: vec![],
    }
}

pub(crate) fn plan_happy_envelope_within() -> Vec<Vec<SupervisorOutput>> {
    vec![vec![SupervisorOutput::Intent(
        SupervisorIntent::Decompose {
            tasks: vec![simple_task(0, "Implement handler")],
            overview: Some("Add OAuth callback handler.".into()),
            tech_stack: Some(vec![TechChoice {
                layer: "auth_provider".into(),
                choice: "Auth0".into(),
                rationale: "matches existing".into(),
                is_new: false,
            }]),
            envelope_fit: Some(all_within_envelope()),
        },
    )]]
}

pub(crate) fn plan_exceeds_risk() -> Vec<Vec<SupervisorOutput>> {
    let mut ef = all_within_envelope();
    ef.risk = bf(BoundFitKind::Exceeds, "touches billing endpoint");
    vec![vec![SupervisorOutput::Intent(
        SupervisorIntent::Decompose {
            tasks: vec![simple_task(0, "Wire billing webhook")],
            overview: Some("Add a webhook that updates the billing record.".into()),
            tech_stack: None,
            envelope_fit: Some(ef),
        },
    )]]
}

pub(crate) fn plan_regenerate_then_clean() -> Vec<Vec<SupervisorOutput>> {
    let mut weak_ef = all_within_envelope();
    weak_ef.quality = bf(BoundFitKind::NearLimit, "no test task");
    let weak = SupervisorOutput::Intent(SupervisorIntent::Decompose {
        tasks: vec![simple_task(0, "Quick patch")],
        overview: Some("Patch the issue.".into()),
        tech_stack: None,
        envelope_fit: Some(weak_ef),
    });
    let clean = SupervisorOutput::Intent(SupervisorIntent::Decompose {
        tasks: vec![
            simple_task(0, "Add failing test"),
            simple_task(1, "Implement fix"),
        ],
        overview: Some("Reproduce the bug with a failing test, then fix it.".into()),
        tech_stack: None,
        envelope_fit: Some(all_within_envelope()),
    });
    vec![vec![weak], vec![clean]]
}

pub(crate) fn plan_no_envelope() -> Vec<Vec<SupervisorOutput>> {
    vec![vec![SupervisorOutput::Intent(
        SupervisorIntent::Decompose {
            tasks: vec![simple_task(0, "Legacy task")],
            overview: None,
            tech_stack: None,
            envelope_fit: None,
        },
    )]]
}
