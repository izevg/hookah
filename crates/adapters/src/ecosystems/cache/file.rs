//! Cache files: small JSON documents, written atomically.

use std::fs;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::fingerprint::Input;

/// Bumped when the stored layout changes; an entry of another schema is a miss.
pub const SCHEMA: u32 = 1;

/// One cache file.
#[derive(Debug, Serialize, Deserialize)]
pub struct Entry<T> {
    /// Layout version, see [`SCHEMA`].
    pub schema: u32,
    /// What the entry is for: the project root. Guards against a hash collision of two roots.
    pub key: String,
    /// Fingerprint of the fixed inputs followed by [`Entry::derived`].
    pub fingerprint: String,
    /// Inputs that only the computation could name, such as the other members of a workspace.
    pub derived: Vec<Input>,
    /// The cached value.
    pub value: T,
}

/// Reads the entry at `path`. A missing, unreadable, or corrupt file is `None`.
pub fn read<T: DeserializeOwned>(path: &Path) -> Option<Entry<T>> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Writes `entry` to `path` through a temporary file in the same directory and a rename, so a
/// reader never sees half a file. Failure is ignored: a cache that cannot be written is only a
/// cache that does not help.
pub fn write<T: Serialize>(path: &Path, entry: &Entry<&T>) {
    let Ok(bytes) = serde_json::to_vec(entry) else {
        return;
    };
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    if fs::write(&tmp, bytes).is_err() || fs::rename(&tmp, path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::fingerprint::tests::Dir;
    use super::*;

    fn entry(value: &Vec<String>) -> Entry<&Vec<String>> {
        Entry {
            schema: SCHEMA,
            key: "/root".into(),
            fingerprint: "abc".into(),
            derived: vec![Input::File("/x".into())],
            value,
        }
    }

    #[test]
    fn an_entry_round_trips() {
        let d = Dir::new("file");
        let path = d.0.join("e.json");
        let value = vec!["a".to_string()];
        write(&path, &entry(&value));
        let back: Entry<Vec<String>> = read(&path).unwrap();
        assert_eq!(back.schema, SCHEMA);
        assert_eq!(back.key, "/root");
        assert_eq!(back.fingerprint, "abc");
        assert_eq!(back.derived, [Input::File("/x".into())]);
        assert_eq!(back.value, value);
    }

    #[test]
    fn writing_replaces_the_file_and_leaves_no_temporary_one() {
        let d = Dir::new("file-replace");
        let path = d.0.join("e.json");
        write(&path, &entry(&vec!["one".to_string()]));
        write(&path, &entry(&vec!["two".to_string()]));
        let back: Entry<Vec<String>> = read(&path).unwrap();
        assert_eq!(back.value, ["two"]);
        let names: Vec<_> = fs::read_dir(&d.0).unwrap().flatten().collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn missing_and_corrupt_files_read_as_none() {
        let d = Dir::new("file-bad");
        assert!(read::<Vec<String>>(&d.0.join("nope.json")).is_none());
        let corrupt = d.write("corrupt.json", "{ not json");
        assert!(read::<Vec<String>>(&corrupt).is_none());
        let wrong = d.write("wrong.json", r#"{"schema":1,"key":"k"}"#);
        assert!(read::<Vec<String>>(&wrong).is_none());
    }

    #[test]
    fn a_failed_write_is_silent_and_cleans_up() {
        let d = Dir::new("file-fail");
        // The target is a directory: the rename fails after the temporary file was written.
        let target = d.0.join("e.json");
        fs::create_dir(&target).unwrap();
        write(&target, &entry(&vec![]));
        assert!(target.is_dir());
        assert_eq!(
            fs::read_dir(&d.0).unwrap().count(),
            1,
            "no temporary file left"
        );
        // The directory does not exist at all.
        write(&d.0.join("missing/e.json"), &entry(&vec![]));
        assert!(!d.0.join("missing").exists());
    }
}
