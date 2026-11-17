//! Ecosystem adapters.
//!
//! [`Ecosystems`] is the registry: it sends each [`ToolchainDetector`] call to the module of the
//! project's ecosystem. Each module implements toolchain detection, tool-output parsing into
//! findings, and related-test selection. Rust is implemented; Node and Python are planned and
//! currently yield an empty toolchain and no test selection.

pub mod node;
pub mod python;
pub mod rust;

use std::path::PathBuf;

use hookah_application::error::PortError;
use hookah_application::ports::{ProcessOutput, ToolchainDetector};
use hookah_domain::finding::Finding;
use hookah_domain::project::{Ecosystem, Project};
use hookah_domain::toolchain::{TestSelection, Tool, Toolchain};

use rust::RustEcosystem;

/// Registry of all ecosystem detectors.
#[derive(Debug, Default)]
pub struct Ecosystems {
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
            Ecosystem::Node | Ecosystem::Python => Ok(Toolchain::default()),
        }
    }

    fn select_tests(
        &self,
        project: &Project,
        changed: &[PathBuf],
    ) -> Result<TestSelection, PortError> {
        match project.ecosystem {
            Ecosystem::Rust => self.rust.select_tests(project, changed),
            Ecosystem::Node | Ecosystem::Python => Ok(TestSelection::None),
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
            Ecosystem::Node | Ecosystem::Python => Ok(vec![]),
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
        for ecosystem in [Ecosystem::Node, Ecosystem::Python] {
            let p = Project {
                root: "/r".into(),
                ecosystem,
                manifests: vec![],
            };
            assert!(reg.detect(&p).unwrap().is_empty());
            assert_eq!(
                reg.select_tests(&p, &["/r/a".into()]).unwrap(),
                TestSelection::None
            );
        }
    }
}
