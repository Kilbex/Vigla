//! Pre-event secret scanner (V3 §4 threat #5).
//!
//! Runs *before* `MemoryProposed` is persisted. If any pattern fires,
//! the kernel emits `MemoryProposalRejected{ reason: Secret, ... }`
//! with a redacted preview, and the raw body is dropped on the floor.
//! The original secret never enters the event store at all — the
//! redacted preview is the only artifact downstream consumers see.
//!
//! Two detectors, both runtime-free:
//!
//!   1. **Fixed patterns** — common credential shapes that are dense
//!      enough to false-positive rarely (AWS access keys, GitHub PATs,
//!      OpenAI / Anthropic tokens, PEM headers).
//!   2. **Entropy floor** — Shannon entropy over sliding 20-character
//!      windows of base64/hex-like spans. A window ≥ 4.0 bits/char is
//!      treated as a probable opaque secret. The cap is log2(20)≈4.32,
//!      so 4.0 is the highest practically useful threshold for this
//!      window size.
//!
//! Patterns are deliberately built without an extra crate dep — we
//! hand-implement the few we need so `scanner.rs` stays a leaf module.
//! Repo operators can extend by editing `secrets/patterns.toml` (P6).

use blake3;

use super::error::MemoryError;
use event_schema::memory::{MemoryProposalRejected, ProposalRejectReason};

/// Per-span entropy threshold (bits/char). Tuned to fire on random
/// base64 / hex blobs but tolerate prose, code identifiers, and
/// reasonable Unicode. A 20-char window of fully-distinct characters
/// gives log2(20) ≈ 4.32 bits, so the threshold must sit below that
/// to catch credentials in a small window. Code identifiers stay
/// safe because whitespace and punctuation break the opaque-run
/// extraction *before* entropy is computed.
const ENTROPY_BITS_PER_CHAR_THRESHOLD: f64 = 4.0;

/// Length of the sliding window for entropy checks. Long enough to
/// resist false positives on short technical strings; short enough to
/// catch a typical 32-char key embedded in prose.
const ENTROPY_WINDOW: usize = 20;

/// Minimum length of a candidate token (longer than the window). Below
/// this we don't bother computing entropy.
const MIN_ENTROPY_CANDIDATE_LEN: usize = ENTROPY_WINDOW;

/// Minimum length of a contiguous hex run treated as a probable
/// secret. Matches the threat-model "32-char key" language. Pure hex
/// draws from a 16-symbol alphabet, so the entropy floor (4.0
/// bits/char) is mathematically unreachable for hex — a 20-char window
/// over 16 symbols peaks at ~3.92 bits/char. This length-based
/// detector is therefore what actually catches hex-encoded keys,
/// HMACs, and digests used as bearer tokens.
///
/// Tradeoff: a full 40-char git commit SHA or a 64-char file hash in a
/// note body will also match and cause the proposal to be rejected —
/// the intended, security-first default for a pre-persist secret
/// guard. Short SHAs (< 32 chars) and UUIDs (hyphens split the hex
/// into ≤ 12-char runs) are unaffected. Operators who need to record
/// long hashes can raise this constant.
const MIN_HEX_SECRET_LEN: usize = 32;

