//! Heuristic residual-risk scorer.
//!
//! Pure function — no IO. Reads the mission's aggregated audit
//! report (or the last per-task audit when no post-integration
//! audit ran) plus a recovery-history summary, returns a closed
//! `RiskBand` for the inbox card.
//!
//! The boundaries below are intentionally conservative — bumping a
//! mission from `Low` to `Medium` is cheap (warning glyph in the
//! inbox), but bumping from `Medium` to `High` makes the completion
//! recommendation fail closed in [`crate::judgment::assemble_verdict`].

use crate::audit::AuditReport;
use crate::judgment::verdict::RiskBand;
use crate::judgment::RecoveryHistorySummary;

/// Heuristic residual-risk classifier.
///
/// Returns the conservative band given the mission's final audit
/// report (overall composite + security flags) and the aggregated
/// recovery history.
///
/// Boundaries:
///
///   * `Low` when `overall >= 0.85` AND zero security flags AND
///     recovery history is quiet (total < 3 occurrences).
///   * `High` when `overall < 0.7` OR more than one security flag.
///   * `Medium` otherwise (the residual band).
///
/// Recovery activity only ever pushes the band *up* — a busy
/// history bumps `Low` to `Medium`, never demotes `High`.
pub fn score_risk(report: &AuditReport, history: &RecoveryHistorySummary) -> RiskBand {
    let overall = report.overall;
    let flag_count = report.security_flags.len();
    let history_total: u32 = history.total_occurrences();

    let base = if overall >= 0.85 && flag_count == 0 {
        RiskBand::Low
    } else if overall < 0.7 || flag_count > 1 {
        RiskBand::High
    } else {
        RiskBand::Medium
    };

    // Soft bump: a busy recovery history is a sign of "we got
    // through it but it wasn't clean." Push Low → Medium; leave
    // Medium and High alone.
    if base == RiskBand::Low && history_total >= 3 {
        return RiskBand::Medium;
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::report::{SecurityFlag, SecurityFlagKind};

    fn report(overall: f64, flag_count: usize) -> AuditReport {
        AuditReport {
            overall,
            security_flags: (0..flag_count)
                .map(|i| SecurityFlag {
                    kind: SecurityFlagKind::SchemaMigration,
                    path: format!("migrations/{i}.sql"),
                    detail: "schema".into(),
                })
                .collect(),
            ..AuditReport::default()
        }
    }

    fn busy_history() -> RecoveryHistorySummary {
        let mut h = RecoveryHistorySummary::default();
        h.add_class("command_error", "retry", 3);
        h
    }

    /// Table-driven boundary tests. Format: (overall, flag_count, expected band, label).
    #[test]
    fn boundary_table_with_quiet_history() {
        let empty = RecoveryHistorySummary::default();
        let cases: &[(f64, usize, RiskBand, &str)] = &[
            (0.92, 0, RiskBand::Low, "clean high overall"),
            (0.85, 0, RiskBand::Low, "boundary at 0.85"),
            (0.849, 0, RiskBand::Medium, "just below 0.85"),
            (0.75, 0, RiskBand::Medium, "middle band"),
            (0.7, 0, RiskBand::Medium, "boundary at 0.7"),
            (0.699, 0, RiskBand::High, "just below 0.7"),
            (0.55, 0, RiskBand::High, "low overall"),
            (0.92, 1, RiskBand::Medium, "single flag bumps to medium"),
            (0.92, 2, RiskBand::High, "multi flag forces high"),
            (0.55, 5, RiskBand::High, "low overall stays high"),
        ];
        for (overall, flags, expected, label) in cases {
            let r = report(*overall, *flags);
            assert_eq!(
                score_risk(&r, &empty),
                *expected,
                "case {label}: overall={overall} flags={flags}",
            );
        }
    }

    #[test]
    fn busy_history_bumps_low_to_medium_only() {
        // Recovery activity can only push risk up, never down.
        let busy = busy_history();
        assert_eq!(score_risk(&report(0.92, 0), &busy), RiskBand::Medium);
        assert_eq!(score_risk(&report(0.75, 0), &busy), RiskBand::Medium);
        assert_eq!(score_risk(&report(0.55, 0), &busy), RiskBand::High);
    }
}
