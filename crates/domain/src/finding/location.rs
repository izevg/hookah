//! `Location` value object: project-relative path, optional line, and optional column.
//!
//! Renders as `path`, `path:line`, or `path:line:col` in reports. The path is written with `/` on
//! every OS, like every other path in a report.

use std::fmt;
use std::path::PathBuf;

/// Where a finding is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Location {
    /// Path relative to the project root.
    pub path: PathBuf,
    /// 1-based line, if the tool reported one.
    pub line: Option<u32>,
    /// 1-based column; only meaningful together with `line`.
    pub column: Option<u32>,
}

impl Location {
    /// A location in a file with no line information.
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            line: None,
            column: None,
        }
    }

    /// A location at a line.
    pub fn line(path: impl Into<PathBuf>, line: u32) -> Self {
        Self {
            path: path.into(),
            line: Some(line),
            column: None,
        }
    }

    /// A location at a line and column.
    pub fn at(path: impl Into<PathBuf>, line: u32, column: u32) -> Self {
        Self {
            path: path.into(),
            line: Some(line),
            column: Some(column),
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path = self.path.to_string_lossy();
        if cfg!(windows) {
            write!(f, "{}", path.replace('\\', "/"))?;
        } else {
            write!(f, "{path}")?;
        }
        if let Some(line) = self.line {
            write!(f, ":{line}")?;
            if let Some(col) = self.column {
                write!(f, ":{col}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders() {
        assert_eq!(Location::file("a.rs").to_string(), "a.rs");
        assert_eq!(Location::line("a.rs", 3).to_string(), "a.rs:3");
        assert_eq!(Location::at("a.rs", 3, 7).to_string(), "a.rs:3:7");
    }

    #[test]
    fn nested_paths_render_with_forward_slashes() {
        let path: PathBuf = ["src", "util", "a.rs"].iter().collect();
        assert_eq!(Location::line(path, 3).to_string(), "src/util/a.rs:3");
    }
}
