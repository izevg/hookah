//! Version-control adapters: what changed since the last commit, via the `git` binary.

pub mod git;
pub mod hunk;

pub use git::GitChanges;
