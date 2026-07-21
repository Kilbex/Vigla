use crate::mission::MissionSpec;
use crate::mission_event::TaskDescriptor;

pub(super) fn format_decompose_prompt(spec: &MissionSpec) -> String {
    let tests = spec.tests.as_deref().unwrap_or("(none configured)");
    let worker_count = spec
        .worker_count
        .map(|n| format!("Requested worker count: exactly {n} task(s) when feasible."))
        .unwrap_or_else(|| "Requested worker count: supervisor decides.".into());
    format!(
        "Decompose this mission.\n\nTitle: {}\nObjective: {}\nTarget branch: {}\nTests: {}\n\n\
        {}\n\n\
        Before decomposing, follow the \"Codebase discovery\" procedure in your \
        playbook: skim the README, any AGENTS.md / CLAUDE.md, the top-level \
        package manifest, and a depth-1-or-2 view of the directory tree. Then \
        emit a single `decompose` action with between 1 and 6 tasks, with \
        task titles that reference real files and idioms from this codebase. \
        Each task must be independently mergeable.",
        spec.title, spec.objective, spec.target_ref, tests, worker_count
    )
}

/// QC-2: regenerate-with-hint variant. Used on the 2nd+ decompose
/// pass when the user asked for a new plan. `prior_generation` is
/// included in the prompt for the supervisor's context.
pub(super) fn format_decompose_prompt_with_hint(
    spec: &MissionSpec,
    hint: &str,
    prior_generation: u32,
) -> String {
    let trimmed = hint.trim();
    let hint_block = if trimmed.is_empty() {
        "The user rejected the previous plan without specific feedback. \
         Try a different decomposition — different granularity, different \
         task boundaries, or different sequencing."
            .to_string()
    } else {
        format!(
            "The user reviewed your previous plan (generation {prior_generation}) \
             and asked you to regenerate with this feedback: \"{trimmed}\". \
             Take the feedback seriously and produce a meaningfully different \
             decomposition that addresses it."
        )
    };
    let base = format_decompose_prompt(spec);
    format!("{base}\n\n{hint_block}")
}

pub(super) fn format_complete_prompt(tasks: &[TaskDescriptor]) -> String {
    format!(
        "All {} workers have submitted and been reviewed. \
        Declare the mission complete with a two-sentence summary.",
        tasks.len()
    )
}
