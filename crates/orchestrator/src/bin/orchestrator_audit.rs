//! Ad-hoc audit CLI for debugging.
//!
//! Usage:
//!   orchestrator_audit --root <path> [--scope <p1,p2>] [--tier smoke|standard|deep]

use orchestrator::audit::{audit_submission, AuditInput, AuditTier};
use std::path::PathBuf;
use std::process::ExitCode;

fn parse_tier(s: &str) -> Option<AuditTier> {
    match s.to_ascii_lowercase().as_str() {
        "smoke" => Some(AuditTier::Smoke),
        "standard" => Some(AuditTier::Standard),
        "deep" => Some(AuditTier::Deep),
        _ => None,
    }
}

fn print_usage() {
    println!("Usage: orchestrator_audit --root <path> [--scope a,b] [--tier smoke|standard|deep]");
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return ExitCode::SUCCESS;
    }
    let mut root: Option<PathBuf> = None;
    let mut scope: Vec<PathBuf> = Vec::new();
    let mut tier = AuditTier::Smoke;

    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--root" => {
                if let Some(v) = iter.next() {
                    root = Some(PathBuf::from(v));
                }
            }
            "--scope" => {
                if let Some(v) = iter.next() {
                    scope = v.split(',').map(PathBuf::from).collect();
                }
            }
            "--tier" => {
                if let Some(v) = iter.next() {
                    if let Some(t) = parse_tier(v) {
                        tier = t;
                    }
                }
            }
            _ => {}
        }
    }

    let Some(root) = root else {
        eprintln!("error: --root is required");
        print_usage();
        return ExitCode::from(2);
    };

    let input = AuditInput {
        worktree_root: root,
        test_command: None,
        // CLI v1: empty touched_files means the scope-adherence scorer
        // sees "no diff" and scores 1.0 trivially. A future flag could
        // accept --touched a,b,c to populate this.
        touched_files: vec![],
        scope_paths: scope,
        tier,
        baseline: None,
        newly_passing: vec![],
        newly_failing: vec![],
    };
    match audit_submission(&input).await {
        Ok(report) => {
            let json = serde_json::to_string_pretty(&report).unwrap();
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("audit failed: {e}");
            ExitCode::from(1)
        }
    }
}
