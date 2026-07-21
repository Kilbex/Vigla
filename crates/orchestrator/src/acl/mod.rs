//! Per-worker file ACL enforcement.
//!
//! Top-level entry points are [`FileAcl`] (the data type),
//! [`check_diff`] (the pre-integration gate), and
//! [`sentinel::write_sentinel`] (the per-worktree audit trail).
//! See module `README.md` for the design overview.

pub mod check;
pub mod file_acl;
pub mod sentinel;

pub use check::{check_diff, AclViolation};
pub use file_acl::FileAcl;
pub use sentinel::{read_sentinel, write_sentinel};
