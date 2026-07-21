//! Path-based security heuristics for a worker submission's touched-file list.
//!
//! This module deliberately does not duplicate repository secret scanners:
//! content scanning belongs in the configured audit command and the repository's
//! `gitleaks` gate. These cheap filename signals make risky paths visible during
//! every mission, including projects without a language-specific scanner.

use crate::audit::report::{SecurityFlag, SecurityFlagKind};

/// Threshold for `MassDeletion` flag. Empirical: typical feature
/// branches touch fewer than ~20 files; submissions above this are
/// worth flagging for review (large refactors, dependency bumps,
/// generated-code regenerations).
const MASS_THRESHOLD: usize = 20;

/// Return a list of security flags raised by the touched-files
/// list. Multiple flags per file are possible.
pub fn scan_security(touched_files: &[String]) -> Vec<SecurityFlag> {
    let mut flags = Vec::new();

    for path in touched_files {
        if looks_like_secret_file(path) {
            flags.push(SecurityFlag {
                kind: SecurityFlagKind::SecretFile,
                path: path.clone(),
                detail: "filename matches a secret-storage pattern".into(),
            });
        }
        if looks_like_migration(path) {
            flags.push(SecurityFlag {
                kind: SecurityFlagKind::SchemaMigration,
                path: path.clone(),
                detail: "touched a database migration".into(),
            });
        }
    }

    if touched_files.len() > MASS_THRESHOLD {
        flags.push(SecurityFlag {
            kind: SecurityFlagKind::MassDeletion,
            path: String::new(),
            detail: format!(
                "submission touches {} files (threshold: {MASS_THRESHOLD}); review carefully",
                touched_files.len()
            ),
        });
    }

    flags
}

fn looks_like_secret_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = std::path::Path::new(&lower)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&lower);

    // Explicit secret-storage patterns. Intentionally narrow —
    // broad substring matches (e.g. any "secret" anywhere in the
    // basename) generate noise on legitimate source files like
    // `secrets_manager.rs` or `credentialsHelper.go`.
    name == ".env"
        || name.starts_with(".env.")
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || name == "credentials"
        || name == "credentials.json"
        || name == "credentials.yaml"
        || name == "credentials.yml"
        || name == ".credentials"
        || name == "secrets.json"
        || name == "secrets.toml"
        || name == "secrets.yaml"
        || name == "secrets.yml"
        || name == ".secrets"
        || name.starts_with(".secrets.")
}

fn looks_like_migration(path: &str) -> bool {
    // Skip vendored libraries that happen to ship migrations dirs.
    if path.contains("node_modules/") || path.contains("/vendor/") || path.starts_with("vendor/") {
        return false;
    }
    path.contains("/migrations/") || path.starts_with("migrations/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::report::SecurityFlagKind;

    #[test]
    fn no_touched_files_means_no_flags() {
        assert!(scan_security(&[]).is_empty());
    }

    #[test]
    fn env_file_is_flagged_as_secret() {
        let flags = scan_security(&[".env".into()]);
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].kind, SecurityFlagKind::SecretFile);
    }

    #[test]
    fn pem_file_in_subdir_is_flagged() {
        let flags = scan_security(&["app/src-tauri/identity.pem".into()]);
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].kind, SecurityFlagKind::SecretFile);
    }

    #[test]
    fn migrations_dir_change_is_flagged() {
        let flags = scan_security(&["orchestrator/migrations/0007_audit.sql".into()]);
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].kind, SecurityFlagKind::SchemaMigration);
    }

    #[test]
    fn mass_deletion_flag_emitted() {
        // 25 paths to a non-secret extension simulate a mass change.
        let files: Vec<String> = (0..25).map(|i| format!("src/m_{i}.rs")).collect();
        let flags = scan_security(&files);
        assert!(flags
            .iter()
            .any(|f| f.kind == SecurityFlagKind::MassDeletion));
    }

    #[test]
    fn normal_source_files_not_flagged() {
        let flags = scan_security(&["src/main.rs".into(), "app/src/App.tsx".into()]);
        assert!(flags.is_empty());
    }

    #[test]
    fn secrets_manager_source_file_not_flagged() {
        // Issue 1 regression: legitimate source files containing
        // "secret" or "credentials" in the name should not flag.
        let flags = scan_security(&[
            "src/secrets_manager.rs".into(),
            "src/credentialsHelper.go".into(),
            "app/src/components/Secret/Modal.tsx".into(),
        ]);
        assert!(flags.is_empty(), "expected no flags, got {flags:?}");
    }

    #[test]
    fn node_modules_migrations_not_flagged() {
        // Issue 2 regression: vendored libraries that ship a
        // migrations/ dir should not flag as SchemaMigration.
        let flags = scan_security(&["node_modules/knex/migrations/init.js".into()]);
        assert!(flags.is_empty(), "expected no flags, got {flags:?}");
    }

    #[test]
    fn mass_deletion_flag_has_no_path() {
        // Issue 4 regression: MassDeletion.path must be empty;
        // count belongs in detail.
        let files: Vec<String> = (0..25).map(|i| format!("src/m_{i}.rs")).collect();
        let flags = scan_security(&files);
        let mass = flags
            .iter()
            .find(|f| f.kind == crate::audit::report::SecurityFlagKind::MassDeletion)
            .expect("MassDeletion flag present");
        assert!(mass.path.is_empty());
        assert!(mass.detail.contains("25"));
    }
}
