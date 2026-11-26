//! Antigravity CLI hooks. The edit hook records feedback for the next PreInvocation.

mod payload;

pub use payload::{EDIT_TOOLS, Event, Payload, parse};

use hookah_application::Outcome;
use serde_json::json;

use crate::protocol::{Response, blocking_report, degraded_warnings, edit_context};

/// Concise edit feedback for the next model invocation.
pub fn edit_feedback(outcome: &Outcome) -> Option<String> {
    let report = blocking_report(outcome).map(ToString::to_string);
    match (report, edit_context(outcome)) {
        (Some(report), Some(context)) => Some(format!("{report}\n{context}")),
        (report, context) => report.or(context),
    }
}

/// An acknowledgement for SessionStart, PostToolUse, and PreInvocation.
pub fn acknowledge() -> Response {
    json_response(json!({}))
}

/// Adds the pending edit feedback to the next model call.
pub fn inject(feedback: String) -> Response {
    json_response(json!({
        "injectSteps": [{"systemMessage": {"systemMessage": feedback}}]
    }))
}

/// Continues the execution loop only when the caller supplies a real finding.
pub fn stop(reason: Option<String>) -> Response {
    match reason {
        Some(reason) => json_response(json!({"decision": "continue", "reason": reason})),
        None => json_response(json!({"decision": "allow"})),
    }
}

/// Adds runtime warnings alongside the protocol response.
pub fn warn(response: &mut Response, outcome: &Outcome) {
    for warning in degraded_warnings(outcome) {
        response.stderr.push_str(&warning);
        response.stderr.push('\n');
    }
}

fn json_response(value: serde_json::Value) -> Response {
    Response {
        stdout: format!("{value}\n"),
        ..Response::silent()
    }
}
