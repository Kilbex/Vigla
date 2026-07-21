//! Gemini memory adapter (V3 §5).
//!
//! Renders into `GEMINI.md`. Gemini CLI's `/memory` and hierarchical
//! file loading both treat the file as plain Markdown, so the same
//! anchor-delimited block we use for Claude / Codex applies here too.
//! User content outside the anchors is preserved verbatim — Gemini's
//! `/memory show` will show both the anchor block and any pre-existing
//! content without surprise.

use event_schema::Vendor;

use super::super::adapter::{MemoryAdapter, RenderedSlot};
use super::claude::{ANCHOR_CLOSE, ANCHOR_OPEN};
use super::render::render_markdown_block;

pub const NATIVE_FILE: &str = "GEMINI.md";

const PREAMBLE: &str = "The following notes are curated by your supervisor for this task. \
Do not edit this block — edits are detected as drift and discarded.\n";

#[derive(Debug, Clone, Copy, Default)]
pub struct GeminiMemoryAdapter;

impl MemoryAdapter for GeminiMemoryAdapter {
    fn vendor(&self) -> Vendor {
        Vendor::Gemini
    }
    fn native_file_name(&self) -> &str {
        NATIVE_FILE
    }
    fn anchor_open(&self) -> &str {
        ANCHOR_OPEN
    }
    fn anchor_close(&self) -> &str {
        ANCHOR_CLOSE
    }
    fn render_block_body(&self, slots: &[RenderedSlot]) -> String {
        render_markdown_block(slots, PREAMBLE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn gemini_writes_to_gemini_md() {
        assert_eq!(GeminiMemoryAdapter.native_file_name(), "GEMINI.md");
    }

    #[test]
    fn gemini_vendor_tag_is_gemini() {
        assert_eq!(GeminiMemoryAdapter.vendor(), Vendor::Gemini);
    }

    #[test]
    fn render_includes_canonical_markers() {
        let s = slot("01J", "alpha", StandardNoteKind::Fact);
        let out = GeminiMemoryAdapter.render_block_body(std::slice::from_ref(&s));
        assert!(out.contains("fact"));
        assert!(out.contains("01J"));
        assert!(out.contains("alpha"));
    }
}
