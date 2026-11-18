//! The one rule for showing a path to the agent.
//!
//! Every path in a report or a header is rendered the same way, in every phase: lexically
//! normalized, relative to the display root (the directory where the agent stands), and
//! absolute (still normalized) only when it lies outside that root. Pure string work: the
//! filesystem is never touched, so symlinks and aliases such as `/var` vs `/private/var` are
//! the caller's job to canonicalize before building requests.
//!
//! On Windows the result is written with `/` separators (`src/a.rs`, `C:/other/a.rs`), so reports
//! look the same on every platform, match the paths of the VCS, and need no escaping in JSON.
//! Windows accepts `/` in every path API, so the result stays usable as a path. The caller should
//! strip verbatim prefixes (`\\?\`) with `dunce::canonicalize` before building requests.

use std::path::{Component, Path, PathBuf};

/// Resolves `.` and `..` segments without touching the filesystem.
///
/// A `..` that would climb above the root of an absolute path stays at the root; one at the start
/// of a relative path is kept.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Renders `path` for display: relative to `display_root`, or absolute when outside it.
///
/// `path` is absolute, or relative to `display_root`. Both are normalized first, so
/// `/r/core/../app/src/lib.rs` shown from `/r` is `app/src/lib.rs`.
pub fn display_path(path: &Path, display_root: &Path) -> PathBuf {
    let root = normalize(display_root);
    let abs = if path.is_absolute() {
        normalize(path)
    } else {
        normalize(&root.join(path))
    };
    let shown = match abs.strip_prefix(&root) {
        Ok(rel) if rel.as_os_str().is_empty() => PathBuf::from("."),
        Ok(rel) => rel.to_path_buf(),
        Err(_) => abs,
    };
    if cfg!(windows) {
        forward_slashes(&shown)
    } else {
        shown
    }
}

/// `path` with every `\` separator turned into `/`. Only Windows calls this: on Unix a
/// backslash is a legal file name character.
fn forward_slashes(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(path: &str, root: &str) -> String {
        display_path(Path::new(path), Path::new(root))
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn normalizes_dot_and_dotdot() {
        assert_eq!(normalize(Path::new("/a/./b/../c")), Path::new("/a/c"));
        assert_eq!(normalize(Path::new("/../a")), Path::new("/a"));
        assert_eq!(normalize(Path::new("../a/../../b")), Path::new("../../b"));
        assert_eq!(normalize(Path::new("a/..")), Path::new(""));
    }

    #[test]
    fn inside_the_root_is_relative() {
        assert_eq!(shown("/r/src/a.rs", "/r"), "src/a.rs");
        assert_eq!(shown("/r/tests/a.rs", "/r/"), "tests/a.rs");
    }

    #[test]
    fn sibling_dirs_resolve_dotdot_before_relativizing() {
        assert_eq!(shown("/r/core/../app/src/lib.rs", "/r"), "app/src/lib.rs");
        assert_eq!(
            shown("/r/core/../app/src/lib.rs", "/r/core"),
            "/r/app/src/lib.rs"
        );
    }

    #[test]
    fn outside_the_root_is_absolute_and_normalized() {
        assert_eq!(shown("/other/x/../a.rs", "/r"), "/other/a.rs");
        assert_eq!(shown("/rr/a.rs", "/r"), "/rr/a.rs");
    }

    #[test]
    fn relative_input_is_taken_from_the_display_root() {
        assert_eq!(shown("../app/lib.rs", "/r/core"), "/r/app/lib.rs");
        assert_eq!(shown("../core/lib.rs", "/r/core"), "lib.rs");
        assert_eq!(shown("src/./a.rs", "/r"), "src/a.rs");
    }

    #[test]
    fn display_root_is_normalized_too() {
        assert_eq!(shown("/r/src/a.rs", "/r/x/.."), "src/a.rs");
    }

    #[test]
    fn backslashes_become_slashes() {
        assert_eq!(
            forward_slashes(Path::new(r"C:\r\src\a.rs")),
            Path::new("C:/r/src/a.rs")
        );
        assert_eq!(forward_slashes(Path::new("a/b")), Path::new("a/b"));
    }

    #[test]
    fn the_root_itself_is_a_dot() {
        assert_eq!(shown("/r", "/r"), ".");
    }
}
