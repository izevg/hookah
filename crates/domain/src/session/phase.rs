//! `Phase`: `Edit` (after each file change) or `Stop` (when the agent declares it is done).
//!
//! Each phase has its own check kinds. Formatters run before linters so lint findings match the
//! final text of the file.
//!
//! - `Edit`: Format, Lint, Typecheck, Architecture, on the edited file.
//! - `Stop`: Lint, Typecheck, Architecture over all changed files of the project (this catches
//!   files changed outside edit hooks, such as by a shell `sed`), then Test, then Mutation.
//!   Format never runs at stop: it rewrites files, and the agent has no chance to re-read them.
//!   Mutation is skipped for a project whose tests failed or degraded ([`TestRun`]): mutating a
//!   red baseline only adds noise.

use crate::toolchain::check::CheckKind;

/// When in the agent run `hookah` is being called.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    /// After a single file edit: format, lint, typecheck, architecture.
    Edit,
    /// When the agent declares it is done: static checks, tests, mutation testing.
    Stop,
}

impl Phase {
    /// The name used in stored records and JSON output.
    pub fn name(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::Stop => "stop",
        }
    }

    /// The inverse of [`Phase::name`].
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Edit, Self::Stop]
            .into_iter()
            .find(|p| p.name() == name)
    }

    /// The check kinds this phase runs, in execution order.
    pub fn check_kinds(self) -> &'static [CheckKind] {
        match self {
            Self::Edit => &[
                CheckKind::Format,
                CheckKind::Lint,
                CheckKind::Typecheck,
                CheckKind::Architecture,
            ],
            Self::Stop => &[
                CheckKind::Lint,
                CheckKind::Typecheck,
                CheckKind::Architecture,
                CheckKind::Test,
                CheckKind::Mutation,
            ],
        }
    }
}

/// How the Test check of one project went in the stop phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestRun {
    /// No tests ran.
    NotRun,
    /// Tests ran and reported no errors.
    Passed,
    /// Tests reported an `Error` finding, or the run failed (crash, timeout, unreadable output).
    Failed,
}

impl TestRun {
    /// Classifies a Test check from what it produced. `ran`: at least one test tool ran.
    pub fn observe(ran: bool, error_findings: bool, degraded: bool) -> Self {
        match (ran, error_findings || degraded) {
            (false, _) => Self::NotRun,
            (true, true) => Self::Failed,
            (true, false) => Self::Passed,
        }
    }

    /// Whether mutation testing may follow. Not after failed tests.
    pub fn allows_mutation(self) -> bool {
        self != Self::Failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_waits_for_green_tests() {
        assert!(TestRun::observe(false, true, true).allows_mutation());
        assert!(TestRun::observe(true, false, false).allows_mutation());
        assert!(!TestRun::observe(true, true, false).allows_mutation());
        assert!(!TestRun::observe(true, false, true).allows_mutation());
    }

    #[test]
    fn phase_names_round_trip() {
        for p in [Phase::Edit, Phase::Stop] {
            assert_eq!(Phase::from_name(p.name()), Some(p));
        }
        assert_eq!(Phase::from_name("x"), None);
    }

    #[test]
    fn phase_rule() {
        use CheckKind::*;
        assert_eq!(
            Phase::Edit.check_kinds(),
            &[Format, Lint, Typecheck, Architecture]
        );
        assert_eq!(
            Phase::Stop.check_kinds(),
            &[Lint, Typecheck, Architecture, Test, Mutation]
        );
        assert!(!Phase::Stop.check_kinds().contains(&Format));
    }
}
