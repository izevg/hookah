//! Python ecosystem adapter.
//!
//! [`PythonEcosystem`] implements [`ToolchainDetector`] for projects with a `pyproject.toml`:
//! `ruff` (or `black`) for format, `ruff` for lint, `mypy` for typecheck, `import-linter` for
//! architecture, `pytest` for tests, and `mutmut` for mutation testing. Tools run from the
//! project's own environment (a virtual environment, `poetry run`, or `PATH`); nothing is ever
//! installed. The details are in [`detect`], [`parse`], and [`select`], and in
//! `docs/ecosystems.md`.
//!
//! The adapter keeps no state: the environment and configuration are cheap to read from disk, so
//! every call sees the project as it is now.

pub mod detect;
mod fixture;
pub mod layout;
pub mod parse;
pub mod select;

#[cfg(test)]
mod e2e;

use std::path::{Path, PathBuf};

use hookah_application::error::PortError;
use hookah_application::ports::{ProcessOutput, ToolchainDetector};
use hookah_domain::finding::Finding;
use hookah_domain::project::Project;
use hookah_domain::session::{ChangedLines, FileRole};
use hookah_domain::toolchain::{MutationSelection, TestSelection, Tool, Toolchain};

use detect::Setup;

/// Detector for Python projects.
#[derive(Debug, Default)]
pub struct PythonEcosystem;

impl PythonEcosystem {
    /// Creates the detector.
    pub fn new() -> Self {
        Self
    }
}

impl ToolchainDetector for PythonEcosystem {
    fn detect(&self, project: &Project) -> Result<Toolchain, PortError> {
        let setup = Setup::load(&project.root);
        Ok(detect::toolchain(&setup, &project.root))
    }

    fn classify(&self, project: &Project, file: &Path, _changed: &ChangedLines) -> FileRole {
        select::classify(project, file)
    }

    fn select_mutants(
        &self,
        project: &Project,
        changed_tests: &[PathBuf],
        changed_sources: &[PathBuf],
    ) -> MutationSelection {
        select::mutants::select(project, changed_tests, changed_sources)
    }

    fn select_tests(
        &self,
        project: &Project,
        changed: &[PathBuf],
    ) -> Result<TestSelection, PortError> {
        let testmon = Setup::load(&project.root).testmon();
        Ok(select::tests(project, testmon, changed))
    }

    fn parse(
        &self,
        project: &Project,
        tool: &Tool,
        output: &ProcessOutput,
    ) -> Result<Vec<Finding>, PortError> {
        parse::parse(project, tool, output)
    }
}
