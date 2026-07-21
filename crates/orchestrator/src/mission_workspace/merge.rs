//! Merge-outcome types and conflict detection helpers for
//! `integrate_worker`. The rebase-first strategy fans out into two
//! observable outcomes:
//!
//! - `MergeOutcome::Success(Integration)` — worker branch was
//!   rebased + fast-forward-merged into the supervisor branch.
//! - `MergeOutcome::Conflict(ConflictReport)` — rebase produced
//!   unmerged paths; the rebase is aborted (workspace is clean) and
//!   the caller escalates as `AuthorityBound::Reversibility`.

use crate::mission_workspace::Integration;
use serde::{Deserialize, Serialize};
use specta::Type;

/// What `integrate_worker` returns. The `?` operator is intentional-
/// ly *not* the failure path — conflicts are a typed first-class
/// outcome that the arbiter consumes, not an Err the supervisor
/// must unwrap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    Success(Integration),
    Conflict(ConflictReport),
}

/// Conflict shape recovered from `git status --porcelain=v2`.
/// Mapped from the porcelain-v2 XY status codes per
/// https://git-scm.com/docs/git-status#_porcelain_format_version_2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// Both ours and theirs added the same path.
    AddAdd,
    /// Both ours and theirs modified the same path.
    EditEdit,
    /// One side deleted, the other edited.
    DeleteEdit,
    /// Other / unrecognised; report as a generic conflict.
    Other,
}

/// Per-path conflict detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ConflictPath {
    pub path: String,
    pub kind: ConflictKind,
}

/// Summary of an aborted rebase. The supervisor worktree has been
/// reset to its pre-rebase state by the time this is returned, so
/// the caller can drop the worker without further git ops.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ConflictReport {
    pub worker_id: String,
    pub conflicts: Vec<ConflictPath>,
}

impl ConflictReport {
    pub fn summary(&self) -> String {
        format!("rebase conflict in {} file(s)", self.conflicts.len())
    }
}

/// Parse `git status --porcelain=v2` output and extract any
/// unmerged paths into `ConflictPath` entries.
///
/// Format reference:
/// https://git-scm.com/docs/git-status#_porcelain_format_version_2
///
/// Unmerged entries start with `u`. The XY codes after `u` tell us
/// the shape:
/// - `UU` → both modified (edit/edit)
/// - `AA` → both added (add/add)
/// - `DU` / `UD` → one side deleted, other modified (delete/edit)
/// - other combinations → `ConflictKind::Other`
pub fn parse_unmerged(porcelain_v2: &str) -> Vec<ConflictPath> {
    let mut out = Vec::new();
    for line in porcelain_v2.lines() {
        let Some(rest) = line.strip_prefix("u ") else {
            continue;
        };
        // u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>
        let mut parts = rest.splitn(10, ' ');
        let xy = parts.next().unwrap_or("");
        // Skip 8 metadata fields (sub + 3 modes + worktree mode + 3 hashes).
        for _ in 0..8 {
            let _ = parts.next();
        }
        let path = parts.next().unwrap_or("").to_string();
        if path.is_empty() {
            continue;
        }
        let kind = classify_xy(xy);
        out.push(ConflictPath { path, kind });
    }
    out
}

fn classify_xy(xy: &str) -> ConflictKind {
    match xy {
        "UU" => ConflictKind::EditEdit,
        "AA" => ConflictKind::AddAdd,
        "DU" | "UD" => ConflictKind::DeleteEdit,
        _ => ConflictKind::Other,
    }
}

use crate::mission_workspace::{MissionGitError, MissionWorkspace};

