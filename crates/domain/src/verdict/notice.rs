//! Notices: messages for the agent that are not findings.
//!
//! `FileRewritten`: a formatter changed the file, so the agent must re-read it before the next
//! edit. `MissingTool` reports a tool the project lacks. Degradation warnings (timeout, port
//! error) are notices too.

use std::fmt;
use std::path::PathBuf;

use crate::toolchain::MissingTool;

/// A message for the agent that is not a finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// A formatter rewrote this file (path as displayed to the agent, relative to the display root when inside it); re-read it before the next edit.
    FileRewritten(PathBuf),
    /// A tool the project should have is not installed. Never blocks; `hookah` installs nothing.
    MissingTool(MissingTool),
    /// `hookah` could not finish part of its job (timeout, missing tool, port error).
    Degraded(String),
}

impl fmt::Display for Notice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileRewritten(p) => {
                write!(
                    f,
                    "{} was reformatted; re-read it before editing",
                    p.display()
                )
            }
            Self::MissingTool(t) => write!(
                f,
                "{} tool {} not installed: {}",
                t.kind.description(),
                t.name,
                t.hint
            ),
            Self::Degraded(reason) => write!(f, "hookah: {reason}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toolchain::CheckKind;

    #[test]
    fn renders_each_notice() {
        assert_eq!(
            Notice::FileRewritten("/r/a.rs".into()).to_string(),
            "/r/a.rs was reformatted; re-read it before editing"
        );
        let t = MissingTool::new("cargo-mutants", CheckKind::Mutation, "cargo install x");
        assert_eq!(
            Notice::MissingTool(t).to_string(),
            "mutation testing tool cargo-mutants not installed: cargo install x"
        );
        assert_eq!(Notice::Degraded("slow".into()).to_string(), "hookah: slow");
    }
}
