//! Cache directory layout.
//!
//! [`project_cache_dir`] resolves the directory where one project may keep derived files, such as
//! incremental compiler state or tool reports. The layout is `<cache base>/hookah/<hash of root>/`:
//!
//! - cache base: `$XDG_CACHE_HOME` when it is set to an absolute path, else `~/Library/Caches` on
//!   macOS and `~/.cache` elsewhere;
//! - hash: a 64-bit FNV-1a of the canonical project root, as 16 hex digits. FNV-1a is written out
//!   here instead of using `std`'s hasher because the std algorithm is unspecified and the
//!   directory name must stay stable across toolchain upgrades.
//!
//! Directories are created with mode `0700`. Nothing is ever written outside this tree.

use std::ffi::OsString;
use std::fs::DirBuilder;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use hookah_application::error::PortError;

/// The cache directory of the project at `project_root`; created when missing.
pub fn project_cache_dir(project_root: &Path) -> Result<PathBuf, PortError> {
    let base =
        cache_base(|key| std::env::var_os(key), cfg!(target_os = "macos")).ok_or_else(|| {
            PortError::new("no cache directory: neither XDG_CACHE_HOME nor HOME is set")
        })?;
    project_dir_in(&base, project_root)
}

/// The directory under which `hookah` keeps its caches, from the environment accessor `env`.
fn cache_base(env: impl Fn(&str) -> Option<OsString>, macos: bool) -> Option<PathBuf> {
    if let Some(xdg) = env("XDG_CACHE_HOME").map(PathBuf::from)
        && xdg.is_absolute()
    {
        return Some(xdg);
    }
    let home = PathBuf::from(env("HOME").filter(|h| !h.is_empty())?);
    Some(if macos {
        home.join("Library").join("Caches")
    } else {
        home.join(".cache")
    })
}

/// `<base>/hookah/<hash of root>`, created with mode `0700`.
fn project_dir_in(base: &Path, project_root: &Path) -> Result<PathBuf, PortError> {
    let root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let dir = base.join("hookah").join(root_hash(&root));
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|e| PortError::new(format!("cannot create {}: {e}", dir.display())))?;
    Ok(dir)
}

/// 64-bit FNV-1a of the root path bytes, as 16 lowercase hex digits.
fn root_hash(root: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in root.as_os_str().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn xdg_wins_when_absolute() {
        let e = env(&[("XDG_CACHE_HOME", "/xdg"), ("HOME", "/home/u")]);
        assert_eq!(cache_base(e, true), Some("/xdg".into()));
    }

    #[test]
    fn relative_or_missing_xdg_falls_back_to_the_platform_default() {
        let e = env(&[("XDG_CACHE_HOME", "rel"), ("HOME", "/home/u")]);
        assert_eq!(cache_base(e, false), Some("/home/u/.cache".into()));
        let e = env(&[("HOME", "/Users/u")]);
        assert_eq!(cache_base(e, true), Some("/Users/u/Library/Caches".into()));
    }

    #[test]
    fn no_home_no_base() {
        assert_eq!(cache_base(env(&[]), false), None);
        assert_eq!(cache_base(env(&[("HOME", "")]), false), None);
    }

    #[test]
    fn hash_is_stable_and_distinguishes_roots() {
        // Known FNV-1a vector: the empty input hashes to the offset basis.
        assert_eq!(root_hash(Path::new("")), "cbf29ce484222325");
        assert_eq!(root_hash(Path::new("/a/b")), root_hash(Path::new("/a/b")));
        assert_ne!(root_hash(Path::new("/a/b")), root_hash(Path::new("/a/c")));
        assert_eq!(root_hash(Path::new("/a/b")).len(), 16);
        // Published FNV-1a 64 test vectors.
        assert_eq!(root_hash(Path::new("a")), "af63dc4c8601ec8c");
        assert_eq!(root_hash(Path::new("foobar")), "85944171f73967e8");
    }

    #[test]
    fn the_public_entry_point_resolves_under_the_environment_base() {
        let root = std::env::temp_dir();
        let Ok(dir) = project_cache_dir(&root) else {
            return; // neither XDG_CACHE_HOME nor HOME is set
        };
        assert!(dir.is_dir());
        let canonical = root.canonicalize().unwrap();
        assert_eq!(dir.file_name().unwrap(), root_hash(&canonical).as_str());
        assert_eq!(dir.parent().unwrap().file_name().unwrap(), "hookah");
    }

    #[test]
    fn creates_private_directories() {
        let base = std::env::temp_dir().join(format!("hookah-cache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let dir = project_dir_in(&base, Path::new("/some/project")).unwrap();
        assert_eq!(dir.parent().unwrap(), base.join("hookah"));
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&base.join("hookah")), 0o700);
        // Calling again is fine and returns the same directory.
        assert_eq!(
            project_dir_in(&base, Path::new("/some/project")).unwrap(),
            dir
        );
        let _ = fs::remove_dir_all(&base);
    }
}