/// Try `git rebase <supervisor_branch>` inside the worker
/// worktree, then fast-forward-merge the worker branch into the
/// supervisor branch. If the rebase produces conflicts, abort the
/// rebase (leaving the worker worktree clean) and return
/// `MergeOutcome::Conflict`.
///
/// Caller invariants:
/// - `worker_worktree` is the worker's checkout, currently on
///   `worker_branch`.
/// - `supervisor_branch` exists locally.
///
/// The supervisor worktree is left untouched on the conflict path.
pub async fn try_rebase_then_ff(
    workspace: &MissionWorkspace,
    worker_id: &str,
    n: u32,
    summary: &str,
) -> Result<MergeOutcome, MissionGitError> {
    let worker_branch = workspace.worker_branch(worker_id)?;
    let worker_worktree = workspace.worker_worktree_path(worker_id)?;
    let supervisor_worktree = workspace.supervisor_worktree_path();
    let supervisor_branch = workspace.supervisor_branch();
    let supervisor_before = workspace
        .run_git_in(&supervisor_worktree, &["rev-parse", "HEAD"])
        .await?;
    let worker_before = workspace
        .run_git_in(&worker_worktree, &["rev-parse", "HEAD"])
        .await?;

    // 1. Create pre-merge tag at current supervisor HEAD.
    let expected_pre_merge_tag = workspace.pre_merge_tag(n);
    let pre_merge_tag_preexisted = workspace.tag_sha(&expected_pre_merge_tag).await?.is_some();
    let snapshot_tag = workspace.snapshot_tag(n);
    workspace.require_tag_absent(&snapshot_tag).await?;
    let pre_merge_tag = workspace.create_pre_merge_tag(n).await?;

    // 2. Try the rebase inside the worker worktree.
    let rebase = workspace
        .run_git_in(&worker_worktree, &["rebase", &supervisor_branch])
        .await;

    if rebase.is_err() {
        // Rebase failed — check whether it failed cleanly (no
        // conflicts) or with conflicts.
        let porcelain = workspace
            .run_git_in(&worker_worktree, &["status", "--porcelain=v2"])
            .await
            .unwrap_or_default();
        let conflicts = parse_unmerged(&porcelain);

        // Always try to abort. Idempotent: if rebase already
        // bailed clean, abort no-ops.
        let _ = workspace
            .run_git_in(&worker_worktree, &["rebase", "--abort"])
            .await;

        if conflicts.is_empty() {
            // Non-conflict rebase failure (e.g., dirty worktree,
            // missing ref). Re-run rebase to surface the real error.
            return Err(MissionGitError::Refused(format!(
                "rebase failed without conflicts in {}",
                worker_worktree.display()
            )));
        }

        return Ok(MergeOutcome::Conflict(ConflictReport {
            worker_id: worker_id.to_string(),
            conflicts,
        }));
    }

    // 3. Fast-forward the supervisor branch onto the rebased
    // worker tip.
    let message = format!("integrate {worker_id}: {summary}");

    // We deliberately keep `--no-ff` to preserve the merge commit
    // shape from before S4 — this is what audit/post-integration
    // expects when re-running tests on supervisor/main.
    if let Err(merge_error) = workspace
        .run_git_in(
            &supervisor_worktree,
            &["merge", "--no-ff", &worker_branch, "-m", &message],
        )
        .await
    {
        rollback_integration_attempt(
            workspace,
            &supervisor_worktree,
            &supervisor_before,
            &worker_worktree,
            &worker_before,
            &pre_merge_tag,
            pre_merge_tag_preexisted,
            &snapshot_tag,
        )
        .await
        .map_err(|rollback_error| {
            MissionGitError::Refused(format!(
                "worker integration failed ({merge_error}); rollback also failed ({rollback_error})"
            ))
        })?;
        return Err(merge_error);
    }

    let integrated_sha = workspace
        .run_git_in(&supervisor_worktree, &["rev-parse", "HEAD"])
        .await?
        .trim()
        .to_string();

    if let Err(tag_error) = workspace
        .run_git(&["tag", &snapshot_tag, &integrated_sha])
        .await
    {
        rollback_integration_attempt(
            workspace,
            &supervisor_worktree,
            &supervisor_before,
            &worker_worktree,
            &worker_before,
            &pre_merge_tag,
            pre_merge_tag_preexisted,
            &snapshot_tag,
        )
        .await
        .map_err(|rollback_error| {
            MissionGitError::Refused(format!(
                "snapshot tag creation failed ({tag_error}); rollback also failed ({rollback_error})"
            ))
        })?;
        return Err(tag_error);
    }

    Ok(MergeOutcome::Success(
        crate::mission_workspace::Integration {
            integration_sha: integrated_sha,
            snapshot_tag,
            pre_merge_tag,
        },
    ))
}

