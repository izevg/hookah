//! Use case input and output, and the run log shared by both use cases.
//!
//! `EditRequest` (changed file) and `StopRequest` (repository root) come in, and an `Outcome`
//! (verdict plus notices) goes out. Harness adapters translate to and from these.
//!
//! A `StartRequest` (session start) asks for the session baseline.
//!
//! The edit and stop requests carry the agent session id when the harness provides one; it keys
//! the baseline. Both requests carry a display root, the directory where the agent stands (the harness `cwd`).
//! Finding paths are rendered relative to it, or absolute when they lie outside it. Filtering
//! works on project-relative paths; only the final rendering uses the display root.

use std::path::{Path, PathBuf};

use hookah_domain::finding::Finding;
use hookah_domain::light::Skip;
use hookah_domain::path::display_path;
use hookah_domain::project::Project;
use hookah_domain::toolchain::{CheckKind, Tool, Toolchain};
use hookah_domain::verdict::{Notice, Report, ReportingFilter, Verdict};

use crate::ports::Ports;

/// Input of the edit phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditRequest {
    /// Absolute path of the file the agent just changed.
    pub file: PathBuf,
    /// Absolute directory finding paths are rendered relative to.
    pub display_root: PathBuf,
    /// The agent session id, if the harness provides one.
    pub session: Option<String>,
}

impl EditRequest {
    /// Builds a request for `file`; `display_root` is the harness `cwd`.
    pub fn new(file: impl Into<PathBuf>, display_root: impl Into<PathBuf>) -> Self {
        Self {
            file: file.into(),
            display_root: display_root.into(),
            session: None,
        }
    }

    /// Tags the request with the agent session id.
    #[must_use]
    pub fn with_session(mut self, session: Option<String>) -> Self {
        self.session = session;
        self
    }

    /// The edited file as shown to the agent: the same rule as for finding paths.
    pub fn shown_file(&self) -> PathBuf {
        display_path(&self.file, &self.display_root)
    }
}

/// Input of the stop phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopRequest {
    /// Absolute root (normally the repository root) to look for changed files under.
    pub root: PathBuf,
    /// Absolute directory finding paths are rendered relative to.
    pub display_root: PathBuf,
    /// The agent session id, if the harness provides one.
    pub session: Option<String>,
}

impl StopRequest {
    /// Builds a request; the display root defaults to `root`, the harness `cwd`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            display_root: root.clone(),
            root,
            session: None,
        }
    }

    /// Tags the request with the agent session id.
    #[must_use]
    pub fn with_session(mut self, session: Option<String>) -> Self {
        self.session = session;
        self
    }

    /// Renders finding paths relative to `display_root` instead of `root`.
    pub fn with_display_root(mut self, display_root: impl Into<PathBuf>) -> Self {
        self.display_root = display_root.into();
        self
    }
}

/// Input of the session start: the agent session began in `cwd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartRequest {
    /// Absolute directory the agent works in.
    pub cwd: PathBuf,
    /// The agent session id.
    pub session: String,
}

impl StartRequest {
    /// Builds a request.
    pub fn new(cwd: impl Into<PathBuf>, session: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            session: session.into(),
        }
    }
}

/// What the use case hands back to the harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The decision.
    pub verdict: Verdict,
    /// Extra messages for the agent, shown whatever the verdict is.
    pub notices: Vec<Notice>,
}

impl Outcome {
    /// Nothing to report.
    pub fn pass() -> Self {
        Self {
            verdict: Verdict::Pass,
            notices: vec![],
        }
    }

    /// `hookah` could not do its job; never blocks the agent.
    pub fn degraded(reason: impl Into<String>) -> Self {
        Self {
            verdict: Verdict::Degraded(reason.into()),
            notices: vec![],
        }
    }
}

/// Collects findings, notices, and failures while tools run.
#[derive(Default, Clone)]
pub(crate) struct RunLog {
    pub findings: Vec<Finding>,
    pub notices: Vec<Notice>,
    failures: Vec<String>,
}

impl RunLog {
    /// Records a degradation reason once.
    pub fn fail(&mut self, reason: String) {
        if !self.failures.contains(&reason) {
            self.failures.push(reason);
        }
    }

    /// Tells the agent a check was skipped because its input was over budget. Never a failure.
    pub fn skipped(&mut self, skip: &Skip) {
        let notice = Notice::Degraded(skip.to_string());
        if !self.notices.contains(&notice) {
            self.notices.push(notice);
        }
    }

    /// The degradation reasons recorded so far.
    pub fn failures(&self) -> &[String] {
        &self.failures
    }

    /// Number of degradation reasons recorded so far.
    pub fn failure_count(&self) -> usize {
        self.failures.len()
    }

    /// Runs `tool` on `targets`, then parses its output. The detector may turn the tool into
    /// several commands, or none. Every error becomes a failure entry.
    pub fn run_tool(
        &mut self,
        ports: &Ports<'_>,
        project: &Project,
        tool: &Tool,
        targets: &[String],
    ) {
        for tool in &ports.detector.specialize(project, tool, targets) {
            match ports.runner.run(&tool.command, targets) {
                Err(e) => self.fail(format!("{}: {e}", tool.name)),
                Ok(out) if out.timed_out => self.fail(format!(
                    "{} timed out after {}s",
                    tool.name,
                    tool.command.timeout.as_secs()
                )),
                Ok(out) => match ports.detector.parse(project, tool, &out) {
                    Ok(found) => self.findings.extend(found),
                    Err(e) => self.fail(format!("{}: {e}", tool.name)),
                },
            }
        }
    }

    /// Keeps reportable findings (by project-relative path), then renders their paths relative to
    /// `display_root`, or absolute when they lie outside it.
    pub fn filter(&mut self, filter: &ReportingFilter, project_root: &Path, display_root: &Path) {
        let kept = filter.apply(std::mem::take(&mut self.findings));
        self.findings = kept
            .into_iter()
            .map(|mut f| {
                f.location.path = display_path(&project_root.join(&f.location.path), display_root);
                f
            })
            .collect();
    }

    /// Adds a `MissingTool` notice for each missing tool whose kind satisfies `wanted`, and a
    /// notice for each tool note of such a kind.
    pub fn report_missing(&mut self, toolchain: &Toolchain, wanted: impl Fn(CheckKind) -> bool) {
        for note in toolchain.notes().iter().filter(|n| wanted(n.kind)) {
            let notice = Notice::Degraded(note.text.clone());
            if !self.notices.contains(&notice) {
                self.notices.push(notice);
            }
        }
        for tool in toolchain.missing().iter().filter(|t| wanted(t.kind)) {
            let notice = Notice::MissingTool(tool.clone());
            if !self.notices.contains(&notice) {
                self.notices.push(notice);
            }
        }
    }

    /// Moves everything from `other` into `self`.
    pub fn absorb(&mut self, other: RunLog) {
        self.findings.extend(other.findings);
        for n in other.notices {
            if !self.notices.contains(&n) {
                self.notices.push(n);
            }
        }
        for f in other.failures {
            self.fail(f);
        }
    }

    /// Decides the verdict: findings make `Fail`; failures alone make `Degraded`.
    pub fn finish(self, limit: usize) -> Outcome {
        let mut notices = self.notices;
        let verdict = match Report::new(self.findings, limit) {
            Some(report) => {
                notices.extend(self.failures.into_iter().map(Notice::Degraded));
                Verdict::Fail(report)
            }
            None if self.failures.is_empty() => Verdict::Pass,
            None => Verdict::Degraded(self.failures.join("; ")),
        };
        Outcome { verdict, notices }
    }
}
