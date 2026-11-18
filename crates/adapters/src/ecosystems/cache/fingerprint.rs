//! Fingerprints of detection inputs.
//!
//! An [`Input`] names a place on disk that detection reads or whose contents decide which tools
//! are available. [`fingerprint`] hashes the observable state of a list of inputs, plus a salt,
//! into a short string. Two equal fingerprints mean that nothing detection depends on changed.
//!
//! The state is cheap to observe, since it never reads file contents: existence, modification
//! time, size, and directory entry names. A change that keeps both the size and the modification
//! time of a file is not seen. Inputs are chosen conservatively by the ecosystems: when it is
//! unclear whether something matters, it is listed.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::store::cache_dir::fnv1a;

/// One place whose state detection depends on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Input {
    /// Only whether the path exists.
    Exists(PathBuf),
    /// A file: existence, modification time, and size.
    File(PathBuf),
    /// The names of the subdirectories of a directory.
    Dirs(PathBuf),
}

impl Input {
    fn path(&self) -> &Path {
        match self {
            Self::Exists(p) | Self::File(p) | Self::Dirs(p) => p,
        }
    }

    /// The observable state of the input, as a string.
    fn state(&self) -> String {
        let path = self.path();
        match self {
            Self::Exists(_) => path.exists().to_string(),
            Self::File(_) => fs::metadata(path).map_or_else(|_| "-".into(), |m| stamp(&m)),
            Self::Dirs(_) => subdirs(path),
        }
    }
}

/// `<mtime in nanoseconds>:<size>`.
fn stamp(meta: &fs::Metadata) -> String {
    let nanos = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    format!("{nanos}:{}", meta.len())
}

/// The sorted names of the subdirectories of `dir`, or `-` when it cannot be read.
fn subdirs(dir: &Path) -> String {
    let Ok(read) = fs::read_dir(dir) else {
        return "-".into();
    };
    let mut names: Vec<String> = read
        .flatten()
        .filter(|e| fs::metadata(e.path()).is_ok_and(|m| m.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names.join("\n")
}

/// A 64-bit hash, as 16 hex digits, of `salt` and the state of every input in order.
pub fn fingerprint(inputs: &[Input], salt: &str) -> String {
    let mut buf = String::from(salt);
    for input in inputs {
        buf.push('\0');
        buf.push_str(&input.path().to_string_lossy());
        buf.push('\0');
        buf.push_str(&input.state());
    }
    format!("{:016x}", fnv1a(buf.as_bytes()))
}

/// `Input::File` for every `dir`/`name` combination, directory by directory.
pub fn files_in(dirs: &[PathBuf], names: &[&str]) -> Vec<Input> {
    dirs.iter()
        .flat_map(|d| names.iter().map(move |n| Input::File(d.join(n))))
        .collect()
}

/// `Input::File` for each executable called one of `names` in each of the `PATH` directories
/// `dirs`. On Windows the usual executable suffixes are tried as well.
pub fn executables_in(dirs: &[PathBuf], names: &[&str]) -> Vec<Input> {
    let suffixes: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    let mut inputs = Vec::new();
    for dir in dirs {
        for name in names {
            for suffix in suffixes {
                inputs.push(Input::File(dir.join(format!("{name}{suffix}"))));
            }
        }
    }
    inputs
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod tests {
    use std::fs::File;
    use std::time::Duration;

    use super::*;

    /// A scratch directory removed on drop.
    pub struct Dir(pub PathBuf);

    impl Dir {
        pub fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("hookah-fp-{tag}-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir.canonicalize().unwrap())
        }

        pub fn write(&self, rel: &str, text: &str) -> PathBuf {
            let path = self.0.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
            path
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Moves the modification time of `path` back by `secs` seconds without touching its size.
    pub fn age(path: &Path, secs: u64) {
        let file = File::options().write(true).open(path).unwrap();
        let time = fs::metadata(path).unwrap().modified().unwrap();
        file.set_modified(time.checked_sub(Duration::from_secs(secs)).unwrap())
            .unwrap();
    }

    fn fp(inputs: &[Input]) -> String {
        fingerprint(inputs, "salt")
    }

    #[test]
    fn a_file_changes_with_its_existence_size_and_mtime() {
        let d = Dir::new("file");
        let path = d.0.join("a.toml");
        let input = [Input::File(path.clone())];
        let missing = fp(&input);
        d.write("a.toml", "x");
        let one = fp(&input);
        assert_ne!(missing, one);
        assert_eq!(one, fp(&input));
        d.write("a.toml", "xy");
        let two = fp(&input);
        assert_ne!(one, two);
        age(&path, 60);
        assert_ne!(two, fp(&input));
        fs::remove_file(&path).unwrap();
        assert_eq!(missing, fp(&input));
    }

    #[test]
    fn exists_only_sees_existence() {
        let d = Dir::new("exists");
        let path = d.write("marker", "x");
        let input = [Input::Exists(path.clone())];
        let before = fp(&input);
        d.write("marker", "longer");
        age(&path, 60);
        assert_eq!(before, fp(&input));
        fs::remove_file(&path).unwrap();
        assert_ne!(before, fp(&input));
    }

    #[test]
    fn dirs_see_subdirectory_names_only() {
        let d = Dir::new("dirs");
        let input = [Input::Dirs(d.0.clone())];
        let empty = fp(&input);
        d.write("file.txt", "x");
        assert_eq!(empty, fp(&input), "files are not listed");
        fs::create_dir(d.0.join("pkg")).unwrap();
        let one = fp(&input);
        assert_ne!(empty, one);
        fs::create_dir(d.0.join("other")).unwrap();
        assert_ne!(one, fp(&input));
        // A missing directory differs from an empty one.
        assert_ne!(empty, fp(&[Input::Dirs(d.0.join("nope"))]));
    }

    #[test]
    fn salt_order_path_and_kind_all_count() {
        let d = Dir::new("salt");
        let a = Input::Exists(d.0.join("a"));
        let b = Input::Exists(d.0.join("b"));
        assert_ne!(
            fingerprint(std::slice::from_ref(&a), "1"),
            fingerprint(std::slice::from_ref(&a), "2")
        );
        assert_ne!(
            fingerprint(&[a.clone(), b.clone()], "s"),
            fingerprint(&[b.clone(), a.clone()], "s")
        );
        assert_ne!(fp(std::slice::from_ref(&a)), fp(&[b]));
        assert_ne!(
            fp(std::slice::from_ref(&a)),
            fp(&[Input::File(d.0.join("a"))])
        );
        assert_ne!(fp(&[]), fp(&[a]));
        assert_eq!(fp(&[]).len(), 16);
    }

    #[test]
    fn executables_are_probed_in_every_path_directory() {
        let found = executables_in(&["/a".into(), "/b".into()], &["x"]);
        let per_dir = if cfg!(windows) { 4 } else { 1 };
        assert_eq!(found.len(), 2 * per_dir);
        assert_eq!(found[0], Input::File(Path::new("/a").join("x")));
        assert_eq!(found[per_dir], Input::File(Path::new("/b").join("x")));
    }

    #[test]
    fn files_in_combines_every_directory_with_every_name() {
        let inputs = files_in(&["/a".into(), "/b".into()], &["x", "y"]);
        assert_eq!(
            inputs,
            [
                Input::File(Path::new("/a").join("x")),
                Input::File(Path::new("/a").join("y")),
                Input::File(Path::new("/b").join("x")),
                Input::File(Path::new("/b").join("y")),
            ]
        );
    }
}
