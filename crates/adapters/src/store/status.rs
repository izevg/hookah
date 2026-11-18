//! `StatusStore` on disk.
//!
//! One JSON file per project: `<project cache dir>/status.json`, replaced on every run:
//!
//! ```json
//! {"v":1,"at_ms":1760000000000,"phase":"stop","standing":"fail","problems":3,
//!  "detail":null,"lines":["src/a.rs:4: message"]}
//! ```
//!
//! The record is written to a temporary file and renamed into place, so a reader never sees half
//! a file. A file that cannot be read, parsed, or understood (another version, an unknown name)
//! counts as absent. Concurrent runs overwrite each other: the last one wins, which is the
//! meaning of "latest".

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use hookah_application::error::PortError;
use hookah_application::ports::StatusStore;
use hookah_domain::session::Phase;
use hookah_domain::status::{RecordedStatus, Standing, Status};
use serde::{Deserialize, Serialize};

use super::cache_dir::project_cache_dir;

const VERSION: u32 = 1;
const FILE: &str = "status.json";

#[derive(Serialize, Deserialize)]
struct Record {
    v: u32,
    at_ms: u64,
    phase: String,
    standing: String,
    problems: usize,
    detail: Option<String>,
    lines: Vec<String>,
}

/// Stores statuses as files in the project cache directory.
#[derive(Debug, Clone, Default)]
pub struct FileStatusStore {
    /// Use this directory for every project instead of the cache directory (tests).
    fixed: Option<PathBuf>,
}

impl FileStatusStore {
    /// A store under [`project_cache_dir`].
    pub fn new() -> Self {
        Self::default()
    }

    /// A store that keeps its file directly in `dir`, whatever the project root.
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            fixed: Some(dir.into()),
        }
    }

    fn dir(&self, root: &Path) -> Result<PathBuf, PortError> {
        match &self.fixed {
            Some(dir) => Ok(dir.clone()),
            None => project_cache_dir(root),
        }
    }
}

impl StatusStore for FileStatusStore {
    fn record(&self, root: &Path, status: &Status) -> Result<(), PortError> {
        let dir = self.dir(root)?;
        fs::create_dir_all(&dir)
            .map_err(|e| PortError::new(format!("cannot create {}: {e}", dir.display())))?;
        write_in(&dir, status, now_ms())
    }

    fn latest(&self, root: &Path) -> Option<RecordedStatus> {
        read_in(&self.dir(root).ok()?)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn write_in(dir: &Path, status: &Status, at_ms: u64) -> Result<(), PortError> {
    let record = Record {
        v: VERSION,
        at_ms,
        phase: status.phase.name().to_owned(),
        standing: status.standing.name().to_owned(),
        problems: status.problems,
        detail: status.detail.clone(),
        lines: status.lines.clone(),
    };
    let text = serde_json::to_string(&record)
        .map_err(|e| PortError::new(format!("cannot encode the status: {e}")))?;
    let tmp = dir.join(format!("{FILE}.{}.tmp", std::process::id()));
    let result = fs::File::create(&tmp)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .and_then(|()| fs::rename(&tmp, dir.join(FILE)));
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(PortError::new(format!("cannot write the status: {e}")));
    }
    Ok(())
}

fn read_in(dir: &Path) -> Option<RecordedStatus> {
    let text = fs::read_to_string(dir.join(FILE)).ok()?;
    let record: Record = serde_json::from_str(&text).ok()?;
    if record.v != VERSION {
        return None;
    }
    Some(RecordedStatus {
        at_ms: record.at_ms,
        status: Status {
            phase: Phase::from_name(&record.phase)?,
            standing: Standing::from_name(&record.standing)?,
            problems: record.problems,
            detail: record.detail,
            lines: record.lines,
        },
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn dir(name: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hookah-status-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn failing() -> Status {
        Status {
            phase: Phase::Stop,
            standing: Standing::Fail,
            problems: 3,
            detail: None,
            lines: vec!["src/a.rs:4: message".into()],
        }
    }

    #[test]
    fn a_status_round_trips_with_its_time() {
        let d = dir("round");
        let store = FileStatusStore::in_dir(&d);
        assert_eq!(store.latest(Path::new("/p")), None);
        store.record(Path::new("/p"), &failing()).unwrap();
        let got = store.latest(Path::new("/p")).unwrap();
        assert_eq!(got.status, failing());
        assert!(got.at_ms > 1_600_000_000_000);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn the_latest_record_replaces_the_previous_one() {
        let d = dir("replace");
        let store = FileStatusStore::in_dir(&d);
        store.record(Path::new("/p"), &failing()).unwrap();
        let pass = Status {
            phase: Phase::Edit,
            standing: Standing::Degraded,
            problems: 0,
            detail: Some("slow".into()),
            lines: vec![],
        };
        store.record(Path::new("/p"), &pass).unwrap();
        assert_eq!(store.latest(Path::new("/p")).unwrap().status, pass);
        // No temporary file is left behind.
        let names: Vec<_> = fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [FILE]);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn unreadable_or_foreign_records_count_as_absent() {
        let d = dir("bad");
        let store = FileStatusStore::in_dir(&d);
        let good = r#"{"v":1,"at_ms":1,"phase":"stop","standing":"pass","problems":0,"detail":null,"lines":[]}"#;
        for (text, present) in [
            (good, true),
            ("not json", false),
            (&good.replace("\"v\":1", "\"v\":2"), false),
            (&good.replace("\"stop\"", "\"nap\""), false),
            (&good.replace("\"pass\"", "\"meh\""), false),
        ] {
            fs::write(d.join(FILE), text).unwrap();
            assert_eq!(store.latest(Path::new("/p")).is_some(), present, "{text}");
        }
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn an_unwritable_directory_is_an_error() {
        let d = dir("file");
        let file = d.join("occupied");
        fs::write(&file, "x").unwrap();
        let store = FileStatusStore::in_dir(file.join("sub"));
        assert!(store.record(Path::new("/p"), &failing()).is_err());
        let _ = fs::remove_dir_all(d);
    }
}
