//! Execution ports.
//!
//! - `CheckRunner`: run one tool against a target within its timeout, and return findings plus
//!   whether the file was rewritten.
//! - `Clock`: current time and deadlines, so time budgets can be tested.
