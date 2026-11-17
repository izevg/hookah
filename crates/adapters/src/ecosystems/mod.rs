//! Ecosystem adapters.
//!
//! A registry that sends each project to the right ecosystem module. Each module implements
//! toolchain detection, tool-output parsing into findings, and related-test selection.

pub mod node;
pub mod python;
pub mod rust;
