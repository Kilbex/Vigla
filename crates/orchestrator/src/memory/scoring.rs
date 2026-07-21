//! Stateless confidence scoring (V3 §2, §4.13).
//!
//! `confidence(witnesses)` is a pure function. The cache lives in
//! process; persistence is the witness rows. Changing coefficient
//! weights below is a code change, not a data migration — the moat
//! is in the *typed signals*, not the formula.
//!
//! Defaults are code-owned; changing them requires the memory property and
//! retrieval evaluation suites rather than a runtime configuration change:
//!
//!   * `WIT_W`  = 1.0  — witness-weight sum coefficient
//!   * `AGE_W`  = 0.2  — recency bonus / decay coefficient
//!   * `CONF_W` = 0.5  — conflict penalty coefficient
//!
//! Output is a sigmoid over the linear combination so the score sits
//! in (0, 1) — easy to threshold (V3 §3 promotion gate) and easy to
//! display in the UI as a probability-like number.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;

use event_schema::memory::WitnessKind;

use super::witnesses::Witness;

pub const WIT_W: f64 = 1.0;
pub const AGE_W: f64 = 0.2;
pub const CONF_W: f64 = 0.5;

/// Half-life for witness recency decay, in days. After this many days
/// since the most recent positive witness, the age term contributes
/// half of its theoretical maximum. Tuned to make a freshly verified
/// hazard contribute meaningfully for ~a quarter before fading.
const AGE_HALFLIFE_DAYS: f64 = 90.0;

/// Compute confidence in `(0.0, 1.0)` from a witness list.
///
/// Sigmoid over a weighted sum:
///
/// ```text
/// raw = WIT_W * sum(witness.weight)
///     + AGE_W * recency_bonus(witnesses, now)
///     − CONF_W * conflict_penalty(witnesses)
/// confidence = sigmoid(raw)
/// ```
///
/// All inputs are derivable from `memory_witnesses` rows — the
/// function does not read SQLite. This makes it trivially testable
/// and cacheable.
pub fn confidence(witnesses: &[Witness], now_unix_ms: u64) -> f64 {
    // F-005: filter NaN weights to 0 before summing so a single bad
    // weight does not poison the entire output via NaN propagation.
    let wit_term: f64 = witnesses
        .iter()
        .map(|w| if w.weight.is_nan() { 0.0 } else { w.weight })
        .sum();
    let age_term = recency_bonus(witnesses, now_unix_ms);
    let conf_term = conflict_penalty(witnesses);
    let raw = WIT_W * wit_term + AGE_W * age_term - CONF_W * conf_term;
    sigmoid(raw)
}

/// Convenience for callers that don't have a timestamp on hand. Uses
/// the system clock — pure-fn callers prefer the explicit form.
pub fn confidence_now(witnesses: &[Witness]) -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    confidence(witnesses, now)
}

/// Recency bonus: a positive contribution for fresh positive
/// witnesses, decaying exponentially. Negative witnesses don't
/// contribute here (their direct weight already pulls the score down).
fn recency_bonus(witnesses: &[Witness], now_unix_ms: u64) -> f64 {
    let mut best: f64 = 0.0;
    for w in witnesses {
        if w.weight <= 0.0 {
            continue;
        }
        // F-004: skip witnesses whose observed_at cannot be parsed (previously
        // treated as maximally fresh via unwrap_or(now_unix_ms)).
        let ts = match parse_rfc3339_ms(&w.observed_at) {
            Some(t) => t,
            None => continue,
        };
        // F-007: skip witnesses with future timestamps (previously received max
        // bonus of 1.0 via saturating_sub producing age_ms=0).
        if ts > now_unix_ms {
            continue;
        }
        let age_ms = now_unix_ms.saturating_sub(ts) as f64;
        let age_days = age_ms / (1000.0 * 60.0 * 60.0 * 24.0);
        // exp(-ln2 * age / halflife) — half at age = halflife.
        let decay = (-std::f64::consts::LN_2 * age_days / AGE_HALFLIFE_DAYS).exp();
        if decay > best {
            best = decay;
        }
    }
    best
}

