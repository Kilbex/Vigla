//! Per-repo Memory Kernel registry (A2 — Tier-2G).
//!
//! Before A2 the orchestrator opened a single global [`MemoryKernel`]
//! rooted at `<app_data>/vigla/memory/`. All repos shared one
//! note store; pinned notes leaked across projects. That's a real
//! product-correctness bug for any user with more than one project
//! and it blocks the GitHub-virality demo where the codex ships
//! alongside the repo via git.
//!
//! This module replaces that with a registry: one
//! `Arc<MemoryKernel>` per canonical repo root, each opening its own
//! SQLite file at `<repo>/.vigla/memory/memory.sqlite`. The
//! kernel's public API is unchanged — only the *who-owns-the-kernel*
//! question is answered differently.
//!
//! ## Lifecycle
//!
//! * **First touch wins.** `get_or_open(cwd)` opens the kernel
//!   lazily; subsequent calls for the same canonical root return
//!   the same `Arc` without I/O.
//! * **Repo root = nearest ancestor with `.git/`**, falling back to
//!   the canonicalized cwd when no git ancestor exists. This means a
//!   user running missions in `<repo>/subdir/` sees the same memory
//!   as `<repo>/`.
//! * **No automatic drop.** Kernels live as long as the registry.
//!   The registry lives as long as the app process. Pool count is
//!   bounded by the number of *distinct* repos the user touched this
//!   session — practically 1–5.
//! * **Concurrent opens for the same repo are serialised** by the
//!   internal mutex. Concurrent opens for *different* repos run
//!   serially through the mutex but each open is bounded.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;

use super::error::MemoryError;
use super::kernel::MemoryKernel;

#[derive(Debug, Default)]
pub struct MemoryRegistry {
    inner: Mutex<HashMap<PathBuf, Arc<MemoryKernel>>>,
}

impl MemoryRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Resolve `cwd` to its canonical repo root and return the kernel
    /// for it, opening one on first touch.
    ///
    /// On any failure (canonicalize, pool open, migration), the error
    /// is returned to the caller. The registry caches *only*
    /// successful opens — a failed open leaves the registry empty so
    /// a retry can succeed once the underlying issue is fixed.
    pub async fn get_or_open(&self, cwd: &Path) -> Result<Arc<MemoryKernel>, MemoryError> {
        let repo_root = canonical_repo_root(cwd).await?;
        // Fast path: existing kernel.
        {
            let map = self.inner.lock().await;
            if let Some(k) = map.get(&repo_root) {
                return Ok(k.clone());
            }
        }
        // Slow path: open under the lock so concurrent callers don't
        // race to create two kernels for the same repo.
        let mut map = self.inner.lock().await;
        if let Some(k) = map.get(&repo_root) {
            return Ok(k.clone());
        }
        let kernel = MemoryKernel::open_for_repo(&repo_root).await?;
        let arc = Arc::new(kernel);
        map.insert(repo_root, arc.clone());
        Ok(arc)
    }

    /// Look up an already-opened kernel by `cwd`. Returns `None` if
    /// the registry has never seen this repo — useful for read-only
    /// surfaces (the drawer) that prefer to render an empty state
    /// rather than open a kernel as a side-effect of merely peeking.
    pub async fn get(&self, cwd: &Path) -> Option<Arc<MemoryKernel>> {
        let repo_root = canonical_repo_root(cwd).await.ok()?;
        let map = self.inner.lock().await;
        map.get(&repo_root).cloned()
    }

    /// Number of distinct repos this session has opened.
    #[cfg(test)]
    pub async fn entry_count(&self) -> usize {
        self.inner.lock().await.len()
    }
}

/// Canonicalize `cwd` and walk upward looking for the nearest
/// `.git/` ancestor. Returns the absolute, symlink-resolved path of
/// the repo root, or the canonical cwd itself if no `.git/` is
/// found anywhere up to the filesystem root.
async fn canonical_repo_root(cwd: &Path) -> Result<PathBuf, MemoryError> {
    let canonical = tokio::fs::canonicalize(cwd).await?;
    let mut p: &Path = canonical.as_path();
    loop {
        if p.join(".git").exists() {
            return Ok(p.to_path_buf());
        }
        match p.parent() {
            Some(parent) if parent != p => p = parent,
            _ => return Ok(canonical),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        dir
    }

    #[tokio::test]
    async fn opens_one_kernel_per_repo() {
        let registry = MemoryRegistry::new();
        let repo_a = fresh_repo();
        let repo_b = fresh_repo();

        let k_a = registry.get_or_open(repo_a.path()).await.unwrap();
        let k_b = registry.get_or_open(repo_b.path()).await.unwrap();
        // Distinct Arcs.
        assert!(!Arc::ptr_eq(&k_a, &k_b));
        assert_eq!(registry.entry_count().await, 2);
    }

    #[tokio::test]
    async fn second_open_for_same_repo_returns_cached_arc() {
        let registry = MemoryRegistry::new();
        let repo = fresh_repo();
        let k1 = registry.get_or_open(repo.path()).await.unwrap();
        let k2 = registry.get_or_open(repo.path()).await.unwrap();
        assert!(Arc::ptr_eq(&k1, &k2));
        assert_eq!(registry.entry_count().await, 1);
    }

    #[tokio::test]
    async fn subdirectory_of_a_repo_resolves_to_the_same_kernel() {
        let registry = MemoryRegistry::new();
        let repo = fresh_repo();
        let sub = repo.path().join("src").join("inner");
        std::fs::create_dir_all(&sub).unwrap();

        let k_root = registry.get_or_open(repo.path()).await.unwrap();
        let k_sub = registry.get_or_open(&sub).await.unwrap();
        assert!(Arc::ptr_eq(&k_root, &k_sub));
        assert_eq!(registry.entry_count().await, 1);
    }

    #[tokio::test]
    async fn non_git_directory_uses_canonical_cwd_as_root() {
        let registry = MemoryRegistry::new();
        let plain = TempDir::new().unwrap();
        // No .git/ — root falls back to the dir itself.
        let _ = registry.get_or_open(plain.path()).await.unwrap();
        // Kernel files appear at .vigla/memory/ under the cwd.
        assert!(plain.path().join(".vigla/memory/memory.sqlite").exists());
    }

    #[tokio::test]
    async fn get_does_not_open() {
        let registry = MemoryRegistry::new();
        let repo = fresh_repo();
        assert!(registry.get(repo.path()).await.is_none());
        // After get_or_open, the cached arc is observable via get.
        let opened = registry.get_or_open(repo.path()).await.unwrap();
        let peeked = registry.get(repo.path()).await.unwrap();
        assert!(Arc::ptr_eq(&opened, &peeked));
    }

    #[tokio::test]
    async fn each_repo_kernel_owns_its_own_sqlite_file() {
        let registry = MemoryRegistry::new();
        let repo_a = fresh_repo();
        let repo_b = fresh_repo();
        let _ = registry.get_or_open(repo_a.path()).await.unwrap();
        let _ = registry.get_or_open(repo_b.path()).await.unwrap();
        assert!(repo_a.path().join(".vigla/memory/memory.sqlite").exists());
        assert!(repo_b.path().join(".vigla/memory/memory.sqlite").exists());
        // And they're distinct files, not symlinks.
        assert_ne!(
            std::fs::canonicalize(repo_a.path().join(".vigla/memory/memory.sqlite")).unwrap(),
            std::fs::canonicalize(repo_b.path().join(".vigla/memory/memory.sqlite")).unwrap()
        );
    }
}
