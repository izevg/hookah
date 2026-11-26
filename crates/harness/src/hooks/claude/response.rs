//! Claude Code hook response.
//!
//! Mapping, by invocation and verdict:
//!
//! - Edit, `Fail`: exit 2, stderr holds a header, the report, and the notices. The tool already
//!   ran, so Claude Code shows stderr to the model as feedback.
//! - Edit, `Pass` with a `FileRewritten` notice: exit 0, stdout JSON with
//!   `hookSpecificOutput.additionalContext` telling the model to re-read the file.
//! - Stop, `Fail`, first attempt: exit 2, stderr holds the report; Claude Code keeps the agent
//!   working instead of stopping.
//! - Stop, `Fail`, `stop_hook_active == true`: do NOT block again (that could loop forever).
//!   Exit 0 with a `systemMessage` that summarizes the failures for the user.
//! - `Degraded` (and degradation notices): exit 0, `systemMessage` warning for the user only.
//!   Never blocks and never reaches the model.
//! - Session start: nothing on stdout and a stderr warning when the baseline could not start. An
//!   update notice (hookah updated itself, or a newer version exists) is a `systemMessage`: it
//!   reaches the user, never the model.
//! - Anything else: exit 0, no output.

use hookah_application::Outcome;
use hookah_domain::session::Phase;
use hookah_domain::verdict::Notice;
use serde_json::{Value, json};

use crate::protocol::{
    Invocation, Response, blocking_report, degraded_warnings, edit_context, problems,
    report_feedback, start_response,
};

/// Renders `outcome` for Claude Code.
pub fn render(invocation: &Invocation, outcome: &Outcome) -> Response {
    match invocation {
        Invocation::Ignore | Invocation::EditBatch(_) => Response::silent(),
        Invocation::Start(_) => start(outcome),
        Invocation::Edit(_) => match blocking_report(outcome) {
            Some(report) => block(report_feedback(Phase::Edit, report, outcome)),
            None => allow(outcome, true),
        },
        Invocation::Stop {
            already_continuing, ..
        } => match blocking_report(outcome) {
            Some(report) if !already_continuing => {
                block(report_feedback(Phase::Stop, report, outcome))
            }
            Some(report) => json_response(json!({
                "systemMessage": format!(
                    "hookah: {} still failing; not blocking again.\n{report}",
                    problems(report)
                ),
            })),
            None => allow(outcome, false),
        },
    }
}

/// Session start: the shared answer, except that update notices go out as a `systemMessage`.
fn start(outcome: &Outcome) -> Response {
    let updates: Vec<String> = outcome
        .notices
        .iter()
        .filter(|n| matches!(n, Notice::Update(_)))
        .map(ToString::to_string)
        .collect();
    let mut rest = outcome.clone();
    rest.notices.retain(|n| !matches!(n, Notice::Update(_)));
    let mut response = start_response(&rest);
    if !updates.is_empty() {
        response.stdout = json_response(json!({ "systemMessage": updates.join("\n") })).stdout;
    }
    response
}

fn block(stderr: String) -> Response {
    Response {
        exit_code: 2,
        stdout: String::new(),
        stderr,
    }
}

/// Non-blocking output: model context for rewritten files (edit phase only) and a user warning
/// for degradation.
fn allow(outcome: &Outcome, edit_phase: bool) -> Response {
    let mut out = serde_json::Map::new();
    if edit_phase && let Some(context) = edit_context(outcome) {
        out.insert(
            "hookSpecificOutput".into(),
            json!({"hookEventName": "PostToolUse", "additionalContext": context}),
        );
    }
    let warnings = degraded_warnings(outcome);
    if !warnings.is_empty() {
        out.insert("systemMessage".into(), Value::String(warnings.join("\n")));
    }
    if out.is_empty() {
        Response::silent()
    } else {
        json_response(Value::Object(out))
    }
}

