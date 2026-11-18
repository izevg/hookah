//! Helpers shared by the end-to-end tests that run `hookah` in a sandbox.

use std::fs;
use std::path::{Path, PathBuf};

/// True if the path a command reported (`reported`, relative to `base` or absolute, and written
/// with `/` on Windows) lies inside `root`.
///
/// Both sides are canonical, so 8.3 short names, drive-letter case, and `/` against `\` do not
/// matter, and components are compared, not text. A path that no longer exists (a file that was
/// just deleted) is judged by its nearest existing ancestor plus the missing tail.
pub fn is_inside(root: &Path, base: &Path, reported: &str) -> bool {
    let Ok(root) = fs::canonicalize(root) else {
        return false;
    };
    let mut existing = base.join(reported);
    let mut tail = PathBuf::new();
    loop {
        if let Ok(canonical) = fs::canonicalize(&existing) {
            return canonical.join(&tail).starts_with(&root);
        }
        let (Some(name), Some(parent)) = (
            existing.file_name().map(ToOwned::to_owned),
            existing.parent().map(Path::to_path_buf),
        ) else {
            return false;
        };
        tail = Path::new(&name).join(&tail);
        existing = parent;
    }
}
