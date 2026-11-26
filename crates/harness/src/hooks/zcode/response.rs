use hookah_application::Outcome;
use hookah_domain::session::Phase;
use serde_json::json;

use crate::protocol::{
    Invocation, Response, blocking_report, degraded_warnings, edit_context, problems,
    report_feedback, start_response,
};

pub(super) fn render(invocation: &Invocation, outcome: &Outcome) -> Response {
    let mut response = Response::silent();
    match invocation {
        Invocation::Ignore => return response,
        Invocation::Start(_) => return start_response(outcome),
        Invocation::Edit(_) | Invocation::EditBatch(_) => {
            let context = blocking_report(outcome)
                .map(|report| report_feedback(Phase::Edit, report, outcome))
                .or_else(|| edit_context(outcome));
            if let Some(context) = context {
                response.stdout = format!(
                    "{}\n",
                    json!({"hookSpecificOutput": {
                        "hookEventName": "PostToolUse",
                        "additionalContext": context
                    }})
                );
            }
        }
        Invocation::Stop {
            already_continuing, ..
        } => {
            if let Some(report) = blocking_report(outcome) {
                if *already_continuing {
                    response.stderr =
                        format!("hookah: {} still failing.\n{report}\n", problems(report));
                } else {
                    let reason = report_feedback(Phase::Stop, report, outcome);
                    response.stdout =
                        format!("{}\n", json!({"decision": "block", "reason": reason}));
                }
            }
        }
    }
    for warning in degraded_warnings(outcome) {
        response.stderr.push_str(&warning);
        response.stderr.push('\n');
    }
    response
}
