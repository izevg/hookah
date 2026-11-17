//! Ports: the traits adapters implement.
//!
//! Grouped by what they abstract: finding things, running things, and (post-MVP) remembering
//! things. All traits are object-safe; use cases borrow them as `&dyn` through [`Ports`].

pub mod discovery;
pub mod execution;
pub mod storage;

pub use discovery::{ChangeSource, ProjectLocator, ToolchainDetector};
pub use execution::{CheckRunner, ContentHasher, ProcessOutput};

/// All ports the use cases need, borrowed. Built by the composition root.
#[derive(Clone, Copy)]
pub struct Ports<'a> {
    /// Finds candidate manifests.
    pub locator: &'a dyn ProjectLocator,
    /// Detects toolchains, selects tests, parses output.
    pub detector: &'a dyn ToolchainDetector,
    /// Runs commands.
    pub runner: &'a dyn CheckRunner,
    /// Reports changes.
    pub changes: &'a dyn ChangeSource,
    /// Hashes file content.
    pub hasher: &'a dyn ContentHasher,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod fakes {
    //! One in-memory world implementing every port, configured field by field.

    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use hookah_domain::finding::Finding;
    use hookah_domain::project::{Manifest, Project};
    use hookah_domain::session::{ChangedLines, FileRole};
    use hookah_domain::toolchain::{
        Arg, CheckKind, CommandSpec, MutationSelection, TestSelection, Tool, Toolchain,
    };

    use super::*;
    use crate::error::PortError;

    pub struct World {
        pub manifests: Vec<Manifest>,
        pub locate_fails: bool,
        pub toolchain: Toolchain,
        pub detect_fails: bool,
        pub selection: TestSelection,
        pub select_fails: bool,
        pub mutants: MutationSelection,
        /// Files (absolute) classified as tests; every other file is a source.
        pub test_files: Vec<PathBuf>,
        /// Arguments of every `select_mutants` call: (tests, sources).
        pub mutant_calls: RefCell<Vec<(Vec<PathBuf>, Vec<PathBuf>)>>,
        /// Findings returned by `parse`, keyed by tool name.
        pub findings: HashMap<String, Vec<Finding>>,
        pub run_fails: Vec<String>,
        pub timeouts: Vec<String>,
        pub calls: RefCell<Vec<String>>,
        pub rewrite: bool,
        hashes: Cell<u64>,
        pub lines: ChangedLines,
        pub lines_fail: bool,
        pub files: Vec<PathBuf>,
        pub files_fail: bool,
    }

    impl World {
        pub fn new(manifest: &str, tools: Vec<Tool>) -> Self {
            Self {
                manifests: vec![Manifest::from_path(Path::new(manifest)).unwrap()],
                locate_fails: false,
                toolchain: Toolchain::new(tools),
                detect_fails: false,
                selection: TestSelection::None,
                select_fails: false,
                mutants: MutationSelection::None,
                test_files: vec![],
                mutant_calls: RefCell::new(vec![]),
                findings: HashMap::new(),
                run_fails: vec![],
                timeouts: vec![],
                calls: RefCell::new(vec![]),
                rewrite: false,
                hashes: Cell::new(0),
                lines: ChangedLines::All,
                lines_fail: false,
                files: vec![],
                files_fail: false,
            }
        }

        pub fn ports(&self) -> Ports<'_> {
            Ports {
                locator: self,
                detector: self,
                runner: self,
                changes: self,
                hasher: self,
            }
        }

        pub fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    /// A tool whose program name equals its tool name.
    pub fn tool(name: &str, kind: CheckKind) -> Tool {
        Tool {
            name: name.into(),
            kind,
            command: CommandSpec::new(name, vec![Arg::Targets], "/r", Duration::from_secs(5)),
        }
    }

    impl ProjectLocator for World {
        fn manifests_above(&self, _: &Path) -> Result<Vec<Manifest>, PortError> {
            if self.locate_fails {
                return Err(PortError::new("walk failed"));
            }
            Ok(self.manifests.clone())
        }
    }

    impl ToolchainDetector for World {
        fn detect(&self, _: &Project) -> Result<Toolchain, PortError> {
            if self.detect_fails {
                return Err(PortError::new("detect failed"));
            }
            Ok(self.toolchain.clone())
        }

        fn classify(&self, _: &Project, file: &Path, _: &ChangedLines) -> FileRole {
            if self.test_files.iter().any(|t| t == file) {
                FileRole::Test
            } else {
                FileRole::Source
            }
        }

        fn select_mutants(
            &self,
            _: &Project,
            tests: &[PathBuf],
            sources: &[PathBuf],
        ) -> MutationSelection {
            self.mutant_calls
                .borrow_mut()
                .push((tests.to_vec(), sources.to_vec()));
            self.mutants.clone()
        }

        fn select_tests(&self, _: &Project, _: &[PathBuf]) -> Result<TestSelection, PortError> {
            if self.select_fails {
                return Err(PortError::new("select failed"));
            }
            Ok(self.selection.clone())
        }

        fn parse(
            &self,
            _: &Project,
            tool: &Tool,
            _: &ProcessOutput,
        ) -> Result<Vec<Finding>, PortError> {
            Ok(self.findings.get(&tool.name).cloned().unwrap_or_default())
        }
    }

    impl CheckRunner for World {
        fn run(&self, spec: &CommandSpec, targets: &[String]) -> Result<ProcessOutput, PortError> {
            self.calls
                .borrow_mut()
                .push(format!("{} {}", spec.program, targets.join(",")));
            if self.run_fails.contains(&spec.program) {
                return Err(PortError::new("not found"));
            }
            Ok(ProcessOutput {
                timed_out: self.timeouts.contains(&spec.program),
                exit_code: Some(0),
                ..ProcessOutput::default()
            })
        }
    }

    impl ChangeSource for World {
        fn changed_lines(&self, _: &Path, _: &Path) -> Result<ChangedLines, PortError> {
            if self.lines_fail {
                return Err(PortError::new("no vcs"));
            }
            Ok(self.lines.clone())
        }

        fn changed_files(&self, _: &Path) -> Result<Vec<PathBuf>, PortError> {
            if self.files_fail {
                return Err(PortError::new("no vcs"));
            }
            Ok(self.files.clone())
        }
    }

    impl ContentHasher for World {
        fn content_hash(&self, _: &Path) -> Result<u64, PortError> {
            if self.rewrite {
                self.hashes.set(self.hashes.get() + 1);
            }
            Ok(self.hashes.get())
        }
    }
}
