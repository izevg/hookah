//! `CheckSession`: the stop phase.
//!
//! Ask the change source for the files changed since the last commit and group them by project.
//! For each project, in order:
//!
//! 1. Lint, Typecheck, Architecture, and Unused over all its changed files. This catches files
//!    changed outside edit hooks (a shell `sed`). Format never runs at stop. Unused (a
//!    whole-project analysis) runs here and never in the edit phase.
//! 2. Test, on the light tests the detector selects as related. Without related tests nothing
//!    runs, and neither does anything when more targets than the test budget are selected.
//! 3. Mutation, only if at least one changed file is a test and the change fits the mutation
//!    budget; the detector picks the targets.
//!
//! Going over a budget is never a failure: the check is skipped and a notice tells the agent.
//!
//! Findings of kind `Test` and `Mutation` are never filtered by changed lines or a baseline. With
//! a ready session baseline, the static findings reported are exactly the new ones. Tools the project
//! lacks become non-blocking `MissingTool` notices. Finding paths are rendered relative to the
//! request's display root, so projects in a monorepo stay distinguishable.

use std::path::Path;

use hookah_domain::finding::Severity;
use hookah_domain::light::{Admission, Budgets};
use hookah_domain::project::locate_project;
use hookah_domain::session::{ChangeSet, ChangedLines, FileRole, Phase, TestRun, mutation_applies};
use hookah_domain::toolchain::{CheckKind, MutationSelection, TestSelection};
use hookah_domain::verdict::{ReportingFilter, Verdict};

use super::baseline::{lookup, probe};
use super::request::{Outcome, RunLog, StopRequest};
use super::status::record;
use crate::ports::Ports;

struct LocatedChanges {
    located: Vec<(hookah_domain::project::Project, std::path::PathBuf)>,
    errors: Vec<String>,
}

/// Runs the static checks, related tests, and mutation testing for everything changed in the
/// session.
pub struct CheckSession<'a> {
    ports: Ports<'a>,
    report_limit: usize,
    budgets: Budgets,
    allow_dir: Option<&'a dyn Fn(&Path) -> bool>,
}

impl<'a> CheckSession<'a> {
    /// Creates the use case; `report_limit` caps the findings rendered in the report.
    pub fn new(ports: Ports<'a>, report_limit: usize) -> Self {
        Self {
            ports,
            report_limit,
            budgets: Budgets::default(),
            allow_dir: None,
        }
    }

    /// Replaces the default [`Budgets`] of the light stop phase.
    #[must_use]
    pub fn with_budgets(mut self, budgets: Budgets) -> Self {
        self.budgets = budgets;
        self
    }

    /// Limits checks to changed files whose directory satisfies `allow`.
    #[must_use]
    pub fn with_dir_filter(mut self, allow: &'a dyn Fn(&Path) -> bool) -> Self {
        self.allow_dir = Some(allow);
        self
    }

    fn allows(&self, dir: &Path) -> bool {
        self.allow_dir.is_none_or(|allow| allow(dir))
    }

    /// Read-only discovery of the changed-file config directories `run` evaluates for `root`
    /// (before filtering). Falls back to `[root]` when there are no changed checkable files.
    pub fn discover_scopes(&self, root: &Path) -> Vec<std::path::PathBuf> {
        let Ok(found) = self.collect_changed_files(root) else {
            return vec![root.to_path_buf()];
        };
        let mut scopes = Vec::new();
        for (project, file) in found.located {
            let dir = file.parent().unwrap_or(&project.root).to_path_buf();
            if !scopes.contains(&dir) {
                scopes.push(dir);
            }
        }
        if scopes.is_empty() {
            scopes.push(root.to_path_buf());
        }
        scopes
    }

    fn collect_changed_files(&self, root: &Path) -> Result<LocatedChanges, crate::PortError> {
        let files = self.ports.changes.changed_files(root)?;
        let mut located = Vec::new();
        let mut errors = Vec::new();
        for file in files {
            match self.ports.locator.manifests_above(&file) {
                Err(e) => errors.push(format!("locating project: {e}")),
                Ok(manifests) => {
                    if let Some(p) = locate_project(&file, &manifests)
                        && p.ecosystem.can_check(&file)
                    {
                        located.push((p, file));
                    }
                }
            }
        }
        Ok(LocatedChanges { located, errors })
    }

