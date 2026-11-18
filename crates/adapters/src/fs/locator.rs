//! `ProjectLocator` over the real filesystem.
//!
//! Walks parent directories from the changed file and collects every recognized manifest. The
//! walk ends after the directory holding `.git` (inclusive), before `$HOME` (exclusive), or at
//! the filesystem root. A file that no longer exists is handled by starting at its nearest
//! existing ancestor directory.
//!
//! Post-MVP: cache results by manifest mtime so repeated hook calls skip the walk.

use std::path::{Path, PathBuf};

use hookah_application::error::PortError;
use hookah_application::ports::ProjectLocator;
use hookah_domain::project::Manifest;

use super::manifest::manifests_in;

/// Locates manifests by walking up the real filesystem.
#[derive(Debug, Clone)]
pub struct FsLocator {
    home: Option<PathBuf>,
}

impl Default for FsLocator {
    fn default() -> Self {
        Self::new()
    }
}

impl FsLocator {
    /// A locator that stops at `$HOME` (read from the environment).
    pub fn new() -> Self {
        Self {
            home: std::env::var_os("HOME")
                .filter(|h| !h.is_empty())
                .map(PathBuf::from),
        }
    }

    /// A locator with an explicit home boundary (or none).
    pub fn with_home(home: Option<PathBuf>) -> Self {
        Self { home }
    }
}

impl ProjectLocator for FsLocator {
    fn manifests_above(&self, file: &Path) -> Result<Vec<Manifest>, PortError> {
        let file = std::path::absolute(file)
            .map_err(|e| PortError::new(format!("cannot resolve {}: {e}", file.display())))?;
        let mut found = Vec::new();
        let Some(start) = file.parent() else {
            return Ok(found);
        };
        for dir in start.ancestors() {
            // A deleted file may sit in a deleted directory: skip until one exists.
            if !dir.is_dir() {
                continue;
            }
            if self.home.as_deref() == Some(dir) {
                break;
            }
            found.extend(manifests_in(dir));
            if dir.join(".git").exists() {
                break;
            }
        }
        Ok(found)
    }

    fn nested_roots(&self, dir: &Path, max_depth: usize) -> Result<Vec<PathBuf>, PortError> {
        let dir = std::path::absolute(dir)
            .map_err(|e| PortError::new(format!("cannot resolve {}: {e}", dir.display())))?;
        let mut found = Vec::new();
        let mut level = vec![dir];
        for _ in 0..max_depth {
            let mut next = Vec::new();
            for parent in &level {
                for child in subdirectories(parent) {
                    if !manifests_in(&child).is_empty() {
                        found.push(child.clone());
                    }
                    next.push(child);
                }
            }
            level = next;
        }
        Ok(found)
    }
}

/// Directory names never searched for nested projects: dependencies, build output, caches.
const SKIPPED: [&str; 7] = [
    "node_modules",
    "target",
    "venv",
    "dist",
    "build",
    "__pycache__",
    "vendor",
];

/// The subdirectories of `dir` worth searching, sorted by name; hidden ones and `SKIPPED` are
/// left out, and so are symlinks (they could loop).
fn subdirectories(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            !name.starts_with('.') && !SKIPPED.contains(&name.as_ref())
        })
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs
}
