//! `CheckKind`: `Format`, `Lint`, `Typecheck`, `Architecture`, `Test`, `Mutation`.
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
    }
}
