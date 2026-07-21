//! A4 (Tier-2G) — workspace-wide SQL-leakage guard.
//!
//! The orchestrator owns the database. Adapters, the host, the
//! frontend and the mock harness all interact with persistence
//! through typed methods on `Repository`, `MemoryKernel`, or
//! `MemoryStore`. None of them should ever issue an `sqlx::query*`
//! call directly — if a new caller needs to read a row, the right
//! answer is to add a typed accessor inside the orchestrator and
//! call THAT.
//!
//! This integration test enforces the rule mechanically. It walks
//! every Rust source file under the workspace root and fails if
//! `sqlx::query` appears outside the `crates/orchestrator/` crate. The
//! `pool()` accessors on `Repository` / `MemoryKernel` /
//! `MemoryStore` are the visibility half of the discipline; this
//! test is the textual half.
//!
//! ## Scope: the WHOLE orchestrator crate, not just src/
//!
//! `crates/orchestrator/tests/` holds integration tests that legitimately
//! verify DB state with raw queries. They're owned by the same team
//! as `crates/orchestrator/src/` and exercise the same contract, so the
//! guard treats the entire crate as one trust boundary.
//!
//! ## Why a Rust test, not a CI shell script?
//!
//! - Runs on every `cargo test --workspace` invocation locally and
//!   in CI without extra config.
//! - The failure message is the same on a contributor's laptop and
//!   on the build server.
//! - Tooling-free — no `cargo-deny`, no `clippy.toml` per-crate
//!   `#[allow]` sprinkles.

use std::fs;
use std::path::{Path, PathBuf};

/// File that is itself the guard — references the disallowed
/// pattern in plain code (not comments) to assemble error messages.
/// Skipped by basename so the guard doesn't false-positive on
/// itself.
const SELF_FILENAME: &str = "no_sql_outside_orchestrator.rs";

#[test]
fn sqlx_query_must_live_inside_orchestrator_crate() {
    let workspace = workspace_root();
    let orchestrator_crate = workspace.join("crates/orchestrator");

    let mut leaks: Vec<String> = Vec::new();
    walk(&workspace, &mut |path| {
        if !is_rust_source(path) {
            return;
        }
        if is_skipped_path(&workspace, path) {
            return;
        }
        if path.starts_with(&orchestrator_crate) {
            // Inside the orchestrator crate — `sqlx::query*` is
            // legitimate here (production code in `src/`, integration
            // tests in `tests/`). Skip the file entirely.
            return;
        }
        if path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s == SELF_FILENAME)
            .unwrap_or(false)
        {
            // The guard test itself references the disallowed
            // pattern in scanning + error messages. Skip self.
            return;
        }
        let Ok(contents) = fs::read_to_string(path) else {
            return;
        };
        for (idx, line) in contents.lines().enumerate() {
            if !line.contains("sqlx::query") {
                continue;
            }
            if is_comment_or_doc(line) {
                continue;
            }
            leaks.push(format!(
                "{}:{}: {}",
                path.strip_prefix(&workspace).unwrap_or(path).display(),
                idx + 1,
                line.trim()
            ));
        }
    });

    assert!(
        leaks.is_empty(),
        "\n  SQL leaked outside the orchestrator crate. All raw query calls must live\n  \
         inside `crates/orchestrator/` so the database surface stays small, typed, and\n  \
         reviewable. If you're adding a new read surface, expose it as a method\n  \
         on `Repository`, `MemoryKernel`, or `MemoryStore` instead.\n\n  \
         Offending lines:\n    {}\n",
        leaks.join("\n    "),
    );
}

/// Find the workspace root by walking upward from this crate's
/// manifest dir. We can't use `cargo locate-project` from a test
/// (would need a process spawn) and `env!("CARGO_MANIFEST_DIR")` is
/// `crates/orchestrator/`, so the workspace root is two levels up.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/orchestrator/ is two levels below the workspace root")
        .to_path_buf()
}

fn is_rust_source(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()) == Some("rs")
}

/// Directories the guard does not walk:
///   * `target/` — compiler artifacts include generated `sqlx::query`
///     code; not source we own.
///   * `node_modules/` — frontend deps.
///   * `.git/` — never has Rust sources we author.
///   * `dist/` — frontend build output.
///   * `vendor/` — vendored deps (if any).
///   * `.claude/` local agent/editor tooling scratch that can hold
///     full workspace copies which would false-positive.
fn is_skipped_path(workspace: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(workspace) else {
        return false;
    };
    rel.components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some("target" | "node_modules" | ".git" | "dist" | "vendor" | ".claude")
        )
    })
}

/// Heuristic: lines that begin with `//`, `*`, or `/*` are comments
/// or doc blocks. A finer parse would require syn / proc-macro
/// machinery; the heuristic is sufficient because comment-style
/// mentions of `sqlx::query` only appear in module-level docs we
/// control, all of which start with one of these prefixes.
fn is_comment_or_doc(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with("/*") || t.starts_with("*")
}

fn walk(dir: &Path, visit: &mut dyn FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, visit);
        } else if path.is_file() {
            visit(&path);
        }
    }
}

// ---------------------------------------------------------------------
// Self-tests for the guard's own predicates. These run inside the
// orchestrator crate's allow-list, so we can use `sqlx::query`-shaped
// strings in fixtures freely.
// ---------------------------------------------------------------------

#[test]
fn is_comment_or_doc_recognises_common_prefixes() {
    assert!(is_comment_or_doc("// a sqlx::query mention"));
    assert!(is_comment_or_doc("    // sqlx::query in indented comment"));
    assert!(is_comment_or_doc("/* opening block */"));
    assert!(is_comment_or_doc(" * inside block"));
    assert!(!is_comment_or_doc("    sqlx::query(\"...\")")); // real code
    assert!(!is_comment_or_doc("let q = sqlx::query(\"...\");"));
}

#[test]
fn is_skipped_path_skips_target_node_modules_dist_git_vendor_claude() {
    let ws = Path::new("/ws");
    let cases = [
        "/ws/target/debug/foo.rs",
        "/ws/node_modules/x/y.rs",
        "/ws/dist/build.rs",
        "/ws/.git/hooks/post.rs",
        "/ws/vendor/foo.rs",
        "/ws/.claude/worktrees/x/y.rs",
    ];
    for c in cases {
        assert!(is_skipped_path(ws, Path::new(c)), "{c} should be skipped");
    }
    assert!(!is_skipped_path(
        ws,
        Path::new("/ws/crates/orchestrator/src/lib.rs")
    ));
    assert!(!is_skipped_path(
        ws,
        Path::new("/ws/crates/adapters/claude/src/lib.rs")
    ));
}
