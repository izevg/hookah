//! Common harness protocol.
//!
//! - [`Harness`]: the trait every adapter implements: `parse(stdin) -> Invocation` and
//!   `render(invocation, outcome) -> Response`.
//! - [`Invocation`]: which use case a payload asks for.
//! - [`Response`]: what the CLI writes and returns (exit code, stdout, stderr).
//!
//! Also holds the shared fail-open rule: only `Verdict::Fail` may block the agent;
//! `Verdict::Degraded` never does. See [`blocking_report`].

use std::fmt;
use std::path::{Path, PathBuf};

use hookah_application::{EditRequest, Outcome, StopRequest};
use hookah_domain::toolchain::MissingTool;
use hookah_domain::verdict::{Notice, Report, Verdict};

/// What a harness payload asks `hookah` to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    /// A file was just edited: run the edit phase.
    Edit(EditRequest),
    /// Several files were edited by one tool call (for example one patch): run the edit phase
    /// on each and merge the outcomes with [`merge_outcomes`] before rendering.
    EditBatch(Vec<EditRequest>),
    /// The agent wants to stop: run the stop phase.
    Stop {
        /// The use case input.
        request: StopRequest,
        /// The harness says this stop attempt already follows a blocked stop. Blocking again
        /// could loop forever, so adapters must not block in this state.
        already_continuing: bool,
    },
    /// An event `hookah` does not care about.
    Ignore,
}

/// What the CLI writes and returns to the harness.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Response {
    /// Process exit code.
    pub exit_code: i32,
    /// Text for standard output.
    pub stdout: String,
    /// Text for standard error.
    pub stderr: String,
}

impl Response {
    /// Exit 0 with nothing to say.
    pub fn silent() -> Self {
        Self::default()
    }
}

/// A payload that could not be understood. The CLI turns it into a warning and exit code 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadError(pub String);

impl fmt::Display for PayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "malformed hook payload: {}", self.0)
    }
}

impl std::error::Error for PayloadError {}

/// A driving adapter for one agent harness.
pub trait Harness {
    /// Parses the raw payload read from standard input.
    fn parse(&self, stdin: &str) -> Result<Invocation, PayloadError>;

    /// Renders the use case outcome in this harness's protocol.
    fn render(&self, invocation: &Invocation, outcome: &Outcome) -> Response;
}

/// The report that may block the agent, if any. This is the shared fail-open rule: `Pass` and
/// `Degraded` verdicts never block.
pub fn blocking_report(outcome: &Outcome) -> Option<&Report> {
    match &outcome.verdict {
        Verdict::Fail(report) => Some(report),
        Verdict::Pass | Verdict::Degraded(_) => None,
    }
}

/// Merges per-file outcomes into one. A `Fail` in any file wins and combines all findings;
/// otherwise a `Degraded` reason wins over `Pass`. Notices are concatenated.
pub fn merge_outcomes(outcomes: Vec<Outcome>, limit: usize) -> Outcome {
    let mut findings = Vec::new();
    let mut degraded = None;
    let mut notices = Vec::new();
    for outcome in outcomes {
        match outcome.verdict {
            Verdict::Fail(report) => findings.extend(report.findings().iter().cloned()),
            Verdict::Degraded(reason) => degraded = degraded.or(Some(reason)),
            Verdict::Pass => {}
        }
        notices.extend(outcome.notices);
    }
    let verdict = match (Report::new(findings, limit), degraded) {
        (Some(report), _) => Verdict::Fail(report),
        (None, Some(reason)) => Verdict::Degraded(reason),
        (None, None) => Verdict::Pass,
    };
    Outcome { verdict, notices }
}

/// The files a formatter rewrote.
pub fn rewritten_files(outcome: &Outcome) -> Vec<String> {
    outcome
        .notices
        .iter()
        .filter_map(|n| match n {
            Notice::FileRewritten(p) => Some(p.display().to_string()),
            Notice::Degraded(_) | Notice::MissingTool(_) => None,
        })
        .collect()
}

fn missing_tools(outcome: &Outcome) -> Vec<&MissingTool> {
    outcome
        .notices
        .iter()
        .filter_map(|n| match n {
            Notice::MissingTool(t) => Some(t),
            _ => None,
        })
        .collect()
}

