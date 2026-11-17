//! `CheckSession`: the stop phase.
//!
//! Ask the change source for the files changed since the last commit, group them by project,
//! select the tests related to each group, and run them. Without related tests nothing runs.
//! Finding paths are relative to the request root, so projects in a monorepo stay distinguishable.

use std::path::Path;

use hookah_domain::project::locate_project;
use hookah_domain::session::{ChangeSet, Phase};
use hookah_domain::toolchain::TestSelection;
use hookah_domain::verdict::ReportingFilter;

use super::request::{Outcome, RunLog, StopRequest};
use crate::ports::Ports;

/// Runs the tests related to everything changed in the session.
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
            let mut log = RunLog::default();
            let toolchain = match self.ports.detector.detect(project) {
                Ok(t) => t,
                Err(e) => {
                    total.fail(format!("detecting toolchain: {e}"));
                    continue;
                }
            };
            let changed: Vec<_> = group.files.iter().cloned().collect();
            let targets = match self.ports.detector.select_tests(project, &changed) {
                Ok(TestSelection::Targets(t)) => t,
                Ok(TestSelection::None) => continue,
                Err(e) => {
                    total.fail(format!("selecting tests: {e}"));
                    continue;
                }
            };
            for &kind in Phase::Stop.check_kinds() {
                for tool in toolchain.of_kind(kind) {
                    log.run_tool(&self.ports, project, tool, &targets);
                }
            }

            let mut filter = ReportingFilter::new();
            for file in &group.files {
                let Ok(rel) = project.relative(file) else {
                    continue;
                };
                if let Ok(lines) = self.ports.changes.changed_lines(&project.root, file) {
                    filter.track(rel, lines);
                }
            }
            let prefix = project
                .root
                .strip_prefix(&request.root)
                .unwrap_or(Path::new(""));
            log.filter(&filter, prefix);
            total.absorb(log);
        }
        total.finish(self.report_limit)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use hookah_domain::finding::{Finding, Location, Severity};
    use hookah_domain::toolchain::CheckKind;
    use hookah_domain::verdict::Verdict;

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
        CheckSession::new(w.ports(), 20).run(&StopRequest { root: "/r".into() })
    }

    #[test]
    fn no_related_tests_runs_nothing() {
        let w = world();
        assert_eq!(run(&w), Outcome::pass());
        assert!(w.calls().is_empty());
    }

    #[test]
    fn runs_only_test_tools_with_selected_targets() {
        let mut w = world();
        w.selection = TestSelection::Targets(vec!["pkg_a".into(), "pkg_b".into()]);
        assert_eq!(run(&w), Outcome::pass());
        assert_eq!(w.calls(), ["test pkg_a,pkg_b"]);
    }

    #[test]
    fn failing_tests_fail_the_verdict() {
        let mut w = world();
        w.selection = TestSelection::Targets(vec!["t".into()]);
        w.findings.insert(
            "test".into(),
            vec![Finding::new(
                CheckKind::Test,
                Severity::Error,
                Location::line("src/a.rs", 3),
                "assertion failed",
            )],
        );
        let Verdict::Fail(r) = run(&w).verdict else {
            panic!("expected fail");
        };
        assert_eq!(r.lines(), ["src/a.rs:3: assertion failed"]);
    }

    #[test]
    fn nested_project_paths_get_a_prefix() {
        let mut w = world();
        w.manifests = vec![
            hookah_domain::project::Manifest::from_path(Path::new("/r/pkg/Cargo.toml")).unwrap(),
        ];
        w.files = vec!["/r/pkg/src/a.rs".into()];
        w.selection = TestSelection::Targets(vec!["t".into()]);
        w.findings.insert(
            "test".into(),
            vec![Finding::new(
                CheckKind::Test,
                Severity::Error,
                Location::line("src/a.rs", 3),
                "boom",
            )],
        );
        let Verdict::Fail(r) = run(&w).verdict else {
            panic!("expected fail");
        };
        assert_eq!(r.lines(), ["pkg/src/a.rs:3: boom"]);
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
