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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use hookah_domain::finding::{Finding, Location, Severity};
    use hookah_domain::toolchain::CheckKind;
    use hookah_domain::verdict::{Notice, Report, Verdict};

    #[test]
    fn edit_feedback_combines_findings_and_a_formatter_warning() {
        let finding = Finding::new(
            CheckKind::Lint,
            Severity::Error,
            Location::line("a.rs", 5),
            "invalid value",
        );
        let mut outcome = Outcome {
            verdict: Verdict::Fail(Report::new(vec![finding], 20).unwrap()),
            notices: vec![Notice::FileRewritten("b.rs".into())],
        };
        let feedback = edit_feedback(&outcome).unwrap();
        assert!(feedback.contains("a.rs:5: invalid value"));
        assert!(feedback.contains("hookah reformatted b.rs") && feedback.contains("Re-read"));

        outcome.verdict = Verdict::Pass;
        let feedback = edit_feedback(&outcome).unwrap();
        assert!(feedback.contains("b.rs") && !feedback.contains("a.rs"));
    }

    #[test]
    fn model_and_stop_responses_match_the_native_json_contract() {
        let as_json = |response: Response| {
            assert_eq!(response.exit_code, 0);
            serde_json::from_str::<serde_json::Value>(&response.stdout).unwrap()
        };
        assert_eq!(as_json(stop(None)), json!({"decision": "allow"}));
        assert_eq!(
            as_json(stop(Some("reason".into()))),
            json!({"decision": "continue", "reason": "reason"})
        );
        assert_eq!(
            as_json(inject("feedback".into())),
            json!({"injectSteps": [{"systemMessage": {"systemMessage": "feedback"}}]})
        );
    }

    #[test]
    fn degraded_checks_warn_without_injecting_model_feedback() {
        let outcome = Outcome::degraded("tool timed out");
        let mut response = acknowledge();
        warn(&mut response, &outcome);
        assert_eq!(edit_feedback(&outcome), None);
        assert_eq!(response.exit_code, 0);
        assert_eq!(response.stdout, "{}\n");
        assert!(response.stderr.contains("tool timed out"));
    }
}
