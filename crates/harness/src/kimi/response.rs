//! Kimi Code hook response.
//!
//! `PostToolUse` is not blockable in Kimi Code. Per the docs, stdout of an exit-0 hook may be
//! appended to the model's context, so that is the feedback channel.
//!
//! - Edit, `Fail`: exit 0, the report on stdout (plus notices such as the re-read hint).
//! - Edit, `Pass` with a `FileRewritten` notice: exit 0, the re-read hint on stdout.
//! - `Degraded` (and degradation notices): exit 0, warning on stderr only; never blocks.
//! - Anything else: exit 0, no output.

use hookah_application::Outcome;

use crate::protocol::{
    Invocation, Response, blocking_report, degraded_warnings, edit_context, problems, report_text,
};

/// Renders `outcome` for Kimi Code.
pub fn render(invocation: &Invocation, outcome: &Outcome) -> Response {
    let Invocation::Edit(request) = invocation else {
        return Response::silent();
    };
    let stdout = match blocking_report(outcome) {
        Some(report) => format!(
            "hookah: {} after editing {} - fix before continuing:\n{}",
            problems(report),
            request.file.display(),
            report_text(report, outcome)
        ),
        None => edit_context(outcome)
            .map(|text| format!("{text}\n"))
            .unwrap_or_default(),
    };
    let warnings = degraded_warnings(outcome);
    Response {
        exit_code: 0,
        stdout,
        stderr: if warnings.is_empty() {
            String::new()
        } else {
            format!("{}\n", warnings.join("\n"))
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::PathBuf;

    use hookah_application::EditRequest;
    use hookah_domain::finding::{Finding, Location, Severity};
    use hookah_domain::toolchain::CheckKind;
    use hookah_domain::verdict::{Notice, Report, Verdict};

    use super::*;

    fn edit() -> Invocation {
        Invocation::Edit(EditRequest::new(
            PathBuf::from("/w/a.py"),
            PathBuf::from("/w"),
        ))
    }

    fn failing(notices: Vec<Notice>) -> Outcome {
        let f = Finding::new(
            CheckKind::Lint,
            Severity::Error,
            Location::line("a.py", 3),
            "unused import",
        );
        Outcome {
            verdict: Verdict::Fail(Report::new(vec![f], 20).unwrap()),
            notices,
        }
    }

    #[test]
    fn edit_fail_prints_report_and_never_blocks() {
        let r = render(
            &edit(),
            &failing(vec![Notice::FileRewritten("/w/a.py".into())]),
        );
        assert_eq!(r.exit_code, 0);
        assert!(
            r.stdout
                .starts_with("hookah: 1 problem after editing /w/a.py")
        );
        assert!(r.stdout.contains("a.py:3: unused import"));
        assert!(r.stdout.contains("re-read it before editing"));
        assert!(r.stderr.is_empty());
    }

    #[test]
    fn edit_pass_with_rewrite_adds_hint() {
        let o = Outcome {
            verdict: Verdict::Pass,
            notices: vec![Notice::FileRewritten("/w/a.py".into())],
        };
        let r = render(&edit(), &o);
        assert!(r.stdout.contains("Re-read"));
    }

    #[test]
    fn plain_pass_and_ignore_are_silent() {
        assert_eq!(render(&edit(), &Outcome::pass()), Response::silent());
        assert_eq!(
            render(&Invocation::Ignore, &failing(vec![])),
            Response::silent()
        );
    }

    #[test]
    fn degraded_warns_on_stderr_only() {
        let r = render(&edit(), &Outcome::degraded("ruff timed out"));
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.is_empty());
        assert!(r.stderr.contains("ruff timed out"));
    }
}
