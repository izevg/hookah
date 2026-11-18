//! Storage and background-job ports for finding baselines.
//!
//! - `BaselineStore`: the baseline of one (project root, agent session) pair and its lifecycle:
//!   not started, in progress, failed, or ready.
//! - `BaselineSpawner`: starts the job that computes a baseline without making the caller wait.
//! - `StatusStore`: the latest check status of a project.

use std::path::Path;

use hookah_domain::finding::baseline::Baseline;
use hookah_domain::status::{RecordedStatus, Status};

use crate::error::PortError;

/// Where the baseline of a (project root, session) pair stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaselineState {
    /// Nothing recorded (or the record is unreadable).
    Absent,
    /// A job is computing it.
    InProgress,
    /// The job failed or never finished; the reason is for display. Reporting falls back to the
    /// changed-lines filter.
    Failed(String),
    /// The baseline is ready.
    Ready(Baseline),
}

/// Remembers baselines per project root and session id.
pub trait BaselineStore {
    /// The current state. An unreadable record is [`BaselineState::Absent`].
    fn state(&self, root: &Path, session: &str) -> BaselineState;

    /// Marks the baseline as in progress. Returns `false` when a record already exists, so only
    /// one job runs per pair. An unreadable record does not count as existing.
    fn begin(&self, root: &Path, session: &str) -> Result<bool, PortError>;

    /// Stores the finished baseline.
    fn save(&self, root: &Path, session: &str, baseline: &Baseline) -> Result<(), PortError>;

    /// Records that computing the baseline failed.
    fn fail(&self, root: &Path, session: &str, reason: &str) -> Result<(), PortError>;
}

/// Starts the baseline job for a (project root, session) pair and returns without waiting for it
/// (an implementation may wait when told to, for tests).
pub trait BaselineSpawner {
    /// Starts the job. An error means it could not be started.
    fn spawn(&self, root: &Path, session: &str) -> Result<(), PortError>;
}

/// Remembers the latest check status of each project, for tools that want to show it.
pub trait StatusStore {
    /// Stores `status` as the latest of the project at `root`, stamped with the current time.
    fn record(&self, root: &Path, status: &Status) -> Result<(), PortError>;

    /// The latest status of the project at `root`. An unreadable record is `None`.
    fn latest(&self, root: &Path) -> Option<RecordedStatus>;
}
