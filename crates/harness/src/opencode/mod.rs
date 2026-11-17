//! opencode harness adapter.
//!
//! opencode has no command hooks, only JS/TS plugins. hookah ships a small TypeScript plugin
//! ([`PLUGIN_SOURCE`]) that runs inside opencode, calls `hookah hook opencode`, and exchanges
//! one versioned JSON document each way (see [`payload`] and [`response`]). See
//! `docs/setup/plugins/opencode.md`.

pub mod payload;
pub mod response;

use hookah_application::Outcome;

use crate::protocol::{Harness, Invocation, PayloadError, Response};

/// The plugin source, printed by `hookah plugin opencode`.
pub const PLUGIN_SOURCE: &str = include_str!("plugin.ts");

/// The opencode adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Opencode;

impl Harness for Opencode {
    fn parse(&self, stdin: &str) -> Result<Invocation, PayloadError> {
        payload::parse(stdin)
    }

    fn render(&self, invocation: &Invocation, outcome: &Outcome) -> Response {
        response::render(invocation, outcome)
    }
}
