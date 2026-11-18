//! Cache directory layout.
//!
//! [`project_cache_dir`] resolves the directory where one project may keep derived files, such as
//! incremental compiler state, tool reports, or the detection cache. The layout is
//! `<hookah cache>/<hash of root>/`, where the hookah cache is:
//!
//! - on every OS: `$HOOKAH_CACHE_DIR` when it is set to an absolute path (used as is, without a
//!   `hookah` suffix); an empty or relative value is ignored;
//! - Linux and other Unix: `$XDG_CACHE_HOME/hookah` when it is set to an absolute path, else
//!   `~/.cache/hookah`;
//! - macOS: `~/Library/Caches/hookah` (an absolute `$XDG_CACHE_HOME` still wins);
//! - Windows: `%LOCALAPPDATA%\hookah\cache`, with `%USERPROFILE%\AppData\Local` standing in for
//!   an unset `%LOCALAPPDATA%`.
//!
//! The hash is a 64-bit FNV-1a of the canonical project root, as 16 hex digits. FNV-1a is written
//! out here instead of using `std`'s hasher because the std algorithm is unspecified and the
//! directory name must stay stable across toolchain upgrades.
//!
//! On Unix, directories are created with mode `0700`; on Windows the per-user defaults apply.
//! Nothing is ever written outside this tree. When no base directory can be resolved, the
//! functions return an error and callers work without a cache.

use std::ffi::OsString;
use std::fs::{self, DirBuilder};
use std::path::{Path, PathBuf};

use hookah_application::error::PortError;

/// The platform families whose cache locations differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    Unix,
    Mac,
    Windows,
}

impl Platform {
    fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Unix
        }
    }
}

/// What `hookah uninstall --purge-cache` removes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachePurge {
    /// The directory under which `hookah` keeps its caches.
    pub base: PathBuf,
    /// The paths to delete: `base` itself, or, when the user chose the directory with
    /// `HOOKAH_CACHE_DIR` (it may hold anything), only the per-project directories inside it.
    pub paths: Vec<PathBuf>,
    /// True when `base` is the user's own directory: it is removed only if purging leaves it empty.
    pub shared: bool,
}

impl CachePurge {
    /// Deletes [`CachePurge::paths`], then a shared `base` that is now empty.
    pub fn run(&self) -> std::io::Result<()> {
        for path in &self.paths {
            fs::remove_dir_all(path)?;
        }
        if self.shared && !self.paths.is_empty() {
            // Not empty means the user keeps something there; that is fine.
            let _ = fs::remove_dir(&self.base);
        }
        Ok(())
    }
}

/// The cache that exists now and would be purged; `None` when there is no cache directory.
pub fn cache_purge() -> Option<CachePurge> {
    let base = cache_base(|key| std::env::var_os(key), Platform::current())?;
    let shared = std::env::var_os("HOOKAH_CACHE_DIR")
        .map(PathBuf::from)
        .is_some_and(|p| p.is_absolute());
    Some(purge_in(base, shared))
}

fn purge_in(base: PathBuf, shared: bool) -> CachePurge {
    let paths = if shared {
        let mut dirs: Vec<PathBuf> = fs::read_dir(&base)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| is_project_dir_name(&entry.file_name()))
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        dirs.sort();
        dirs
    } else {
        vec![base.clone()]
    };
    CachePurge {
        paths: paths.into_iter().filter(|p| p.exists()).collect(),
        base,
        shared,
    }
}

/// True for the 16 lowercase hex digits of [`root_hash`].
fn is_project_dir_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|n| n.len() == 16 && n.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

/// The cache directory of the project at `project_root`; created when missing.
pub fn project_cache_dir(project_root: &Path) -> Result<PathBuf, PortError> {
    let base = cache_base(|key| std::env::var_os(key), Platform::current()).ok_or_else(|| {
        PortError::new("no cache directory: no home or local application data directory is set")
    })?;
    project_dir_in(&base, project_root)
}

/// The directory under which `hookah` keeps its caches, from the environment accessor `env`.
fn cache_base(env: impl Fn(&str) -> Option<OsString>, os: Platform) -> Option<PathBuf> {
    let absolute = |key: &str| env(key).map(PathBuf::from).filter(|p| p.is_absolute());
    let non_empty = |key: &str| env(key).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(explicit) = absolute("HOOKAH_CACHE_DIR") {
        return Some(explicit);
    }
    if os == Platform::Windows {
        let local = absolute("LOCALAPPDATA")
            .or_else(|| non_empty("USERPROFILE").map(|p| p.join("AppData").join("Local")))?;
        return Some(local.join("hookah").join("cache"));
    }
    if let Some(xdg) = absolute("XDG_CACHE_HOME") {
        return Some(xdg.join("hookah"));
    }
    let home = non_empty("HOME")?;
    Some(match os {
        Platform::Mac => home.join("Library").join("Caches").join("hookah"),
        _ => home.join(".cache").join("hookah"),
    })
}

