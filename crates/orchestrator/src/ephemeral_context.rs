//! Snapshot and cleanup for vendor-native context injected into worktrees.
//!
//! Memory and skills are delivery context, never worker output. This guard
//! restores an unchanged native file byte-for-byte and removes only
//! Vigla-owned anchor regions when a worker edited surrounding user content.

use std::io;
use std::path::{Path, PathBuf};

const NATIVE_FILES: [&str; 3] = ["CLAUDE.md", "AGENTS.md", "GEMINI.md"];
const OWNED_ANCHORS: [(&str, &str); 2] = [
    (
        crate::memory::MEMORY_ANCHOR_OPEN,
        crate::memory::MEMORY_ANCHOR_CLOSE,
    ),
    (
        crate::skills::SKILLS_ANCHOR_OPEN,
        crate::skills::SKILLS_ANCHOR_CLOSE,
    ),
];

#[derive(Debug, Clone)]
pub(crate) struct EphemeralContextSnapshot {
    files: Vec<ContextFile>,
}

#[derive(Debug, Clone)]
struct ContextFile {
    relative: PathBuf,
    original: Option<Vec<u8>>,
    injected_clean: Option<Vec<u8>>,
}

impl EphemeralContextSnapshot {
    pub(crate) async fn capture(worktree: &Path) -> io::Result<Self> {
        let mut files = Vec::with_capacity(NATIVE_FILES.len());
        for relative in NATIVE_FILES {
            files.push(ContextFile {
                relative: PathBuf::from(relative),
                original: read_optional(&worktree.join(relative)).await?,
                injected_clean: None,
            });
        }
        Ok(Self { files })
    }

    /// Record the post-injection shape. Calling this again after a vendor swap
    /// is intentional; the original snapshot remains unchanged.
    pub(crate) async fn seal(&mut self, worktree: &Path) -> io::Result<()> {
        for file in &mut self.files {
            file.injected_clean = match read_optional(&worktree.join(&file.relative)).await? {
                Some(bytes) => Some(strip_owned_regions(&bytes)?),
                None => None,
            };
        }
        Ok(())
    }

    pub(crate) async fn restore(&self, worktree: &Path) -> io::Result<()> {
        for file in &self.files {
            let path = worktree.join(&file.relative);
            let Some(current) = read_optional(&path).await? else {
                // A worker may intentionally delete a pre-existing native file.
                continue;
            };
            if !contains_owned_delimiter(&current) {
                continue;
            }
            let cleaned = strip_owned_regions(&current)?;
            if file.injected_clean.as_ref() == Some(&cleaned) {
                write_optional(&path, file.original.as_deref()).await?;
            } else if cleaned.iter().all(u8::is_ascii_whitespace) && file.original.is_none() {
                write_optional(&path, None).await?;
            } else {
                tokio::fs::write(&path, cleaned).await?;
            }
        }
        Ok(())
    }
}

async fn read_optional(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

async fn write_optional(path: &Path, contents: Option<&[u8]>) -> io::Result<()> {
    match contents {
        Some(contents) => tokio::fs::write(path, contents).await,
        None => match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        },
    }
}

fn contains_owned_delimiter(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    OWNED_ANCHORS
        .iter()
        .any(|(open, close)| text.contains(open) || text.contains(close))
}

fn strip_owned_regions(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut text = String::from_utf8(bytes.to_vec()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "native context file is not UTF-8",
        )
    })?;
    for (open, close) in OWNED_ANCHORS {
        while let Some(span) = crate::memory::find_anchor_span(&text, open, close) {
            text.replace_range(
                span.open_offset as usize..span.close_end_offset as usize,
                "",
            );
        }
        if text.contains(open) || text.contains(close) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("malformed Vigla context anchor in native file: {open}"),
            ));
        }
    }
    Ok(text.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn restores_absent_and_unchanged_native_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut snapshot = EphemeralContextSnapshot::capture(dir.path()).await.unwrap();
        crate::memory::write_anchor_block(
            &dir.path().join("CLAUDE.md"),
            crate::memory::MEMORY_ANCHOR_OPEN,
            crate::memory::MEMORY_ANCHOR_CLOSE,
            "secret context",
        )
        .await
        .unwrap();
        snapshot.seal(dir.path()).await.unwrap();
        snapshot.restore(dir.path()).await.unwrap();
        assert!(!dir.path().join("CLAUDE.md").exists());
    }

    #[tokio::test]
    async fn preserves_worker_edits_outside_owned_regions() {
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("AGENTS.md");
        tokio::fs::write(&native, b"original\n").await.unwrap();
        let mut snapshot = EphemeralContextSnapshot::capture(dir.path()).await.unwrap();
        crate::memory::write_anchor_block(
            &native,
            crate::memory::MEMORY_ANCHOR_OPEN,
            crate::memory::MEMORY_ANCHOR_CLOSE,
            "private memory",
        )
        .await
        .unwrap();
        snapshot.seal(dir.path()).await.unwrap();
        let mut edited = tokio::fs::read_to_string(&native).await.unwrap();
        edited.insert_str(0, "worker edit\n");
        tokio::fs::write(&native, edited).await.unwrap();
        snapshot.restore(dir.path()).await.unwrap();
        let final_text = tokio::fs::read_to_string(&native).await.unwrap();
        assert!(final_text.contains("worker edit"));
        assert!(final_text.contains("original"));
        assert!(!final_text.contains("private memory"));
        assert!(!final_text.contains("vigla:memory"));
    }
}
