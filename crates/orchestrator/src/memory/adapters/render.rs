//! Shared markdown renderer for the per-vendor adapters.
//!
//! All three currently-supported vendors (Claude / Codex / Gemini)
//! load a Markdown file at startup and treat HTML comments as
//! invisible. We exploit that by giving every vendor the same anchor
//! delimiters and the same body format. Differences are confined to
//! the [`MemoryAdapter::native_file_name`] return value and the
//! [`MemoryAdapter::vendor`] tag — the bytes between the anchors are
//! produced by a single function.
//!
//! Keeping rendering centralised means:
//!
//!   * Cross-vendor parity is structural, not "we copy-pasted the
//!     formatter and hope it stays in sync". Tests can assert
//!     byte-for-byte equality of the body for the same inputs.
//!   * Adding a fourth vendor is ~12 lines of code.
//!   * Future format ratchets (anchor `v1 → v2`, new heading shape)
//!     touch one place.

use crate::memory::adapter::RenderedSlot;

/// The single source-of-truth markdown renderer for the anchor body.
/// Pure transform over the slot list and a vendor-supplied preamble.
pub fn render_markdown_block(slots: &[RenderedSlot], preamble: &str) -> String {
    let mut out = String::with_capacity(256 + slots.len() * 256);
    out.push_str(preamble);
    if slots.is_empty() {
        out.push_str("\n_(no notes selected for this turn)_\n");
        return out;
    }
    for slot in slots {
        out.push('\n');
        let title = derive_title(&slot.note.body);
        out.push_str(&format!(
            "### {kind}: {title}\n",
            kind = slot.note.kind.as_str(),
            title = title,
        ));
        out.push_str(slot.note.body.trim_end());
        out.push('\n');
        // Note id footer locks the slot identity to a stable string
        // the worker can quote in `memory.correct` proposals (P2).
        out.push_str(&format!("_(note {id})_\n", id = slot.note.id));
    }
    out
}

/// First non-empty line, capped at 80 characters with a `...` suffix
/// on truncation. Title is cosmetic — not the source of truth — so
/// the truncation is safe.
fn derive_title(body: &str) -> String {
    let first = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if first.chars().count() <= 80 {
        first.to_owned()
    } else {
        let mut out: String = first.chars().take(77).collect();
        out.push_str("...");
        out
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
    fn empty_slots_renders_placeholder() {
        let out = render_markdown_block(&[], "PRE");
        assert!(out.starts_with("PRE"));
        assert!(out.contains("no notes selected"));
    }

    #[test]
    fn is_deterministic() {
        let s = slot("01J", "alpha", StandardNoteKind::Hazard);
        let a = render_markdown_block(std::slice::from_ref(&s), "PRE");
        let b = render_markdown_block(std::slice::from_ref(&s), "PRE");
        assert_eq!(a, b);
    }

    #[test]
    fn long_title_truncates_with_ellipsis() {
        let body = "x".repeat(200);
        let title = derive_title(&body);
        assert!(title.ends_with("..."));
        assert!(title.chars().count() <= 80);
    }
}
