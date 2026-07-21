//! Step 22 — disk-backed playbook templates.
//!
//! Stores user-saved playbook JSON in `<app_local_data_dir>/playbooks/`
//! with one `*.json` file per playbook. The frontend defines the
//! authoritative shape (`PlaybookTemplate` in `app/src/playbooks/types.ts`);
//! this Rust layer treats the body as opaque JSON and only validates
//! that:
//!   * the id is a safe filename (alphanumeric, dash, underscore only —
//!     no `..`, no path separators, no dotfiles),
//!   * the body parses as valid JSON.
//!
//! Atomic writes go through a `<id>.tmp` sibling and a rename so a
//! crash mid-write never leaves a half-flushed playbook on disk.

use std::path::PathBuf;

use serde::Serialize;
use specta::Type;

#[derive(Debug, Clone, Serialize, Type)]
pub struct StoredPlaybook {
    pub id: String,
    pub json: String,
}

#[derive(Debug)]
pub struct PlaybookStore {
    root: PathBuf,
}

impl PlaybookStore {
    /// Create the playbook directory if missing and return a handle.
    pub fn open(root: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    fn path_for(&self, id: &str) -> Result<PathBuf, String> {
        validate_id(id)?;
        Ok(self.root.join(format!("{id}.json")))
    }

    /// Read every `*.json` file in the playbook root, returning the id
    /// plus raw JSON body for each.
    ///
    /// Malformed files (non-JSON, unreadable) are skipped with a
    /// `tracing::warn!` instead of aborting; the frontend should still
    /// see every healthy playbook.
    pub fn list(&self) -> std::io::Result<Vec<StoredPlaybook>> {
        let mut out = Vec::new();
        if !self.root.exists() {
            return Ok(out);
        }
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            // Skip the temp-file sibling we use during atomic save.
            if stem.starts_with('.') {
                continue;
            }
            // Re-validate the id to avoid surfacing a path that
            // wouldn't be writable — defends against manual edits.
            if validate_id(stem).is_err() {
                tracing::warn!("vigla-host: skipping playbook with invalid id: {stem}");
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(json) => {
                    if serde_json::from_str::<serde_json::Value>(&json).is_ok() {
                        out.push(StoredPlaybook {
                            id: stem.to_string(),
                            json,
                        });
                    } else {
                        tracing::error!(
                            "vigla-host: skipping malformed playbook {}",
                            path.display()
                        );
                    }
                }
                Err(e) => {
                    tracing::error!("vigla-host: read playbook {} failed: {e}", path.display());
                }
            }
        }
        // Stable order so the frontend list isn't shuffled by inode order.
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// Save (or overwrite) a playbook. Validates that `json` is parseable
    /// JSON; rejects with a `String` error on either schema or filename
    /// failure so the IPC client can surface a useful message.
    pub fn save(&self, id: &str, json: &str) -> Result<(), String> {
        if serde_json::from_str::<serde_json::Value>(json).is_err() {
            return Err("playbook body is not valid JSON".into());
        }
        let path = self.path_for(id)?;
        let tmp = self.root.join(format!(".{id}.tmp"));
        std::fs::write(&tmp, json).map_err(|e| format!("write tmp: {e}"))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("rename: {e}"))?;
        Ok(())
    }

    /// Idempotent delete: missing file is treated as success so the UI
    /// can re-fire delete without seeing a confusing error after a
    /// concurrent removal.
    pub fn delete(&self, id: &str) -> Result<(), String> {
        let path = self.path_for(id)?;
        match std::fs::remove_file(&path) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("delete: {e}")),
        }
    }
}

