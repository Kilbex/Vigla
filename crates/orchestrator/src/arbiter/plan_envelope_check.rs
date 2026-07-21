//! QC-3 plan envelope gate.
//!
//! Pure function over an [`EnvelopeFit`]: if any bound is
//! `Exceeds`, returns the first such bound (in
//! Scope → Reversibility → Risk → Quality order) wrapped in a
//! typed [`EnvelopeTrip`] that the FE renders as a banner.
//! Otherwise returns `None` and the mission proceeds.
//!
//! Why a dedicated module: `mission_loop.rs` is already long, and
//! the gate's return type needs to flow through specta to the FE.
//! Keeping it here means the FE can pattern-match on
//! [`EnvelopeTrip`] without pulling in the rest of `mission_loop`.

use crate::arbiter::{AuthorityBound, SuggestedUserAction};
use crate::mission_event::EnvelopeFit;
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct EnvelopeTrip {
    pub bound: AuthorityBound,
    pub note: String,
    pub suggested_user_action: SuggestedUserAction,
}

/// If any bound is `Exceeds`, return the first tripped bound's
/// trip metadata. Otherwise `None`.
pub fn check(envelope_fit: &EnvelopeFit) -> Option<EnvelopeTrip> {
    envelope_fit.exceeded().map(|(bound, bf)| EnvelopeTrip {
        bound,
        note: bf.note.clone(),
        suggested_user_action: SuggestedUserAction::ReviewPlan,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mission_event::{BoundFit, BoundFitKind};

    fn bf(fit: BoundFitKind, note: &str) -> BoundFit {
        BoundFit {
            fit,
            note: note.into(),
        }
    }

    fn all_within() -> EnvelopeFit {
        EnvelopeFit {
            scope: bf(BoundFitKind::Within, ""),
            reversibility: bf(BoundFitKind::Within, ""),
            risk: bf(BoundFitKind::Within, ""),
            quality: bf(BoundFitKind::Within, ""),
        }
    }

    #[test]
    fn check_returns_none_when_all_within() {
        assert!(check(&all_within()).is_none());
    }

    #[test]
    fn check_returns_none_when_only_near_limit() {
        let mut ef = all_within();
        ef.risk = bf(BoundFitKind::NearLimit, "noted");
        assert!(check(&ef).is_none());
    }

    #[test]
    fn check_returns_first_exceeds_in_canonical_order() {
        // Multiple bounds Exceeds — canonical order is
        // Scope → Reversibility → Risk → Quality.
        let mut ef = all_within();
        ef.risk = bf(BoundFitKind::Exceeds, "risk note");
        ef.reversibility = bf(BoundFitKind::Exceeds, "rev note");
        let trip = check(&ef).expect("should trip");
        assert_eq!(trip.bound, AuthorityBound::Reversibility);
        assert_eq!(trip.note, "rev note");
        assert_eq!(trip.suggested_user_action, SuggestedUserAction::ReviewPlan);
    }

    #[test]
    fn check_returns_quality_when_only_quality_exceeds() {
        let mut ef = all_within();
        ef.quality = bf(BoundFitKind::Exceeds, "no tests");
        let trip = check(&ef).expect("should trip");
        assert_eq!(trip.bound, AuthorityBound::Quality);
        assert_eq!(trip.note, "no tests");
    }
}
