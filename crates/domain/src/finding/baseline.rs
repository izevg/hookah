//! `Baseline` and the "only new findings" rule.
//!
//! A baseline is the set of fingerprints that existed before the agent's change. Comparing a
//! fresh run with it gives the findings this change introduced; only those are reported.
