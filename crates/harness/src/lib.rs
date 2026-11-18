//! Driving adapters: one module per AI coding agent harness.
//!
//! Each module turns that harness's hook payload into a use-case request, and the resulting
//! outcome into the response that harness understands (exit code, stdout, stderr, JSON). No
//! business logic lives here.
//!
//! - Command hooks: [`claude`], [`codex`], [`kimi`], [`qoder`], [`cursor`].
//! - Plugins: [`opencode`] (a TypeScript plugin that calls the `hookah` binary), [`orca`].
//! - Extensions: [`plugins::pi`] (Pi: a TypeScript extension that calls the `hookah` binary).
//! - Plugins: [`plugins::openclaw`] (OpenClaw: a JavaScript plugin that calls the `hookah` binary).

pub mod claude;
pub mod codex;
pub mod cursor;
pub mod kimi;
pub mod opencode;
pub mod orca;
pub mod plugins;
pub mod protocol;
pub mod qoder;

#[cfg(test)]
mod testing;
