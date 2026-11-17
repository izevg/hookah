//! Claude Code hook payload.
//!
//! Parses the JSON on stdin:
//!
//! - `PostToolUse` with tool `Edit`, `Write`, `MultiEdit` (`tool_input.file_path`) or
//!   `NotebookEdit` (`tool_input.notebook_path`) becomes an edit request. Relative paths resolve
//!   against `cwd`. Other tools are ignored.
//! - `Stop` and `SubagentStop` become a stop request rooted at `cwd`; `stop_hook_active` tells
//!   whether the agent is already continuing because of an earlier blocked stop.
//! - Any other event is ignored.

use std::path::{Path, PathBuf};

use hookah_application::{EditRequest, StopRequest};
use serde::Deserialize;

use crate::protocol::{Invocation, PayloadError};

#[derive(Deserialize)]
struct Payload {
    hook_event_name: Option<String>,
    cwd: Option<PathBuf>,
    tool_name: Option<String>,
    tool_input: Option<ToolInput>,
    stop_hook_active: Option<bool>,
}

#[derive(Deserialize)]
struct ToolInput {
    file_path: Option<PathBuf>,
    notebook_path: Option<PathBuf>,
}

/// Parses a Claude Code hook payload.
pub fn parse(stdin: &str) -> Result<Invocation, PayloadError> {
    let p: Payload = serde_json::from_str(stdin).map_err(|e| PayloadError(e.to_string()))?;
    match p.hook_event_name.as_deref() {
        Some("PostToolUse") => edit(&p),
        Some("Stop" | "SubagentStop") => {
            let root = p
                .cwd
                .filter(|c| c.is_absolute())
                .ok_or_else(|| PayloadError("stop event without an absolute cwd".into()))?;
            Ok(Invocation::Stop {
                request: StopRequest { root },
                already_continuing: p.stop_hook_active.unwrap_or(false),
            })
        }
        _ => Ok(Invocation::Ignore),
    }
}

fn edit(p: &Payload) -> Result<Invocation, PayloadError> {
    let Some(input) = &p.tool_input else {
        return Ok(Invocation::Ignore);
    };
    let path = match p.tool_name.as_deref() {
        Some("Edit" | "Write" | "MultiEdit") => &input.file_path,
        Some("NotebookEdit") => &input.notebook_path,
        _ => return Ok(Invocation::Ignore),
    };
    let Some(path) = path else {
        return Ok(Invocation::Ignore);
    };
    Ok(Invocation::Edit(EditRequest {
        file: resolve(path, p.cwd.as_deref())?,
    }))
}

fn resolve(path: &Path, cwd: Option<&Path>) -> Result<PathBuf, PayloadError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    match cwd.filter(|c| c.is_absolute()) {
        Some(cwd) => Ok(cwd.join(path)),
        None => Err(PayloadError(format!(
            "relative path {} without an absolute cwd",
            path.display()
        ))),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const WRITE: &str = r#"{
      "session_id": "abc123",
      "transcript_path": "/home/u/.claude/projects/p/t.jsonl",
      "cwd": "/work/app",
      "permission_mode": "default",
      "hook_event_name": "PostToolUse",
      "tool_name": "Write",
      "tool_input": {"file_path": "/work/app/src/main.rs", "content": "fn main() {}"},
      "tool_response": {"filePath": "/work/app/src/main.rs", "type": "create"},
      "tool_use_id": "toolu_01",
      "duration_ms": 12
    }"#;

    fn edit_of(inv: Invocation) -> PathBuf {
        match inv {
            Invocation::Edit(r) => r.file,
            other => panic!("expected edit, got {other:?}"),
        }
    }

    #[test]
    fn write_is_an_edit() {
        assert_eq!(
            edit_of(parse(WRITE).unwrap()),
            PathBuf::from("/work/app/src/main.rs")
        );
    }

    #[test]
    fn edit_and_multiedit_use_file_path() {
        for tool in ["Edit", "MultiEdit"] {
            let json = format!(
                r#"{{"cwd":"/w","hook_event_name":"PostToolUse","tool_name":"{tool}",
                "tool_input":{{"file_path":"/w/a.py","old_string":"a","new_string":"b"}}}}"#
            );
            assert_eq!(edit_of(parse(&json).unwrap()), PathBuf::from("/w/a.py"));
        }
    }

    #[test]
    fn notebook_edit_uses_notebook_path() {
        let json = r#"{"cwd":"/w","hook_event_name":"PostToolUse","tool_name":"NotebookEdit",
            "tool_input":{"notebook_path":"/w/n.ipynb","new_source":"x"}}"#;
        assert_eq!(edit_of(parse(json).unwrap()), PathBuf::from("/w/n.ipynb"));
    }

    #[test]
    fn relative_path_resolves_against_cwd() {
        let json = r#"{"cwd":"/w/app","hook_event_name":"PostToolUse","tool_name":"Edit",
            "tool_input":{"file_path":"src/lib.rs"}}"#;
        assert_eq!(
            edit_of(parse(json).unwrap()),
            PathBuf::from("/w/app/src/lib.rs")
        );
    }

    #[test]
    fn relative_path_without_cwd_is_malformed() {
        let json = r#"{"hook_event_name":"PostToolUse","tool_name":"Edit",
            "tool_input":{"file_path":"src/lib.rs"}}"#;
        assert!(parse(json).is_err());
    }

    #[test]
    fn other_tools_and_events_are_ignored() {
        let bash = r#"{"cwd":"/w","hook_event_name":"PostToolUse","tool_name":"Bash",
            "tool_input":{"command":"ls"}}"#;
        assert_eq!(parse(bash).unwrap(), Invocation::Ignore);
        let prompt = r#"{"cwd":"/w","hook_event_name":"UserPromptSubmit","prompt":"hi"}"#;
        assert_eq!(parse(prompt).unwrap(), Invocation::Ignore);
    }

    #[test]
    fn stop_events_carry_cwd_and_loop_flag() {
        for (event, active) in [("Stop", false), ("SubagentStop", true)] {
            let json = format!(
                r#"{{"session_id":"s","cwd":"/w/app","hook_event_name":"{event}",
                "stop_hook_active":{active}}}"#
            );
            assert_eq!(
                parse(&json).unwrap(),
                Invocation::Stop {
                    request: StopRequest {
                        root: PathBuf::from("/w/app")
                    },
                    already_continuing: active,
                }
            );
        }
    }

    #[test]
    fn missing_stop_hook_active_means_false() {
        let json = r#"{"cwd":"/w","hook_event_name":"Stop"}"#;
        assert!(matches!(
            parse(json).unwrap(),
            Invocation::Stop {
                already_continuing: false,
                ..
            }
        ));
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(parse("not json").is_err());
        assert!(parse("").is_err());
    }
}
