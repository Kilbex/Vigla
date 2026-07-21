//! Promotion predicate and threshold lookup (V3 §3, §7.5).
//!
//! ## User-oracle shortcut (Tier-1 fix)
//!
//! Per the V3 design, the user is the highest-confidence signal we
//! have. Without a shortcut, a single `UserAuthored` witness produces
//! confidence ≈ 0.77 (sigmoid of 1.0 + recency bonus 0.2), which
//! clears `fact`/`hazard`/`procedure` thresholds but *not* the
//! `decision` threshold of 0.85. That breaks the product promise:
//! "talking to Vigla teaches it."
//!
//! Fix: when any `UserAuthored` witness is present, treat the
//! effective threshold as `min(τ_kind, 0.5)`. The user's authority
//! overrides the kind-asymmetric bar — they can author a `decision`
//! note and have it promote on the spot. Other witnesses still flow
//! through the normal threshold.

use sqlx::SqlitePool;

use super::error::MemoryError;
use super::hierarchy::{Note, NoteKind, NoteState, ScopeKind};
use super::witnesses::{has_user_authored, qualifying_count, Witness};

/// Confidence bar applied when a `UserAuthored` witness is present.
/// Low enough that one fresh user authorship clears it (single
/// `UserAuthored` ⇒ confidence ≈ 0.77), high enough that an
/// adversarial UserAuthored followed by overwhelming negative
/// witnesses can still hold promotion.
pub const USER_AUTHORED_FAST_PATH_BAR: f64 = 0.5;

/// Hard-coded fallback thresholds (V3 §3). Live values come from
/// `memory_taxonomy`; this table is the boot default and the safety
/// net if the row is missing.
pub fn fallback_threshold(kind: &NoteKind) -> f64 {
    match kind.as_str() {
        "hazard" => 0.55,
        "fact" => 0.70,
        "procedure" => 0.75,
        "decision" => 0.85,
        // Unknown kinds promote conservatively. Encourages the user
        // to register a threshold via taxonomy migration.
        _ => 0.90,
    }
}