/// Outcome of a scan. `Clean` may still be promoted to rejection by
/// other policy (e.g. `Oversize`); the kernel chains them in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanResult {
    Clean,
    Match {
        reason: MatchReason,
        redacted: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchReason {
    Pattern(&'static str),
    Entropy,
}

/// Scan `body`. Returns the first hit; a clean scan walks the whole
/// input. Idempotent and pure.
pub fn scan(body: &str) -> ScanResult {
    if let Some((label, span)) = find_first_pattern(body) {
        return ScanResult::Match {
            reason: MatchReason::Pattern(label),
            redacted: redact_span(body, span),
        };
    }
    if let Some(span) = find_high_entropy_span(body) {
        return ScanResult::Match {
            reason: MatchReason::Entropy,
            redacted: redact_span(body, span),
        };
    }
    ScanResult::Clean
}

/// Build a `MemoryProposalRejected` event payload from a scan match.
/// Convenience for kernel call sites.
pub fn rejection_event(proposal_id: &str, result: &ScanResult) -> Option<MemoryProposalRejected> {
    match result {
        ScanResult::Match { redacted, .. } => Some(MemoryProposalRejected {
            proposal_id: proposal_id.to_owned(),
            reason: ProposalRejectReason::Secret,
            redacted_preview: redacted.clone(),
        }),
        ScanResult::Clean => None,
    }
}

/// Public for kernel callers that need a non-secret rejection
/// (oversize, malformed) — keeps the redaction format consistent.
pub fn redact_preview(body: &str, max_len: usize) -> String {
    if body.chars().count() <= max_len {
        body.to_owned()
    } else {
        let mut out: String = body.chars().take(max_len.saturating_sub(3)).collect();
        out.push_str("...");
        out
    }
}

// ---------------------------------------------------------------------
// Pattern detector
// ---------------------------------------------------------------------

/// Returns `(pattern_label, (start, end))` of the first hit. Hand-
/// rolled so we don't pull in a regex crate. Patterns are
/// substring-anchored and short — total cost is linear in `body.len()`.
fn find_first_pattern(body: &str) -> Option<(&'static str, (usize, usize))> {
    // Keep detector signatures out of one literal so repository-level secret
    // scanners do not mistake the scanner implementation for an embedded key.
    const PEM_BEGIN: &str = concat!("-----BEGIN PRIVATE ", "KEY-----");
    const PEM_END: &str = concat!("-----END PRIVATE ", "KEY-----");
    const RSA_PEM_BEGIN: &str = concat!("-----BEGIN RSA PRIVATE ", "KEY-----");
    const RSA_PEM_END: &str = concat!("-----END RSA PRIVATE ", "KEY-----");

    // AWS access key: AKIA + 16 uppercase alphanum.
    if let Some(span) = find_anchored(body, "AKIA", 16, |c| {
        c.is_ascii_uppercase() || c.is_ascii_digit()
    }) {
        return Some(("aws_access_key", span));
    }
    // GitHub PAT classic: ghp_ + 36 alphanum.
    if let Some(span) = find_anchored(body, "ghp_", 36, |c| c.is_ascii_alphanumeric()) {
        return Some(("github_pat", span));
    }
    // GitHub fine-grained PAT: github_pat_ + 22 + `_` + 59.
    if let Some(span) = find_anchored(body, "github_pat_", 80, |c| {
        c.is_ascii_alphanumeric() || c == '_'
    }) {
        return Some(("github_pat_fg", span));
    }
    // OpenAI: sk- + 20+ alphanum.
    if let Some(span) = find_anchored(body, "sk-", 20, |c| {
        c.is_ascii_alphanumeric() || c == '-' || c == '_'
    }) {
        // Exclude the Anthropic "sk-ant-" prefix to avoid double-matching;
        // we still want the Anthropic check below to fire.
        let (s, _) = span;
        if !body[s..].starts_with("sk-ant-") {
            return Some(("openai_key", span));
        }
    }
    // Anthropic: sk-ant- + 32+ alphanum/-/_.
    if let Some(span) = find_anchored(body, "sk-ant-", 32, |c| {
        c.is_ascii_alphanumeric() || c == '-' || c == '_'
    }) {
        return Some(("anthropic_key", span));
    }
    // PEM private key block. Match the header — the body is opaque.
    if let Some(start) = body.find(PEM_BEGIN) {
        let end = body[start..]
            .find(PEM_END)
            .map(|e| start + e + PEM_END.len())
            .unwrap_or(body.len());
        return Some(("pem_private_key", (start, end)));
    }
    // RSA PEM variant.
    if let Some(start) = body.find(RSA_PEM_BEGIN) {
        let end = body[start..]
            .find(RSA_PEM_END)
            .map(|e| start + e + RSA_PEM_END.len())
            .unwrap_or(body.len());
        return Some(("pem_rsa_key", (start, end)));
    }
    // Long contiguous hex run — hex-encoded keys / HMACs / digests used
    // as bearer tokens. The entropy detector cannot catch these because
    // hex's 16-symbol alphabet never reaches the 4.0 bits/char floor
    // (see MIN_HEX_SECRET_LEN).
    if let Some(span) = find_long_hex_run(body, MIN_HEX_SECRET_LEN) {
        return Some(("hex_secret", span));
    }
    None
}

/// Find the first maximal contiguous run of ASCII hex digits whose
/// length is ≥ `min_len`. Returns the `(start, end)` byte span of the
/// whole run. Runs are bounded by any non-hex byte (whitespace,
/// punctuation, or a non-hex letter), so embedded short hex (commit
/// prefixes) and hyphen-split UUIDs do not trip it.
fn find_long_hex_run(body: &str, min_len: usize) -> Option<(usize, usize)> {
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_hexdigit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
            i += 1;
        }
        if i - start >= min_len {
            return Some((start, i));
        }
    }
    None
}

