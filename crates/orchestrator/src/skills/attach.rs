//! Fail-soft bridge: render selected skills into the worker's native file as a
//! second anchor region. Reuses the memory subsystem's pure, parameterized
//! anchor writer; touches no memory state.

use std::path::Path;

use event_schema::Vendor;

use super::library::SkillLibrary;
use super::render::{self, SKILLS_ANCHOR_CLOSE, SKILLS_ANCHOR_OPEN, SKILLS_TOKEN_BUDGET};

/// What a successful attach wrote — surfaced as telemetry by the caller.
#[derive(Debug, Clone)]
pub(crate) struct SkillsAttachOutcome {
    pub(crate) injected_ids: Vec<String>,
    pub(crate) dropped_ids: Vec<String>,
    pub(crate) tokens: u32,
}

/// Select + render + write the `vigla:skills` region into `worktree`'s native
/// file. Fail-soft: empty selection ⇒ `None` (no write); any write error ⇒
/// logged + `None`. Never blocks dispatch.
///
pub(crate) async fn attach_skills_to_worktree(
    library: &SkillLibrary,
    vendor: Vendor,
    worktree: &Path,
) -> Option<SkillsAttachOutcome> {
    let selected = library.select_for_worker(vendor);
    let rendered = render::render_skills(&selected, SKILLS_TOKEN_BUDGET)?;
    let native = worktree.join(render::native_file_name(vendor));

    match crate::memory::coherence::write_anchor_block(
        &native,
        SKILLS_ANCHOR_OPEN,
        SKILLS_ANCHOR_CLOSE,
        &rendered.body,
    )
    .await
    {
        Ok(_) => Some(SkillsAttachOutcome {
            injected_ids: rendered.injected_ids,
            dropped_ids: rendered.dropped_ids,
            tokens: rendered.tokens,
        }),
        Err(e) => {
            tracing::warn!(
                "vigla: skills attach skipped — write failed for {}: {e}",
                native.display()
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::library::{Skill, SkillLibrary, SkillScope};
    use event_schema::Vendor;

    fn lib(skills: Vec<Skill>) -> SkillLibrary {
        SkillLibrary::from_skills(skills)
    }
    fn s(id: &str) -> Skill {
        Skill {
            id: id.into(),
            name: id.into(),
            description: "d".into(),
            scope: SkillScope::Repo,
            enabled: true,
            priority: 0,
            body: format!("{id} body"),
        }
    }

    #[tokio::test]
    async fn writes_skills_region_into_claude_md() {
        let wt = tempfile::TempDir::new().unwrap();
        let out = attach_skills_to_worktree(&lib(vec![s("alpha")]), Vendor::Claude, wt.path())
            .await
            .expect("attach writes a region");
        assert_eq!(out.injected_ids, vec!["alpha"]);
        let content = std::fs::read_to_string(wt.path().join("CLAUDE.md")).unwrap();
        assert!(content.contains(super::super::render::SKILLS_ANCHOR_OPEN));
        assert!(content.contains("### alpha"));
    }

    #[tokio::test]
    async fn empty_library_is_noop_and_writes_nothing() {
        let wt = tempfile::TempDir::new().unwrap();
        let out = attach_skills_to_worktree(&lib(vec![]), Vendor::Claude, wt.path()).await;
        assert!(out.is_none());
        assert!(!wt.path().join("CLAUDE.md").exists());
    }

    #[tokio::test]
    async fn over_budget_attach_truncates_and_reports_dropped() {
        let wt = tempfile::TempDir::new().unwrap();
        // estimate_tokens = chars/4 + 10; each section also has ~40 chars of
        // markdown framing. 8000-char body → ~2010 tokens per skill; 3 skills
        // overflow the 4000-token budget.
        let big = "x".repeat(8000);
        let lib = lib(vec![
            {
                let mut k = s("a");
                k.body = big.clone();
                k
            },
            {
                let mut k = s("b");
                k.body = big.clone();
                k
            },
            {
                let mut k = s("c");
                k.body = big.clone();
                k
            },
        ]);
        let out = attach_skills_to_worktree(&lib, Vendor::Claude, wt.path())
            .await
            .expect("attach writes a region");
        assert!(!out.injected_ids.is_empty(), "at least one skill injected");
        assert!(
            !out.dropped_ids.is_empty(),
            "budget overflow must report dropped ids"
        );
        let content = std::fs::read_to_string(wt.path().join("CLAUDE.md")).unwrap();
        assert!(content.contains(super::super::render::SKILLS_ANCHOR_OPEN));
    }

    /// Coexistence: a pre-existing memory region + user content must survive a
    /// skills attach untouched; both anchored regions end up in the file.
    #[tokio::test]
    async fn coexists_with_memory_region_and_preserves_user_content() {
        let wt = tempfile::TempDir::new().unwrap();
        let claude_md = wt.path().join("CLAUDE.md");
        // Simulate memory attach writing its own region first.
        crate::memory::coherence::write_anchor_block(
            &claude_md,
            "<!-- vigla:memory:begin v1 -->",
            "<!-- vigla:memory:end -->",
            "remembered: always run tests",
        )
        .await
        .unwrap();
        // Plus some user content.
        let with_user = format!(
            "# My project notes\n\n{}",
            std::fs::read_to_string(&claude_md).unwrap()
        );
        std::fs::write(&claude_md, with_user).unwrap();

        attach_skills_to_worktree(&lib(vec![s("alpha")]), Vendor::Claude, wt.path())
            .await
            .expect("skills attach");

        let content = std::fs::read_to_string(&claude_md).unwrap();
        assert!(
            content.contains("# My project notes"),
            "user content preserved"
        );
        assert!(
            content.contains("remembered: always run tests"),
            "memory region preserved"
        );
        assert!(content.contains("<!-- vigla:memory:begin v1 -->"));
        assert!(content.contains(super::super::render::SKILLS_ANCHOR_OPEN));
        assert!(content.contains("### alpha"));
    }
}
