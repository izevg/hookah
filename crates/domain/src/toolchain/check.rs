//! `CheckKind`: `Format`, `Lint`, `Typecheck`, `Architecture`, `Unused`, `Test`, `Mutation`.
//!
//! Which kinds run in which phase is decided by `Phase::check_kinds`.

/// The kind of check a tool performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CheckKind {
    /// Rewrites the file into canonical style.
    Format,
    /// Static analysis of the file.
    Lint,
    /// Static type checking (`tsc`, `mypy`).
    Typecheck,
    /// Checks module dependency rules (`dependency-cruiser`, `import-linter`).
    Architecture,
    /// Finds unused code, files, or dependencies across the paths the project configures.
    /// Runs for the baseline and stop phase.
    Unused,
    /// Runs tests related to the changed files.
    Test,
    /// Mutation testing of the sources covered by the changed tests.
    Mutation,
}

impl CheckKind {
    /// Lowercase name, stable for use in output and configuration.
    pub fn name(self) -> &'static str {
        match self {
            Self::Format => "format",
            Self::Lint => "lint",
            Self::Typecheck => "typecheck",
            Self::Architecture => "architecture",
            Self::Unused => "unused",
            Self::Test => "test",
            Self::Mutation => "mutation",
        }
    }

    /// Phrase for messages about a tool of this kind, such as `mutation testing`.
    pub fn description(self) -> &'static str {
        match self {
            Self::Format => "format",
            Self::Lint => "lint",
            Self::Typecheck => "typecheck",
            Self::Architecture => "architecture check",
            Self::Unused => "unused code check",
            Self::Test => "test",
            Self::Mutation => "mutation testing",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_descriptions() {
        assert_eq!(CheckKind::Typecheck.name(), "typecheck");
        assert_eq!(CheckKind::Mutation.description(), "mutation testing");
        assert_eq!(CheckKind::Unused.name(), "unused");
        assert_eq!(CheckKind::Unused.description(), "unused code check");
    }
}
