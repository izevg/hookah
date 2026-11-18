//! OpenClaw harness adapter.
//!
//! OpenClaw has typed plugin hooks (`api.on(...)`), not command hooks. hookah ships a small
//! JavaScript plugin ([`PLUGIN_SOURCE`], with its [`MANIFEST`]) that runs inside OpenClaw, calls
//! `hookah hook openclaw`, and exchanges one versioned JSON document each way. The wire contract
//! is the same as opencode's, so this adapter reuses its payload and response (see
//! [`crate::plugins::opencode::payload`] and [`crate::plugins::opencode::response`]). See
//! `docs/setup/assistants/openclaw.md`.

use hookah_application::Outcome;

use crate::plugins::opencode::{payload, response};
use crate::protocol::{Harness, Invocation, PayloadError, Response};

/// The plugin entry point, printed by `hookah plugin openclaw`.
pub const PLUGIN_SOURCE: &str = include_str!("plugin.js");

/// The plugin manifest OpenClaw requires next to the entry point.
pub const MANIFEST: &str = include_str!("openclaw.plugin.json");

/// The OpenClaw adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Openclaw;

impl Harness for Openclaw {
    fn parse(&self, stdin: &str) -> Result<Invocation, PayloadError> {
        payload::parse_for(stdin, "openclaw")
    }

    fn render(&self, invocation: &Invocation, outcome: &Outcome) -> Response {
        response::render(invocation, outcome)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::testing::os_json;

    #[test]
    fn parses_the_shared_contract_and_names_openclaw_in_version_errors() {
        let ok = os_json(r#"{"v":1,"event":"edit","cwd":"/w","files":["/w/a.ts"]}"#);
        assert!(matches!(Openclaw.parse(&ok), Ok(Invocation::Edit(_))));
        let err = Openclaw.parse(r#"{"v":9,"event":"stop"}"#).unwrap_err();
        assert!(err.0.contains("hookah plugin openclaw"), "{}", err.0);
    }

    #[test]
    fn the_manifest_names_the_hookah_plugin_and_the_source_uses_its_hooks() {
        assert!(MANIFEST.contains(r#""id": "hookah""#));
        // The middleware is trusted surface: OpenClaw rejects it unless the manifest declares it.
        assert!(MANIFEST.contains(r#""agentToolResultMiddleware": ["openclaw"]"#));
        for hook in [
            "session_start",
            "registerAgentToolResultMiddleware",
            "before_agent_finalize",
        ] {
            assert!(PLUGIN_SOURCE.contains(hook), "{hook}");
        }
    }
}
