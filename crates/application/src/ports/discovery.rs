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

    /// Directories below `dir` (not `dir` itself), at most `max_depth` levels down, that hold a
    /// recognized manifest, shallowest first. Skips hidden directories and the usual dependency
    /// and build output directories. A session in a monorepo root uses this to baseline the
    /// projects it will edit.
    fn nested_roots(&self, dir: &Path, max_depth: usize) -> Result<Vec<PathBuf>, PortError>;
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

    /// The targets that make `tool` (a lint, typecheck, or architecture tool) cover the whole
    /// project, for the session baseline. Tools whose command already names the whole project
    /// return none. Default: the project root, `.`.
    fn baseline_targets(&self, project: &Project, tool: &Tool) -> Vec<String> {
        let _ = (project, tool);
        vec![".".to_string()]
    }

    /// The commands that run `tool` for `targets` (relative to the project root, or `.` for the
    /// whole project), where `tool` came from [`ToolchainDetector::detect`]. Default: `tool`
    /// itself. An adapter overrides this when the right command depends on which files changed:
    /// a TypeScript project with references checks only the tsconfig that owns them. An empty
    /// answer means nothing applies, and the tool is skipped.
    fn specialize(&self, project: &Project, tool: &Tool, targets: &[String]) -> Vec<Tool> {
        let _ = (project, targets);
        vec![tool.clone()]
    }

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

    /// The number of lines of `file` (absolute): what [`ChangedLines::All`] means for a new or
    /// untracked file.
    fn line_count(&self, file: &Path) -> Result<usize, PortError>;

    /// Absolute paths of all files changed versus `HEAD`, plus untracked files, under `root`.
    fn changed_files(&self, root: &Path) -> Result<Vec<PathBuf>, PortError>;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use hookah_domain::project::Ecosystem;
    use hookah_domain::toolchain::{CheckKind, CommandSpec};

    use super::*;

    /// A detector that overrides nothing optional.
    struct Plain;

    impl ToolchainDetector for Plain {
        fn detect(&self, _: &Project) -> Result<Toolchain, PortError> {
            Ok(Toolchain::new(vec![]))
        }

        fn select_tests(&self, _: &Project, _: &[PathBuf]) -> Result<TestSelection, PortError> {
            Ok(TestSelection::None)
        }

        fn parse(
            &self,
            _: &Project,
            _: &Tool,
            _: &ProcessOutput,
        ) -> Result<Vec<Finding>, PortError> {
            Ok(vec![])
        }
    }

    #[test]
    fn a_tool_is_specialized_to_itself_by_default() {
        let tool = Tool {
            name: "t".into(),
            kind: CheckKind::Lint,
            command: CommandSpec::new("t", vec![], "/r", Duration::from_secs(1)),
        };
        let project = Project {
            root: "/r".into(),
            ecosystem: Ecosystem::Rust,
            manifests: vec![],
        };
        assert_eq!(Plain.specialize(&project, &tool, &["a".into()]), [tool]);
    }
}
