//! Quality Audit Layer.
//!
//! Top-level entry point is [`audit_submission`].
//! Sub-modules implement individual scorers (test-pass, scope,
//! regression, lint, security) and a composite blender.

pub mod composite;
pub mod lint;
pub mod persist;
mod process;
pub mod project;
pub mod regression;
pub mod report;
pub mod scope;
pub mod security;
pub mod test_pass;
pub mod tier;

pub use composite::{blend_overall, WeightProfile};
pub use project::{detect_project, detect_project_layout, ProjectLayout, ProjectType};
pub use report::{
    AuditReport, LintScore, RegressionScore, ScopeScore, SecurityFlag, SecurityFlagKind,
    TestPassScore,
};
pub use tier::{AuditTier, TierInput};

use std::path::PathBuf;

/// Input bundle for [`audit_submission`].
#[derive(Debug, Clone)]
pub struct AuditInput {
    pub worktree_root: PathBuf,
    /// Optional user-supplied test command. When present and non-blank it
    /// replaces project auto-detection for the test-pass scorer.
    pub test_command: Option<String>,
    pub touched_files: Vec<String>,
    pub scope_paths: Vec<PathBuf>,
    pub tier: AuditTier,
    /// Pre-mission test result for regression comparison. None for
    /// Smoke tier or when baseline capture failed.
    pub baseline: Option<report::TestPassScore>,
    pub newly_passing: Vec<String>,
    pub newly_failing: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("project detection: {0}")]
    ProjectDetection(#[from] project::ProjectDetectionError),
    #[error("test runner: {0}")]
    TestRunner(#[from] test_pass::TestPassError),
    #[error("lint runner: {0}")]
    Lint(#[from] lint::LintError),
}

/// Entry point: run the configured audit tier over a worker
/// submission and return a blended [`AuditReport`].
///
/// **Smoke** tier runs only scope and security (pure functions, no
/// subprocesses) — typically completes in <1ms.
/// **Standard / Deep** additionally invoke the project's test runner
/// and lint tools, dispatched by [`project::detect_project`].
///
/// Errors from sub-runners (`TestPassError`, `LintError`) propagate as
/// [`AuditError`]. Mission execution treats those infrastructure failures
/// as a failed pass rather than manufacturing a passing score.
pub async fn audit_submission(input: &AuditInput) -> Result<AuditReport, AuditError> {
    use project::{detect_project_layout, ProjectType};

    let layout = detect_project_layout(&input.worktree_root)?;
    let project_type = layout.project_type;
    let mut report = AuditReport {
        scope: Some(scope::score_scope(&input.touched_files, &input.scope_paths)),
        security_flags: security::scan_security(&input.touched_files),
        ..AuditReport::default()
    };

    if input.tier != AuditTier::Smoke {
        // Test pass.
        let test_score = if let Some(command) = input
            .test_command
            .as_deref()
            .map(str::trim)
            .filter(|command| !command.is_empty())
        {
            Some(test_pass::run_custom_tests(&input.worktree_root, command).await?)
        } else {
            match project_type {
                ProjectType::Rust => Some(test_pass::run_rust_tests(&input.worktree_root).await?),
                ProjectType::Node => Some(
                    test_pass::run_node_tests(
                        layout.node_root.as_deref().expect("Node layout has a root"),
                    )
                    .await?,
                ),
                ProjectType::Mixed => {
                    let rust = test_pass::run_rust_tests(&input.worktree_root).await?;
                    let node = test_pass::run_node_tests(
                        layout
                            .node_root
                            .as_deref()
                            .expect("Mixed layout has a Node root"),
                    )
                    .await?;
                    Some(combine_test_scores(rust, node))
                }
                ProjectType::None => None,
            }
        };
        if let Some(ts) = test_score.as_ref() {
            // Regression vs baseline — only when a baseline exists. With
            // no baseline (first run), this is None so it is excluded from
            // the blended denominator instead of contributing a free 1.0
            // that inflates `overall` toward the quality floor (F-1).
            report.regression = regression::regression_if_baselined(
                input.baseline.as_ref(),
                ts,
                &input.newly_passing,
                &input.newly_failing,
            );
        }
        report.test_pass = test_score;

        // Lint.
        let lint_score = match project_type {
            ProjectType::Rust => Some(lint::run_rust_lint(&input.worktree_root).await?),
            ProjectType::Node => Some(
                lint::run_node_lint(layout.node_root.as_deref().expect("Node layout has a root"))
                    .await?,
            ),
            ProjectType::Mixed => {
                let rust = lint::run_rust_lint(&input.worktree_root).await?;
                let node = lint::run_node_lint(
                    layout
                        .node_root
                        .as_deref()
                        .expect("Mixed layout has a Node root"),
                )
                .await?;
                Some(combine_lint_scores(rust, node))
            }
            ProjectType::None => None,
        };
        // If no lint tool actually scored (e.g. host has neither
        // rustfmt nor clippy installed), every sub-field is None and
        // the placeholder score is 1.0. composite.rs blends Some-lint
        // with full weight, so a placeholder score would silently
        // inflate `overall` past the arbiter's quality floor —
        // contradicting composite.rs's "unscored sub-scores contribute
        // 0 weight" contract. Treat all-None as truly unscored.
        report.lint = lint_score.and_then(|s| {
            if s.rustfmt_clean.is_none()
                && s.clippy_warnings.is_none()
                && s.biome_diagnostics.is_none()
            {
                None
            } else {
                Some(s)
            }
        });
    }

    report.overall = blend_overall(&report, &WeightProfile::default());
    Ok(report)
}

fn combine_test_scores(
    first: report::TestPassScore,
    second: report::TestPassScore,
) -> report::TestPassScore {
    let scored = [
        first.ran.then_some(first.score),
        second.ran.then_some(second.score),
    ];
    let score = scored.into_iter().flatten().reduce(f64::min).unwrap_or(0.0);
    report::TestPassScore {
        ran: first.ran || second.ran,
        passed: first.passed.saturating_add(second.passed),
        failed: first.failed.saturating_add(second.failed),
        skipped: first.skipped.saturating_add(second.skipped),
        score,
    }
}

fn combine_lint_scores(first: report::LintScore, second: report::LintScore) -> report::LintScore {
    report::LintScore {
        rustfmt_clean: first.rustfmt_clean.or(second.rustfmt_clean),
        clippy_warnings: first.clippy_warnings.or(second.clippy_warnings),
        biome_diagnostics: first.biome_diagnostics.or(second.biome_diagnostics),
        score: first.score.min(second.score),
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[tokio::test]
    async fn nested_only_node_project_runs_from_its_manifest_root() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app");
        std::fs::create_dir(&app).unwrap();
        std::fs::write(
            app.join("package.json"),
            r#"{
              "name": "nested-audit-fixture",
              "version": "0.0.1",
              "scripts": {
                "test": "node -e \"require('fs').writeFileSync('ran-here', process.cwd()); console.log('1 passing')\""
              }
            }"#,
        )
        .unwrap();

        let report = audit_submission(&AuditInput {
            worktree_root: dir.path().to_path_buf(),
            test_command: None,
            touched_files: vec!["app/package.json".into()],
            scope_paths: vec![],
            tier: AuditTier::Standard,
            baseline: None,
            newly_passing: vec![],
            newly_failing: vec![],
        })
        .await
        .unwrap();

        assert_eq!(
            report.test_pass.as_ref().map(|score| score.score),
            Some(1.0)
        );
        assert!(app.join("ran-here").is_file());
        assert!(!dir.path().join("ran-here").exists());
    }
}
