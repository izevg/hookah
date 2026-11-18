//! Pi extension response: one JSON document on stdout, always exit 0.
//!
//! ```json
//! {"v": 1, "block": true, "model_text": "...", "user_text": "..."}
//! ```
//!
//! `block` means "the model has to act": the extension appends `model_text` to the tool result
//! (edit) or sends it as a new message that starts a run (stop). `model_text` without `block` is
//! advice (re-read a rewritten file). `user_text` is for the human only (a notification) and never
//! reaches the model. A missing field is omitted from the JSON.
//!
//! - Edit, `Fail`: `block`, report as `model_text`.
//! - Edit, `Pass` with a `FileRewritten` or missing-tool notice: no `block`, `model_text` holds
//!   the re-read hint and the missing-tool advice.
//! - Stop, `Fail`, first attempt: `block`, report as `model_text`.
//! - Stop, `Fail`, `already_continuing`: do not block again (no loop); a summary as `user_text`.
//! - `Degraded` (and degradation or missing-tool notices): `user_text` only; never blocks.
//! - Anything else: `{"v":1,"block":false}`.

use hookah_application::Outcome;
use serde::Serialize;

use crate::plugins::pi::payload::VERSION;
use crate::protocol::{
    Invocation, Response, blocking_report, degraded_warnings, edit_context, problems, report_text,
};

#[derive(Debug, Serialize, PartialEq, Eq)]
struct Reply {
    v: u32,
    block: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_text: Option<String>,
}

/// Renders `outcome` for the Pi extension.
pub fn render(invocation: &Invocation, outcome: &Outcome) -> Response {
    let warnings = degraded_warnings(outcome);
    let user_text = (!warnings.is_empty()).then(|| warnings.join("\n"));
    let mut reply = Reply {
        v: VERSION,
        block: false,
        model_text: None,
        user_text,
    };
    match invocation {
        Invocation::Ignore | Invocation::Start(_) => {}
        Invocation::Edit(_) | Invocation::EditBatch(_) => match blocking_report(outcome) {
            Some(report) => {
                reply.block = true;
                reply.model_text = Some(format!(
                    "hookah: {} after editing {} - fix before continuing:\n{}",
                    problems(report),
                    edited_files(invocation),
                    report_text(report, outcome)
                ));
            }
            None => reply.model_text = edit_context(outcome),
        },
        Invocation::Stop {
            already_continuing, ..
        } => {
            if let Some(report) = blocking_report(outcome) {
                if *already_continuing {
                    reply.user_text = Some(format!(
                        "hookah: {} still failing; not blocking again.\n{report}",
                        problems(report)
                    ));
                } else {
                    reply.block = true;
                    reply.model_text = Some(format!(
                        "hookah: {} in the files changed in this session - tests or checks \
                         are failing, fix before finishing:\n{}",
                        problems(report),
                        report_text(report, outcome)
                    ));
                }
            }
        }
    }
    Response {
        exit_code: 0,
        stdout: format!("{}\n", serde_json::to_string(&reply).unwrap_or_default()),
        stderr: String::new(),
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::PathBuf;

    use hookah_application::{EditRequest, StopRequest};
    use hookah_domain::finding::{Finding, Location, Severity};
    use hookah_domain::toolchain::{CheckKind, MissingTool};
    use hookah_domain::verdict::{Notice, Report, Verdict};
    use serde_json::{Value, json};

    use super::*;

    fn edit() -> Invocation {
        Invocation::Edit(EditRequest::new(
            PathBuf::from("/w/a.rs"),
            PathBuf::from("/w"),
        ))
    }

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

    fn failing() -> Outcome {
        let f = Finding::new(
            CheckKind::Lint,
            Severity::Error,
            Location::line("a.rs", 3),
            "unused variable",
        );
        Outcome {
            verdict: Verdict::Fail(Report::new(vec![f], 20).unwrap()),
            notices: vec![],
        }
    }

    fn json_of(r: &Response) -> Value {
        assert_eq!(r.exit_code, 0);
        assert!(r.stderr.is_empty());
        assert!(r.stdout.ends_with('\n'));
        serde_json::from_str(&r.stdout).unwrap()
    }

    #[test]
    fn edit_fail_blocks_with_the_report() {
        let v = json_of(&render(&edit(), &failing()));
        assert_eq!(v["v"], 1);
        assert_eq!(v["block"], true);
        let text = v["model_text"].as_str().unwrap();
        assert!(text.starts_with("hookah: 1 problem after editing a.rs - fix before"));
        assert!(text.contains("a.rs:3: unused variable"));
        assert!(v.get("user_text").is_none());
    }

    #[test]
    fn batch_names_every_file() {
        let v = json_of(&render(&batch(), &failing()));
        assert!(
            v["model_text"]
                .as_str()
                .unwrap()
                .starts_with("hookah: 1 problem after editing a.rs, b.rs - fix")
        );
    }

    #[test]
    fn rewrite_and_missing_tool_advise_without_blocking() {
        let o = Outcome {
            verdict: Verdict::Pass,
            notices: vec![
                Notice::FileRewritten("/w/a.rs".into()),
                Notice::MissingTool(MissingTool::new(
                    "ruff",
                    CheckKind::Lint,
                    "pip install ruff",
                )),
            ],
        };
        let v = json_of(&render(&edit(), &o));
        assert_eq!(v["block"], false);
        let text = v["model_text"].as_str().unwrap();
        assert!(text.contains("Re-read") && text.contains("ruff"));
        assert_eq!(
            v["user_text"],
            "hookah: tool not installed: ruff (lint): pip install ruff"
        );
    }

    #[test]
    fn plain_pass_and_ignore_are_empty() {
        let quiet = json!({"v": 1, "block": false});
        assert_eq!(json_of(&render(&edit(), &Outcome::pass())), quiet);
        assert_eq!(json_of(&render(&stop(false), &Outcome::pass())), quiet);
        assert_eq!(json_of(&render(&Invocation::Ignore, &failing())), quiet);
    }

    #[test]
    fn degraded_warns_the_user_only() {
        for inv in [edit(), stop(false)] {
            let v = json_of(&render(&inv, &Outcome::degraded("clippy timed out")));
            assert_eq!(v["block"], false);
            assert!(v.get("model_text").is_none());
            assert!(
                v["user_text"]
                    .as_str()
                    .unwrap()
                    .contains("clippy timed out")
            );
        }
    }

    #[test]
    fn stop_fail_blocks_once() {
        let v = json_of(&render(&stop(false), &failing()));
        assert_eq!(v["block"], true);
        let text = v["model_text"].as_str().unwrap();
        assert!(text.contains("fix before finishing"));
        assert!(text.contains("a.rs:3: unused variable"));
        assert!(v.get("user_text").is_none());
    }

    #[test]
    fn stop_fail_when_already_continuing_only_warns_the_user() {
        let v = json_of(&render(&stop(true), &failing()));
        assert_eq!(v["block"], false);
        assert!(v.get("model_text").is_none());
        let text = v["user_text"].as_str().unwrap();
        assert!(text.contains("1 problem still failing; not blocking again."));
        assert!(text.contains("a.rs:3: unused variable"));
    }

    #[test]
    fn a_session_start_gets_a_quiet_reply() {
        let start = Invocation::Start(hookah_application::StartRequest::new("/w", "s"));
        let quiet = json_of(&render(&Invocation::Ignore, &Outcome::pass()));
        assert_eq!(json_of(&render(&start, &Outcome::pass())), quiet);
    }
}
