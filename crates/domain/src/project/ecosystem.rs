//! `Ecosystem`: the language family of a project (`Node`, `Python`, `Rust`).
//!
//! Decides which toolchain detector applies. Also maps a file extension to the ecosystems that
//! can check it, so files the project cannot check are skipped early.
