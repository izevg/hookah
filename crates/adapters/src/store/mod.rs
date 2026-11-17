//! File-based storage. Post-MVP: doc-only stubs, nothing is implemented.
//!
//! The MVP keeps no state between hook calls. Later this module will implement the baseline
//! store behind the storage ports.

pub mod baseline;
pub mod cache_dir;
pub mod session;
