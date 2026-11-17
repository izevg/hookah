//! The toolchain of a project.
//!
//! `Toolchain`: the tools a project configures, in execution order. It answers "which tools run
//! for this check kind" and never runs anything itself.

pub mod check;
pub mod selection;
pub mod tool;

pub use check::CheckKind;
pub use selection::{MutationSelection, TestSelection};
pub use tool::{Arg, CommandSpec, MissingTool, Tool};

/// The tools configured for a project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Toolchain {
    tools: Vec<Tool>,
    missing: Vec<MissingTool>,
}

impl Toolchain {
    /// Creates a toolchain; tools of one kind run in the given order.
    pub fn new(tools: Vec<Tool>) -> Self {
        Self {
            tools,
            missing: vec![],
        }
    }

    /// Sets the tools the project should have but does not.
    pub fn with_missing(mut self, missing: Vec<MissingTool>) -> Self {
        self.missing = missing;
        self
    }

    /// Tools the project should have but does not. Use cases turn each into a non-blocking
    /// notice; they never fail a check and nothing is installed.
    pub fn missing(&self) -> &[MissingTool] {
        &self.missing
    }

    /// All tools.
    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// The tools serving `kind`, in order.
    pub fn of_kind(&self, kind: CheckKind) -> impl Iterator<Item = &Tool> {
        self.tools.iter().filter(move |t| t.kind == kind)
    }

    /// Whether no tool is configured and none is missing.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty() && self.missing.is_empty()
    }
}
