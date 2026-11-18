//! `Report`: the compact, capped rendering policy for findings.
//!
//! One `path:line: message` per finding, in a total order (severity, path, line, column, kind,
//! rule, message), cut off at a limit, followed by a `+N more` line. Findings with the same path,
//! line, column, and rule id are one problem (the earliest check kind wins); findings without a
//! rule id are one problem when path, line, column, severity, and message all match (again the
//! earliest kind wins). Agent-agnostic; harness adapters wrap it in their own protocol.

use std::collections::HashSet;
use std::fmt;

use crate::finding::{Finding, Severity};
use crate::toolchain::CheckKind;

/// Default cap on rendered findings.
pub const DEFAULT_LIMIT: usize = 20;

/// A non-empty, ordered, de-duplicated list of findings with a rendering cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    findings: Vec<Finding>,
    limit: usize,
}

impl Report {
    /// Orders and de-duplicates `findings`. Returns `None` if there are none.
    pub fn new(mut findings: Vec<Finding>, limit: usize) -> Option<Self> {
        findings.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
        let mut seen_ruled = HashSet::new();
        let mut seen_ruleless = HashSet::new();
        findings.retain(|f| match &f.rule {
            Some(rule) => seen_ruled.insert((
                f.location.path.clone(),
                f.location.line,
                f.location.column,
                rule.clone(),
            )),
            None => seen_ruleless.insert((
                f.location.path.clone(),
                f.location.line,
                f.location.column,
                f.severity,
                f.message.clone(),
            )),
        });
        (!findings.is_empty()).then_some(Self { findings, limit })
    }

    /// All findings, in report order (not capped).
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// Number of findings beyond the cap.
    pub fn hidden(&self) -> usize {
        self.findings.len().saturating_sub(self.limit)
    }

    /// The rendered lines: at most `limit` findings plus an optional `+N more` line.
    pub fn lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self.findings.iter().take(self.limit).map(render).collect();
        if self.hidden() > 0 {
            lines.push(format!("+{} more", self.hidden()));
        }
        lines
    }
}

/// Total order: severity, path, line, column, kind (phase order), rule, message.
fn sort_key(f: &Finding) -> impl Ord + '_ {
    (
        f.severity,
        &f.location.path,
        f.location.line,
        f.location.column,
        f.kind,
        &f.rule,
        &f.message,
    )
}

