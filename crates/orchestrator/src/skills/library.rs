//! Skill module type + dependency-free frontmatter parser + library loader.

use event_schema::Vendor;

/// Injection scope for a skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SkillScope {
    Repo,
    Vendor(Vendor),
}

/// One curated skill: parsed frontmatter + markdown body.
#[derive(Debug, Clone)]
pub(crate) struct Skill {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) scope: SkillScope,
    pub(crate) enabled: bool,
    pub(crate) priority: i64,
    pub(crate) body: String,
}

/// Parse a `SKILL.md`: `---`-fenced single-line scalar frontmatter, then body.
/// `id` is the skill's stable key (its directory name). Returns `None` (caller
/// logs) on a missing/malformed fence or a missing required `name`.
pub(crate) fn parse_skill(id: &str, raw: &str) -> Option<Skill> {
    let after_open = raw.strip_prefix("---")?;
    let (frontmatter, body) = split_frontmatter(after_open)?;

    let mut name: Option<String> = None;
    let mut description = String::new();
    let mut scope = SkillScope::Repo;
    let mut enabled = true;
    let mut priority: i64 = 0;

    for line in frontmatter.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        let (key, val) = (key.trim(), val.trim());
        match key {
            "name" => name = Some(val.to_string()),
            "description" => description = val.to_string(),
            // Unrecognized scope (typo / not-yet-known vendor) falls back to
            // repo scope so the skill stays available rather than vanishing.
            "scope" => scope = parse_scope(val).unwrap_or(SkillScope::Repo),
            "enabled" => enabled = matches!(val, "true" | "yes" | "1"),
            "priority" => priority = val.parse().unwrap_or(0),
            _ => {} // forward-compatible: ignore unknown keys
        }
    }

    Some(Skill {
        id: id.to_string(),
        name: name?,
        description,
        scope,
        enabled,
        priority,
        body: body.trim_start_matches(['\n', '\r']).to_string(),
    })
}

/// Split text *after* the opening `---` into (frontmatter, body) at the next
/// line that is exactly `---`. `None` if there is no closing fence.
fn split_frontmatter(after_open: &str) -> Option<(&str, &str)> {
    let rest = after_open
        .strip_prefix("\r\n")
        .or_else(|| after_open.strip_prefix('\n'))
        .unwrap_or(after_open);
    let mut idx = 0usize;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\n', '\r']) == "---" {
            return Some((&rest[..idx], &rest[idx + line.len()..]));
        }
        idx += line.len();
    }
    None
}

use std::collections::BTreeMap;
use std::path::Path;

/// In-memory, shadow-resolved set of skills for one repo.
#[derive(Debug, Clone, Default)]
pub struct SkillLibrary {
    skills: Vec<Skill>,
}

impl SkillLibrary {
    /// Load bundled skills, then overlay user skills from
    /// `<repo>/.vigla/skills/<id>/SKILL.md` (a user `id` shadows the bundled
    /// one). Fail-soft: any unreadable/invalid skill is skipped with a warning;
    /// the library is always returned (at least the parseable bundled set).
    pub(crate) async fn open_for_repo(repo_root: &Path) -> SkillLibrary {
        let mut by_id: BTreeMap<String, Skill> = BTreeMap::new();

        for (id, raw) in super::bundled::bundled_skills() {
            match parse_skill(id, raw) {
                Some(s) => {
                    by_id.insert(s.id.clone(), s);
                }
                None => tracing::warn!("vigla: bundled skill '{id}' failed to parse; skipping"),
            }
        }

        let user_dir = repo_root.join(".vigla").join("skills");
        if let Ok(mut entries) = tokio::fs::read_dir(&user_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let is_dir = matches!(entry.file_type().await, Ok(t) if t.is_dir());
                if !is_dir {
                    continue;
                }
                let path = entry.path();
                let Some(id) = path.file_name().and_then(|s| s.to_str()).map(str::to_owned) else {
                    continue;
                };
                let skill_md = path.join("SKILL.md");
                match tokio::fs::read_to_string(&skill_md).await {
                    Ok(raw) => match parse_skill(&id, &raw) {
                        Some(s) => {
                            by_id.insert(s.id.clone(), s);
                        }
                        None => {
                            tracing::warn!("vigla: user skill '{id}' failed to parse; skipping")
                        }
                    },
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => tracing::warn!("vigla: user skill '{id}' unreadable: {e}"),
                }
            }
        }

        SkillLibrary {
            skills: by_id.into_values().collect(),
        }
    }

    /// Skills to inject for a worker of `vendor`: enabled, scope `repo` or the
    /// worker's vendor, ordered `priority` desc then `id` asc (deterministic).
    pub(crate) fn select_for_worker(&self, vendor: Vendor) -> Vec<Skill> {
        let mut out: Vec<Skill> = self
            .skills
            .iter()
            .filter(|s| s.enabled)
            .filter(|s| match &s.scope {
                SkillScope::Repo => true,
                SkillScope::Vendor(v) => *v == vendor,
            })
            .cloned()
            .collect();
        out.sort_by(|a, b| b.priority.cmp(&a.priority).then_with(|| a.id.cmp(&b.id)));
        out
    }

    #[cfg(test)]
    pub(crate) fn from_skills(skills: Vec<Skill>) -> Self {
        Self { skills }
    }
}

