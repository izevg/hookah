//! `CheckKind`: `Format`, `Lint`, `Test`.
//!
//! Which kinds run in which phase is decided by `Phase::check_kinds`.

/// The kind of check a tool performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CheckKind {
    /// Rewrites the file into canonical style.
    Format,
    /// Static analysis of the file.
    Lint,
    /// Runs tests related to the changed files.
    Test,
}

impl CheckKind {
    /// Lowercase name, stable for use in output and configuration.
    pub fn name(self) -> &'static str {
        match self {
            Self::Format => "format",
            Self::Lint => "lint",
            Self::Test => "test",
        }
    }
}
