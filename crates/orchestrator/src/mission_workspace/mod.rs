//! Mission-scoped git workspace.
//!
//! Implements the Single Supervisor Integration Branch topology
//! (see `ARCHITECTURE.md`, "Mission Lifecycle") against a
//! real git repository. All operations are confined to the
//! `vigla/<mission-id>/*` ref namespace and the
//! `.vigla/worktrees/<mission-id>/` directory; the user's main
//! checkout and target ref are never touched except by explicit
//! [`MissionWorkspace::final_merge`] and merged-mission rollback actions.
//!
//! This module contains only git operations. Mission lifecycle policy,
//! worker adapters, and host IPC stay in their respective layers.

use crate::mission::MissionId;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

pub mod merge;
pub mod retention;
pub mod revert;

pub use merge::{ConflictKind, ConflictPath, ConflictReport, MergeOutcome};
pub use revert::RevertOutcome;

// Repository operations may legitimately run hooks or scan large histories.
// Five minutes keeps them finite without imposing an interactive-command
// deadline; the aggregate cap still prevents stdout/stderr from growing
// without bound while the child runs.
const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MAX_GIT_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
struct GitCommandOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn read_bounded_git_output<R>(
    mut reader: R,
    stream: &'static str,
    captured_bytes: Arc<AtomicUsize>,
    max_output_bytes: usize,
) -> std::io::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut captured = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            return Ok(captured);
        }
        let previous = captured_bytes.fetch_add(read, Ordering::Relaxed);
        if previous.saturating_add(read) > max_output_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("git {stream} exceeded {max_output_bytes} bytes of captured output"),
            ));
        }
        captured.extend_from_slice(&chunk[..read]);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MissionGitError {
    #[error("invalid id ({kind}): {id}")]
    InvalidId { kind: String, id: String },

    #[error("git command failed (exit {code}): {stderr}")]
    Git { code: i32, stderr: String },

    #[error("io: {0}")]
    Io(String),

    #[error("refused: {0}")]
    Refused(String),
}

/// One successful integration of a worker branch into the supervisor
/// branch. Returned by [`MissionWorkspace::integrate_worker`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Integration {
    pub integration_sha: String,
    pub snapshot_tag: String,
    pub pre_merge_tag: String,
}

/// Handle to a mission's git workspace inside a host repo. Stateless:
/// every method shells out to `git` against the repo each time. Cheap
/// to construct and pass around.
#[derive(Debug, Clone)]
pub struct MissionWorkspace {
    repo_root: PathBuf,
    mission_id: MissionId,
}

impl MissionWorkspace {
    pub fn new(repo_root: PathBuf, mission_id: MissionId) -> Result<Self, MissionGitError> {
        Self::validate_id("mission_id", &mission_id)?;
        Ok(Self {
            repo_root,
            mission_id,
        })
    }

