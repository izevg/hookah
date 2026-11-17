//! Use case input and output, and the run log shared by both use cases.
//!
//! `EditRequest` (changed file) and `StopRequest` (repository root) come in, and an `Outcome`
//! (verdict plus notices) goes out. Harness adapters translate to and from these.

use std::path::{Path, PathBuf};

use hookah_domain::finding::Finding;
use hookah_domain::project::Project;
use hookah_domain::toolchain::Tool;
use hookah_domain::verdict::{Notice, Report, ReportingFilter, Verdict};

use crate::ports::Ports;

/// Input of the edit phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditRequest {
    /// Absolute path of the file the agent just changed.
    pub file: PathBuf,
}

/// Input of the stop phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopRequest {
    /// Absolute root (normally the repository root) to look for changed files under.
    pub root: PathBuf,
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
#[derive(Default)]
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

    /// Runs `tool` on `targets`, then parses its output. Every error becomes a failure entry.
    pub fn run_tool(
        &mut self,
        ports: &Ports<'_>,
        project: &Project,
        tool: &Tool,
        targets: &[String],
    ) {
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

    /// Keeps reportable findings and prefixes their paths with `prefix`.
    pub fn filter(&mut self, filter: &ReportingFilter, prefix: &Path) {
        let kept = filter.apply(std::mem::take(&mut self.findings));
        self.findings = kept
            .into_iter()
            .map(|mut f| {
                f.location.path = prefix.join(&f.location.path);
                f
            })
            .collect();
    }

    /// Moves everything from `other` into `self`.
    pub fn absorb(&mut self, other: RunLog) {
        self.findings.extend(other.findings);
        self.notices.extend(other.notices);
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
