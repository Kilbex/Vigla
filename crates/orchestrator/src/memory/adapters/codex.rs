//! Codex memory adapter (V3 §5).
//!
//! Renders into `AGENTS.md`. Codex's startup scanner default cap is
//! 32 KiB; Vigla's per-anchor token budget keeps us well under
//! that even with the file's own (preserved) user content.
//!
//! Anchor delimiters and body layout are *identical* to the Claude
//! adapter — the moat is cross-vendor parity, which depends on the
//! kernel composing one structural artifact that all three CLIs
//! read uniformly. The only vendor-axis variability lives in
//! [`MemoryAdapter::vendor`] / [`MemoryAdapter::native_file_name`].

use event_schema::Vendor;

use super::super::adapter::{MemoryAdapter, RenderedSlot};
use super::claude::{ANCHOR_CLOSE, ANCHOR_OPEN};
use super::render::render_markdown_block;

pub const NATIVE_FILE: &str = "AGENTS.md";

const PREAMBLE: &str = "The following notes are curated by your supervisor for this task. \
Do not edit this block — edits are detected as drift and discarded.\n";

#[derive(Debug, Clone, Copy, Default)]
pub struct CodexMemoryAdapter;

impl MemoryAdapter for CodexMemoryAdapter {
    fn vendor(&self) -> Vendor {
        Vendor::Codex
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
    fn codex_writes_to_agents_md() {
        assert_eq!(CodexMemoryAdapter.native_file_name(), "AGENTS.md");
    }

    #[test]
    fn codex_vendor_tag_is_codex() {
        assert_eq!(CodexMemoryAdapter.vendor(), Vendor::Codex);
    }

    #[test]
    fn render_includes_canonical_markers() {
        let s = slot("01J", "alpha", StandardNoteKind::Fact);
        let out = CodexMemoryAdapter.render_block_body(std::slice::from_ref(&s));
        assert!(out.contains("fact"));
        assert!(out.contains("01J"));
        assert!(out.contains("alpha"));
    }
}