fn parse_scope(val: &str) -> Option<SkillScope> {
    if val == "repo" {
        return Some(SkillScope::Repo);
    }
    let vendor = match val.strip_prefix("vendor:")? {
        "claude" => Vendor::Claude,
        "codex" => Vendor::Codex,
        "gemini" => Vendor::Gemini,
        "antigravity" => Vendor::Antigravity,
        "kiro" => Vendor::Kiro,
        "copilot" => Vendor::Copilot,
        "opencode" => Vendor::Opencode,
        "mock" => Vendor::Mock,
        _ => return None,
    };
    Some(SkillScope::Vendor(vendor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_schema::Vendor;

    fn skill(id: &str, scope: SkillScope, enabled: bool, priority: i64) -> Skill {
        Skill {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            scope,
            enabled,
            priority,
            body: format!("body of {id}"),
        }
    }

    #[test]
    fn bundled_skills_all_parse() {
        for (id, raw) in crate::skills::bundled_skills_for_test() {
            assert!(
                parse_skill(id, raw).is_some(),
                "bundled skill {id} must parse"
            );
        }
    }

    #[test]
    fn select_filters_disabled_and_other_vendor_and_orders() {
        let lib = SkillLibrary::from_skills(vec![
            skill("a-repo", SkillScope::Repo, true, 10),
            skill("z-repo", SkillScope::Repo, true, 10),
            skill("claude-only", SkillScope::Vendor(Vendor::Claude), true, 99),
            skill("gemini-only", SkillScope::Vendor(Vendor::Gemini), true, 99),
            skill("disabled", SkillScope::Repo, false, 100),
        ]);
        let picked: Vec<String> = lib
            .select_for_worker(Vendor::Claude)
            .into_iter()
            .map(|s| s.id)
            .collect();
        // claude-only (prio 99) first; then the two repo skills tie at 10 → id asc.
        assert_eq!(picked, vec!["claude-only", "a-repo", "z-repo"]);
        // gemini-only and disabled are excluded.
        assert!(!picked.contains(&"gemini-only".to_string()));
        assert!(!picked.contains(&"disabled".to_string()));
    }

    #[tokio::test]
    async fn user_skill_shadows_bundled_by_id() {
        let dir = tempfile::TempDir::new().unwrap();
        let sd = dir.path().join(".vigla/skills/systematic-debugging");
        std::fs::create_dir_all(&sd).unwrap();
        std::fs::write(
            sd.join("SKILL.md"),
            "---\nname: Custom debugging\nscope: repo\nenabled: false\n---\nmine",
        )
        .unwrap();

        let lib = SkillLibrary::open_for_repo(dir.path()).await;
        // The bundled systematic-debugging is shadowed → now disabled → not selected.
        let has = lib
            .select_for_worker(Vendor::Claude)
            .iter()
            .any(|s| s.id == "systematic-debugging");
        assert!(
            !has,
            "user skill (enabled:false) must shadow the bundled one"
        );
        // Other bundled skills still load.
        assert!(lib
            .select_for_worker(Vendor::Claude)
            .iter()
            .any(|s| s.id == "test-first"));
    }

    #[tokio::test]
    async fn open_for_repo_without_user_dir_loads_bundled_only() {
        let dir = tempfile::TempDir::new().unwrap();
        let lib = SkillLibrary::open_for_repo(dir.path()).await;
        let ids: Vec<String> = lib
            .select_for_worker(Vendor::Claude)
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert!(ids.contains(&"systematic-debugging".to_string()));
        assert!(ids.contains(&"verify-before-done".to_string()));
    }

    const SAMPLE: &str = "---\nname: Systematic debugging\ndescription: Use when a worker hits a bug.\nscope: vendor:claude\nenabled: true\npriority: 50\n---\n# Body\nReproduce before fixing.\n";

    #[test]
    fn parses_full_frontmatter_and_body() {
        let s = parse_skill("systematic-debugging", SAMPLE).expect("parses");
        assert_eq!(s.id, "systematic-debugging");
        assert_eq!(s.name, "Systematic debugging");
        assert_eq!(s.description, "Use when a worker hits a bug.");
        assert_eq!(s.scope, SkillScope::Vendor(Vendor::Claude));
        assert!(s.enabled);
        assert_eq!(s.priority, 50);
        assert!(s.body.starts_with("# Body"));
    }

    #[test]
    fn defaults_scope_repo_enabled_true_priority_zero() {
        let raw = "---\nname: X\n---\nbody";
        let s = parse_skill("x", raw).expect("parses");
        assert_eq!(s.scope, SkillScope::Repo);
        assert!(s.enabled);
        assert_eq!(s.priority, 0);
        assert_eq!(s.description, "");
    }

    #[test]
    fn missing_name_is_rejected() {
        let raw = "---\ndescription: no name here\n---\nbody";
        assert!(parse_skill("x", raw).is_none());
    }

    #[test]
    fn missing_frontmatter_fence_is_rejected() {
        assert!(parse_skill("x", "no frontmatter at all").is_none());
    }

    #[test]
    fn unknown_keys_are_ignored_and_enabled_false_parsed() {
        let raw = "---\nname: X\nfuture_key: whatever\nenabled: false\n---\nb";
        let s = parse_skill("x", raw).expect("parses");
        assert!(!s.enabled);
    }

    #[test]
    fn description_may_contain_colons() {
        let raw = "---\nname: X\ndescription: Use when: a thing happens\n---\nb";
        let s = parse_skill("x", raw).unwrap();
        assert_eq!(s.description, "Use when: a thing happens");
    }

    #[test]
    fn unrecognized_scope_value_falls_back_to_repo() {
        let s = parse_skill("x", "---\nname: X\nscope: vendor:bogus\n---\nbody").expect("parses");
        assert_eq!(s.scope, SkillScope::Repo);
        assert_eq!(s.name, "X");
    }

    #[test]
    fn frontmatter_without_trailing_newline_parses() {
        let s = parse_skill("x", "---\nname: X\n---").expect("parses");
        assert_eq!(s.name, "X");
        assert_eq!(s.body, "");
    }
}
