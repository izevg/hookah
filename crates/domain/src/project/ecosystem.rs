//! `Ecosystem`: the language family of a project (`Java`, `Node`, `Python`, `Rust`).
//!
//! Decides which toolchain detector applies. Also maps a file extension to the ecosystems that
//! can check it, so files the project cannot check are skipped early.

use std::path::Path;

/// The language family of a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ecosystem {
    /// Java, built with Maven or Gradle.
    Java,
    /// TypeScript and JavaScript.
    Node,
    /// Python.
    Python,
    /// Rust.
    Rust,
}

impl Ecosystem {
    /// Lowercase name, stable for use in output and configuration.
    pub fn name(self) -> &'static str {
        match self {
            Self::Java => "java",
            Self::Node => "node",
            Self::Python => "python",
            Self::Rust => "rust",
        }
    }

    /// Whether the file's extension marks it as a source file of this ecosystem.
    pub fn can_check(self, file: &Path) -> bool {
        let Some(ext) = file.extension().and_then(|e| e.to_str()) else {
            return false;
        };
        let exts: &[&str] = match self {
            Self::Java => &["java"],
            Self::Node => &["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts"],
            Self::Python => &["py", "pyi"],
            Self::Rust => &["rs"],
        };
        exts.contains(&ext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_stable() {
        assert_eq!(Ecosystem::Java.name(), "java");
        assert_eq!(Ecosystem::Node.name(), "node");
        assert_eq!(Ecosystem::Python.name(), "python");
        assert_eq!(Ecosystem::Rust.name(), "rust");
    }

    #[test]
    fn maps_extensions() {
        assert!(Ecosystem::Rust.can_check(Path::new("/a/b.rs")));
        assert!(Ecosystem::Java.can_check(Path::new("/a/Foo.java")));
        assert!(!Ecosystem::Java.can_check(Path::new("Foo.kt")));
        assert!(Ecosystem::Node.can_check(Path::new("x.tsx")));
        assert!(Ecosystem::Python.can_check(Path::new("x.py")));
        assert!(!Ecosystem::Python.can_check(Path::new("x.rs")));
        assert!(!Ecosystem::Rust.can_check(Path::new("Makefile")));
    }
}
