//! ZCode native command hooks for a session start, a successful file edit, and a stop.

mod payload;
mod response;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;

use hookah_application::Outcome;

use crate::protocol::{Harness, Invocation, Parsed, PayloadError, Response};

/// The ZCode adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct ZCode;

impl Harness for ZCode {
    fn parse_event(&self, stdin: &str) -> Result<Parsed, PayloadError> {
        payload::parse_event(stdin)
    }

    fn render(&self, invocation: &Invocation, outcome: &Outcome) -> Response {
        response::render(invocation, outcome)
    }
}
