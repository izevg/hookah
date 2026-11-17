//! The verdict handed back to the agent.
//!
//! `Verdict`: `Pass`, `Fail` (findings the agent must fix), or `Degraded` (`hookah` could not
//! finish: internal error, timeout, missing tool). `Degraded` never blocks the agent: this is the
//! fail-open rule. Only real findings make `Fail`.

pub mod filter;
pub mod notice;
pub mod report;

pub use filter::ReportingFilter;
pub use notice::Notice;
pub use report::{DEFAULT_LIMIT, Report};

/// The decision about a check run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing to report.
    Pass,
    /// Findings the agent must fix.
    Fail(Report),
    /// `hookah` could not run the checks; the reason is for display. Never blocks the agent.
    Degraded(String),
}