    pub fn mission_id(&self) -> &str {
        &self.mission_id
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    // -----------------------------------------------------------------
    // Pure name/path derivation
    // -----------------------------------------------------------------

    pub fn supervisor_branch(&self) -> String {
        format!("vigla/{}/supervisor", self.mission_id)
    }

    pub fn supervisor_worktree_path(&self) -> PathBuf {
        self.repo_root
            .join(".vigla/worktrees")
            .join(&self.mission_id)
            .join("supervisor")
    }

    pub fn worker_branch(&self, worker_id: &str) -> Result<String, MissionGitError> {
        Self::validate_worker_id(worker_id)?;
        Ok(format!("vigla/{}/worker/{}", self.mission_id, worker_id))
    }

    pub fn worker_worktree_path(&self, worker_id: &str) -> Result<PathBuf, MissionGitError> {
        Self::validate_worker_id(worker_id)?;
        Ok(self
            .repo_root
            .join(".vigla/worktrees")
            .join(&self.mission_id)
            .join(worker_id))
    }

    pub fn snapshot_tag(&self, n: u32) -> String {
        format!("vigla/snap/{}/{}", self.mission_id, n)
    }

    /// Name of the pre-merge tag for integration index `n`. Always
    /// uses the same namespace prefix so the compaction job can
    /// pattern-match.
    pub fn pre_merge_tag(&self, n: u32) -> String {
        format!("vigla/pre-merge/{}/{}", self.mission_id, n)
    }

    /// Persistent rollback anchor at the target branch's pre-mission commit.
    /// The target ref is encoded in the suffix so a later host process can
    /// resolve the correct branch without an in-memory mission handle.
    pub fn final_before_tag(&self, target_ref: &str) -> String {
        format!("vigla/revert/{}/before/{target_ref}", self.mission_id)
    }

    /// Persistent anchor at the mission's final merge commit.
    pub fn final_merged_tag(&self, target_ref: &str) -> String {
        format!("vigla/revert/{}/merged/{target_ref}", self.mission_id)
    }

    /// Durable proof that the merge was reverted. This closes the Git/SQLite
    /// crash window: a retry can reuse the first revert commit instead of
    /// creating a second inverse commit.
    pub fn final_reverted_tag(&self, target_ref: &str) -> String {
        format!("vigla/revert/{}/reverted/{target_ref}", self.mission_id)
    }

    /// Create the pre-merge tag at the current supervisor-branch
    /// HEAD. Idempotent: if the tag already exists at the same SHA,
    /// no-ops; if it exists at a different SHA (shouldn't happen),
    /// errors out so we don't silently lose a snapshot.
    pub async fn create_pre_merge_tag(&self, n: u32) -> Result<String, MissionGitError> {
        let tag = self.pre_merge_tag(n);
        let supervisor_branch = self.supervisor_branch();

        let target_sha = self.run_git(&["rev-parse", &supervisor_branch]).await?;

        // Check if tag exists already.
        let existing = self
            .run_git(&[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/tags/{tag}"),
            ])
            .await
            .ok();

        match existing {
            Some(sha) if sha.trim() == target_sha.trim() => {
                // Idempotent: same tag at same SHA.
                Ok(tag)
            }
            Some(_) => Err(MissionGitError::Refused(format!(
                "pre-merge tag {tag:?} already exists at a different SHA"
            ))),
            None => {
                self.run_git(&["tag", &tag, target_sha.trim()]).await?;
                Ok(tag)
            }
        }
    }

    /// List all pre-merge tags for this mission, newest-first by
    /// integration index.
    pub async fn list_pre_merge_tags(&self) -> Result<Vec<String>, MissionGitError> {
        let prefix = format!("vigla/pre-merge/{}/", self.mission_id);
        let raw = self
            .run_git(&[
                "for-each-ref",
                "--format=%(refname:short)",
                &format!("refs/tags/{prefix}*"),
            ])
            .await?;
        let mut tags: Vec<String> = raw.lines().map(|s| s.to_string()).collect();
        // Sort by trailing integer descending so [0] is the latest.
        tags.sort_by(|a, b| {
            let na: u32 = a
                .rsplit('/')
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let nb: u32 = b
                .rsplit('/')
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            nb.cmp(&na)
        });
        Ok(tags)
    }

    fn mission_worktrees_dir(&self) -> PathBuf {
        self.repo_root
            .join(".vigla/worktrees")
            .join(&self.mission_id)
    }

    // -----------------------------------------------------------------
    // Git ops
    // -----------------------------------------------------------------

    /// Create the supervisor branch from `target_ref`. Refuses if
    /// `target_ref` is inside our own `vigla/` namespace, which
    /// would create a cycle. The branch is created without a checkout;
    /// call [`Self::create_supervisor_worktree`] separately.
    pub async fn create_supervisor_branch(&self, target_ref: &str) -> Result<(), MissionGitError> {
        self.install_runtime_excludes().await?;
        if target_ref.starts_with("vigla/") {
            return Err(MissionGitError::Refused(format!(
                "target_ref {target_ref:?} is inside vigla/ namespace"
            )));
        }
        let branch = self.supervisor_branch();
        self.run_git(&["branch", &branch, target_ref]).await?;
        Ok(())
    }

    async fn install_runtime_excludes(&self) -> Result<(), MissionGitError> {
        const HEADER: &str = "# Vigla generated runtime state";
        const RULES: &[&str] = &[
            "/.vigla/worktrees/",
            "/.vigla/temp/",
            "/.vigla/memory/",
            "/.vigla/endurance/",
            "/.vigla/missions/",
            "/.vigla/l1-claude-quota-exhausted.seen",
        ];

        let git_path = self
            .run_git(&["rev-parse", "--git-path", "info/exclude"])
            .await?;
        let exclude_path = {
            let path = PathBuf::from(git_path);
            if path.is_absolute() {
                path
            } else {
                self.repo_root.join(path)
            }
        };
        let existing = match tokio::fs::read_to_string(&exclude_path).await {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(MissionGitError::Io(error.to_string())),
        };
        let existing_lines = existing.lines().collect::<std::collections::HashSet<_>>();
        let missing: Vec<&str> = RULES
            .iter()
            .copied()
            .filter(|rule| !existing_lines.contains(rule))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        if let Some(parent) = exclude_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| MissionGitError::Io(error.to_string()))?;
        }
        let mut updated = existing;
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        if !updated.lines().any(|line| line == HEADER) {
            updated.push_str(HEADER);
            updated.push('\n');
        }
        for rule in missing {
            updated.push_str(rule);
            updated.push('\n');
        }
        tokio::fs::write(&exclude_path, updated)
            .await
            .map_err(|error| MissionGitError::Io(error.to_string()))
    }

    /// Add a worktree for the supervisor at
    /// `.vigla/worktrees/<mid>/supervisor` checked out to the
    /// supervisor branch.
    pub async fn create_supervisor_worktree(&self) -> Result<PathBuf, MissionGitError> {
        let path = self.supervisor_worktree_path();
        self.ensure_worktrees_parent().await?;
        self.run_git(&[
            "worktree",
            "add",
            path.to_str().ok_or_else(|| {
                MissionGitError::Io("supervisor worktree path is not UTF-8".into())
            })?,
            &self.supervisor_branch(),
        ])
        .await?;
        Ok(path)
    }

    /// Create a worker branch from supervisor HEAD. Branch must not
    /// already exist; supervisor branch must.
    pub async fn create_worker_branch(&self, worker_id: &str) -> Result<(), MissionGitError> {
        let branch = self.worker_branch(worker_id)?;
        let from = self.supervisor_branch();
        self.run_git(&["branch", &branch, &from]).await?;
        Ok(())
    }

    /// Add a worktree for the worker at
    /// `.vigla/worktrees/<mid>/<wid>` checked out to its branch.
    pub async fn create_worker_worktree(
        &self,
        worker_id: &str,
    ) -> Result<PathBuf, MissionGitError> {
        let path = self.worker_worktree_path(worker_id)?;
        let branch = self.worker_branch(worker_id)?;
        self.ensure_worktrees_parent().await?;
        self.run_git(&[
            "worktree",
            "add",
            path.to_str()
                .ok_or_else(|| MissionGitError::Io("worker worktree path is not UTF-8".into()))?,
            &branch,
        ])
        .await?;
        Ok(path)
    }

    /// Write the worker's effective [`crate::acl::FileAcl`] into a
    /// sentinel file at the worktree root
    /// (`<worktree>/.vigla/acl.json`). Idempotent; safe to
    /// call before the worker starts running.
    ///
    /// Mission-loop callers pass the live ACL they constructed via
    /// [`crate::acl::FileAcl::from_mission_and_task`]. Tests and
    /// callers that don't care about ACLs simply don't call this —
    /// the absence of a sentinel is treated as "unconstrained" by
    /// readers.
    pub async fn write_worker_acl_sentinel(
        &self,
        worker_id: &str,
        acl: &crate::acl::FileAcl,
    ) -> Result<(), MissionGitError> {
        let path = self.worker_worktree_path(worker_id)?;
        crate::acl::write_sentinel(&path, acl)
            .await
            .map_err(|e| MissionGitError::Io(e.to_string()))
    }

    /// Integrate a worker's branch into the supervisor branch.
    /// Rebase-first; on conflict, returns `MergeOutcome::Conflict`
    /// and the supervisor worktree is left at its pre-rebase state
    /// (the rebase is aborted internally).
    ///
    /// Snapshot tagging: `pre-merge-{mid}-{n}` is created BEFORE
    /// the merge attempt (even on the conflict path) so the
    /// integration history is fully reversible. `vigla/snap/
    /// {mid}/{n}` is created AT THE INTEGRATED SHA only on the
    /// Success path.
    pub async fn integrate_worker(
        &self,
        worker_id: &str,
        n: u32,
        summary: &str,
    ) -> Result<merge::MergeOutcome, MissionGitError> {
        merge::try_rebase_then_ff(self, worker_id, n, summary).await
    }

    /// Merge the supervisor branch onto `target_ref` with `--no-ff` and create
    /// persistent before/merged anchors for whole-mission revert.
    ///
    /// If the branch is checked out, its worktree must be clean and the merge
    /// runs there so HEAD, index, and files advance together. Otherwise a
    /// detached temporary worktree computes the merge and a single
    /// `update-ref` transaction atomically advances the branch and creates both
    /// anchors. `target_ref` must be a local branch outside `vigla/`.
    /// Mission worktrees are not cleaned here; call [`Self::discard`] after the
    /// merge is recorded.
    pub async fn final_merge(&self, target_ref: &str) -> Result<(), MissionGitError> {
        let branch_ref = self.validate_target_ref(target_ref).await?;
        if let Some(target_worktree) = self.checked_out_worktree(&branch_ref).await? {
            return self
                .final_merge_in_checked_out_worktree(target_ref, &branch_ref, &target_worktree)
                .await;
        }

        let temp = self
            .repo_root
            .join(".vigla/temp/final_merge")
            .join(&self.mission_id);
        if let Some(parent) = temp.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| MissionGitError::Io(e.to_string()))?;
        }
        let temp_str = temp
            .to_str()
            .ok_or_else(|| MissionGitError::Io("temp path is not UTF-8".into()))?;

        // Detached: the worktree references the commit at target_ref but
        // does not pin the branch, so the user's checkout (which may be
        // on target_ref) remains untouched.
        self.run_git(&["worktree", "add", "--detach", temp_str, target_ref])
            .await?;

        let result: Result<(), MissionGitError> = async {
            // The commit the worktree detached at — the base our merge
            // builds on, and the value `target_ref` must still hold for the
            // branch move to be lossless.
            let base_sha = self.run_git_in(&temp, &["rev-parse", "HEAD"]).await?;
            self.require_unmerged_mission_commits(&base_sha).await?;
            let supervisor_branch = self.supervisor_branch();
            let merge_msg = format!("merge Vigla mission {}", self.mission_id);
            self.run_git_in(
                &temp,
                &["merge", "--no-ff", &supervisor_branch, "-m", &merge_msg],
            )
            .await?;
            let new_sha = self.run_git_in(&temp, &["rev-parse", "HEAD"]).await?;
            self.validate_final_merge_topology(&base_sha, &new_sha)
                .await?;
            self.commit_final_refs(&branch_ref, &base_sha, &new_sha, target_ref)
                .await?;
            Ok(())
        }
        .await;

        // Always remove the temp worktree, even if the merge failed.
        let _ = self
            .run_git(&["worktree", "remove", "--force", temp_str])
            .await;

        result
    }

    async fn final_merge_in_checked_out_worktree(
        &self,
        target_ref: &str,
        branch_ref: &str,
        target_worktree: &Path,
    ) -> Result<(), MissionGitError> {
        self.require_clean_worktree(target_worktree, target_ref)
            .await?;
        let base_sha = self.run_git(&["rev-parse", branch_ref]).await?;
        let worktree_sha = self
            .run_git_in(target_worktree, &["rev-parse", "HEAD"])
            .await?;
        if base_sha != worktree_sha {
            return Err(MissionGitError::Refused(format!(
                "target worktree for {target_ref:?} is not at the branch head"
            )));
        }
        self.require_unmerged_mission_commits(&base_sha).await?;

        let before_tag = self.final_before_tag(target_ref);
        let before_tag_preexisted = self.tag_sha(&before_tag).await?.is_some();
        self.ensure_tag_at(&before_tag, &base_sha).await?;
        let merged_tag = self.final_merged_tag(target_ref);
        self.require_tag_absent(&merged_tag).await?;
        let supervisor_branch = self.supervisor_branch();
        let merge_msg = format!("merge Vigla mission {}", self.mission_id);
        if let Err(error) = self
            .run_git_in(
                target_worktree,
                &["merge", "--no-ff", &supervisor_branch, "-m", &merge_msg],
            )
            .await
        {
            let _ = self
                .run_git_in(target_worktree, &["merge", "--abort"])
                .await;
            if !before_tag_preexisted {
                let _ = self.run_git(&["tag", "-d", &before_tag]).await;
            }
            return Err(error);
        }

        let merged_sha = self
            .run_git_in(target_worktree, &["rev-parse", "HEAD"])
            .await?;
        if let Err(topology_error) = self
            .validate_final_merge_topology(&base_sha, &merged_sha)
            .await
        {
            self.rollback_checked_out_final_merge(
                target_worktree,
                &base_sha,
                &before_tag,
                &merged_tag,
                before_tag_preexisted,
            )
            .await
            .map_err(|reset_error| {
                MissionGitError::Refused(format!(
                    "final merge validation failed ({topology_error}); restoring target {target_ref:?} also failed ({reset_error})"
                ))
            })?;
            return Err(topology_error);
        }
        if let Err(tag_error) = self.ensure_tag_at(&merged_tag, &merged_sha).await {
            self.rollback_checked_out_final_merge(
                target_worktree,
                &base_sha,
                &before_tag,
                &merged_tag,
                before_tag_preexisted,
            )
            .await
            .map_err(|rollback_error| {
                MissionGitError::Refused(format!(
                    "creating final merge anchor failed ({tag_error}); restoring target {target_ref:?} also failed ({rollback_error})"
                ))
            })?;
            return Err(tag_error);
        }
        Ok(())
    }

    async fn rollback_checked_out_final_merge(
        &self,
        target_worktree: &Path,
        base_sha: &str,
        before_tag: &str,
        merged_tag: &str,
        before_tag_preexisted: bool,
    ) -> Result<(), MissionGitError> {
        self.run_git_in(target_worktree, &["reset", "--hard", base_sha])
            .await?;
        if self.tag_sha(merged_tag).await?.is_some() {
            self.run_git(&["tag", "-d", merged_tag]).await?;
        }
        if !before_tag_preexisted && self.tag_sha(before_tag).await?.is_some() {
            self.run_git(&["tag", "-d", before_tag]).await?;
        }
        Ok(())
    }

    /// Return whether the durable anchors prove that this mission's final
    /// merge is already present on `target_ref`. Used to make retries and
    /// startup reconciliation safe after a crash between Git and SQLite.
    pub async fn final_merge_is_applied(&self, target_ref: &str) -> Result<bool, MissionGitError> {
        let branch_ref = self.validate_target_ref(target_ref).await?;
        let before_tag = self.final_before_tag(target_ref);
        let before = self.tag_sha(&before_tag).await?;
        let merged = self.tag_sha(&self.final_merged_tag(target_ref)).await?;
        match (before, merged) {
            (None, None) => Ok(false),
            (Some(before_sha), Some(merged_sha)) => {
                self.validate_final_merge_topology(&before_sha, &merged_sha)
                    .await?;
                self.run_git(&["merge-base", "--is-ancestor", &merged_sha, &branch_ref])
                    .await
                    .map_err(|_| {
                        MissionGitError::Refused(format!(
                            "final merge anchor {merged_sha} is not an ancestor of target {target_ref:?}"
                        ))
                    })?;
                Ok(true)
            }
            (Some(before_sha), None) => {
                self.recover_before_only_final_merge(
                    target_ref,
                    &branch_ref,
                    &before_tag,
                    &before_sha,
                )
                .await
            }
            (None, Some(_)) => Err(MissionGitError::Refused(format!(
                "mission {} has only one final rollback anchor; refusing to infer disposition",
                self.mission_id
            ))),
        }
    }

    async fn recover_before_only_final_merge(
        &self,
        target_ref: &str,
        branch_ref: &str,
        before_tag: &str,
        before_sha: &str,
    ) -> Result<bool, MissionGitError> {
        let target_sha = self.run_git(&["rev-parse", branch_ref]).await?;
        if target_sha == before_sha {
            return Ok(false);
        }

        let candidate = self
            .first_parent_candidate_after(before_sha, &target_sha)
            .await?;
        let supervisor_branch = self.supervisor_branch();
        let supervisor_ref = format!("refs/heads/{supervisor_branch}");
        let supervisor_sha = self
            .run_git(&["rev-parse", "--verify", &supervisor_ref])
            .await
            .map_err(|_| {
                MissionGitError::Refused(format!(
                    "mission {} has a before-only final anchor but no supervisor branch; refusing to infer disposition",
                    self.mission_id
                ))
            })?;

        let recoverable_merge = if let Some(candidate) = candidate.as_deref() {
            match self.final_merge_parents(before_sha, candidate).await {
                Ok((_, second_parent)) if second_parent == supervisor_sha => true,
                Ok(_) | Err(MissionGitError::Refused(_)) => false,
                Err(error) => return Err(error),
            }
        } else {
            false
        };

        if let Some(candidate) = candidate.filter(|_| recoverable_merge) {
            let before_ref = format!("refs/tags/{before_tag}");
            let merged_ref = format!("refs/tags/{}", self.final_merged_tag(target_ref));
            let transaction = format!(
                "start\nverify {branch_ref} {target_sha}\nverify {before_ref} {before_sha}\nverify {supervisor_ref} {supervisor_sha}\ncreate {merged_ref} {candidate}\nprepare\ncommit\n"
            );
            self.run_git_with_stdin(&["update-ref", "--stdin"], transaction.as_bytes())
                .await?;
            return Ok(true);
        }

        // The branch moved, but not through the exact two-parent merge that
        // Vigla started from `before_sha`. Remove only the stale tag, guarded
        // by compare-and-swap checks on both refs, so a retry can anchor the
        // current target without blessing or overwriting concurrent progress.
        let before_ref = format!("refs/tags/{before_tag}");
        let merged_ref = format!("refs/tags/{}", self.final_merged_tag(target_ref));
        let transaction = format!(
            "start\nverify {branch_ref} {target_sha}\nverify {supervisor_ref} {supervisor_sha}\nverify {merged_ref}\ndelete {before_ref} {before_sha}\nprepare\ncommit\n"
        );
        self.run_git_with_stdin(&["update-ref", "--stdin"], transaction.as_bytes())
            .await?;
        Ok(false)
    }

    async fn first_parent_candidate_after(
        &self,
        before_sha: &str,
        target_sha: &str,
    ) -> Result<Option<String>, MissionGitError> {
        let range = format!("{before_sha}..{target_sha}");
        let count = self
            .run_git(&["rev-list", "--first-parent", "--count", &range])
            .await?;
        let count = count.parse::<u64>().map_err(|_| {
            MissionGitError::Io(format!(
                "git returned an invalid first-parent count for {range:?}: {count:?}"
            ))
        })?;
        if count == 0 {
            return Ok(None);
        }
        let skip = format!("--skip={}", count - 1);
        let candidate = self
            .run_git(&[
                "rev-list",
                "--first-parent",
                "--max-count=1",
                &skip,
                target_sha,
            ])
            .await?;
        Ok((!candidate.is_empty()).then_some(candidate))
    }

    async fn require_unmerged_mission_commits(
        &self,
        target_sha: &str,
    ) -> Result<(), MissionGitError> {
        let supervisor_branch = self.supervisor_branch();
        let range = format!("{target_sha}..{supervisor_branch}");
        let count = self.run_git(&["rev-list", "--count", &range]).await?;
        let count = count.parse::<u64>().map_err(|_| {
            MissionGitError::Io(format!(
                "git returned an invalid commit count for {range:?}: {count:?}"
            ))
        })?;
        if count == 0 {
            return Err(MissionGitError::Refused(format!(
                "mission {} has no mission commits that are not already in the target; refusing a no-op final merge",
                self.mission_id
            )));
        }
        Ok(())
    }

    pub(crate) async fn validate_final_merge_topology(
        &self,
        before_sha: &str,
        merged_sha: &str,
    ) -> Result<(), MissionGitError> {
        self.final_merge_parents(before_sha, merged_sha)
            .await
            .map(|_| ())
    }

    async fn final_merge_parents(
        &self,
        before_sha: &str,
        merged_sha: &str,
    ) -> Result<(String, String), MissionGitError> {
        let raw = self
            .run_git(&["rev-list", "--parents", "--max-count=1", merged_sha])
            .await?;
        let commits: Vec<&str> = raw.split_whitespace().collect();
        if commits.len() != 3 {
            return Err(MissionGitError::Refused(format!(
                "final merge anchor {merged_sha} is not a two-parent merge commit"
            )));
        }
        if commits[1] != before_sha {
            return Err(MissionGitError::Refused(format!(
                "final merge commit {merged_sha} first parent {} does not match before anchor {before_sha}",
                commits[1]
            )));
        }
        Ok((commits[1].to_owned(), commits[2].to_owned()))
    }

    async fn commit_final_refs(
        &self,
        branch_ref: &str,
        base_sha: &str,
        merged_sha: &str,
        target_ref: &str,
    ) -> Result<(), MissionGitError> {
        let before_tag = self.final_before_tag(target_ref);
        let before_ref = format!("refs/tags/{before_tag}");
        let merged_ref = format!("refs/tags/{}", self.final_merged_tag(target_ref));
        let before_operation = match self.tag_sha(&before_tag).await? {
            None => format!("create {before_ref} {base_sha}"),
            Some(existing) if existing == base_sha => {
                format!("verify {before_ref} {base_sha}")
            }
            Some(existing) => {
                return Err(MissionGitError::Refused(format!(
                    "tag {before_tag} already points to {existing}, expected {base_sha}"
                )));
            }
        };
        let (_, supervisor_sha) = self.final_merge_parents(base_sha, merged_sha).await?;
        let supervisor_ref = format!("refs/heads/{}", self.supervisor_branch());
        let transaction = format!(
            "start\nupdate {branch_ref} {merged_sha} {base_sha}\n{before_operation}\nverify {supervisor_ref} {supervisor_sha}\ncreate {merged_ref} {merged_sha}\nprepare\ncommit\n"
        );
        self.run_git_with_stdin(&["update-ref", "--stdin"], transaction.as_bytes())
            .await
            .map(|_| ())
    }

    pub(crate) async fn validate_target_ref(
        &self,
        target_ref: &str,
    ) -> Result<String, MissionGitError> {
        if target_ref.starts_with("vigla/") {
            return Err(MissionGitError::Refused(format!(
                "target {target_ref:?} is inside the vigla/ namespace"
            )));
        }
        let branch_ref = format!("refs/heads/{target_ref}");
        self.run_git(&["rev-parse", "--verify", &branch_ref])
            .await
            .map_err(|_| {
                MissionGitError::Refused(format!("target {target_ref:?} is not a local branch"))
            })?;
        Ok(branch_ref)
    }

    pub(crate) async fn checked_out_worktree(
        &self,
        branch_ref: &str,
    ) -> Result<Option<PathBuf>, MissionGitError> {
        let listing = self.run_git(&["worktree", "list", "--porcelain"]).await?;
        let mut worktree: Option<PathBuf> = None;
        for line in listing.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                worktree = Some(PathBuf::from(path));
            } else if line.strip_prefix("branch ") == Some(branch_ref) {
                return Ok(worktree);
            } else if line.is_empty() {
                worktree = None;
            }
        }
        Ok(None)
    }

    pub(crate) async fn require_clean_worktree(
        &self,
        worktree: &Path,
        target_ref: &str,
    ) -> Result<(), MissionGitError> {
        let status = self
            .run_git_in(
                worktree,
                &[
                    "status",
                    "--porcelain",
                    "--untracked-files=normal",
                    "--",
                    ".",
                    ":(exclude).vigla",
                ],
            )
            .await?;
        if status.is_empty() {
            Ok(())
        } else {
            Err(MissionGitError::Refused(format!(
                "target branch {target_ref:?} is checked out with uncommitted changes; commit or stash them before merging or reverting"
            )))
        }
    }

    pub(crate) async fn ensure_tag_at(&self, tag: &str, sha: &str) -> Result<(), MissionGitError> {
        let tag_ref = format!("refs/tags/{tag}");
        if let Ok(existing) = self
            .run_git(&["rev-parse", "--verify", "--quiet", &tag_ref])
            .await
        {
            if existing == sha {
                return Ok(());
            }
            return Err(MissionGitError::Refused(format!(
                "rollback tag {tag:?} already points at a different commit"
            )));
        }
        self.run_git(&["tag", tag, sha]).await.map(|_| ())
    }

    pub(crate) async fn tag_sha(&self, tag: &str) -> Result<Option<String>, MissionGitError> {
        let tag_ref = format!("refs/tags/{tag}^{{commit}}");
        match self
            .run_git(&["rev-parse", "--verify", "--quiet", &tag_ref])
            .await
        {
            Ok(sha) => Ok(Some(sha)),
            Err(MissionGitError::Git { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn require_tag_absent(&self, tag: &str) -> Result<(), MissionGitError> {
        let tag_ref = format!("refs/tags/{tag}");
        if self
            .run_git(&["rev-parse", "--verify", "--quiet", &tag_ref])
            .await
            .is_ok()
        {
            return Err(MissionGitError::Refused(format!(
                "rollback tag {tag:?} already exists"
            )));
        }
        Ok(())
    }

    /// Drop all of this mission's branches, worktrees, and intermediate
    /// snapshot tags. Missing artifacts are idempotent; failures are collected
    /// and returned only after every cleanup target has been attempted.
    pub async fn discard(&self) -> Result<(), MissionGitError> {
        // Use a path-segment substring rather than a full-path prefix:
        // on macOS, `git worktree list` resolves symlinks (/var ->
        // /private/var), so an absolute prefix derived from the repo
        // root may not match git's reported paths character-for-char.
        let segment = format!(".vigla/worktrees/{}/", self.mission_id);
        // `final_merge` parks a detached worktree here and removes it on
        // completion; if that removal ever fails it would leak past
        // discard, since it lives outside the worktrees/<mid> tree.
        let temp_segment = format!(".vigla/temp/final_merge/{}", self.mission_id);
        let revert_temp_segment = format!(".vigla/temp/revert/{}", self.mission_id);

        let mut errors = Vec::new();
        let mut registered_cleanup_failed = false;
        let worktree_list = self.run_git(&["worktree", "list", "--porcelain"]).await?;
        for line in worktree_list.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                // `segment` is slash-terminated and worker worktrees have a
                // `/<worker_id>` component after `<mid>`, so `contains` is a
                // safe prefix test. The temp worktree, by contrast, lives AT
                // the `<mid>` leaf (`.vigla/temp/final_merge/<mid>`) with
                // nothing after it, so a `contains(temp_segment)` would also
                // match a sibling mission whose id has this id as a prefix
                // (e.g. `m1` matching `m10`'s temp path) and yank its
                // worktree out from under a concurrent final_merge. Anchor
                // it with `ends_with` so only the exact mission matches.
                if path.contains(&segment)
                    || path.ends_with(&temp_segment)
                    || path.ends_with(&revert_temp_segment)
                {
                    if let Err(error) = self.run_git(&["worktree", "remove", "--force", path]).await
                    {
                        registered_cleanup_failed = true;
                        errors.push(format!("remove worktree {path:?}: {error}"));
                    }
                }
            }
        }
        // Sweep any leftover dirs (e.g. partial state from a crashed run,
        // or a final_merge temp worktree whose `worktree remove` failed).
        let mission_worktrees_prefix = self.mission_worktrees_dir();
        if !registered_cleanup_failed && mission_worktrees_prefix.exists() {
            if let Err(error) = tokio::fs::remove_dir_all(&mission_worktrees_prefix).await {
                errors.push(format!(
                    "remove {}: {error}",
                    mission_worktrees_prefix.display()
                ));
            }
        }
        let final_merge_temp = self
            .repo_root
            .join(".vigla/temp/final_merge")
            .join(&self.mission_id);
        if !registered_cleanup_failed && final_merge_temp.exists() {
            if let Err(error) = tokio::fs::remove_dir_all(&final_merge_temp).await {
                errors.push(format!("remove {}: {error}", final_merge_temp.display()));
            }
        }
        let revert_temp = self
            .repo_root
            .join(".vigla/temp/revert")
            .join(&self.mission_id);
        if !registered_cleanup_failed && revert_temp.exists() {
            if let Err(error) = tokio::fs::remove_dir_all(&revert_temp).await {
                errors.push(format!("remove {}: {error}", revert_temp.display()));
            }
        }

        if let Err(error) = self.run_git(&["worktree", "prune"]).await {
            errors.push(format!("prune worktree metadata: {error}"));
        }

        // 2. Delete every branch under vigla/<mid>/.
        let branch_list = self
            .run_git(&["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
            .await?;
        let branch_prefix = format!("vigla/{}/", self.mission_id);
        for branch in branch_list.lines() {
            if branch.starts_with(&branch_prefix) {
                if let Err(error) = self.run_git(&["branch", "-D", branch]).await {
                    errors.push(format!("delete branch {branch:?}: {error}"));
                }
            }
        }

        // 3. Delete every snapshot tag under vigla/snap/<mid>/ and
        //    every pre-merge tag under vigla/pre-merge/<mid>/. The
        //    pre-merge tags are minted by `create_pre_merge_tag` on each
        //    `integrate_worker` and would otherwise accumulate across
        //    missions, bloating packed-refs and keeping objects pinned
        //    against git gc. Final `vigla/revert/<mid>/...` anchors survive
        //    until retention compaction so a merged mission remains revertible.
        let tag_list = self.run_git(&["tag", "--list"]).await?;
        let snap_prefix = format!("vigla/snap/{}/", self.mission_id);
        let pre_merge_prefix = format!("vigla/pre-merge/{}/", self.mission_id);
        for tag in tag_list.lines() {
            if tag.starts_with(&snap_prefix) || tag.starts_with(&pre_merge_prefix) {
                if let Err(error) = self.run_git(&["tag", "-d", tag]).await {
                    errors.push(format!("delete tag {tag:?}: {error}"));
                }
            }
        }
        self.delete_unmatched_before_anchors(&tag_list, &mut errors)
            .await;

        let remaining_worktrees = self.run_git(&["worktree", "list", "--porcelain"]).await?;
        if remaining_worktrees.lines().any(|line| {
            line.strip_prefix("worktree ").is_some_and(|path| {
                path.contains(&segment)
                    || path.ends_with(&temp_segment)
                    || path.ends_with(&revert_temp_segment)
            })
        }) {
            errors.push("mission worktree registration remains".into());
        }
        let remaining_branches = self
            .run_git(&[
                "for-each-ref",
                "--format=%(refname:short)",
                &format!("refs/heads/{branch_prefix}"),
            ])
            .await?;
        if !remaining_branches.is_empty() {
            errors.push(format!(
                "mission branches remain: {}",
                remaining_branches.replace('\n', ", ")
            ));
        }
        let remaining_tags = self.run_git(&["tag", "--list"]).await?;
        let remaining_intermediate: Vec<_> = remaining_tags
            .lines()
            .filter(|tag| tag.starts_with(&snap_prefix) || tag.starts_with(&pre_merge_prefix))
            .collect();
        if !remaining_intermediate.is_empty() {
            errors.push(format!(
                "mission intermediate tags remain: {}",
                remaining_intermediate.join(", ")
            ));
        }

        if !errors.is_empty() {
            return Err(MissionGitError::Refused(format!(
                "mission cleanup incomplete: {}",
                errors.join("; ")
            )));
        }

        Ok(())
    }

    async fn delete_unmatched_before_anchors(&self, tag_list: &str, errors: &mut Vec<String>) {
        let before_prefix = format!("vigla/revert/{}/before/", self.mission_id);
        let tags = tag_list.lines().collect::<std::collections::HashSet<_>>();
        for before_tag in tag_list
            .lines()
            .filter(|tag| tag.starts_with(&before_prefix))
        {
            let target_ref = &before_tag[before_prefix.len()..];
            let merged_tag = self.final_merged_tag(target_ref);
            if tags.contains(merged_tag.as_str()) {
                continue;
            }

            let before_ref = format!("refs/tags/{before_tag}");
            let merged_ref = format!("refs/tags/{merged_tag}");
            let before_object = match self.run_git(&["rev-parse", "--verify", &before_ref]).await {
                Ok(object) => object,
                Err(error) => {
                    errors.push(format!("resolve unmatched tag {before_tag:?}: {error}"));
                    continue;
                }
            };
            let transaction = format!(
                "start\nverify {merged_ref}\ndelete {before_ref} {before_object}\nprepare\ncommit\n"
            );
            if let Err(error) = self
                .run_git_with_stdin(&["update-ref", "--stdin"], transaction.as_bytes())
                .await
            {
                errors.push(format!("delete unmatched tag {before_tag:?}: {error}"));
            }
        }
    }

    // -----------------------------------------------------------------
    // Internals
    // -----------------------------------------------------------------

    async fn ensure_worktrees_parent(&self) -> Result<(), MissionGitError> {
        let parent = self.mission_worktrees_dir();
        tokio::fs::create_dir_all(&parent)
            .await
            .map_err(|e| MissionGitError::Io(e.to_string()))
    }

    pub(crate) async fn run_git(&self, args: &[&str]) -> Result<String, MissionGitError> {
        self.run_git_in(&self.repo_root, args).await
    }

    pub(crate) async fn run_git_in(
        &self,
        cwd: &Path,
        args: &[&str],
    ) -> Result<String, MissionGitError> {
        Self::run_git_process_in(cwd, args).await
    }

    pub(crate) async fn run_git_process_in(
        cwd: &Path,
        args: &[&str],
    ) -> Result<String, MissionGitError> {
        let output =
            Self::run_git_command_in(cwd, args, None, GIT_COMMAND_TIMEOUT, MAX_GIT_OUTPUT_BYTES)
                .await?;
        Self::checked_git_stdout(output)
    }

    pub(crate) async fn run_git_process_bytes_in(
        cwd: &Path,
        args: &[&str],
    ) -> Result<Vec<u8>, MissionGitError> {
        let output =
            Self::run_git_command_in(cwd, args, None, GIT_COMMAND_TIMEOUT, MAX_GIT_OUTPUT_BYTES)
                .await?;
        Self::checked_git_output(output).map(|output| output.stdout)
    }

    pub(crate) fn run_git_process_bytes_sync_in(
        cwd: &Path,
        args: &[&str],
    ) -> Result<Vec<u8>, MissionGitError> {
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|error| MissionGitError::Io(error.to_string()))?
                        .block_on(Self::run_git_process_bytes_in(cwd, args))
                })
                .join()
                .map_err(|_| MissionGitError::Io("bounded git worker thread panicked".into()))?
        })
    }

    #[cfg(test)]
    async fn run_git_in_with_limits(
        &self,
        cwd: &Path,
        args: &[&str],
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<String, MissionGitError> {
        let output = Self::run_git_command_in(cwd, args, None, timeout, max_output_bytes).await?;
        Self::checked_git_stdout(output)
    }

    async fn run_git_with_stdin(
        &self,
        args: &[&str],
        input: &[u8],
    ) -> Result<String, MissionGitError> {
        let output = Self::run_git_command_in(
            &self.repo_root,
            args,
            Some(input),
            GIT_COMMAND_TIMEOUT,
            MAX_GIT_OUTPUT_BYTES,
        )
        .await?;
        Self::checked_git_stdout(output)
    }

    async fn run_git_command_in(
        cwd: &Path,
        args: &[&str],
        input: Option<&[u8]>,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<GitCommandOutput, MissionGitError> {
        let mut command = Command::new("git");
        command.args(args).current_dir(cwd);
        Self::run_command_with_limits(command, input, timeout, max_output_bytes).await
    }

    async fn run_command_with_limits(
        mut command: Command,
        input: Option<&[u8]>,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<GitCommandOutput, MissionGitError> {
        command
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::process_tree::configure(&mut command);
        let mut child = command
            .spawn()
            .map_err(|e| MissionGitError::Io(e.to_string()))?;
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                crate::process_tree::terminate_and_reap(&mut child).await;
                return Err(MissionGitError::Io("git stdout was not piped".into()));
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                crate::process_tree::terminate_and_reap(&mut child).await;
                return Err(MissionGitError::Io("git stderr was not piped".into()));
            }
        };
        let mut stdin = child.stdin.take();
        let captured_bytes = Arc::new(AtomicUsize::new(0));
        let operation = async {
            let write_input = async {
                if let Some(input) = input {
                    let mut stdin = stdin.take().ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::BrokenPipe,
                            "git stdin was not piped",
                        )
                    })?;
                    stdin.write_all(input).await?;
                    stdin.shutdown().await?;
                    drop(stdin);
                }
                Ok::<(), std::io::Error>(())
            };
            // Drain both pipes before reaping the group leader. If a hook
            // backgrounds a descendant that inherits either pipe, this join
            // deliberately remains pending until the outer timeout kills the
            // still-owned process group. Reaping Git first would make its
            // numeric PGID reusable while cleanup still intends to signal it.
            let (stdout, stderr, ()) = tokio::try_join!(
                read_bounded_git_output(
                    stdout,
                    "stdout",
                    Arc::clone(&captured_bytes),
                    max_output_bytes,
                ),
                read_bounded_git_output(
                    stderr,
                    "stderr",
                    Arc::clone(&captured_bytes),
                    max_output_bytes,
                ),
                write_input,
            )?;
            let status = child.wait().await?;
            Ok::<GitCommandOutput, std::io::Error>(GitCommandOutput {
                status,
                stdout,
                stderr,
            })
        };

        match tokio::time::timeout(timeout, operation).await {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(error)) => {
                crate::process_tree::terminate_and_reap(&mut child).await;
                Err(MissionGitError::Io(error.to_string()))
            }
            Err(_) => {
                crate::process_tree::terminate_and_reap(&mut child).await;
                Err(MissionGitError::Io(format!(
                    "git command timed out after {} ms",
                    timeout.as_millis()
                )))
            }
        }
    }

    fn checked_git_stdout(output: GitCommandOutput) -> Result<String, MissionGitError> {
        let output = Self::checked_git_output(output)?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn checked_git_output(output: GitCommandOutput) -> Result<GitCommandOutput, MissionGitError> {
        if !output.status.success() {
            return Err(MissionGitError::Git {
                code: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }
        Ok(output)
    }

    fn validate_id(kind: &str, id: &str) -> Result<(), MissionGitError> {
        if id.is_empty() {
            return Err(MissionGitError::InvalidId {
                kind: kind.into(),
                id: id.into(),
            });
        }
        if !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(MissionGitError::InvalidId {
                kind: kind.into(),
                id: id.into(),
            });
        }
        if id.starts_with('-') || id.starts_with('.') {
            return Err(MissionGitError::InvalidId {
                kind: kind.into(),
                id: id.into(),
            });
        }
        Ok(())
    }

    fn validate_worker_id(id: &str) -> Result<(), MissionGitError> {
        Self::validate_id("worker_id", id)?;
        // Reserved: the supervisor's worktree subdirectory uses this name.
        if id == "supervisor" {
            return Err(MissionGitError::InvalidId {
                kind: "worker_id".into(),
                id: id.into(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::process::Command as SyncCommand;
    use tempfile::TempDir;

    pub(crate) fn make_sandbox_repo() -> (TempDir, PathBuf) {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().to_path_buf();

        let run = |args: &[&str]| {
            let out = SyncCommand::new("git")
                .args(args)
                .current_dir(&path)
                .output()
                .expect("git command");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };

        run(&["init", "--initial-branch=main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        run(&["config", "commit.gpgsign", "false"]);
        std::fs::write(path.join("README.md"), "test\n").expect("write");
        run(&["add", "README.md"]);
        run(&["commit", "-m", "initial"]);

        (temp, path)
    }

    pub(crate) fn ws(repo_root: PathBuf, mid: &str) -> MissionWorkspace {
        MissionWorkspace::new(repo_root, mid.into()).expect("workspace")
    }

    #[cfg(unix)]
    fn install_pre_commit_hook(root: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;

        let hook = root.join(".git/hooks/pre-commit");
        let git_dir = root.join(".git").to_string_lossy().replace('\'', "'\\''");
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\nVIGLA_TEST_GIT_DIR='{git_dir}'\necho $$ > \"$VIGLA_TEST_GIT_DIR/vigla-hook.pid\"\n{body}\n"
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(hook, permissions).unwrap();
    }

    #[cfg(unix)]
    fn hook_pid(root: &Path, name: &str) -> i32 {
        std::fs::read_to_string(root.join(format!(".git/{name}")))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    #[cfg(unix)]
    async fn assert_process_exited(pid: i32) {
        let exited = tokio::time::timeout(Duration::from_secs(2), async move {
            loop {
                if unsafe { libc::kill(pid, 0) } != 0 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if exited.is_err() {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
            panic!("timed-out git hook was not terminated");
        }
    }

    #[cfg(unix)]
    async fn kill_test_hook_if_running(root: &Path) {
        let pid_file = root.join(".git/vigla-hook.pid");
        if let Ok(raw) = std::fs::read_to_string(pid_file) {
            if let Ok(pid) = raw.trim().parse() {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    /// Bootstrap a sandbox repo and create the supervisor branch off
    /// `main`. Returns `(MissionWorkspace, TempDir)`; the `TempDir`
    /// must be held by the caller until the test finishes so the
    /// repo isn't reaped mid-test.
    pub(crate) async fn bootstrap_workspace_with_supervisor_branch() -> (MissionWorkspace, TempDir)
    {
        let (temp, root) = make_sandbox_repo();
        let w = ws(root, "demo-7a3f");
        w.create_supervisor_branch("main")
            .await
            .expect("create supervisor branch");
        w.create_supervisor_worktree()
            .await
            .expect("create supervisor worktree");
        (w, temp)
    }

    #[test]
    fn new_validates_mission_id() {
        let temp = TempDir::new().unwrap();
        assert!(MissionWorkspace::new(temp.path().into(), "".into()).is_err());
        assert!(MissionWorkspace::new(temp.path().into(), "../escape".into()).is_err());
        assert!(MissionWorkspace::new(temp.path().into(), "ok-id-1234".into()).is_ok());
    }

    #[test]
    fn supervisor_branch_name_follows_spec() {
        let temp = TempDir::new().unwrap();
        let w = ws(temp.path().into(), "demo-7a3f");
        assert_eq!(w.supervisor_branch(), "vigla/demo-7a3f/supervisor");
    }

    #[test]
    fn worker_branch_name_follows_spec() {
        let temp = TempDir::new().unwrap();
        let w = ws(temp.path().into(), "demo-7a3f");
        assert_eq!(
            w.worker_branch("mock-1").unwrap(),
            "vigla/demo-7a3f/worker/mock-1"
        );
    }

    #[test]
    fn worker_branch_rejects_invalid_id() {
        let temp = TempDir::new().unwrap();
        let w = ws(temp.path().into(), "demo-7a3f");
        assert!(w.worker_branch("").is_err());
        assert!(w.worker_branch("../escape").is_err());
        assert!(w.worker_branch("has space").is_err());
        assert!(w.worker_branch("supervisor").is_err());
    }

    #[test]
    fn worker_worktree_path_under_mission_dir() {
        let temp = TempDir::new().unwrap();
        let w = ws(temp.path().into(), "demo-7a3f");
        let p = w.worker_worktree_path("mock-1").unwrap();
        assert!(
            p.ends_with(".vigla/worktrees/demo-7a3f/mock-1"),
            "got {p:?}"
        );
    }

    #[test]
    fn snapshot_tag_follows_spec() {
        let temp = TempDir::new().unwrap();
        let w = ws(temp.path().into(), "demo-7a3f");
        assert_eq!(w.snapshot_tag(0), "vigla/snap/demo-7a3f/0");
        assert_eq!(w.snapshot_tag(7), "vigla/snap/demo-7a3f/7");
    }

    #[test]
    fn pre_merge_tag_follows_spec() {
        let w = MissionWorkspace::new(std::env::temp_dir(), "demo-7a3f".into()).unwrap();
        assert_eq!(w.pre_merge_tag(0), "vigla/pre-merge/demo-7a3f/0");
        assert_eq!(w.pre_merge_tag(7), "vigla/pre-merge/demo-7a3f/7");
    }

    #[tokio::test]
    async fn create_pre_merge_tag_creates_and_is_idempotent() {
        let (w, _td) = bootstrap_workspace_with_supervisor_branch().await;
        let tag1 = w.create_pre_merge_tag(0).await.unwrap();
        let tag2 = w.create_pre_merge_tag(0).await.unwrap();
        assert_eq!(tag1, tag2);
        assert!(tag1.ends_with("/0"));
    }

    #[tokio::test]
    async fn list_pre_merge_tags_sorted_newest_first() {
        let (w, _td) = bootstrap_workspace_with_supervisor_branch().await;
        w.create_pre_merge_tag(0).await.unwrap();
        // Advance supervisor branch so tag at 1 can exist at a
        // different SHA.
        let sup = w.supervisor_worktree_path();
        tokio::fs::write(sup.join("touch.txt"), "x").await.unwrap();
        w.run_git_in(&sup, &["add", "touch.txt"]).await.unwrap();
        w.run_git_in(&sup, &["commit", "-m", "advance"])
            .await
            .unwrap();
        w.create_pre_merge_tag(1).await.unwrap();

        let tags = w.list_pre_merge_tags().await.unwrap();
        assert_eq!(tags.len(), 2);
        assert!(tags[0].ends_with("/1")); // newest first
        assert!(tags[1].ends_with("/0"));
    }

    #[tokio::test]
    async fn create_supervisor_branch_creates_ref() {
        let (_temp, root) = make_sandbox_repo();
        let w = ws(root.clone(), "demo-7a3f");

        w.create_supervisor_branch("main").await.expect("create");

        // Branch exists.
        let out = SyncCommand::new("git")
            .args(["rev-parse", "--verify", "vigla/demo-7a3f/supervisor"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(out.status.success(), "supervisor branch should exist");
    }

    #[tokio::test]
    async fn workspace_setup_hides_generated_runtime_state_from_git_status() {
        let (_temp, root) = make_sandbox_repo();
        let w = ws(root.clone(), "exclude-demo-0001");

        w.create_supervisor_branch("main").await.unwrap();
        w.create_supervisor_worktree().await.unwrap();
        tokio::fs::create_dir_all(root.join(".vigla/memory"))
            .await
            .unwrap();
        tokio::fs::write(root.join(".vigla/memory/memory.sqlite"), "runtime")
            .await
            .unwrap();

        assert!(
            w.run_git(&["status", "--porcelain", "--untracked-files=all"])
                .await
                .unwrap()
                .is_empty(),
            "generated Vigla state must not appear as untracked repository content"
        );
    }

    #[tokio::test]
    async fn runtime_git_excludes_are_idempotent_and_preserve_shareable_skills() {
        let (_temp, root) = make_sandbox_repo();
        let w = ws(root.clone(), "exclude-demo-0002");

        w.install_runtime_excludes().await.unwrap();
        w.install_runtime_excludes().await.unwrap();

        let exclude_path = root.join(".git/info/exclude");
        let contents = tokio::fs::read_to_string(exclude_path).await.unwrap();
        assert_eq!(
            contents
                .lines()
                .filter(|line| *line == "/.vigla/memory/")
                .count(),
            1
        );
        assert!(!contents.contains("/.vigla/skills/"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn git_command_timeout_returns_within_its_configured_bound() {
        let (_temp, root) = make_sandbox_repo();
        let w = ws(root.clone(), "git-timeout-0001");
        std::fs::write(root.join("blocked.txt"), "blocked\n").unwrap();
        w.run_git_in(&root, &["add", "blocked.txt"]).await.unwrap();
        install_pre_commit_hook(&root, "while :; do sleep 1; done");

        let result = tokio::time::timeout(
            Duration::from_secs(2),
            w.run_git_in_with_limits(
                &root,
                &["commit", "-m", "must time out"],
                Duration::from_millis(500),
                64 * 1024,
            ),
        )
        .await;
        if result.is_err() {
            kill_test_hook_if_running(&root).await;
            panic!("git command ignored its configured timeout");
        }
        let error = result.unwrap().unwrap_err();
        assert!(
            matches!(&error, MissionGitError::Io(message) if message.contains("timed out")),
            "got: {error}"
        );
        if root.join(".git/vigla-hook.pid").is_file() {
            assert_process_exited(hook_pid(&root, "vigla-hook.pid")).await;
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn git_command_output_limit_kills_the_blocking_hook() {
        let (_temp, root) = make_sandbox_repo();
        let w = ws(root.clone(), "git-output-limit-0001");
        std::fs::write(root.join("blocked.txt"), "blocked\n").unwrap();
        w.run_git_in(&root, &["add", "blocked.txt"]).await.unwrap();
        install_pre_commit_hook(
            &root,
            "sleep 60 &\necho $! > \"$VIGLA_TEST_GIT_DIR/vigla-hook-child.pid\"\nhead -c 4096 /dev/zero | tr '\\000' x >&2\nwait",
        );

        let result = tokio::time::timeout(
            Duration::from_secs(10),
            w.run_git_in_with_limits(
                &root,
                &["commit", "-m", "must cap output"],
                Duration::from_secs(5),
                1024,
            ),
        )
        .await;
        if result.is_err() {
            kill_test_hook_if_running(&root).await;
            panic!("git command buffered output past its configured limit");
        }
        let error = result.unwrap().unwrap_err();
        assert!(
            matches!(&error, MissionGitError::Io(message) if message.contains("exceeded 1024 bytes")),
            "got: {error}"
        );
        assert_process_exited(hook_pid(&root, "vigla-hook.pid")).await;
        assert_process_exited(hook_pid(&root, "vigla-hook-child.pid")).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_output_limit_kills_descendant_after_group_leader_exits() {
        let (_temp, root) = make_sandbox_repo();
        let child_pid_path = root.join(".git/vigla-hook-child.pid");
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg("(sleep 1; head -c 4096 /dev/zero | tr '\\000' x >&2; sleep 60) & echo $! > \"$VIGLA_TEST_CHILD_PID\"; exit 0")
            .env("VIGLA_TEST_CHILD_PID", &child_pid_path)
            .current_dir(&root);

        let error =
            MissionWorkspace::run_command_with_limits(command, None, Duration::from_secs(10), 1024)
                .await
                .unwrap_err();
        assert!(
            matches!(&error, MissionGitError::Io(message) if message.contains("exceeded 1024 bytes")),
            "got: {error}"
        );
        assert_process_exited(hook_pid(&root, "vigla-hook-child.pid")).await;
    }

    #[tokio::test]
    async fn final_merge_brings_supervisor_work_into_target() {
        // Happy path / regression: the compare-and-swap update-ref (new
        // SHA, then the worktree's base SHA as expected-old) must still
        // fast-path a normal merge — a swapped arg order would make the
        // CAS reject and this would fail.
        let (_temp, root) = make_sandbox_repo();
        let w = ws(root.clone(), "demo-7a3f");
        w.create_supervisor_branch("main").await.unwrap();
        let sup = w.create_supervisor_worktree().await.unwrap();
        tokio::fs::write(sup.join("feature.txt"), "feat")
            .await
            .unwrap();
        w.run_git_in(&sup, &["add", "feature.txt"]).await.unwrap();
        w.run_git_in(&sup, &["commit", "-m", "supervisor feature"])
            .await
            .unwrap();

        w.final_merge("main").await.expect("final_merge");

        let tree = w
            .run_git(&["ls-tree", "-r", "--name-only", "main"])
            .await
            .unwrap();
        assert!(
            tree.contains("feature.txt"),
            "target branch must contain the merged work"
        );
        let log = w.run_git(&["log", "--oneline", "main"]).await.unwrap();
        assert!(
            log.contains("merge Vigla mission"),
            "target history must include the --no-ff merge commit"
        );
    }

    #[tokio::test]
    async fn stale_compare_and_swap_update_ref_is_rejected() {
        // Locks the guarantee final_merge's CAS relies on: a 3-arg
        // `update-ref <ref> <new> <expected_old>` with a stale
        // expected-old is rejected (surfaced as Err by run_git) and
        // leaves the branch untouched — so final_merge's `?` converts a
        // concurrent target-branch move into an error, not a silent
        // clobber of the user's commit.
        let (_temp, root) = make_sandbox_repo();
        let w = ws(root.clone(), "demo-7a3f");
        let original = w.run_git(&["rev-parse", "main"]).await.unwrap();
        tokio::fs::write(root.join("x.txt"), "x").await.unwrap();
        w.run_git(&["add", "x.txt"]).await.unwrap();
        w.run_git(&["commit", "-m", "advance main"]).await.unwrap();
        let advanced = w.run_git(&["rev-parse", "main"]).await.unwrap();
        assert_ne!(original, advanced);

        let res = w
            .run_git(&["update-ref", "refs/heads/main", &original, &original])
            .await;
        assert!(res.is_err(), "stale expected-old CAS must be rejected");
        assert_eq!(
            w.run_git(&["rev-parse", "main"]).await.unwrap(),
            advanced,
            "a rejected CAS must leave the branch untouched"
        );
    }

    #[tokio::test]
    async fn create_supervisor_branch_refuses_vigla_target() {
        let (_temp, root) = make_sandbox_repo();
        let w = ws(root, "demo-7a3f");
        let err = w
            .create_supervisor_branch("vigla/other/supervisor")
            .await
            .expect_err("should refuse");
        assert!(matches!(err, MissionGitError::Refused(_)));
    }

    #[tokio::test]
    async fn create_supervisor_worktree_yields_directory() {
        let (_temp, root) = make_sandbox_repo();
        let w = ws(root.clone(), "demo-7a3f");

        w.create_supervisor_branch("main").await.unwrap();
        let p = w.create_supervisor_worktree().await.expect("worktree");

        assert!(p.exists(), "supervisor worktree directory should exist");
        assert!(p.join(".git").exists(), ".git pointer should exist");
        assert_eq!(p, w.supervisor_worktree_path());
    }

    #[tokio::test]
    async fn create_worker_branch_off_supervisor() {
        let (_temp, root) = make_sandbox_repo();
        let w = ws(root.clone(), "demo-7a3f");

        w.create_supervisor_branch("main").await.unwrap();
        w.create_worker_branch("mock-1")
            .await
            .expect("worker branch");

        let out = SyncCommand::new("git")
            .args(["rev-parse", "--verify", "vigla/demo-7a3f/worker/mock-1"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(out.status.success(), "worker branch should exist");
    }

    #[tokio::test]
    async fn create_worker_worktree_creates_directory() {
        let (_temp, root) = make_sandbox_repo();
        let w = ws(root.clone(), "demo-7a3f");

        w.create_supervisor_branch("main").await.unwrap();
        w.create_supervisor_worktree().await.unwrap();
        w.create_worker_branch("mock-1").await.unwrap();
        let p = w.create_worker_worktree("mock-1").await.expect("worktree");

        assert!(p.exists());
        assert!(
            p.join("README.md").exists(),
            "worker checkout has the seed file"
        );
    }

    #[tokio::test]
    async fn write_worker_acl_sentinel_round_trips() {
        use crate::acl::FileAcl;
        use std::path::PathBuf;
        let (_temp, root) = make_sandbox_repo();
        let w = ws(root.clone(), "demo-7a3f");

        w.create_supervisor_branch("main").await.unwrap();
        w.create_supervisor_worktree().await.unwrap();
        w.create_worker_branch("mock-1").await.unwrap();
        let wt = w.create_worker_worktree("mock-1").await.unwrap();

        let acl = FileAcl::from_mission_and_task(&[PathBuf::from("src")], None);
        w.write_worker_acl_sentinel("mock-1", &acl)
            .await
            .expect("sentinel write");

        let read = crate::acl::read_sentinel(&wt).await.unwrap();
        assert_eq!(read, acl);
    }

    #[tokio::test]
    async fn integrate_worker_merges_no_ff_and_tags() {
        let (_temp, root) = make_sandbox_repo();
        let w = ws(root.clone(), "demo-7a3f");

        w.create_supervisor_branch("main").await.unwrap();
        let sup_wt = w.create_supervisor_worktree().await.unwrap();
        w.create_worker_branch("mock-1").await.unwrap();
        let worker_wt = w.create_worker_worktree("mock-1").await.unwrap();

        // Worker does some "work": writes a file and commits.
        std::fs::write(worker_wt.join("MOCK.md"), "hello\n").unwrap();
        SyncCommand::new("git")
            .args(["add", "MOCK.md"])
            .current_dir(&worker_wt)
            .output()
            .unwrap();
        SyncCommand::new("git")
            .args(["commit", "-m", "mock work"])
            .current_dir(&worker_wt)
            .output()
            .unwrap();

        let outcome = w
            .integrate_worker("mock-1", 0, "mock task")
            .await
            .expect("integrate");
        let integration = match outcome {
            MergeOutcome::Success(i) => i,
            MergeOutcome::Conflict(c) => panic!("expected success, got conflict: {c:?}"),
        };

        assert_eq!(integration.snapshot_tag, "vigla/snap/demo-7a3f/0");
        assert_eq!(integration.integration_sha.len(), 40);
        assert!(integration.pre_merge_tag.starts_with("vigla/pre-merge/"));
        assert!(integration.snapshot_tag.starts_with("vigla/snap/"));

        // The merge commit has two parents (--no-ff).
        let parents = SyncCommand::new("git")
            .args(["rev-list", "--parents", "-n", "1", "HEAD"])
            .current_dir(&sup_wt)
            .output()
            .unwrap();
        let line = String::from_utf8_lossy(&parents.stdout);
        let parent_count = line.split_whitespace().count() - 1;
        assert_eq!(parent_count, 2, "expected --no-ff merge commit");

        // Tag points at the merge commit.
        let tag_sha = SyncCommand::new("git")
            .args(["rev-parse", "vigla/snap/demo-7a3f/0"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&tag_sha.stdout).trim(),
            integration.integration_sha
        );

        // MOCK.md is now visible from the supervisor worktree.
        assert!(sup_wt.join("MOCK.md").exists());
    }

    #[tokio::test]
    async fn integrate_worker_with_multiple_snapshots() {
        let (_temp, root) = make_sandbox_repo();
        let w = ws(root, "demo-7a3f");

        w.create_supervisor_branch("main").await.unwrap();
        let sup_wt = w.create_supervisor_worktree().await.unwrap();

        for (i, wid) in ["mock-1", "mock-2"].iter().enumerate() {
            w.create_worker_branch(wid).await.unwrap();
            let wt = w.create_worker_worktree(wid).await.unwrap();
            std::fs::write(wt.join(format!("F{i}.md")), "x\n").unwrap();
            SyncCommand::new("git")
                .args(["add", "."])
                .current_dir(&wt)
                .output()
                .unwrap();
            SyncCommand::new("git")
                .args(["commit", "-m", "work"])
                .current_dir(&wt)
                .output()
                .unwrap();

            let outcome = w
                .integrate_worker(wid, i as u32, "step")
                .await
                .expect("integrate");
            let r = match outcome {
                MergeOutcome::Success(i) => i,
                MergeOutcome::Conflict(c) => panic!("expected success, got conflict: {c:?}"),
            };
            assert_eq!(r.snapshot_tag, format!("vigla/snap/demo-7a3f/{i}"));
        }

        // Both files visible on supervisor branch.
        assert!(sup_wt.join("F0.md").exists());
        assert!(sup_wt.join("F1.md").exists());
    }

    #[tokio::test]
    async fn final_merge_advances_target_ref() {
        let (_temp, root) = make_sandbox_repo();
        let w = ws(root.clone(), "demo-7a3f");

        w.create_supervisor_branch("main").await.unwrap();
        let sup_wt = w.create_supervisor_worktree().await.unwrap();
        w.create_worker_branch("mock-1").await.unwrap();
        let worker_wt = w.create_worker_worktree("mock-1").await.unwrap();

        std::fs::write(worker_wt.join("F.md"), "x\n").unwrap();
        SyncCommand::new("git")
            .args(["add", "."])
            .current_dir(&worker_wt)
            .output()
            .unwrap();
        SyncCommand::new("git")
            .args(["commit", "-m", "w"])
            .current_dir(&worker_wt)
            .output()
            .unwrap();

        w.integrate_worker("mock-1", 0, "step").await.unwrap();

        // Capture pre-merge main HEAD.
        let pre = SyncCommand::new("git")
            .args(["rev-parse", "main"])
            .current_dir(&root)
            .output()
            .unwrap();
        let pre_sha = String::from_utf8_lossy(&pre.stdout).trim().to_string();

        w.final_merge("main").await.expect("final merge");

        // main advanced.
        let post = SyncCommand::new("git")
            .args(["rev-parse", "main"])
            .current_dir(&root)
            .output()
            .unwrap();
        let post_sha = String::from_utf8_lossy(&post.stdout).trim().to_string();
        assert_ne!(pre_sha, post_sha, "main should have advanced");

        // F.md is reachable from main.
        let ls = SyncCommand::new("git")
            .args(["cat-file", "-e", "main:F.md"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(ls.status.success(), "F.md should be reachable from main");

        // Temp worktree cleaned up.
        let tmp = root.join(".vigla/temp/final_merge/demo-7a3f");
        assert!(!tmp.exists(), "temp worktree should be removed");

        // Supervisor worktree still alive — discard() is the user's next call.
        assert!(sup_wt.exists());
    }

    #[tokio::test]
    async fn final_merge_refuses_vigla_target() {
        let (_temp, root) = make_sandbox_repo();
        let w = ws(root, "demo-7a3f");
        w.create_supervisor_branch("main").await.unwrap();
        let err = w
            .final_merge("vigla/demo-7a3f/supervisor")
            .await
            .expect_err("refused");
        assert!(matches!(err, MissionGitError::Refused(_)));
    }

    #[tokio::test]
    async fn discard_removes_branches_worktrees_and_tags() {
        let (_temp, root) = make_sandbox_repo();
        let w = ws(root.clone(), "demo-7a3f");

        w.create_supervisor_branch("main").await.unwrap();
        w.create_supervisor_worktree().await.unwrap();
        w.create_worker_branch("mock-1").await.unwrap();
        let worker_wt = w.create_worker_worktree("mock-1").await.unwrap();
        std::fs::write(worker_wt.join("F.md"), "x\n").unwrap();
        SyncCommand::new("git")
            .args(["add", "."])
            .current_dir(&worker_wt)
            .output()
            .unwrap();
        SyncCommand::new("git")
            .args(["commit", "-m", "w"])
            .current_dir(&worker_wt)
            .output()
            .unwrap();
        w.integrate_worker("mock-1", 0, "step").await.unwrap();

        w.discard().await.expect("discard");

        // No mission branches.
        let branches = SyncCommand::new("git")
            .args(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
            .current_dir(&root)
            .output()
            .unwrap();
        let listing = String::from_utf8_lossy(&branches.stdout);
        assert!(
            !listing.contains("vigla/demo-7a3f/"),
            "no mission branches should remain: {listing}"
        );

        // No mission worktrees on disk.
        assert!(!root.join(".vigla/worktrees/demo-7a3f").exists());

        // No mission tags — both snapshot and pre-merge namespaces are
        // swept (regression guard: pre-merge tags were leaking pre-fix).
        let tags = SyncCommand::new("git")
            .args(["tag", "--list"])
            .current_dir(&root)
            .output()
            .unwrap();
        let tag_listing = String::from_utf8_lossy(&tags.stdout);
        assert!(!tag_listing.contains("vigla/snap/demo-7a3f/"));
        assert!(!tag_listing.contains("vigla/pre-merge/demo-7a3f/"));
    }

    #[tokio::test]
    async fn discard_is_idempotent() {
        let (_temp, root) = make_sandbox_repo();
        let w = ws(root, "never-existed-0000");
        // Nothing was created. Discard should still succeed.
        w.discard().await.expect("idempotent discard");
    }

    #[tokio::test]
    async fn discard_removes_leaked_final_merge_temp_worktree() {
        let (_temp, root) = make_sandbox_repo();
        let w = ws(root.clone(), "demo-7a3f");

        // `final_merge` removes its temp worktree on the happy path, but
        // if `worktree remove --force` fails the worktree stays
        // registered under `.vigla/temp/final_merge/<mid>`. discard()
        // sweeps only `.vigla/worktrees/<mid>`, so it must also reach
        // the temp path. Simulate the leak directly.
        let temp = root.join(".vigla/temp/final_merge/demo-7a3f");
        std::fs::create_dir_all(temp.parent().unwrap()).unwrap();
        let out = SyncCommand::new("git")
            .args([
                "worktree",
                "add",
                "--detach",
                temp.to_str().unwrap(),
                "main",
            ])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "setup: worktree add failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(temp.exists(), "precondition: temp worktree exists");

        w.discard().await.expect("discard");

        assert!(
            !temp.exists(),
            "leaked final_merge temp worktree should be removed by discard"
        );
        let listing = SyncCommand::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(&root)
            .output()
            .unwrap();
        let listing = String::from_utf8_lossy(&listing.stdout);
        assert!(
            !listing.contains("temp/final_merge/demo-7a3f"),
            "temp worktree registration should be gone: {listing}"
        );
    }
}
