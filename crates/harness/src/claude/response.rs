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
//! - Anything else: exit 0, no output.

use hookah_application::Outcome;
use hookah_domain::verdict::{Notice, Report, Verdict};
use serde_json::{Value, json};

use crate::protocol::{Invocation, Response, blocking_report};

/// Renders `outcome` for Claude Code.
pub fn render(invocation: &Invocation, outcome: &Outcome) -> Response {
    match invocation {
        Invocation::Ignore => Response::silent(),
        Invocation::Edit(request) => match blocking_report(outcome) {
            Some(report) => block(
                format!(
                    "hookah: {} after editing {} - fix before continuing:",
                    problems(report),
                    request.file.display()
                ),
                report,
                outcome,
            ),
            None => allow(outcome, true),
        },
        Invocation::Stop {
            already_continuing, ..
        } => match blocking_report(outcome) {
            Some(report) if !already_continuing => block(
                format!(
                    "hookah: {} in the files changed in this session - tests or checks are \
                     failing, fix before finishing:",
                    problems(report)
                ),
                report,
                outcome,
            ),
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

fn problems(report: &Report) -> String {
    match report.findings().len() {
        1 => "1 problem".into(),
        n => format!("{n} problems"),
    }
}

fn block(header: String, report: &Report, outcome: &Outcome) -> Response {
    let mut stderr = format!("{header}\n{report}\n");
    for notice in &outcome.notices {
        stderr.push_str(&format!("{notice}\n"));
    }
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
    let rewritten: Vec<String> = outcome
        .notices
        .iter()
        .filter_map(|n| match n {
            Notice::FileRewritten(p) => Some(p.display().to_string()),
            Notice::Degraded(_) => None,
        })
        .collect();
    if edit_phase && !rewritten.is_empty() {
        let context = format!(
            "hookah reformatted {}. Re-read the file before editing it again.",
            rewritten.join(", ")
        );
        out.insert(
            "hookSpecificOutput".into(),
            json!({"hookEventName": "PostToolUse", "additionalContext": context}),
        );
    }
    let mut warnings: Vec<String> = outcome
        .notices
        .iter()
        .filter(|n| matches!(n, Notice::Degraded(_)))
        .map(ToString::to_string)
        .collect();
    if let Verdict::Degraded(reason) = &outcome.verdict {
        warnings.push(format!("hookah: checks incomplete: {reason}"));
    }
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

    use super::*;

    fn edit() -> Invocation {
        Invocation::Edit(EditRequest {
            file: PathBuf::from("/w/a.rs"),
        })
    }

    fn stop(already_continuing: bool) -> Invocation {
        Invocation::Stop {
            request: StopRequest {
                root: PathBuf::from("/w"),
            },
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
                .starts_with("hookah: 1 problem after editing /w/a.rs")
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
        assert!(r.stderr.contains("fix before finishing"));
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
}
