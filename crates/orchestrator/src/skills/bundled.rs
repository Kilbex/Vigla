//! Compile-time curated skill set. Bundled into the binary the same way
//! vendor profiles are (`include_str!` from a repo-top directory). Each entry
//! is `(id, raw SKILL.md contents)`; `id` is the skill's directory name.

macro_rules! seed {
    ($id:literal) => {
        (
            $id,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/resources/skills/",
                $id,
                "/SKILL.md"
            )),
        )
    };
}

pub(crate) fn bundled_skills() -> &'static [(&'static str, &'static str)] {
    &[
        seed!("systematic-debugging"),
        seed!("test-first"),
        seed!("verify-before-done"),
    ]
}
