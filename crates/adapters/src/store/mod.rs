//! File-based storage. [`cache_dir`] is implemented: it resolves the per-project cache
//! directory. Baseline and session storage ([`baseline`], [`session`]) are post-MVP doc-only
//! stubs.
//!
//! The MVP keeps no state between hook calls. Later the stubs will implement the storage ports.

pub mod baseline;
pub mod cache_dir;
pub mod session;
