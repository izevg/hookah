//! The session aggregate.
//!
//! `Session`: one agent run, identified by the harness's session id. It collects the files
//! changed during the run so the stop phase can test all of them together.

pub mod changes;
pub mod phase;
