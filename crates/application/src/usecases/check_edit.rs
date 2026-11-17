//! `CheckEdit`: the edit phase.
//!
//! Locate the project, detect the toolchain, then format and lint the changed file. A formatter
//! that rewrites the file yields a `FileRewritten` notice. Changed lines are read after
//! formatting so they match the final text. Errors anywhere never fail the verdict. Tools the
//! project lacks become non-blocking `MissingTool` notices.

use hookah_domain::path::display_path;
use hookah_domain::project::locate_project;
use hookah_domain::session::{ChangedLines, Phase};
use hookah_domain::toolchain::CheckKind;
use hookah_domain::verdict::{Notice, ReportingFilter};

use super::request::{EditRequest, Outcome, RunLog};
use crate::ports::Ports;

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
        log.report_missing(&toolchain, |k| Phase::Edit.check_kinds().contains(&k));
        for &kind in Phase::Edit.check_kinds() {
            for tool in toolchain.of_kind(kind) {
                let before = self.ports.hasher.content_hash(file).ok();
                log.run_tool(&self.ports, &project, tool, &targets);
                if kind == CheckKind::Format {
                    let after = self.ports.hasher.content_hash(file).ok();
                    let rewritten = matches!((before, after), (Some(b), Some(a)) if a != b);
                    let notice = Notice::FileRewritten(display_path(file, &request.display_root));
                    if rewritten && !log.notices.contains(&notice) {
                        log.notices.push(notice);
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
        log.filter(&filter, &project.root, &request.display_root);
        log.finish(self.report_limit)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::Path;

    use hookah_domain::finding::{Finding, Location, Severity};
    use hookah_domain::session::LineRange;
    use hookah_domain::toolchain::MissingTool;
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
        run_with(w, "/r/src/a.rs", "/r")
    }

    fn run_with(w: &World, file: &str, root: &str) -> Outcome {
        CheckEdit::new(w.ports(), 20).run(&EditRequest::new(file, root))
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
        let out = run_with(&w, "/r/README.md", "/r");
        assert_eq!(out, Outcome::pass());
        let out = run_with(&w, "/elsewhere/a.rs", "/r");
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
    fn runs_typecheck_and_architecture_after_lint_in_order() {
        let w = World::new(
            "/r/Cargo.toml",
            vec![
                tool("arch", CheckKind::Architecture),
                tool("types", CheckKind::Typecheck),
                tool("lint", CheckKind::Lint),
                tool("fmt", CheckKind::Format),
                tool("mut", CheckKind::Mutation),
            ],
        );
        run(&w);
        assert_eq!(
            w.calls(),
            [
                "fmt src/a.rs",
                "lint src/a.rs",
                "types src/a.rs",
                "arch src/a.rs"
            ]
        );
    }

    #[test]
    fn typecheck_findings_show_their_kind() {
        let mut w = World::new("/r/Cargo.toml", vec![tool("types", CheckKind::Typecheck)]);
        w.findings.insert(
            "types".into(),
            vec![Finding::new(
                CheckKind::Typecheck,
                Severity::Error,
                Location::line("src/a.rs", 2),
                "bad type",
            )],
        );
        let Verdict::Fail(report) = run(&w).verdict else {
            panic!("expected fail");
        };
        assert_eq!(report.lines(), ["src/a.rs:2: [typecheck] bad type"]);
    }

    #[test]
    fn paths_are_relative_to_the_display_root_or_absolute() {
        let mut w = world();
        w.findings
            .insert("lint".into(), vec![lint(Severity::Error, "src/a.rs", 3)]);
        let lines = |root| {
            let Verdict::Fail(r) = run_with(&w, "/r/src/a.rs", root).verdict else {
                panic!("expected fail");
            };
            r.lines()
        };
        assert_eq!(lines("/r"), ["src/a.rs:3: msg"]);
        assert_eq!(lines("/"), ["r/src/a.rs:3: msg"]);
        assert_eq!(lines("/r/src"), ["a.rs:3: msg"]);
        assert_eq!(lines("/elsewhere"), ["/r/src/a.rs:3: msg"]);
    }

    #[test]
    fn display_root_is_normalized_and_the_edited_file_is_shown_the_same_way() {
        let mut w = world();
        w.findings
            .insert("lint".into(), vec![lint(Severity::Error, "src/a.rs", 3)]);
        let req = EditRequest::new("/r/src/a.rs", "/r/x/..");
        assert_eq!(req.shown_file(), Path::new("src/a.rs"));
        let Verdict::Fail(r) = CheckEdit::new(w.ports(), 20).run(&req).verdict else {
            panic!("expected fail");
        };
        assert_eq!(r.lines(), ["src/a.rs:3: msg"]);
        let outside = EditRequest::new("/r/src/../src/a.rs", "/elsewhere");
        assert_eq!(outside.shown_file(), Path::new("/r/src/a.rs"));
    }

    #[test]
    fn missing_tools_become_notices_but_only_for_edit_kinds() {
        let mut w = world();
        w.toolchain = w.toolchain.clone().with_missing(vec![
            MissingTool::new("tsc", CheckKind::Typecheck, "npm i -D typescript"),
            MissingTool::new("cargo-mutants", CheckKind::Mutation, "cargo install"),
        ]);
        let out = run(&w);
        assert_eq!(out.verdict, Verdict::Pass);
        assert_eq!(
            out.notices,
            [Notice::MissingTool(MissingTool::new(
                "tsc",
                CheckKind::Typecheck,
                "npm i -D typescript"
            ))]
        );
    }

    #[test]
    fn reports_rewritten_file() {
        let mut w = world();
        w.rewrite = true;
        let out = run(&w);
        assert_eq!(out.verdict, Verdict::Pass);
        assert_eq!(out.notices, [Notice::FileRewritten("src/a.rs".into())]);
    }

    #[test]
    fn rewritten_file_is_shown_relative_to_the_display_root() {
        let mut w = world();
        w.rewrite = true;
        let out = run_with(&w, "/r/src/a.rs", "/r/src");
        assert_eq!(out.notices, [Notice::FileRewritten("a.rs".into())]);
        let out = run_with(&w, "/r/src/a.rs", "/elsewhere");
        assert_eq!(out.notices, [Notice::FileRewritten("/r/src/a.rs".into())]);
    }

    #[test]
    fn only_a_formatter_can_rewrite_the_file() {
        let mut w = World::new("/r/Cargo.toml", vec![tool("lint", CheckKind::Lint)]);
        w.rewrite = true;
        assert_eq!(run(&w), Outcome::pass());
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
