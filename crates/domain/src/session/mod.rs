//! Phases of an agent run and the files it changed.
//!
//! There is no persisted session aggregate in the MVP: changes come from git through the
//! `ChangeSource` port, not from a session store.

pub mod changes;
pub mod phase;

pub use changes::{ChangeSet, ChangedLines, LineRange, ProjectChanges};
pub use phase::Phase;
