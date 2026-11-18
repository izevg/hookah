//! Ecosystem adapters.
//!
//! [`Ecosystems`] is the registry: it sends each [`ToolchainDetector`] call to the module of the
//! project's ecosystem. Each module implements toolchain detection, tool-output parsing into
//! findings, and related-test selection. Rust, Node, Python, and Java are implemented.
//!
//! Rust toolchain detection goes through the detection [`cache`]: the result is kept between hook
//! calls until one of the inputs that the Rust module declares changes.

mod baseline;
pub mod cache;
pub mod java;
pub mod node;
pub mod python;
pub mod rust;
#[cfg(test)]
mod testing;

use std::path::{Path, PathBuf};

use hookah_application::error::PortError;
use hookah_application::ports::{ProcessOutput, ToolchainDetector};
use hookah_domain::finding::Finding;
use hookah_domain::project::{Ecosystem, Project};
use hookah_domain::session::{ChangedLines, FileRole};
use hookah_domain::toolchain::{MutationSelection, TestSelection, Tool, Toolchain};

use cache::{CacheConfig, Origin, detect_cached};
use java::JavaEcosystem;
use node::NodeEcosystem;
use python::PythonEcosystem;
use rust::RustEcosystem;

/// Registry of all ecosystem detectors.
#[derive(Debug)]
pub struct Ecosystems {
    java: JavaEcosystem,
    node: NodeEcosystem,
    python: PythonEcosystem,
    rust: RustEcosystem,
    cache: CacheConfig,
}

impl Ecosystems {
    /// Creates the registry for the running process; the cache follows the environment (`HOOKAH_NO_CACHE`).
    pub fn from_env() -> Self {
        Self::with_cache(CacheConfig::from_env())
    }

    /// Creates the registry with an explicit cache configuration.
    pub fn with_cache(cache: CacheConfig) -> Self {
        Self {
            java: JavaEcosystem::new(),
            node: NodeEcosystem::new(),
            python: PythonEcosystem::new(),
            rust: RustEcosystem::with_cache(cache.clone()),
            cache,
        }
    }

    /// Like [`ToolchainDetector::detect`], and says whether the result came from the cache. Only
    /// Rust detection is cached (it runs `cargo`, about 40 ms); Node, Python, and Java detection
    /// reads a few small files and costs about as much as starting the process, so caching it
    /// would add risk for no gain. To cache
    /// another ecosystem, give it a `detection_inputs` and `derived_inputs` like Rust's and add an
    /// arm here.
    pub fn detect_traced(&self, project: &Project) -> Result<(Toolchain, Origin), PortError> {
        if project.ecosystem != Ecosystem::Rust || !self.cache.enabled() {
            return Ok((self.detect_uncached(project)?, Origin::Off));
        }
        detect_cached(
            &self.cache,
            project,
            &self.rust.detection_inputs(project),
            || {
                let toolchain = self.detect_uncached(project)?;
                Ok((toolchain, self.rust.derived_inputs(project)))
            },
        )
    }

    fn detect_uncached(&self, project: &Project) -> Result<Toolchain, PortError> {
        match project.ecosystem {
            Ecosystem::Java => self.java.detect(project),
            Ecosystem::Rust => self.rust.detect(project),
            Ecosystem::Node => self.node.detect(project),
            Ecosystem::Python => self.python.detect(project),
        }
    }
}

impl ToolchainDetector for Ecosystems {
    fn detect(&self, project: &Project) -> Result<Toolchain, PortError> {
        self.detect_traced(project).map(|(toolchain, _)| toolchain)
    }

    fn classify(&self, project: &Project, file: &Path, changed: &ChangedLines) -> FileRole {
        match project.ecosystem {
            Ecosystem::Java => self.java.classify(project, file, changed),
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
            Ecosystem::Java => self
                .java
                .select_mutants(project, changed_tests, changed_sources),
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
            Ecosystem::Java => self.java.select_tests(project, changed),
            Ecosystem::Rust => self.rust.select_tests(project, changed),
            Ecosystem::Node => self.node.select_tests(project, changed),
            Ecosystem::Python => self.python.select_tests(project, changed),
        }
    }

    fn baseline_targets(&self, project: &Project, tool: &Tool) -> Vec<String> {
        baseline::targets(project, tool)
    }

    fn specialize(&self, project: &Project, tool: &Tool, targets: &[String]) -> Vec<Tool> {
        match project.ecosystem {
            Ecosystem::Node => self.node.specialize(project, tool, targets),
            Ecosystem::Java | Ecosystem::Rust | Ecosystem::Python => vec![tool.clone()],
        }
    }

    fn parse(
        &self,
        project: &Project,
        tool: &Tool,
        output: &ProcessOutput,
    ) -> Result<Vec<Finding>, PortError> {
        match project.ecosystem {
            Ecosystem::Java => self.java.parse(project, tool, output),
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
    fn every_ecosystem_keeps_a_tool_it_does_not_specialize() {
        use hookah_domain::toolchain::{CheckKind, CommandSpec};
        use std::time::Duration;

        let tool = Tool {
            name: "x".into(),
            kind: CheckKind::Lint,
            command: CommandSpec::new("x", vec![], "/r", Duration::from_secs(1)),
        };
        let reg = Ecosystems::from_env();
        for ecosystem in [
            Ecosystem::Java,
            Ecosystem::Rust,
            Ecosystem::Node,
            Ecosystem::Python,
        ] {
            let out = reg.specialize(&project(ecosystem), &tool, &["a".into()]);
            assert_eq!(out, std::slice::from_ref(&tool), "{ecosystem:?}");
        }
    }

    #[test]
    fn baseline_targets_come_from_the_ecosystem_rules() {
        use hookah_domain::toolchain::{CheckKind, CommandSpec};
        use std::time::Duration;

        let tool = |name: &str| Tool {
            name: name.into(),
            kind: CheckKind::Lint,
            command: CommandSpec::new(name, vec![], "/r", Duration::from_secs(1)),
        };
        let reg = Ecosystems::from_env();
        let py = project(Ecosystem::Python);
        assert_eq!(reg.baseline_targets(&py, &tool("ruff")), ["."]);
        assert!(reg.baseline_targets(&py, &tool("mypy")).is_empty());
    }

    #[test]
    fn python_projects_go_to_the_python_adapter() {
        let reg = Ecosystems::from_env();
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

    #[test]
    fn node_python_and_java_detection_is_never_cached() {
        use crate::ecosystems::node::detect::package::scratch::Scratch;

        let dir = std::env::temp_dir().join(format!("hookah-reg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let reg = Ecosystems::with_cache(CacheConfig::from_lookup(|_| None).in_dir(dir.clone()));
        let s = Scratch::new();
        s.write("package.json", "{}");
        let p = Project {
            root: s.path(""),
            ecosystem: Ecosystem::Node,
            manifests: vec![],
        };
        assert_eq!(reg.detect_traced(&p).unwrap().1, Origin::Off);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
