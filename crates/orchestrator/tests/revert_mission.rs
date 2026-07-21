//! S4 integration test for the revert_mission path. Drives a
//! supervisor workspace through one integration + revert and
//! asserts the supervisor branch returns to its pre-merge SHA.

use orchestrator::mission::MissionId;
use orchestrator::mission_workspace::{MergeOutcome, MissionWorkspace};
use tempfile::TempDir;

async fn run_git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = tokio::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .await
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

async fn bootstrap_with_diverged_worker() -> (MissionWorkspace, TempDir) {
    let td = tempfile::tempdir().unwrap();
    let root = td.path().to_path_buf();

    let _ = run_git(&root, &["init", "-q", "-b", "main"]).await;
    let _ = run_git(&root, &["config", "user.email", "test@example.com"]).await;
    let _ = run_git(&root, &["config", "user.name", "Test"]).await;
    tokio::fs::write(root.join("base.txt"), "base")
        .await
        .unwrap();
    let _ = run_git(&root, &["add", "."]).await;
    let _ = run_git(&root, &["commit", "-q", "-m", "base"]).await;

    let mid: MissionId = "revert-test".into();
    let w = MissionWorkspace::new(root.clone(), mid).unwrap();
    w.create_supervisor_branch("main").await.unwrap();
    w.create_supervisor_worktree().await.unwrap();

    // Worker adds a new file (clean rebase target).
    w.create_worker_branch("mock-1").await.unwrap();
    let worker_wt = w.create_worker_worktree("mock-1").await.unwrap();
    tokio::fs::write(worker_wt.join("worker.txt"), "y")
        .await
        .unwrap();
    let _ = run_git(&worker_wt, &["add", "."]).await;
    let _ = run_git(&worker_wt, &["commit", "-q", "-m", "worker add"]).await;

    (w, td)
}

#[tokio::test]
async fn integrate_then_revert_restores_supervisor() {
    let (w, _td) = bootstrap_with_diverged_worker().await;
    let supervisor_branch = w.supervisor_branch();

    let baseline = run_git(w.repo_root(), &["rev-parse", &supervisor_branch]).await;

    // Integrate (should succeed cleanly).
    let outcome = w
        .integrate_worker("mock-1", 0, "feat: add worker.txt")
        .await
        .unwrap();
    let int = match outcome {
        MergeOutcome::Success(i) => i,
        MergeOutcome::Conflict(c) => panic!("expected success: {c:?}"),
    };
    assert!(int.pre_merge_tag.ends_with("/0"));

    let after_integrate = run_git(w.repo_root(), &["rev-parse", &supervisor_branch]).await;
    assert_ne!(
        after_integrate, baseline,
        "supervisor advanced after integration"
    );

    // Revert.
    let revert = w.revert_mission().await.unwrap();
    assert_eq!(revert.restored_sha, baseline);
    assert_eq!(revert.pre_merge_tag, int.pre_merge_tag);

    let after_revert = run_git(w.repo_root(), &["rev-parse", &supervisor_branch]).await;
    assert_eq!(after_revert, baseline);
}

#[tokio::test]
async fn revert_with_no_integration_refuses() {
    let (w, _td) = bootstrap_with_diverged_worker().await;
    let err = w.revert_mission().await.unwrap_err();
    assert!(err.to_string().contains("no pre-merge"));
}

#[tokio::test]
async fn final_merge_refuses_a_noop_without_creating_rollback_anchors() {
    let (w, _td) = bootstrap_with_diverged_worker().await;
    let before = run_git(w.repo_root(), &["rev-parse", "main"]).await;

    let error = w.final_merge("main").await.unwrap_err().to_string();

    assert!(error.contains("no mission commits"), "got: {error}");
    assert_eq!(run_git(w.repo_root(), &["rev-parse", "main"]).await, before);
    assert!(
        run_git(
            w.repo_root(),
            &[
                "rev-parse",
                "--verify",
                &format!("refs/tags/{}", w.final_before_tag("main")),
            ],
        )
        .await
        .is_empty(),
        "a refused no-op merge must not create a before anchor"
    );
    assert!(
        run_git(
            w.repo_root(),
            &[
                "rev-parse",
                "--verify",
                &format!("refs/tags/{}", w.final_merged_tag("main")),
            ],
        )
        .await
        .is_empty(),
        "a refused no-op merge must not create a merged anchor"
    );
}

