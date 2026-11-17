//! Node (TypeScript / JavaScript) ecosystem adapter.
//!
//! [`NodeEcosystem`] implements [`ToolchainDetector`] for projects with a `package.json`. It runs
//! the project's own tools straight from `node_modules/.bin`: Biome or Prettier to format, Biome
//! and ESLint to lint, `tsc` to typecheck, dependency-cruiser for architecture rules, Vitest or
//! Jest for the tests related to a change, and Stryker for mutation testing. Detection reads only
//! the manifest, lockfile, and config files, so it is cheap enough to run on every edit; see
//! [`detect`] for the rules and [`parse`] for the output formats.

pub mod detect;
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

    fn parse(
        &self,
        project: &Project,
        tool: &Tool,
        output: &ProcessOutput,
    ) -> Result<Vec<Finding>, PortError> {
        parse::parse(project, tool, output)
    }
}
