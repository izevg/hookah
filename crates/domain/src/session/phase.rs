//! `Phase`: `Edit` (after each file change) or `Stop` (when the agent declares it is done).
//!
//! Each phase has its own check kinds. Formatters run before linters so lint findings match the
//! final text of the file.
//!
//! - `Edit`: Format, Lint, Typecheck, Architecture, on the edited file.
//! - `Stop`: Lint, Typecheck, Architecture over all changed files of the project (this catches
//!   files changed outside edit hooks, such as by a shell `sed`), then Test, then Mutation.
//!   Format never runs at stop: it rewrites files, and the agent has no chance to re-read them.

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

#[cfg(test)]
mod tests {
    use super::*;

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
