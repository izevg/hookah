//! Composition root.
//!
//! The only place that builds concrete adapters and connects them to the application ports.
//! All construction lives in [`build`], so swapping an adapter touches one line.

use std::path::{Path, PathBuf};

use hookah_adapters::process::ProcessRunner;
use hookah_application::ports::{
    ChangeSource, ContentHasher, ProcessOutput, ProjectLocator, ToolchainDetector,
};
use hookah_application::{PortError, Ports};
use hookah_domain::finding::Finding;
use hookah_domain::project::{Manifest, Project};
use hookah_domain::session::ChangedLines;
use hookah_domain::toolchain::{TestSelection, Tool, Toolchain};

/// The concrete adapters, owned. Borrow them as [`Ports`] with [`Adapters::ports`].
pub struct Adapters {
    locator: NullLocator,
    detector: NullDetector,
    runner: ProcessRunner,
    changes: NullChanges,
    hasher: NullHasher,
}

impl Adapters {
    /// The ports the use cases take.
    pub fn ports(&self) -> Ports<'_> {
        Ports {
            locator: &self.locator,
            detector: &self.detector,
            runner: &self.runner,
            changes: &self.changes,
            hasher: &self.hasher,
        }
    }
}

/// Builds every adapter. The single construction site.
pub fn build() -> Adapters {
    Adapters {
        // Replaced by real adapters at integration (P3).
        locator: NullLocator,
        // Replaced by real adapters at integration (P3).
        detector: NullDetector,
        runner: ProcessRunner,
        // Replaced by real adapters at integration (P3).
        changes: NullChanges,
        // Replaced by real adapters at integration (P3).
        hasher: NullHasher,
    }
}

// Replaced by real adapters at integration (P3): `hookah_adapters::fs::FsLocator`.
struct NullLocator;

impl ProjectLocator for NullLocator {
    fn manifests_above(&self, _: &Path) -> Result<Vec<Manifest>, PortError> {
        Ok(vec![])
    }
}

// Replaced by real adapters at integration (P3): `hookah_adapters::ecosystems::Ecosystems`.
struct NullDetector;

impl ToolchainDetector for NullDetector {
    fn detect(&self, _: &Project) -> Result<Toolchain, PortError> {
        Ok(Toolchain::new(vec![]))
    }

    fn select_tests(&self, _: &Project, _: &[PathBuf]) -> Result<TestSelection, PortError> {
        Ok(TestSelection::None)
    }

    fn parse(&self, _: &Project, _: &Tool, _: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
        Ok(vec![])
    }
}

// Replaced by real adapters at integration (P3): `hookah_adapters::vcs::GitChanges`.
struct NullChanges;

impl ChangeSource for NullChanges {
    fn changed_lines(&self, _: &Path, _: &Path) -> Result<ChangedLines, PortError> {
        Ok(ChangedLines::All)
    }

    fn changed_files(&self, _: &Path) -> Result<Vec<PathBuf>, PortError> {
        Ok(vec![])
    }
}

// Replaced by real adapters at integration (P3): `hookah_adapters::fs::FileHasher`.
struct NullHasher;

impl ContentHasher for NullHasher {
    fn content_hash(&self, _: &Path) -> Result<u64, PortError> {
        Ok(0)
    }
}
