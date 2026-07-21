//! Shared, vendor-neutral detector for quota / rate-limit exhaustion
//! in a single line of free-text CLI output.
//!
//! Every adapter (the structured claude/codex/gemini ones and the
//! raw-log stubs for antigravity/kiro/copilot) routes its free-text
//! matching through here, so quota-pause behaviour is uniform across
//! vendors and the false-positive surface lives in exactly one place.
//!
//! The detector matches explicit exhaustion **phrases** and **bounded
//! tokens** rather than loose substrings. That distinction is the
//! whole point: a bare `contains("429")` fires on `artifact-429.bin`
//! and `line 429`, and a bare `contains("usage limit")` fires on the
//! purely informational `current usage limit is 1M tokens`. Those
//! false positives would pause a mission for the vendor's full
//! quota-window fallback (up to hours) with no real wall behind it.

/// True iff `line` looks like a vendor quota / rate-limit *exhaustion*
/// message (not a mere mention of a limit). Case-insensitive.
pub fn is_quota_exhaustion_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();

    // Tier 1 — phrases that state exhaustion outright. These are
    // specific enough that a plain substring match is safe.
    const EXPLICIT: &[&str] = &[
        "rate limit exceeded",
        "rate_limit_exceeded",
        "usage limit exceeded",
        "usage_limit_exceeded",
        "quota exceeded",
        "quota_exceeded",
        "too many requests",
    ];
    if EXPLICIT.iter().any(|p| lower.contains(p)) {
        return true;
    }

    // Gemini's canonical gRPC code — matched as a whole token so the
    // benign identifier `resource_exhausted_path` does not trip it.
    if contains_word_token(&lower, "resource_exhausted") {
        return true;
    }

    // HTTP 429 — only as a standalone status number (not `4290`, not
    // `artifact-429.bin`) AND alongside an HTTP/rate context word, so a
    // stray `line 429` does not pause a mission. `"request"` is
    // deliberately NOT a context word: it is a substring of the very
    // common structured-log field name `request_id`, so a benign
    // `assigned request_id=429` would otherwise pause the worker for the
    // vendor's full fallback window. A genuine "429 too many requests" is
    // already caught by the EXPLICIT tier above.
    if contains_word_token(&lower, "429")
        && [
            "http", "status", "rate", "quota", "retry", "too many", "limit",
        ]
        .iter()
        .any(|w| lower.contains(w))
    {
        return true;
    }

    // A bare "usage limit" only counts when the line also says it was
    // hit — `current usage limit is 1M tokens` is informational, not an
    // exhaustion event.
    if lower.contains("usage limit")
        && ["exceed", "reached", "hit", "exhaust"]
            .iter()
            .any(|w| lower.contains(w))
    {
        return true;
    }

    false
}

/// True iff `needle` occurs in `haystack` bounded on both sides by a
/// non-"word" byte (or a string edge). A word byte is an ASCII
/// alphanumeric or `_` — so `resource_exhausted` does not match inside
/// `resource_exhausted_path`, and `429` does not match inside `4290` or
/// `v429`. `needle` is assumed ASCII (all call sites pass ASCII), so
/// every `find` offset lands on a char boundary; non-ASCII neighbours
/// (bytes ≥ 0x80) count as boundaries.
fn contains_word_token(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let bytes = haystack.as_bytes();
    let mut from = 0;
    while let Some(rel) = haystack[from..].find(needle) {
        let start = from + rel;
        let end = start + needle.len();
        let before_ok = start == 0 || !is_word_byte(bytes[start - 1]);
        let after_ok = end == bytes.len() || !is_word_byte(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::is_quota_exhaustion_line as q;

    #[test]
    fn matches_explicit_exhaustion_phrases() {
        assert!(q("API error: rate limit exceeded"));
        assert!(q(
            "ERROR: API request failed with status 429: Too Many Requests"
        ));
        assert!(q("Error: Quota exceeded. Try again later."));
        assert!(q("HTTP 429: rate limit exceeded"));
        assert!(q("error code usage_limit_exceeded for this account"));
        assert!(q("monthly usage limit exceeded"));
    }

    #[test]
    fn matches_resource_exhausted_token() {
        assert!(q("RESOURCE_EXHAUSTED: quota exceeded for the day"));
        assert!(q("RESOURCE_EXHAUSTED"));
        assert!(q("grpc status: resource_exhausted"));
    }

    #[test]
    fn matches_bare_429_only_with_http_context() {
        assert!(q("got HTTP 429"));
        assert!(q("server returned status 429"));
        assert!(q("rate limited (429)"));
    }

    #[test]
    fn matches_usage_limit_only_when_hit() {
        assert!(q("you have reached your usage limit"));
        assert!(q("you've hit your usage limit for today"));
    }

    #[test]
    fn ignores_informational_usage_limit_mention() {
        assert!(!q("Note: current usage limit is 1M tokens"));
        assert!(!q("your usage limit is 500 requests per minute"));
    }

    #[test]
    fn ignores_429_inside_filenames_numbers_and_logs() {
        assert!(!q("wrote build/artifact-429.bin"));
        assert!(!q("line 429 of src/main.rs"));
        assert!(!q("downloaded 4290 packages"));
        assert!(!q("completed in 429ms"));
    }

    #[test]
    fn ignores_resource_exhausted_inside_identifiers() {
        assert!(!q("Test resource_exhausted_path passed"));
        assert!(!q("running resource_exhausted_handler"));
    }

    #[test]
    fn ignores_429_next_to_request_id_field() {
        // `request_id` is a ubiquitous structured-log field; a value that
        // happens to be 429 must NOT be read as a quota wall (it would
        // pause the worker for the vendor's whole fallback window).
        assert!(!q("assigned request_id=429"));
        assert!(!q("completed request 429 of 500"));
    }

    #[test]
    fn ignores_ordinary_errors() {
        assert!(!q("tool execution failed"));
        assert!(!q("compilation error: missing semicolon"));
        assert!(!q(""));
    }
}
