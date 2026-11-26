use std::path::PathBuf;

use hookah_application::{EditRequest, StopRequest};
use serde::Deserialize;

use crate::hooks::patch;
use crate::protocol::{Invocation, PayloadError, display_root, resolve_path, start_invocation};

#[derive(Deserialize)]
struct Payload {
    // ZCode emits both spellings. Read only the documented snake_case keys so duplicates of
    // the same information do not cause a deserialization error.
    hook_event_name: Option<String>,
    session_id: Option<String>,
    cwd: Option<PathBuf>,
    tool_name: Option<String>,
    tool_input: Option<ToolInput>,
    stop_hook_active: Option<bool>,
}

#[derive(Deserialize)]
struct ToolInput {
    file_path: Option<PathBuf>,
    patch_text: Option<String>,
}

pub(super) fn parse(stdin: &str) -> Result<Invocation, PayloadError> {
    let payload: Payload = serde_json::from_str(stdin).map_err(|e| PayloadError(e.to_string()))?;
    match payload.hook_event_name.as_deref() {
        Some("PostToolUse") => edit(&payload),
        Some("SessionStart") => Ok(start_invocation(payload.cwd, payload.session_id)),
        Some("Stop") => {
            let root = payload
                .cwd
                .filter(|cwd| cwd.is_absolute())
                .ok_or_else(|| PayloadError("stop event without an absolute cwd".into()))?;
            Ok(Invocation::Stop {
                request: StopRequest::new(root).with_session(payload.session_id),
                already_continuing: payload.stop_hook_active.unwrap_or(false),
            })
        }
        _ => Ok(Invocation::Ignore),
    }
}

fn edit(payload: &Payload) -> Result<Invocation, PayloadError> {
    let Some(input) = &payload.tool_input else {
        return Ok(Invocation::Ignore);
    };
    match payload.tool_name.as_deref() {
        Some("Write" | "Edit") => {
            let Some(path) = input
                .file_path
                .as_deref()
                .filter(|p| !p.as_os_str().is_empty())
            else {
                return Ok(Invocation::Ignore);
            };
            let file = resolve_path(path, payload.cwd.as_deref())?;
            let root = display_root(&file, payload.cwd.as_deref());
            Ok(Invocation::Edit(
                EditRequest::new(file, root).with_session(payload.session_id.clone()),
            ))
        }
        Some("ApplyPatch") => match &input.patch_text {
            Some(text) => {
                patch::invocation(text, payload.cwd.as_deref(), payload.session_id.as_deref())
            }
            None => Ok(Invocation::Ignore),
        },
        _ => Ok(Invocation::Ignore),
    }
}
