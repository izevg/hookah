//! `Phase`: `Edit` (after each file change) or `Stop` (when the agent declares it is done).
//!
//! Each phase has its own check kinds. Formatters run before linters so lint findings match the
//! final text of the file.

use crate::toolchain::check::CheckKind;

/// When in the agent run `hookah` is being called.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    /// After a single file edit: format, then lint.
    Edit,
    /// When the agent declares it is done: tests.
    Stop,
}

impl Phase {
    /// The check kinds this phase runs, in execution order.
    pub fn check_kinds(self) -> &'static [CheckKind] {
        match self {
            Self::Edit => &[CheckKind::Format, CheckKind::Lint],
            Self::Stop => &[CheckKind::Test],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_rule() {
        assert_eq!(
            Phase::Edit.check_kinds(),
            &[CheckKind::Format, CheckKind::Lint]
        );
        assert_eq!(Phase::Stop.check_kinds(), &[CheckKind::Test]);
    }
}
