//! Driven adapters: concrete implementations of the application ports.
//!
//! - [`fs`]: project location and manifest reading on the real filesystem.
//! - [`process`]: running external tools with timeouts and capturing output.
//! - [`store`]: post-MVP storage stubs (baseline); nothing implemented yet.
//! - [`config`]: `.hookah.toml` loading and overrides on top of auto-detection.
//! - [`ecosystems`]: per-ecosystem toolchain detection, output parsing, and test selection.

pub mod config;
pub mod ecosystems;
pub mod fs;
pub mod process;
pub mod store;
