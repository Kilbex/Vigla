//! Authority bounds the arbiter checks. See roadmap §2 — every
//! supervisor decision is checked against four orthogonal bounds.
//! The first bound to trip (in [`crate::arbiter::decide`] priority
//! order) determines whether the worker is accepted, reworked,
//! scrubbed, or escalated.

use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityBound {
    /// Worker touched files outside the declared `scope_paths`.
    Scope,
    /// Snapshot creation, rebase, or integration failed. The pure arbiter
    /// cannot probe Git; the mission integration boundary maps those I/O
    /// failures to this bound before entering `Attention`.
    Reversibility,
    /// A risk detector tripped (schema migration, mass deletion,
    /// secret-touching change, …). Mapped from
    /// [`crate::audit::SecurityFlagKind`].
    Risk,
    /// Audit score is below the policy quality floor and rework
    /// budget cannot rescue it.
    Quality,
}

/// Carries enough detail for the inbox card (S3) to explain what
/// tripped to the user, without re-parsing the audit report.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
pub struct EscalationEvidence {
    /// Free-form human-readable explanation. Keep < 200 chars; long
    /// detail goes in `payload_json`.
    pub summary: String,
    /// Optional machine-readable JSON blob. Used by the inbox to
    /// render structured breakdowns (e.g. list of out-of-scope
    /// paths, list of failing tests).
    pub payload_json: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_four_bounds_round_trip() {
        for bound in [
            AuthorityBound::Scope,
            AuthorityBound::Reversibility,
            AuthorityBound::Risk,
            AuthorityBound::Quality,
        ] {
            let json = serde_json::to_string(&bound).unwrap();
            let back: AuthorityBound = serde_json::from_str(&json).unwrap();
            assert_eq!(bound, back);
        }
    }

    #[test]
    fn evidence_default_is_empty() {
        let e = EscalationEvidence::default();
        assert!(e.summary.is_empty());
        assert!(e.payload_json.is_none());
    }

    #[test]
    fn evidence_round_trips_with_payload() {
        let e = EscalationEvidence {
            summary: "3 files outside scope".to_string(),
            payload_json: Some(r#"{"paths":["a.rs","b.rs"]}"#.to_string()),
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: EscalationEvidence = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }
}
