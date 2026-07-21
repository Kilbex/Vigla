//! Anchor-block I/O and drift detection (V3 §3, §7).
//!
//! The kernel owns one contiguous region inside the worker's native
//! memory file. Everything outside the anchors is treated as user
//! content and preserved byte-exact across writes.
//!
//! Two operations:
//!
//!   1. [`write_anchor_block`] — replace (or insert) the kernel-owned
//!      region with new body bytes. Atomic via same-dir tmp + rename.
//!
//!   2. [`detect_drift`] — read the file at the start of the worker's
//!      next turn; compare the anchor-block body hash against the
//!      hash recorded at write time. Mismatch ⇒ `Drift`; missing
//!      anchors ⇒ `AnchorMissing`; missing file ⇒ `FileMissing`.
//!
//! No part of this module knows about codex or composer types. It is
//! a pure file-level primitive — easy to test in isolation.

use std::path::{Path, PathBuf};

use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::error::MemoryError;
use super::store::hash_hex;

/// Span of the kernel-owned content inside a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnchorSpan {
    /// Byte offset of the `open` delimiter's first character.
    pub open_offset: u64,
    /// Byte offset immediately after the `close` delimiter's last
    /// character.
    pub close_end_offset: u64,
    /// Byte offset of the first byte of the *body* (i.e. just after
    /// the `open` delimiter + its trailing newline if present).
    pub body_start: u64,
    /// Byte offset just past the last byte of the body (i.e. just
    /// before the `close` delimiter).
    pub body_end: u64,
}

/// Locate the anchor span in `text`. Returns `None` if either delimiter
/// is absent or if `close` appears before `open`. Tolerant of an
/// optional newline immediately after `open` and before `close` — the
/// renderer always writes one, but human edits may not.
pub fn find_anchor_span(text: &str, open: &str, close: &str) -> Option<AnchorSpan> {
    let open_idx = text.find(open)?;
    let after_open = open_idx + open.len();
    let close_rel = text[after_open..].find(close)?;
    let close_idx = after_open + close_rel;

    // Body excludes the delimiters themselves. Strip a single leading
    // newline (from the renderer's `<open>\n<body>`) and a single
    // trailing newline (from `<body>\n<close>`) so the body's own
    // bytes are what we hash.
    let mut body_start = after_open;
    if text.as_bytes().get(body_start) == Some(&b'\n') {
        body_start += 1;
    }
    let mut body_end = close_idx;
    if body_end > body_start && text.as_bytes()[body_end - 1] == b'\n' {
        body_end -= 1;
    }

    Some(AnchorSpan {
        open_offset: open_idx as u64,
        close_end_offset: (close_idx + close.len()) as u64,
        body_start: body_start as u64,
        body_end: body_end as u64,
    })
}

/// Outcome of a successful anchor-block write. Caller archives these
/// fields in `memory_bundles` so drift detection on the next turn has
/// a ground truth.
#[derive(Debug, Clone)]
pub struct AnchorWriteOutcome {
    pub anchor_open_offset: u64,
    pub anchor_close_offset: u64,
    pub body_hash: String,
    pub file_hash: String,
    pub written_file_size: u64,
}

/// Replace (or insert) the anchor block in `file` with `body`. If the
/// file doesn't exist, it's created with only the block. If the file
/// exists but has no anchors, the block is appended (with a leading
/// blank line to separate from existing content).
pub async fn write_anchor_block(
    file: &Path,
    open: &str,
    close: &str,
    body: &str,
) -> Result<AnchorWriteOutcome, MemoryError> {
    let existing = match fs::read_to_string(file).await {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(MemoryError::Io(e)),
    };

    let new_contents = compose_file_contents(existing.as_deref(), open, close, body);
    // Hash the canonical anchor-body form (see composer rationale).
    // `compose_file_contents` writes `body.trim_end()` between the
    // delimiters; we hash that same byte sequence so a read-back
    // through `detect_drift` produces the same digest.
    let body_hash = hash_hex(body.trim_end_matches('\n').as_bytes());
    let file_hash = hash_hex(new_contents.as_bytes());

    write_atomic_string(file, &new_contents).await?;

    // After writing, the anchor must be locatable in the new contents.
    let span = find_anchor_span(&new_contents, open, close)
        .ok_or_else(|| MemoryError::RowCorrupt("anchor not locatable after write".into()))?;

    Ok(AnchorWriteOutcome {
        anchor_open_offset: span.open_offset,
        anchor_close_offset: span.close_end_offset,
        body_hash,
        file_hash,
        written_file_size: new_contents.len() as u64,
    })
}

