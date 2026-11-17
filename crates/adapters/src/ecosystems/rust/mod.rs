//! Rust ecosystem adapter.
//!
//! [`RustEcosystem`] implements [`ToolchainDetector`] for Cargo projects: `rustfmt` for format,
//! `clippy` for lint, and `cargo test` for tests, all scoped to the package that owns the
//! project's `Cargo.toml`. `cargo metadata` results are cached per project root for the lifetime
//! of the adapter, so one hook call runs it once.

pub mod detect;
pub mod parse;
pub mod select;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use hookah_application::error::PortError;
use hookah_application::ports::{ProcessOutput, ToolchainDetector};
use hookah_domain::finding::Finding;
use hookah_domain::project::Project;
use hookah_domain::toolchain::{TestSelection, Tool, Toolchain};

use crate::process::ProcessRunner;
use detect::PackageInfo;
use parse::Paths;

/// Detector for Cargo projects.
#[derive(Debug, Default)]
pub struct RustEcosystem {
    runner: ProcessRunner,
    cache: Mutex<HashMap<PathBuf, Option<PackageInfo>>>,
}

impl RustEcosystem {
    /// Creates the detector.
    pub fn new() -> Self {
        Self::default()
    }

    fn info(&self, root: &Path) -> Result<Option<PackageInfo>, PortError> {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(hit) = cache.get(root) {
            return Ok(hit.clone());
        }
        let info = detect::package_info(&self.runner, root)?;
        cache.insert(root.to_path_buf(), info.clone());
        Ok(info)
    }
}

impl ToolchainDetector for RustEcosystem {
    fn detect(&self, project: &Project) -> Result<Toolchain, PortError> {
        Ok(match self.info(&project.root)? {
            Some(info) => detect::toolchain(&info, &project.root),
            None => Toolchain::default(),
        })
    }

    fn select_tests(
        &self,
        project: &Project,
        changed: &[PathBuf],
    ) -> Result<TestSelection, PortError> {
        Ok(match self.info(&project.root)? {
            Some(info) => select::select(&info, project, changed),
            None => TestSelection::None,
        })
    }

    fn parse(
        &self,
        project: &Project,
        tool: &Tool,
        output: &ProcessOutput,
    ) -> Result<Vec<Finding>, PortError> {
        let project_root = project
            .root
            .canonicalize()
            .unwrap_or_else(|_| project.root.clone());
        match tool.name.as_str() {
            detect::RUSTFMT => parse::rustfmt(&project_root, output),
            detect::CLIPPY | detect::CARGO_TEST => {
                let workspace_root = self
                    .info(&project.root)?
                    .map_or_else(|| project_root.clone(), |i| i.workspace_root);
                let paths = Paths::new(project_root, workspace_root);
                if tool.name == detect::CLIPPY {
                    parse::clippy(&paths, output)
                } else {
                    parse::cargo_test(&paths, output)
                }
            }
            other => Err(PortError::new(format!("unknown Rust tool `{other}`"))),
        }
    }
}
