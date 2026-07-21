//! Worker memory intent vocabulary + a pure line-level parser.
//!
//! Workers emit memory operations as JSON lines of the shape
//!
//! ```jsonc
//! { "vigla_memory": { "type": "propose", "kind": "hazard",
//!                           "scope": { "kind": "path", "value": "..." },
//!                           "body": "...",
//!                           "derived_from": [ "..." ],
//!                           "evidence_event_ids": [ "..." ] } }
//! ```
//!
//! The vendor CLI wraps that text in its own envelope (Claude
//! stream-json, Codex `agent_message`, Gemini text). Each adapter
//! extracts the inner assistant text and feeds it to this module's
//! [`extract_intents`] helper, which scans the text *line by line*
//! and returns every well-formed intent it finds.
//!
//! Tier-2D ships `propose` only. The wire schema reserves `fetch` and
//! `correct` types via an `untagged` fallback so older builds don't
//! crash when newer workers start emitting them.
//!
//! # Design choices
//!
//! - **Line-based**: V3 §5 specifies one JSON object per line. We
//!   parse per line; non-JSON lines are dropped fast (cheap `trim` +
//!   `starts_with('{')` short-circuit). False-positives are virtually
//!   impossible because the marker `"vigla_memory"` is unique
//!   across the Vigla vocabulary.
//! - **Tolerant**: if a line has `"vigla_memory"` but the inner
//!   shape is wrong, we drop it silently. The kernel's `on_proposal`
//!   surface is the wide validation gate — adapters only see
//!   already-typed input.
//! - **Pure**: no I/O, no async, no globals. Trivially fuzz-testable.

use serde::{Deserialize, Serialize};

/// Top-level worker memory intent. Internally tagged on `"type"`.
/// Unknown future variants (e.g. `fetch`, `correct`) fail to
/// deserialize and are silently dropped by the line parser's
/// `Err(_) => None` arm — preserving forward-compat for older adapter
/// builds running against newer workers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MemoryIntent {
    Propose(ProposeIntent),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposeIntent {
    /// Note kind in taxonomy string form (e.g. `"hazard"`). The
    /// kernel side maps this to its typed `NoteKind` and rejects
    /// unknown values — adapters only carry the string.
    pub kind: String,
    pub scope: ScopeIntent,
    pub body: String,
    /// Provenance trail (V3 §4 threat #2). Each entry is an opaque
    /// source identifier (`"worktree:src/x.rs:42"`, `"url:..."`, ...).
    #[serde(default)]
    pub derived_from: Vec<String>,
    /// Worker event ids the proposal is causally tied to. Used by the
    /// kernel for replay only — adapters don't interpret them.
    #[serde(default)]
    pub evidence_event_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeIntent {
    pub kind: String,
    #[serde(default)]
    pub value: Option<String>,
}

/// Wire envelope: `{ "vigla_memory": { ... } }`. Workers always
/// emit at the top level on their own line.
#[derive(Debug, Clone, Deserialize)]
struct EnvelopeWire {
    vigla_memory: MemoryIntent,
}

/// Extract every memory intent from `text`. `text` may be a single
/// line, an assistant-message body with embedded newlines, or even
/// the full stdout of a vendor CLI — we scan line by line and skip
/// anything that isn't a JSON object starting with `{`.
///
/// Empty input returns an empty vec. Order is preserved.
pub fn extract_intents(text: &str) -> Vec<MemoryIntent> {
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(intent) = parse_line(line) {
            out.push(intent);
        }
    }
    out
}

