//! Qoder CLI hook payload.
//!
//! Parses the JSON on stdin:
//!
//! - `PostToolUse` with tool `Write` or `Edit` becomes an edit request for
//!   `tool_input.file_path` (documented for `Write`; `Edit` is assumed to match). Relative
//!   paths resolve against `cwd`. Other tools are ignored.
//! - `Stop` becomes a stop request rooted at `cwd`; `stop_hook_active` tells whether the turn
//!   is already driven by a Stop hook.
//! - `SessionStart` becomes a start request for `cwd` and `session_id`: it starts the baseline.
//! - `session_id` tags every edit and stop request, so they find the session's baseline.
//! - Any other event is ignored.

use std::path::PathBuf;

use hookah_application::{EditRequest, StopRequest};
use serde::Deserialize;

use crate::protocol::{
    Invocation, ModelHint, Parsed, PayloadError, clean_model, clean_transcript, display_root,
    resolve_path, start_invocation,
};

#[derive(Deserialize)]
struct Payload {
    hook_event_name: Option<String>,
    session_id: Option<String>,
    cwd: Option<PathBuf>,
    model: Option<String>,
    transcript_path: Option<PathBuf>,
    tool_name: Option<String>,
    tool_input: Option<ToolInput>,
    stop_hook_active: Option<bool>,
}

#[derive(Deserialize)]
struct ToolInput {
    file_path: Option<PathBuf>,
}

/// Parses a Qoder CLI hook payload.
pub fn parse(stdin: &str) -> Result<Invocation, PayloadError> {
    parse_event(stdin).map(|parsed| parsed.invocation)
}

/// Parses a Qoder CLI hook payload along with its model metadata.
pub fn parse_event(stdin: &str) -> Result<Parsed, PayloadError> {
    let p: Payload = serde_json::from_str(stdin).map_err(|e| PayloadError(e.to_string()))?;
    let model = ModelHint {
        model: clean_model(p.model.clone()),
        transcript: clean_transcript(p.transcript_path.clone(), p.cwd.as_deref()),
    };
    let invocation = match p.hook_event_name.as_deref() {
        Some("PostToolUse") => edit(&p)?,
        Some("SessionStart") => start_invocation(p.cwd, p.session_id),
        Some("Stop") => {
            let root = p
                .cwd
                .filter(|c| c.is_absolute())
                .ok_or_else(|| PayloadError("stop event without an absolute cwd".into()))?;
            Invocation::Stop {
                request: StopRequest::new(root).with_session(p.session_id),
                already_continuing: p.stop_hook_active.unwrap_or(false),
            }
        }
        _ => Invocation::Ignore,
    };
    Ok(Parsed { invocation, model })
}

fn edit(p: &Payload) -> Result<Invocation, PayloadError> {
    if !matches!(p.tool_name.as_deref(), Some("Write" | "Edit")) {
        return Ok(Invocation::Ignore);
    }
    let Some(path) = p.tool_input.as_ref().and_then(|i| i.file_path.as_ref()) else {
        return Ok(Invocation::Ignore);
    };
    let file = resolve_path(path, p.cwd.as_deref())?;
    let root = display_root(&file, p.cwd.as_deref());
    Ok(Invocation::Edit(
        EditRequest::new(file, root).with_session(p.session_id.clone()),
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::testing::{abs, os_json};

    fn parse(json: &str) -> Result<Invocation, PayloadError> {
        super::parse(&os_json(json))
    }
    use hookah_application::StartRequest;

    const WRITE: &str = r#"{
      "session_id": "abc123",
      "transcript_path": "/home/u/.qoder/projects/p/t.jsonl",
      "cwd": "/work/app",
      "hook_event_name": "PostToolUse",
      "permission_mode": "default",
      "tool_name": "Write",
      "tool_input": {"file_path": "/work/app/src/app.ts", "content": "export {}"},
      "tool_response": {"success": true},
      "tool_use_id": "toolu_01"
    }"#;

    fn edit_of(inv: Invocation) -> PathBuf {
        match inv {
            Invocation::Edit(r) => r.file,
            other => panic!("expected edit, got {other:?}"),
        }
    }

    #[test]
    fn write_is_an_edit() {
        assert_eq!(edit_of(parse(WRITE).unwrap()), abs("/work/app/src/app.ts"));
    }

    #[test]
    fn edit_tool_and_relative_path() {
        let json = r#"{"cwd":"/w/app","hook_event_name":"PostToolUse","tool_name":"Edit",
            "tool_input":{"file_path":"src/a.py","old_string":"a","new_string":"b"}}"#;
        assert_eq!(edit_of(parse(json).unwrap()), abs("/w/app/src/a.py"));
    }

    #[test]
    fn relative_path_without_cwd_is_malformed() {
        let json = r#"{"hook_event_name":"PostToolUse","tool_name":"Write",
            "tool_input":{"file_path":"a.py"}}"#;
        assert!(parse(json).is_err());
    }

    #[test]
    fn other_tools_and_events_are_ignored() {
        let bash = r#"{"cwd":"/w","hook_event_name":"PostToolUse","tool_name":"Bash",
            "tool_input":{"command":"ls"}}"#;
        assert_eq!(parse(bash).unwrap(), Invocation::Ignore);
        let start = r#"{"cwd":"/w","hook_event_name":"SessionStart"}"#;
        assert_eq!(parse(start).unwrap(), Invocation::Ignore);
    }

    #[test]
    fn stop_carries_cwd_and_loop_flag() {
        let json = r#"{"session_id":"s","cwd":"/w/app","hook_event_name":"Stop",
            "stop_hook_active":true,"last_assistant_message":"done"}"#;
        assert_eq!(
            parse(json).unwrap(),
            Invocation::Stop {
                request: StopRequest::new(abs("/w/app")).with_session(Some("s".into())),
                already_continuing: true,
            }
        );
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(parse("not json").is_err());
    }

    #[test]
    fn session_start_needs_a_session_id_and_an_absolute_cwd() {
        let ok = r#"{"session_id":"ses 1","cwd":"/w/app","hook_event_name":"SessionStart","source":"startup"}"#;
        assert_eq!(
            parse(ok).unwrap(),
            Invocation::Start(StartRequest::new(abs("/w/app"), "ses 1"))
        );
        for json in [
            r#"{"cwd":"/w/app","hook_event_name":"SessionStart"}"#,
            r#"{"session_id":"","cwd":"/w/app","hook_event_name":"SessionStart"}"#,
            r#"{"session_id":"s","hook_event_name":"SessionStart"}"#,
            r#"{"session_id":"s","cwd":"rel","hook_event_name":"SessionStart"}"#,
        ] {
            assert_eq!(parse(json).unwrap(), Invocation::Ignore, "{json}");
        }
    }

    #[test]
    fn edits_carry_the_session_id() {
        let Invocation::Edit(r) = parse(WRITE).unwrap() else {
            panic!("expected edit");
        };
        assert_eq!(r.session.as_deref(), Some("abc123"));
    }
}
