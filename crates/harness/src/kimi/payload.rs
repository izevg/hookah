//! Kimi Code hook payload.
//!
//! Parses the stdin payload for `PostToolUse` (`tool_name`, `tool_input`, `tool_call_id`) from
//! `[[hooks]]` entries in `config.toml`. Hooks are in beta upstream; the payload shape is
//! checked against the current docs when the adapter is built.
