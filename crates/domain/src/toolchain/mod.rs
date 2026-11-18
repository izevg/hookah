//! The toolchain of a project.
//!
//! `Toolchain`: the tools a project configures, in execution order. It answers "which tools run
//! for this check kind" and never runs anything itself.

pub mod check;
pub mod selection;
pub mod tool;

pub use check::CheckKind;
pub use selection::{MutationSelection, TestSelection};
pub use tool::{Arg, CommandSpec, MissingTool, Tool, ToolNote};

/// The tools configured for a project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Toolchain {
    tools: Vec<Tool>,
    missing: Vec<MissingTool>,
    notes: Vec<ToolNote>,
}

impl Toolchain {
    /// Creates a toolchain; tools of one kind run in the given order.
    pub fn new(tools: Vec<Tool>) -> Self {
        Self {
            tools,
            missing: vec![],
            notes: vec![],
        }
    }

    /// Sets the tools the project should have but does not.
    pub fn with_missing(mut self, missing: Vec<MissingTool>) -> Self {
        self.missing = missing;
        self
    }

    /// Sets the heads-ups about how the tools run.
    pub fn with_notes(mut self, notes: Vec<ToolNote>) -> Self {
        self.notes = notes;
        self
    }

    /// Heads-ups about how the tools run. Use cases turn each into a non-blocking notice.
    pub fn notes(&self) -> &[ToolNote] {
        &self.notes
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn tool(name: &str, kind: CheckKind) -> Tool {
        Tool {
            name: name.into(),
            kind,
            command: CommandSpec::new(name, vec![], "/r", Duration::from_secs(1)),
        }
    }

    #[test]
    fn exposes_tools_and_filters_by_kind_in_order() {
        let t = Toolchain::new(vec![
            tool("fmt", CheckKind::Format),
            tool("clippy", CheckKind::Lint),
            tool("other", CheckKind::Lint),
        ]);
        assert_eq!(t.tools().len(), 3);
        let lint: Vec<_> = t
            .of_kind(CheckKind::Lint)
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(lint, ["clippy", "other"]);
        assert_eq!(t.of_kind(CheckKind::Test).count(), 0);
    }

    #[test]
    fn missing_tools_are_stored() {
        let m = MissingTool::new("cargo-mutants", CheckKind::Mutation, "install it");
        let t = Toolchain::default().with_missing(vec![m.clone()]);
        assert_eq!(t.missing(), [m]);
        assert!(Toolchain::default().missing().is_empty());
    }

    #[test]
    fn notes_are_stored_and_do_not_make_a_toolchain_non_empty() {
        let note = ToolNote::new(CheckKind::Typecheck, "left out");
        let t = Toolchain::default().with_notes(vec![note.clone()]);
        assert_eq!(t.notes(), [note]);
        assert!(Toolchain::default().notes().is_empty());
        assert!(t.is_empty());
    }

    #[test]
    fn empty_only_without_tools_and_without_missing() {
        assert!(Toolchain::default().is_empty());
        assert!(!Toolchain::new(vec![tool("fmt", CheckKind::Format)]).is_empty());
        let missing = MissingTool::new("x", CheckKind::Lint, "h");
        assert!(!Toolchain::default().with_missing(vec![missing]).is_empty());
    }
}
