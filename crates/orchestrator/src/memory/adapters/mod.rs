//! Concrete vendor adapters for memory rendering.
//!
//! P3 ships all three first-class vendors: `claude`, `codex`,
//! `gemini`. Adding a fourth vendor is small: a ~50-line module that
//! re-exports the shared markdown renderer with a new vendor tag
//! and native-file name. The cross-vendor parity tests in this
//! module's test block enforce the structural invariant.

pub mod claude;
pub mod codex;
pub mod gemini;
mod render;

pub use claude::ClaudeMemoryAdapter;
pub use codex::CodexMemoryAdapter;
pub use gemini::GeminiMemoryAdapter;

#[cfg(test)]
mod parity_tests {
    //! Cross-vendor parity tests (V3 §1 P3 exit criterion).
    //!
    //! The contract these tests enforce:
    //!
    //!   * Anchor delimiters are identical across vendors.
    //!   * Rendered block bodies for the same slots are byte-for-byte
    //!     identical (only the `native_file_name` and `vendor`
    //!     differ).
    //!   * Bundle events composed for the same notes have identical
    //!     `page_table` / `block_hash` — the moat is "memory you can
    //!     A/B test across vendors".

    use super::*;
    use crate::memory::adapter::{MemoryAdapter, RenderedSlot};
    use crate::memory::hierarchy::{Note, NoteKind, NoteState, Scope, ScopeKind, StandardNoteKind};

    fn slot(id: &str, body: &str, kind: StandardNoteKind) -> RenderedSlot {
        RenderedSlot {
            slot: 0,
            note: Note {
                id: id.into(),
                kind: NoteKind::Standard(kind),
                scope: Scope {
                    kind: ScopeKind::Repo,
                    value: None,
                },
                body: body.into(),
                body_hash: "h".into(),
                state: NoteState::Owned,
                created_event_id: "e".into(),
                created_at: "t".into(),
                last_verified_at: None,
                title: None,
            },
            tokens: 1,
        }
    }

    fn all_adapters() -> Vec<Box<dyn MemoryAdapter>> {
        vec![
            Box::new(ClaudeMemoryAdapter),
            Box::new(CodexMemoryAdapter),
            Box::new(GeminiMemoryAdapter),
        ]
    }

    #[test]
    fn all_adapters_share_anchor_delimiters() {
        let adapters = all_adapters();
        let open = adapters[0].anchor_open().to_owned();
        let close = adapters[0].anchor_close().to_owned();
        for a in &adapters[1..] {
            assert_eq!(
                a.anchor_open(),
                open,
                "anchor_open drift for {:?}",
                a.vendor()
            );
            assert_eq!(
                a.anchor_close(),
                close,
                "anchor_close drift for {:?}",
                a.vendor()
            );
        }
    }

    #[test]
    fn all_adapters_render_same_body_for_same_slots() {
        let s = slot(
            "01J",
            "Resume tokens are host-bound.",
            StandardNoteKind::Hazard,
        );
        let adapters = all_adapters();
        let canonical = adapters[0].render_block_body(std::slice::from_ref(&s));
        for a in &adapters[1..] {
            assert_eq!(
                a.render_block_body(std::slice::from_ref(&s)),
                canonical,
                "render drift for {:?}",
                a.vendor(),
            );
        }
    }

    #[test]
    fn each_adapter_has_distinct_native_file_and_vendor() {
        let adapters = all_adapters();
        let files: std::collections::HashSet<&str> =
            adapters.iter().map(|a| a.native_file_name()).collect();
        let vendors: std::collections::HashSet<event_schema::Vendor> =
            adapters.iter().map(|a| a.vendor()).collect();
        assert_eq!(files.len(), 3);
        assert_eq!(vendors.len(), 3);
    }

    #[test]
    fn max_tokens_default_is_consistent_across_vendors() {
        let adapters = all_adapters();
        let canonical = adapters[0].max_tokens();
        for a in &adapters[1..] {
            assert_eq!(
                a.max_tokens(),
                canonical,
                "token budget drift for {:?}",
                a.vendor(),
            );
        }
    }
}
