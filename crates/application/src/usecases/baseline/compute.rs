//! `ComputeBaseline`: the background job.
//!
//! Runs every Lint, Typecheck, Architecture, and Unused tool of the project over the whole project (the
//! detector says which targets make a tool cover it) and stores the fingerprints of what they
//! find. Format never runs: it rewrites files. Test and Mutation do not run: they are not
//! baseline-filtered. A baseline missing the findings of a tool that failed would make old
//! problems look new, so any failure marks the whole baseline failed and the phases fall back to
//! the changed-lines filter. A project without any such tool gets no baseline either.

use std::path::Path;

use hookah_domain::finding::baseline::Baseline;
use hookah_domain::project::locate_project;
use hookah_domain::toolchain::CheckKind;

use super::probe;
use crate::error::PortError;
use crate::ports::Ports;
use crate::usecases::request::RunLog;

/// The kinds a baseline covers, in the order they run.
const KINDS: [CheckKind; 4] = [
    CheckKind::Lint,
    CheckKind::Typecheck,
    CheckKind::Architecture,
    CheckKind::Unused,
];

/// Computes and stores the baseline of one project for one session.
pub struct ComputeBaseline<'a> {
    ports: Ports<'a>,
}

impl<'a> ComputeBaseline<'a> {
    /// Creates the use case.
    pub fn new(ports: Ports<'a>) -> Self {
        Self { ports }
    }

    /// Computes the baseline of the project at `root`. On failure the store records it and the
    /// error is returned too.
    pub fn run(&self, root: &Path, session: &str) -> Result<(), PortError> {
        match self.compute(root) {
            Ok(baseline) => self.ports.baselines.save(root, session, &baseline),
            Err(reason) => {
                self.ports.baselines.fail(root, session, &reason)?;
                Err(PortError::new(reason))
            }
        }
    }

    fn compute(&self, root: &Path) -> Result<Baseline, String> {
        let file = probe(root);
        let manifests = self
            .ports
            .locator
            .manifests_above(&file)
            .map_err(|e| format!("locating project: {e}"))?;
        let project = locate_project(&file, &manifests)
            .ok_or_else(|| format!("no project at {}", root.display()))?;
        let toolchain = self
            .ports
            .detector
            .detect(&project)
            .map_err(|e| format!("detecting toolchain: {e}"))?;
        let mut log = RunLog::default();
        let mut ran = false;
        for kind in KINDS {
            for tool in toolchain.of_kind(kind) {
                ran = true;
                let targets = self.ports.detector.baseline_targets(&project, tool);
                log.run_tool(&self.ports, &project, tool, &targets);
            }
        }
        // Nothing to compare against would make every later finding look new.
        if !ran {
            return Err("the project has no static check tools".into());
        }
        if !log.failures().is_empty() {
            return Err(log.failures().join("; "));
        }
        Ok(Baseline::from_findings(&log.findings))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use hookah_domain::finding::fingerprint::Fingerprint;
    use hookah_domain::finding::{Finding, Location, Severity};

    use super::*;
    use crate::ports::BaselineState;
    use crate::ports::fakes::{World, tool};

    fn world() -> World {
        let mut w = World::new(
            "/r/Cargo.toml",
            vec![
                tool("fmt", CheckKind::Format),
                tool("lint", CheckKind::Lint),
                tool("types", CheckKind::Typecheck),
                tool("arch", CheckKind::Architecture),
                tool("unused", CheckKind::Unused),
                tool("test", CheckKind::Test),
                tool("mut", CheckKind::Mutation),
            ],
        );
        let f = |kind, line| Finding::new(kind, Severity::Error, Location::line("a.rs", line), "m");
        w.findings.insert(
            "lint".into(),
            vec![f(CheckKind::Lint, 1), f(CheckKind::Lint, 2)],
        );
        w.findings
            .insert("types".into(), vec![f(CheckKind::Typecheck, 3)]);
        w.findings
            .insert("test".into(), vec![f(CheckKind::Test, 4)]);
        w
    }

    fn run(w: &World) -> Result<(), PortError> {
        ComputeBaseline::new(w.ports()).run("/r".as_ref(), "s")
    }

    #[test]
    fn runs_the_static_tools_over_the_whole_project_in_order() {
        let w = world();
        run(&w).unwrap();
        assert_eq!(w.calls(), ["lint .", "types .", "arch .", "unused ."]);
    }

    #[test]
    fn stores_the_fingerprint_counts_of_static_findings() {
        let w = world();
        run(&w).unwrap();
        let BaselineState::Ready(base) = w.baseline_state("s") else {
            panic!("expected a ready baseline");
        };
        let lint = Fingerprint::from_stored("lint|a.rs||m");
        let types = Fingerprint::from_stored("typecheck|a.rs||m");
        assert_eq!(base.count(&lint), 2);
        assert_eq!(base.count(&types), 1);
        assert_eq!(base.counts().count(), 2);
    }

    #[test]
    fn any_tool_failure_fails_the_baseline() {
        for failing in ["lint", "types"] {
            let mut w = world();
            w.run_fails = vec![failing.into()];
            let err = run(&w).unwrap_err();
            assert!(err.message().contains(failing));
            assert!(
                matches!(w.baseline_state("s"), BaselineState::Failed(r) if r.contains(failing))
            );
        }
        let mut w = world();
        w.timeouts = vec!["arch".into()];
        assert!(run(&w).is_err());
        assert!(matches!(w.baseline_state("s"), BaselineState::Failed(_)));
    }

    #[test]
    fn a_missing_project_or_toolchain_fails_the_baseline() {
        let mut w = world();
        w.manifests = vec![];
        assert!(run(&w).unwrap_err().message().contains("no project"));
        assert!(matches!(w.baseline_state("s"), BaselineState::Failed(_)));
        let mut w = world();
        w.detect_fails = true;
        assert!(run(&w).unwrap_err().message().contains("detecting"));
        let mut w = world();
        w.locate_fails = true;
        assert!(run(&w).unwrap_err().message().contains("locating"));
    }

    #[test]
    fn a_project_without_static_tools_has_no_baseline() {
        let mut w = world();
        w.toolchain = hookah_domain::toolchain::Toolchain::new(vec![
            tool("fmt", CheckKind::Format),
            tool("test", CheckKind::Test),
        ]);
        let err = run(&w).unwrap_err();
        assert!(err.message().contains("no static check tools"));
        assert!(matches!(w.baseline_state("s"), BaselineState::Failed(_)));
        assert!(w.calls().is_empty());
    }
}
