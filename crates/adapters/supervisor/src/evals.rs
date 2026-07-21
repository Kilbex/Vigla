//! Deterministic supervisor eval suite v1.
//!
//! These are not live model benchmarks. They are cheap, reproducible
//! contract checks for the four quality axes Phase 2 cares about:
//! decomposition, assignment, synthesis, and escalation. Live Claude
//! sign-off remains in `orchestrator/tests/supervisor_live.rs`; this
//! suite makes the playbook/intent contract measurable in normal CI.

use crate::{extract_intent, ReviewDecisionTag, SupervisorIntent};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvalAxis {
    Decomposition,
    Assignment,
    Synthesis,
    Escalation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalCase {
    pub name: &'static str,
    pub axis: EvalAxis,
    pub transcript: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalOutcome {
    pub name: &'static str,
    pub axis: EvalAxis,
    pub passed: bool,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalReport {
    pub outcomes: Vec<EvalOutcome>,
}

impl EvalReport {
    pub fn passed(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.passed)
            .count()
    }

    pub fn total(&self) -> usize {
        self.outcomes.len()
    }

    pub fn failures(&self) -> Vec<&EvalOutcome> {
        self.outcomes
            .iter()
            .filter(|outcome| !outcome.passed)
            .collect()
    }
}

pub fn supervisor_eval_suite_v1() -> Vec<EvalCase> {
    vec![
        EvalCase {
            name: "decomposition references concrete files and stays bounded",
            axis: EvalAxis::Decomposition,
            transcript: "The work separates cleanly into implementation and tests.\n\n```json\n{\"action\":\"decompose\",\"tasks\":[{\"title\":\"Update orchestrator/src/mission.rs validation\",\"description\":\"Tighten MissionSpec validation for empty objectives.\"},{\"title\":\"Add orchestrator/src/mission.rs invalid-spec tests\",\"description\":\"Cover the new validation path in orchestrator unit tests.\"}]}\n```",
        },
        EvalCase {
            name: "assignment selects a valid task index",
            axis: EvalAxis::Assignment,
            transcript: "The next task is ready for a worker.\n\n```json\n{\"action\":\"spawn_worker\",\"task_index\":1}\n```",
        },
        EvalCase {
            name: "synthesis is short and integration-oriented",
            axis: EvalAxis::Synthesis,
            transcript: "Everything needed for the review screen is ready.\n\n```json\n{\"action\":\"declare_complete\",\"summary\":\"Integrated validation and tests for the mission spec path. The branch is ready for user review.\"}\n```",
        },
        EvalCase {
            name: "escalation requests a targeted revision for TODO work",
            axis: EvalAxis::Escalation,
            transcript: "The submission is pointed at the right file but still contains a TODO marker, so it needs one revision.\n\n```json\n{\"action\":\"review\",\"worker_id\":\"mock-2\",\"decision\":\"revise\",\"directive\":\"Replace the TODO marker with real validation logic and update the affected test.\"}\n```",
        },
    ]
}

pub fn run_supervisor_eval_suite_v1() -> EvalReport {
    let outcomes = supervisor_eval_suite_v1()
        .into_iter()
        .map(run_case)
        .collect();
    EvalReport { outcomes }
}

fn run_case(case: EvalCase) -> EvalOutcome {
    match extract_intent(case.transcript) {
        Ok(intent) => score_intent(&case, intent),
        Err(err) => EvalOutcome {
            name: case.name,
            axis: case.axis,
            passed: false,
            note: format!("intent parse failed: {err}"),
        },
    }
}

fn score_intent(case: &EvalCase, intent: SupervisorIntent) -> EvalOutcome {
    let (passed, note) = match (case.axis, intent) {
        (EvalAxis::Decomposition, SupervisorIntent::Decompose { tasks, .. }) => {
            let bounded = (1..=6).contains(&tasks.len());
            let concrete = tasks.iter().all(|task| {
                task.title.contains('/')
                    || task.description.as_deref().is_some_and(|d| d.contains('/'))
            });
            (
                bounded && concrete,
                format!("tasks={}, concrete={concrete}", tasks.len()),
            )
        }
        (EvalAxis::Assignment, SupervisorIntent::SpawnWorker { task_index }) => {
            (task_index < 2, format!("task_index={task_index}"))
        }
        (EvalAxis::Synthesis, SupervisorIntent::DeclareComplete { summary }) => {
            let sentence_count = summary
                .split(['.', '!', '?'])
                .filter(|s| !s.trim().is_empty())
                .count();
            let integration_oriented =
                summary.contains("Integrated") || summary.contains("integrated");
            (
                sentence_count <= 2 && integration_oriented,
                format!("sentences={sentence_count}, integration_oriented={integration_oriented}"),
            )
        }
        (EvalAxis::Escalation, SupervisorIntent::Review(review)) => {
            let targeted = review
                .directive
                .as_deref()
                .is_some_and(|d| d.contains("TODO") && d.contains("test"));
            (
                review.decision == ReviewDecisionTag::Revise && targeted,
                format!("decision={:?}, targeted={targeted}", review.decision),
            )
        }
        (axis, other) => (false, format!("wrong intent for {axis:?}: {other:?}")),
    };

    EvalOutcome {
        name: case.name,
        axis: case.axis,
        passed,
        note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn supervisor_eval_suite_v1_covers_all_phase2_axes() {
        let axes: HashSet<_> = supervisor_eval_suite_v1()
            .iter()
            .map(|case| case.axis)
            .collect();
        assert!(axes.contains(&EvalAxis::Decomposition));
        assert!(axes.contains(&EvalAxis::Assignment));
        assert!(axes.contains(&EvalAxis::Synthesis));
        assert!(axes.contains(&EvalAxis::Escalation));
    }

    #[test]
    fn supervisor_eval_suite_v1_passes() {
        let report = run_supervisor_eval_suite_v1();
        assert_eq!(
            report.passed(),
            report.total(),
            "eval failures: {:?}",
            report.failures()
        );
    }
}
