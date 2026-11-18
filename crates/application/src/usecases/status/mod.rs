//! Check status: recording the latest outcome per project, and reading it back.
//!
//! - [`record`] (edit and stop phases): stores the [`Status`] of a finished run. A store that
//!   fails is ignored: remembering a status never changes a verdict.
//! - [`ReadStatus`] (`hookah status`): the latest status of the project at a directory and of the
//!   projects nested below it.

use std::path::{Path, PathBuf};

use hookah_domain::project::locate_project;
use hookah_domain::session::Phase;
use hookah_domain::status::{RecordedStatus, Status};
use hookah_domain::verdict::Verdict;

use super::baseline::probe;
use crate::ports::Ports;

/// How many levels below the directory nested projects are looked for.
const NESTED_DEPTH: usize = 3;
/// At most this many projects are reported.
const MAX_PROJECTS: usize = 16;

/// Stores the status of a finished run of `phase` in the project at `root`. Failures are ignored.
pub(crate) fn record(ports: &Ports<'_>, root: &Path, phase: Phase, verdict: &Verdict) {
    let _ = ports.statuses.record(root, &Status::of(phase, verdict));
}

/// The latest status of one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectStatus {
    /// The project root.
    pub root: PathBuf,
    /// The latest recorded status; `None` when no run was recorded yet.
    pub recorded: Option<RecordedStatus>,
}

/// Reads the recorded statuses of the projects under a directory.
pub struct ReadStatus<'a> {
    ports: Ports<'a>,
}

impl<'a> ReadStatus<'a> {
    /// Creates the use case.
    pub fn new(ports: Ports<'a>) -> Self {
        Self { ports }
    }

    /// The project that holds `dir` (if any), then the projects nested below it, each with its
    /// latest status. A monorepo root holds no project itself and lists its members.
    pub fn run(&self, dir: &Path) -> Vec<ProjectStatus> {
        let mut dirs = vec![dir.to_path_buf()];
        if let Ok(nested) = self.ports.locator.nested_roots(dir, NESTED_DEPTH) {
            dirs.extend(nested);
        }
        let mut roots: Vec<PathBuf> = Vec::new();
        for dir in dirs {
            let file = probe(&dir);
            let Ok(manifests) = self.ports.locator.manifests_above(&file) else {
                continue;
            };
            if let Some(project) = locate_project(&file, &manifests)
                && !roots.contains(&project.root)
            {
                roots.push(project.root);
            }
        }
        roots.truncate(MAX_PROJECTS);
        roots
            .into_iter()
            .map(|root| ProjectStatus {
                recorded: self.ports.statuses.latest(&root),
                root,
            })
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use hookah_domain::project::Manifest;
    use hookah_domain::status::Standing;

    use super::*;
    use crate::ports::fakes::World;

    fn world() -> World {
        World::new("/r/Cargo.toml", vec![])
    }

    #[test]
    fn a_project_without_a_record_is_listed_without_a_status() {
        let w = world();
        let found = ReadStatus::new(w.ports()).run(Path::new("/r/src"));
        assert_eq!(
            found,
            [ProjectStatus {
                root: "/r".into(),
                recorded: None
            }]
        );
    }

    #[test]
    fn the_recorded_status_comes_back() {
        let w = world();
        record(&w.ports(), Path::new("/r"), Phase::Stop, &Verdict::Pass);
        let found = ReadStatus::new(w.ports()).run(Path::new("/r"));
        let recorded = found[0].recorded.as_ref().unwrap();
        assert_eq!(recorded.status.phase, Phase::Stop);
        assert_eq!(recorded.status.standing, Standing::Pass);
    }

    #[test]
    fn a_failing_store_is_ignored() {
        let mut w = world();
        w.record_fails = true;
        record(&w.ports(), Path::new("/r"), Phase::Edit, &Verdict::Pass);
        assert!(w.statuses.borrow().is_empty());
    }

    #[test]
    fn nested_projects_are_listed_once_each() {
        let mut w = world();
        w.manifests = vec![
            Manifest::from_path(Path::new("/m/a/Cargo.toml")).unwrap(),
            Manifest::from_path(Path::new("/m/b/Cargo.toml")).unwrap(),
        ];
        w.nested = vec!["/m/a".into(), "/m/b".into(), "/m/a".into()];
        let found = ReadStatus::new(w.ports()).run(Path::new("/m"));
        let roots: Vec<_> = found.iter().map(|p| p.root.clone()).collect();
        assert_eq!(roots, [PathBuf::from("/m/a"), PathBuf::from("/m/b")]);
    }

    #[test]
    fn nothing_is_listed_outside_projects() {
        let w = world();
        assert!(ReadStatus::new(w.ports()).run(Path::new("/x")).is_empty());
    }

    #[test]
    fn a_failing_locator_lists_nothing() {
        let mut w = world();
        w.locate_fails = true;
        assert!(ReadStatus::new(w.ports()).run(Path::new("/r")).is_empty());
    }
}
