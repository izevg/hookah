//! Codex CLI hook payload.
//!
//! Parses the JSON on stdin:
//!
//! - `PostToolUse` with `tool_name` `apply_patch` becomes one edit request per file the patch
//!   adds, updates, or moves to (deleted files are skipped). The patch text is in
//!   `tool_input.command` (documented) or `tool_input.patch`; its file headers are
//!   `*** Add File: <path>`, `*** Update File: <path>`, `*** Move to: <path>`. Relative paths
//!   resolve against `cwd`. Other tools are ignored.
//! - `Stop` becomes a stop request rooted at `cwd`; `stop_hook_active` tells whether the turn
//!   was already continued by an earlier `Stop` hook.
//! - `SessionStart` becomes a start request for `cwd` and `session_id`: it starts the baseline.
//! - `session_id` tags every edit and stop request, so they find the session's baseline.
//! - Any other event is ignored.

use std::path::{Path, PathBuf};

use hookah_application::{EditRequest, StopRequest};
use serde::Deserialize;

use crate::protocol::{Invocation, PayloadError, display_root, resolve_path, start_invocation};

#[derive(Deserialize)]
struct Payload {
    hook_event_name: Option<String>,
    session_id: Option<String>,
    cwd: Option<PathBuf>,
    tool_name: Option<String>,
    tool_input: Option<ToolInput>,
    stop_hook_active: Option<bool>,
}

#[derive(Deserialize)]
struct ToolInput {
    command: Option<String>,
    patch: Option<String>,
}

/// Parses a Codex CLI hook payload.
pub fn parse(stdin: &str) -> Result<Invocation, PayloadError> {
    let p: Payload = serde_json::from_str(stdin).map_err(|e| PayloadError(e.to_string()))?;
    match p.hook_event_name.as_deref() {
        Some("PostToolUse") => edit(&p),
        Some("SessionStart") => Ok(start_invocation(p.cwd, p.session_id)),
        Some("Stop") => {
            let root = p
                .cwd
                .filter(|c| c.is_absolute())
                .ok_or_else(|| PayloadError("stop event without an absolute cwd".into()))?;
            Ok(Invocation::Stop {
                request: StopRequest::new(root).with_session(p.session_id),
                already_continuing: p.stop_hook_active.unwrap_or(false),
            })
        }
        _ => Ok(Invocation::Ignore),
    }
}

fn edit(p: &Payload) -> Result<Invocation, PayloadError> {
    if p.tool_name.as_deref() != Some("apply_patch") {
        return Ok(Invocation::Ignore);
    }
    let Some(input) = &p.tool_input else {
        return Ok(Invocation::Ignore);
    };
    let Some(patch) = input.command.as_deref().or(input.patch.as_deref()) else {
        return Ok(Invocation::Ignore);
    };
    let mut requests = Vec::new();
    for path in patched_paths(patch) {
        let file = resolve_path(Path::new(&path), p.cwd.as_deref())?;
        let root = display_root(&file, p.cwd.as_deref());
        requests.push(EditRequest::new(file, root).with_session(p.session_id.clone()));
    }
    Ok(if requests.is_empty() {
        Invocation::Ignore
    } else {
        Invocation::EditBatch(requests)
    })
}

/// The paths a patch leaves files at: added, updated (or moved to). Deletions are skipped.
fn patched_paths(patch: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    // Whether the last header was an update, so a following `*** Move to:` replaces its path.
    let mut last_is_update = false;
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            paths.push(path.trim().to_string());
            last_is_update = false;
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            paths.push(path.trim().to_string());
            last_is_update = true;
        } else if let Some(path) = line.strip_prefix("*** Move to: ") {
            if last_is_update && let Some(last) = paths.last_mut() {
                *last = path.trim().to_string();
            }
            last_is_update = false;
        } else if line.starts_with("*** Delete File: ") {
            last_is_update = false;
        }
    }
    paths.dedup();
    paths
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use hookah_application::StartRequest;

    const PATCH: &str = "*** Begin Patch\n*** Update File: src/main.rs\n@@\n-a\n+b\n\
        *** Add File: /w/app/src/new.rs\n+fn x() {}\n*** Delete File: old.rs\n*** End Patch";

    fn payload(command: &str) -> String {
        serde_json::json!({
            "session_id": "s", "turn_id": "t", "transcript_path": null, "cwd": "/w/app",
            "hook_event_name": "PostToolUse", "model": "gpt-5", "permission_mode": "default",
            "tool_name": "apply_patch", "tool_use_id": "call_1",
            "tool_input": {"command": command}, "tool_response": "Success"
        })
        .to_string()
    }

    fn files(inv: Invocation) -> Vec<PathBuf> {
        match inv {
            Invocation::EditBatch(rs) => rs.into_iter().map(|r| r.file).collect(),
            other => panic!("expected batch, got {other:?}"),
        }
    }

    #[test]
    fn patch_yields_one_request_per_added_or_updated_file() {
        assert_eq!(
            files(parse(&payload(PATCH)).unwrap()),
            vec![
                PathBuf::from("/w/app/src/main.rs"),
                PathBuf::from("/w/app/src/new.rs")
            ]
        );
    }

    #[test]
    fn move_replaces_the_updated_path() {
        let patch =
            "*** Begin Patch\n*** Update File: a.py\n*** Move to: pkg/b.py\n@@\n*** End Patch";
        assert_eq!(
            files(parse(&payload(patch)).unwrap()),
            vec![PathBuf::from("/w/app/pkg/b.py")]
        );
    }

    #[test]
    fn patch_field_is_accepted_too() {
        let json = r#"{"cwd":"/w","hook_event_name":"PostToolUse","tool_name":"apply_patch",
            "tool_input":{"patch":"*** Begin Patch\n*** Add File: a.rs\n+x\n*** End Patch"}}"#;
        assert_eq!(files(parse(json).unwrap()), vec![PathBuf::from("/w/a.rs")]);
    }

    #[test]
    fn delete_only_patch_is_ignored() {
        let patch = "*** Begin Patch\n*** Delete File: a.rs\n*** End Patch";
        assert_eq!(parse(&payload(patch)).unwrap(), Invocation::Ignore);
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
    fn relative_path_without_cwd_is_malformed() {
        let json = r#"{"hook_event_name":"PostToolUse","tool_name":"apply_patch",
            "tool_input":{"command":"*** Add File: a.rs"}}"#;
        assert!(parse(json).is_err());
    }

    #[test]
    fn stop_carries_cwd_and_loop_flag() {
        let json = r#"{"session_id":"s","turn_id":"t","cwd":"/w/app","hook_event_name":"Stop",
            "model":"gpt-5","stop_hook_active":true,"last_assistant_message":null}"#;
        assert_eq!(
            parse(json).unwrap(),
            Invocation::Stop {
                request: StopRequest::new(PathBuf::from("/w/app")).with_session(Some("s".into())),
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
            Invocation::Start(StartRequest::new("/w/app", "ses 1"))
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
        let Invocation::EditBatch(rs) = parse(&payload(PATCH)).unwrap() else {
            panic!("expected batch");
        };
        assert!(rs.iter().all(|r| r.session.as_deref() == Some("s")));
    }
}
