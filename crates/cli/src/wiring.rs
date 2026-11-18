//! Composition root.
//!
//! The only place that builds concrete adapters and connects them to the application ports.
//! All construction lives in [`build`], so swapping an adapter touches one line.

use hookah_adapters::config::env::{Host, ProjectEnvRunner};
use hookah_adapters::ecosystems::Ecosystems;
use hookah_adapters::fs::{FileHasher, FsLocator};
use hookah_adapters::process::{BackgroundSpawner, ProcessRunner};
use hookah_adapters::store::baseline::FileBaselineStore;
use hookah_adapters::store::status::FileStatusStore;
use hookah_adapters::vcs::GitChanges;
use hookah_application::Ports;
use hookah_domain::light::Budgets;

/// The concrete adapters, owned. Borrow them as [`Ports`] with [`Adapters::ports`].
pub struct Adapters {
    locator: FsLocator,
    detector: Ecosystems,
    runner: ProjectEnvRunner<ProcessRunner>,
    changes: GitChanges,
    hasher: FileHasher,
    baselines: FileBaselineStore,
    spawner: BackgroundSpawner,
    statuses: FileStatusStore,
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
            statuses: &self.statuses,
        }
    }
}

/// Builds every adapter. The single construction site.
pub fn build() -> Adapters {
    Adapters {
        locator: FsLocator::new(),
        detector: Ecosystems::from_env(),
        runner: ProjectEnvRunner::new(ProcessRunner, Host::from_process()),
        changes: GitChanges::new(),
        hasher: FileHasher,
        baselines: FileBaselineStore::new(),
        statuses: FileStatusStore::new(),
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

/// Environment variable: the most test targets that run at stop. `0` runs no tests.
pub const TEST_BUDGET: &str = "HOOKAH_TEST_BUDGET";
/// Environment variable: the most changed files mutation testing accepts. `0` switches it off.
pub const MUTATION_MAX_FILES: &str = "HOOKAH_MUTATION_MAX_FILES";
/// Environment variable: the most changed lines mutation testing accepts. `0` switches it off.
pub const MUTATION_MAX_LINES: &str = "HOOKAH_MUTATION_MAX_LINES";

/// The budgets of the light stop phase: the defaults, overridden by the environment.
pub fn budgets() -> Budgets {
    budgets_from(|name| std::env::var(name).ok())
}

/// [`budgets`] with an explicit environment. A value that is not a whole number is ignored, so a
/// typo never blocks the agent.
pub fn budgets_from(get: impl Fn(&str) -> Option<String>) -> Budgets {
    let number = |name: &str| get(name).and_then(|v| v.trim().parse::<usize>().ok());
    let mut budgets = Budgets::default();
    if let Some(n) = number(TEST_BUDGET) {
        budgets = budgets.with_tests(n);
    }
    if let Some(n) = number(MUTATION_MAX_FILES) {
        budgets = budgets.with_mutation_files(n);
    }
    if let Some(n) = number(MUTATION_MAX_LINES) {
        budgets = budgets.with_mutation_lines(n);
    }
    budgets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from(vars: &[(&str, &str)]) -> Budgets {
        budgets_from(|name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_string())
        })
    }

    #[test]
    fn budgets_default_and_read_each_variable() {
        assert_eq!(from(&[]), Budgets::default());
        assert_eq!(from(&[(TEST_BUDGET, "7")]), Budgets::new(7, 3, 150));
        assert_eq!(
            from(&[(MUTATION_MAX_FILES, " 9 ")]),
            Budgets::new(25, 9, 150)
        );
        assert_eq!(from(&[(MUTATION_MAX_LINES, "40")]), Budgets::new(25, 3, 40));
        assert_eq!(
            from(&[(TEST_BUDGET, "0"), (MUTATION_MAX_FILES, "0")]),
            Budgets::new(0, 0, 150)
        );
    }

    #[test]
    fn unusable_values_keep_the_default() {
        for bad in ["", "many", "-1", "2.5"] {
            assert_eq!(from(&[(TEST_BUDGET, bad)]), Budgets::default(), "{bad:?}");
        }
    }

    #[test]
    fn the_baseline_job_runs_in_the_background_unless_the_environment_says_otherwise() {
        if std::env::var_os(FOREGROUND_BASELINE).is_none() {
            assert!(!foreground_baseline());
        }
    }
}
