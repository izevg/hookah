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
}
