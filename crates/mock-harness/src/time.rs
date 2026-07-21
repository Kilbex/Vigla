//! RFC 3339 formatting for the mock-harness.
//!
//! The implementation now lives in [`event_schema::time`] so every
//! event producer in the workspace shares ONE formatter (see that
//! module for why). Re-exported here so the mock-harness's existing
//! `crate::time::rfc3339_from_unix_ms` call sites keep working
//! unchanged.

pub use event_schema::time::rfc3339_from_unix_ms;
