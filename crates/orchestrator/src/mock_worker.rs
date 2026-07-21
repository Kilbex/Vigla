//! Mock worker variants.
//!
//! Three deterministic submission shapes assigned by task index in
//! `run_mock_mission` and the supervisor-driven path (U3.4). Each
//! variant produces a distinct first-pass submission so a real
//! supervisor reading the playbook can make varied accept/revise/reject
//! calls without us scripting the decision side. After a revision
//! directive, the variant's `next_pass` returns the same kind with
//! incremented `pass`; `run_pass` returns cleaner content on pass >= 1.
//!
//! Variant 0 (`Happy`, `pass = 0`) reproduces today's mock output byte
//! for byte so existing `MissionRuntime` tests stay green when this
//! module replaces the inline write in `run_mock_mission`.

use crate::mission_event::TaskDescriptor;

/// Which behavioral shape a mock worker exhibits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MockWorkerKind {
    /// Submits complete work on the first pass. Supervisor playbook
    /// reads it as "ready to integrate."
    Happy,
    /// First pass submits an explicit draft (`// TODO: implement` style)
    /// and asks for review. Playbook should request a revise.
    NeedsRevision,
    /// First pass submits content that visibly doesn't match the task
    /// (placeholder text, wrong file body). Playbook should request a
    /// revise (or, with a strict playbook, reject — both are
    /// acceptable supervisor decisions).
    BadThenRevise,
}

impl MockWorkerKind {
    /// Deterministic assignment by task index: 0 → Happy, 1 →
    /// NeedsRevision, 2 → BadThenRevise. Cycles beyond 3.
    pub fn for_task_index(task_index: u32) -> Self {
        match task_index % 3 {
            0 => Self::Happy,
            1 => Self::NeedsRevision,
            _ => Self::BadThenRevise,
        }
    }
}

/// A mock worker variant — `kind` plus the revision pass counter.
/// `pass = 0` is the first submission; `pass >= 1` is a revision pass
/// after the supervisor returned `decision: revise`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MockWorkerVariant {
    pub kind: MockWorkerKind,
    pub pass: u32,
}

impl MockWorkerVariant {
    pub fn new(kind: MockWorkerKind) -> Self {
        Self { kind, pass: 0 }
    }

    /// Pick the variant for the given task index, starting at pass 0.
    pub fn for_task_index(task_index: u32) -> Self {
        Self::new(MockWorkerKind::for_task_index(task_index))
    }

    /// The variant to use on the next pass after the supervisor asked
    /// for a revision. Same kind, `pass + 1`.
    pub fn next_pass(self) -> Self {
        Self {
            kind: self.kind,
            pass: self.pass + 1,
        }
    }
}

/// What a worker should write + how it describes its submission.
/// Returned by [`MockWorkerVariant::run_pass`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockWorkerPass {
    pub file_name: String,
    pub file_content: String,
    pub commit_message: String,
    pub submission_summary: String,
}

impl MockWorkerVariant {
    /// Materialize the variant + pass into concrete content for a task.
    /// Pure: no I/O, no random.
    pub fn run_pass(&self, task: &TaskDescriptor) -> MockWorkerPass {
        let file_name = format!("MOCK_{}.md", task.index);
        match (self.kind, self.pass) {
            // Happy first pass MUST match the pre-U3.1 inline mock
            // byte-for-byte (content, commit msg, submission summary)
            // so existing mission_runtime tests don't shift on this
            // refactor. On revision passes (pass >= 1) we *must* alter
            // file content — git refuses an empty commit, and the
            // supervisor flow re-runs `git commit` on each pass.
            (MockWorkerKind::Happy, 0) => {
                let content = format!("# {}\n\n(mock content)\n", task.title);
                let msg = format!("mock work for {}", task.title);
                MockWorkerPass {
                    file_name,
                    file_content: content,
                    commit_message: msg.clone(),
                    submission_summary: msg,
                }
            }
            (MockWorkerKind::Happy, pass) => MockWorkerPass {
                file_name,
                file_content: format!(
                    "# {}\n\n(mock content)\n\n(revised at pass {})\n",
                    task.title, pass
                ),
                commit_message: format!("revise pass {}: {}", pass, task.title),
                submission_summary: format!(
                    "revised submission for {} (pass {})",
                    task.title, pass
                ),
            },

            (MockWorkerKind::NeedsRevision, 0) => {
                let content = format!("# {}\n\n<!-- TODO: implement -->\n", task.title);
                let msg = format!("draft: {}", task.title);
                MockWorkerPass {
                    file_name,
                    file_content: content,
                    commit_message: msg,
                    submission_summary: format!(
                        "draft submitted for review: {} (contains a TODO marker)",
                        task.title
                    ),
                }
            }
            (MockWorkerKind::NeedsRevision, pass) => clean_pass(task, file_name, pass),

            (MockWorkerKind::BadThenRevise, 0) => {
                let content = "# Placeholder\n\nplaceholder text\n".to_string();
                let msg = format!("first cut at {}", task.title);
                MockWorkerPass {
                    file_name,
                    file_content: content,
                    commit_message: msg,
                    submission_summary: format!(
                        "first cut at {} (placeholder text only)",
                        task.title
                    ),
                }
            }
            (MockWorkerKind::BadThenRevise, pass) => clean_pass(task, file_name, pass),
        }
    }
}

