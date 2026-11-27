//! Cursor hook payload (the IDE agent and `cursor-agent`).
//!
//! Parses the JSON on stdin. Every event carries `hook_event_name` and `workspace_roots`; the
//! `cursor-agent` CLI is reported to omit some common fields such as `conversation_id`, so none
//! of them is required except where a request cannot be built without it.
//!
//! - `afterFileEdit` becomes an edit request for `file_path` (absolute in the docs; a relative
//!   path resolves against the first workspace root).
//! - `stop` becomes a stop request rooted at the first workspace root. `loop_count` is how many
//!   automatic follow-ups the conversation already had, so a value above 0 means the stop
//!   already follows a `followup_message`. A `status` of `aborted` or `error` is ignored: the
//!   user cut the turn short, and a follow-up would override that.
//! - `sessionStart` becomes a start request for the first workspace root and `session_id`
//!   (the same value as `conversation_id`): it starts the baseline.
//! - `conversation_id` (or `session_id`) tags edit and stop requests, so they find the session's
//!   baseline.
//! - Any other event is ignored.

use std::path::{Path, PathBuf};

use hookah_application::{EditRequest, StopRequest};
use serde::Deserialize;

use crate::protocol::{
    Invocation, ModelHint, Parsed, PayloadError, clean_model, clean_transcript, display_root,
    resolve_path, start_invocation,
};

#[derive(Deserialize)]
struct Payload {
    hook_event_name: Option<String>,
    conversation_id: Option<String>,
    session_id: Option<String>,
    model: Option<String>,
    transcript_path: Option<PathBuf>,
    workspace_roots: Option<Vec<PathBuf>>,
    file_path: Option<PathBuf>,
    status: Option<String>,
    loop_count: Option<u32>,
}

impl Payload {
    /// The first workspace root that is an absolute path.
    fn root(&self) -> Option<&Path> {
        self.workspace_roots
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(PathBuf::as_path)
            .find(|r| r.is_absolute())
    }

    fn session(&self) -> Option<String> {
        self.conversation_id
            .clone()
            .or_else(|| self.session_id.clone())
            .filter(|s| !s.is_empty())
    }
}

/// Parses a Cursor hook payload.
pub fn parse(stdin: &str) -> Result<Invocation, PayloadError> {
    parse_event(stdin).map(|parsed| parsed.invocation)
}

/// Parses a Cursor hook payload along with its model metadata.
pub fn parse_event(stdin: &str) -> Result<Parsed, PayloadError> {
    let p: Payload = serde_json::from_str(stdin).map_err(|e| PayloadError(e.to_string()))?;
    let model = ModelHint {
        model: clean_model(p.model.clone()),
        transcript: clean_transcript(p.transcript_path.clone(), p.root()),
    };
    let invocation = match p.hook_event_name.as_deref() {
        Some("afterFileEdit") => edit(&p)?,
        Some("sessionStart") => start_invocation(p.root().map(Path::to_path_buf), p.session()),
        Some("stop") => stop(&p)?,
        _ => Invocation::Ignore,
    };
    Ok(Parsed { invocation, model })
}

fn edit(p: &Payload) -> Result<Invocation, PayloadError> {
    let Some(path) = &p.file_path else {
        return Ok(Invocation::Ignore);
    };
    let file = resolve_path(path, p.root())?;
    // With several workspace roots, show paths relative to the one that holds the file.
    let root = p
        .workspace_roots
        .as_deref()
        .unwrap_or_default()
        .iter()
        .find(|r| r.is_absolute() && file.starts_with(r))
        .map(PathBuf::as_path)
        .or_else(|| p.root());
    Ok(Invocation::Edit(
        EditRequest::new(file.clone(), display_root(&file, root)).with_session(p.session()),
    ))
}

