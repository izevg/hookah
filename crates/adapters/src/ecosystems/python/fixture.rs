//! Test support: scratch project directories.

#![cfg(test)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A temporary directory removed on drop. The root is canonical, so paths compare equal to what
/// tools report.
pub struct Scratch(PathBuf);

impl Scratch {
    /// Creates an empty directory.
    pub fn new(name: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("hookah-py-{name}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dunce::canonicalize(dir).unwrap())
    }

    /// The canonical root.
    pub fn root(&self) -> &Path {
        &self.0
    }

    /// Writes a file, creating parent directories.
    pub fn write(&self, rel: &str, text: &str) {
        let path = self.0.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[cfg(unix)]
    /// Creates an executable file, creating parent directories.
    pub fn executable(&self, rel: &str) {
        use std::os::unix::fs::PermissionsExt;
        self.write(rel, "#!/bin/sh\n");
        let path = self.0.join(rel);
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