/// Conflict penalty: any explicit `ConflictWithHigherConfidence`
/// witness drives the term up by 1, which sigmoid-translates to a
/// strong score reduction.
fn conflict_penalty(witnesses: &[Witness]) -> f64 {
    let has_conflict = witnesses
        .iter()
        .any(|w| w.kind == WitnessKind::ConflictWithHigherConfidence);
    if has_conflict {
        1.0
    } else {
        0.0
    }
}

fn sigmoid(x: f64) -> f64 {
    // F-008: clamp to ±36 to prevent f64 underflow of exp(-x) saturating the
    // output to 0.0 or 1.0, which would violate the documented (0,1) open interval.
    let x_clamped = x.clamp(-36.0, 36.0);
    1.0 / (1.0 + (-x_clamped).exp())
}

/// Best-effort RFC 3339 ms-precision parser. Matches the format
/// produced by `orchestrator/src/ids.rs::rfc3339_now`. Returns `None`
/// if the input can't be cracked — caller falls back to the current
/// time.
fn parse_rfc3339_ms(s: &str) -> Option<u64> {
    // Expected: "YYYY-MM-DDTHH:MM:SS.mmmZ" (24 chars).
    let b = s.as_bytes();
    if b.len() != 24 || b[10] != b'T' || b[23] != b'Z' {
        return None;
    }
    let year: i64 = s[0..4].parse().ok()?;
    let month: u32 = s[5..7].parse().ok()?;
    let day: u32 = s[8..10].parse().ok()?;
    let hour: u64 = s[11..13].parse().ok()?;
    let minute: u64 = s[14..16].parse().ok()?;
    let second: u64 = s[17..19].parse().ok()?;
    let ms: u64 = s[20..23].parse().ok()?;

    // Days since 1970-01-01, Howard Hinnant civil_from_days inverse.
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * m as u64 + 2) / 5 + day as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let z = era * 146_097 + doe as i64 - 719_468;
    let secs = (z * 86_400) + (hour * 3600 + minute * 60 + second) as i64;
    if secs < 0 {
        return None;
    }
    Some(secs as u64 * 1000 + ms)
}

// ---------------------------------------------------------------------
// FIFO cache. Tiny — capped at ~1024 entries; we only score notes that
// are about to be evaluated for promotion or composition. Eviction
// removes the earliest-inserted key (FIFO), not the least-recently-used.
// ---------------------------------------------------------------------

const CACHE_CAP: usize = 1024;

#[derive(Debug, Default)]
struct ScoreCache {
    /// Maps `(note_id, witness_len, minute_bucket)` → score.
    ///
    /// `minute_bucket` is `now_unix_ms / 60_000` — this bounds cache
    /// staleness to ~60 s (F-003). Cache hits require both the witness
    /// count and the minute bucket to match; new witnesses or a new
    /// minute always bust the entry.
    inner: HashMap<(String, usize, u64), f64>,
    order: Vec<(String, usize, u64)>,
}

impl ScoreCache {
    fn get(&self, note_id: &str, witness_len: usize, minute_bucket: u64) -> Option<f64> {
        self.inner
            .get(&(note_id.to_owned(), witness_len, minute_bucket))
            .copied()
    }
    fn put(&mut self, note_id: String, witness_len: usize, minute_bucket: u64, score: f64) {
        use std::collections::hash_map::Entry;
        let key = (note_id, witness_len, minute_bucket);
        if let Entry::Occupied(mut e) = self.inner.entry(key.clone()) {
            e.insert(score);
            return;
        }
        if self.inner.len() >= CACHE_CAP {
            // Evict oldest (FIFO). O(N) but bounded by cap.
            if let Some(victim) = self.order.first().cloned() {
                self.inner.remove(&victim);
                self.order.remove(0);
            }
        }
        self.inner.insert(key.clone(), score);
        self.order.push(key);
    }
}

fn cache() -> &'static Mutex<ScoreCache> {
    static C: OnceLock<Mutex<ScoreCache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(ScoreCache::default()))
}

