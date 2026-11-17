//! Kimi Code hook payload.
//!
//! Parses the JSON on stdin (snake_case fields):
//!
//! - `PostToolUse` with tool `Write` or `Edit` becomes an edit request for `tool_input.path`
//!   (the built-in tools take `path`, not `file_path`). The older names `WriteFile` and
//!   `StrReplaceFile` are accepted too. Relative paths resolve against `cwd`. Other tools are
//!   ignored.
//! - Every other event, `Stop` included, is ignored: Kimi's `Stop` hook has no documented
//!   loop guard, so `hookah` does not block there.

use std::path::PathBuf;

use hookah_application::EditRequest;
use serde::Deserialize;

use crate::protocol::{Invocation, PayloadError, display_root, resolve_path};

#[derive(Deserialize)]
struct Payload {
    hook_event_name: Option<String>,
    cwd: Option<PathBuf>,
    tool_name: Option<String>,
    tool_input: Option<ToolInput>,
}

#[derive(Deserialize)]
struct ToolInput {
    path: Option<PathBuf>,
}

/// Parses a Kimi Code hook payload.
pub fn parse(stdin: &str) -> Result<Invocation, PayloadError> {
    let p: Payload = serde_json::from_str(stdin).map_err(|e| PayloadError(e.to_string()))?;
    if p.hook_event_name.as_deref() != Some("PostToolUse") {
        return Ok(Invocation::Ignore);
    }
    if !matches!(
        p.tool_name.as_deref(),
        Some("Write" | "Edit" | "WriteFile" | "StrReplaceFile")
    ) {
        return Ok(Invocation::Ignore);
    }
    let Some(path) = p.tool_input.as_ref().and_then(|i| i.path.as_ref()) else {
        return Ok(Invocation::Ignore);
    };
    let file = resolve_path(path, p.cwd.as_deref())?;
    let root = display_root(&file, p.cwd.as_deref());
    Ok(Invocation::Edit(EditRequest::new(file, root)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const WRITE: &str = r#"{
      "hook_event_name": "PostToolUse",
      "session_id": "session_abc",
      "session_title": "Fix the login page",
      "client_type": "kimi_code_cli",
      "cwd": "/work/app",
      "tool_name": "Write",
      "tool_input": {"path": "src/login.py", "content": "x = 1"},
      "tool_call_id": "call_1"
    }"#;

    fn edit_of(inv: Invocation) -> PathBuf {
        match inv {
            Invocation::Edit(r) => r.file,
            other => panic!("expected edit, got {other:?}"),
        }
    }

    #[test]
    fn write_resolves_against_cwd() {
        assert_eq!(
            edit_of(parse(WRITE).unwrap()),
            PathBuf::from("/work/app/src/login.py")
        );
    }

    #[test]
    fn edit_with_absolute_path() {
        let json = r#"{"hook_event_name":"PostToolUse","tool_name":"Edit",
            "tool_input":{"path":"/w/a.py","old_string":"a","new_string":"b"}}"#;
        assert_eq!(edit_of(parse(json).unwrap()), PathBuf::from("/w/a.py"));
    }

    #[test]
    fn relative_path_without_cwd_is_malformed() {
        let json = r#"{"hook_event_name":"PostToolUse","tool_name":"Write",
            "tool_input":{"path":"a.py"}}"#;
        assert!(parse(json).is_err());
    }

    #[test]
    fn other_tools_and_events_are_ignored() {
        let bash = r#"{"hook_event_name":"PostToolUse","cwd":"/w","tool_name":"Bash",
            "tool_input":{"command":"ls"}}"#;
        assert_eq!(parse(bash).unwrap(), Invocation::Ignore);
        let stop = r#"{"hook_event_name":"Stop","cwd":"/w"}"#;
        assert_eq!(parse(stop).unwrap(), Invocation::Ignore);
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(parse("not json").is_err());
    }
}