/// Model context for a non-blocking edit-phase result: re-read hints for rewritten files and
/// missing-tool notices (the agent should tell the user; `hookah` never installs anything).
pub fn edit_context(outcome: &Outcome) -> Option<String> {
    let mut parts = Vec::new();
    let files = rewritten_files(outcome);
    if !files.is_empty() {
        parts.push(format!(
            "hookah reformatted {}. Re-read the file before editing it again.",
            files.join(", ")
        ));
    }
    let missing = missing_tools(outcome);
    if !missing.is_empty() {
        let lines: Vec<String> = missing
            .iter()
            .map(|t| Notice::MissingTool((*t).clone()).to_string())
            .collect();
        parts.push(format!(
            "hookah skipped some checks: {}. hookah never installs tools; tell the user.",
            lines.join("; ")
        ));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// Warnings meant for the user only: degradation notices, missing tools, and a degraded verdict.
pub fn degraded_warnings(outcome: &Outcome) -> Vec<String> {
    let mut warnings: Vec<String> = outcome
        .notices
        .iter()
        .filter(|n| matches!(n, Notice::Degraded(_)))
        .map(ToString::to_string)
        .collect();
    let missing = missing_tools(outcome);
    if !missing.is_empty() {
        let names: Vec<String> = missing
            .iter()
            .map(|t| format!("{} ({})", t.name, t.kind.name()))
            .collect();
        warnings.push(format!("hookah: tools not installed: {}", names.join(", ")));
    }
    if let Verdict::Degraded(reason) = &outcome.verdict {
        warnings.push(format!("hookah: checks incomplete: {reason}"));
    }
    warnings
}

/// The directory paths are shown relative to: the payload `cwd` when absolute, else the file's
/// parent directory.
pub fn display_root(file: &Path, cwd: Option<&Path>) -> PathBuf {
    match cwd.filter(|c| c.is_absolute()) {
        Some(cwd) => cwd.to_path_buf(),
        None => file.parent().map(Path::to_path_buf).unwrap_or_default(),
    }
}

/// "1 problem" or "N problems".
pub fn problems(report: &Report) -> String {
    match report.findings().len() {
        1 => "1 problem".into(),
        n => format!("{n} problems"),
    }
}

/// The report followed by every notice, one per line.
pub fn report_text(report: &Report, outcome: &Outcome) -> String {
    let mut text = format!("{report}\n");
    for notice in &outcome.notices {
        text.push_str(&format!("{notice}\n"));
    }
    text
}

/// Resolves `path` against `cwd` when relative.
pub fn resolve_path(path: &Path, cwd: Option<&Path>) -> Result<PathBuf, PayloadError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    match cwd.filter(|c| c.is_absolute()) {
        Some(cwd) => Ok(cwd.join(path)),
        None => Err(PayloadError(format!(
            "relative path {} without an absolute cwd",
            path.display()
        ))),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use hookah_domain::finding::{Finding, Location, Severity};
    use hookah_domain::toolchain::CheckKind;

    use super::*;

    fn failing(path: &str) -> Outcome {
        let f = Finding::new(
            CheckKind::Lint,
            Severity::Error,
            Location::line(path, 1),
            "bad",
        );
        Outcome {
            verdict: Verdict::Fail(Report::new(vec![f], 20).unwrap()),
            notices: vec![],
        }
    }

    #[test]
    fn merge_combines_findings_and_prefers_fail_over_degraded() {
        let merged = merge_outcomes(
            vec![failing("a.rs"), Outcome::degraded("slow"), failing("b.rs")],
            20,
        );
        match merged.verdict {
            Verdict::Fail(r) => assert_eq!(r.findings().len(), 2),
            other => panic!("expected fail, got {other:?}"),
        }
    }

    #[test]
    fn missing_tool_reaches_model_context_and_user_warning() {
        let o = Outcome {
            verdict: Verdict::Pass,
            notices: vec![Notice::MissingTool(MissingTool::new(
                "ruff",
                CheckKind::Lint,
                "pip install ruff",
            ))],
        };
        let ctx = edit_context(&o).unwrap();
        assert!(ctx.contains("ruff") && ctx.contains("tell the user"));
        assert!(degraded_warnings(&o)[0].contains("tools not installed: ruff"));
        assert_eq!(edit_context(&Outcome::pass()), None);
    }

    #[test]
    fn merge_degraded_beats_pass_and_empty_passes() {
        let d = merge_outcomes(vec![Outcome::pass(), Outcome::degraded("slow")], 20);
        assert_eq!(d.verdict, Verdict::Degraded("slow".into()));
        assert_eq!(merge_outcomes(vec![], 20).verdict, Verdict::Pass);
    }
}
