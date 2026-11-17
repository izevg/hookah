//! Filesystem adapters: project location, manifest recognition, content hashing.

pub mod hash;
pub mod locator;
pub mod manifest;

pub use hash::FileHasher;
pub use locator::FsLocator;
