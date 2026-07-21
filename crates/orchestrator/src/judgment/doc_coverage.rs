//! Doc-coverage v1 scorer.
//!
//! Lightweight heuristic: for each touched file with a known
//! source-code extension, check whether the file begins with a
//! doc-comment block. Returns the ratio of documented files
//! over total qualified files.
//!
//! Deliberate bounds:
//!
//!   * Counts top-of-file blocks only; in-function doc comments
//!     are ignored.
//!   * Treats markdown/json/toml/lock files as ineligible
//!     (drops from denominator).
//!   * No language-aware parsing — pure byte scan.
//!
//! This score is a bounded consistency signal, not a claim of semantic API
//! documentation coverage. Compiler and linter diagnostics remain part of the
//! separate audit layer.

use std::path::{Path, PathBuf};

/// Source-code extensions for which doc-block heuristics apply.
const ELIGIBLE_EXTS: &[&str] = &["rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go"];

/// Score the doc-coverage ratio over touched files.
///
/// Returns the fraction of *eligible* files (those whose extension
/// is in [`ELIGIBLE_EXTS`]) whose body begins with a doc-comment
/// block. Vacuously returns 1.0 when no eligible files are
/// present (e.g. a documentation-only PR touching just markdown).
///
/// Files that cannot be read (missing on disk, permission denied)
/// are dropped from the denominator — they contribute neither to
/// the numerator nor the count of qualified files.
pub fn score_doc_coverage(worktree_root: &Path, touched: &[String]) -> f64 {
    let (eligible_count, documented_count) = touched
        .iter()
        .filter_map(|rel| qualify(worktree_root, rel))
        .fold((0u32, 0u32), |(total, doc), is_documented| {
            (total + 1, doc + u32::from(is_documented))
        });

    if eligible_count == 0 {
        return 1.0;
    }
    let ratio = documented_count as f64 / eligible_count as f64;
    ratio.clamp(0.0, 1.0)
}

/// Returns `Some(is_documented)` if the file qualifies (eligible
/// extension AND readable on disk); `None` to drop it from the
/// denominator.
fn qualify(worktree_root: &Path, rel: &str) -> Option<bool> {
    let rel_path = PathBuf::from(rel);
    let ext = rel_path.extension().and_then(|s| s.to_str())?;
    if !ELIGIBLE_EXTS.contains(&ext) {
        return None;
    }
    let abs = worktree_root.join(&rel_path);
    let body = std::fs::read_to_string(&abs).ok()?;
    Some(has_top_of_file_doc_block(&body))
}

/// True if the file begins (after optional leading whitespace) with
/// a doc-comment block: Rust `//!`, Rust outer `///`, or
/// C-style `/** … */`.
///
/// Conservative — a leading non-doc comment (`//` or `/* */` without
/// the doc marker) does NOT count.
fn has_top_of_file_doc_block(body: &str) -> bool {
    let trimmed = body.trim_start();
    trimmed.starts_with("//!")
        || trimmed.starts_with("///")
        || trimmed.starts_with("/**")
        || trimmed.starts_with("\"\"\"") // Python docstring at file top
        || trimmed.starts_with("# ") // Python single-line module comment (loose)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, body: &str) -> String {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&p, body).unwrap();
        rel.to_string()
    }

    #[test]
    fn all_documented_returns_one() {
        let dir = TempDir::new().unwrap();
        let f1 = write(
            dir.path(),
            "src/lib.rs",
            "//! Library entry.\npub fn x() {}\n",
        );
        let f2 = write(dir.path(), "src/a.rs", "//! Module A.\npub fn a() {}\n");
        assert_eq!(score_doc_coverage(dir.path(), &[f1, f2]), 1.0);
    }

    #[test]
    fn none_documented_returns_zero() {
        let dir = TempDir::new().unwrap();
        let f1 = write(dir.path(), "src/lib.rs", "pub fn x() {}\n");
        let f2 = write(dir.path(), "src/a.rs", "pub fn a() {}\n");
        assert_eq!(score_doc_coverage(dir.path(), &[f1, f2]), 0.0);
    }

    #[test]
    fn half_documented_returns_half() {
        let dir = TempDir::new().unwrap();
        let f1 = write(dir.path(), "src/lib.rs", "//! Documented.\npub fn x() {}\n");
        let f2 = write(dir.path(), "src/a.rs", "pub fn a() {}\n");
        assert_eq!(score_doc_coverage(dir.path(), &[f1, f2]), 0.5);
    }

    #[test]
    fn js_jsdoc_block_counts_as_documented() {
        let dir = TempDir::new().unwrap();
        let f1 = write(
            dir.path(),
            "src/index.ts",
            "/**\n * Module entry.\n */\nexport function x() {}\n",
        );
        assert_eq!(score_doc_coverage(dir.path(), &[f1]), 1.0);
    }

    #[test]
    fn ineligible_and_missing_files_drop_from_denominator() {
        let dir = TempDir::new().unwrap();
        let f1 = write(dir.path(), "src/lib.rs", "//! ok\npub fn x() {}\n");
        let _f2 = write(dir.path(), "Cargo.toml", "[package]\nname = \"x\"\n");
        let _f3 = write(dir.path(), "README.md", "# X\n");
        // Only lib.rs counts; ineligible exts + missing files drop.
        let inputs = vec![
            f1,
            "Cargo.toml".to_string(),
            "README.md".to_string(),
            "src/ghost.rs".to_string(),
        ];
        assert_eq!(score_doc_coverage(dir.path(), &inputs), 1.0);
    }

    #[test]
    fn empty_input_returns_one() {
        // No qualified files → vacuously "documented".
        let dir = TempDir::new().unwrap();
        assert_eq!(score_doc_coverage(dir.path(), &[]), 1.0);
    }
}
