//! Composition root.
//!
//! The only place that builds concrete adapters and connects them to the application ports.
//! All construction lives in [`build`], so swapping an adapter touches one line.

use hookah_adapters::ecosystems::Ecosystems;
use hookah_adapters::fs::{FileHasher, FsLocator};
use hookah_adapters::process::{BackgroundSpawner, ProcessRunner};
use hookah_adapters::store::baseline::FileBaselineStore;
use hookah_adapters::vcs::GitChanges;
use hookah_application::Ports;

/// The concrete adapters, owned. Borrow them as [`Ports`] with [`Adapters::ports`].
pub struct Adapters {
    locator: FsLocator,
    detector: Ecosystems,
    runner: ProcessRunner,
    changes: GitChanges,
    hasher: FileHasher,
    baselines: FileBaselineStore,
    spawner: BackgroundSpawner,
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
            baselines: &self.baselines,
            spawner: &self.spawner,
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
        baselines: FileBaselineStore::new(),
        spawner: BackgroundSpawner::new(
            std::env::current_exe().unwrap_or_else(|_| "hookah".into()),
            foreground_baseline(),
        ),
    }
}

/// Environment switch that makes the baseline job run to completion before the hook returns.
/// Tests and the sample runner set it to get a baseline synchronously.
pub const FOREGROUND_BASELINE: &str = "HOOKAH_BASELINE_FOREGROUND";

fn foreground_baseline() -> bool {
    std::env::var_os(FOREGROUND_BASELINE).is_some_and(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_baseline_job_runs_in_the_background_unless_the_environment_says_otherwise() {
        if std::env::var_os(FOREGROUND_BASELINE).is_none() {
            assert!(!foreground_baseline());
        }
    }
}
