//! Silent-vs-Notify policy gate.
//!
//! Routes every [`crate::mission_event::MissionEventKind`] through
//! [`visibility_for`] which returns an [`EventVisibility`] verdict:
//! `Internal` (never surfaced), `PowerUserOnly` (shown only when
//! the user enables "Show all events"), or `Inbox` (always shown
//! as a card on the user's primary surface).
//!
//! The mapping is an O(1) exhaustive `match` — adding a new
//! `MissionEventKind` variant fails to compile until classified.
//! Document and preserve that invariant.

pub mod visibility;

pub use visibility::{visibility_for, EventVisibility, InboxKind, Severity};
