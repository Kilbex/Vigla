//! Whole-mission rollback.
//!
//! A merged mission is undone on its recorded target branch with a normal Git
//! revert commit, preserving any later work. Before final merge, the same API
//! falls back to rewinding the staged supervisor branch to its earliest
//! pre-integration tag; this is the recovery-receipt path.

use crate::mission_workspace::{MissionGitError, MissionWorkspace};
use std::path::Path;

/// What `revert_mission` returns on success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertOutcome {
    /// SHA the affected branch points at after rollback.
    pub restored_sha: String,
    /// Rollback anchor that identifies the state before this mission.
    pub pre_merge_tag: String,
}

impl MissionWorkspace {
    /// Undo a completed mission from its recorded target branch.
    ///
    /// Unlike [`Self::revert_mission`], this never falls back to rewinding the
    /// staging-only supervisor branch. Host commands use this narrower entry
    /// point after validating the durable `Merged` disposition and target ref.
    pub async fn revert_merged_mission(
        &self,
        target_ref: &str,
    ) -> Result<RevertOutcome, MissionGitError> {
        let before_tag = self.final_before_tag(target_ref);
        self.run_git(&["rev-parse", "--verify", &format!("refs/tags/{before_tag}")])
            .await
            .map_err(|_| {
                MissionGitError::Refused(format!("final rollback anchor {before_tag:?} is missing"))
            })?;
        let branch_ref = self.validate_target_ref(target_ref).await?;
        let reverted_tag = self.final_reverted_tag(target_ref);
        if let Some(reverted_sha) = self.tag_sha(&reverted_tag).await? {
            self.run_git(&["merge-base", "--is-ancestor", &reverted_sha, &branch_ref])
                .await
                .map_err(|_| {
                    MissionGitError::Refused(format!(
                        "revert proof {reverted_tag:?} is no longer on target branch {target_ref:?}"
                    ))
                })?;
            return Ok(RevertOutcome {
                restored_sha: reverted_sha,
                pre_merge_tag: before_tag,
            });
        }
        self.revert_merged_target(target_ref, &before_tag).await
    }

    /// Revert a mission at the strongest available boundary.
    ///
    /// Final-merge anchors take precedence and undo the mission on its target
    /// branch. If no final anchors exist, the mission has not been merged and
    /// the staged supervisor branch is reset to its earliest pre-integration
    /// tag. The repository audit log provides click-level idempotency.
    pub async fn revert_mission(&self) -> Result<RevertOutcome, MissionGitError> {
        let final_prefix = format!("vigla/revert/{}/before/", self.mission_id());
        let raw = self
            .run_git(&[
                "for-each-ref",
                "--format=%(refname:short)",
                &format!("refs/tags/{final_prefix}"),
            ])
            .await?;
        let before_tags: Vec<&str> = raw.lines().filter(|line| !line.is_empty()).collect();
        match before_tags.as_slice() {
            [] => self.revert_staged_integrations().await,
            [before_tag] => {
                let target_ref = before_tag.strip_prefix(&final_prefix).ok_or_else(|| {
                    MissionGitError::Refused("malformed final rollback anchor".into())
                })?;
                self.revert_merged_mission(target_ref).await
            }
            _ => Err(MissionGitError::Refused(format!(
                "multiple final rollback anchors found for mission {}; refusing to guess the target branch",
                self.mission_id()
            ))),
        }
    }

    async fn revert_merged_target(
        &self,
        target_ref: &str,
        before_tag: &str,
    ) -> Result<RevertOutcome, MissionGitError> {
        let branch_ref = self.validate_target_ref(target_ref).await?;
        let before_sha = self
            .run_git(&[
                "rev-parse",
                "--verify",
                &format!("refs/tags/{before_tag}^{{commit}}"),
            ])
            .await
            .map_err(|_| {
                MissionGitError::Refused(format!(
                    "final rollback anchor {before_tag:?} does not resolve to a commit"
                ))
            })?;
        let merged_tag = self.final_merged_tag(target_ref);
        let merged_sha = self
            .run_git(&[
                "rev-parse",
                "--verify",
                &format!("refs/tags/{merged_tag}^{{commit}}"),
            ])
            .await
            .map_err(|_| {
                MissionGitError::Refused(format!("final merge anchor {merged_tag:?} is missing"))
            })?;
        self.validate_final_merge_topology(&before_sha, &merged_sha)
            .await?;
        self.run_git(&["merge-base", "--is-ancestor", &merged_sha, &branch_ref])
            .await
            .map_err(|_| {
                MissionGitError::Refused(format!(
                    "mission merge {merged_sha} is no longer an ancestor of target branch {target_ref:?}"
                ))
            })?;

        let restored_sha = if let Some(worktree) = self.checked_out_worktree(&branch_ref).await? {
            self.revert_in_checked_out_worktree(target_ref, &branch_ref, &merged_sha, &worktree)
                .await?
        } else {
            self.revert_in_detached_worktree(target_ref, &branch_ref, &merged_sha)
                .await?
        };

        Ok(RevertOutcome {
            restored_sha,
            pre_merge_tag: before_tag.to_string(),
        })
    }

