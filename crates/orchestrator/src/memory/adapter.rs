//! Vendor-specific memory rendering contract (V3 §4.10).
//!
//! A `MemoryAdapter` is the *render-side* counterpart to the existing
//! parser-side `adapter_core::Adapter`. It tells the Memory Kernel:
//!
//!   * the worker's native memory file (`CLAUDE.md` / `AGENTS.md` /
//!     `GEMINI.md`)
//!   * the anchor delimiters that fence the kernel-owned block (we
//!     only ever write inside these — user content outside is
//!     preserved verbatim)
//!   * how to translate a `page_table` of notes into the rendered
//!     block body
//!
//! Implementations are pure transforms over inputs — no I/O, no
//! globals — so determinism is straightforward to verify in tests.
//!
//! **Layering note:** P1 keeps concrete adapter implementations under
//! `orchestrator/src/memory/adapters/` rather than in the per-vendor
//! adapter crates (`adapters/claude/`, etc.). The current dep direction
//! has the orchestrator depending on adapter crates, so moving the
//! `MemoryAdapter` trait into `adapters/core` would either invert the
//! dependency or force a runtime dispatch boundary we don't need yet.
//! Once we either accept that inversion (P3+) or extract a shared
//! contracts crate, the concrete impls relocate; the trait stays here.

use std::path::Path;

use event_schema::Vendor;

use super::hierarchy::Note;

/// One entry in a composed bundle's page table — a note plus the
/// composer's token estimate.
#[derive(Debug, Clone)]
pub struct RenderedSlot {
    pub slot: u32,
    pub note: Note,
    pub tokens: u32,
}

/// Contract for vendor-specific rendering of the Memory Kernel's
/// anchor block. Pure transforms only.
pub trait MemoryAdapter: Send + Sync {
    /// Vendor identity — matches `event-schema::Vendor`.
    fn vendor(&self) -> Vendor;

    /// Native memory file the vendor CLI auto-loads at startup, e.g.
    /// `CLAUDE.md`. Resolved relative to the worker's worktree root.
    fn native_file_name(&self) -> &str;

    /// Opening anchor delimiter. Stable across versions of the same
    /// vendor profile — once written into a user's repo, the kernel
    /// must keep recognising it.
    fn anchor_open(&self) -> &str;

    /// Closing anchor delimiter.
    fn anchor_close(&self) -> &str;

    /// Per-worker T1 token budget (V3 §2). The composer uses this to
    /// drop slots that don't fit; adapters may override per-vendor.
    fn max_tokens(&self) -> usize {
        super::hierarchy::T1_MAX_TOKENS_DEFAULT
    }

    /// Render the page table into the body that goes *between* the
    /// anchor delimiters. Implementations are deterministic: same
    /// slots → same bytes, every time. No leading/trailing anchor
    /// delimiters — the kernel adds those.
    fn render_block_body(&self, slots: &[RenderedSlot]) -> String;

    /// Resolve the absolute path to the native file for a given
    /// worktree root. Adapters may override (e.g. nested config dirs)
    /// but the default `<worktree>/<native_file_name>` covers all P1
    /// vendors.
    fn native_file_path(&self, worktree_root: &Path) -> std::path::PathBuf {
        worktree_root.join(self.native_file_name())
    }
}

/// Cheap token estimator. P1 doesn't have a tokeniser; chars / 4 is
/// the standard rough approximation for English text and matches the
/// budgets in vendor docs. The composer in P4 may replace this with
/// per-vendor BPE estimators.
pub fn estimate_tokens(text: &str) -> u32 {
    // +10 covers slot framing overhead (heading, separators).
    let chars = text.chars().count() as u32;
    chars / 4 + 10
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_tokens_grows_with_length() {
        let short = estimate_tokens("hi");
        let long = estimate_tokens(&"hi".repeat(100));
        assert!(long > short);
    }
}
