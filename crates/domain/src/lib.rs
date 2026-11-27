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
//! - [`path`]: the single rule for rendering paths in reports.
//! - [`finding`]: problems reported by checks.
//! - [`verdict`]: the decision and compact report handed back to the agent, and the rule that
//!   decides which findings are reported.
//! - [`session`]: phases of an agent run, the files changed in it, and their roles.
//! - [`light`]: test weight and budgets, so the stop phase helps instead of gatekeeping.
//! - [`model`]: model selection rules that decide which AI models hooks run for.
//! - [`status`]: the latest outcome of a check run per project, for other tools to show.
//! - [`update`]: self-update rules: versions, release assets, the throttle, and what the user is told.

pub mod error;
pub mod finding;
pub mod light;
pub mod model;
pub mod path;
pub mod project;
pub mod session;
pub mod status;
pub mod toolchain;
pub mod update;
pub mod verdict;
