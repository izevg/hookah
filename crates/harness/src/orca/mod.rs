//! Orca harness adapter.
//!
//! Orca runs other agents (Claude Code, Codex, ...) in managed worktrees, so per-edit feedback
//! comes from the hooks of the agent it hosts. This module covers the Orca-level part: an Orca
//! plugin that reacts to workspace and agent events, plus setup notes. Its scope will be settled
//! once Orca's plugin event model is confirmed.
