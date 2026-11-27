//! Qoder CLI harness adapter.
//!
//! Command hooks configured in `.qoder/settings.json`: `PostToolUse` (matcher `Write|Edit`)
//! runs the edit phase and `Stop` runs the stop phase. See `docs/setup/hooks/cli/qoder.md`.

pub mod payload;
pub mod response;

use hookah_application::Outcome;

use crate::protocol::{Harness, Invocation, Parsed, PayloadError, Response};

/// The Qoder CLI adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Qoder;

impl Harness for Qoder {
    fn parse_event(&self, stdin: &str) -> Result<Parsed, PayloadError> {
        payload::parse_event(stdin)
    }

    fn render(&self, invocation: &Invocation, outcome: &Outcome) -> Response {
        response::render(invocation, outcome)
    }
}
