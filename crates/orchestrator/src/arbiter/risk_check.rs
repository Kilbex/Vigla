//! Risk-bound check. Maps [`crate::audit::SecurityFlag`]s plus
//! policy's `risk_detectors_enabled` into a `RiskViolation`.
//!
//! S1's security scorer already populates `audit.security_flags`
//! based on path heuristics (secret files, schema migrations, mass
//! deletions). This check filters the flags by what the policy has
//! enabled and constructs evidence for the inbox.

use crate::arbiter::{
    bound::{AuthorityBound, EscalationEvidence},
    decision::SuggestedUserAction,
    policy::RiskDetectorSet,
};
use crate::audit::{AuditReport, SecurityFlag, SecurityFlagKind};

pub struct RiskViolation {
    pub bound: AuthorityBound,
    pub evidence: EscalationEvidence,
    pub suggested_user_action: SuggestedUserAction,
}

pub fn check_risk(audit: &AuditReport, detectors: &RiskDetectorSet) -> Option<RiskViolation> {
    let active: Vec<&SecurityFlag> = audit
        .security_flags
        .iter()
        .filter(|f| detector_enabled(&f.kind, detectors))
        .collect();

    if active.is_empty() {
        return None;
    }

    let detail = active
        .iter()
        .map(|f| format!("{:?} at {}: {}", f.kind, f.path, f.detail))
        .collect::<Vec<_>>()
        .join("; ");

    let summary = format!("{} risk detector(s) tripped", active.len());
    let payload = serde_json::to_string(&active).ok();

    Some(RiskViolation {
        bound: AuthorityBound::Risk,
        evidence: EscalationEvidence {
            summary,
            payload_json: payload,
        },
        suggested_user_action: SuggestedUserAction::CoSignRisk { detail },
    })
}

fn detector_enabled(kind: &SecurityFlagKind, set: &RiskDetectorSet) -> bool {
    match kind {
        SecurityFlagKind::SchemaMigration => set.schema_migration,
        SecurityFlagKind::MassDeletion => set.mass_deletion,
        SecurityFlagKind::SecretFile => set.secret_files,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{AuditReport, SecurityFlag, SecurityFlagKind};

    fn audit_with_flags(flags: Vec<SecurityFlag>) -> AuditReport {
        AuditReport {
            security_flags: flags,
            ..AuditReport::default()
        }
    }

    #[test]
    fn empty_flags_no_violation() {
        let v = check_risk(&audit_with_flags(vec![]), &RiskDetectorSet::default());
        assert!(v.is_none());
    }

    #[test]
    fn secret_file_with_detector_enabled_violates() {
        let v = check_risk(
            &audit_with_flags(vec![SecurityFlag {
                kind: SecurityFlagKind::SecretFile,
                path: ".env.local".to_string(),
                detail: "secret-file pattern".to_string(),
            }]),
            &RiskDetectorSet::default(),
        )
        .expect("violation");
        assert_eq!(v.bound, AuthorityBound::Risk);
        assert!(v.evidence.summary.contains("1"));
    }

    #[test]
    fn disabled_detector_skipped() {
        let set = RiskDetectorSet {
            schema_migration: false,
            ..RiskDetectorSet::default()
        };
        let v = check_risk(
            &audit_with_flags(vec![SecurityFlag {
                kind: SecurityFlagKind::SchemaMigration,
                path: "migrations/0099_x.sql".to_string(),
                detail: String::new(),
            }]),
            &set,
        );
        assert!(v.is_none());
    }

    #[test]
    fn multiple_flags_aggregate() {
        let v = check_risk(
            &audit_with_flags(vec![
                SecurityFlag {
                    kind: SecurityFlagKind::SecretFile,
                    path: ".env".to_string(),
                    detail: String::new(),
                },
                SecurityFlag {
                    kind: SecurityFlagKind::MassDeletion,
                    path: "(deleted 25 files)".to_string(),
                    detail: String::new(),
                },
            ]),
            &RiskDetectorSet::default(),
        )
        .expect("violation");
        assert!(v.evidence.summary.contains("2"));
    }
}
