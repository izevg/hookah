//! Qoder CLI hook response.
//!
//! The Qoder docs do not say that `PostToolUse` can block, so edit feedback goes through the
//! documented `hookSpecificOutput.additionalContext` field instead of exit code 2.
//!
//! - Edit, `Fail`: exit 0, JSON `hookSpecificOutput` with `hookEventName` `PostToolUse` and
//!   `additionalContext` holding the report (plus the re-read hint for rewritten files).
//! - Edit, `Pass` with a `FileRewritten` notice: the same JSON, with the re-read hint only.
//! - Stop, `Fail`, first attempt: exit 2, report on stderr. Qoder injects stderr into the
//!   conversation and the agent keeps working.
//! - Stop, `Fail`, `stop_hook_active == true`: do not block again. Exit 0, summary on stderr
//!   for the user.
//! - `Degraded` (and degradation notices): exit 0, warning on stderr only; never blocks.
//! - Anything else: exit 0, no output.

use hookah_application::Outcome;
use serde_json::json;

use crate::protocol::{
    Invocation, Response, blocking_report, degraded_warnings, edit_context, problems, report_text,
};

/// Renders `outcome` for Qoder CLI.
pub fn render(invocation: &Invocation, outcome: &Outcome) -> Response {
    match invocation {
        Invocation::Ignore | Invocation::EditBatch(_) => Response::silent(),
        Invocation::Edit(request) => match blocking_report(outcome) {
            Some(report) => context(format!(
                "hookah: {} after editing {} - fix before continuing:\n{}",
                problems(report),
                request.file.display(),
                report_text(report, outcome)
            )),
            None => match edit_context(outcome) {
                Some(text) => context(text).with_warnings(outcome),
                None => warnings(outcome),
            },
        },
        Invocation::Stop {
            already_continuing, ..
        } => match blocking_report(outcome) {
            Some(report) if !already_continuing => Response {
                exit_code: 2,
                stdout: String::new(),
                stderr: format!(
                    "hookah: {} in the files changed in this session - tests or checks are \
                     failing, fix before finishing:\n{}",
                    problems(report),
                    report_text(report, outcome)
                ),
            },
            Some(report) => Response {
                exit_code: 0,
                stdout: String::new(),
                stderr: format!(
                    "hookah: {} still failing; not blocking again.\n{report}\n",
                    problems(report)
                ),
            },
            None => warnings(outcome),
        },
    }
}

fn context(text: String) -> Response {
    let value = json!({
        "hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": text}
    });
    Response {
        exit_code: 0,
        stdout: format!("{value}\n"),
        stderr: String::new(),
    }
}

fn warnings(outcome: &Outcome) -> Response {
    let lines = degraded_warnings(outcome);
    Response {
        exit_code: 0,
        stdout: String::new(),
        stderr: if lines.is_empty() {
            String::new()
        } else {
            format!("{}\n", lines.join("\n"))
        },
    }
}

impl Response {
    fn with_warnings(mut self, outcome: &Outcome) -> Self {
        self.stderr = warnings(outcome).stderr;
        self
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
    fn edit_fail_feeds_report_as_context() {
        let o = failing(vec![Notice::FileRewritten("/w/a.rs".into())]);
        let r = render(&edit(), &o);
        assert_eq!(r.exit_code, 0);
        let v: Value = serde_json::from_str(&r.stdout).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        let ctx = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(ctx.starts_with("hookah: 1 problem after editing /w/a.rs"));
        assert!(ctx.contains("a.rs:3: unused variable"));
    }

    #[test]
    fn edit_pass_with_rewrite_adds_context() {
        let o = Outcome {
            verdict: Verdict::Pass,
            notices: vec![Notice::FileRewritten("/w/a.rs".into())],
        };
        let v: Value = serde_json::from_str(&render(&edit(), &o).stdout).unwrap();
        assert!(
            v["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .contains("Re-read")
        );
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
        assert!(r.stderr.contains("not blocking again"));
    }
}
