//! `Status`: the latest outcome of a check run for one project, kept so that other tools (an
//! editor, an orchestrator) can show it without running the checks again.
//!
//! A status is derived from a [`Verdict`] and the [`Phase`] that produced it. Only the latest run
//! counts, so a stop-phase status replaces the edit-phase statuses before it.

use crate::session::Phase;
use crate::verdict::Verdict;

/// At most this many report lines are kept in a status.
pub const STATUS_LINES: usize = 5;

/// How a run ended, ordered from best to worst so the worst of several is their maximum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Standing {
    /// No findings.
    Pass,
    /// `hookah` could not finish the checks (fail-open).
    Degraded,
    /// Findings the agent must fix.
    Fail,
}

impl Standing {
    /// The name used in stored records and JSON output.
    pub fn name(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Degraded => "degraded",
            Self::Fail => "fail",
        }
    }

    /// The inverse of [`Standing::name`].
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Pass, Self::Degraded, Self::Fail]
            .into_iter()
            .find(|s| s.name() == name)
    }
}

/// The worst standing of several; `None` when there are none.
pub fn overall(standings: impl IntoIterator<Item = Standing>) -> Option<Standing> {
    standings.into_iter().max()
}

/// The outcome of the latest run in one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// The phase that ran.
    pub phase: Phase,
    /// How it ended.
    pub standing: Standing,
    /// Number of findings (all of them, not only the rendered ones).
    pub problems: usize,
    /// Why the run was degraded.
    pub detail: Option<String>,
    /// The first rendered report lines (`path:line: message`), at most [`STATUS_LINES`].
    pub lines: Vec<String>,
}

impl Status {
    /// The status of a run of `phase` that ended with `verdict`.
    pub fn of(phase: Phase, verdict: &Verdict) -> Self {
        let (standing, problems, detail, lines) = match verdict {
            Verdict::Pass => (Standing::Pass, 0, None, vec![]),
            Verdict::Degraded(reason) => (Standing::Degraded, 0, Some(reason.clone()), vec![]),
            Verdict::Fail(report) => (
                Standing::Fail,
                report.findings().len(),
                None,
                report.lines().into_iter().take(STATUS_LINES).collect(),
            ),
        };
        Self {
            phase,
            standing,
            problems,
            detail,
            lines,
        }
    }
}

/// A status and the time it was recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedStatus {
    /// Milliseconds since the Unix epoch.
    pub at_ms: u64,
    /// The status.
    pub status: Status,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::finding::{Finding, Location, Severity};
    use crate::toolchain::CheckKind;
    use crate::verdict::Report;

    fn finding(line: u32) -> Finding {
        Finding::new(
            CheckKind::Lint,
            Severity::Error,
            Location::line("a.rs", line),
            "msg",
        )
    }

    #[test]
    fn a_pass_has_no_problems_and_no_lines() {
        let s = Status::of(Phase::Edit, &Verdict::Pass);
        assert_eq!(s.standing, Standing::Pass);
        assert_eq!((s.problems, s.detail, s.lines.len()), (0, None, 0));
        assert_eq!(s.phase, Phase::Edit);
    }

    #[test]
    fn a_failure_counts_every_finding_but_keeps_few_lines() {
        let findings = (1..=9).map(finding).collect();
        let report = Report::new(findings, 20).unwrap();
        let s = Status::of(Phase::Stop, &Verdict::Fail(report));
        assert_eq!(s.standing, Standing::Fail);
        assert_eq!(s.problems, 9);
        assert_eq!(s.lines.len(), STATUS_LINES);
        assert_eq!(s.lines[0], "a.rs:1: msg");
    }

    #[test]
    fn a_degraded_run_keeps_the_reason() {
        let s = Status::of(Phase::Edit, &Verdict::Degraded("tool died".into()));
        assert_eq!(s.standing, Standing::Degraded);
        assert_eq!(s.detail.as_deref(), Some("tool died"));
        assert_eq!(s.problems, 0);
    }

    #[test]
    fn the_worst_standing_wins() {
        use Standing::*;
        assert_eq!(overall([]), None);
        assert_eq!(overall([Pass, Pass]), Some(Pass));
        assert_eq!(overall([Pass, Degraded]), Some(Degraded));
        assert_eq!(overall([Degraded, Fail, Pass]), Some(Fail));
    }

    #[test]
    fn standing_names_round_trip() {
        for s in [Standing::Pass, Standing::Degraded, Standing::Fail] {
            assert_eq!(Standing::from_name(s.name()), Some(s));
        }
        assert_eq!(Standing::from_name("nope"), None);
    }
}
