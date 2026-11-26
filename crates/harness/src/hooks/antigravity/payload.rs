use std::path::PathBuf;

use hookah_application::EditRequest;
use hookah_domain::path::normalize;
use serde::Deserialize;

use crate::protocol::{PayloadError, canonical_dir};

/// The file-writing tools whose completed edit should be checked.
pub const EDIT_TOOLS: &str =
    "^(write_file|edit_file|write_to_file|replace_file_content|multi_replace_file_content)$";

/// The events configured by setup, distinguished by their event-specific fields.
#[derive(Debug, Eq, PartialEq)]
pub enum Event {
    /// Initialize the baselines.
    Start,
    /// Check a successful workspace edit and queue its response for the model.
    Edit(EditRequest),
    /// Deliver queued edit feedback just before a model call.
    BeforeModel,
    /// Run the stop phase only when the conversation reaches an ordinary, fully idle stop.
    Stop { eligible: bool },
    /// Unrelated, failed, or non-workspace tool call.
    Ignore,
}

/// Parsed Antigravity event and the conversation it belongs to.
pub struct Payload {
    /// Stable conversation identifier used to isolate baselines and feedback.
    pub session: String,
    /// Mounted workspace paths in the order Antigravity provided them.
    pub roots: Vec<PathBuf>,
    /// User input associated with the current turn, used to bound continuations.
    pub prompt: String,
    /// What this hook should do.
    pub event: Event,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Input {
    conversation_id: Option<String>,
    #[serde(default)]
    workspace_paths: Vec<PathBuf>,
    last_user_input: Option<String>,
    tool_call: Option<ToolCall>,
    step_idx: Option<usize>,
    invocation_num: Option<usize>,
    execution_num: Option<usize>,
    termination_reason: Option<String>,
    fully_idle: Option<bool>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct ToolCall {
    name: String,
    args: ToolArgs,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ToolArgs {
    target_file: Option<PathBuf>,
    path: Option<PathBuf>,
    #[serde(default)]
    is_artifact: bool,
}

/// Reads the native Antigravity contract. There is no common event-name field.
pub fn parse(stdin: &str) -> Result<Payload, PayloadError> {
    let input: Input = serde_json::from_str(stdin).map_err(|e| PayloadError(e.to_string()))?;
    let mut roots = Vec::new();
    for path in input
        .workspace_paths
        .iter()
        .filter(|path| path.is_absolute())
    {
        let root = canonical_dir(path);
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    let session = input.conversation_id.unwrap_or_default();
    let event = if input.execution_num.is_some() || input.fully_idle.is_some() {
        Event::Stop {
            eligible: !session.is_empty()
                && input.fully_idle == Some(true)
                && matches!(
                    input.termination_reason.as_deref(),
                    Some("model_stop" | "NO_TOOL_CALL")
                )
                && input.error.as_deref().is_none_or(str::is_empty),
        }
    } else if session.is_empty() || roots.is_empty() {
        Event::Ignore
    } else if input.invocation_num.is_some() {
        Event::BeforeModel
    } else if input.tool_call.is_some() || input.step_idx.is_some() {
        input
            .tool_call
            .filter(|_| input.error.as_deref().is_none_or(str::is_empty))
            .and_then(|tool| workspace_edit(tool, &roots, &session))
            .map_or(Event::Ignore, Event::Edit)
    } else {
        Event::Start
    };
    Ok(Payload {
        session,
        roots,
        prompt: input.last_user_input.unwrap_or_default(),
        event,
    })
}

fn workspace_edit(tool: ToolCall, roots: &[PathBuf], session: &str) -> Option<EditRequest> {
    if tool.args.is_artifact
        || !matches!(
            tool.name.as_str(),
            "write_file"
                | "edit_file"
                | "write_to_file"
                | "replace_file_content"
                | "multi_replace_file_content"
        )
    {
        return None;
    }
    let path = tool
        .args
        .target_file
        .or(tool.args.path)
        .filter(|p| p.file_name().is_some())?;
    let path = if path.is_absolute() {
        normalize(&path)
    } else {
        // Relative tool paths refer to the primary workspace.
        normalize(&roots.first()?.join(path))
    };
    let file = canonical_dir(path.parent()?).join(path.file_name()?);
    let root = roots
        .iter()
        .filter(|root| file.starts_with(root))
        .max_by_key(|root| root.components().count())?;
    Some(EditRequest::new(file, root).with_session(Some(session.into())))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testing::{abs, os_json};

    fn decode(fields: &str) -> Payload {
        parse(&os_json(&format!(
            r#"{{"conversationId":"s","workspacePaths":["/w","/w/deep","/other"],{fields}}}"#
        )))
        .unwrap()
    }

    #[test]
    fn documented_and_current_tools_resolve_paths_in_the_owning_workspace() {
        for (name, field, path, root) in [
            ("write_file", "Path", "a.rs", "/w"),
            ("edit_file", "Path", "/other/a.rs", "/other"),
            ("write_to_file", "TargetFile", "/w/a.rs", "/w"),
            (
                "replace_file_content",
                "TargetFile",
                "/w/deep/a.rs",
                "/w/deep",
            ),
            ("multi_replace_file_content", "TargetFile", "/w/a.rs", "/w"),
        ] {
            let payload = decode(&format!(
                r#""stepIdx":1,"toolCall":{{"name":"{name}","args":{{"{field}":"{path}"}}}}"#
            ));
            let path = if path == "a.rs" { "/w/a.rs" } else { path };
            assert_eq!(
                payload.event,
                Event::Edit(EditRequest::new(abs(path), abs(root)).with_session(Some("s".into())))
            );
        }
    }

    #[test]
    fn failed_artifact_and_outside_workspace_edits_are_ignored() {
        for fields in [
            r#""stepIdx":1,"error":"failed","toolCall":{"name":"edit_file","args":{"Path":"a.rs"}}"#,
            r#""stepIdx":1,"toolCall":{"name":"write_to_file","args":{"TargetFile":"a.rs","IsArtifact":true}}"#,
            r#""stepIdx":1,"toolCall":{"name":"write_file","args":{"Path":"../outside.rs"}}"#,
            r#""stepIdx":1,"toolCall":{"name":"view_file","args":{"Path":"a.rs"}}"#,
            r#""stepIdx":1"#,
        ] {
            assert_eq!(decode(fields).event, Event::Ignore);
        }
    }

    #[test]
    fn stop_requires_an_ordinary_idle_completion_without_a_runtime_error() {
        for (fields, eligible) in [
            (
                r#""executionNum":0,"fullyIdle":true,"terminationReason":"NO_TOOL_CALL","error":"""#,
                true,
            ),
            (
                r#""executionNum":0,"fullyIdle":true,"terminationReason":"model_stop""#,
                true,
            ),
            (
                r#""executionNum":0,"fullyIdle":true,"terminationReason":"model_stop","error":"failed""#,
                false,
            ),
            (
                r#""executionNum":0,"fullyIdle":false,"terminationReason":"model_stop""#,
                false,
            ),
            (
                r#""executionNum":0,"fullyIdle":true,"terminationReason":"error""#,
                false,
            ),
        ] {
            assert_eq!(decode(fields).event, Event::Stop { eligible });
        }
    }
}