fn json_response(value: Value) -> Response {
    Response {
        exit_code: 0,
        stdout: format!("{value}\n"),
        stderr: String::new(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use hookah_application::{EditRequest, StopRequest};
    use hookah_domain::finding::{Finding, Location, Severity};
    use hookah_domain::toolchain::{CheckKind, MissingTool};
    use hookah_domain::verdict::{Report, Verdict};

    use super::*;

    fn missing_notice() -> Notice {
        Notice::MissingTool(MissingTool::new(
            "cargo-mutants",
            CheckKind::Mutation,
            "cargo install cargo-mutants",
        ))
    }

    fn edit() -> Invocation {
        Invocation::Edit(EditRequest::new("/w/a.rs", "/w"))
    }

    fn stop(already_continuing: bool) -> Invocation {
        Invocation::Stop {
            request: StopRequest::new("/w"),
            already_continuing,
        }
    }

    fn failing(notices: Vec<Notice>) -> Outcome {
        let f = Finding::new(
            CheckKind::Lint,
            Severity::Error,
            Location::line("a.rs", 3),
            "unused variable",
        );
        Outcome {
            verdict: Verdict::Fail(Report::new(vec![f], 20).unwrap()),
            notices,
        }
    }

    fn stdout_json(r: &Response) -> Value {
        serde_json::from_str(&r.stdout).unwrap()
    }

    #[test]
    fn edit_fail_blocks_with_report_and_notices() {
        let o = failing(vec![Notice::FileRewritten("/w/a.rs".into())]);
        let r = render(&edit(), &o);
        assert_eq!(r.exit_code, 2);
        assert!(r.stdout.is_empty());
        assert!(
            r.stderr
                .starts_with("hookah: 1 problem. Fix before continuing.")
        );
        assert!(r.stderr.contains("a.rs:3: unused variable"));
        assert!(r.stderr.contains("re-read it before editing"));
    }

    #[test]
    fn edit_pass_with_rewrite_adds_context() {
        let o = Outcome {
            verdict: Verdict::Pass,
            notices: vec![Notice::FileRewritten("/w/a.rs".into())],
        };
        let r = render(&edit(), &o);
        assert_eq!(r.exit_code, 0);
        assert!(r.stderr.is_empty());
        let v = stdout_json(&r);
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        let ctx = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(ctx.contains("/w/a.rs") && ctx.contains("Re-read"));
    }

    #[test]
    fn edit_pass_with_missing_tool_warns_the_user_only() {
        let o = Outcome {
            verdict: Verdict::Pass,
            notices: vec![missing_notice()],
        };
        let r = render(&edit(), &o);
        assert_eq!(r.exit_code, 0);
        let v = stdout_json(&r);
        assert!(v.get("hookSpecificOutput").is_none());
        let msg = v["systemMessage"].as_str().unwrap();
        assert_eq!(
            msg,
            "hookah: tool not installed: cargo-mutants (mutation): cargo install cargo-mutants"
        );
    }

    #[test]
    fn stop_pass_with_missing_tool_warns_the_user_only() {
        let o = Outcome {
            verdict: Verdict::Pass,
            notices: vec![missing_notice()],
        };
        let r = render(&stop(false), &o);
        assert_eq!(r.exit_code, 0);
        let v = stdout_json(&r);
        assert!(v.get("hookSpecificOutput").is_none());
        assert!(
            v["systemMessage"]
                .as_str()
                .unwrap()
                .contains("cargo-mutants")
        );
    }

    #[test]
    fn edit_fail_lists_missing_tools_in_stderr() {
        let r = render(&edit(), &failing(vec![missing_notice()]));
        assert_eq!(r.exit_code, 2);
        assert!(r.stderr.contains("cargo-mutants not installed"));
    }

    #[test]
    fn plain_pass_is_silent() {
        assert_eq!(render(&edit(), &Outcome::pass()), Response::silent());
        assert_eq!(render(&stop(false), &Outcome::pass()), Response::silent());
        assert_eq!(
            render(&Invocation::Ignore, &failing(vec![])),
            Response::silent()
        );
    }

    #[test]
    fn degraded_warns_the_user_only() {
        for inv in [edit(), stop(false)] {
            let r = render(&inv, &Outcome::degraded("clippy timed out"));
            assert_eq!(r.exit_code, 0);
            let v = stdout_json(&r);
            assert!(
                v["systemMessage"]
                    .as_str()
                    .unwrap()
                    .contains("clippy timed out")
            );
            assert!(v.get("hookSpecificOutput").is_none());
        }
    }

    #[test]
    fn stop_fail_blocks_once() {
        let r = render(&stop(false), &failing(vec![]));
        assert_eq!(r.exit_code, 2);
        assert!(r.stderr.contains("Fix before finishing"));
        assert!(r.stderr.contains("a.rs:3: unused variable"));
    }

    #[test]
    fn stop_fail_when_already_continuing_does_not_block() {
        let r = render(&stop(true), &failing(vec![]));
        assert_eq!(r.exit_code, 0);
        assert!(r.stderr.is_empty());
        let v = stdout_json(&r);
        let msg = v["systemMessage"].as_str().unwrap();
        assert!(msg.contains("not blocking again") && msg.contains("a.rs:3"));
    }

    #[test]
    fn a_session_start_is_silent_on_stdout_and_warns_on_degradation() {
        let start = Invocation::Start(hookah_application::StartRequest::new("/w", "s"));
        assert_eq!(render(&start, &Outcome::pass()), Response::silent());
        let r = render(&start, &Outcome::degraded("no store"));
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.is_empty());
        assert!(r.stderr.contains("no store"));
    }

    #[test]
    fn a_session_start_shows_an_update_notice_to_the_user_only() {
        let start = Invocation::Start(hookah_application::StartRequest::new("/w", "s"));
        let mut outcome = Outcome::pass();
        outcome
            .notices
            .push(Notice::Update("updated hookah from 1.0.0 to 1.1.0".into()));
        let r = render(&start, &outcome);
        assert_eq!(r.exit_code, 0);
        assert!(r.stderr.is_empty());
        let json: Value = serde_json::from_str(&r.stdout).unwrap();
        assert_eq!(
            json,
            json!({"systemMessage": "hookah: updated hookah from 1.0.0 to 1.1.0"})
        );
        assert!(
            json.get("hookSpecificOutput").is_none(),
            "never model context"
        );
    }
}
