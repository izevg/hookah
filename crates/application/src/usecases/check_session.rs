//! `CheckSession`: the stop phase.
//!
//! For every project touched in the session, select the tests related to its changed files, run
//! them, and return the new failures. A failing verdict blocks the agent from finishing.
