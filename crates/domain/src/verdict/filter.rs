//! `ReportingFilter`: which findings are worth the agent's attention.
//!
//! Without a baseline (none taken, still running, or failed) the rules are:
//! - `Test` and `Mutation` findings are always reported: their scope is already narrow.
//! - `Error` findings are always reported, in any file (a change can break callers elsewhere).
//! - `Warning` findings are reported only in a changed file and on a changed line. A warning
//!   without a line in a changed file counts as on a changed line.
//!
//! With a [`Baseline`] these rules give way to one: `Test` and `Mutation` findings are reported,
//! and every static finding is reported exactly when it is new against the baseline, any severity,
//! in any file ([`Baseline::introduced`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::finding::baseline::Baseline;
use crate::finding::{Finding, Severity};
use crate::session::ChangedLines;
use crate::toolchain::CheckKind;

/// Decides which findings to report, given the changed files of one project.
#[derive(Debug, Clone, Default)]
pub struct ReportingFilter {
    changed: BTreeMap<PathBuf, ChangedLines>,
    baseline: Option<Baseline>,
}

impl ReportingFilter {
    /// Creates a filter with no changed files: only errors pass.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reports only what is new against `baseline`, replacing the changed-lines rules.
    #[must_use]
    pub fn with_baseline(mut self, baseline: Baseline) -> Self {
        self.baseline = Some(baseline);
        self
    }

    /// Whether a baseline decides what is reported.
    pub fn has_baseline(&self) -> bool {
        self.baseline.is_some()
    }

    /// Records a changed file (path relative to the project root) and its changed lines.
    pub fn track(&mut self, path: impl Into<PathBuf>, lines: ChangedLines) {
        self.changed.insert(path.into(), lines);
    }

    /// The changed lines recorded for `path`, if any.
    pub fn lines_of(&self, path: &Path) -> Option<&ChangedLines> {
        self.changed.get(path)
    }

    /// Whether `finding` passes the changed-lines rules alone; ignores any baseline (a baseline
    /// compares counts, so it decides over the whole list in [`ReportingFilter::apply`]).
    pub fn keeps(&self, finding: &Finding) -> bool {
        if matches!(finding.kind, CheckKind::Test | CheckKind::Mutation) {
            return true;
        }
        match finding.severity {
            Severity::Error => true,
            Severity::Warning => match self.changed.get(&finding.location.path) {
                None => false,
                Some(lines) => finding.location.line.is_none_or(|l| lines.contains(l)),
            },
        }
    }

    /// Keeps only the reportable findings, preserving order.
    pub fn apply(&self, findings: Vec<Finding>) -> Vec<Finding> {
        match &self.baseline {
            Some(baseline) => baseline.introduced(findings),
            None => findings.into_iter().filter(|f| self.keeps(f)).collect(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::finding::Location;
    use crate::session::LineRange;

    fn finding(sev: Severity, loc: Location) -> Finding {
        Finding::new(CheckKind::Lint, sev, loc, "m")
    }

    fn filter() -> ReportingFilter {
        let mut f = ReportingFilter::new();
        f.track(
            "src/a.rs",
            ChangedLines::Ranges(vec![LineRange::new(10, 12).unwrap()]),
        );
        f.track("src/new.rs", ChangedLines::All);
        f
    }

    #[test]
    fn lines_of_returns_tracked_lines() {
        let f = filter();
        assert_eq!(
            f.lines_of(Path::new("src/new.rs")),
            Some(&ChangedLines::All)
        );
        assert!(f.lines_of(Path::new("src/a.rs")).is_some());
        assert!(f.lines_of(Path::new("nope.rs")).is_none());
    }

    #[test]
    fn errors_always_pass_even_in_unchanged_files() {
        let f = filter();
        assert!(f.keeps(&finding(Severity::Error, Location::line("src/other.rs", 1))));
        assert!(f.keeps(&finding(Severity::Error, Location::line("src/a.rs", 1))));
        assert!(f.keeps(&finding(Severity::Error, Location::file("src/other.rs"))));
        assert!(ReportingFilter::new().keeps(&finding(Severity::Error, Location::file("x"))));
    }

    #[test]
    fn warnings_need_changed_file_and_line() {
        let f = filter();
        assert!(f.keeps(&finding(Severity::Warning, Location::line("src/a.rs", 10))));
        assert!(f.keeps(&finding(Severity::Warning, Location::line("src/a.rs", 12))));
        assert!(!f.keeps(&finding(Severity::Warning, Location::line("src/a.rs", 9))));
        assert!(!f.keeps(&finding(Severity::Warning, Location::line("src/a.rs", 13))));
        assert!(!f.keeps(&finding(
            Severity::Warning,
            Location::line("src/other.rs", 10)
        )));
    }

    #[test]
    fn warning_without_line_in_changed_file_passes() {
        let f = filter();
        assert!(f.keeps(&finding(Severity::Warning, Location::file("src/a.rs"))));
        assert!(!f.keeps(&finding(Severity::Warning, Location::file("src/other.rs"))));
    }

    #[test]
    fn test_and_mutation_findings_are_never_filtered() {
        let f = filter();
        for kind in [CheckKind::Test, CheckKind::Mutation] {
            let w = Finding::new(
                kind,
                Severity::Warning,
                Location::line("src/other.rs", 1),
                "m",
            );
            assert!(f.keeps(&w));
        }
    }

    #[test]
    fn new_file_reports_every_line() {
        let f = filter();
        assert!(f.keeps(&finding(
            Severity::Warning,
            Location::line("src/new.rs", 999)
        )));
    }

    #[test]
    fn a_baseline_replaces_the_changed_lines_rules() {
        let old_error = finding(Severity::Error, Location::line("src/other.rs", 1));
        let old_warning = finding(Severity::Warning, Location::line("src/a.rs", 99));
        let mut new_warning = finding(Severity::Warning, Location::line("src/other.rs", 7));
        new_warning.message = "new".into();
        let test = Finding::new(
            CheckKind::Test,
            Severity::Error,
            Location::line("src/other.rs", 1),
            "m",
        );
        let base = Baseline::from_findings([&old_error, &old_warning, &test]);
        let f = filter().with_baseline(base);
        assert!(f.has_baseline());
        assert!(!filter().has_baseline());
        let out = f.apply(vec![
            old_error,
            old_warning,
            new_warning.clone(),
            test.clone(),
        ]);
        assert_eq!(out, vec![new_warning, test]);
    }

    #[test]
    fn empty_ranges_drop_warnings_keep_errors() {
        let mut f = ReportingFilter::new();
        f.track("a.rs", ChangedLines::Ranges(vec![]));
        let out = f.apply(vec![
            finding(Severity::Warning, Location::line("a.rs", 1)),
            finding(Severity::Error, Location::line("a.rs", 1)),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].severity, Severity::Error);
    }
}