/// Find an anchored pattern: a constant `prefix` immediately followed
/// by ≥ `min_tail` characters satisfying `pred`. Returns `(start,
/// end)` covering the entire matched span.
fn find_anchored(
    body: &str,
    prefix: &str,
    min_tail: usize,
    pred: impl Fn(char) -> bool,
) -> Option<(usize, usize)> {
    let prefix_len = prefix.len();
    let mut from = 0;
    while let Some(off) = body[from..].find(prefix) {
        let start = from + off;
        let tail_start = start + prefix_len;
        let mut end = tail_start;
        for c in body[tail_start..].chars() {
            if pred(c) {
                end += c.len_utf8();
            } else {
                break;
            }
        }
        if end - tail_start >= min_tail {
            return Some((start, end));
        }
        from = start + prefix_len.max(1);
    }
    None
}

// ---------------------------------------------------------------------
// Entropy detector
// ---------------------------------------------------------------------

/// Returns the span of the first opaque-looking high-entropy window.
/// "Opaque" means base64/hex-like — we explicitly do not flag prose,
/// even very dense prose, because Shannon entropy on plain English
/// text falls well below 4.5 bits/char.
fn find_high_entropy_span(body: &str) -> Option<(usize, usize)> {
    if body.len() < MIN_ENTROPY_CANDIDATE_LEN {
        return None;
    }
    // Step 1: extract candidate runs of opaque characters (no
    // whitespace, alnum / base64 / hex punctuation).
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !is_opaque_byte(bytes[i]) {
            i += 1;
            continue;
        }
        let run_start = i;
        while i < bytes.len() && is_opaque_byte(bytes[i]) {
            i += 1;
        }
        let run_end = i;
        if run_end - run_start < MIN_ENTROPY_CANDIDATE_LEN {
            continue;
        }
        // Step 2: slide a window across the run and check entropy.
        let run = &body[run_start..run_end];
        for start in 0..=run.len() - ENTROPY_WINDOW {
            let window = &run.as_bytes()[start..start + ENTROPY_WINDOW];
            if shannon_entropy(window) >= ENTROPY_BITS_PER_CHAR_THRESHOLD {
                // Redact the ENTIRE opaque run, not just the 20-char
                // window that tripped the floor. A longer token (e.g. a
                // 40-char base64 secret) would otherwise leak every byte
                // outside the triggering window into the stored preview.
                // The whole contiguous opaque run is suspect once any
                // window is high-entropy — this matches the whole-span
                // redaction the pattern and hex detectors already do.
                return Some((run_start, run_end));
            }
        }
    }
    None
}

fn is_opaque_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_')
}

fn shannon_entropy(window: &[u8]) -> f64 {
    let len = window.len() as f64;
    let mut counts = [0u32; 256];
    for &b in window {
        counts[b as usize] += 1;
    }
    let mut h = 0.0f64;
    for &c in counts.iter() {
        if c == 0 {
            continue;
        }
        let p = c as f64 / len;
        h -= p * p.log2();
    }
    h
}

