//! `CheckSession`: the stop phase.
//!
//! Ask the change source for the files changed since the last commit and group them by project.
//! For each project, in order:
//!
//! 1. Lint, Typecheck, and Architecture over all its changed files. This catches files changed
//!    outside edit hooks (a shell `sed`). Format never runs at stop.
//! 2. Test, on the tests the detector selects as related. Without related tests nothing runs.
//! 3. Mutation, only if at least one changed file is a test; the detector picks the targets.
//!
//! Findings of kind `Test` and `Mutation` are never filtered by changed lines. Tools the project
//! lacks become non-blocking `MissingTool` notices. Finding paths are rendered relative to the
//! request's display root, so projects in a monorepo stay distinguishable.

use hookah_domain::finding::Severity;
use hookah_domain::project::locate_project;
use hookah_domain::session::{ChangeSet, ChangedLines, FileRole, Phase, TestRun, mutation_applies};
use hookah_domain::toolchain::{CheckKind, MutationSelection, TestSelection};
use hookah_domain::verdict::ReportingFilter;

use super::request::{Outcome, RunLog, StopRequest};
use crate::ports::Ports;

/// Runs the static checks, related tests, and mutation testing for everything changed in the
/// session.
pub struct CheckSession<'a> {
    ports: Ports<'a>,
    report_limit: usize,
}

impl<'a> CheckSession<'a> {
    /// Creates the use case; `report_limit` caps the findings rendered in the report.
    pub fn new(ports: Ports<'a>, report_limit: usize) -> Self {
        Self {
            ports,
            report_limit,
        }
    }

