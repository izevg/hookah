//! Claude Code harness adapter.
//!
//! Command hooks configured in `.claude/settings.json`: `PostToolUse` runs the edit phase and
//! `Stop` / `SubagentStop` run the stop phase. See `docs/setup/hooks/claude-code.md`.

pub mod payload;
pub mod response;

use hookah_application::Outcome;

use crate::protocol::{Harness, Invocation, PayloadError, Response};

/// The Claude Code adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Claude;

impl Harness for Claude {
    fn parse(&self, stdin: &str) -> Result<Invocation, PayloadError> {
        payload::parse(stdin)
    }

    fn render(&self, invocation: &Invocation, outcome: &Outcome) -> Response {
        response::render(invocation, outcome)
    }
}

#[cfg(test)]
mod tests {
    use hookah_application::EditRequest;
    use hookah_domain::finding::{Finding, Location, Severity};
    use hookah_domain::toolchain::CheckKind;
    use hookah_domain::verdict::{Report, Verdict};

    use super::*;

    #[test]
    fn render_delegates_to_the_response_rules() {
        let finding = Finding::new(
            CheckKind::Lint,
            Severity::Error,
            Location::line("a.rs", 3),
            "unused variable",
        );
        let outcome = Outcome {
            verdict: Verdict::Fail(
                Report::new(vec![finding], 20).unwrap_or_else(|| unreachable!()),
            ),
            notices: vec![],
        };
        let invocation = Invocation::Edit(EditRequest::new("/w/a.rs", "/w"));
        let response = Claude.render(&invocation, &outcome);
        assert_eq!(response.exit_code, 2);
        assert!(response.stderr.contains("a.rs:3: unused variable"));
    }
}
