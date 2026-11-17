//! Codex CLI hook response.
//!
//! Mapping, by invocation and verdict (all exit 0, JSON on stdout; `Stop` requires JSON):
//!
//! - Edit, `Fail`: `{"decision":"block","reason":...}`. The patch already ran; Codex replaces
//!   the tool result with the reason, so the model sees the report.
//! - Edit, `Pass` with a `FileRewritten` notice: `hookSpecificOutput.additionalContext` telling
//!   the model to re-read the file.
//! - Stop, `Fail`, first attempt: `{"decision":"block","reason":...}`; Codex starts a
//!   continuation turn with the reason as the prompt.
//! - Stop, `Fail`, `stop_hook_active == true`: do not block again (no loop). A `systemMessage`
//!   summarizes the failures for the user.
//! - `Degraded` (and degradation notices): `systemMessage` for the user only; never blocks.
//! - Anything else: no output.

use hookah_application::Outcome;
use serde_json::{Value, json};

use crate::protocol::{
    Invocation, Response, blocking_report, degraded_warnings, edit_context, problems, report_text,
};

/// Renders `outcome` for Codex CLI.
pub fn render(invocation: &Invocation, outcome: &Outcome) -> Response {
    match invocation {
        Invocation::Ignore => Response::silent(),
        Invocation::Edit(_) | Invocation::EditBatch(_) => match blocking_report(outcome) {
            Some(report) => block(format!(
                "hookah: {} after editing {} - fix before continuing:\n{}",
                problems(report),
                edited_files(invocation),
                report_text(report, outcome)
            )),
            None => allow(outcome, true),
        },
        Invocation::Stop {
            already_continuing, ..
        } => match blocking_report(outcome) {
            Some(report) if !already_continuing => block(format!(
                "hookah: {} in the files changed in this session - tests or checks are \
                 failing, fix before finishing:\n{}",
                problems(report),
                report_text(report, outcome)
            )),
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

fn edited_files(invocation: &Invocation) -> String {
    let files: Vec<String> = match invocation {
        Invocation::Edit(r) => vec![r.shown_file().display().to_string()],
        Invocation::EditBatch(rs) => rs
            .iter()
            .map(|r| r.shown_file().display().to_string())
            .collect(),
        _ => vec![],
    };
    files.join(", ")
}

fn block(reason: String) -> Response {
    json_response(json!({"decision": "block", "reason": reason}))
}

fn allow(outcome: &Outcome, edit_phase: bool) -> Response {
    let mut out = serde_json::Map::new();
    if let Some(context) = edit_context(outcome).filter(|_| edit_phase) {
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
    use std::path::PathBuf;

    use hookah_application::{EditRequest, StopRequest};
    use hookah_domain::finding::{Finding, Location, Severity};
    use hookah_domain::toolchain::CheckKind;
    use hookah_domain::verdict::{Notice, Report, Verdict};

    use super::*;

    fn batch() -> Invocation {
        Invocation::EditBatch(vec![
            EditRequest::new(PathBuf::from("/w/a.rs"), PathBuf::from("/w")),
            EditRequest::new(PathBuf::from("/w/b.rs"), PathBuf::from("/w")),
        ])
    }

    fn stop(already_continuing: bool) -> Invocation {
        Invocation::Stop {
            request: StopRequest::new(PathBuf::from("/w")),
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
    fn edit_fail_blocks_with_files_report_and_notices() {
        let o = failing(vec![Notice::FileRewritten("/w/a.rs".into())]);
        let r = render(&batch(), &o);
        assert_eq!(r.exit_code, 0);
        let v = stdout_json(&r);
        assert_eq!(v["decision"], "block");
        let reason = v["reason"].as_str().unwrap();
        assert!(reason.starts_with("hookah: 1 problem after editing a.rs, b.rs"));
        assert!(reason.contains("a.rs:3: unused variable"));
        assert!(reason.contains("re-read it before editing"));
    }

    #[test]
    fn edit_pass_with_rewrite_adds_context() {
        let o = Outcome {
            verdict: Verdict::Pass,
            notices: vec![Notice::FileRewritten("/w/a.rs".into())],
        };
        let v = stdout_json(&render(&batch(), &o));
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        assert!(
            v["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .contains("Re-read")
        );
    }

    #[test]
    fn plain_pass_is_silent() {
        assert_eq!(render(&batch(), &Outcome::pass()), Response::silent());
        assert_eq!(render(&stop(false), &Outcome::pass()), Response::silent());
        assert_eq!(
            render(&Invocation::Ignore, &failing(vec![])),
            Response::silent()
        );
    }

    #[test]
    fn degraded_warns_the_user_only() {
        for inv in [batch(), stop(false)] {
            let r = render(&inv, &Outcome::degraded("clippy timed out"));
            assert_eq!(r.exit_code, 0);
            let v = stdout_json(&r);
            assert!(
                v["systemMessage"]
                    .as_str()
                    .unwrap()
                    .contains("clippy timed out")
            );
            assert!(v.get("decision").is_none());
        }
    }

    #[test]
    fn stop_fail_blocks_once() {
        let v = stdout_json(&render(&stop(false), &failing(vec![])));
        assert_eq!(v["decision"], "block");
        assert!(
            v["reason"]
                .as_str()
                .unwrap()
                .contains("fix before finishing")
        );
    }

    #[test]
    fn stop_fail_when_already_continuing_does_not_block() {
        let v = stdout_json(&render(&stop(true), &failing(vec![])));
        assert!(v.get("decision").is_none());
        assert!(
            v["systemMessage"]
                .as_str()
                .unwrap()
                .contains("not blocking again")
        );
    }
}