fn render(f: &Finding) -> String {
    let kind = match f.kind {
        CheckKind::Typecheck | CheckKind::Architecture | CheckKind::Mutation => {
            format!("[{}] ", f.kind.name())
        }
        _ => String::new(),
    };
    let tag = if f.severity == Severity::Warning {
        "warning: "
    } else {
        ""
    };
    match &f.rule {
        Some(rule) => format!("{}: {kind}{tag}{} [{rule}]", f.location, f.message),
        None => format!("{}: {kind}{tag}{}", f.location, f.message),
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.lines().join("\n"))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::finding::Location;

    fn f(sev: Severity, path: &str, line: u32, msg: &str) -> Finding {
        Finding::new(CheckKind::Lint, sev, Location::line(path, line), msg)
    }

    #[test]
    fn exposes_findings_and_displays_lines() {
        let r = Report::new(vec![f(Severity::Error, "a.rs", 1, "m")], 5).unwrap();
        assert_eq!(r.findings().len(), 1);
        assert_eq!(r.to_string(), "a.rs:1: m");
        let r = Report::new(
            vec![
                f(Severity::Error, "a.rs", 1, "m"),
                f(Severity::Error, "a.rs", 2, "m"),
            ],
            1,
        )
        .unwrap();
        assert_eq!(r.to_string(), "a.rs:1: m\n+1 more");
    }

    #[test]
    fn empty_is_none() {
        assert!(Report::new(vec![], 5).is_none());
    }

    #[test]
    fn orders_errors_first_then_path_and_line() {
        let r = Report::new(
            vec![
                f(Severity::Warning, "a.rs", 1, "w"),
                f(Severity::Error, "b.rs", 2, "e2"),
                f(Severity::Error, "b.rs", 1, "e1"),
                f(Severity::Error, "a.rs", 9, "e0"),
            ],
            20,
        )
        .unwrap();
        assert_eq!(
            r.lines(),
            [
                "a.rs:9: e0",
                "b.rs:1: e1",
                "b.rs:2: e2",
                "a.rs:1: warning: w"
            ]
        );
    }

    #[test]
    fn caps_with_more_line() {
        let all = (1..=5)
            .map(|i| f(Severity::Error, "a.rs", i, "m"))
            .collect();
        let r = Report::new(all, 3).unwrap();
        assert_eq!(r.lines().len(), 4);
        assert_eq!(r.lines()[3], "+2 more");
        assert_eq!(r.hidden(), 2);
    }

    #[test]
    fn shows_kind_for_typecheck_architecture_and_mutation_only() {
        let mk = |kind| Finding::new(kind, Severity::Error, Location::line("a.rs", 1), "m");
        let r = Report::new(vec![mk(CheckKind::Typecheck)], 5).unwrap();
        assert_eq!(r.lines(), ["a.rs:1: [typecheck] m"]);
        let r = Report::new(vec![mk(CheckKind::Mutation)], 5).unwrap();
        assert_eq!(r.lines(), ["a.rs:1: [mutation] m"]);
        let r = Report::new(vec![mk(CheckKind::Lint)], 5).unwrap();
        assert_eq!(r.lines(), ["a.rs:1: m"]);
    }

    #[test]
    fn dedupes_and_renders_rule_and_column() {
        let a = Finding::new(
            CheckKind::Lint,
            Severity::Error,
            Location::at("a.rs", 1, 2),
            "m",
        )
        .with_rule("r1");
        let r = Report::new(vec![a.clone(), a], 20).unwrap();
        assert_eq!(r.lines(), ["a.rs:1:2: m [r1]"]);
    }

    fn ruled(kind: CheckKind, col: u32, rule: &str, msg: &str) -> Finding {
        Finding::new(kind, Severity::Error, Location::at("a.rs", 8, col), msg).with_rule(rule)
    }

    #[test]
    fn same_place_and_rule_is_one_problem_keeping_the_earliest_kind() {
        let test = ruled(CheckKind::Test, 47, "E0599", "no method on Money");
        let lint = ruled(
            CheckKind::Lint,
            47,
            "E0599",
            "no method on ledger_core::Money",
        );
        let r = Report::new(vec![test, lint], 20).unwrap();
        assert_eq!(r.findings().len(), 1);
        assert_eq!(r.findings()[0].kind, CheckKind::Lint);
        assert_eq!(
            r.lines(),
            ["a.rs:8:47: no method on ledger_core::Money [E0599]"]
        );
    }

    #[test]
    fn dedup_needs_same_column_rule_and_line_and_path() {
        let base = ruled(CheckKind::Lint, 47, "E0599", "m");
        let other_col = ruled(CheckKind::Test, 48, "E0599", "m");
        let other_rule = ruled(CheckKind::Test, 47, "E0600", "m");
        let mut other_line = ruled(CheckKind::Test, 47, "E0599", "m");
        other_line.location.line = Some(9);
        let mut other_path = ruled(CheckKind::Test, 47, "E0599", "m");
        other_path.location.path = "b.rs".into();
        let r = Report::new(
            vec![base, other_col, other_rule, other_line, other_path],
            20,
        )
        .unwrap();
        assert_eq!(r.findings().len(), 5);
    }

    #[test]
    fn dedup_applies_even_when_other_findings_sit_between() {
        let a = ruled(CheckKind::Lint, 47, "E1", "x");
        let b = ruled(CheckKind::Lint, 47, "E2", "y");
        let c = ruled(CheckKind::Test, 47, "E1", "z");
        let r = Report::new(vec![c, b, a], 20).unwrap();
        assert_eq!(r.lines(), ["a.rs:8:47: x [E1]", "a.rs:8:47: y [E2]"]);
    }

    #[test]
    fn findings_without_rule_with_different_messages_stay_apart() {
        let a = Finding::new(
            CheckKind::Lint,
            Severity::Error,
            Location::at("a.rs", 1, 1),
            "m1",
        );
        let b = Finding::new(
            CheckKind::Test,
            Severity::Error,
            Location::at("a.rs", 1, 1),
            "m2",
        );
        let r = Report::new(vec![a.clone(), b, a], 20).unwrap();
        assert_eq!(r.findings().len(), 2);
    }

    #[test]
    fn ruleless_duplicate_from_lint_and_test_is_one_problem() {
        let mk = |kind| {
            Finding::new(
                kind,
                Severity::Error,
                Location::at("crates/adapters/src/ecosystems/mod.rs", 10, 1),
                "encountered diff marker",
            )
        };
        let r = Report::new(vec![mk(CheckKind::Test), mk(CheckKind::Lint)], 20).unwrap();
        assert_eq!(r.findings().len(), 1);
        assert_eq!(r.findings()[0].kind, CheckKind::Lint);
        assert_eq!(
            r.lines(),
            ["crates/adapters/src/ecosystems/mod.rs:10:1: encountered diff marker"]
        );
    }

    #[test]
    fn ruleless_findings_differing_in_severity_location_or_message_stay_apart() {
        let base = Finding::new(
            CheckKind::Lint,
            Severity::Error,
            Location::at("a.rs", 1, 1),
            "m",
        );
        let mut warn = base.clone();
        warn.kind = CheckKind::Test;
        warn.severity = Severity::Warning;
        let mut col = base.clone();
        col.kind = CheckKind::Test;
        col.location.column = Some(2);
        let mut line = base.clone();
        line.kind = CheckKind::Test;
        line.location.line = Some(2);
        let mut path = base.clone();
        path.kind = CheckKind::Test;
        path.location.path = "b.rs".into();
        let mut msg = base.clone();
        msg.kind = CheckKind::Test;
        msg.message = "other".into();
        let r = Report::new(vec![base, warn, col, line, path, msg], 20).unwrap();
        assert_eq!(r.findings().len(), 6);
    }

    #[test]
    fn order_is_total_across_every_key() {
        let mk = |sev, col, kind, rule: Option<&str>, msg: &str| {
            let f = Finding::new(kind, sev, Location::at("a.rs", 1, col), msg);
            match rule {
                Some(r) => f.with_rule(r),
                None => f,
            }
        };
        let want = vec![
            mk(Severity::Error, 1, CheckKind::Lint, None, "a"),
            mk(Severity::Error, 1, CheckKind::Lint, None, "b"),
            mk(Severity::Error, 1, CheckKind::Lint, Some("r1"), "a"),
            mk(Severity::Error, 1, CheckKind::Lint, Some("r2"), "a"),
            mk(Severity::Error, 1, CheckKind::Test, None, "0"),
            mk(Severity::Error, 2, CheckKind::Lint, None, "a"),
            mk(Severity::Warning, 1, CheckKind::Lint, None, "a"),
        ];
        let mut shuffled = want.clone();
        shuffled.reverse();
        let r = Report::new(shuffled.clone(), 20).unwrap();
        assert_eq!(r.findings(), want.as_slice());
        shuffled.rotate_left(3);
        let r = Report::new(shuffled, 20).unwrap();
        assert_eq!(r.findings(), want.as_slice());
    }
}
