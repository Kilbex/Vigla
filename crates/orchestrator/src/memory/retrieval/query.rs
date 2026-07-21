//! Retrieval query + result types.
//!
//! Pure data types shared by all retrieval backends (substring fallback,
//! V1.1 BM25, V1.2 hybrid). Kept free of kernel handles so unit tests
//! can exercise scorers in isolation.

use crate::memory::hierarchy::NoteKind;

/// Input to any retrieval backend.
///
/// `context_hints` is reserved for V1.3 task-aware queries (concat of
/// `TaskDescriptor.title`, `MissionSpec.objective`, upstream
/// `HandoffNote`s); Phase 0 callers pass an empty `Vec`.
#[derive(Debug, Clone)]
pub struct RetrievalQuery {
    pub detail: String,
    pub kind: Option<NoteKind>,
    pub context_hints: Vec<String>,
}

impl RetrievalQuery {
    /// Convenience constructor for the common case: just `detail`,
    /// no kind filter, no hints.
    pub fn from_detail(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
            kind: None,
            context_hints: Vec::new(),
        }
    }
}

/// A note ranked by some retrieval backend.
///
/// `terms_matched` is for explainability — tests assert which query
/// terms drove a particular ranking, so a tokeniser change that
/// silently changes recall is caught at the unit-test layer rather
/// than only at the evaluation-harness layer.
#[derive(Debug, Clone)]
pub struct ScoredNote {
    pub note_id: String,
    pub score: f64,
    pub terms_matched: Vec<String>,
}
