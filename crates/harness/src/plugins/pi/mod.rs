//! Pi coding agent adapter.
//!
//! Pi has no command hooks, only TypeScript extensions. hookah ships a small TypeScript
//! extension ([`PLUGIN_SOURCE`]) that runs inside Pi, calls `hookah hook pi`, and exchanges
//! one versioned JSON document each way (see [`payload`] and [`response`]). See
//! `docs/setup/plugins/pi.md`.

pub mod payload;
pub mod response;

use hookah_application::Outcome;

use crate::protocol::{Harness, Invocation, PayloadError, Response};

/// The extension source, printed by `hookah plugin pi`.
pub const PLUGIN_SOURCE: &str = include_str!("extension.ts");

/// The Pi adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pi;

impl Harness for Pi {
    fn parse(&self, stdin: &str) -> Result<Invocation, PayloadError> {
        payload::parse(stdin)
    }

    fn render(&self, invocation: &Invocation, outcome: &Outcome) -> Response {
        response::render(invocation, outcome)
    }
}
