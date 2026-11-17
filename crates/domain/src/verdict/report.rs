//! `Report`: the compact, capped rendering policy for findings.
//!
//! One `path:line: message` per finding, errors first, then by path and line, cut off at a limit,
//! followed by a `+N more` line. Agent-agnostic; harness adapters wrap it in their own protocol.

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
        findings.sort_by(|a, b| {
            (
                a.severity,
                &a.location.path,
                a.location.line,
                a.location.column,
            )
                .cmp(&(
                    b.severity,
                    &b.location.path,
                    b.location.line,
                    b.location.column,
                ))
        });
        findings.dedup();
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
}
