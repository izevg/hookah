//! The finding entity.
//!
//! `Finding`: one problem reported by a check. It has a location, severity, message, optional
//! rule id, and the check kind that produced it. This is the only shape tool output takes once it
//! leaves an adapter.

pub mod baseline;
pub mod fingerprint;
pub mod location;

pub use location::Location;

use crate::toolchain::CheckKind;

/// How serious a finding is. `Error` sorts before `Warning`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Breaks the build, a test, or the tool itself. Always reported.
    Error,
    /// Style or suspicious code. Reported only on changed lines.
    Warning,
}

/// One problem reported by a check.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Finding {
    /// Where the problem is.
    pub location: Location,
    /// How serious it is.
    pub severity: Severity,
    /// Single-line message.
    pub message: String,
    /// The tool's rule id, such as `clippy::needless_return`.
    pub rule: Option<String>,
    /// The check that produced it.
    pub kind: CheckKind,
}

impl Finding {
    /// Creates a finding without a rule id.
    pub fn new(
        kind: CheckKind,
        severity: Severity,
        location: Location,
        message: impl Into<String>,
    ) -> Self {
        Self {
            location,
            severity,
            message: message.into(),
            rule: None,
            kind,
        }
    }

    /// Sets the rule id.
    pub fn with_rule(mut self, rule: impl Into<String>) -> Self {
        self.rule = Some(rule.into());
        self
    }
}
