//! Hermes Agent (Nous Research) harness adapter.
//!
//! Hermes' shell hooks cannot put text in front of the model after a file edit (their
//! `post_tool_call` result is ignored), so hookah ships a small Python plugin
//! ([`PLUGIN_SOURCE`]) that runs inside Hermes, calls `hookah hook hermes`, and exchanges one
//! versioned JSON document each way (see [`payload`] and [`response`]). See
//! `docs/setup/assistants/hermes.md`.

pub mod payload;
pub mod response;

use hookah_application::Outcome;

use crate::protocol::{Harness, Invocation, Parsed, PayloadError, Response};

/// The plugin source, installed as `__init__.py` and printed by `hookah plugin hermes`.
pub const PLUGIN_SOURCE: &str = include_str!("plugin.py");

/// The plugin manifest Hermes needs next to [`PLUGIN_SOURCE`] to discover the plugin.
pub const PLUGIN_MANIFEST: &str = "\
name: hookah
version: \"1.0\"
description: Runs the project's own format, lint, and test checks on what the agent edits.
provides_hooks:
  - on_session_start
  - pre_llm_call
  - transform_tool_result
  - pre_verify
";

/// The Hermes adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Hermes;

impl Harness for Hermes {
    fn parse_event(&self, stdin: &str) -> Result<Parsed, PayloadError> {
        payload::parse_event(stdin)
    }

    fn render(&self, invocation: &Invocation, outcome: &Outcome) -> Response {
        response::render(invocation, outcome)
    }
}