fn clean_pass(task: &TaskDescriptor, file_name: String, pass: u32) -> MockWorkerPass {
    // Pass number embedded in content + summary so consecutive revision
    // passes produce non-identical commits — required because git
    // refuses an empty commit and the supervisor flow re-runs
    // `git commit` on every revision pass.
    let content = format!(
        "# {}\n\n{}\n\nImplemented per supervisor directive (pass {}).\n",
        task.title, task.title, pass
    );
    let msg = format!("revise pass {}: {}", pass, task.title);
    MockWorkerPass {
        file_name,
        file_content: content,
        commit_message: msg,
        submission_summary: format!("revised submission for {} (pass {})", task.title, pass),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(index: u32, title: &str) -> TaskDescriptor {
        TaskDescriptor {
            index,
            title: title.into(),
            ..Default::default()
        }
    }

    #[test]
    fn for_task_index_cycles_through_three_kinds() {
        assert_eq!(MockWorkerKind::for_task_index(0), MockWorkerKind::Happy);
        assert_eq!(
            MockWorkerKind::for_task_index(1),
            MockWorkerKind::NeedsRevision
        );
        assert_eq!(
            MockWorkerKind::for_task_index(2),
            MockWorkerKind::BadThenRevise
        );
        assert_eq!(MockWorkerKind::for_task_index(3), MockWorkerKind::Happy);
        assert_eq!(
            MockWorkerKind::for_task_index(7),
            MockWorkerKind::NeedsRevision
        );
    }

    #[test]
    fn happy_first_pass_reproduces_legacy_mock_output() {
        // Locking the content shape against the pre-U3.1 inline mock
        // so MissionRuntime tests don't drift on the U3.1 refactor.
        let v = MockWorkerVariant::for_task_index(0);
        let pass = v.run_pass(&task(0, "Plan integration"));
        assert_eq!(pass.file_name, "MOCK_0.md");
        assert_eq!(pass.file_content, "# Plan integration\n\n(mock content)\n");
        assert_eq!(pass.commit_message, "mock work for Plan integration");
        assert_eq!(pass.submission_summary, "mock work for Plan integration");
    }

    #[test]
    fn needs_revision_first_pass_marks_a_draft() {
        let v = MockWorkerVariant::for_task_index(1);
        let pass = v.run_pass(&task(1, "Implement changes"));
        assert!(
            pass.file_content.contains("TODO: implement"),
            "draft must carry a TODO marker: {}",
            pass.file_content
        );
        assert!(pass.commit_message.starts_with("draft:"));
        assert!(pass.submission_summary.contains("draft"));
    }

    #[test]
    fn bad_then_revise_first_pass_is_placeholder_only() {
        let v = MockWorkerVariant::for_task_index(2);
        let pass = v.run_pass(&task(2, "Update documentation"));
        assert_eq!(
            pass.file_content, "# Placeholder\n\nplaceholder text\n",
            "first pass should be obviously off-task"
        );
        assert!(pass.submission_summary.contains("placeholder"));
    }

    #[test]
    fn revise_pass_returns_clean_content_for_needs_revision() {
        let mut v = MockWorkerVariant::for_task_index(1);
        v = v.next_pass();
        let pass = v.run_pass(&task(1, "Implement changes"));
        assert!(pass
            .file_content
            .contains("Implemented per supervisor directive"));
        assert!(!pass.file_content.contains("TODO"));
        assert!(pass.commit_message.starts_with("revise pass"));
    }

    #[test]
    fn revise_pass_returns_clean_content_for_bad_then_revise() {
        let mut v = MockWorkerVariant::for_task_index(2);
        v = v.next_pass();
        let pass = v.run_pass(&task(2, "Update documentation"));
        assert!(pass.file_content.contains("Update documentation"));
        assert!(!pass.file_content.contains("placeholder"));
        assert!(pass.commit_message.starts_with("revise pass"));
    }

    #[test]
    fn revision_passes_produce_distinct_content_across_all_kinds() {
        // Every variant must produce *different* content on consecutive
        // revision passes — git refuses an empty commit and the
        // supervisor flow re-runs `git commit` on each pass. A regression
        // here turns a single supervisor `revise` into a worker-side
        // crash.
        for kind in [
            MockWorkerKind::Happy,
            MockWorkerKind::NeedsRevision,
            MockWorkerKind::BadThenRevise,
        ] {
            let mut variant = MockWorkerVariant::new(kind);
            let t = task(0, "Some task");
            let p0 = variant.run_pass(&t);
            variant = variant.next_pass();
            let p1 = variant.run_pass(&t);
            variant = variant.next_pass();
            let p2 = variant.run_pass(&t);
            assert_ne!(
                p0.file_content, p1.file_content,
                "{:?} pass 0 → 1 must change content",
                kind
            );
            assert_ne!(
                p1.file_content, p2.file_content,
                "{:?} pass 1 → 2 must change content",
                kind
            );
        }
    }

    #[test]
    fn next_pass_increments_pass_counter() {
        let v = MockWorkerVariant::for_task_index(1);
        assert_eq!(v.pass, 0);
        assert_eq!(v.next_pass().pass, 1);
        assert_eq!(v.next_pass().next_pass().pass, 2);
    }
}