/// Parse a single line. Returns `Some` only if the line is a JSON
/// object containing the `"vigla_memory"` key and the inner
/// payload deserialises to a known [`MemoryIntent`] variant.
pub fn parse_line(line: &str) -> Option<MemoryIntent> {
    let trimmed = line.trim();
    // Cheap short-circuit — almost every adapter line we see is not
    // a JSON object, so we don't pay the serde cost on those.
    if !trimmed.starts_with('{') {
        return None;
    }
    if !trimmed.contains("\"vigla_memory\"") {
        return None;
    }
    match serde_json::from_str::<EnvelopeWire>(trimmed) {
        Ok(env) => Some(env.vigla_memory),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn propose_line(body: &str) -> String {
        serde_json::json!({
            "vigla_memory": {
                "type": "propose",
                "kind": "hazard",
                "scope": { "kind": "repo" },
                "body": body,
                "derived_from": ["worktree:src/x.rs:42"],
                "evidence_event_ids": []
            }
        })
        .to_string()
    }

    #[test]
    fn empty_text_yields_no_intents() {
        assert!(extract_intents("").is_empty());
        assert!(extract_intents("   \n  ").is_empty());
    }

    #[test]
    fn parses_propose_intent_from_canonical_line() {
        let line = propose_line("Resume tokens are host-bound.");
        let intents = extract_intents(&line);
        assert_eq!(intents.len(), 1);
        let MemoryIntent::Propose(p) = &intents[0];
        assert_eq!(p.kind, "hazard");
        assert_eq!(p.scope.kind, "repo");
        assert!(p.scope.value.is_none());
        assert_eq!(p.body, "Resume tokens are host-bound.");
        assert_eq!(p.derived_from, vec!["worktree:src/x.rs:42".to_string()]);
    }

    #[test]
    fn skips_non_json_prose_lines() {
        let mixed = format!(
            "Here is my analysis.\nI propose the following:\n{}\nLet me know.",
            propose_line("x")
        );
        let intents = extract_intents(&mixed);
        assert_eq!(intents.len(), 1);
    }

    #[test]
    fn ignores_json_without_vigla_memory_key() {
        let line = "{\"other\":\"thing\"}";
        assert!(extract_intents(line).is_empty());
    }

    #[test]
    fn ignores_malformed_envelope_silently() {
        let line = "{\"vigla_memory\":{\"type\":\"propose\"}}"; // missing required fields
        assert!(extract_intents(line).is_empty());
    }

    #[test]
    fn ignores_unknown_intent_type() {
        let line = "{\"vigla_memory\":{\"type\":\"unknown_future_variant\"}}";
        assert!(extract_intents(line).is_empty());
    }

    #[test]
    fn defaults_missing_optional_fields() {
        let line = serde_json::json!({
            "vigla_memory": {
                "type": "propose",
                "kind": "fact",
                "scope": { "kind": "vendor", "value": "claude" },
                "body": "x"
            }
        })
        .to_string();
        let intents = extract_intents(&line);
        assert_eq!(intents.len(), 1);
        let MemoryIntent::Propose(p) = &intents[0];
        assert!(p.derived_from.is_empty());
        assert!(p.evidence_event_ids.is_empty());
        assert_eq!(p.scope.value.as_deref(), Some("claude"));
    }

    #[test]
    fn parses_multiple_intents_from_separate_lines() {
        let two = format!("{}\n{}", propose_line("first"), propose_line("second"));
        let intents = extract_intents(&two);
        assert_eq!(intents.len(), 2);
    }

    #[test]
    fn whitespace_around_json_is_tolerated() {
        let padded = format!("    {}    ", propose_line("x"));
        assert_eq!(extract_intents(&padded).len(), 1);
    }

    #[test]
    fn marker_substring_in_prose_does_not_match() {
        // Prose mention of the marker without surrounding JSON.
        let line = "the worker can emit vigla_memory blocks";
        assert!(extract_intents(line).is_empty());
    }

    #[test]
    fn parse_line_short_circuits_on_non_json_prefix() {
        // The cheap short-circuit path: assertions about behavior are
        // observable via Option<MemoryIntent>, but the perf gate is
        // that we never invoke serde on these.
        assert!(parse_line("not json").is_none());
        assert!(parse_line("// comment").is_none());
        assert!(parse_line("[1,2,3]").is_none());
    }
}
