//! Score how well a worker's diff adhered to declared scope.
//!
//! Scope paths are directory or file prefixes relative to the
//! worktree root. A touched file matches scope if any scope path
//! is a prefix of the touched file's path. Final score is routed
//! through [`crate::audit::report::clamp_score`].

use crate::audit::report::{clamp_score, ScopeScore};
use std::path::Path;

/// Returns a [`ScopeScore`] over a worker's touched file list.
/// Empty scope means "no constraint" → all touched files are
/// in-scope. Empty touched-files means "no change" → score 1.0.
pub fn score_scope(touched_files: &[String], scope_paths: &[std::path::PathBuf]) -> ScopeScore {
    if touched_files.is_empty() {
        return ScopeScore {
            in_scope: 0,
            out_of_scope: 0,
            score: 1.0,
        };
    }
    if scope_paths.is_empty() {
        return ScopeScore {
            in_scope: touched_files.len() as u32,
            out_of_scope: 0,
            score: 1.0,
        };
    }

    let mut in_scope = 0u32;
    let mut out_of_scope = 0u32;
    for f in touched_files {
        if scope_paths.iter().any(|sp| Path::new(f).starts_with(sp)) {
            in_scope += 1;
        } else {
            out_of_scope += 1;
        }
    }
    let total = (in_scope + out_of_scope) as f64;
    let raw = if total == 0.0 {
        1.0
    } else {
        in_scope as f64 / total
    };
    ScopeScore {
        in_scope,
        out_of_scope,
        score: clamp_score(raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn pb(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn no_scope_paths_means_unbounded() {
        let touched = vec!["src/main.rs".into(), "README.md".into()];
        let score = score_scope(&touched, &[]);
        assert_eq!(score.in_scope, 2);
        assert_eq!(score.out_of_scope, 0);
        assert_eq!(score.score, 1.0);
    }

    #[test]
    fn all_files_inside_scope() {
        let touched = vec!["src/main.rs".into(), "src/lib.rs".into()];
        let scope = vec![pb("src")];
        let score = score_scope(&touched, &scope);
        assert_eq!(score.in_scope, 2);
        assert_eq!(score.out_of_scope, 0);
        assert_eq!(score.score, 1.0);
    }

    #[test]
    fn one_file_outside_scope_lowers_score() {
        let touched = vec!["src/main.rs".into(), "docs/README.md".into()];
        let scope = vec![pb("src")];
        let score = score_scope(&touched, &scope);
        assert_eq!(score.in_scope, 1);
        assert_eq!(score.out_of_scope, 1);
        assert_eq!(score.score, 0.5);
    }

    #[test]
    fn no_touched_files_scores_one() {
        let scope = vec![pb("src")];
        let score = score_scope(&[], &scope);
        assert_eq!(score.score, 1.0);
    }
}
