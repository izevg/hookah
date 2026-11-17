//! The toolchain of a project.
//!
//! `Toolchain`: the tools a project configures, grouped by check kind. It answers "which tools
//! run for this check on this file" and never runs anything itself.

pub mod check;
pub mod selection;
pub mod tool;
