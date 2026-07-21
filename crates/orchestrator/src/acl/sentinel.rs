//! ACL sentinel — a `.vigla/acl.json` file written into the
//! worker's worktree root at spawn time so audit replay and debug
//! tooling can re-derive the worker's effective ACL.
//!
//! The sentinel is purely informational: the mission loop holds
//! the live `FileAcl` in memory and uses that for the actual gate.
//! Missing sentinel is not an error — it implies "no ACL declared"
//! (returns an unconstrained ACL).

use crate::acl::FileAcl;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use tokio::fs;

const SENTINEL_DIR: &str = ".vigla";
const SENTINEL_NAME: &str = "acl.json";

/// Write the worker's effective ACL into the worktree's sentinel
/// file. Creates `.vigla/` if needed. Idempotent — repeated
/// writes are safe.
///
/// Also drops a `.gitignore` next to the sentinel containing `*`,
/// which excludes every entry in `.vigla/` (including the
/// `.gitignore` itself) from `git add`. Without this, a worker
/// that runs `git add -A` would sweep `acl.json` into the
/// integration commit and contaminate the supervisor branch.
pub async fn write_sentinel(worktree_root: &Path, acl: &FileAcl) -> std::io::Result<()> {
    let dir = worktree_root.join(SENTINEL_DIR);
    fs::create_dir_all(&dir).await?;
    fs::write(dir.join(".gitignore"), "*\n").await?;
    let path = dir.join(SENTINEL_NAME);
    let json = serde_json::to_string_pretty(acl)
        .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
    fs::write(&path, json).await
}

/// Read the worker's effective ACL from the sentinel. Returns the
/// stored ACL on success; if the file is missing returns an
/// unconstrained ACL (the safe default for replay tooling).
pub async fn read_sentinel(worktree_root: &Path) -> std::io::Result<FileAcl> {
    let path: PathBuf = worktree_root.join(SENTINEL_DIR).join(SENTINEL_NAME);
    let bytes = match fs::read(&path).await {
        Ok(b) => b,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Ok(FileAcl::from_mission_and_task(&[], None));
        }
        Err(e) => return Err(e),
    };
    serde_json::from_slice(&bytes).map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acl::FileAcl;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn pb(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[tokio::test]
    async fn write_then_read_round_trips() {
        let dir = TempDir::new().unwrap();
        let acl = FileAcl::from_mission_and_task(&[pb("src"), pb("tests")], Some(&[pb("src")]));
        write_sentinel(dir.path(), &acl).await.unwrap();
        let back = read_sentinel(dir.path()).await.unwrap();
        assert_eq!(back, acl);
    }

    #[tokio::test]
    async fn read_missing_sentinel_returns_unconstrained() {
        let dir = TempDir::new().unwrap();
        let back = read_sentinel(dir.path()).await.unwrap();
        assert!(back.is_unconstrained());
    }

    #[tokio::test]
    async fn write_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let acl = FileAcl::from_mission_and_task(&[pb("src")], None);
        write_sentinel(dir.path(), &acl).await.unwrap();
        write_sentinel(dir.path(), &acl).await.unwrap();
        let back = read_sentinel(dir.path()).await.unwrap();
        assert_eq!(back, acl);
    }
}
