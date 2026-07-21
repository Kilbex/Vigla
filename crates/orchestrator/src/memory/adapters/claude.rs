//! Claude memory adapter (V3 §5).
//!
//! Renders the Memory Kernel's anchor block into `CLAUDE.md`. The
//! block is delimited by HTML comments so it sits invisibly in
//! Markdown views and so Claude's own startup loader treats the
//! whole file uniformly. User content outside the anchors is never
//! touched.

use event_schema::Vendor;

use super::super::adapter::{MemoryAdapter, RenderedSlot};
use super::render::render_markdown_block;

/// Versioned anchor delimiter — a future format ratchet can move
/// `v1 → v2` while remaining able to recognise old blocks.
pub const ANCHOR_OPEN: &str = "<!-- vigla:memory:begin v1 -->";
pub const ANCHOR_CLOSE: &str = "<!-- vigla:memory:end -->";
pub const NATIVE_FILE: &str = "CLAUDE.md";

const PREAMBLE: &str = "The following notes are curated by your supervisor for this task. \
Do not edit this block — edits are detected as drift and discarded.\n";

#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeMemoryAdapter;

impl MemoryAdapter for ClaudeMemoryAdapter {
    fn vendor(&self) -> Vendor {
        Vendor::Claude
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

    fn note(id: &str, body: &str, kind: StandardNoteKind) -> Note {
        Note {
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
        }
    }

    #[test]
    fn render_includes_kind_id_and_body() {
        let adapter = ClaudeMemoryAdapter;
        let s = RenderedSlot {
            slot: 0,
            note: note(
                "01J",
                "Resume tokens are host-bound.",
                StandardNoteKind::Hazard,
            ),
            tokens: 20,
        };
        let out = adapter.render_block_body(std::slice::from_ref(&s));
        assert!(out.contains("hazard"));
        assert!(out.contains("01J"));
        assert!(out.contains("Resume tokens"));
    }

    #[test]
    fn empty_bundle_renders_placeholder() {
        let out = ClaudeMemoryAdapter.render_block_body(&[]);
        assert!(out.contains("no notes selected"));
    }

    #[test]
    fn slot_order_affects_render() {
        let adapter = ClaudeMemoryAdapter;
        let s0 = RenderedSlot {
            slot: 0,
            note: note("a", "AAA", StandardNoteKind::Fact),
            tokens: 1,
        };
        let s1 = RenderedSlot {
            slot: 1,
            note: note("b", "BBB", StandardNoteKind::Fact),
            tokens: 1,
        };
        assert_ne!(
            adapter.render_block_body(&[s0.clone(), s1.clone()]),
            adapter.render_block_body(&[s1, s0]),
        );
    }
}
