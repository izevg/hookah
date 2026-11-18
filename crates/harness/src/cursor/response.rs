//! Cursor hook response.
//!
//! `afterFileEdit` has no documented output, so the edit phase cannot reach the model; the
//! stop phase is the enforcement point. Mapping, by invocation and verdict:
//!
//! - Edit, any outcome: exit 0, never blocking. Problems, the re-read hint for a rewritten file,
//!   and warnings go to stderr, which Cursor shows in its Hooks output for the user.
//! - Stop, `Fail`, first attempt (`loop_count == 0`): exit 0, stdout JSON `followup_message`
//!   holding the report. Cursor submits it as the next user message, so the agent keeps working.
//! - Stop, `Fail`, `loop_count > 0`: do not follow up again. Exit 0, summary on stderr.
//! - `Degraded` (and degradation notices): exit 0, warning on stderr only; never blocks.
//! - Session start and anything else: exit 0, nothing on stdout (a `sessionStart` hook's
//!   `additional_context` would go to the model).

use hookah_application::Outcome;
use serde_json::json;

use crate::protocol::{
    Invocation, Response, blocking_report, degraded_warnings, edit_context, problems, report_text,
    start_response,
};

/// Renders `outcome` for Cursor.
pub fn render(invocation: &Invocation, outcome: &Outcome) -> Response {
    match invocation {
        Invocation::Ignore | Invocation::EditBatch(_) => Response::silent(),
        Invocation::Start(_) => start_response(outcome),
        Invocation::Edit(request) => {
            let mut lines = Vec::new();
            if let Some(report) = blocking_report(outcome) {
                lines.push(format!(
                    "hookah: {} after editing {}:\n{}",
                    problems(report),
                    request.shown_file().display(),
                    report_text(report, outcome).trim_end()
                ));
            } else if let Some(text) = edit_context(outcome) {
                lines.push(text);
            }
            lines.extend(degraded_warnings(outcome));
            stderr(&lines)
        }
        Invocation::Stop {
            already_continuing, ..
        } => match blocking_report(outcome) {
            Some(report) if !already_continuing => {
                let message = format!(
                    "hookah: {} in the files changed in this session - tests or checks are \
                     failing, fix before finishing:\n{}",
                    problems(report),
                    report_text(report, outcome)
                );
                Response {
                    exit_code: 0,
                    stdout: format!("{}\n", json!({ "followup_message": message })),
                    stderr: String::new(),
                }
            }
            Some(report) => stderr(&[format!(
                "hookah: {} still failing; not asking the agent to continue again.\n{report}",
                problems(report)
            )]),
            None => stderr(&degraded_warnings(outcome)),
        },
    }
}

fn stderr(lines: &[String]) -> Response {
    Response {
        exit_code: 0,
        stdout: String::new(),
        stderr: lines.iter().map(|l| format!("{l}\n")).collect(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::PathBuf;

    use hookah_application::{EditRequest, StartRequest, StopRequest};
    use hookah_domain::finding::{Finding, Location, Severity};
    use hookah_domain::toolchain::CheckKind;
    use hookah_domain::verdict::{Notice, Report, Verdict};
    use serde_json::Value;

    use super::*;

    fn edit() -> Invocation {
        Invocation::Edit(EditRequest::new(
            PathBuf::from("/w/a.rs"),
            PathBuf::from("/w"),
        ))
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

    #[test]
    fn edit_fail_is_for_the_user_and_never_blocks() {
        let r = render(&edit(), &failing(vec![]));
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.is_empty());
        assert!(
            r.stderr
                .starts_with("hookah: 1 problem after editing a.rs:\n")
        );
        assert!(r.stderr.contains("a.rs:3: unused variable"));
    }

    #[test]
    fn edit_pass_with_a_rewrite_says_so_on_stderr() {
        let o = Outcome {
            verdict: Verdict::Pass,
            notices: vec![Notice::FileRewritten("/w/a.rs".into())],
        };
        let r = render(&edit(), &o);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.is_empty());
        assert!(r.stderr.contains("Re-read"));
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
    fn degraded_warns_on_stderr_only() {
        for inv in [edit(), stop(false)] {
            let r = render(&inv, &Outcome::degraded("clippy timed out"));
            assert_eq!(r.exit_code, 0);
            assert!(r.stdout.is_empty());
            assert!(r.stderr.contains("clippy timed out"));
        }
    }

    #[test]
    fn stop_fail_continues_the_agent_once_with_a_followup_message() {
        let r = render(&stop(false), &failing(vec![]));
        assert_eq!(r.exit_code, 0);
        assert!(r.stderr.is_empty());
        let v: Value = serde_json::from_str(&r.stdout).unwrap();
        let message = v["followup_message"].as_str().unwrap();
        assert!(message.contains("fix before finishing"));
        assert!(message.contains("a.rs:3: unused variable"));
    }

    #[test]
    fn stop_fail_after_a_followup_does_not_continue_again() {
        let r = render(&stop(true), &failing(vec![]));
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.is_empty());
        assert!(r.stderr.contains("not asking the agent to continue again"));
        assert!(r.stderr.contains("a.rs:3: unused variable"));
    }

    #[test]
    fn a_session_start_is_silent_on_stdout_and_warns_on_degradation() {
        let start = Invocation::Start(StartRequest::new("/w", "s"));
        assert_eq!(render(&start, &Outcome::pass()), Response::silent());
        let r = render(&start, &Outcome::degraded("no store"));
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.is_empty());
        assert!(r.stderr.contains("no store"));
    }
}
