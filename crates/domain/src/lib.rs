//! Core domain model of `hookah`.
//!
//! Pure business rules with no I/O, no async, and no knowledge of agents, processes, or the
//! filesystem. Every other crate depends on this one; it depends on nothing.
//!
//! Paths in the model are absolute unless stated otherwise. The exceptions are
//! [`finding::location::Location`] and tool targets, which are relative to the project root.
//!
//! - [`project`]: the project a changed file belongs to.
//! - [`toolchain`]: the checks a project can run, and how they target a file.
//! - [`finding`]: problems reported by checks.
//! - [`verdict`]: the decision and compact report handed back to the agent, and the rule that
//!   decides which findings are reported.
//! - [`session`]: phases of an agent run and the files changed in it.

pub mod error;
pub mod finding;
pub mod project;
pub mod session;
pub mod toolchain;
pub mod verdict;
