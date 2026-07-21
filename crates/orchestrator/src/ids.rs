//! ID and time helpers used by the supervision pipeline.

use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// New UUIDv7 string for a freshly spawned worker. Time-ordered, so
/// SQLite indexes on ID columns track creation order naturally.
pub fn new_worker_id() -> String {
    Uuid::now_v7().to_string()
}

/// New UUIDv7 string for a freshly created task.
pub fn new_task_id() -> String {
    Uuid::now_v7().to_string()
}

/// Canonical worker_id format for a task at `task_index` in a
/// decomposition (1-based: task 0 → `mock-1`, task 1 → `mock-2`, …).
///
/// **Single source of truth** for the worker-id naming convention.
/// V1.3 retrieval (`run_task.rs`) reconstructs upstream worker_ids
/// from `TaskDescriptor.depends_on` to filter `list_handoffs_for_mission`
/// — that reconstruction *must* agree with the format spawn sites use,
/// or `upstream_handoffs` silently degrades to empty and retrieval
/// loses upstream context with no warning. Centralising here removes
/// the latent coupling between `mission_runtime/mock.rs`,
/// `mission_supervisor_run/run_task.rs` (spawn site), and the
/// retrieval-brief construction (reconstruction site).
///
/// The `mock-` prefix is the project-wide convention while real CLI
/// vendors are multiplexed under a pre-production wrapper; it defines
/// the worker-id shape the retrieval side reconstructs against.
pub fn worker_id_for_task_index(task_index: u32) -> String {
    format!("mock-{}", task_index + 1)
}

/// Inverse of [`worker_id_for_task_index`]. Returns `Some(idx)` iff
/// `wid` was produced by that function; `None` otherwise. Used by
/// tests (and any future replay tooling) to guard the round-trip
/// invariant — if a code change ever silently changes the format, the
/// `worker_id_roundtrip` test in this module will fail loudly.
pub fn parse_worker_id_task_index(wid: &str) -> Option<u32> {
    let suffix = wid.strip_prefix("mock-")?;
    let n: u32 = suffix.parse().ok()?;
    n.checked_sub(1)
}

/// Generate a human-readable mission ID: kebab-slug of `title` plus a
/// 16-hex-char suffix (the final 64 bits of a fresh UUIDv7). The
/// slug appears in branch names
/// (e.g. `vigla/add-logout-7a3f/supervisor`) so it must stay
/// terminal-friendly.
pub fn new_mission_id(title: &str) -> String {
    let slug = slugify(title);
    let uuid_str = Uuid::now_v7().simple().to_string();
    let suffix = &uuid_str[uuid_str.len() - 16..];
    format!("{slug}-{suffix}")
}

/// ASCII kebab-case slug: lowercase alphanumerics joined by single
/// hyphens. Non-ASCII characters and punctuation collapse to a hyphen;
/// runs of hyphens are deduped; leading/trailing hyphens are trimmed.
/// Returns `"mission"` if the input slugs to empty (e.g. all CJK).
fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_hyphen = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_hyphen = false;
        } else if !prev_hyphen {
            out.push('-');
            prev_hyphen = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("mission");
    }
    out
}

/// Current wall clock as RFC 3339 UTC with millisecond precision.
/// Defers to the workspace-shared formatter in `event_schema::time`.
pub fn rfc3339_now() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    event_schema::time::rfc3339_from_unix_ms(ms)
}

/// Public alias used by the memory archive sweep (A5) and workspace
/// retention to derive arbitrary cutoff timestamps. Delegates to the
/// shared [`event_schema::time::rfc3339_from_unix_ms`].
pub fn rfc3339_from_unix_ms_pub(ms: u64) -> String {
    event_schema::time::rfc3339_from_unix_ms(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_well_formed() {
        let a = new_worker_id();
        let b = new_worker_id();
        assert_ne!(a, b);
        // UUIDv7 string is 36 chars: 8-4-4-4-12.
        assert_eq!(a.len(), 36);
        assert_eq!(a.matches('-').count(), 4);
    }

    #[test]
    fn rfc3339_now_has_canonical_shape() {
        let ts = rfc3339_now();
        assert!(ts.ends_with('Z'));
        assert!(ts.contains('T'));
        // "YYYY-MM-DDTHH:MM:SS.mmmZ" = 24 chars.
        assert_eq!(ts.len(), 24);
    }

    #[test]
    fn slugify_handles_common_cases() {
        assert_eq!(slugify("Add logout endpoint"), "add-logout-endpoint");
        assert_eq!(slugify("  Spaces  around  "), "spaces-around");
        assert_eq!(slugify("Punctuation!?"), "punctuation");
        assert_eq!(slugify("Multiple   spaces"), "multiple-spaces");
        assert_eq!(slugify("Mixed 中文 chars"), "mixed-chars");
        assert_eq!(slugify(""), "mission");
        assert_eq!(slugify("中文"), "mission");
        assert_eq!(slugify("---"), "mission");
    }

    #[test]
    fn new_mission_id_has_slug_and_suffix_shape() {
        let id = new_mission_id("Add logout endpoint");
        // expect "add-logout-endpoint-<16hex>"
        assert!(id.starts_with("add-logout-endpoint-"), "got {id}");
        let suffix = id.rsplit('-').next().unwrap();
        assert_eq!(suffix.len(), 16);
        assert!(
            suffix.chars().all(|c| c.is_ascii_hexdigit()),
            "suffix {suffix} should be 16 hex chars"
        );
    }

    #[test]
    fn new_mission_id_falls_back_when_title_is_empty() {
        let id = new_mission_id("");
        assert!(id.starts_with("mission-"), "got {id}");
        assert_eq!(id.len(), "mission-".len() + 16);
    }

    #[test]
    fn new_mission_id_is_unique_across_calls() {
        let ids = (0..10_000)
            .map(|_| new_mission_id("same title"))
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), 10_000, "mission IDs must not collide");
    }

    #[test]
    fn worker_id_for_task_index_golden_format() {
        // Pin the canonical worker_id format. If this test ever needs
        // updating, every call site that reconstructs upstream
        // worker_ids from task indices must be reviewed in the same
        // PR — silent retrieval degradation otherwise (V1.3 retrieval
        // brief upstream_handoffs filter loses all matches).
        assert_eq!(worker_id_for_task_index(0), "mock-1");
        assert_eq!(worker_id_for_task_index(1), "mock-2");
        assert_eq!(worker_id_for_task_index(41), "mock-42");
    }

    #[test]
    fn worker_id_roundtrip() {
        for idx in [0u32, 1, 2, 9, 41, 999] {
            let wid = worker_id_for_task_index(idx);
            assert_eq!(
                parse_worker_id_task_index(&wid),
                Some(idx),
                "round-trip failed for idx {idx} via {wid}"
            );
        }
    }

    #[test]
    fn parse_worker_id_task_index_rejects_unknown_shapes() {
        assert_eq!(parse_worker_id_task_index("worker-1"), None);
        assert_eq!(parse_worker_id_task_index("mock-"), None);
        assert_eq!(parse_worker_id_task_index("mock-abc"), None);
        assert_eq!(parse_worker_id_task_index("mock-0"), None); // 1-based
        assert_eq!(parse_worker_id_task_index(""), None);
    }
}
