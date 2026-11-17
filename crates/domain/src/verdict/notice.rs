//! Notices: messages for the agent that are not findings.
//!
//! The main one is `FileRewritten`: a formatter changed the file, so the agent must re-read it
//! before the next edit. Degradation warnings (timeout, missing tool) are notices too.

use std::fmt;
use std::path::PathBuf;

/// A message for the agent that is not a finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// A formatter rewrote this file (absolute path); re-read it before the next edit.
    FileRewritten(PathBuf),
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
            Self::Degraded(reason) => write!(f, "hookah: {reason}"),
        }
    }
}
