//! Driven adapters: concrete implementations of the application ports.
//!
//! - [`fs`]: project location and manifest reading on the real filesystem.
//! - [`process`]: running external tools with timeouts and capturing output, and starting the
//!   detached baseline job.
//! - [`store`]: the per-project cache directory and the session finding baselines in it.
//! - [`update`]: self-update from the official GitHub releases: download, verify, replace.
//! - [`vcs`]: changed files and lines as reported by `git`.
//! - [`config`]: `.hookah.toml` loading and overrides on top of auto-detection.
//! - [`ecosystems`]: the `Ecosystems` registry plus per-ecosystem toolchain detection, output
//!   parsing, and test selection (Rust, Node, and Python).
//! - [`transcript`]: reads the active model identifier from session transcript JSONL files.

pub mod config;
pub mod ecosystems;
pub mod fs;
pub mod process;
pub mod store;
pub mod transcript;
pub mod update;
pub mod vcs;
