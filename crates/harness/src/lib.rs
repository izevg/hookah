//! Driving adapters: one module per AI coding agent harness.
//!
//! Each module turns that harness's hook payload into a use-case request, and the resulting
//! outcome into the response that harness understands (exit code, stdout, stderr, JSON). No
//! business logic lives here.
//!
//! - Command hooks ([`hooks`]): [`antigravity`], [`claude`], [`codex`], [`cursor`], [`kimi`],
//!   [`qoder`], [`zcode`].
//! - Code plugins and extensions ([`plugins`]): [`opencode`] (a TypeScript plugin), [`orca`],
//!   [`pi`] (a TypeScript extension), [`openclaw`] (a JavaScript plugin), [`hermes`] (a Python
//!   plugin). Each one calls the `hookah` binary.

pub mod hooks;
pub mod plugins;
pub mod protocol;

pub use hooks::{antigravity, claude, codex, cursor, kimi, qoder, zcode};
pub use plugins::{hermes, openclaw, opencode, orca, pi};

#[cfg(test)]
mod testing;
