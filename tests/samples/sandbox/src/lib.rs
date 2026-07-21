//! Tiny library used as the Step-11 gate target.
//!
//! `multiply` is intentionally wrong (returns a + b instead of a * b)
//! so the test in `tests/multiply.rs` fails. Claude's task is to
//! correct the implementation.

pub fn multiply(a: i64, b: i64) -> i64 {
a * b
}
