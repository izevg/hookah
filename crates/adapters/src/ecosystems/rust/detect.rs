//! Rust toolchain detection.
//!
//! Reads `Cargo.toml` (workspace and member) to find `rustfmt`, `clippy`, and `cargo test`,
//! scoped to the crate that owns the changed file.