/// `<base>/<hash of root>`, created private to the user.
fn project_dir_in(base: &Path, project_root: &Path) -> Result<PathBuf, PortError> {
    let root = dunce::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let dir = base.join(root_hash(&root));
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(&dir)
        .map_err(|e| PortError::new(format!("cannot create {}: {e}", dir.display())))?;
    Ok(dir)
}

/// 64-bit FNV-1a of the root path, as 16 lowercase hex digits.
fn root_hash(root: &Path) -> String {
    #[cfg(unix)]
    let bytes = std::os::unix::ffi::OsStrExt::as_bytes(root.as_os_str()).to_vec();
    #[cfg(not(unix))]
    let bytes = root.to_string_lossy().into_owned().into_bytes();
    format!("{:016x}", fnv1a(&bytes))
}

/// 64-bit FNV-1a: stable across Rust versions, unlike `std`'s hashers.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::fs;

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
    fn the_explicit_override_wins_on_every_os() {
        let dir = std::env::temp_dir();
        let value = dir.to_str().unwrap();
        for os in [Platform::Unix, Platform::Mac, Platform::Windows] {
            let pairs = [
                ("HOOKAH_CACHE_DIR", value),
                ("XDG_CACHE_HOME", "/xdg"),
                ("LOCALAPPDATA", "/lad"),
                ("HOME", "/home/u"),
            ];
            assert_eq!(cache_base(env(&pairs), os), Some(dir.clone()));
        }
    }

    #[test]
    fn an_empty_or_relative_override_is_ignored() {
        for bad in ["", "rel"] {
            let pairs = [("HOOKAH_CACHE_DIR", bad), ("HOME", "/home/u")];
            assert_eq!(
                cache_base(env(&pairs), Platform::Unix),
                Some("/home/u/.cache/hookah".into())
            );
        }
    }

    #[test]
    fn xdg_wins_when_absolute() {
        // A Windows path needs a drive to be absolute.
        let xdg = if cfg!(windows) { "C:/xdg" } else { "/xdg" };
        let want = Some(Path::new(xdg).join("hookah"));
        let pairs = [("XDG_CACHE_HOME", xdg), ("HOME", "/home/u")];
        assert_eq!(cache_base(env(&pairs), Platform::Mac), want);
        assert_eq!(cache_base(env(&pairs), Platform::Unix), want);
    }

    #[test]
    fn relative_or_missing_xdg_falls_back_to_the_platform_default() {
        let e = env(&[("XDG_CACHE_HOME", "rel"), ("HOME", "/home/u")]);
        assert_eq!(
            cache_base(e, Platform::Unix),
            Some("/home/u/.cache/hookah".into())
        );
        let e = env(&[("HOME", "/Users/u")]);
        assert_eq!(
            cache_base(e, Platform::Mac),
            Some("/Users/u/Library/Caches/hookah".into())
        );
    }

    #[test]
    fn no_home_no_base() {
        assert_eq!(cache_base(env(&[]), Platform::Unix), None);
        assert_eq!(cache_base(env(&[("HOME", "")]), Platform::Unix), None);
        assert_eq!(cache_base(env(&[("HOME", "")]), Platform::Mac), None);
    }

    #[test]
    fn windows_uses_local_app_data() {
        let local = if cfg!(windows) {
            r"C:\Users\u\AppData\Local"
        } else {
            "/lad"
        };
        let pairs = [
            ("LOCALAPPDATA", local),
            ("USERPROFILE", "/profile"),
            ("XDG_CACHE_HOME", "/xdg"),
            ("HOME", "/home/u"),
        ];
        let e = env(&pairs);
        assert_eq!(
            cache_base(e, Platform::Windows),
            Some(Path::new(local).join("hookah").join("cache"))
        );
    }

    #[test]
    fn windows_falls_back_to_the_profile_directory() {
        let e = env(&[("USERPROFILE", "/profile"), ("HOME", "/home/u")]);
        let want = Path::new("/profile")
            .join("AppData")
            .join("Local")
            .join("hookah")
            .join("cache");
        assert_eq!(cache_base(e, Platform::Windows), Some(want.clone()));
        // A relative or empty %LOCALAPPDATA% is not trusted.
        let e = env(&[("LOCALAPPDATA", "rel"), ("USERPROFILE", "/profile")]);
        assert_eq!(cache_base(e, Platform::Windows), Some(want));
        let e = env(&[("LOCALAPPDATA", "rel"), ("USERPROFILE", "")]);
        assert_eq!(cache_base(e, Platform::Windows), None);
        assert_eq!(cache_base(env(&[("HOME", "/h")]), Platform::Windows), None);
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
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn the_public_entry_point_resolves_under_the_environment_base() {
        let root = std::env::temp_dir();
        let Ok(dir) = project_cache_dir(&root) else {
            return; // no base directory is set
        };
        assert!(dir.is_dir());
        let canonical = dunce::canonicalize(root).unwrap();
        assert_eq!(dir.file_name().unwrap(), root_hash(&canonical).as_str());
    }

    #[test]
    #[cfg(unix)]
    fn creates_private_directories() {
        use std::os::unix::fs::PermissionsExt;

        let base = std::env::temp_dir().join(format!("hookah-cache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let dir = project_dir_in(&base.join("hookah"), Path::new("/some/project")).unwrap();
        assert_eq!(dir.parent().unwrap(), base.join("hookah"));
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&base.join("hookah")), 0o700);
        // Calling again is fine and returns the same directory.
        assert_eq!(
            project_dir_in(&base.join("hookah"), Path::new("/some/project")).unwrap(),
            dir
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn an_uncreatable_directory_is_an_error() {
        let file = std::env::temp_dir().join(format!("hookah-cache-file-{}", std::process::id()));
        fs::write(&file, "x").unwrap();
        assert!(project_dir_in(&file, Path::new("/p")).is_err());
        let _ = fs::remove_file(&file);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hookah-purge-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn purging_the_default_location_removes_the_whole_directory() {
        let base = scratch("whole").join("hookah");
        fs::create_dir_all(base.join("0123456789abcdef")).unwrap();
        fs::write(base.join("other"), "x").unwrap();
        let purge = purge_in(base.clone(), false);
        assert_eq!(purge.paths, std::slice::from_ref(&base));
        purge.run().unwrap();
        assert!(!base.exists());
        assert!(base.parent().unwrap().exists());
        // Nothing there: nothing to purge, and running it is fine.
        let again = purge_in(base.clone(), false);
        assert!(again.paths.is_empty());
        again.run().unwrap();
    }

    #[test]
    fn purging_a_user_chosen_directory_removes_only_project_directories() {
        let base = scratch("shared");
        for name in [
            "0123456789abcdef",
            "fedcba9876543210",
            "0123456789ABCDEF",
            "0123456789abcde",
            "0123456789abcdeg",
        ] {
            fs::create_dir_all(base.join(name)).unwrap();
        }
        fs::write(base.join("aaaaaaaaaaaaaaaa"), "a file named like a project").unwrap();
        fs::write(base.join("notes.txt"), "mine").unwrap();
        let purge = purge_in(base.clone(), true);
        assert_eq!(
            purge.paths,
            [base.join("0123456789abcdef"), base.join("fedcba9876543210")]
        );
        purge.run().unwrap();
        assert!(base.join("notes.txt").is_file());
        assert!(base.join("0123456789abcde").is_dir());
        assert!(base.join("0123456789abcdeg").is_dir());
        assert!(base.join("aaaaaaaaaaaaaaaa").is_file());
        assert!(!base.join("0123456789abcdef").exists());
        assert!(!base.join("fedcba9876543210").exists());
    }

    #[test]
    fn a_user_chosen_directory_that_ends_up_empty_goes_too_but_an_untouched_one_stays() {
        let base = scratch("empty");
        fs::create_dir_all(base.join("0123456789abcdef")).unwrap();
        purge_in(base.clone(), true).run().unwrap();
        assert!(!base.exists());
        let base = scratch("untouched");
        let purge = purge_in(base.clone(), true);
        assert!(purge.paths.is_empty());
        purge.run().unwrap();
        assert!(base.is_dir());
        assert!(purge_in(base.join("missing"), true).paths.is_empty());
    }

    #[test]
    fn a_failing_delete_is_an_error() {
        let base = scratch("fail");
        let purge = CachePurge {
            base: base.clone(),
            paths: vec![base.join("nope")],
            shared: false,
        };
        assert!(purge.run().is_err());
    }

    #[test]
    fn the_public_entry_point_follows_the_environment() {
        let base = cache_base(|key| std::env::var_os(key), Platform::current());
        assert_eq!(cache_purge().is_some(), base.is_some());
        if let Some(purge) = cache_purge() {
            let explicit =
                std::env::var_os("HOOKAH_CACHE_DIR").is_some_and(|v| Path::new(&v).is_absolute());
            assert_eq!(purge.shared, explicit);
            assert!(purge.paths.iter().all(|p| p.starts_with(&purge.base)));
        }
    }
}
