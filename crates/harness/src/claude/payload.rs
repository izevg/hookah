//! Claude Code hook payload.
//!
//! Parses the JSON on stdin for `PostToolUse` (tools `Edit`, `Write`, `MultiEdit`,
//! `NotebookEdit`) and `Stop` / `SubagentStop` events: session id, tool name, and the edited
//! file path.