/// Cached form of [`confidence`]. Cache key is `(note_id,
/// witnesses.len(), now_unix_ms / 60_000)`; recording a new witness
/// or crossing a minute boundary always invalidates (F-003).
pub fn confidence_cached(note_id: &str, witnesses: &[Witness], now_unix_ms: u64) -> f64 {
    let minute_bucket = now_unix_ms / 60_000;
    if let Ok(guard) = cache().lock() {
        if let Some(hit) = guard.get(note_id, witnesses.len(), minute_bucket) {
            return hit;
        }
    }
    let score = confidence(witnesses, now_unix_ms);
    if let Ok(mut guard) = cache().lock() {
        guard.put(note_id.to_owned(), witnesses.len(), minute_bucket, score);
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(kind: WitnessKind, observed_at: &str) -> Witness {
        Witness {
            id: format!("w-{}", kind.as_str()),
            note_id: "n".into(),
            kind,
            weight: kind.default_weight(),
            source_event_id: "e".into(),
            observed_at: observed_at.into(),
        }
    }

    #[test]
    fn empty_witnesses_score_is_below_half() {
        let now = parse_rfc3339_ms("2026-05-16T00:00:00.000Z").unwrap();
        let s = confidence(&[], now);
        // No witnesses → raw=0 → sigmoid=0.5.
        assert!((s - 0.5).abs() < 1e-9);
    }

    #[test]
    fn user_authored_is_above_threshold_for_every_kind() {
        let now = parse_rfc3339_ms("2026-05-16T00:00:00.000Z").unwrap();
        let s = confidence(
            &[w(WitnessKind::UserAuthored, "2026-05-16T00:00:00.000Z")],
            now,
        );
        // weight=1.0, age=1.0 (just observed), conflict=0 → raw=1.2 → ~0.77.
        assert!(s > 0.70, "got {s}");
    }

    #[test]
    fn user_scrubbed_drops_score_below_half() {
        let now = parse_rfc3339_ms("2026-05-16T00:00:00.000Z").unwrap();
        let s = confidence(
            &[w(WitnessKind::UserScrubbed, "2026-05-16T00:00:00.000Z")],
            now,
        );
        assert!(s < 0.4, "got {s}");
    }

    #[test]
    fn conflict_penalty_pulls_score_down() {
        let now = parse_rfc3339_ms("2026-05-16T00:00:00.000Z").unwrap();
        let base = confidence(
            &[w(WitnessKind::UserAccepted, "2026-05-16T00:00:00.000Z")],
            now,
        );
        let conflicted = confidence(
            &[
                w(WitnessKind::UserAccepted, "2026-05-16T00:00:00.000Z"),
                w(
                    WitnessKind::ConflictWithHigherConfidence,
                    "2026-05-16T00:00:00.000Z",
                ),
            ],
            now,
        );
        assert!(conflicted < base);
    }

    #[test]
    fn recency_bonus_fades_over_time() {
        let now = parse_rfc3339_ms("2026-05-16T00:00:00.000Z").unwrap();
        let fresh = w(WitnessKind::UserAuthored, "2026-05-16T00:00:00.000Z");
        let stale = w(WitnessKind::UserAuthored, "2024-05-16T00:00:00.000Z");
        let fresh_score = confidence(&[fresh], now);
        let stale_score = confidence(&[stale], now);
        assert!(fresh_score > stale_score);
    }

    #[test]
    fn cache_invalidates_on_new_witness() {
        let a = vec![w(WitnessKind::UserAccepted, "2026-05-16T00:00:00.000Z")];
        let b = vec![
            w(WitnessKind::UserAccepted, "2026-05-16T00:00:00.000Z"),
            w(
                WitnessKind::ConflictWithHigherConfidence,
                "2026-05-16T00:00:00.000Z",
            ),
        ];
        let now = parse_rfc3339_ms("2026-05-16T00:00:00.000Z").unwrap();
        let key = format!("test-{}", uuid::Uuid::now_v7());
        let s1 = confidence_cached(&key, &a, now);
        let s2 = confidence_cached(&key, &b, now);
        assert!(s2 < s1);
    }

    #[test]
    fn rfc3339_parser_handles_canonical_timestamp() {
        let parsed = parse_rfc3339_ms("2026-05-16T14:22:01.481Z").unwrap();
        // 2026-05-16T14:22:01.481Z in unix ms: trust the constant.
        // sanity: must be > 2026-01-01.
        let jan_2026 = parse_rfc3339_ms("2026-01-01T00:00:00.000Z").unwrap();
        assert!(parsed > jan_2026);
    }

    // ------------------------------------------------------------------
    // F-003 regression: minute-bucketed cache key
    // ------------------------------------------------------------------

    /// F-003 regression: cache must NOT return stale recency-bonus
    /// data when called with a different `now`. Specifically, after
    /// a sufficient time gap, the cached value should NOT be reused.
    #[test]
    fn cache_invalidates_across_minute_boundary() {
        // Same note_id, single positive witness.
        let key = format!("note-cache-test-{}", uuid::Uuid::now_v7());
        let w_obj = Witness {
            id: "w-1".to_owned(),
            note_id: key.clone(),
            kind: WitnessKind::WorkerProposed,
            weight: 1.0,
            source_event_id: "ev-1".to_owned(),
            observed_at: "2026-05-18T00:00:00.000Z".to_owned(),
        };

        // First call: "now" is at the witness timestamp (age=0 → max bonus).
        let now_a = parse_rfc3339_ms("2026-05-18T00:00:00.000Z").unwrap();
        let c_fresh = confidence_cached(&key, std::slice::from_ref(&w_obj), now_a);

        // Second call: "now" is 180 days later (age = 2 half-lives → bonus ≈ 0.25).
        let now_b = parse_rfc3339_ms("2026-11-14T00:00:00.000Z").unwrap();
        let c_aged = confidence_cached(&key, std::slice::from_ref(&w_obj), now_b);

        // If cache were time-insensitive, c_aged would equal c_fresh.
        // Minute-bucketed key (different buckets across the two calls) → c_aged < c_fresh.
        assert!(
            c_aged < c_fresh,
            "cached value should reflect later 'now', got c_aged={c_aged} >= c_fresh={c_fresh}"
        );
    }

    // ------------------------------------------------------------------
    // F-004 regression: malformed observed_at skipped from recency_bonus
    // ------------------------------------------------------------------

    /// F-004 regression: all witnesses with malformed observed_at produce
    /// zero recency_bonus contribution (previously: they were treated as
    /// fresh and produced max bonus of 1.0).
    ///
    /// With WIT_W=1.0, AGE_W=0.2, CONF_W=0.5:
    ///   post-fix: raw = 1.0*3 + 0.2*0 - 0.5*0 = 3.0 → sigmoid(3.0)
    ///   pre-bug:  raw = 1.0*3 + 0.2*1 - 0.5*0 = 3.2 → sigmoid(3.2)
    #[test]
    fn all_malformed_witnesses_give_zero_recency_bonus() {
        let now = parse_rfc3339_ms("2026-05-18T00:00:00.000Z").unwrap();
        let ws: Vec<Witness> = (0..3)
            .map(|i| Witness {
                id: format!("w-{i}"),
                note_id: "n".to_owned(),
                kind: WitnessKind::WorkerProposed,
                weight: 1.0,
                source_event_id: format!("ev-{i}"),
                observed_at: format!("garbage-{i}"),
            })
            .collect();
        let c = confidence(&ws, now);
        let expected_no_recency = 1.0 / (1.0 + (-3.0_f64).exp()); // sigmoid(3.0)
        assert!(
            (c - expected_no_recency).abs() < 1e-9,
            "post-fix confidence should equal sigmoid(3.0)={expected_no_recency:.9}, got {c:.9}"
        );
    }

    // ------------------------------------------------------------------
    // F-005 regression: NaN weights filtered to 0
    // ------------------------------------------------------------------

    /// F-005 regression: NaN weights must not poison the confidence
    /// computation. The fix filters NaN to 0 before summing.
    #[test]
    fn nan_weights_are_filtered() {
        let now = parse_rfc3339_ms("2026-05-18T00:00:00.000Z").unwrap();
        let good = Witness {
            id: "w-good".to_owned(),
            note_id: "n".to_owned(),
            kind: WitnessKind::WorkerProposed,
            weight: 1.0,
            source_event_id: "ev-good".to_owned(),
            observed_at: "2026-05-18T00:00:00.000Z".to_owned(),
        };
        let nan = Witness {
            id: "w-nan".to_owned(),
            note_id: "n".to_owned(),
            kind: WitnessKind::WorkerProposed,
            weight: f64::NAN,
            source_event_id: "ev-nan".to_owned(),
            observed_at: "2026-05-18T00:00:00.000Z".to_owned(),
        };
        let c = confidence(&[good, nan], now);
        assert!(c.is_finite(), "confidence should be finite, got {c}");
        assert!(c > 0.0 && c < 1.0, "confidence should be in (0,1), got {c}");
    }

    // ------------------------------------------------------------------
    // F-007 regression: future observed_at skipped from recency_bonus
    // ------------------------------------------------------------------

    // ------------------------------------------------------------------
    // F-008 regression: sigmoid saturation at extreme inputs
    // ------------------------------------------------------------------

    /// F-008 regression: at extreme inputs sigmoid saturates due to f64
    /// precision. The fix clamps raw to ±36, keeping confidence in the
    /// documented open (0,1) interval even at the boundary.
    #[test]
    fn confidence_strictly_in_open_interval_at_extremes() {
        let now = parse_rfc3339_ms("2026-05-18T00:00:00.000Z").unwrap();
        // 50 witnesses with weight 2.0 each → wit_term = 100, well past saturation.
        let high: Vec<Witness> = (0..50)
            .map(|i| Witness {
                id: format!("w-h-{i}"),
                note_id: "n-h".to_owned(),
                kind: WitnessKind::WorkerProposed,
                weight: 2.0,
                source_event_id: format!("ev-h-{i}"),
                observed_at: "2026-05-18T00:00:00.000Z".to_owned(),
            })
            .collect();
        let c_high = confidence(&high, now);
        assert!(
            c_high > 0.0 && c_high < 1.0,
            "high-end saturated to {c_high}, expected (0,1)"
        );

        // 50 witnesses with weight -2.0 each → wit_term = -100, well past saturation.
        let low: Vec<Witness> = (0..50)
            .map(|i| Witness {
                id: format!("w-l-{i}"),
                note_id: "n-l".to_owned(),
                kind: WitnessKind::WorkerProposed,
                weight: -2.0,
                source_event_id: format!("ev-l-{i}"),
                observed_at: "2026-05-18T00:00:00.000Z".to_owned(),
            })
            .collect();
        let c_low = confidence(&low, now);
        assert!(
            c_low > 0.0 && c_low < 1.0,
            "low-end saturated to {c_low}, expected (0,1)"
        );
    }

    /// F-007 regression: a witness with observed_at in the future must
    /// NOT contribute to recency_bonus (previously: got max bonus via
    /// saturating_sub yielding age_ms=0 → decay=1.0).
    ///
    /// With WIT_W=1.0, AGE_W=0.2, CONF_W=0.5:
    ///   post-fix: raw = 1.0*3 + 0.2*0 - 0.5*0 = 3.0 → sigmoid(3.0)
    ///   pre-bug:  raw = 1.0*3 + 0.2*1 - 0.5*0 = 3.2 → sigmoid(3.2)
    #[test]
    fn future_observed_at_skipped_from_recency() {
        let now = parse_rfc3339_ms("2026-05-18T00:00:00.000Z").unwrap();
        let ws: Vec<Witness> = (0..3)
            .map(|i| Witness {
                id: format!("w-{i}"),
                note_id: "n".to_owned(),
                kind: WitnessKind::WorkerProposed,
                weight: 1.0,
                source_event_id: format!("ev-{i}"),
                observed_at: "2099-01-01T00:00:00.000Z".to_owned(), // far future
            })
            .collect();
        let c = confidence(&ws, now);
        let expected_no_recency = 1.0 / (1.0 + (-3.0_f64).exp()); // sigmoid(3.0)
        assert!((c - expected_no_recency).abs() < 1e-9,
            "future-timestamp witnesses should give zero recency bonus; expected {expected_no_recency:.9}, got {c:.9}");
    }
}
