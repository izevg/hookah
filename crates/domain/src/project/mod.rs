//! The project aggregate.
//!
//! `Project`: the unit `hookah` checks against. It has a root directory, an ecosystem, and the
//! manifests found at that root. In a monorepo, every package with its own manifest is a
//! separate project.

pub mod ecosystem;
pub mod locate;
pub mod manifest;

use std::path::{Path, PathBuf};

use crate::error::DomainError;
pub use ecosystem::Ecosystem;
pub use locate::locate_project;
pub use manifest::Manifest;

/// A project: the root directory a file is checked against.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Project {
    /// Absolute project root (the directory of the nearest manifest).
    pub root: PathBuf,
    /// The primary ecosystem of the project.
    pub ecosystem: Ecosystem,
    /// Manifests found directly at the root.
    pub manifests: Vec<Manifest>,
}

impl Project {
    /// Whether `file` lies under the project root.
    pub fn contains(&self, file: &Path) -> bool {
        file.starts_with(&self.root)
    }

    /// The path of `file` relative to the project root.
    pub fn relative(&self, file: &Path) -> Result<PathBuf, DomainError> {
        file.strip_prefix(&self.root)
            .map(Path::to_path_buf)
            .map_err(|_| DomainError::FileOutsideProject {
                file: file.to_path_buf(),
                root: self.root.clone(),
            })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths() {
        let p = Project {
            root: "/r".into(),
            ecosystem: Ecosystem::Rust,
            manifests: vec![],
        };
        assert_eq!(
            p.relative(Path::new("/r/src/a.rs")).unwrap(),
            Path::new("src/a.rs")
        );
        assert!(p.relative(Path::new("/x/a.rs")).is_err());
        assert!(p.contains(Path::new("/r/a")));
        assert!(!p.contains(Path::new("/x/a")));
    }
}
