//! Task Graph — DAG-based scheduling for parallel workers.
//!
//! Replaces the sequential `for (idx, task) in tasks.iter()` loop in
//! `mission_supervisor_run::mission_loop` with a topological-order
//! scheduler. Workers run concurrently up to
//! [`crate::arbiter::ArbiterPolicy::max_parallel_workers`];
//! integration remains serial against the supervisor branch.
//!
//! Public entry points:
//! - [`validate`] — accept a list of [`TaskDescriptor`]s, return a
//!   topologically-validated [`Dag`] or [`GraphError`].
//! - [`Scheduler`] — drive ready/running/done state as workers
//!   spawn and complete.
//! - [`criteria_eval::evaluate`] — fold per-task acceptance
//!   criteria into an arbiter Quality bound.
//! - [`role_routing::select_vendor_for_role`] — heuristic
//!   role → vendor mapping.

pub mod criteria_eval;
pub mod descriptor;
pub mod role_routing;
pub mod scheduler;
pub mod validate;

pub use criteria_eval::evaluate as evaluate_criteria;
pub use descriptor::{effective_scope_paths, AcceptanceCriteria, CriteriaOutcome, TaskRole};
pub use role_routing::select_vendor_for_role;
pub use scheduler::{Scheduler, TaskState};
pub use validate::{validate, Dag, GraphError};
