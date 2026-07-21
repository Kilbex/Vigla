//! S4 integration tests — three rebase conflict shapes the
//! arbiter is required to detect and escalate as
//! AuthorityBound::Reversibility.

use orchestrator::mission::MissionId;
use orchestrator::mission_workspace::{ConflictKind, MergeOutcome, MissionWorkspace};
use tempfile::TempDir;

/// Initialise a bare-bones git repo, create a supervisor branch
/// pointed at HEAD, and return a workspace handle + tempdir.
async fn bootstrap() -> (MissionWorkspace, TempDir) {
    let td = tempfile::tempdir().unwrap();
    let root = td.path().to_path_buf();
    // git init
    tokio::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&root)
        .status()
        .await
        .unwrap();
    tokio::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&root)
        .status()
        .await
        .unwrap();
    tokio::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&root)
        .status()
        .await
        .unwrap();
    // Initial commit so we have a parent.
    tokio::fs::write(root.join("base.txt"), "base")
        .await
        .unwrap();
    tokio::process::Command::new("git")
        .args(["add", "."])
        .current_dir(&root)
        .status()
        .await
        .unwrap();
    tokio::process::Command::new("git")
        .args(["commit", "-q", "-m", "base"])
        .current_dir(&root)
        .status()
        .await
        .unwrap();

    let mid: MissionId = "conflict-test".into();
    let w = MissionWorkspace::new(root, mid).unwrap();
    w.create_supervisor_branch("main").await.unwrap();
    w.create_supervisor_worktree().await.unwrap();
    (w, td)
}

async fn run_git(dir: &std::path::Path, args: &[&str]) {
    tokio::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .await
        .unwrap();
}

#[tokio::test]
async fn add_add_conflict_escalates() {
    let (w, _td) = bootstrap().await;
    let sup = w.supervisor_worktree_path();

    // Supervisor adds `new.txt` with "from-supervisor".
    tokio::fs::write(sup.join("new.txt"), "from-supervisor")
        .await
        .unwrap();
    run_git(&sup, &["add", "new.txt"]).await;
    run_git(&sup, &["commit", "-q", "-m", "sup adds new"]).await;

    // Worker branches off supervisor BEFORE the add, then adds the
    // same file with different content.
    w.create_worker_branch("mock-1").await.unwrap();
    let worker_wt = w.create_worker_worktree("mock-1").await.unwrap();
    // Rewind worker to before supervisor's add. (`create_worker_branch`
    // branches off supervisor HEAD which already includes the add —
    // so we manually reset.)
    run_git(&worker_wt, &["reset", "--hard", "HEAD~1"]).await;
    tokio::fs::write(worker_wt.join("new.txt"), "from-worker")
        .await
        .unwrap();
    run_git(&worker_wt, &["add", "new.txt"]).await;
    run_git(&worker_wt, &["commit", "-q", "-m", "worker adds new"]).await;

    let outcome = w.integrate_worker("mock-1", 0, "test").await.unwrap();
    let report = match outcome {
        MergeOutcome::Conflict(r) => r,
        MergeOutcome::Success(_) => panic!("expected conflict, got success"),
    };
    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(report.conflicts[0].path, "new.txt");
    assert_eq!(report.conflicts[0].kind, ConflictKind::AddAdd);
}

#[tokio::test]
async fn edit_edit_conflict_escalates() {
    let (w, _td) = bootstrap().await;
    let sup = w.supervisor_worktree_path();

    // Both sides start with base.txt = "base"; they edit it differently.
    tokio::fs::write(sup.join("base.txt"), "supervisor-edit")
        .await
        .unwrap();
    run_git(&sup, &["add", "base.txt"]).await;
    run_git(&sup, &["commit", "-q", "-m", "sup edits base"]).await;

    w.create_worker_branch("mock-1").await.unwrap();
    let worker_wt = w.create_worker_worktree("mock-1").await.unwrap();
    // Reset to before supervisor's edit.
    run_git(&worker_wt, &["reset", "--hard", "HEAD~1"]).await;
    tokio::fs::write(worker_wt.join("base.txt"), "worker-edit")
        .await
        .unwrap();
    run_git(&worker_wt, &["add", "base.txt"]).await;
    run_git(&worker_wt, &["commit", "-q", "-m", "worker edits base"]).await;

    let outcome = w.integrate_worker("mock-1", 0, "test").await.unwrap();
    let report = match outcome {
        MergeOutcome::Conflict(r) => r,
        MergeOutcome::Success(_) => panic!("expected conflict"),
    };
    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(report.conflicts[0].kind, ConflictKind::EditEdit);
}

#[tokio::test]
async fn delete_edit_conflict_escalates() {
    let (w, _td) = bootstrap().await;
    let sup = w.supervisor_worktree_path();

    // Supervisor deletes base.txt.
    run_git(&sup, &["rm", "base.txt"]).await;
    run_git(&sup, &["commit", "-q", "-m", "sup deletes base"]).await;

    w.create_worker_branch("mock-1").await.unwrap();
    let worker_wt = w.create_worker_worktree("mock-1").await.unwrap();
    run_git(&worker_wt, &["reset", "--hard", "HEAD~1"]).await;
    tokio::fs::write(worker_wt.join("base.txt"), "worker-edit")
        .await
        .unwrap();
    run_git(&worker_wt, &["add", "base.txt"]).await;
    run_git(&worker_wt, &["commit", "-q", "-m", "worker edits base"]).await;

    let outcome = w.integrate_worker("mock-1", 0, "test").await.unwrap();
    let report = match outcome {
        MergeOutcome::Conflict(r) => r,
        MergeOutcome::Success(_) => panic!("expected conflict"),
    };
    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(report.conflicts[0].kind, ConflictKind::DeleteEdit);
}

#[tokio::test]
async fn clean_rebase_succeeds() {
    let (w, _td) = bootstrap().await;
    let sup = w.supervisor_worktree_path();

    // Supervisor edits one file.
    tokio::fs::write(sup.join("sup_only.txt"), "x")
        .await
        .unwrap();
    run_git(&sup, &["add", "."]).await;
    run_git(&sup, &["commit", "-q", "-m", "sup unrelated"]).await;

    // Worker edits a different file off the same base.
    w.create_worker_branch("mock-1").await.unwrap();
    let worker_wt = w.create_worker_worktree("mock-1").await.unwrap();
    run_git(&worker_wt, &["reset", "--hard", "HEAD~1"]).await;
    tokio::fs::write(worker_wt.join("worker_only.txt"), "y")
        .await
        .unwrap();
    run_git(&worker_wt, &["add", "."]).await;
    run_git(&worker_wt, &["commit", "-q", "-m", "worker unrelated"]).await;

    let outcome = w.integrate_worker("mock-1", 0, "test").await.unwrap();
    match outcome {
        MergeOutcome::Success(i) => {
            assert!(i.pre_merge_tag.starts_with("vigla/pre-merge/"));
            assert!(i.snapshot_tag.starts_with("vigla/snap/"));
            assert!(!i.integration_sha.is_empty());
        }
        MergeOutcome::Conflict(c) => panic!("expected success, got conflict: {c:?}"),
    }
}
