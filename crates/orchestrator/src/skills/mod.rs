//! Curated skill-module injection (MVP). See plan 2026-06-24-vigla-skills.
mod attach;
mod bundled;
mod library;
mod render;
pub(crate) use render::{SKILLS_ANCHOR_CLOSE, SKILLS_ANCHOR_OPEN};

pub(crate) use attach::attach_skills_to_worktree;
pub use library::SkillLibrary;

#[cfg(test)]
pub(crate) fn bundled_skills_for_test() -> &'static [(&'static str, &'static str)] {
    bundled::bundled_skills()
}
