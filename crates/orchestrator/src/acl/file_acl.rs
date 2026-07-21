//! Effective per-worker file allow-list.
//!
//! Constructed at worker-spawn time from the intersection of the
//! mission's `scope_paths` (set by the user) and the task's
//! `scope_paths` (set by the supervisor during decomposition).
//! Empty mission scope with no task scope means "unconstrained" —
//! the ACL is a pass-through. If a task scope is present, it still
//! narrows the worker even when the mission itself was unconstrained.
//! A task scope outside the mission scope produces an empty
//! intersection — every path is denied.
//!
//! Paths are relative to the worker's worktree root. Path matching
//! uses [`Path::starts_with`] (same semantics as the audit's
//! `score_scope`).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Effective allow-list for one worker. Build via
/// [`FileAcl::from_mission_and_task`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAcl {
    /// Empty Vec carries two distinct meanings, disambiguated by
    /// [`Self::is_unconstrained`]:
    ///
    /// * `unconstrained: true` + `allow_list: []` → no mission
    ///   scope; every path allowed.
    /// * `unconstrained: false` + `allow_list: []` → mission
    ///   scope existed but intersection with task scope was
    ///   empty; every path denied.
    allow_list: Vec<PathBuf>,
    unconstrained: bool,
}

impl FileAcl {
    /// Build the effective ACL from mission and (optional) task
    /// scope. See the module-level doc for the intersection logic.
    pub fn from_mission_and_task(
        mission_scope: &[PathBuf],
        task_scope: Option<&[PathBuf]>,
    ) -> Self {
        if mission_scope.is_empty() {
            if let Some(task) = task_scope.filter(|task| !task.is_empty()) {
                return Self {
                    allow_list: task.to_vec(),
                    unconstrained: false,
                };
            }
            return Self {
                allow_list: Vec::new(),
                unconstrained: true,
            };
        }

        // An empty task scope means "no additional constraint" (a
        // documented no-op Narrow, see arbiter::rework), NOT deny-all —
        // fall back to the mission scope, mirroring the empty-mission
        // branch above. Without the `filter`, `intersect(mission, [])`
        // returns `[]` → allow_list empty + constrained = deny every path,
        // turning a no-op rework into a total lockout.
        let allow_list = match task_scope.filter(|task| !task.is_empty()) {
            None => mission_scope.to_vec(),
            Some(task) => intersect(mission_scope, task),
        };

        Self {
            allow_list,
            unconstrained: false,
        }
    }

    /// True if the ACL is a pass-through (no mission scope was
    /// declared, so every path is allowed).
    pub fn is_unconstrained(&self) -> bool {
        self.unconstrained
    }

    /// True if `path` is inside any allow-list entry. Always true
    /// for unconstrained ACLs.
    pub fn is_path_allowed(&self, path: &str) -> bool {
        if self.unconstrained {
            return true;
        }
        let p = Path::new(path);
        self.allow_list.iter().any(|allow| p.starts_with(allow))
    }

    /// Read access to the effective allow-list. Empty + non-
    /// unconstrained means "deny all".
    pub fn allow_list(&self) -> &[PathBuf] {
        &self.allow_list
    }
}

/// Intersect two path-prefix lists. A path is in the intersection
/// if either:
///   * it appears verbatim in both, OR
///   * one side is a strict prefix of the other.
///
/// The narrower path wins (we keep the prefix that lets through
/// the smallest set of files).
fn intersect(mission: &[PathBuf], task: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for t in task {
        for m in mission {
            if t.starts_with(m) {
                // `t` is within this mission prefix — it's the narrowest
                // allowed set for `t`, so stop scanning mission prefixes.
                push_unique(&mut out, t.clone());
                break;
            }
            if m.starts_with(t) {
                // `t` is a broad prefix and `m` is one mission path under
                // it. Do NOT break: `t` may be a prefix of MULTIPLE mission
                // paths (siblings like `src/a`, `src/b`), and each must be
                // kept — breaking here dropped every sibling after the
                // first, spuriously denying legitimate in-scope writes.
                push_unique(&mut out, m.clone());
            }
        }
    }
    out
}

fn push_unique(v: &mut Vec<PathBuf>, p: PathBuf) {
    if !v.contains(&p) {
        v.push(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn pb(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn empty_mission_scope_means_unconstrained() {
        let acl = FileAcl::from_mission_and_task(&[], None);
        assert!(acl.is_unconstrained());
        assert!(acl.is_path_allowed("anything/at/all.rs"));
    }

    #[test]
    fn task_scope_narrows_empty_mission_scope() {
        let acl = FileAcl::from_mission_and_task(&[], Some(&[pb("src")]));
        assert!(!acl.is_unconstrained());
        assert!(acl.is_path_allowed("src/lib.rs"));
        assert!(!acl.is_path_allowed("docs/README.md"));
    }

    #[test]
    fn mission_scope_only_used_when_task_has_none() {
        let acl = FileAcl::from_mission_and_task(&[pb("src")], None);
        assert!(!acl.is_unconstrained());
        assert!(acl.is_path_allowed("src/lib.rs"));
        assert!(!acl.is_path_allowed("docs/README.md"));
    }

    #[test]
    fn task_scope_intersected_with_mission_scope() {
        let acl = FileAcl::from_mission_and_task(&[pb("src"), pb("tests")], Some(&[pb("src")]));
        assert!(acl.is_path_allowed("src/lib.rs"));
        assert!(!acl.is_path_allowed("tests/foo.rs"));
    }

    #[test]
    fn task_scope_outside_mission_scope_is_dropped() {
        let acl = FileAcl::from_mission_and_task(&[pb("src")], Some(&[pb("docs")]));
        assert!(!acl.is_path_allowed("src/lib.rs"));
        assert!(!acl.is_path_allowed("docs/README.md"));
        assert!(acl.allow_list().is_empty());
    }

    #[test]
    fn task_scope_completely_inside_mission_scope_keeps_task() {
        let acl = FileAcl::from_mission_and_task(&[pb("src")], Some(&[pb("src/audit")]));
        assert!(acl.is_path_allowed("src/audit/mod.rs"));
        assert!(!acl.is_path_allowed("src/main.rs"));
    }

    #[test]
    fn empty_task_scope_falls_back_to_mission_scope_not_deny_all() {
        // A no-op Narrow (empty reduced_scope) must NOT lock the worker
        // out of the whole mission scope. Regression for the deny-all
        // that `intersect(mission, [])` produced.
        let acl = FileAcl::from_mission_and_task(&[pb("src")], Some(&[]));
        assert!(!acl.is_unconstrained());
        assert!(acl.is_path_allowed("src/lib.rs"));
        assert!(!acl.is_path_allowed("docs/README.md"));
    }

    #[test]
    fn broad_task_scope_keeps_all_sibling_mission_prefixes() {
        // Task/Narrow "src" over granular mission ["src/a","src/b"] must
        // allow BOTH children. Regression for the `intersect` early
        // `break` that dropped every sibling after the first.
        let acl = FileAcl::from_mission_and_task(&[pb("src/a"), pb("src/b")], Some(&[pb("src")]));
        assert!(acl.is_path_allowed("src/a/mod.rs"));
        assert!(
            acl.is_path_allowed("src/b/mod.rs"),
            "second sibling mission prefix was dropped"
        );
        assert!(!acl.is_path_allowed("src/c/mod.rs"));
    }
}
