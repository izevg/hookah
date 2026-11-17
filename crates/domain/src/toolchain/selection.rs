//! Test selection.
//!
//! `TestSelection` and `MutationSelection`: either the specific tests related to a set of changed files, or `None`. The
//! rule: if no related test can be found, run nothing. Never fall back to the full suite.

/// The tests chosen for a set of changed files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestSelection {
    /// No related test found. Do not run tests; never run the whole suite instead.
    None,
    /// Run these targets: package names or test ids, as the test tool expects them.
    Targets(Vec<String>),
}

impl TestSelection {
    /// Builds a selection from targets; an empty list means [`TestSelection::None`].
    pub fn from_targets(targets: Vec<String>) -> Self {
        if targets.is_empty() {
            Self::None
        } else {
            Self::Targets(targets)
        }
    }
}

/// The mutation targets chosen for the sources covered by changed tests.
///
/// Targets are tool-specific strings (source files, module names). Mutation testing is slow, so
/// the adapter must keep the selection narrow; `None` means nothing to mutate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationSelection {
    /// Nothing to mutate. Do not run mutation testing.
    None,
    /// Mutate these targets, as the mutation tool expects them.
    Targets(Vec<String>),
}

impl MutationSelection {
    /// Builds a selection from targets; an empty list means [`MutationSelection::None`].
    pub fn from_targets(targets: Vec<String>) -> Self {
        if targets.is_empty() {
            Self::None
        } else {
            Self::Targets(targets)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_mutation_selection_is_none() {
        assert_eq!(
            MutationSelection::from_targets(vec![]),
            MutationSelection::None
        );
    }

    #[test]
    fn empty_is_none() {
        assert_eq!(TestSelection::from_targets(vec![]), TestSelection::None);
        assert_eq!(
            TestSelection::from_targets(vec!["a".into()]),
            TestSelection::Targets(vec!["a".into()])
        );
    }
}
