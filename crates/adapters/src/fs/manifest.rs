//! Manifest recognition.
//!
//! Lists a directory and keeps the entries the domain's `Manifest::from_path` recognizes by
//! file name. Reading manifest contents is left to `ecosystems`.

use std::path::Path;

use hookah_domain::project::Manifest;

/// The recognized manifests directly inside `dir`, sorted by path. An unreadable or missing
/// directory yields none. Directories named like a manifest are ignored.
pub fn manifests_in(dir: &Path) -> Vec<Manifest> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Manifest> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir()))
        .filter_map(|e| Manifest::from_path(&e.path()))
        .collect();
    found.sort();
    found
}
