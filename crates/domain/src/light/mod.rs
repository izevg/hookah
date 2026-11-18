//! Light by default: `hookah` helps development, it does not gatekeep it.
//!
//! Two rules keep the stop phase cheap enough to run on every finish:
//!
//! - [`TestWeight`]: only light tests run. Integration, end-to-end, and browser tests are heavy
//!   and are never selected; the ecosystem adapters use the naming rules here.
//! - [`Budgets`]: how many test targets and how large a change the checks accept. Over budget,
//!   the check is skipped with a [`Skip`] notice instead of blocking the agent.

pub mod budgets;
pub mod weight;

pub use budgets::{Admission, Budgets, Skip};
pub use weight::TestWeight;
