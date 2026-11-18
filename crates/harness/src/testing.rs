//! Test support: absolute paths that are absolute on every OS.
//!
//! The payload fixtures are written with Unix paths (`/w/app`). On Windows such a path has no
//! drive, so it is not absolute and the parsers rightly refuse it as a working directory. These
//! helpers add `C:` there and are the identity elsewhere.

use std::path::PathBuf;

/// `path`, with `C:` in front on Windows when it starts with `/`.
pub fn abs(path: &str) -> PathBuf {
    if cfg!(windows) && path.starts_with('/') {
        PathBuf::from(format!("C:{path}"))
    } else {
        PathBuf::from(path)
    }
}

/// `json` with every string that starts with `/` made absolute for the host OS.
pub fn os_json(json: &str) -> String {
    if cfg!(windows) {
        json.replace("\"/", "\"C:/")
    } else {
        json.to_owned()
    }
}
