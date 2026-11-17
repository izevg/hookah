//! Qoder hook payload.
//!
//! Parses the stdin payload for `PostToolUse` (matcher `Write|Edit`) and stop events from
//! `.qoder/settings.json`. The protocol is close to Claude Code's, but the two are kept as
//! separate adapters so they can change independently.