fn stop(p: &Payload) -> Result<Invocation, PayloadError> {
    if matches!(p.status.as_deref(), Some("aborted" | "error")) {
        return Ok(Invocation::Ignore);
    }
    let root = p
        .root()
        .ok_or_else(|| PayloadError("stop event without an absolute workspace root".into()))?;
    Ok(Invocation::Stop {
        request: StopRequest::new(root).with_session(p.session()),
        already_continuing: p.loop_count.unwrap_or(0) > 0,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::testing::{abs, os_json};
    use hookah_application::StartRequest;

    fn parse(json: &str) -> Result<Invocation, PayloadError> {
        super::parse(&os_json(json))
    }

    /// The shape documented at <https://cursor.com/docs/agent/hooks>: the common fields plus
    /// `file_path` and `edits`.
    const AFTER_FILE_EDIT: &str = r#"{
      "conversation_id": "conv-1",
      "generation_id": "gen-1",
      "model": "claude-sonnet",
      "hook_event_name": "afterFileEdit",
      "cursor_version": "1.7.0",
      "workspace_roots": ["/work/app"],
      "user_email": "dev@example.com",
      "transcript_path": null,
      "file_path": "/work/app/src/main.rs",
      "edits": [{"old_string": "a", "new_string": "b"}]
    }"#;

    const STOP: &str = r#"{
      "conversation_id": "conv-1",
      "hook_event_name": "stop",
      "workspace_roots": ["/work/app"],
      "status": "completed",
      "loop_count": 0
    }"#;

    #[test]
    fn after_file_edit_is_an_edit_with_the_session() {
        let Invocation::Edit(r) = parse(AFTER_FILE_EDIT).unwrap() else {
            panic!("expected edit");
        };
        assert_eq!(r.file, abs("/work/app/src/main.rs"));
        assert_eq!(r.display_root, abs("/work/app"));
        assert_eq!(r.session.as_deref(), Some("conv-1"));
    }

    #[test]
    fn a_relative_path_resolves_against_the_workspace_root() {
        let json = r#"{"hook_event_name":"afterFileEdit","workspace_roots":["/w/app"],
            "file_path":"src/a.py"}"#;
        let Invocation::Edit(r) = parse(json).unwrap() else {
            panic!("expected edit");
        };
        assert_eq!(r.file, abs("/w/app/src/a.py"));
        assert_eq!(r.session, None);
    }

    #[test]
    fn the_display_root_is_the_workspace_root_that_holds_the_file() {
        let json = r#"{"hook_event_name":"afterFileEdit","workspace_roots":["/w/a","/w/b"],
            "file_path":"/w/b/x.rs"}"#;
        let Invocation::Edit(r) = parse(json).unwrap() else {
            panic!("expected edit");
        };
        assert_eq!(r.display_root, abs("/w/b"));
        let outside = r#"{"hook_event_name":"afterFileEdit","workspace_roots":["/w/a","/w/b"],
            "file_path":"/elsewhere/x.rs"}"#;
        let Invocation::Edit(r) = parse(outside).unwrap() else {
            panic!("expected edit");
        };
        assert_eq!(r.display_root, abs("/w/a"));
    }

    #[test]
    fn a_relative_path_without_a_root_is_malformed_and_a_missing_path_is_ignored() {
        let rel = r#"{"hook_event_name":"afterFileEdit","file_path":"a.py"}"#;
        assert!(parse(rel).is_err());
        let none = r#"{"hook_event_name":"afterFileEdit","workspace_roots":["/w"]}"#;
        assert_eq!(parse(none).unwrap(), Invocation::Ignore);
    }

    #[test]
    fn stop_carries_the_root_and_the_loop_state() {
        assert_eq!(
            parse(STOP).unwrap(),
            Invocation::Stop {
                request: StopRequest::new(abs("/work/app")).with_session(Some("conv-1".into())),
                already_continuing: false,
            }
        );
        let again = STOP.replace("\"loop_count\": 0", "\"loop_count\": 1");
        let Invocation::Stop {
            already_continuing, ..
        } = parse(&again).unwrap()
        else {
            panic!("expected stop");
        };
        assert!(already_continuing);
    }

    #[test]
    fn a_stop_the_user_cut_short_is_ignored() {
        for status in ["aborted", "error"] {
            let json = STOP.replace("completed", status);
            assert_eq!(parse(&json).unwrap(), Invocation::Ignore, "{status}");
        }
    }

    #[test]
    fn a_stop_without_a_root_is_malformed() {
        assert!(parse(r#"{"hook_event_name":"stop","status":"completed"}"#).is_err());
        assert!(parse(r#"{"hook_event_name":"stop","workspace_roots":["rel"]}"#).is_err());
    }

    #[test]
    fn a_missing_loop_count_and_status_still_stop() {
        let json = r#"{"hook_event_name":"stop","workspace_roots":["/w"]}"#;
        assert_eq!(
            parse(json).unwrap(),
            Invocation::Stop {
                request: StopRequest::new(abs("/w")),
                already_continuing: false,
            }
        );
    }

    #[test]
    fn session_start_needs_a_session_id_and_an_absolute_root() {
        let ok = r#"{"hook_event_name":"sessionStart","session_id":"ses 1",
            "is_background_agent":false,"composer_mode":"agent","workspace_roots":["/w/app"]}"#;
        assert_eq!(
            parse(ok).unwrap(),
            Invocation::Start(StartRequest::new(abs("/w/app"), "ses 1"))
        );
        for json in [
            r#"{"hook_event_name":"sessionStart","workspace_roots":["/w/app"]}"#,
            r#"{"hook_event_name":"sessionStart","session_id":"","workspace_roots":["/w"]}"#,
            r#"{"hook_event_name":"sessionStart","session_id":"s"}"#,
            r#"{"hook_event_name":"sessionStart","session_id":"s","workspace_roots":["rel"]}"#,
        ] {
            assert_eq!(parse(json).unwrap(), Invocation::Ignore, "{json}");
        }
    }

    #[test]
    fn other_events_are_ignored_and_garbage_is_an_error() {
        let json = r#"{"hook_event_name":"beforeShellExecution","command":"ls"}"#;
        assert_eq!(parse(json).unwrap(), Invocation::Ignore);
        assert_eq!(parse("{}").unwrap(), Invocation::Ignore);
        assert!(parse("not json").is_err());
    }

    #[test]
    fn extracts_model_from_common_fields() {
        let parsed = parse_event(&os_json(AFTER_FILE_EDIT)).unwrap();
        assert_eq!(parsed.model.model.as_deref(), Some("claude-sonnet"));
    }
}
