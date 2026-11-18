//! `CheckEdit`: the edit phase.
//!
//! Locate the project, detect the toolchain, then format and lint the changed file. A formatter
//! that rewrites the file yields a `FileRewritten` notice. Changed lines are read after
//! formatting so they match the final text. Errors anywhere never fail the verdict. Tools the
//! project lacks become non-blocking `MissingTool` notices.
//!
//! With a session baseline, only static findings that are new against it are reported (any
//! severity, anywhere); otherwise the changed-lines filter decides. A session without a baseline
//! gets its baseline job started by its first edit, which is itself not baseline-filtered.

use hookah_domain::path::display_path;
use hookah_domain::project::locate_project;
use hookah_domain::session::{ChangedLines, Phase};
use hookah_domain::toolchain::CheckKind;
use hookah_domain::verdict::{Notice, ReportingFilter};

use super::baseline::lookup;
use super::request::{EditRequest, Outcome, RunLog};
use super::status::record;
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
        // Looked up before the tools run: a baseline job started by this very edit must not
        // finish in time to be used for it.
        let baseline = lookup(&self.ports, &project, request.session.as_deref(), true);

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
        if let Some(baseline) = baseline {
            filter = filter.with_baseline(baseline);
        }
        match self.ports.changes.changed_lines(&project.root, file) {
            Ok(lines) => filter.track(rel, lines),
            Err(e) => {
                log.fail(format!("reading changes: {e}"));
                filter.track(rel, ChangedLines::Ranges(vec![]));
            }
        }
        log.filter(&filter, &project.root, &request.display_root);
        let outcome = log.finish(self.report_limit);
        record(&self.ports, &project.root, Phase::Edit, &outcome.verdict);
        outcome
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::Path;

    use hookah_domain::finding::{Finding, Location, Severity};
    use hookah_domain::session::LineRange;
    use hookah_domain::toolchain::{MissingTool, ToolNote};
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
    fn unused_code_never_runs_in_the_edit_phase() {
        let w = World::new("/r/Cargo.toml", vec![tool("unused", CheckKind::Unused)]);
        assert_eq!(run(&w), Outcome::pass());
        assert!(w.calls().is_empty());
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
    fn tool_notes_become_notices_but_only_for_edit_kinds() {
        let mut w = world();
        w.toolchain = w.toolchain.clone().with_notes(vec![
            ToolNote::new(CheckKind::Typecheck, "dependents left out"),
            ToolNote::new(CheckKind::Mutation, "not for the edit phase"),
        ]);
        let out = run(&w);
        assert_eq!(out.verdict, Verdict::Pass);
        assert_eq!(
            out.notices,
            [Notice::Degraded("dependents left out".into())]
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

    mod baseline {
        use hookah_domain::finding::baseline::Baseline;

        use super::*;
        use crate::ports::BaselineState;

        fn run_in_session(w: &World, session: Option<&str>) -> Outcome {
            let request =
                EditRequest::new("/r/src/a.rs", "/r").with_session(session.map(Into::into));
            CheckEdit::new(w.ports(), 20).run(&request)
        }

        fn lines(out: Outcome) -> Vec<String> {
            match out.verdict {
                Verdict::Fail(r) => r.lines(),
                Verdict::Pass => vec![],
                Verdict::Degraded(d) => panic!("degraded: {d}"),
            }
        }

        fn ready(w: &World, old: &[Finding]) {
            w.set_baseline("s", BaselineState::Ready(Baseline::from_findings(old)));
        }

        #[test]
        fn a_pre_existing_error_elsewhere_is_hidden() {
            let mut w = world();
            let old = lint(Severity::Error, "src/other.rs", 1);
            w.findings.insert("lint".into(), vec![old.clone()]);
            assert_eq!(
                lines(run_in_session(&w, Some("s"))),
                ["src/other.rs:1: msg"]
            );
            ready(&w, &[old]);
            assert!(lines(run_in_session(&w, Some("s"))).is_empty());
        }

        #[test]
        fn a_new_error_is_reported_and_an_old_one_after_a_line_shift_is_not() {
            let mut w = world();
            let old = lint(Severity::Error, "src/other.rs", 1);
            ready(&w, &[old]);
            let mut fresh = lint(Severity::Error, "src/a.rs", 4);
            fresh.message = "brand new".into();
            w.findings.insert(
                "lint".into(),
                vec![lint(Severity::Error, "src/other.rs", 25), fresh],
            );
            assert_eq!(
                lines(run_in_session(&w, Some("s"))),
                ["src/a.rs:4: brand new"]
            );
        }

        #[test]
        fn warnings_anywhere_are_reported_when_new_and_hidden_when_old() {
            let mut w = world();
            w.lines = ChangedLines::Ranges(vec![LineRange::new(5, 6).unwrap()]);
            let old = lint(Severity::Warning, "src/a.rs", 50);
            ready(&w, std::slice::from_ref(&old));
            let mut fresh = lint(Severity::Warning, "src/other.rs", 2);
            fresh.message = "fresh".into();
            w.findings.insert("lint".into(), vec![old, fresh]);
            assert_eq!(
                lines(run_in_session(&w, Some("s"))),
                ["src/other.rs:2: warning: fresh"]
            );
        }

        #[test]
        fn one_more_occurrence_than_the_baseline_is_one_new_finding() {
            let mut w = world();
            ready(&w, &[lint(Severity::Error, "src/a.rs", 1)]);
            w.findings.insert(
                "lint".into(),
                vec![
                    lint(Severity::Error, "src/a.rs", 1),
                    lint(Severity::Error, "src/a.rs", 8),
                ],
            );
            assert_eq!(lines(run_in_session(&w, Some("s"))), ["src/a.rs:8: msg"]);
        }

        #[test]
        fn an_absent_baseline_starts_the_job_once_and_the_edit_uses_the_fallback() {
            let mut w = world();
            w.findings.insert(
                "lint".into(),
                vec![lint(Severity::Error, "src/other.rs", 1)],
            );
            assert_eq!(
                lines(run_in_session(&w, Some("s"))),
                ["src/other.rs:1: msg"]
            );
            assert_eq!(*w.spawned.borrow(), [("/r".into(), "s".to_string())]);
            assert_eq!(w.baseline_state("s"), BaselineState::InProgress);
            assert_eq!(
                lines(run_in_session(&w, Some("s"))),
                ["src/other.rs:1: msg"]
            );
            assert_eq!(w.spawned.borrow().len(), 1);
        }

        #[test]
        fn no_session_id_means_no_baseline_and_no_job() {
            let mut w = world();
            w.findings.insert(
                "lint".into(),
                vec![lint(Severity::Error, "src/other.rs", 1)],
            );
            assert_eq!(lines(run_in_session(&w, None)), ["src/other.rs:1: msg"]);
            assert_eq!(lines(run_in_session(&w, Some(""))), ["src/other.rs:1: msg"]);
            assert!(w.spawned.borrow().is_empty());
        }

        #[test]
        fn a_failed_or_running_baseline_falls_back_without_a_new_job() {
            let mut w = world();
            w.lines = ChangedLines::Ranges(vec![]);
            w.findings.insert(
                "lint".into(),
                vec![
                    lint(Severity::Error, "src/other.rs", 1),
                    lint(Severity::Warning, "src/a.rs", 1),
                ],
            );
            for state in [BaselineState::InProgress, BaselineState::Failed("x".into())] {
                w.set_baseline("s", state);
                assert_eq!(
                    lines(run_in_session(&w, Some("s"))),
                    ["src/other.rs:1: msg"]
                );
            }
            assert!(w.spawned.borrow().is_empty());
        }

        #[test]
        fn a_job_that_cannot_start_never_blocks_the_edit() {
            let mut w = world();
            w.spawn_fails = true;
            w.findings
                .insert("lint".into(), vec![lint(Severity::Error, "src/a.rs", 1)]);
            assert_eq!(lines(run_in_session(&w, Some("s"))), ["src/a.rs:1: msg"]);
            assert!(matches!(w.baseline_state("s"), BaselineState::Failed(_)));
        }
    }

    #[test]
    fn records_the_edit_status() {
        use hookah_domain::status::Standing;
        let mut w = world();
        w.findings
            .insert("lint".into(), vec![lint(Severity::Error, "src/a.rs", 1)]);
        run(&w);
        {
            let statuses = w.statuses.borrow();
            assert_eq!(statuses[Path::new("/r")].standing, Standing::Fail);
            assert_eq!(statuses[Path::new("/r")].phase, Phase::Edit);
        }
        w.findings.clear();
        run(&w);
        assert_eq!(
            w.statuses.borrow()[Path::new("/r")].standing,
            Standing::Pass
        );
    }

    #[test]
    fn a_skipped_file_records_nothing() {
        let w = world();
        run_with(&w, "/r/README.md", "/r");
        assert!(w.statuses.borrow().is_empty());
    }
}
