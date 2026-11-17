//! Application errors.
//!
//! `ApplicationError`: failures of a use case, sorted into "the agent's code is broken" (turns
//! into a failing verdict) and "hookah could not do its job" (turns into a degraded,
//! non-blocking verdict).
