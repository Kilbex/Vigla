//! Pure, deterministic rendering of selected skills into the `vigla:skills`
//! anchor-block body. No I/O. Token estimation is local (chars/4) — no
//! dependency on the memory subsystem.

use event_schema::Vendor;

use super::library::Skill;

pub(crate) const SKILLS_ANCHOR_OPEN: &str = "<!-- vigla:skills:begin v1 -->";
pub(crate) const SKILLS_ANCHOR_CLOSE: &str = "<!-- vigla:skills:end -->";
pub(crate) const SKILLS_TOKEN_BUDGET: usize = 4000;

/// Worker native file the vendor CLI auto-loads. Mirrors the memory adapters'
/// file names; unknown/auto vendors fall back to `CLAUDE.md`.
pub(crate) fn native_file_name(vendor: Vendor) -> &'static str {
    match vendor {
        Vendor::Codex => "AGENTS.md",
        Vendor::Gemini => "GEMINI.md",
        _ => "CLAUDE.md",
    }
}

fn estimate_tokens(text: &str) -> usize {
    text.chars().count() / 4 + 10
}

#[derive(Debug, Clone)]
pub(crate) struct RenderedSkills {
    pub(crate) body: String,
    pub(crate) injected_ids: Vec<String>,
    pub(crate) dropped_ids: Vec<String>,
    pub(crate) tokens: u32,
}

/// Render `skills` (already selected + ordered) into the anchor-block body.
/// Honors `budget` by tail-drop: at least the first skill is always included.
/// Returns `None` when `skills` is empty (caller treats as a no-op).
pub(crate) fn render_skills(skills: &[Skill], budget: usize) -> Option<RenderedSkills> {
    if skills.is_empty() {
        return None;
    }
    let header = "## Vigla Skills\n";
    let mut body = String::from(header);
    let mut injected_ids = Vec::new();
    let mut dropped_ids = Vec::new();
    let mut used = estimate_tokens(header);

    for (i, sk) in skills.iter().enumerate() {
        // Guard: skip any skill whose text contains an anchor delimiter to
        // prevent corrupting the worktree file on a naive substring re-parse.
        let contains_delimiter = sk.name.contains(SKILLS_ANCHOR_OPEN)
            || sk.name.contains(SKILLS_ANCHOR_CLOSE)
            || sk.description.contains(SKILLS_ANCHOR_OPEN)
            || sk.description.contains(SKILLS_ANCHOR_CLOSE)
            || sk.body.contains(SKILLS_ANCHOR_OPEN)
            || sk.body.contains(SKILLS_ANCHOR_CLOSE);
        if contains_delimiter {
            tracing::warn!(
                "vigla: skill '{}' contains a skills anchor delimiter; \
                 skipping to avoid corrupting the worktree file",
                sk.id
            );
            continue;
        }

        let section = format!(
            "\n### {}\n_{}_\n\n{}\n",
            sk.name,
            sk.description,
            sk.body.trim()
        );
        let cost = estimate_tokens(&section);
        if !injected_ids.is_empty() && used + cost > budget {
            dropped_ids.extend(skills[i..].iter().map(|s| s.id.clone()));
            break;
        }
        body.push_str(&section);
        injected_ids.push(sk.id.clone());
        used += cost;
    }

    Some(RenderedSkills {
        body,
        injected_ids,
        dropped_ids,
        tokens: used as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::library::{Skill, SkillScope};
    use event_schema::Vendor;

    fn s(id: &str, body: &str) -> Skill {
        Skill {
            id: id.into(),
            name: id.into(),
            description: "d".into(),
            scope: SkillScope::Repo,
            enabled: true,
            priority: 0,
            body: body.into(),
        }
    }

    #[test]
    fn native_file_per_vendor() {
        assert_eq!(native_file_name(Vendor::Claude), "CLAUDE.md");
        assert_eq!(native_file_name(Vendor::Codex), "AGENTS.md");
        assert_eq!(native_file_name(Vendor::Gemini), "GEMINI.md");
        assert_eq!(native_file_name(Vendor::Mock), "CLAUDE.md");
    }

    #[test]
    fn empty_selection_is_none() {
        assert!(render_skills(&[], SKILLS_TOKEN_BUDGET).is_none());
    }

    #[test]
    fn deterministic_same_input_same_bytes() {
        let skills = vec![s("a", "alpha"), s("b", "beta")];
        let one = render_skills(&skills, SKILLS_TOKEN_BUDGET).unwrap();
        let two = render_skills(&skills, SKILLS_TOKEN_BUDGET).unwrap();
        assert_eq!(one.body, two.body);
        assert_eq!(one.injected_ids, vec!["a", "b"]);
        assert!(one.body.contains("## Vigla Skills"));
    }

    #[test]
    fn budget_drops_the_tail() {
        let big = "x".repeat(4000);
        let skills = vec![s("first", &big), s("second", &big), s("third", &big)];
        let r = render_skills(&skills, 1100).unwrap(); // ~1000 tokens per section
        assert_eq!(r.injected_ids, vec!["first"]);
        assert_eq!(r.dropped_ids, vec!["second", "third"]);
        assert!(!r.body.contains("### second"));
    }

    #[test]
    fn single_over_budget_always_included() {
        let r = render_skills(&[s("only", &"x".repeat(8000))], 100).unwrap();
        assert_eq!(r.injected_ids, vec!["only"]);
        assert!(r.dropped_ids.is_empty());
    }

    #[test]
    fn skill_containing_anchor_delimiter_is_skipped() {
        let mut bad = s("evil", &format!("before {} after", SKILLS_ANCHOR_CLOSE));
        bad.enabled = true;
        let good = s("ok", "safe body");
        let r = render_skills(&[bad, good], SKILLS_TOKEN_BUDGET).unwrap();
        assert_eq!(r.injected_ids, vec!["ok"]);
        assert!(!r.dropped_ids.contains(&"evil".to_string()));
        assert!(!r.body.contains(SKILLS_ANCHOR_CLOSE));
        assert!(r.body.contains("safe body"));
    }
}
