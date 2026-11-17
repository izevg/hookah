//! `FileRole`: whether a changed file is production code or a test.
//!
//! The ecosystem adapter classifies files (it knows naming conventions and, for Rust, trailing
//! `#[cfg(test)]` modules). The domain rule built on it: mutation testing runs in the stop phase
//! only if at least one changed file of the project is a test.

/// What a changed file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileRole {
    /// Production code.
    Source,
    /// Test code, or an edit inside a test module.
    Test,
}

/// Whether mutation testing applies, given the roles of all changed files of one project.
pub fn mutation_applies(roles: impl IntoIterator<Item = FileRole>) -> bool {
    roles.into_iter().any(|r| r == FileRole::Test)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_needs_a_changed_test() {
        assert!(!mutation_applies([]));
        assert!(!mutation_applies([FileRole::Source, FileRole::Source]));
        assert!(mutation_applies([FileRole::Source, FileRole::Test]));
    }
}