    async fn revert_in_checked_out_worktree(
        &self,
        target_ref: &str,
        branch_ref: &str,
        merged_sha: &str,
        worktree: &Path,
    ) -> Result<String, MissionGitError> {
        self.require_clean_worktree(worktree, target_ref).await?;
        let branch_sha = self.run_git(&["rev-parse", branch_ref]).await?;
        let worktree_sha = self.run_git_in(worktree, &["rev-parse", "HEAD"]).await?;
        if branch_sha != worktree_sha {
            return Err(MissionGitError::Refused(format!(
                "target worktree for {target_ref:?} is not at the branch head"
            )));
        }
        if let Err(error) = self
            .run_git_in(worktree, &["revert", "--no-edit", "-m", "1", merged_sha])
            .await
        {
            let _ = self.run_git_in(worktree, &["revert", "--abort"]).await;
            return Err(error);
        }
        let reverted_sha = self.run_git_in(worktree, &["rev-parse", "HEAD"]).await?;
        let reverted_tag = self.final_reverted_tag(target_ref);
        if let Err(tag_error) = self.ensure_tag_at(&reverted_tag, &reverted_sha).await {
            self.run_git_in(worktree, &["reset", "--hard", &branch_sha])
                .await
                .map_err(|rollback_error| {
                    MissionGitError::Refused(format!(
                        "recording revert proof failed ({tag_error}); restoring target {target_ref:?} also failed ({rollback_error})"
                    ))
                })?;
            if self.tag_sha(&reverted_tag).await?.as_deref() == Some(&reverted_sha) {
                let _ = self.run_git(&["tag", "-d", &reverted_tag]).await;
            }
            return Err(tag_error);
        }
        Ok(reverted_sha)
    }

