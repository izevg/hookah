//! Rust ecosystem adapter.
//!
//! [`RustEcosystem`] implements [`ToolchainDetector`] for Cargo projects: `rustfmt` for format,
//! `clippy` for lint, and `cargo test` for tests, and `cargo-mutants` (when installed) for mutation testing, all scoped to the package that owns the
//! project's `Cargo.toml`. `cargo metadata` results are cached per project root for the lifetime
//! of the adapter, so one hook call runs it once.

pub mod detect;
pub mod mutants;
pub mod parse;
pub mod select;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use hookah_application::error::PortError;
use hookah_application::ports::{ProcessOutput, ToolchainDetector};
use hookah_domain::finding::Finding;
use hookah_domain::project::Project;
use hookah_domain::session::{ChangedLines, FileRole};
use hookah_domain::toolchain::{MutationSelection, TestSelection, Tool, Toolchain};

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

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

impl ToolchainDetector for RustEcosystem {
    fn detect(&self, project: &Project) -> Result<Toolchain, PortError> {
        let Some(info) = self.info(&project.root)? else {
            return Ok(Toolchain::default());
        };
        let mut tools = detect::toolchain(&info, &project.root).tools().to_vec();
        let mut missing = vec![];
        if mutants::available(&self.runner, &project.root) {
            tools.push(mutants::tool(&info, &project.root, mutants::jobs()));
        } else if mutants::has_tests(&project.root) {
            missing.push(mutants::missing());
        }
        Ok(Toolchain::new(tools).with_missing(missing))
    }

    fn classify(&self, project: &Project, file: &Path, changed: &ChangedLines) -> FileRole {
        mutants::classify::classify(&project.root, file, changed)
    }

    fn select_mutants(
        &self,
        project: &Project,
        changed_tests: &[PathBuf],
        changed_sources: &[PathBuf],
    ) -> MutationSelection {
        let Ok(Some(info)) = self.info(&project.root) else {
            return MutationSelection::None;
        };
        let root = canonical(&project.root);
        let crate_dir = root
            .strip_prefix(&info.workspace_root)
            .unwrap_or(Path::new(""));
        let targets = mutants::select::select(
            crate_dir,
            &project.root,
            changed_tests,
            changed_sources,
            &|rel| project.root.join(rel).is_file(),
        );
        if !targets.is_empty() {
            // `cargo-mutants` creates its output directory but not the parent. Best effort:
            // on failure the tool itself reports the problem.
            let _ = std::fs::create_dir_all(info.workspace_root.join("target"));
        }
        MutationSelection::from_targets(targets)
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
            detect::CLIPPY | detect::CARGO_TEST | mutants::CARGO_MUTANTS => {
                let workspace_root = self
                    .info(&project.root)?
                    .map_or_else(|| project_root.clone(), |i| i.workspace_root);
                let paths = Paths::new(project_root, workspace_root.clone());
                match tool.name.as_str() {
                    detect::CLIPPY => parse::clippy(&paths, output),
                    mutants::CARGO_MUTANTS => {
                        let file = mutants::output_dir(&workspace_root)
                            .join("mutants.out")
                            .join("outcomes.json");
                        let json = std::fs::read_to_string(file).ok();
                        mutants::outcomes::parse(&paths, output, json.as_deref())
                    }
                    _ => parse::cargo_test(&paths, output),
                }
            }
            other => Err(PortError::new(format!("unknown Rust tool `{other}`"))),
        }
    }
}
