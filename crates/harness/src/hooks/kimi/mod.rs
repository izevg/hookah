//! Kimi Code harness adapter.
//!
//! Command hooks configured as `[[hooks]]` in `config.toml` (beta upstream): `PostToolUse`
//! runs the edit phase. Kimi's `Stop` event can block but has no loop guard in its payload, so
//! the stop phase is not supported. See `docs/setup/hooks/cli/kimi.md`.

pub mod payload;
pub mod response;

use hookah_application::Outcome;

use crate::protocol::{Harness, Invocation, Parsed, PayloadError, Response};

/// The Kimi Code adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Kimi;

impl Harness for Kimi {
    fn parse_event(&self, stdin: &str) -> Result<Parsed, PayloadError> {
        payload::parse_event(stdin)
    }

    fn render(&self, invocation: &Invocation, outcome: &Outcome) -> Response {
        response::render(invocation, outcome)
    }
}
