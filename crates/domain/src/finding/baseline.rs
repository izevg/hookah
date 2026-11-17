//! `Baseline` and the "only new findings" rule. Post-MVP.
//!
//! A baseline is the set of fingerprints that existed before the agent's change. Comparing a
//! fresh run with it gives the findings this change introduced. The MVP has no storage; it uses
//! `verdict::filter::ReportingFilter` (changed lines reported by version control) instead.
