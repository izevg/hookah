//! The project aggregate.
//!
//! `Project`: the unit `hookah` checks against. It has a root directory, an ecosystem, and the
//! manifests found at that root. In a monorepo, every package with its own manifest is a
//! separate project.

pub mod ecosystem;
pub mod locate;
pub mod manifest;
