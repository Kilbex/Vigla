//! ID helpers for the Memory Kernel.
//!
//! All memory identifiers are UUIDv7 strings (time-ordered, matches
//! `orchestrator/src/ids.rs`). Distinct constructors per concept keep
//! call sites readable.

use uuid::Uuid;

pub fn new_note_id() -> String {
    Uuid::now_v7().to_string()
}

pub fn new_proposal_id() -> String {
    Uuid::now_v7().to_string()
}

pub fn new_bundle_id() -> String {
    Uuid::now_v7().to_string()
}

pub fn new_memory_event_id() -> String {
    Uuid::now_v7().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_well_formed() {
        let a = new_note_id();
        let b = new_note_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(a.matches('-').count(), 4);
    }
}
