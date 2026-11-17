//! `Manifest` value object.
//!
//! A recognized package manifest (`package.json`, `pyproject.toml`, `Cargo.toml`): its path and
//! the ecosystem it implies. It does not parse file contents; adapters do that.

use std::path::{Path, PathBuf};

use super::ecosystem::Ecosystem;

/// A package manifest file and the ecosystem it implies.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Manifest {
    /// Absolute path of the manifest file.
    pub path: PathBuf,
    /// The ecosystem the manifest belongs to.
    pub ecosystem: Ecosystem,
}

impl Manifest {
    /// Recognizes a manifest by its file name. Returns `None` for any other file.
    pub fn from_path(path: &Path) -> Option<Self> {
        let ecosystem = match path.file_name()?.to_str()? {
            "package.json" => Ecosystem::Node,
            "pyproject.toml" => Ecosystem::Python,
            "Cargo.toml" => Ecosystem::Rust,
            _ => return None,
        };
        Some(Self {
            path: path.to_path_buf(),
            ecosystem,
        })
    }

    /// The directory holding the manifest.
    pub fn dir(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new(""))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_manifests() {
        let m = Manifest::from_path(Path::new("/p/Cargo.toml")).unwrap();
        assert_eq!(m.ecosystem, Ecosystem::Rust);
        assert_eq!(m.dir(), Path::new("/p"));
        assert!(Manifest::from_path(Path::new("/p/README.md")).is_none());
    }
}