#[tokio::test]
async fn final_merge_refuses_a_noop_on_a_non_checked_out_target() {
    let (w, _td) = bootstrap_with_diverged_worker().await;
    let _ = run_git(w.repo_root(), &["branch", "release", "main"]).await;
    let before = run_git(w.repo_root(), &["rev-parse", "release"]).await;

    let error = w.final_merge("release").await.unwrap_err().to_string();

    assert!(error.contains("no mission commits"), "got: {error}");
    assert_eq!(
        run_git(w.repo_root(), &["rev-parse", "release"]).await,
        before
    );
    assert!(run_git(
        w.repo_root(),
        &[
            "rev-parse",
            "--verify",
            &format!("refs/tags/{}", w.final_before_tag("release")),
        ],
    )
    .await
    .is_empty());
    assert!(run_git(
        w.repo_root(),
        &[
            "rev-parse",
            "--verify",
            &format!("refs/tags/{}", w.final_merged_tag("release")),
        ],
    )
    .await
    .is_empty());
}

#[tokio::test]
async fn merged_mission_revert_refuses_non_merge_anchor_without_touching_target() {
    let (w, _td) = bootstrap_with_diverged_worker().await;
    let before = run_git(w.repo_root(), &["rev-parse", "main"]).await;
    let _ = run_git(
        w.repo_root(),
        &["tag", &w.final_before_tag("main"), &before],
    )
    .await;
    let _ = run_git(
        w.repo_root(),
        &["tag", &w.final_merged_tag("main"), &before],
    )
    .await;

    let error = w
        .revert_merged_mission("main")
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("merge commit"), "got: {error}");
    assert_eq!(run_git(w.repo_root(), &["rev-parse", "main"]).await, before);
    assert_eq!(
        run_git(w.repo_root(), &["show", "main:base.txt"]).await,
        "base",
        "a forged rollback anchor must not undo target content"
    );
}

#[tokio::test]
async fn merged_mission_revert_refuses_mismatched_first_parent() {
    let (w, _td) = bootstrap_with_diverged_worker().await;
    let outcome = w
        .integrate_worker("mock-1", 0, "feat: add worker.txt")
        .await
        .unwrap();
    assert!(matches!(outcome, MergeOutcome::Success(_)));
    w.final_merge("main").await.unwrap();

    let merged = run_git(w.repo_root(), &["rev-parse", "main"]).await;
    let second_parent = run_git(w.repo_root(), &["rev-parse", "main^2"]).await;
    let _ = run_git(
        w.repo_root(),
        &[
            "tag",
            "--force",
            &w.final_before_tag("main"),
            &second_parent,
        ],
    )
    .await;

    let error = w
        .revert_merged_mission("main")
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("first parent"), "got: {error}");
    assert_eq!(run_git(w.repo_root(), &["rev-parse", "main"]).await, merged);
    assert_eq!(
        run_git(w.repo_root(), &["show", "main:worker.txt"]).await,
        "y"
    );
}