    /// Runs the stop phase. Never panics and never fails on a port error.
    pub fn run(&self, request: &StopRequest) -> Outcome {
        let files = match self.ports.changes.changed_files(&request.root) {
            Ok(f) => f,
            Err(e) => return Outcome::degraded(format!("reading changes: {e}")),
        };
        let mut total = RunLog::default();
        let mut set = ChangeSet::new();
        for file in &files {
            match self.ports.locator.manifests_above(file) {
                Err(e) => total.fail(format!("locating project: {e}")),
                Ok(manifests) => {
                    if let Some(p) = locate_project(file, &manifests)
                        && p.ecosystem.can_check(file)
                    {
                        set.add(&p, file);
                    }
                }
            }
        }

        for group in set.groups() {
            let project = &group.project;
            let toolchain = match self.ports.detector.detect(project) {
                Ok(t) => t,
                Err(e) => {
                    total.fail(format!("detecting toolchain: {e}"));
                    continue;
                }
            };
            let mut log = RunLog::default();

            // Changed lines drive both the reporting filter and the file roles.
            let mut filter = ReportingFilter::new();
            let mut static_targets = Vec::new();
            let (mut tests, mut sources) = (Vec::new(), Vec::new());
            for file in &group.files {
                let Ok(rel) = project.relative(file) else {
                    continue;
                };
                static_targets.push(rel.to_string_lossy().into_owned());
                let lines = self.ports.changes.changed_lines(&project.root, file).ok();
                let fallback = ChangedLines::All;
                let role = self.ports.detector.classify(
                    project,
                    file,
                    lines.as_ref().unwrap_or(&fallback),
                );
                match role {
                    FileRole::Test => tests.push(file.clone()),
                    FileRole::Source => sources.push(file.clone()),
                }
                if let Some(lines) = lines {
                    filter.track(rel, lines);
                }
            }

            let changed: Vec<_> = group.files.iter().cloned().collect();
            let test_targets = match self.ports.detector.select_tests(project, &changed) {
                Ok(TestSelection::Targets(t)) => t,
                Ok(TestSelection::None) => vec![],
                Err(e) => {
                    log.fail(format!("selecting tests: {e}"));
                    vec![]
                }
            };

            let mutation = mutation_applies(
                std::iter::repeat_n(FileRole::Test, tests.len())
                    .chain(std::iter::repeat_n(FileRole::Source, sources.len())),
            );
            let mutant_targets = if mutation {
                match self
                    .ports
                    .detector
                    .select_mutants(project, &tests, &sources)
                {
                    MutationSelection::Targets(t) => t,
                    MutationSelection::None => vec![],
                }
            } else {
                vec![]
            };

            log.report_missing(&toolchain, |k| match k {
                CheckKind::Format => false,
                CheckKind::Test => !test_targets.is_empty(),
                CheckKind::Mutation => mutation,
                _ => true,
            });
            let mut test_run = TestRun::NotRun;
            for &kind in Phase::Stop.check_kinds() {
                if kind == CheckKind::Mutation && !test_run.allows_mutation() {
                    continue;
                }
                let targets = match kind {
                    CheckKind::Test => &test_targets,
                    CheckKind::Mutation => &mutant_targets,
                    _ => &static_targets,
                };
                if targets.is_empty() {
                    continue;
                }
                let (found_before, failed_before) = (log.findings.len(), log.failure_count());
                let mut ran = false;
                for tool in toolchain.of_kind(kind) {
                    ran = true;
                    log.run_tool(&self.ports, project, tool, targets);
                }
                if kind == CheckKind::Test {
                    let errors = log.findings[found_before..]
                        .iter()
                        .any(|f| f.kind == CheckKind::Test && f.severity == Severity::Error);
                    test_run = TestRun::observe(ran, errors, log.failure_count() > failed_before);
                }
            }

            log.filter(&filter, &project.root, &request.display_root);
            total.absorb(log);
        }
        total.finish(self.report_limit)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::Path;

    use hookah_domain::finding::{Finding, Location, Severity};
    use hookah_domain::toolchain::MissingTool;
    use hookah_domain::verdict::{Notice, Verdict};

    use super::*;
    use crate::ports::fakes::{World, tool};

    fn world() -> World {
        let mut w = World::new(
            "/r/Cargo.toml",
            vec![
                tool("fmt", CheckKind::Format),
                tool("test", CheckKind::Test),
            ],
        );
        w.files = vec![
            "/r/src/a.rs".into(),
            "/r/src/b.rs".into(),
            "/r/README.md".into(),
        ];
        w
    }

    fn run(w: &World) -> Outcome {
        CheckSession::new(w.ports(), 20).run(&StopRequest::new("/r"))
    }

    fn finding(kind: CheckKind, sev: Severity, path: &str, line: u32, msg: &str) -> Finding {
        Finding::new(kind, sev, Location::line(path, line), msg)
    }

    #[test]
    fn no_related_tests_runs_nothing() {
        let w = world();
        assert_eq!(run(&w), Outcome::pass());
        assert!(w.calls().is_empty());
    }

    #[test]
    fn runs_test_tools_with_selected_targets_and_never_format() {
        let mut w = world();
        w.selection = TestSelection::Targets(vec!["pkg_a".into(), "pkg_b".into()]);
        assert_eq!(run(&w), Outcome::pass());
        assert_eq!(w.calls(), ["test pkg_a,pkg_b"]);
    }

    #[test]
    fn static_checks_run_over_all_changed_files_before_tests() {
        let mut w = World::new(
            "/r/Cargo.toml",
            vec![
                tool("mut", CheckKind::Mutation),
                tool("test", CheckKind::Test),
                tool("arch", CheckKind::Architecture),
                tool("types", CheckKind::Typecheck),
                tool("lint", CheckKind::Lint),
                tool("fmt", CheckKind::Format),
            ],
        );
        w.files = vec!["/r/src/b.rs".into(), "/r/src/a.rs".into()];
        w.selection = TestSelection::Targets(vec!["t".into()]);
        assert_eq!(run(&w), Outcome::pass());
        assert_eq!(
            w.calls(),
            [
                "lint src/a.rs,src/b.rs",
                "types src/a.rs,src/b.rs",
                "arch src/a.rs,src/b.rs",
                "test t"
            ]
        );
    }

    #[test]
    fn mutation_runs_only_when_a_changed_file_is_a_test() {
        let mut w = world();
        w.toolchain =
            hookah_domain::toolchain::Toolchain::new(vec![tool("mut", CheckKind::Mutation)]);
        w.mutants = MutationSelection::Targets(vec!["src/a.rs".into()]);
        assert_eq!(run(&w), Outcome::pass());
        assert!(w.calls().is_empty());
        assert!(w.mutant_calls.borrow().is_empty());

        w.test_files = vec!["/r/src/b.rs".into()];
        assert_eq!(run(&w), Outcome::pass());
        assert_eq!(w.calls(), ["mut src/a.rs"]);
        assert_eq!(
            *w.mutant_calls.borrow(),
            [(
                vec![Path::new("/r/src/b.rs").to_path_buf()],
                vec![Path::new("/r/src/a.rs").to_path_buf()]
            )]
        );
    }

    #[test]
    fn mutation_with_no_targets_runs_nothing() {
        let mut w = world();
        w.toolchain =
            hookah_domain::toolchain::Toolchain::new(vec![tool("mut", CheckKind::Mutation)]);
        w.test_files = vec!["/r/src/b.rs".into()];
        assert_eq!(run(&w), Outcome::pass());
        assert!(w.calls().is_empty());
    }

    fn mutation_world() -> World {
        let mut w = world();
        w.toolchain = hookah_domain::toolchain::Toolchain::new(vec![
            tool("test", CheckKind::Test),
            tool("mut", CheckKind::Mutation),
        ]);
        w.test_files = vec!["/r/src/b.rs".into()];
        w.selection = TestSelection::Targets(vec!["t".into()]);
        w.mutants = MutationSelection::Targets(vec!["src/a.rs".into()]);
        w
    }

    #[test]
    fn mutation_is_skipped_after_failed_tests() {
        let mut w = mutation_world();
        w.findings.insert(
            "test".into(),
            vec![finding(
                CheckKind::Test,
                Severity::Error,
                "src/a.rs",
                3,
                "assertion failed",
            )],
        );
        assert!(matches!(run(&w).verdict, Verdict::Fail(_)));
        assert_eq!(w.calls(), ["test t"]);

        let mut w = mutation_world();
        w.run_fails = vec!["test".into()];
        assert!(matches!(run(&w).verdict, Verdict::Degraded(_)));
        assert_eq!(w.calls(), ["test t"]);

        let mut w = mutation_world();
        w.timeouts = vec!["test".into()];
        assert!(matches!(run(&w).verdict, Verdict::Degraded(_)));
        assert_eq!(w.calls(), ["test t"]);
    }

    #[test]
    fn mutation_runs_after_green_tests_and_when_no_tests_ran() {
        let w = mutation_world();
        assert_eq!(run(&w), Outcome::pass());
        assert_eq!(w.calls(), ["test t", "mut src/a.rs"]);

        let mut w = mutation_world();
        w.selection = TestSelection::None;
        assert_eq!(run(&w), Outcome::pass());
        assert_eq!(w.calls(), ["mut src/a.rs"]);
    }

    #[test]
    fn test_warnings_do_not_block_mutation() {
        let mut w = mutation_world();
        w.findings.insert(
            "test".into(),
            vec![finding(
                CheckKind::Test,
                Severity::Warning,
                "src/a.rs",
                3,
                "slow test",
            )],
        );
        run(&w);
        assert_eq!(w.calls(), ["test t", "mut src/a.rs"]);
    }

    #[test]
    fn surviving_mutants_fail_and_are_not_line_filtered() {
        let mut w = world();
        w.toolchain =
            hookah_domain::toolchain::Toolchain::new(vec![tool("mut", CheckKind::Mutation)]);
        w.test_files = vec!["/r/src/b.rs".into()];
        w.mutants = MutationSelection::Targets(vec!["src/a.rs".into()]);
        w.lines = ChangedLines::Ranges(vec![]);
        w.findings.insert(
            "mut".into(),
            vec![finding(
                CheckKind::Mutation,
                Severity::Error,
                "src/a.rs",
                9,
                "mutant survived: replace + with -",
            )],
        );
        let Verdict::Fail(r) = run(&w).verdict else {
            panic!("expected fail");
        };
        assert_eq!(
            r.lines(),
            ["src/a.rs:9: [mutation] mutant survived: replace + with -"]
        );
    }

    #[test]
    fn test_warnings_outside_changed_lines_are_kept_but_lint_warnings_are_not() {
        let mut w = world();
        w.toolchain = hookah_domain::toolchain::Toolchain::new(vec![
            tool("lint", CheckKind::Lint),
            tool("test", CheckKind::Test),
        ]);
        w.selection = TestSelection::Targets(vec!["t".into()]);
        w.lines = ChangedLines::Ranges(vec![]);
        w.findings.insert(
            "lint".into(),
            vec![finding(
                CheckKind::Lint,
                Severity::Warning,
                "src/a.rs",
                1,
                "l",
            )],
        );
        w.findings.insert(
            "test".into(),
            vec![finding(
                CheckKind::Test,
                Severity::Warning,
                "src/a.rs",
                2,
                "t",
            )],
        );
        let Verdict::Fail(r) = run(&w).verdict else {
            panic!("expected fail");
        };
        assert_eq!(r.lines(), ["src/a.rs:2: warning: t"]);
    }

    #[test]
    fn lint_findings_at_stop_are_filtered_by_changed_lines() {
        let mut w = world();
        w.toolchain = hookah_domain::toolchain::Toolchain::new(vec![tool("lint", CheckKind::Lint)]);
        w.findings.insert(
            "lint".into(),
            vec![
                finding(CheckKind::Lint, Severity::Warning, "src/a.rs", 1, "w"),
                finding(CheckKind::Lint, Severity::Error, "src/c.rs", 1, "e"),
            ],
        );
        let Verdict::Fail(r) = run(&w).verdict else {
            panic!("expected fail");
        };
        assert_eq!(r.lines(), ["src/c.rs:1: e", "src/a.rs:1: warning: w"]);
    }

    #[test]
    fn missing_tools_become_notices_per_phase_rules() {
        let mut w = world();
        let missing = vec![
            MissingTool::new("fmt", CheckKind::Format, "h"),
            MissingTool::new("tsc", CheckKind::Typecheck, "h"),
            MissingTool::new("runner", CheckKind::Test, "h"),
            MissingTool::new("mutator", CheckKind::Mutation, "h"),
        ];
        w.toolchain = hookah_domain::toolchain::Toolchain::default().with_missing(missing);
        let names = |o: &Outcome| -> Vec<String> {
            o.notices
                .iter()
                .map(|n| match n {
                    Notice::MissingTool(t) => t.name.clone(),
                    other => panic!("unexpected {other:?}"),
                })
                .collect()
        };
        // No related tests and no changed test: only the static kind is missing-relevant.
        let out = run(&w);
        assert_eq!(out.verdict, Verdict::Pass);
        assert_eq!(names(&out), ["tsc"]);
        // Related tests and a changed test file: everything but Format.
        w.selection = TestSelection::Targets(vec!["t".into()]);
        w.test_files = vec!["/r/src/b.rs".into()];
        assert_eq!(names(&run(&w)), ["tsc", "runner", "mutator"]);
    }

    #[test]
    fn failing_tests_fail_the_verdict() {
        let mut w = world();
        w.selection = TestSelection::Targets(vec!["t".into()]);
        w.findings.insert(
            "test".into(),
            vec![finding(
                CheckKind::Test,
                Severity::Error,
                "src/a.rs",
                3,
                "assertion failed",
            )],
        );
        let Verdict::Fail(r) = run(&w).verdict else {
            panic!("expected fail");
        };
        assert_eq!(r.lines(), ["src/a.rs:3: assertion failed"]);
    }

    #[test]
    fn nested_project_paths_are_relative_to_the_display_root() {
        let mut w = world();
        w.manifests = vec![
            hookah_domain::project::Manifest::from_path(Path::new("/r/pkg/Cargo.toml")).unwrap(),
        ];
        w.files = vec!["/r/pkg/src/a.rs".into()];
        w.selection = TestSelection::Targets(vec!["t".into()]);
        w.findings.insert(
            "test".into(),
            vec![finding(
                CheckKind::Test,
                Severity::Error,
                "src/a.rs",
                3,
                "boom",
            )],
        );
        let lines = |req: StopRequest| {
            let Verdict::Fail(r) = CheckSession::new(w.ports(), 20).run(&req).verdict else {
                panic!("expected fail");
            };
            r.lines()
        };
        assert_eq!(lines(StopRequest::new("/r")), ["pkg/src/a.rs:3: boom"]);
        assert_eq!(
            lines(StopRequest::new("/r").with_display_root("/r/pkg")),
            ["src/a.rs:3: boom"]
        );
        assert_eq!(
            lines(StopRequest::new("/r").with_display_root("/other")),
            ["/r/pkg/src/a.rs:3: boom"]
        );
    }

    #[test]
    fn sibling_project_paths_render_normalized_like_in_the_edit_phase() {
        let mut w = world();
        w.manifests = vec![
            hookah_domain::project::Manifest::from_path(Path::new("/r/core/Cargo.toml")).unwrap(),
        ];
        w.files = vec!["/r/core/src/a.rs".into()];
        w.selection = TestSelection::Targets(vec!["t".into()]);
        w.findings.insert(
            "test".into(),
            vec![finding(
                CheckKind::Test,
                Severity::Error,
                "src/a.rs",
                3,
                "boom",
            )],
        );
        let lines = |req: StopRequest| {
            let Verdict::Fail(r) = CheckSession::new(w.ports(), 20).run(&req).verdict else {
                panic!("expected fail");
            };
            r.lines()
        };
        // A non-normalized display root renders the same as its normalized form.
        assert_eq!(
            lines(StopRequest::new("/r").with_display_root("/r/app/..")),
            ["core/src/a.rs:3: boom"]
        );
        // A sibling directory is outside the display root: absolute, never `core/../`.
        assert_eq!(
            lines(StopRequest::new("/r").with_display_root("/r/app")),
            ["/r/core/src/a.rs:3: boom"]
        );
    }

    #[test]
    fn port_errors_degrade() {
        let mut w = world();
        w.files_fail = true;
        assert!(matches!(run(&w).verdict, Verdict::Degraded(_)));
        let mut w = world();
        w.select_fails = true;
        assert!(matches!(run(&w).verdict, Verdict::Degraded(_)));
        let mut w = world();
        w.selection = TestSelection::Targets(vec!["t".into()]);
        w.timeouts = vec!["test".into()];
        assert!(matches!(run(&w).verdict, Verdict::Degraded(_)));
    }
}
