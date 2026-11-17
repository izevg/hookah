//! Core domain model of `hookah`.
//!
//! Pure business rules with no I/O, no async, and no knowledge of agents, processes, or the
//! filesystem. Every other crate depends on this one; it depends on nothing.
//!
//! - [`project`]: the project a changed file belongs to.
//! - [`toolchain`]: the checks a project can run, and how they target a file.
//! - [`finding`]: problems reported by checks, and the baseline that filters them.
//! - [`verdict`]: the decision and compact report handed back to the agent.
//! - [`session`]: one agent run, its phases, and the files it changed.

pub mod error;
pub mod finding;
pub mod project;
pub mod session;
pub mod toolchain;
pub mod verdict;