/// Compose the new file contents from optional `existing` text, the
/// anchor delimiters, and the new `body`. Pure function; tests poke
/// this directly.
pub fn compose_file_contents(
    existing: Option<&str>,
    open: &str,
    close: &str,
    body: &str,
) -> String {
    let block = format!(
        "{open}\n{body_clean}\n{close}\n",
        body_clean = body.trim_end()
    );

    match existing {
        // No prior file → just the block.
        None | Some("") => block,
        Some(prior) => {
            if let Some(span) = find_anchor_span(prior, open, close) {
                // Replace existing block.
                let head = &prior[..span.open_offset as usize];
                let tail = &prior[span.close_end_offset as usize..];
                // Preserve user content exactly; only normalize separator
                // between head and our block so we don't accumulate
                // blank lines on every dispatch.
                let head_trimmed = head.trim_end_matches('\n');
                let mut out = String::with_capacity(prior.len() + body.len());
                out.push_str(head_trimmed);
                if !head_trimmed.is_empty() {
                    out.push_str("\n\n");
                }
                out.push_str(&block);
                let tail_leading_trim = tail.trim_start_matches('\n');
                if !tail_leading_trim.is_empty() {
                    out.push('\n');
                    out.push_str(tail_leading_trim);
                }
                out
            } else {
                // No anchors yet — append, separated by a blank line.
                let mut out = String::with_capacity(prior.len() + body.len() + 64);
                out.push_str(prior.trim_end_matches('\n'));
                if !prior.trim().is_empty() {
                    out.push_str("\n\n");
                }
                out.push_str(&block);
                out
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftStatus {
    /// File present, anchors found, body hash matches the recorded
    /// hash. No drift.
    Match,
    /// File present, anchors found, but the body bytes have changed.
    Drift {
        expected_hash: String,
        observed_hash: String,
        anchor_open_offset: u64,
    },
    /// File present but the anchors are gone (user deleted or
    /// corrupted them).
    AnchorMissing,
    /// File doesn't exist (worker deleted it, or the worktree was
    /// never seeded).
    FileMissing,
}

/// Read `file`, find the anchor block, hash the body, compare to the
/// expected hash. Pure I/O + comparison.
pub async fn detect_drift(
    file: &Path,
    open: &str,
    close: &str,
    expected_body_hash: &str,
) -> Result<DriftStatus, MemoryError> {
    let contents = match fs::read_to_string(file).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(DriftStatus::FileMissing),
        Err(e) => return Err(MemoryError::Io(e)),
    };
    let span = match find_anchor_span(&contents, open, close) {
        Some(s) => s,
        None => return Ok(DriftStatus::AnchorMissing),
    };
    let body = &contents[span.body_start as usize..span.body_end as usize];
    let observed = hash_hex(body.as_bytes());
    if observed == expected_body_hash {
        Ok(DriftStatus::Match)
    } else {
        Ok(DriftStatus::Drift {
            expected_hash: expected_body_hash.to_owned(),
            observed_hash: observed,
            anchor_open_offset: span.open_offset,
        })
    }
}

// ---------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------

async fn write_atomic_string(dest: &Path, s: &str) -> Result<(), MemoryError> {
    let parent: PathBuf = dest
        .parent()
        .map(|p| p.to_owned())
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("."));
    fs::create_dir_all(&parent).await?;

    let tmp_name = format!(
        ".{}.tmp.{}",
        dest.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("memory"),
        uuid::Uuid::now_v7().simple()
    );
    let tmp_path = parent.join(tmp_name);

    let mut f = fs::File::create(&tmp_path).await?;
    f.write_all(s.as_bytes()).await?;
    f.flush().await?;
    f.sync_all().await?;
    drop(f);

    match fs::rename(&tmp_path, dest).await {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp_path).await;
            Err(MemoryError::Io(e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const OPEN: &str = "<!-- vigla:memory:begin v1 -->";
    const CLOSE: &str = "<!-- vigla:memory:end -->";

    #[test]
    fn find_anchor_returns_none_when_missing() {
        assert!(find_anchor_span("no anchors here", OPEN, CLOSE).is_none());
    }

    #[test]
    fn find_anchor_strips_newlines_around_body() {
        let text = format!("preface\n\n{OPEN}\nbody body\n{CLOSE}\n");
        let span = find_anchor_span(&text, OPEN, CLOSE).unwrap();
        let body = &text[span.body_start as usize..span.body_end as usize];
        assert_eq!(body, "body body");
    }

    #[test]
    fn compose_creates_new_file_when_none_existed() {
        let out = compose_file_contents(None, OPEN, CLOSE, "hello world");
        assert!(out.starts_with(OPEN));
        assert!(out.contains("hello world"));
        assert!(out.contains(CLOSE));
    }

    #[test]
    fn compose_replaces_existing_block_preserving_surround() {
        let prior = format!("# Title\n\nuser content\n\n{OPEN}\nold body\n{CLOSE}\n\ntrailing\n");
        let updated = compose_file_contents(Some(&prior), OPEN, CLOSE, "new body");
        assert!(updated.contains("user content"));
        assert!(updated.contains("trailing"));
        assert!(updated.contains("new body"));
        assert!(!updated.contains("old body"));
        // Exactly one block (no double-write).
        assert_eq!(updated.matches(OPEN).count(), 1);
        assert_eq!(updated.matches(CLOSE).count(), 1);
    }

    #[test]
    fn compose_appends_block_to_file_without_anchors() {
        let prior = "# Pre-existing\n\nsome user content\n";
        let out = compose_file_contents(Some(prior), OPEN, CLOSE, "fresh body");
        assert!(out.contains("# Pre-existing"));
        assert!(out.contains("some user content"));
        assert!(out.contains("fresh body"));
    }

    #[tokio::test]
    async fn write_then_drift_match() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("CLAUDE.md");
        let out = write_anchor_block(&f, OPEN, CLOSE, "alpha\nbeta")
            .await
            .unwrap();
        let status = detect_drift(&f, OPEN, CLOSE, &out.body_hash).await.unwrap();
        assert_eq!(status, DriftStatus::Match);
    }

    #[tokio::test]
    async fn drift_reports_observed_hash_on_edit() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("CLAUDE.md");
        let out = write_anchor_block(&f, OPEN, CLOSE, "alpha").await.unwrap();
        // Mutate the body inside the anchor.
        let s = std::fs::read_to_string(&f).unwrap();
        let mutated = s.replace("alpha", "ZETA");
        std::fs::write(&f, mutated).unwrap();
        let status = detect_drift(&f, OPEN, CLOSE, &out.body_hash).await.unwrap();
        match status {
            DriftStatus::Drift {
                observed_hash,
                expected_hash,
                ..
            } => {
                assert_eq!(expected_hash, out.body_hash);
                assert_ne!(observed_hash, out.body_hash);
            }
            other => panic!("expected drift, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn drift_reports_anchor_missing_when_block_deleted() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("CLAUDE.md");
        let out = write_anchor_block(&f, OPEN, CLOSE, "alpha").await.unwrap();
        std::fs::write(&f, "anchors deleted\n").unwrap();
        let status = detect_drift(&f, OPEN, CLOSE, &out.body_hash).await.unwrap();
        assert_eq!(status, DriftStatus::AnchorMissing);
    }

    #[tokio::test]
    async fn drift_reports_file_missing_when_deleted() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("CLAUDE.md");
        let out = write_anchor_block(&f, OPEN, CLOSE, "alpha").await.unwrap();
        std::fs::remove_file(&f).unwrap();
        let status = detect_drift(&f, OPEN, CLOSE, &out.body_hash).await.unwrap();
        assert_eq!(status, DriftStatus::FileMissing);
    }

    #[tokio::test]
    async fn user_content_outside_block_is_byte_preserved_across_writes() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("CLAUDE.md");
        // Pre-populate with user content surrounding a stub block.
        let original = format!(
            "# Header\n\nUser line 1.\nUser line 2.\n\n{OPEN}\nold\n{CLOSE}\n\n## After\nMore user text.\n"
        );
        std::fs::write(&f, &original).unwrap();
        // Write a new block.
        write_anchor_block(&f, OPEN, CLOSE, "fresh body")
            .await
            .unwrap();
        let after = std::fs::read_to_string(&f).unwrap();
        assert!(after.contains("# Header"));
        assert!(after.contains("User line 1."));
        assert!(after.contains("User line 2."));
        assert!(after.contains("## After"));
        assert!(after.contains("More user text."));
        assert!(after.contains("fresh body"));
        assert!(!after.contains("old"));
    }
}
