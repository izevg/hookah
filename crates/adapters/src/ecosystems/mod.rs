//! Ecosystem adapters.
//!
//! [`Ecosystems`] is the registry: it sends each [`ToolchainDetector`] call to the module of the
//! project's ecosystem. Each module implements toolchain detection, tool-output parsing into
//! findings, and related-test selection. Rust and Node are implemented; Python is planned and
//! currently yields an empty toolchain and no test selection.

pub mod node;
pub mod python;
pub mod rust;

use std::path::{Path, PathBuf};

use hookah_application::error::PortError;
use hookah_application::ports::{ProcessOutput, ToolchainDetector};
use hookah_domain::finding::Finding;
use hookah_domain::project::{Ecosystem, Project};
use hookah_domain::session::{ChangedLines, FileRole};
use hookah_domain::toolchain::{MutationSelection, TestSelection, Tool, Toolchain};

use node::NodeEcosystem;
use rust::RustEcosystem;

/// Registry of all ecosystem detectors.
#[derive(Debug, Default)]
pub struct Ecosystems {
    node: NodeEcosystem,
    rust: RustEcosystem,
}

impl Ecosystems {
    /// Creates the registry.
    pub fn new() -> Self {
        Self::default()
    }
}

impl ToolchainDetector for Ecosystems {
    fn detect(&self, project: &Project) -> Result<Toolchain, PortError> {
        match project.ecosystem {
            Ecosystem::Rust => self.rust.detect(project),
            Ecosystem::Node => self.node.detect(project),
            Ecosystem::Python => Ok(Toolchain::default()),
        }
    }

    fn classify(&self, project: &Project, file: &Path, changed: &ChangedLines) -> FileRole {
        match project.ecosystem {
            Ecosystem::Rust => self.rust.classify(project, file, changed),
            Ecosystem::Node => self.node.classify(project, file, changed),
            Ecosystem::Python => FileRole::Source,
        }
    }

    fn select_mutants(
        &self,
        project: &Project,
        changed_tests: &[PathBuf],
        changed_sources: &[PathBuf],
    ) -> MutationSelection {
        match project.ecosystem {
            Ecosystem::Rust => self
                .rust
                .select_mutants(project, changed_tests, changed_sources),
            Ecosystem::Node => self
                .node
                .select_mutants(project, changed_tests, changed_sources),
            Ecosystem::Python => MutationSelection::None,
        }
    }

    fn select_tests(
        &self,
        project: &Project,
        changed: &[PathBuf],
    ) -> Result<TestSelection, PortError> {
        match project.ecosystem {
            Ecosystem::Rust => self.rust.select_tests(project, changed),
            Ecosystem::Node => self.node.select_tests(project, changed),
            Ecosystem::Python => Ok(TestSelection::None),
        }
    }

    fn parse(
        &self,
        project: &Project,
        tool: &Tool,
        output: &ProcessOutput,
    ) -> Result<Vec<Finding>, PortError> {
        match project.ecosystem {
            Ecosystem::Rust => self.rust.parse(project, tool, output),
            Ecosystem::Node => self.node.parse(project, tool, output),
            Ecosystem::Python => Ok(vec![]),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn planned_ecosystems_are_empty() {
        let reg = Ecosystems::new();
        let p = Project {
            root: "/r".into(),
            ecosystem: Ecosystem::Python,
            manifests: vec![],
        };
        assert!(reg.detect(&p).unwrap().is_empty());
        assert_eq!(
            reg.select_tests(&p, &["/r/a".into()]).unwrap(),
            TestSelection::None
        );
    }
}