async fn rollback_integration_attempt(
    workspace: &MissionWorkspace,
    supervisor_worktree: &std::path::Path,
    supervisor_before: &str,
    worker_worktree: &std::path::Path,
    worker_before: &str,
    pre_merge_tag: &str,
    pre_merge_tag_preexisted: bool,
    snapshot_tag: &str,
) -> Result<(), MissionGitError> {
    let _ = workspace
        .run_git_in(supervisor_worktree, &["merge", "--abort"])
        .await;
    workspace
        .run_git_in(supervisor_worktree, &["reset", "--hard", supervisor_before])
        .await?;
    workspace
        .run_git_in(worker_worktree, &["reset", "--hard", worker_before])
        .await?;
    if workspace.tag_sha(snapshot_tag).await?.is_some() {
        workspace.run_git(&["tag", "-d", snapshot_tag]).await?;
    }
    if !pre_merge_tag_preexisted && workspace.tag_sha(pre_merge_tag).await?.is_some() {
        workspace.run_git(&["tag", "-d", pre_merge_tag]).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_report_summary_includes_count() {
        let r = ConflictReport {
            worker_id: "mock-1".into(),
            conflicts: vec![
                ConflictPath {
                    path: "src/lib.rs".into(),
                    kind: ConflictKind::EditEdit,
                },
                ConflictPath {
                    path: "Cargo.toml".into(),
                    kind: ConflictKind::AddAdd,
                },
            ],
        };
        assert_eq!(r.summary(), "rebase conflict in 2 file(s)");
    }

    #[test]
    fn conflict_kind_serializes_snake_case() {
        let kinds = [
            (ConflictKind::AddAdd, "add_add"),
            (ConflictKind::EditEdit, "edit_edit"),
            (ConflictKind::DeleteEdit, "delete_edit"),
            (ConflictKind::Other, "other"),
        ];
        for (k, expected) in kinds {
            let json = serde_json::to_string(&k).unwrap();
            assert_eq!(json, format!("\"{expected}\""));
            let back: ConflictKind = serde_json::from_str(&json).unwrap();
            assert_eq!(k, back);
        }
    }

    #[test]
    fn parse_unmerged_recovers_three_shapes() {
        // Sample porcelain-v2 output with one of each interesting shape.
        let porcelain = "\
1 .M N... 100644 100644 100644 abc123 def456 normal.rs
u UU N... 100644 100644 100644 100644 aaa bbb ccc src/edit_both.rs
u AA N... 000000 100644 100644 100644 000 ddd eee src/add_both.rs
u DU N... 100644 100644 000000 100644 fff ggg 000 src/we_deleted_they_edited.rs
u UD N... 100644 000000 100644 100644 hhh 000 iii src/we_edited_they_deleted.rs
";
        let cs = parse_unmerged(porcelain);
        assert_eq!(cs.len(), 4);
        assert_eq!(cs[0].path, "src/edit_both.rs");
        assert_eq!(cs[0].kind, ConflictKind::EditEdit);
        assert_eq!(cs[1].kind, ConflictKind::AddAdd);
        assert_eq!(cs[2].kind, ConflictKind::DeleteEdit);
        assert_eq!(cs[3].kind, ConflictKind::DeleteEdit);
    }

    #[test]
    fn parse_unmerged_empty_when_no_unmerged() {
        let porcelain = "\
1 .M N... 100644 100644 100644 abc def normal.rs
1 .M N... 100644 100644 100644 ghi jkl another.rs
";
        assert!(parse_unmerged(porcelain).is_empty());
    }

    #[test]
    fn classify_xy_other_falls_through() {
        assert_eq!(classify_xy("XZ"), ConflictKind::Other);
        assert_eq!(classify_xy(""), ConflictKind::Other);
    }
}
