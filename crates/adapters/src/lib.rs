//! Driven adapters: concrete implementations of the application ports.
//!
//! - [`fs`]: project location and manifest reading on the real filesystem.
//! - [`process`]: running external tools with timeouts and capturing output.
//! - [`store`]: the per-project cache directory. Baseline and session storage are post-MVP stubs.
//! - [`vcs`]: changed files and lines as reported by `git`.
//! - [`config`]: `.hookah.toml` loading and overrides on top of auto-detection.
//! - [`ecosystems`]: the `Ecosystems` registry plus per-ecosystem toolchain detection, output
//!   parsing, and test selection (Rust, Node, and Python).

pub mod config;
pub mod ecosystems;
pub mod fs;
pub mod process;
pub mod store;
pub mod vcs;