/// Reject ids that would escape the playbook root or collide with the
/// atomic-save tempfile namespace. Allowed: ASCII alphanumeric, dash,
/// underscore. Rejected: anything else (slash, dot, space, unicode).
fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("playbook id is empty".into());
    }
    if id.len() > 128 {
        return Err("playbook id too long (max 128 chars)".into());
    }
    if id.starts_with('.') {
        return Err("playbook id may not start with a dot".into());
    }
    for ch in id.chars() {
        if !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_') {
            return Err(format!(
                "playbook id contains invalid character {ch:?} (only [A-Za-z0-9_-] allowed)"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(suffix: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "vigla-playbook-store-{suffix}-{}",
            std::process::id()
        ));
        // Ensure clean each test.
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn open_creates_root_directory() {
        let root = temp_root("open");
        assert!(!root.exists());
        let _ = PlaybookStore::open(root.clone()).unwrap();
        assert!(root.exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn list_empty_root_returns_empty() {
        let root = temp_root("list-empty");
        let store = PlaybookStore::open(root.clone()).unwrap();
        assert_eq!(store.list().unwrap().len(), 0);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn save_then_list_returns_one() {
        let root = temp_root("save-list");
        let store = PlaybookStore::open(root.clone()).unwrap();
        store.save("trio-sweep", r#"{"name":"trio"}"#).unwrap();
        let entries = store.list().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "trio-sweep");
        assert!(entries[0].json.contains("trio"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn save_rejects_invalid_json() {
        let root = temp_root("invalid-json");
        let store = PlaybookStore::open(root.clone()).unwrap();
        let err = store.save("x", "{not valid").unwrap_err();
        assert!(err.contains("not valid JSON"), "msg = {err}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn save_rejects_invalid_ids() {
        let root = temp_root("invalid-id");
        let store = PlaybookStore::open(root.clone()).unwrap();
        for bad in [
            "",
            "..",
            "../escape",
            "with/slash",
            "dot.in.middle",
            ".dotfile",
            "with space",
        ] {
            let err = store.save(bad, r#"{}"#).unwrap_err();
            assert!(
                err.contains("invalid character") || err.contains("empty") || err.contains("dot"),
                "id {bad:?} should fail; got: {err}",
            );
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn save_rejects_overlong_id() {
        let root = temp_root("overlong");
        let store = PlaybookStore::open(root.clone()).unwrap();
        let big = "a".repeat(200);
        let err = store.save(&big, r#"{}"#).unwrap_err();
        assert!(err.contains("too long"), "msg = {err}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn save_overwrites_existing_atomically() {
        let root = temp_root("overwrite");
        let store = PlaybookStore::open(root.clone()).unwrap();
        store.save("p", r#"{"v":1}"#).unwrap();
        store.save("p", r#"{"v":2}"#).unwrap();
        let entries = store.list().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].json.contains("\"v\":2"));
        // Tempfile is gone after rename.
        let tmp = root.join(".p.tmp");
        assert!(!tmp.exists(), "tempfile should be cleaned up");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn delete_missing_is_idempotent() {
        let root = temp_root("delete-missing");
        let store = PlaybookStore::open(root.clone()).unwrap();
        store.delete("does-not-exist").unwrap();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn delete_removes_file() {
        let root = temp_root("delete-removes");
        let store = PlaybookStore::open(root.clone()).unwrap();
        store.save("doomed", r#"{}"#).unwrap();
        store.delete("doomed").unwrap();
        assert_eq!(store.list().unwrap().len(), 0);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn list_skips_non_json_extensions() {
        let root = temp_root("non-json-ext");
        let store = PlaybookStore::open(root.clone()).unwrap();
        store.save("p", r#"{}"#).unwrap();
        // A file with a different extension shouldn't appear.
        std::fs::write(root.join("notes.txt"), "not a playbook").unwrap();
        let entries = store.list().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "p");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn list_skips_malformed_json() {
        let root = temp_root("malformed");
        let store = PlaybookStore::open(root.clone()).unwrap();
        std::fs::write(root.join("broken.json"), "this is not json").unwrap();
        store.save("good", r#"{"ok":true}"#).unwrap();
        let entries = store.list().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "good");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn list_returns_stable_alphabetical_order() {
        let root = temp_root("ordering");
        let store = PlaybookStore::open(root.clone()).unwrap();
        store.save("zebra", r#"{}"#).unwrap();
        store.save("alpha", r#"{}"#).unwrap();
        store.save("middle", r#"{}"#).unwrap();
        let ids: Vec<String> = store.list().unwrap().into_iter().map(|p| p.id).collect();
        assert_eq!(ids, vec!["alpha", "middle", "zebra"]);
        std::fs::remove_dir_all(&root).ok();
    }
}
