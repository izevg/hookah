//! `StartSession`: the session-start event.
//!
//! Finds the project of the agent's working directory, plus the projects nested below it (a
//! monorepo root holds none itself), and starts their baseline jobs in the background. A project
//! the agent edits later without having one gets its baseline lazily, on its first edit.

use std::path::{Path, PathBuf};

use hookah_domain::project::{Project, locate_project};

use super::{ensure, probe};
use crate::ports::Ports;
use crate::usecases::request::{Outcome, StartRequest};

/// How many levels below the working directory nested projects are looked for.
const NESTED_DEPTH: usize = 3;
/// At most this many projects get a baseline job at session start.
const MAX_PROJECTS: usize = 8;

/// Starts the baseline of the session's project.
pub struct StartSession<'a> {
    ports: Ports<'a>,
    allow_dir: Option<&'a dyn Fn(&Path) -> bool>,
}

impl<'a> StartSession<'a> {
    /// Creates the use case.
    pub fn new(ports: Ports<'a>) -> Self {
        Self {
            ports,
            allow_dir: None,
        }
    }

    /// Limits baseline jobs to discovered project roots that satisfy `allow`.
    #[must_use]
    pub fn with_dir_filter(mut self, allow: &'a dyn Fn(&Path) -> bool) -> Self {
        self.allow_dir = Some(allow);
        self
    }

    fn allows(&self, dir: &Path) -> bool {
        self.allow_dir.is_none_or(|allow| allow(dir))
    }

    /// Never fails: problems become a degraded outcome.
    pub fn run(&self, request: &StartRequest) -> Outcome {
        let mut roots: Vec<PathBuf> = Vec::new();
        let mut problem = None;
        let mut dirs = vec![request.cwd.clone()];
        match self.ports.locator.nested_roots(&request.cwd, NESTED_DEPTH) {
            Ok(nested) => dirs.extend(nested),
            Err(e) if self.allows(&request.cwd) => {
                problem = Some(format!("looking for nested projects: {e}"));
            }
            Err(_) => {}
        }
        for dir in dirs {
            match self.project_at(&dir) {
                Ok(Some(project))
                    if self.allows(&project.root) && !roots.contains(&project.root) =>
                {
                    roots.push(project.root);
                }
                Ok(_) => {}
                Err(reason) if self.allows(&dir) => problem = problem.or(Some(reason)),
                Err(_) => {}
            }
        }
        roots.truncate(MAX_PROJECTS);
        for root in &roots {
            if let Err(reason) = ensure(&self.ports, root, &request.session) {
                problem = problem.or(Some(reason));
            }
        }
        match problem {
            Some(reason) => Outcome::degraded(reason),
            None => Outcome::pass(),
        }
    }

