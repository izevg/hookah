//! Codex CLI harness adapter.
//!
//! Command hooks configured in `config.toml` `[hooks]` or `hooks.json`: `PostToolUse` for
//! `apply_patch` runs the edit phase and `Stop` runs the stop phase. See
//! `docs/setup/hooks/codex.md`.

pub mod payload;
pub mod response;

use hookah_application::Outcome;

use crate::protocol::{Harness, Invocation, PayloadError, Response};

/// The Codex CLI adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Codex;

impl Harness for Codex {
    fn parse(&self, stdin: &str) -> Result<Invocation, PayloadError> {
        payload::parse(stdin)
    }

    fn render(&self, invocation: &Invocation, outcome: &Outcome) -> Response {
        response::render(invocation, outcome)
    }
}