#[tokio::test]
async fn merged_mission_revert_undoes_target_after_workspace_cleanup() {
    let (w, _td) = bootstrap_with_diverged_worker().await;

    let outcome = w
        .integrate_worker("mock-1", 0, "feat: add worker.txt")
        .await
        .unwrap();
    assert!(matches!(outcome, MergeOutcome::Success(_)));

    w.final_merge("main").await.unwrap();
    tokio::fs::write(w.repo_root().join("later.txt"), "keep")
        .await
        .unwrap();
    let _ = run_git(w.repo_root(), &["add", "later.txt"]).await;
    let _ = run_git(w.repo_root(), &["commit", "-q", "-m", "later work"]).await;
    w.discard().await.unwrap();
    assert_eq!(
        run_git(w.repo_root(), &["show", "main:worker.txt"]).await,
        "y"
    );

    w.revert_mission().await.unwrap();

    let missing = tokio::process::Command::new("git")
        .args(["cat-file", "-e", "main:worker.txt"])
        .current_dir(w.repo_root())
        .output()
        .await
        .unwrap();
    assert!(
        !missing.status.success(),
        "reverting a merged mission must remove its change from the target branch"
    );
    assert_eq!(
        run_git(w.repo_root(), &["show", "main:later.txt"]).await,
        "keep",
        "a mission revert must preserve commits made after the mission merge"
    );
    assert!(
        run_git(w.repo_root(), &["status", "--porcelain"])
            .await
            .is_empty(),
        "target checkout must remain coherent after merge and revert"
    );
}

#[tokio::test]
async fn merged_mission_revert_works_when_target_is_not_checked_out() {
    let (w, _td) = bootstrap_with_diverged_worker().await;
    let _ = run_git(w.repo_root(), &["branch", "release", "main"]).await;

    let outcome = w
        .integrate_worker("mock-1", 0, "feat: add worker.txt")
        .await
        .unwrap();
    assert!(matches!(outcome, MergeOutcome::Success(_)));

    w.final_merge("release").await.unwrap();
    w.discard().await.unwrap();
    assert_eq!(
        run_git(w.repo_root(), &["show", "release:worker.txt"]).await,
        "y"
    );

    w.revert_mission().await.unwrap();

    let missing = tokio::process::Command::new("git")
        .args(["cat-file", "-e", "release:worker.txt"])
        .current_dir(w.repo_root())
        .output()
        .await
        .unwrap();
    assert!(!missing.status.success());
    assert_eq!(
        run_git(w.repo_root(), &["symbolic-ref", "--short", "HEAD"]).await,
        "main",
        "detached rollback must not switch the user's checkout"
    );
}

#[tokio::test]
async fn final_merge_refuses_a_dirty_checked_out_target() {
    let (w, _td) = bootstrap_with_diverged_worker().await;
    let outcome = w
        .integrate_worker("mock-1", 0, "feat: add worker.txt")
        .await
        .unwrap();
    assert!(matches!(outcome, MergeOutcome::Success(_)));
    let before = run_git(w.repo_root(), &["rev-parse", "main"]).await;
    tokio::fs::write(w.repo_root().join("base.txt"), "unsaved")
        .await
        .unwrap();

    let error = w.final_merge("main").await.unwrap_err().to_string();

    assert!(error.contains("uncommitted changes"), "got: {error}");
    assert_eq!(run_git(w.repo_root(), &["rev-parse", "main"]).await, before);
    assert_eq!(
        tokio::fs::read_to_string(w.repo_root().join("base.txt"))
            .await
            .unwrap(),
        "unsaved"
    );
}

#[tokio::test]
async fn final_merge_refuses_conflicting_rollback_anchor_before_advancing_target() {
    let (w, _td) = bootstrap_with_diverged_worker().await;
    let outcome = w
        .integrate_worker("mock-1", 0, "feat: add worker.txt")
        .await
        .unwrap();
    assert!(matches!(outcome, MergeOutcome::Success(_)));

    let before = run_git(w.repo_root(), &["rev-parse", "main"]).await;
    let conflicting = w.final_merged_tag("main");
    let _ = run_git(w.repo_root(), &["tag", &conflicting, &before]).await;

    let error = w.final_merge("main").await.unwrap_err().to_string();

    assert!(error.contains("rollback tag"), "got: {error}");
    assert_eq!(
        run_git(w.repo_root(), &["rev-parse", "main"]).await,
        before,
        "a conflicting final anchor must be rejected before the checked-out target moves"
    );
    let worker_file = tokio::process::Command::new("git")
        .args(["cat-file", "-e", "main:worker.txt"])
        .current_dir(w.repo_root())
        .output()
        .await
        .unwrap();
    assert!(!worker_file.status.success());
}