/// Look up the live promotion threshold from `memory_taxonomy`.
/// Falls back to [`fallback_threshold`] if the row is missing —
/// taxonomy seeds ship in the migration so this only kicks in for
/// kinds added at runtime that someone forgot to register.
pub async fn promotion_threshold(pool: &SqlitePool, kind: &NoteKind) -> Result<f64, MemoryError> {
    let name = kind.as_str();
    let row: Option<(Option<f64>,)> = sqlx::query_as(
        "SELECT promote_threshold FROM memory_taxonomy WHERE category = 'kind' AND name = ?",
    )
    .bind(name)
    .fetch_optional(pool)
    .await?;

    Ok(row
        .and_then(|(v,)| v)
        .unwrap_or_else(|| fallback_threshold(kind)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionDecision {
    Promote,
    Hold { reasons: Vec<HoldReason> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HoldReason {
    WrongState(NoteState),
    BelowThreshold {
        confidence_x1000: u32,
        threshold_x1000: u32,
    },
    ScopeMissingValue,
    NoOutcomeOrHumanWitness,
}

/// Promotion predicate (V3 §7.5).
///
/// Conjuncts:
///   * `state == owned`
///   * At least one *qualifying* witness (UserAuthored / UserAccepted
///     / ReviewApproved / TestPassedAfterUse)
///   * `confidence ≥ effective_threshold`, where the effective
///     threshold drops to `USER_AUTHORED_FAST_PATH_BAR` when any
///     `UserAuthored` witness is present
///   * Scope value present when required (i.e. non-`repo` kinds)
///
/// Conflict-with-higher-confidence is checked outside the predicate
/// (see `reflection::blocking_conflict_exists`) so the predicate
/// stays a pure function of the note + witnesses + threshold.
pub fn predicate(
    note: &Note,
    confidence: f64,
    threshold: f64,
    witnesses: &[Witness],
) -> PromotionDecision {
    let mut reasons = Vec::new();

    if note.state != NoteState::Owned {
        reasons.push(HoldReason::WrongState(note.state));
    }

    let user_oracle = has_user_authored(witnesses);
    let effective_threshold = if user_oracle {
        USER_AUTHORED_FAST_PATH_BAR.min(threshold)
    } else {
        threshold
    };

    if confidence + f64::EPSILON < effective_threshold {
        reasons.push(HoldReason::BelowThreshold {
            confidence_x1000: (confidence * 1000.0).round() as u32,
            threshold_x1000: (effective_threshold * 1000.0).round() as u32,
        });
    }

    if note.scope.kind != ScopeKind::Repo && note.scope.value.as_deref().is_none_or(str::is_empty) {
        reasons.push(HoldReason::ScopeMissingValue);
    }

    if qualifying_count(witnesses) == 0 {
        reasons.push(HoldReason::NoOutcomeOrHumanWitness);
    }

    if reasons.is_empty() {
        PromotionDecision::Promote
    } else {
        PromotionDecision::Hold { reasons }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::hierarchy::{Note, NoteKind, NoteState, Scope, ScopeKind, StandardNoteKind};
    use event_schema::memory::WitnessKind;

    fn note_with(kind: StandardNoteKind, scope: Scope, state: NoteState) -> Note {
        Note {
            id: "n".into(),
            kind: NoteKind::Standard(kind),
            scope,
            body: "x".into(),
            body_hash: "h".into(),
            state,
            created_event_id: "e".into(),
            created_at: "t".into(),
            last_verified_at: None,
            title: None,
        }
    }

    fn witness(kind: WitnessKind) -> Witness {
        Witness {
            id: format!("w-{}", kind.as_str()),
            note_id: "n".into(),
            kind,
            weight: kind.default_weight(),
            source_event_id: "ev".into(),
            observed_at: "2026-05-16T00:00:00.000Z".into(),
        }
    }

    #[test]
    fn fallback_thresholds_match_taxonomy_seed() {
        assert!(
            (fallback_threshold(&NoteKind::Standard(StandardNoteKind::Hazard)) - 0.55).abs() < 1e-9
        );
        assert!(
            (fallback_threshold(&NoteKind::Standard(StandardNoteKind::Fact)) - 0.70).abs() < 1e-9
        );
        assert!(
            (fallback_threshold(&NoteKind::Standard(StandardNoteKind::Procedure)) - 0.75).abs()
                < 1e-9
        );
        assert!(
            (fallback_threshold(&NoteKind::Standard(StandardNoteKind::Decision)) - 0.85).abs()
                < 1e-9
        );
        assert!((fallback_threshold(&NoteKind::Other("lesson".into())) - 0.90).abs() < 1e-9);
    }

    #[test]
    fn promote_when_all_conjuncts_hold() {
        let note = note_with(
            StandardNoteKind::Hazard,
            Scope {
                kind: ScopeKind::Path,
                value: Some("adapters/claude".into()),
            },
            NoteState::Owned,
        );
        let ws = vec![witness(WitnessKind::UserAccepted)];
        assert_eq!(predicate(&note, 0.6, 0.55, &ws), PromotionDecision::Promote);
    }

    #[test]
    fn hold_when_below_threshold() {
        let note = note_with(
            StandardNoteKind::Decision,
            Scope {
                kind: ScopeKind::Repo,
                value: None,
            },
            NoteState::Owned,
        );
        let ws = vec![witness(WitnessKind::UserAccepted)];
        let d = predicate(&note, 0.50, 0.85, &ws);
        assert!(matches!(
            d,
            PromotionDecision::Hold { ref reasons }
              if reasons.iter().any(|r| matches!(r, HoldReason::BelowThreshold { .. }))
        ));
    }

    #[test]
    fn hold_when_not_owned() {
        let note = note_with(
            StandardNoteKind::Fact,
            Scope {
                kind: ScopeKind::Repo,
                value: None,
            },
            NoteState::Promoted,
        );
        let ws = vec![witness(WitnessKind::UserAccepted)];
        let d = predicate(&note, 0.99, 0.70, &ws);
        match d {
            PromotionDecision::Hold { reasons } => {
                assert!(reasons
                    .iter()
                    .any(|r| matches!(r, HoldReason::WrongState(NoteState::Promoted))));
            }
            _ => panic!("expected hold"),
        }
    }

    #[test]
    fn hold_when_scope_missing_value() {
        let note = note_with(
            StandardNoteKind::Hazard,
            Scope {
                kind: ScopeKind::Path,
                value: None,
            },
            NoteState::Owned,
        );
        let ws = vec![witness(WitnessKind::UserAccepted)];
        let d = predicate(&note, 0.99, 0.55, &ws);
        match d {
            PromotionDecision::Hold { reasons } => {
                assert!(reasons
                    .iter()
                    .any(|r| matches!(r, HoldReason::ScopeMissingValue)));
            }
            _ => panic!("expected hold"),
        }
    }

    #[test]
    fn hold_when_no_qualifying_witness() {
        let note = note_with(
            StandardNoteKind::Hazard,
            Scope {
                kind: ScopeKind::Path,
                value: Some("p".into()),
            },
            NoteState::Owned,
        );
        // Only a WorkerProposed (not qualifying) → still hold.
        let ws = vec![witness(WitnessKind::WorkerProposed)];
        let d = predicate(&note, 0.99, 0.55, &ws);
        match d {
            PromotionDecision::Hold { reasons } => {
                assert!(reasons
                    .iter()
                    .any(|r| matches!(r, HoldReason::NoOutcomeOrHumanWitness)));
            }
            _ => panic!("expected hold"),
        }
    }

    /// The Tier-1 promise: a single UserAuthored witness promotes a
    /// `decision` note (highest threshold). Single witness ⇒
    /// confidence ≈ 0.77; without the shortcut this would not clear
    /// the 0.85 bar.
    #[test]
    fn user_authored_promotes_decision_kind() {
        let note = note_with(
            StandardNoteKind::Decision,
            Scope {
                kind: ScopeKind::Repo,
                value: None,
            },
            NoteState::Owned,
        );
        let ws = vec![witness(WitnessKind::UserAuthored)];
        // Confidence below the kind threshold but above the user bar.
        assert_eq!(
            predicate(&note, 0.77, 0.85, &ws),
            PromotionDecision::Promote
        );
    }

    #[test]
    fn user_authored_alone_still_needs_above_user_bar() {
        let note = note_with(
            StandardNoteKind::Decision,
            Scope {
                kind: ScopeKind::Repo,
                value: None,
            },
            NoteState::Owned,
        );
        let ws = vec![witness(WitnessKind::UserAuthored)];
        // Drop confidence below the 0.5 user bar (e.g. heavy scrub
        // history) — must hold.
        let d = predicate(&note, 0.30, 0.85, &ws);
        assert!(matches!(d, PromotionDecision::Hold { .. }));
    }
}
