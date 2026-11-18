//! `Budgets`: how much work the stop phase accepts before it steps aside.
//!
//! - **Tests.** At most `tests` selected test targets run (default 25). More means the change is
//!   too broad for a quick check; the tests are not run and the agent is told to run them.
//! - **Mutation.** Mutation testing runs only for a small change: at most `mutation_files`
//!   changed code files (default 3) and `mutation_lines` changed lines in them (default 150).
//!
//! A budget of `0` switches that check off entirely, silently: the user asked for it.
//! Exceeding a budget is never a failure: the result is a [`Skip`], rendered as a notice.

use std::fmt;

/// Default most test targets that run at stop.
pub const DEFAULT_TESTS: usize = 25;
/// Default most changed files mutation testing accepts.
pub const DEFAULT_MUTATION_FILES: usize = 3;
/// Default most changed lines mutation testing accepts.
pub const DEFAULT_MUTATION_LINES: usize = 150;

/// The limits of the light stop phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budgets {
    tests: usize,
    mutation_files: usize,
    mutation_lines: usize,
}

impl Default for Budgets {
    fn default() -> Self {
        Self::new(
            DEFAULT_TESTS,
            DEFAULT_MUTATION_FILES,
            DEFAULT_MUTATION_LINES,
        )
    }
}

/// What a budget says about a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// Within budget: run the check.
    Run,
    /// Switched off with a budget of `0`: do not run, say nothing.
    Off,
    /// Over budget: do not run, tell the agent why.
    Skip(Skip),
}

/// Why a check did not run: its input was over budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// More test targets were selected than the budget allows.
    Tests {
        /// Targets selected.
        selected: usize,
        /// The budget.
        budget: usize,
    },
    /// The change is too large for mutation testing.
    Mutation {
        /// Changed code files.
        files: usize,
        /// Changed lines in them.
        lines: usize,
        /// The file budget.
        max_files: usize,
        /// The line budget.
        max_lines: usize,
    },
}

impl fmt::Display for Skip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tests { selected, budget } => write!(
                f,
                "{selected} related tests selected; above the light budget of {budget}, run them yourself"
            ),
            Self::Mutation {
                files,
                lines,
                max_files,
                max_lines,
            } => write!(
                f,
                "mutation testing skipped: change too large ({files} files / {lines} lines, budget {max_files} / {max_lines})"
            ),
        }
    }
}

impl Budgets {
    /// Builds budgets; `0` switches the matching check off.
    pub fn new(tests: usize, mutation_files: usize, mutation_lines: usize) -> Self {
        Self {
            tests,
            mutation_files,
            mutation_lines,
        }
    }

    /// Replaces the test budget.
    #[must_use]
    pub fn with_tests(mut self, tests: usize) -> Self {
        self.tests = tests;
        self
    }

    /// Replaces the mutation file budget.
    #[must_use]
    pub fn with_mutation_files(mut self, files: usize) -> Self {
        self.mutation_files = files;
        self
    }

    /// Replaces the mutation line budget.
    #[must_use]
    pub fn with_mutation_lines(mut self, lines: usize) -> Self {
        self.mutation_lines = lines;
        self
    }

    /// Whether `selected` test targets may run.
    pub fn admit_tests(&self, selected: usize) -> Admission {
        match self.tests {
            0 => Admission::Off,
            budget if selected > budget => Admission::Skip(Skip::Tests { selected, budget }),
            _ => Admission::Run,
        }
    }

    /// Whether mutation testing may run for a change of `files` code files and `lines` changed
    /// lines.
    pub fn admit_mutation(&self, files: usize, lines: usize) -> Admission {
        let (max_files, max_lines) = (self.mutation_files, self.mutation_lines);
        if max_files == 0 || max_lines == 0 {
            Admission::Off
        } else if files > max_files || lines > max_lines {
            Admission::Skip(Skip::Mutation {
                files,
                lines,
                max_files,
                max_lines,
            })
        } else {
            Admission::Run
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        assert_eq!(Budgets::default(), Budgets::new(25, 3, 150));
    }

    #[test]
    fn builders_replace_one_budget_each() {
        let b = Budgets::default()
            .with_tests(1)
            .with_mutation_files(2)
            .with_mutation_lines(3);
        assert_eq!(b, Budgets::new(1, 2, 3));
        assert_eq!(Budgets::default().with_tests(9), Budgets::new(9, 3, 150));
        assert_eq!(
            Budgets::default().with_mutation_files(9),
            Budgets::new(25, 9, 150)
        );
        assert_eq!(
            Budgets::default().with_mutation_lines(9),
            Budgets::new(25, 3, 9)
        );
    }

    #[test]
    fn tests_run_up_to_the_budget_inclusive() {
        let b = Budgets::new(2, 3, 150);
        assert_eq!(b.admit_tests(0), Admission::Run);
        assert_eq!(b.admit_tests(2), Admission::Run);
        assert_eq!(
            b.admit_tests(3),
            Admission::Skip(Skip::Tests {
                selected: 3,
                budget: 2
            })
        );
    }

    #[test]
    fn a_zero_test_budget_switches_tests_off() {
        assert_eq!(Budgets::new(0, 3, 150).admit_tests(0), Admission::Off);
        assert_eq!(Budgets::new(0, 3, 150).admit_tests(9), Admission::Off);
    }

    #[test]
    fn mutation_needs_both_budgets_met_inclusive() {
        let b = Budgets::new(25, 3, 150);
        assert_eq!(b.admit_mutation(3, 150), Admission::Run);
        assert_eq!(b.admit_mutation(0, 0), Admission::Run);
        for (files, lines) in [(4, 10), (1, 151), (4, 151)] {
            assert_eq!(
                b.admit_mutation(files, lines),
                Admission::Skip(Skip::Mutation {
                    files,
                    lines,
                    max_files: 3,
                    max_lines: 150
                })
            );
        }
    }

    #[test]
    fn a_zero_mutation_budget_switches_mutation_off() {
        assert_eq!(
            Budgets::new(25, 0, 150).admit_mutation(1, 1),
            Admission::Off
        );
        assert_eq!(Budgets::new(25, 3, 0).admit_mutation(1, 1), Admission::Off);
        assert_eq!(Budgets::new(25, 0, 0).admit_mutation(9, 9), Admission::Off);
    }

    #[test]
    fn skips_read_as_notices() {
        let tests = Skip::Tests {
            selected: 30,
            budget: 25,
        };
        assert_eq!(
            tests.to_string(),
            "30 related tests selected; above the light budget of 25, run them yourself"
        );
        let mutation = Skip::Mutation {
            files: 5,
            lines: 410,
            max_files: 3,
            max_lines: 150,
        };
        assert_eq!(
            mutation.to_string(),
            "mutation testing skipped: change too large (5 files / 410 lines, budget 3 / 150)"
        );
    }
}
