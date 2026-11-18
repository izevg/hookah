//! Node (TypeScript / JavaScript) ecosystem adapter.
//!
//! [`NodeEcosystem`] implements [`ToolchainDetector`] for projects with a `package.json`. It runs
//! the project's own tools straight from `node_modules/.bin`: Biome or Prettier to format, Biome
//! and ESLint to lint, `tsc` to typecheck, dependency-cruiser for architecture rules, Knip for
//! unused files, exports, and dependencies (stop phase only), Vitest or Jest for the tests related
//! to a change, and Stryker for mutation testing. Detection reads only the manifest, lockfile, and
//! config files, so it is cheap enough to run on every edit; see [`detect`] for the rules and
//! [`parse`] for the output formats.

pub mod detect;
pub mod heavy;
pub mod parse;
pub mod select;

use std::path::{Path, PathBuf};

use hookah_application::error::PortError;
use hookah_application::ports::{ProcessOutput, ToolchainDetector};
use hookah_domain::finding::Finding;
use hookah_domain::project::Project;
use hookah_domain::session::{ChangedLines, FileRole};
use hookah_domain::toolchain::{MutationSelection, TestSelection, Tool, Toolchain};

use crate::store::cache_dir::project_cache_dir;
use detect::package::PackageInfo;

/// Detector for Node projects.
#[derive(Debug, Default)]
pub struct NodeEcosystem;

impl NodeEcosystem {
    /// Creates the detector.
    pub fn new() -> Self {
        Self
    }
}

impl ToolchainDetector for NodeEcosystem {
    fn detect(&self, project: &Project) -> Result<Toolchain, PortError> {
        let info = PackageInfo::read(&project.root)?;
        // The cache directory is created on demand and only when a tool wants to keep state in
        // it; failing to create it drops the incremental state, not the check.
        Ok(detect::toolchain(&info, || {
            project_cache_dir(&project.root).ok()
        }))
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
        select::mutants(project, changed_tests, changed_sources)
    }

    fn select_tests(
        &self,
        project: &Project,
        changed: &[PathBuf],
    ) -> Result<TestSelection, PortError> {
        let info = PackageInfo::read(&project.root)?;
        Ok(select::related_tests(
            project,
            changed,
            detect::has_test_runner(&info),
        ))
    }

    fn specialize(&self, project: &Project, tool: &Tool, targets: &[String]) -> Vec<Tool> {
        detect::typecheck::specialize(&project.root, tool, targets, || {
            project_cache_dir(&project.root).ok()
        })
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use hookah_domain::toolchain::{CheckKind, CommandSpec};

    use super::*;

    #[test]
    fn a_tool_that_is_not_tsc_is_kept_as_it_is() {
        let tool = Tool {
            name: "eslint".into(),
            kind: CheckKind::Lint,
            command: CommandSpec::new("eslint", vec![], "/r", Duration::from_secs(1)),
        };
        let project = Project {
            root: "/r".into(),
            ecosystem: hookah_domain::project::Ecosystem::Node,
            manifests: vec![],
        };
        let out = NodeEcosystem::new().specialize(&project, &tool, &["a.ts".into()]);
        assert_eq!(out, [tool]);
    }
}
