//! Ecosystem adapters.
//!
//! [`Ecosystems`] is the registry: it sends each [`ToolchainDetector`] call to the module of the
//! project's ecosystem. Each module implements toolchain detection, tool-output parsing into
//! findings, and related-test selection. Rust, Node, and Python are implemented.

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
use python::PythonEcosystem;
use rust::RustEcosystem;

/// Registry of all ecosystem detectors.
#[derive(Debug, Default)]
pub struct Ecosystems {
    node: NodeEcosystem,
    python: PythonEcosystem,
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
            Ecosystem::Python => self.python.detect(project),
        }
    }

    fn classify(&self, project: &Project, file: &Path, changed: &ChangedLines) -> FileRole {
        match project.ecosystem {
            Ecosystem::Rust => self.rust.classify(project, file, changed),
            Ecosystem::Node => self.node.classify(project, file, changed),
            Ecosystem::Python => self.python.classify(project, file, changed),
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
            Ecosystem::Python => {
                self.python
                    .select_mutants(project, changed_tests, changed_sources)
            }
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
            Ecosystem::Python => self.python.select_tests(project, changed),
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
            Ecosystem::Python => self.python.parse(project, tool, output),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn project(ecosystem: Ecosystem) -> Project {
        Project {
            root: "/r".into(),
            ecosystem,
            manifests: vec![],
        }
    }

    #[test]
    fn python_projects_go_to_the_python_adapter() {
        let reg = Ecosystems::new();
        let p = project(Ecosystem::Python);
        let changed = ChangedLines::All;
        assert_eq!(
            reg.classify(&p, Path::new("/r/tests/test_a.py"), &changed),
            FileRole::Test
        );
        assert_eq!(
            reg.classify(&p, Path::new("/r/pkg/a.py"), &changed),
            FileRole::Source
        );
    }
}
