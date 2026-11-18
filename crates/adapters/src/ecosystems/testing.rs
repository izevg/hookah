//! Test support: fixtures written with Unix paths, made absolute on every OS.
//!
//! Tool output in the unit tests names files like `/ws/app/src/a.ts`. On Windows such a path has
//! no drive, so it is not absolute. These helpers add `C:` there and change nothing elsewhere.

#![cfg(test)]

use std::path::PathBuf;

/// The workspace root of the fixtures.
pub const WS: &str = if cfg!(windows) { "C:/ws" } else { "/ws" };

/// `text` with every `/ws` path made absolute for the host OS.
pub fn rooted(text: &str) -> String {
    text.replace("/ws", WS)
}

/// `path` with `C:` in front on Windows when it starts with `/`.
pub fn abs(path: &str) -> PathBuf {
    if cfg!(windows) && path.starts_with('/') {
        PathBuf::from(format!("C:{path}"))
    } else {
        PathBuf::from(path)
    }
}
