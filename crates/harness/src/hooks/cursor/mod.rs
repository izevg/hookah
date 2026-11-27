//! Cursor harness adapter (the IDE agent and the `cursor-agent` CLI).
//!
//! Command hooks configured in `.cursor/hooks.json`: `sessionStart` starts the baseline,
//! `afterFileEdit` runs the edit phase, and `stop` runs the stop phase. `afterFileEdit` cannot
//! talk to the model, so `stop` is the enforcement point. See `docs/setup/hooks/ide/cursor.md`.

pub mod payload;
pub mod response;

use hookah_application::Outcome;

use crate::protocol::{Harness, Invocation, Parsed, PayloadError, Response};

/// The Cursor adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Cursor;

impl Harness for Cursor {
    fn parse_event(&self, stdin: &str) -> Result<Parsed, PayloadError> {
        payload::parse_event(stdin)
    }

    fn render(&self, invocation: &Invocation, outcome: &Outcome) -> Response {
        response::render(invocation, outcome)
    }
}
