//! The verdict handed back to the agent.
//!
//! `Verdict`: `Pass`, `Fail` (new findings the agent must fix), or `Degraded` (`hookah` could not
//! finish: internal error or timeout). `Degraded` never blocks the agent: this is the fail-open
//! rule.

pub mod notice;
pub mod report;
