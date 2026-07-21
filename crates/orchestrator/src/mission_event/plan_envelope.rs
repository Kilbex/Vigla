//! Typed plan-context payloads attached to `PlanProposed` events.
//!
//! These describe the supervisor's plan beyond the existing
//! `Vec<TaskDescriptor>`: a typed tech stack, a four-bound
//! envelope-fit self-assessment, and the supporting enums.
//!
//! All types live behind `Option<…>` on the wire (see
//! `MissionEventKind::PlanProposed`), so earlier event-log rows deserialize
//! unchanged.

use crate::arbiter::AuthorityBound;
use serde::{Deserialize, Serialize};
use specta::Type;

/// One row of the supervisor's `tech_stack` summary. `layer` is a
/// short descriptor (e.g. `"framework"`, `"database"`,
/// `"test runner"`), `choice` is the specific selection
/// (e.g. `"Tauri 2"`, `"SQLite"`), and `is_new` flags stack
/// elements not already present in the user's repo so the FE can
/// badge them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TechChoice {
    pub layer: String,
    pub choice: String,
    pub rationale: String,
    #[serde(default)]
    pub is_new: bool,
}

/// Per-bound classification of the proposed plan against the
/// user's authority envelope. Three buckets:
///
/// - `Within` — the plan fits comfortably under the bound.
/// - `NearLimit` — close enough that an extra rework iteration
///   could push it over.
/// - `Exceeds` — the plan as proposed is past the bound; the
///   `mission_loop` gate forces `PendingPlanApproval` even in
///   Direct mode when any bound reaches this value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum BoundFitKind {
    Within,
    NearLimit,
    Exceeds,
}

/// A bound classification plus the supervisor's free-form note
/// explaining *why* — surfaced in the FE envelope-panel tooltip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct BoundFit {
    pub fit: BoundFitKind,
    #[serde(default)]
    pub note: String,
}

/// The supervisor's self-assessment of the proposed plan against
/// each of the four authority bounds. Carried on `PlanProposed`
/// when the supervisor adapter emits it. Decompositions without an envelope
/// pause only when `MissionSpec.confirm_plan == Some(true)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct EnvelopeFit {
    pub scope: BoundFit,
    pub reversibility: BoundFit,
    pub risk: BoundFit,
    pub quality: BoundFit,
}

impl EnvelopeFit {
    /// Returns the worst classification across all four bounds.
    /// Order: `Within < NearLimit < Exceeds`.
    pub fn worst(&self) -> BoundFitKind {
        [
            self.scope.fit,
            self.reversibility.fit,
            self.risk.fit,
            self.quality.fit,
        ]
        .into_iter()
        .max_by_key(|k| match k {
            BoundFitKind::Within => 0,
            BoundFitKind::NearLimit => 1,
            BoundFitKind::Exceeds => 2,
        })
        .unwrap_or(BoundFitKind::Within)
    }

    /// Returns the first bound classified as `Exceeds`, in the
    /// canonical bound order (Scope → Reversibility → Risk →
    /// Quality). Used by `arbiter::plan_envelope_check::check`
    /// to drive the gate.
    pub fn exceeded(&self) -> Option<(AuthorityBound, &BoundFit)> {
        for (bound, bf) in [
            (AuthorityBound::Scope, &self.scope),
            (AuthorityBound::Reversibility, &self.reversibility),
            (AuthorityBound::Risk, &self.risk),
            (AuthorityBound::Quality, &self.quality),
        ] {
            if bf.fit == BoundFitKind::Exceeds {
                return Some((bound, bf));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bf(fit: BoundFitKind, note: &str) -> BoundFit {
        BoundFit {
            fit,
            note: note.to_string(),
        }
    }

    #[test]
    fn tech_choice_round_trips_with_is_new() {
        let t = TechChoice {
            layer: "auth_provider".into(),
            choice: "Auth0".into(),
            rationale: "matches existing setup".into(),
            is_new: false,
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: TechChoice = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn tech_choice_defaults_is_new_to_false() {
        let json = r#"{"layer":"db","choice":"sqlite","rationale":"existing"}"#;
        let t: TechChoice = serde_json::from_str(json).unwrap();
        assert!(!t.is_new);
    }

    #[test]
    fn bound_fit_kind_serializes_snake_case() {
        for (k, s) in [
            (BoundFitKind::Within, "\"within\""),
            (BoundFitKind::NearLimit, "\"near_limit\""),
            (BoundFitKind::Exceeds, "\"exceeds\""),
        ] {
            assert_eq!(serde_json::to_string(&k).unwrap(), s);
            let back: BoundFitKind = serde_json::from_str(s).unwrap();
            assert_eq!(k, back);
        }
    }

    #[test]
    fn envelope_fit_worst_returns_max() {
        let ef = EnvelopeFit {
            scope: bf(BoundFitKind::Within, ""),
            reversibility: bf(BoundFitKind::NearLimit, ""),
            risk: bf(BoundFitKind::Within, ""),
            quality: bf(BoundFitKind::Within, ""),
        };
        assert_eq!(ef.worst(), BoundFitKind::NearLimit);
    }

    #[test]
    fn envelope_fit_exceeded_returns_first_bound_in_order() {
        let ef = EnvelopeFit {
            scope: bf(BoundFitKind::Within, ""),
            reversibility: bf(BoundFitKind::Exceeds, "migration"),
            risk: bf(BoundFitKind::Exceeds, "secrets"),
            quality: bf(BoundFitKind::Within, ""),
        };
        let (bound, bfit) = ef.exceeded().expect("should be exceeded");
        assert_eq!(bound, AuthorityBound::Reversibility);
        assert_eq!(bfit.note, "migration");
    }

    #[test]
    fn envelope_fit_exceeded_none_when_all_within() {
        let ef = EnvelopeFit {
            scope: bf(BoundFitKind::Within, ""),
            reversibility: bf(BoundFitKind::Within, ""),
            risk: bf(BoundFitKind::Within, ""),
            quality: bf(BoundFitKind::Within, ""),
        };
        assert!(ef.exceeded().is_none());
    }
}
