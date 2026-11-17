//! `CheckEdit`: the edit phase.
//!
//! Locate the project, detect the toolchain, then format and lint the changed file. A formatter
//! that rewrites the file yields a `FileRewritten` notice. Changed lines are read after
//! formatting so they match the final text. Errors anywhere never fail the verdict.

use hookah_domain::project::locate_project;
use hookah_domain::session::{ChangedLines, Phase};
use hookah_domain::toolchain::CheckKind;
use hookah_domain::verdict::{Notice, ReportingFilter};

use super::request::{EditRequest, Outcome, RunLog};
use crate::ports::Ports;
use std::path::Path;

/// Runs format and lint on one changed file.
pub struct CheckEdit<'a> {
    ports: Ports<'a>,
    report_limit: usize,
}

impl<'a> CheckEdit<'a> {
    /// Creates the use case; `report_limit` caps the findings rendered in the report.
    pub fn new(ports: Ports<'a>, report_limit: usize) -> Self {
        Self {
            ports,
            report_limit,
        }
    }

    /// Checks `request.file`. Never panics and never fails on a port error.
    pub fn run(&self, request: &EditRequest) -> Outcome {
        let file = request.file.as_path();
        let manifests = match self.ports.locator.manifests_above(file) {
            Ok(m) => m,
            Err(e) => return Outcome::degraded(format!("locating project: {e}")),
        };
        let Some(project) = locate_project(file, &manifests) else {
            return Outcome::pass();
        };
        if !project.ecosystem.can_check(file) {
            return Outcome::pass();
        }
        let rel = match project.relative(file) {
            Ok(r) => r,
            Err(e) => return Outcome::degraded(e.to_string()),
        };
        let toolchain = match self.ports.detector.detect(&project) {
            Ok(t) => t,
            Err(e) => return Outcome::degraded(format!("detecting toolchain: {e}")),
        };
        let targets = [rel.to_string_lossy().into_owned()];

        let mut log = RunLog::default();
        for &kind in Phase::Edit.check_kinds() {
            for tool in toolchain.of_kind(kind) {
                let before = self.ports.hasher.content_hash(file).ok();
                log.run_tool(&self.ports, &project, tool, &targets);
                if kind == CheckKind::Format {
                    let after = self.ports.hasher.content_hash(file).ok();
                    let rewritten = matches!((before, after), (Some(b), Some(a)) if a != b);
                    if rewritten
                        && !log
                            .notices
                            .contains(&Notice::FileRewritten(request.file.clone()))
                    {
                        log.notices
                            .push(Notice::FileRewritten(request.file.clone()));
                    }
                }
            }
        }

        let mut filter = ReportingFilter::new();
        match self.ports.changes.changed_lines(&project.root, file) {
            Ok(lines) => filter.track(rel, lines),
            Err(e) => {
                log.fail(format!("reading changes: {e}"));
                filter.track(rel, ChangedLines::Ranges(vec![]));
            }
        }
        log.filter(&filter, Path::new(""));
        log.finish(self.report_limit)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use hookah_domain::finding::{Finding, Location, Severity};
    use hookah_domain::session::LineRange;
    use hookah_domain::verdict::Verdict;

    use super::*;
    use crate::ports::fakes::{World, tool};

    fn world() -> World {
        World::new(
            "/r/Cargo.toml",
            vec![
                tool("lint", CheckKind::Lint),
                tool("fmt", CheckKind::Format),
                tool("test", CheckKind::Test),
            ],
        )
    }

    fn run(w: &World) -> Outcome {
        let req = EditRequest {
            file: "/r/src/a.rs".into(),
        };
        CheckEdit::new(w.ports(), 20).run(&req)
    }

    fn lint(sev: Severity, path: &str, line: u32) -> Finding {
        Finding::new(CheckKind::Lint, sev, Location::line(path, line), "msg")
    }

    #[test]
    fn passes_and_runs_format_then_lint_only_on_relative_target() {
        let w = world();
        let out = run(&w);
        assert_eq!(out, Outcome::pass());
        assert_eq!(w.calls(), ["fmt src/a.rs", "lint src/a.rs"]);
    }

    #[test]
    fn skips_unsupported_files_and_missing_projects() {
        let w = world();
        let out = CheckEdit::new(w.ports(), 20).run(&EditRequest {
            file: "/r/README.md".into(),
        });
        assert_eq!(out, Outcome::pass());
        let out = CheckEdit::new(w.ports(), 20).run(&EditRequest {
            file: "/elsewhere/a.rs".into(),
        });
        assert_eq!(out, Outcome::pass());
        assert!(w.calls().is_empty());
    }

    #[test]
    fn error_findings_in_other_files_are_reported_warnings_outside_changes_are_not() {
        let mut w = world();
        w.lines = ChangedLines::Ranges(vec![LineRange::new(5, 6).unwrap()]);
        w.findings.insert(
            "lint".into(),
            vec![
                lint(Severity::Error, "src/other.rs", 1),
                lint(Severity::Warning, "src/a.rs", 5),
                lint(Severity::Warning, "src/a.rs", 50),
            ],
        );
        let Verdict::Fail(report) = run(&w).verdict else {
            panic!("expected fail");
        };
        assert_eq!(
            report.lines(),
            ["src/other.rs:1: msg", "src/a.rs:5: warning: msg"]
        );
    }

    #[test]
    fn reports_rewritten_file() {
        let mut w = world();
        w.rewrite = true;
        let out = run(&w);
        assert_eq!(out.verdict, Verdict::Pass);
        assert_eq!(out.notices, [Notice::FileRewritten("/r/src/a.rs".into())]);
    }

    #[test]
    fn missing_tool_degrades_but_never_fails() {
        let mut w = world();
        w.run_fails = vec!["lint".into()];
        let out = run(&w);
        assert!(matches!(out.verdict, Verdict::Degraded(ref r) if r.contains("lint")));
    }

    #[test]
    fn timeout_degrades() {
        let mut w = world();
        w.timeouts = vec!["lint".into()];
        let out = run(&w);
        assert!(matches!(out.verdict, Verdict::Degraded(ref r) if r.contains("timed out")));
    }

    #[test]
    fn findings_survive_partial_failure_with_a_notice() {
        let mut w = world();
        w.run_fails = vec!["fmt".into()];
        w.findings
            .insert("lint".into(), vec![lint(Severity::Error, "src/a.rs", 1)]);
        let out = run(&w);
        assert!(matches!(out.verdict, Verdict::Fail(_)));
        assert!(matches!(out.notices.as_slice(), [Notice::Degraded(_)]));
    }

    #[test]
    fn port_errors_degrade() {
        let mut w = world();
        w.locate_fails = true;
        assert!(matches!(run(&w).verdict, Verdict::Degraded(_)));
        let mut w = world();
        w.detect_fails = true;
        assert!(matches!(run(&w).verdict, Verdict::Degraded(_)));
    }

    #[test]
    fn unreadable_changes_drop_warnings_keep_errors() {
        let mut w = world();
        w.lines_fail = true;
        w.findings.insert(
            "lint".into(),
            vec![
                lint(Severity::Warning, "src/a.rs", 1),
                lint(Severity::Error, "src/a.rs", 2),
            ],
        );
        let out = run(&w);
        let Verdict::Fail(report) = out.verdict else {
            panic!("expected fail");
        };
        assert_eq!(report.findings().len(), 1);
        assert_eq!(out.notices.len(), 1);
    }
}