    /// Runs the stop phase. Never panics and never fails on a port error.
    pub fn run(&self, request: &StopRequest) -> Outcome {
        let found = match self.collect_changed_files(&request.root) {
            Ok(found) => found,
            Err(_) if !self.allows(&request.root) => return Outcome::pass(),
            Err(e) => return Outcome::degraded(format!("reading changes: {e}")),
        };
        let mut total = RunLog::default();
        for err in found.errors {
            total.fail(err);
        }
        let mut set = ChangeSet::new();
        let mut filtered_out = false;
        for (p, file) in &found.located {
            if self.allows(file.parent().unwrap_or(&p.root)) {
                set.add(p, file);
            } else {
                filtered_out = true;
            }
        }

        let mut checked = false;
        for group in set.groups() {
            checked = true;
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
            if let Some(baseline) = lookup(&self.ports, project, request.session.as_deref(), false)
            {
                filter = filter.with_baseline(baseline);
            }
            let mut static_targets = Vec::new();
            let (mut tests, mut sources) = (Vec::new(), Vec::new());
            let mut line_counts = Vec::new();
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
                line_counts.push((file.clone(), lines.as_ref().and_then(ChangedLines::count)));
                if let Some(lines) = lines {
                    filter.track(rel, lines);
                }
            }

            let changed: Vec<_> = group.files.iter().cloned().collect();
            let test_targets = match self.ports.detector.select_tests(project, &changed) {
                Ok(TestSelection::Targets(t)) => match self.budgets.admit_tests(t.len()) {
                    Admission::Run => t,
                    Admission::Off => vec![],
                    Admission::Skip(skip) => {
                        log.skipped(&skip);
                        vec![]
                    }
                },
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
            let mut mutant_targets = if mutation {
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

            // Mutation is skipped, with a notice once it would have run, for a large change.
            let mut mutation_skip = None;
            if !mutant_targets.is_empty() && toolchain.of_kind(CheckKind::Mutation).next().is_some()
            {
                let lines = self.changed_line_total(&line_counts);
                match self.budgets.admit_mutation(line_counts.len(), lines) {
                    Admission::Run => {}
                    Admission::Off => mutant_targets.clear(),
                    Admission::Skip(skip) => {
                        mutant_targets.clear();
                        mutation_skip = Some(skip);
                    }
                }
            }

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
                if kind == CheckKind::Mutation
                    && let Some(skip) = &mutation_skip
                {
                    log.skipped(skip);
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
            let project_outcome = log.clone().finish(self.report_limit);
            record(
                &self.ports,
                &project.root,
                Phase::Stop,
                &project_outcome.verdict,
            );
            total.absorb(log);
        }
        if !checked && !filtered_out && self.allows(&request.root) {
            self.record_clean(&request.root);
        }
        total.finish(self.report_limit)
    }

    /// The changed lines over all files; a new file counts as long as the file is.
    fn changed_line_total(&self, counts: &[(std::path::PathBuf, Option<usize>)]) -> usize {
        counts
            .iter()
            .map(|(file, count)| {
                count.unwrap_or_else(|| self.ports.changes.line_count(file).unwrap_or(0))
            })
            .sum()
    }

    /// Nothing changed: the project at `root` is clean, which replaces an older failure.
    fn record_clean(&self, root: &std::path::Path) {
        let probe = probe(root);
        let Ok(manifests) = self.ports.locator.manifests_above(&probe) else {
            return;
        };
        if let Some(project) = locate_project(&probe, &manifests)
            && self.allows(&project.root)
        {
            record(&self.ports, &project.root, Phase::Stop, &Verdict::Pass);
        }
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
    fn unused_code_runs_after_architecture_at_stop() {
        let mut w = World::new(
            "/r/Cargo.toml",
            vec![
                tool("unused", CheckKind::Unused),
                tool("arch", CheckKind::Architecture),
            ],
        );
        w.files = vec!["/r/src/a.rs".into()];
        assert_eq!(run(&w), Outcome::pass());
        assert_eq!(w.calls(), ["arch src/a.rs", "unused src/a.rs"]);
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

    fn run_with(w: &World, budgets: Budgets) -> Outcome {
        CheckSession::new(w.ports(), 20)
            .with_budgets(budgets)
            .run(&StopRequest::new("/r"))
    }

    fn degraded(text: &str) -> Notice {
        Notice::Degraded(text.into())
    }

    #[test]
    fn tests_over_the_budget_are_not_run_and_a_notice_says_so() {
        let mut w = world();
        w.selection = TestSelection::Targets(vec!["a".into(), "b".into(), "c".into()]);
        let out = run_with(&w, Budgets::new(2, 3, 150));
        assert_eq!(out.verdict, Verdict::Pass);
        assert_eq!(
            out.notices,
            [degraded(
                "3 related tests selected; above the light budget of 2, run them yourself"
            )]
        );
        assert!(w.calls().is_empty());
        // Exactly at the budget still runs.
        let out = run_with(&w, Budgets::new(3, 3, 150));
        assert_eq!(out, Outcome::pass());
        assert_eq!(w.calls(), ["test a,b,c"]);
    }

    #[test]
    fn a_zero_test_budget_runs_no_tests_and_says_nothing() {
        let mut w = world();
        w.selection = TestSelection::Targets(vec!["a".into()]);
        assert_eq!(run_with(&w, Budgets::new(0, 3, 150)), Outcome::pass());
        assert!(w.calls().is_empty());
    }

    #[test]
    fn skipped_tests_do_not_report_a_missing_test_tool() {
        let mut w = world();
        w.toolchain =
            hookah_domain::toolchain::Toolchain::new(vec![]).with_missing(vec![MissingTool::new(
                "runner",
                CheckKind::Test,
                "install it",
            )]);
        w.selection = TestSelection::Targets(vec!["a".into(), "b".into()]);
        let out = run_with(&w, Budgets::new(1, 3, 150));
        assert_eq!(out.notices.len(), 1);
        assert!(matches!(&out.notices[0], Notice::Degraded(_)));
    }

    #[test]
    fn mutation_over_the_file_budget_is_skipped_with_a_notice() {
        let w = mutation_world();
        // Two code files changed (a.rs and b.rs), ten lines each by default.
        let out = run_with(&w, Budgets::new(25, 1, 150));
        assert_eq!(out.verdict, Verdict::Pass);
        assert_eq!(
            out.notices,
            [degraded(
                "mutation testing skipped: change too large (2 files / 20 lines, budget 1 / 150)"
            )]
        );
        assert_eq!(w.calls(), ["test t"]);
        assert_eq!(run_with(&w, Budgets::new(25, 2, 20)), Outcome::pass());
    }

    #[test]
    fn mutation_over_the_line_budget_counts_ranges_and_new_files() {
        let mut w = mutation_world();
        w.lines = ChangedLines::Ranges(vec![
            hookah_domain::session::LineRange::new(3, 7).unwrap(),
            hookah_domain::session::LineRange::new(9, 9).unwrap(),
        ]);
        let out = run_with(&w, Budgets::new(25, 3, 11));
        assert_eq!(
            out.notices,
            [degraded(
                "mutation testing skipped: change too large (2 files / 12 lines, budget 3 / 11)"
            )]
        );
        assert_eq!(w.calls(), ["test t"]);
        assert_eq!(run_with(&w, Budgets::new(25, 3, 12)), Outcome::pass());

        // A new file counts as long as the file is; an unreadable one counts as nothing.
        let mut w = mutation_world();
        w.file_lines = 100;
        assert_eq!(run_with(&w, Budgets::new(25, 3, 199)).notices.len(), 1);
        assert_eq!(run_with(&w, Budgets::new(25, 3, 200)), Outcome::pass());
        w.lines_fail = true;
        assert_eq!(run_with(&w, Budgets::new(25, 3, 1)), Outcome::pass());
    }

    #[test]
    fn a_zero_mutation_budget_switches_mutation_off_silently() {
        let w = mutation_world();
        for budgets in [Budgets::new(25, 0, 150), Budgets::new(25, 3, 0)] {
            assert_eq!(run_with(&w, budgets), Outcome::pass());
            assert_eq!(w.calls().last().unwrap(), "test t");
        }
        assert_eq!(w.calls(), ["test t", "test t"]);
    }

    #[test]
    fn a_large_change_without_a_mutation_tool_says_nothing_about_mutation() {
        let mut w = mutation_world();
        w.toolchain = hookah_domain::toolchain::Toolchain::new(vec![tool("test", CheckKind::Test)]);
        assert_eq!(run_with(&w, Budgets::new(25, 1, 1)), Outcome::pass());
    }

    #[test]
    fn mutation_skip_is_not_announced_when_failed_tests_stop_the_run_first() {
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
        let out = run_with(&w, Budgets::new(25, 1, 150));
        assert!(matches!(out.verdict, Verdict::Fail(_)));
        assert!(out.notices.is_empty());
    }

    #[test]
    fn a_mutation_timeout_degrades_and_never_fails() {
        let mut w = mutation_world();
        w.timeouts = vec!["mut".into()];
        let out = run(&w);
        assert!(matches!(out.verdict, Verdict::Degraded(r) if r.contains("mut timed out")));
        assert_eq!(w.calls(), ["test t", "mut src/a.rs"]);
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

    mod baseline {
        use hookah_domain::finding::baseline::Baseline;

        use super::*;
        use crate::ports::BaselineState;

        fn lint_world() -> World {
            let mut w = World::new(
                "/r/Cargo.toml",
                vec![tool("lint", CheckKind::Lint), tool("test", CheckKind::Test)],
            );
            w.files = vec!["/r/src/a.rs".into()];
            w.selection = TestSelection::Targets(vec!["t".into()]);
            w
        }

        fn run_in_session(w: &World, session: Option<&str>) -> Outcome {
            let request = StopRequest::new("/r").with_session(session.map(Into::into));
            CheckSession::new(w.ports(), 20).run(&request)
        }

        fn lines(out: Outcome) -> Vec<String> {
            match out.verdict {
                Verdict::Fail(r) => r.lines(),
                _ => vec![],
            }
        }

        #[test]
        fn only_new_static_findings_are_reported_and_tests_never_filtered() {
            let mut w = lint_world();
            let old = finding(CheckKind::Lint, Severity::Error, "src/other.rs", 1, "old");
            let new = finding(CheckKind::Lint, Severity::Warning, "src/other.rs", 2, "new");
            let test = finding(CheckKind::Test, Severity::Error, "src/other.rs", 1, "boom");
            w.findings.insert("lint".into(), vec![old.clone(), new]);
            w.findings.insert("test".into(), vec![test.clone()]);
            assert_eq!(
                lines(run_in_session(&w, None)),
                ["src/other.rs:1: old", "src/other.rs:1: boom"]
            );
            w.set_baseline(
                "s",
                BaselineState::Ready(Baseline::from_findings([&old, &test])),
            );
            assert_eq!(
                lines(run_in_session(&w, Some("s"))),
                ["src/other.rs:1: boom", "src/other.rs:2: warning: new"]
            );
        }

        #[test]
        fn the_stop_phase_never_starts_a_baseline_job() {
            let w = lint_world();
            run_in_session(&w, Some("s"));
            assert!(w.spawned.borrow().is_empty());
            assert_eq!(w.baseline_state("s"), BaselineState::Absent);
        }
    }

    #[test]
    fn records_the_stop_status_of_each_project() {
        use hookah_domain::status::Standing;
        let mut w = world();
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
        run(&w);
        let statuses = w.statuses.borrow();
        let status = &statuses[Path::new("/r")];
        assert_eq!(status.phase, Phase::Stop);
        assert_eq!(status.standing, Standing::Fail);
        assert_eq!(status.problems, 1);
    }

    #[test]
    fn a_clean_session_replaces_an_old_failure() {
        use hookah_domain::status::Standing;
        let mut w = world();
        w.files = vec![];
        let failing = Verdict::Degraded("old".into());
        record(&w.ports(), Path::new("/r"), Phase::Edit, &failing);
        assert_eq!(run(&w), Outcome::pass());
        let statuses = w.statuses.borrow();
        assert_eq!(statuses[Path::new("/r")].standing, Standing::Pass);
        assert_eq!(statuses[Path::new("/r")].phase, Phase::Stop);
    }

    #[test]
    fn an_unwritable_status_store_changes_no_verdict() {
        let mut w = world();
        w.record_fails = true;
        assert_eq!(run(&w), Outcome::pass());
    }
}