    fn project_at(&self, dir: &std::path::Path) -> Result<Option<Project>, String> {
        let file = probe(dir);
        let manifests = self
            .ports
            .locator
            .manifests_above(&file)
            .map_err(|e| format!("locating project: {e}"))?;
        Ok(locate_project(&file, &manifests))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::Path;

    use hookah_domain::project::Manifest;
    use hookah_domain::toolchain::CheckKind;
    use hookah_domain::verdict::Verdict;

    use super::*;
    use crate::ports::BaselineState;
    use crate::ports::fakes::{World, tool};

    fn world() -> World {
        World::new("/r/Cargo.toml", vec![tool("lint", CheckKind::Lint)])
    }

    fn run(w: &World, session: &str) -> Outcome {
        StartSession::new(w.ports()).run(&StartRequest::new("/r/crates/x", session))
    }

    #[test]
    fn claims_the_baseline_and_spawns_the_job_for_the_project_root() {
        let w = world();
        assert_eq!(run(&w, "s1"), Outcome::pass());
        assert_eq!(w.baseline_state("s1"), BaselineState::InProgress);
        assert_eq!(*w.spawned.borrow(), [("/r".into(), "s1".to_string())]);
    }

    #[test]
    fn a_second_start_of_the_same_session_does_not_spawn_again() {
        let w = world();
        run(&w, "s1");
        assert_eq!(run(&w, "s1"), Outcome::pass());
        assert_eq!(w.spawned.borrow().len(), 1);
        run(&w, "s2");
        assert_eq!(w.spawned.borrow().len(), 2);
    }

    #[test]
    fn outside_any_project_nothing_happens() {
        let mut w = world();
        w.manifests = vec![];
        assert_eq!(run(&w, "s1"), Outcome::pass());
        assert!(w.spawned.borrow().is_empty());
        assert_eq!(w.baseline_state("s1"), BaselineState::Absent);
    }

    #[test]
    fn a_spawn_failure_is_recorded_and_degrades() {
        let mut w = world();
        w.spawn_fails = true;
        let Verdict::Degraded(reason) = run(&w, "s1").verdict else {
            panic!("expected degraded");
        };
        assert!(reason.contains("cannot spawn"));
        assert!(matches!(w.baseline_state("s1"), BaselineState::Failed(_)));
    }

    #[test]
    fn store_and_locator_failures_degrade() {
        let mut w = world();
        w.begin_fails = true;
        assert!(matches!(run(&w, "s").verdict, Verdict::Degraded(r) if r.contains("store")));
        let mut w = world();
        w.locate_fails = true;
        assert!(matches!(run(&w, "s").verdict, Verdict::Degraded(r) if r.contains("locating")));
    }

    fn monorepo() -> World {
        let mut w = world();
        w.manifests = ["/r", "/r/a", "/r/b", "/r/c"]
            .iter()
            .filter(|d| **d != "/r")
            .map(|d| Manifest::from_path(&PathBuf::from(format!("{d}/Cargo.toml"))).unwrap())
            .collect();
        w.nested = vec!["/r/a".into(), "/r/b".into(), "/r/c".into()];
        w
    }

    #[test]
    fn a_monorepo_root_starts_a_job_per_nested_project() {
        let w = monorepo();
        assert_eq!(run(&w, "s"), Outcome::pass());
        let roots: Vec<_> = w.spawned.borrow().iter().map(|(r, _)| r.clone()).collect();
        assert_eq!(roots, [PathBuf::from("/r/a"), "/r/b".into(), "/r/c".into()]);
    }

    #[test]
    fn the_working_directory_project_and_nested_ones_are_not_started_twice() {
        let mut w = monorepo();
        w.manifests
            .push(Manifest::from_path(Path::new("/r/Cargo.toml")).unwrap());
        w.nested.push("/r/a".into());
        let outcome = StartSession::new(w.ports()).run(&StartRequest::new("/r", "s"));
        assert_eq!(outcome, Outcome::pass());
        let roots: Vec<_> = w.spawned.borrow().iter().map(|(r, _)| r.clone()).collect();
        assert_eq!(roots.len(), 4);
        assert_eq!(roots[0], PathBuf::from("/r"));
    }

    #[test]
    fn at_most_eight_projects_get_a_job() {
        let mut w = world();
        w.manifests.clear();
        for i in 0..12 {
            let dir = format!("/r/p{i}");
            w.manifests
                .push(Manifest::from_path(&PathBuf::from(format!("{dir}/Cargo.toml"))).unwrap());
            w.nested.push(dir.into());
        }
        let outcome = StartSession::new(w.ports()).run(&StartRequest::new("/r", "s"));
        assert_eq!(outcome, Outcome::pass());
        assert_eq!(w.spawned.borrow().len(), 8);
    }

    #[test]
    fn a_nested_search_failure_still_baselines_the_rest_and_degrades() {
        struct Failing(World);
        let w = world();
        // `nested_roots` fails through a locator wrapper.
        impl crate::ports::ProjectLocator for Failing {
            fn manifests_above(&self, f: &Path) -> Result<Vec<Manifest>, crate::PortError> {
                self.0.manifests_above(f)
            }
            fn nested_roots(&self, _: &Path, _: usize) -> Result<Vec<PathBuf>, crate::PortError> {
                Err(crate::PortError::new("walk failed"))
            }
        }
        let failing = Failing(world());
        let mut ports = w.ports();
        ports.locator = &failing;
        let outcome = StartSession::new(ports).run(&StartRequest::new("/r", "s"));
        assert!(matches!(outcome.verdict, Verdict::Degraded(r) if r.contains("nested")));
        assert_eq!(w.spawned.borrow().len(), 1);
    }

    #[test]
    fn repeated_directories_do_not_use_up_the_project_cap() {
        let mut w = world();
        w.manifests = ["/r/p0", "/r/p1"]
            .iter()
            .map(|d| Manifest::from_path(&PathBuf::from(format!("{d}/Cargo.toml"))).unwrap())
            .collect();
        w.nested = std::iter::repeat_n(PathBuf::from("/r/p0"), 8)
            .chain([PathBuf::from("/r/p1")])
            .collect();
        StartSession::new(w.ports()).run(&StartRequest::new("/r", "s"));
        let roots: Vec<_> = w.spawned.borrow().iter().map(|(r, _)| r.clone()).collect();
        assert_eq!(roots, [PathBuf::from("/r/p0"), "/r/p1".into()]);
    }
}
