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
