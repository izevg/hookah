//! Discovery ports.
//!
//! - `ProjectLocator`: walk up from a file and return the candidate manifests on the way.
//! - `ToolchainDetector`: given a project, return its configured toolchain (and the tools it
//!   lacks), classify files as source or test, select related tests and mutation targets for
//!   changed files, and parse tool output. The ecosystem adapter owns both building commands
//!   and reading their output, so the two never drift apart.
//! - `ChangeSource`: what changed, as reported by version control.

use std::path::{Path, PathBuf};

use hookah_domain::finding::Finding;
use hookah_domain::project::{Manifest, Project};
use hookah_domain::session::{ChangedLines, FileRole};
use hookah_domain::toolchain::{MutationSelection, TestSelection, Tool, Toolchain};

use super::execution::ProcessOutput;
use crate::error::PortError;

/// Finds the manifests that could define the project of a file.
pub trait ProjectLocator {
    /// All recognized manifests in the directories from `file`'s directory up to the boundary
    /// (filesystem or repository root). The domain rule picks the nearest.
    fn manifests_above(&self, file: &Path) -> Result<Vec<Manifest>, PortError>;
}

/// Knows the tools of one or more ecosystems.
pub trait ToolchainDetector {
    /// The tools the project configures. Commands use the project root as `cwd`. Tools the
    /// project should have but lacks go to [`Toolchain::missing`]; nothing is ever installed.
    fn detect(&self, project: &Project) -> Result<Toolchain, PortError>;

    /// Whether `file` (absolute, inside the project) is production code or a test, given its
    /// `changed` lines. The lines let an adapter treat an edit inside a trailing Rust
    /// `#[cfg(test)]` module as a test edit. Default: everything is [`FileRole::Source`].
    fn classify(&self, project: &Project, file: &Path, changed: &ChangedLines) -> FileRole {
        let _ = (project, file, changed);
        FileRole::Source
    }

    /// Chooses what to mutate. The stop phase calls this only when at least one changed file is a
    /// test. `changed_tests` and `changed_sources` are absolute paths inside the project; the
    /// adapter maps tests to the sources they cover and returns tool-specific targets. Must
    /// return [`MutationSelection::None`] when nothing is related. Default: `None`.
    fn select_mutants(
        &self,
        project: &Project,
        changed_tests: &[PathBuf],
        changed_sources: &[PathBuf],
    ) -> MutationSelection {
        let _ = (project, changed_tests, changed_sources);
        MutationSelection::None
    }

    /// Chooses the tests related to `changed` (absolute paths inside the project). Must return
    /// [`TestSelection::None`], never the whole suite, when nothing related is found.
    fn select_tests(
        &self,
        project: &Project,
        changed: &[PathBuf],
    ) -> Result<TestSelection, PortError>;

    /// Turns the output of `tool` into findings whose paths are relative to the project root.
    /// Returns an error when the output cannot be understood (for example, the tool crashed).
    fn parse(
        &self,
        project: &Project,
        tool: &Tool,
        output: &ProcessOutput,
    ) -> Result<Vec<Finding>, PortError>;
}

/// Reports what changed since the last commit, including untracked files.
pub trait ChangeSource {
    /// Changed lines of `file` (absolute) in the project at `project_root`.
    fn changed_lines(&self, project_root: &Path, file: &Path) -> Result<ChangedLines, PortError>;

    /// Absolute paths of all files changed versus `HEAD`, plus untracked files, under `root`.
    fn changed_files(&self, root: &Path) -> Result<Vec<PathBuf>, PortError>;
}
