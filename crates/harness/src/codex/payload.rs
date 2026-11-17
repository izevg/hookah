//! Codex CLI hook payload.
//!
//! Parses the hook payload for `PostToolUse` (`apply_patch` and file-writing tools) and `Stop`
//! events, as configured in `config.toml` `[hooks]` or `hooks.json`. One patch can touch several
//! files; each one becomes a request.