#[tokio::test]
async fn checked_out_final_merge_rolls_back_when_merged_anchor_cannot_be_written() {
    let (w, _td) = bootstrap_with_diverged_worker().await;
    assert!(matches!(
        w.integrate_worker("mock-1", 0, "feat: add worker.txt")
            .await
            .unwrap(),
        MergeOutcome::Success(_)
    ));
    let before = run_git(w.repo_root(), &["rev-parse", "main"]).await;

    let lock = w
        .repo_root()
        .join(".git/refs/tags")
        .join(format!("{}.lock", w.final_merged_tag("main")));
    tokio::fs::create_dir_all(lock.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&lock, "force tag transaction failure")
        .await
        .unwrap();

    let error = w.final_merge("main").await.unwrap_err().to_string();
    tokio::fs::remove_file(lock).await.unwrap();

    assert!(
        error.contains("cannot lock ref") || error.contains("File exists"),
        "got: {error}"
    );
    assert_eq!(run_git(w.repo_root(), &["rev-parse", "main"]).await, before);
    assert!(run_git(
        w.repo_root(),
        &[
            "rev-parse",
            "--verify",
            &format!("refs/tags/{}", w.final_before_tag("main")),
        ],
    )
    .await
    .is_empty());
    assert!(run_git(
        w.repo_root(),
        &[
            "rev-parse",
            "--verify",
            &format!("refs/tags/{}", w.final_merged_tag("main")),
        ],
    )
    .await
    .is_empty());
}

#[tokio::test]
async fn merged_revert_retry_reuses_durable_git_proof() {
    let (w, _td) = bootstrap_with_diverged_worker().await;
    assert!(matches!(
        w.integrate_worker("mock-1", 0, "feat: add worker.txt")
            .await
            .unwrap(),
        MergeOutcome::Success(_)
    ));
    w.final_merge("main").await.unwrap();

    let first = w.revert_merged_mission("main").await.unwrap();
    let target_after_first = run_git(w.repo_root(), &["rev-parse", "main"]).await;
    let second = w.revert_merged_mission("main").await.unwrap();

    assert_eq!(second, first);
    assert_eq!(
        run_git(w.repo_root(), &["rev-parse", "main"]).await,
        target_after_first,
        "retry after a DB-write crash must not create a second revert commit"
    );
    assert_eq!(
        run_git(
            w.repo_root(),
            &[
                "rev-parse",
                &format!("refs/tags/{}", w.final_reverted_tag("main"))
            ],
        )
        .await,
        target_after_first
    );
}

#[tokio::test]
async fn integration_rolls_back_branches_when_snapshot_tag_fails() {
    let (w, _td) = bootstrap_with_diverged_worker().await;
    let supervisor_before = run_git(w.repo_root(), &["rev-parse", &w.supervisor_branch()]).await;
    let worker_branch = w.worker_branch("mock-1").unwrap();
    let worker_before = run_git(w.repo_root(), &["rev-parse", &worker_branch]).await;
    let snapshot_tag = w.snapshot_tag(0);
    let lock = w
        .repo_root()
        .join(".git/refs/tags")
        .join(format!("{snapshot_tag}.lock"));
    tokio::fs::create_dir_all(lock.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&lock, "force snapshot failure")
        .await
        .unwrap();

    let error = w
        .integrate_worker("mock-1", 0, "feat: add worker.txt")
        .await
        .unwrap_err()
        .to_string();
    tokio::fs::remove_file(lock).await.unwrap();

    assert!(
        error.contains("cannot lock ref") || error.contains("File exists"),
        "got: {error}"
    );
    assert_eq!(
        run_git(w.repo_root(), &["rev-parse", &w.supervisor_branch()]).await,
        supervisor_before
    );
    assert_eq!(
        run_git(w.repo_root(), &["rev-parse", &worker_branch]).await,
        worker_before
    );
    assert!(run_git(
        w.repo_root(),
        &[
            "rev-parse",
            "--verify",
            &format!("refs/tags/{}", w.pre_merge_tag(0))
        ],
    )
    .await
    .is_empty());
}