    async fn revert_in_detached_worktree(
        &self,
        target_ref: &str,
        branch_ref: &str,
        merged_sha: &str,
    ) -> Result<String, MissionGitError> {
        let temp = self
            .repo_root()
            .join(".vigla/temp/revert")
            .join(self.mission_id());
        if let Some(parent) = temp.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| MissionGitError::Io(e.to_string()))?;
        }
        let temp_str = temp
            .to_str()
            .ok_or_else(|| MissionGitError::Io("revert temp path is not UTF-8".into()))?;
        self.run_git(&["worktree", "add", "--detach", temp_str, target_ref])
            .await?;

        let result: Result<String, MissionGitError> = async {
            let base_sha = self.run_git_in(&temp, &["rev-parse", "HEAD"]).await?;
            if let Err(error) = self
                .run_git_in(&temp, &["revert", "--no-edit", "-m", "1", merged_sha])
                .await
            {
                let _ = self.run_git_in(&temp, &["revert", "--abort"]).await;
                return Err(error);
            }
            let reverted_sha = self.run_git_in(&temp, &["rev-parse", "HEAD"]).await?;
            let reverted_ref = format!("refs/tags/{}", self.final_reverted_tag(target_ref));
            let transaction = format!(
                "start\nupdate {branch_ref} {reverted_sha} {base_sha}\ncreate {reverted_ref} {reverted_sha}\nprepare\ncommit\n"
            );
            self.run_git_with_stdin(&["update-ref", "--stdin"], transaction.as_bytes())
                .await?;
            Ok(reverted_sha)
        }
        .await;

        let _ = self
            .run_git(&["worktree", "remove", "--force", temp_str])
            .await;
        result
    }

    async fn revert_staged_integrations(&self) -> Result<RevertOutcome, MissionGitError> {
        // `list_pre_merge_tags` is sorted newest-first, so the earliest
        // integration's snapshot (the mission branch point) is `.last()`.
        let tags = self.list_pre_merge_tags().await?;
        let earliest = tags.last().ok_or_else(|| {
            MissionGitError::Refused(format!(
                "no pre-merge tags found for mission {}",
                self.mission_id()
            ))
        })?;
        let pre_merge_tag = earliest.clone();

        let supervisor_branch = self.supervisor_branch();
        // Confirm supervisor branch exists.
        self.run_git(&["rev-parse", "--verify", &supervisor_branch])
            .await
            .map_err(|_| {
                MissionGitError::Refused(format!(
                    "supervisor branch {supervisor_branch:?} not found"
                ))
            })?;

        // Resolve target SHA.
        let target_sha = self
            .run_git(&["rev-parse", &format!("refs/tags/{pre_merge_tag}")])
            .await?
            .trim()
            .to_string();

        // Reset the supervisor branch to the tag. Use
        // update-ref instead of `reset --hard` so we don't need a
        // worktree checkout — `reset --hard` requires being inside
        // a worktree.
        self.run_git(&[
            "update-ref",
            &format!(
                "refs/heads/{}",
                supervisor_branch.trim_start_matches("refs/heads/")
            ),
            &target_sha,
        ])
        .await?;

        // Also reset the supervisor worktree (if it exists) so it
        // reflects the new branch HEAD.
        let supervisor_worktree = self.supervisor_worktree_path();
        if supervisor_worktree.exists() {
            self.run_git_in(&supervisor_worktree, &["reset", "--hard", &target_sha])
                .await?;
        }

        Ok(RevertOutcome {
            restored_sha: target_sha,
            pre_merge_tag,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::mission_workspace::tests::bootstrap_workspace_with_supervisor_branch;

    #[tokio::test]
    async fn revert_resets_supervisor_branch_to_pre_merge_tag() {
        let (w, _td) = bootstrap_workspace_with_supervisor_branch().await;

        // Capture baseline supervisor SHA.
        let baseline = w
            .run_git(&["rev-parse", &w.supervisor_branch()])
            .await
            .unwrap()
            .trim()
            .to_string();

        // Create pre-merge tag at baseline.
        w.create_pre_merge_tag(0).await.unwrap();

        // Advance the supervisor branch with a fake "integration".
        let sup_wt = w.supervisor_worktree_path();
        tokio::fs::write(sup_wt.join("integrate.txt"), "x")
            .await
            .unwrap();
        w.run_git_in(&sup_wt, &["add", "integrate.txt"])
            .await
            .unwrap();
        w.run_git_in(&sup_wt, &["commit", "-m", "integrate"])
            .await
            .unwrap();

        let advanced = w
            .run_git(&["rev-parse", &w.supervisor_branch()])
            .await
            .unwrap()
            .trim()
            .to_string();
        assert_ne!(advanced, baseline);

        // Revert.
        let outcome = w.revert_mission().await.unwrap();
        assert_eq!(outcome.restored_sha, baseline);
        assert!(outcome.pre_merge_tag.ends_with("/0"));

        let after = w
            .run_git(&["rev-parse", &w.supervisor_branch()])
            .await
            .unwrap()
            .trim()
            .to_string();
        assert_eq!(after, baseline);
    }

    #[tokio::test]
    async fn revert_full_mission_resets_to_earliest_tag_not_last_integration() {
        // Multi-integration mission: tags /0 (branch point), /1 (after
        // task 0). Reverting must undo BOTH tasks back to the branch
        // point, not just the final integration. Regression for the
        // `tags.first()` (newest) vs `tags.last()` (earliest) bug that
        // left earlier tasks on the branch after a "full" revert.
        let (w, _td) = bootstrap_workspace_with_supervisor_branch().await;
        let sup_wt = w.supervisor_worktree_path();

        // Baseline = the mission branch point (before any integration).
        let baseline = w
            .run_git(&["rev-parse", &w.supervisor_branch()])
            .await
            .unwrap()
            .trim()
            .to_string();

        // Integration 0: snapshot at baseline, then advance the branch.
        w.create_pre_merge_tag(0).await.unwrap();
        tokio::fs::write(sup_wt.join("t0.txt"), "0").await.unwrap();
        w.run_git_in(&sup_wt, &["add", "t0.txt"]).await.unwrap();
        w.run_git_in(&sup_wt, &["commit", "-m", "integrate 0"])
            .await
            .unwrap();
        let after0 = w
            .run_git(&["rev-parse", &w.supervisor_branch()])
            .await
            .unwrap()
            .trim()
            .to_string();
        assert_ne!(after0, baseline);

        // Integration 1: snapshot at S0, then advance again.
        w.create_pre_merge_tag(1).await.unwrap();
        tokio::fs::write(sup_wt.join("t1.txt"), "1").await.unwrap();
        w.run_git_in(&sup_wt, &["add", "t1.txt"]).await.unwrap();
        w.run_git_in(&sup_wt, &["commit", "-m", "integrate 1"])
            .await
            .unwrap();

        // Revert must return the supervisor branch to the branch point,
        // undoing both integrations.
        let outcome = w.revert_mission().await.unwrap();
        assert_eq!(
            outcome.restored_sha, baseline,
            "full-mission revert must reset to the branch point, not the last integration"
        );
        assert!(outcome.pre_merge_tag.ends_with("/0"));

        let after = w
            .run_git(&["rev-parse", &w.supervisor_branch()])
            .await
            .unwrap()
            .trim()
            .to_string();
        assert_eq!(after, baseline);
    }

    #[tokio::test]
    async fn revert_with_no_pre_merge_tag_refuses() {
        let (w, _td) = bootstrap_workspace_with_supervisor_branch().await;
        let err = w.revert_mission().await.unwrap_err();
        let s = err.to_string();
        assert!(s.contains("no pre-merge tags"), "got: {s}");
    }
}