// ---------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------

fn redact_span(body: &str, (start, end): (usize, usize)) -> String {
    let mut out = String::with_capacity(body.len());
    out.push_str(&body[..start]);
    let span = &body[start..end];
    let hash = blake3::hash(span.as_bytes());
    let mut short_hex = String::with_capacity(8);
    for b in &hash.as_bytes()[..4] {
        short_hex.push_str(&format!("{b:02x}"));
    }
    out.push_str(&format!("[REDACTED:{}:{}]", span.len(), short_hex));
    out.push_str(&body[end..]);
    out
}

// ---------------------------------------------------------------------
// Public bridge used by the kernel — converts a `ScanResult` to a
// (`MemoryError`, redacted preview) pair when rejection is the
// outcome. Kept here so the kernel doesn't grow its own scanner
// awareness.
// ---------------------------------------------------------------------

pub fn into_rejection(
    proposal_id: &str,
    result: ScanResult,
) -> Result<MemoryProposalRejected, MemoryError> {
    match result {
        ScanResult::Match { redacted, .. } => Ok(MemoryProposalRejected {
            proposal_id: proposal_id.to_owned(),
            reason: ProposalRejectReason::Secret,
            redacted_preview: redacted,
        }),
        ScanResult::Clean => Err(MemoryError::RowCorrupt(
            "into_rejection called with Clean result".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aws_access_key_caught_and_redacted() {
        let body = "deploy with AKIAIOSFODNN7EXAMPLE in env";
        match scan(body) {
            ScanResult::Match { reason, redacted } => {
                assert_eq!(reason, MatchReason::Pattern("aws_access_key"));
                assert!(!redacted.contains("AKIAIOSFODNN7EXAMPLE"));
                assert!(redacted.contains("[REDACTED:"));
            }
            other => panic!("expected match, got {other:?}"),
        }
    }

    #[test]
    fn github_pat_caught() {
        let body = format!("token: ghp_{}", "a".repeat(36));
        let r = scan(&body);
        assert!(matches!(
            r,
            ScanResult::Match {
                reason: MatchReason::Pattern("github_pat"),
                ..
            }
        ));
    }

    #[test]
    fn anthropic_key_preferred_over_openai_when_prefix_overlaps() {
        let body = format!("key sk-ant-{}", "x".repeat(40));
        match scan(&body) {
            ScanResult::Match {
                reason: MatchReason::Pattern(label),
                ..
            } => {
                assert_eq!(label, "anthropic_key");
            }
            other => panic!("expected anthropic match, got {other:?}"),
        }
    }

    #[test]
    fn pem_block_caught_with_full_span() {
        let body = "before\n-----BEGIN PRIVATE KEY-----\nMIIE...\n-----END PRIVATE KEY-----\nafter";
        match scan(body) {
            ScanResult::Match { reason, redacted } => {
                assert_eq!(reason, MatchReason::Pattern("pem_private_key"));
                assert!(!redacted.contains("MIIE"));
                assert!(redacted.starts_with("before"));
                assert!(redacted.ends_with("after"));
            }
            other => panic!("expected pem match, got {other:?}"),
        }
    }

    #[test]
    fn high_entropy_blob_caught() {
        // Mixed alphabet, random-looking, 32 chars — well above the
        // 4.5 bits/char threshold.
        let body = "session=Xq7r2L9pNAv8KsCm3eBdYzWfHJ4tT0gE before";
        let r = scan(body);
        match r {
            ScanResult::Match {
                reason: MatchReason::Entropy,
                redacted,
            } => {
                assert!(redacted.contains("[REDACTED:"));
            }
            other => panic!("expected entropy match, got {other:?}"),
        }
    }

    #[test]
    fn long_entropy_secret_is_fully_redacted_not_just_the_window() {
        // A 40-char opaque token — longer than the 20-char entropy
        // window. The whole run must be redacted; regression against the
        // detector redacting only the first window and leaking the tail.
        let token = "Xq7r2L9pNAv8KsCm3eBdYzWfHJ4tT0gEaB3cD5fG"; // gitleaks:allow
        assert_eq!(token.len(), 40);
        let body = format!("value {token} rest");
        match scan(&body) {
            ScanResult::Match {
                reason: MatchReason::Entropy,
                redacted,
            } => {
                assert!(!redacted.contains(token), "full token leaked: {redacted}");
                // The bytes past the first 20-char window must not survive.
                assert!(
                    !redacted.contains(&token[20..]),
                    "token tail leaked past the entropy window: {redacted}"
                );
                assert!(redacted.starts_with("value "));
                assert!(redacted.ends_with(" rest"));
            }
            other => panic!("expected entropy match, got {other:?}"),
        }
    }

    #[test]
    fn hex_secret_is_caught() {
        // A 64-char hex token (e.g. a hex-encoded key / HMAC / digest
        // used as a bearer token) embedded in prose. Pure hex draws
        // from a 16-symbol alphabet, so a 20-char entropy window maxes
        // at ~3.92 bits/char — below the 4.0 floor — and no fixed
        // credential prefix matches a bare hex string. It must still be
        // caught (threat #5: "a 32-char key embedded in prose").
        let body =
            "auth: bearer 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08 ok";
        match scan(body) {
            ScanResult::Match { redacted, .. } => {
                assert!(!redacted.contains("9f86d081884c7d659a2feaa0c55ad015"));
                assert!(redacted.contains("[REDACTED:"));
                assert!(redacted.starts_with("auth: bearer "));
                assert!(redacted.ends_with(" ok"));
            }
            other => panic!("expected hex secret to be caught, got {other:?}"),
        }
    }

    #[test]
    fn english_prose_is_clean() {
        let body = "Build with `cargo build --workspace`. Tests live under \
                    `orchestrator/src/memory/`. Run `cargo test -p vigla-orchestrator` \
                    to verify the suite before pushing.";
        assert_eq!(scan(body), ScanResult::Clean);
    }

    #[test]
    fn short_strings_below_window_are_ignored() {
        // Under entropy window — clean, even if individually random.
        let body = "abc xyz 123";
        assert_eq!(scan(body), ScanResult::Clean);
    }

    #[test]
    fn redaction_keeps_surrounding_bytes_intact() {
        let body = "prefix AKIAIOSFODNN7EXAMPLE suffix";
        if let ScanResult::Match { redacted, .. } = scan(body) {
            assert!(redacted.starts_with("prefix "));
            assert!(redacted.ends_with(" suffix"));
        } else {
            panic!("expected match");
        }
    }

    /// F-002 regression: removing the redundant prefix_len parameter
    /// from find_anchored must not break the AWS credential detector
    /// (which depends on correct tail offset computation).
    #[test]
    fn aws_key_detection_after_find_anchored_refactor() {
        let body = "config: AKIAIOSFODNN7EXAMPLE in the env";
        match scan(body) {
            ScanResult::Match {
                reason: MatchReason::Pattern(label),
                redacted,
            } => {
                assert!(
                    label.contains("aws") || label.contains("AKIA"),
                    "expected an AWS-labeled match, got label={label}"
                );
                assert!(redacted.contains("[REDACTED:"), "missing redaction marker");
            }
            other => panic!("expected Match{{Pattern}}, got {other:?}"),
        }
    }

    #[test]
    fn shannon_entropy_floors_and_peaks() {
        // All same char → 0 bits.
        let zero = shannon_entropy(b"aaaaaaaaaaaaaaaaaaaa");
        assert!(zero < 0.01);
        // 16 distinct chars uniformly distributed → 4 bits/char.
        let bytes: Vec<u8> = (0..16)
            .flat_map(|i| std::iter::repeat_n(b'a' + i, 1))
            .collect();
        // 16 chars, all distinct. Need 20 for the window — pad with
        // 4 additional distinct.
        let mut bytes = bytes;
        bytes.extend_from_slice(b"WXYZ");
        let high = shannon_entropy(&bytes);
        assert!(high > 4.0);
    }
}
