//! Composition root.
//!
//! The only place that builds concrete adapters and connects them to the application ports.
//! All construction lives in [`build`], so swapping an adapter touches one line.

use hookah_adapters::ecosystems::Ecosystems;
use hookah_adapters::fs::{FileHasher, FsLocator};
use hookah_adapters::process::ProcessRunner;
use hookah_adapters::vcs::GitChanges;
use hookah_application::Ports;

/// The concrete adapters, owned. Borrow them as [`Ports`] with [`Adapters::ports`].
pub struct Adapters {
    locator: FsLocator,
    detector: Ecosystems,
    runner: ProcessRunner,
    changes: GitChanges,
    hasher: FileHasher,
}

impl Adapters {
    /// The ecosystem registry itself, for `hookah detect` to ask where a result came from.
    pub fn detector(&self) -> &Ecosystems {
        &self.detector
    }

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
        locator: FsLocator::new(),
        detector: Ecosystems::from_env(),
        runner: ProcessRunner,
        changes: GitChanges::new(),
        hasher: FileHasher,
    }
}
