//! `BaselineStore` on disk.
//!
//! One JSON file per (project, session): `<project cache dir>/baselines/<session>.json`, where
//! the project cache dir is [`project_cache_dir`]. The file holds one of three records:
//!
//! ```json
//! {"v":1,"status":"in_progress"}
//! {"v":1,"status":"failed","reason":"..."}
//! {"v":1,"status":"ready","counts":{"<fingerprint>":2}}
//! ```
//!
//! - Claiming a baseline creates the file exclusively, so only one job runs per pair.
//! - Results are written to a temporary file and renamed over the record, so a reader never sees
//!   half a file.
//! - A file that cannot be read or parsed counts as absent (and may be claimed again).
//! - An `in_progress` record older than [`STALE_AFTER`] belongs to a job that died; it reads as
//!   failed.
//! - Records older than [`MAX_AGE`] are deleted whenever a baseline is claimed.
//! - The session id becomes the file name when it is short and made of `[A-Za-z0-9_-]`; any other
//!   id is replaced by `h` and a hash of it, so an id can never escape the directory.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use hookah_application::error::PortError;
use hookah_application::ports::{BaselineState, BaselineStore};
use hookah_domain::finding::baseline::Baseline;
use hookah_domain::finding::fingerprint::Fingerprint;
use serde::{Deserialize, Serialize};

use super::cache_dir::project_cache_dir;

/// Records older than this are pruned.
pub const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// An `in_progress` record older than this is a dead job.
pub const STALE_AFTER: Duration = Duration::from_secs(30 * 60);
const VERSION: u32 = 1;
const MAX_ID_LEN: usize = 64;

#[derive(Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Record {
    InProgress {
        v: u32,
    },
    Failed {
        v: u32,
        reason: String,
    },
    Ready {
        v: u32,
        counts: BTreeMap<String, u32>,
    },
}

/// Stores baselines as files in the project cache directory.
#[derive(Debug, Clone, Default)]
pub struct FileBaselineStore {
    /// Use this directory for every project instead of the cache directory (tests).
    fixed: Option<PathBuf>,
}

impl FileBaselineStore {
    /// A store under [`project_cache_dir`].
    pub fn new() -> Self {
        Self::default()
    }

    /// A store that keeps its files directly in `dir`, whatever the project root.
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            fixed: Some(dir.into()),
        }
    }

    fn dir(&self, root: &Path) -> Result<PathBuf, PortError> {
        match &self.fixed {
            Some(dir) => Ok(dir.clone()),
            None => Ok(project_cache_dir(root)?.join("baselines")),
        }
    }
}

impl BaselineStore for FileBaselineStore {
    fn state(&self, root: &Path, session: &str) -> BaselineState {
        match self.dir(root) {
            Ok(dir) => state_in(&dir, session, SystemTime::now()),
            Err(_) => BaselineState::Absent,
        }
    }

    fn begin(&self, root: &Path, session: &str) -> Result<bool, PortError> {
        let dir = self.dir(root)?;
        prune_in(&dir, SystemTime::now(), MAX_AGE);
        begin_in(&dir, session)
    }

    fn save(&self, root: &Path, session: &str, baseline: &Baseline) -> Result<(), PortError> {
        let counts = baseline
            .counts()
            .map(|(f, c)| (f.as_str().to_string(), c))
            .collect();
        write_in(
            &self.dir(root)?,
            session,
            &Record::Ready { v: VERSION, counts },
        )
    }

    fn fail(&self, root: &Path, session: &str, reason: &str) -> Result<(), PortError> {
        let record = Record::Failed {
            v: VERSION,
            reason: reason.to_string(),
        };
        write_in(&self.dir(root)?, session, &record)
    }
}

/// The record file name for a session id.
fn file_name(session: &str) -> String {
    let safe = !session.is_empty()
        && session.len() <= MAX_ID_LEN
        && session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if safe {
        format!("{session}.json")
    } else {
        format!("h{:016x}.json", fnv1a(session.as_bytes()))
    }
}

/// 64-bit FNV-1a: stable across toolchain upgrades, unlike `std`'s hasher.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn read_record(path: &Path) -> Option<(Record, SystemTime)> {
    let meta = fs::metadata(path).ok()?;
    let text = fs::read_to_string(path).ok()?;
    let record = serde_json::from_str(&text).ok()?;
    Some((record, meta.modified().ok()?))
}

fn state_in(dir: &Path, session: &str, now: SystemTime) -> BaselineState {
    let Some((record, modified)) = read_record(&dir.join(file_name(session))) else {
        return BaselineState::Absent;
    };
    match record {
        Record::InProgress { .. } => {
            let age = now.duration_since(modified).unwrap_or_default();
            if age > STALE_AFTER {
                BaselineState::Failed("the baseline job did not finish".into())
            } else {
                BaselineState::InProgress
            }
        }
        Record::Failed { reason, .. } => BaselineState::Failed(reason),
        Record::Ready { counts, .. } => BaselineState::Ready(Baseline::from_counts(
            counts
                .into_iter()
                .map(|(f, c)| (Fingerprint::from_stored(f), c)),
        )),
    }
}

fn create_dir(dir: &Path) -> Result<(), PortError> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(dir)
        .map_err(|e| PortError::new(format!("cannot create {}: {e}", dir.display())))
}

/// Claims the record: `true` when this call created it.
fn begin_in(dir: &Path, session: &str) -> Result<bool, PortError> {
    create_dir(dir)?;
    let path = dir.join(file_name(session));
    let text = encode(&Record::InProgress { v: VERSION })?;
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            file.write_all(text.as_bytes())
                .map_err(|e| PortError::new(format!("cannot write {}: {e}", path.display())))?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if read_record(&path).is_some() {
                return Ok(false);
            }
            // Unreadable record: take it over.
            write_in(dir, session, &Record::InProgress { v: VERSION })?;
            Ok(true)
        }
        Err(e) => Err(PortError::new(format!(
            "cannot create {}: {e}",
            path.display()
        ))),
    }
}

fn encode(record: &Record) -> Result<String, PortError> {
    serde_json::to_string(record).map_err(|e| PortError::new(format!("cannot encode: {e}")))
}

/// Writes the record atomically: temporary file in the same directory, then rename.
fn write_in(dir: &Path, session: &str, record: &Record) -> Result<(), PortError> {
    create_dir(dir)?;
    let name = file_name(session);
    let tmp = dir.join(format!("{name}.{}.tmp", std::process::id()));
    let target = dir.join(&name);
    let text = encode(record)?;
    let result = File::create(&tmp)
        .and_then(|mut f| f.write_all(text.as_bytes()).and_then(|()| f.sync_all()))
        .and_then(|()| fs::rename(&tmp, &target));
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(PortError::new(format!(
            "cannot write {}: {e}",
            target.display()
        )));
    }
    Ok(())
}

/// Deletes the files in `dir` last modified more than `max_age` before `now`.
fn prune_in(dir: &Path, now: SystemTime, max_age: Duration) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .ok()
            .filter(|m| m.is_file())
            .and_then(|m| m.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > max_age);
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use hookah_domain::finding::{Finding, Location, Severity};
    use hookah_domain::toolchain::CheckKind;

    use super::*;

    struct Temp(PathBuf);

    impl Temp {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "hookah-baseline-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sample() -> Baseline {
        let f = |line| {
            Finding::new(
                CheckKind::Lint,
                Severity::Error,
                Location::line("a.rs", line),
                "m",
            )
        };
        Baseline::from_findings([&f(1), &f(2)])
    }

    fn age(path: &Path, by: Duration) {
        let file = File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - by).unwrap();
    }

    fn root() -> &'static Path {
        Path::new("/any/root")
    }

    #[test]
    fn absent_until_claimed_then_in_progress_then_ready() {
        let t = Temp::new("life");
        let store = FileBaselineStore::in_dir(&t.0);
        assert_eq!(store.state(root(), "s1"), BaselineState::Absent);
        assert!(store.begin(root(), "s1").unwrap());
        assert_eq!(store.state(root(), "s1"), BaselineState::InProgress);
        assert!(!store.begin(root(), "s1").unwrap());
        store.save(root(), "s1", &sample()).unwrap();
        assert_eq!(store.state(root(), "s1"), BaselineState::Ready(sample()));
        assert!(!store.begin(root(), "s1").unwrap());
        // Other sessions are independent.
        assert_eq!(store.state(root(), "s2"), BaselineState::Absent);
    }

    #[test]
    fn failure_is_recorded_with_its_reason() {
        let t = Temp::new("fail");
        let store = FileBaselineStore::in_dir(&t.0);
        store.begin(root(), "s").unwrap();
        store.fail(root(), "s", "clippy timed out").unwrap();
        assert_eq!(
            store.state(root(), "s"),
            BaselineState::Failed("clippy timed out".into())
        );
        assert!(!store.begin(root(), "s").unwrap());
    }

    #[test]
    fn writes_are_atomic_and_leave_no_temporary_files() {
        let t = Temp::new("atomic");
        let store = FileBaselineStore::in_dir(&t.0);
        store.save(root(), "s", &sample()).unwrap();
        store.save(root(), "s", &Baseline::default()).unwrap();
        let names: Vec<_> = fs::read_dir(&t.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["s.json"]);
        assert_eq!(
            store.state(root(), "s"),
            BaselineState::Ready(Baseline::default())
        );
    }

    #[test]
    fn a_corrupt_file_is_absent_and_can_be_claimed_again() {
        let t = Temp::new("corrupt");
        let store = FileBaselineStore::in_dir(&t.0);
        fs::create_dir_all(&t.0).unwrap();
        for garbage in ["", "{not json", "{\"status\":\"ready\"}", "[]"] {
            fs::write(t.0.join("s.json"), garbage).unwrap();
            assert_eq!(store.state(root(), "s"), BaselineState::Absent, "{garbage}");
            assert!(store.begin(root(), "s").unwrap(), "{garbage}");
            assert_eq!(store.state(root(), "s"), BaselineState::InProgress);
        }
    }

    #[test]
    fn a_dead_job_reads_as_failed() {
        let t = Temp::new("stale");
        let store = FileBaselineStore::in_dir(&t.0);
        store.begin(root(), "s").unwrap();
        let path = t.0.join("s.json");
        age(&path, STALE_AFTER - Duration::from_secs(60));
        assert_eq!(store.state(root(), "s"), BaselineState::InProgress);
        age(&path, STALE_AFTER + Duration::from_secs(60));
        assert!(matches!(store.state(root(), "s"), BaselineState::Failed(_)));
    }

    #[test]
    fn a_ready_baseline_does_not_go_stale() {
        let t = Temp::new("ready-old");
        let store = FileBaselineStore::in_dir(&t.0);
        store.save(root(), "s", &sample()).unwrap();
        age(&t.0.join("s.json"), STALE_AFTER * 2);
        assert!(matches!(store.state(root(), "s"), BaselineState::Ready(_)));
    }

    #[test]
    fn claiming_prunes_old_records_only() {
        let t = Temp::new("prune");
        let store = FileBaselineStore::in_dir(&t.0);
        store.save(root(), "old", &sample()).unwrap();
        store.save(root(), "fresh", &sample()).unwrap();
        age(&t.0.join("old.json"), MAX_AGE + Duration::from_secs(3600));
        age(&t.0.join("fresh.json"), MAX_AGE - Duration::from_secs(3600));
        store.begin(root(), "new").unwrap();
        assert_eq!(store.state(root(), "old"), BaselineState::Absent);
        assert!(matches!(
            store.state(root(), "fresh"),
            BaselineState::Ready(_)
        ));
        assert_eq!(store.state(root(), "new"), BaselineState::InProgress);
    }

    #[test]
    fn prune_ignores_missing_directories_and_subdirectories() {
        let t = Temp::new("prune-odd");
        prune_in(&t.0, SystemTime::now(), MAX_AGE);
        fs::create_dir_all(t.0.join("sub")).unwrap();
        File::open(t.0.join("sub"))
            .unwrap()
            .set_modified(SystemTime::now() - MAX_AGE * 2)
            .unwrap();
        prune_in(&t.0, SystemTime::now(), MAX_AGE);
        assert!(t.0.join("sub").is_dir());
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("abc-123_X"), "abc-123_X.json");
        let hashed = file_name("../../etc/passwd");
        assert!(hashed.starts_with('h') && hashed.ends_with(".json"));
        assert!(!hashed.contains('/'));
        assert_ne!(file_name("a b"), file_name("a c"));
        assert_eq!(file_name("a b"), file_name("a b"));
        assert!(file_name("").starts_with('h'));
        let max = "a".repeat(MAX_ID_LEN);
        assert_eq!(file_name(&max), format!("{max}.json"));
        assert!(file_name(&"a".repeat(MAX_ID_LEN + 1)).starts_with('h'));
        // FNV-1a test vector.
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn an_unusable_directory_is_an_error_on_write_and_absent_on_read() {
        let t = Temp::new("blocked");
        fs::create_dir_all(&t.0).unwrap();
        let blocker = t.0.join("file");
        fs::write(&blocker, "x").unwrap();
        let store = FileBaselineStore::in_dir(blocker.join("sub"));
        assert!(store.begin(root(), "s").is_err());
        assert!(store.save(root(), "s", &sample()).is_err());
        assert!(store.fail(root(), "s", "r").is_err());
        assert_eq!(store.state(root(), "s"), BaselineState::Absent);
    }

    #[test]
    fn the_default_store_resolves_a_directory_under_the_project_cache() {
        let dir = FileBaselineStore::new().dir(&std::env::temp_dir());
        if let Ok(dir) = dir {
            assert_eq!(dir.file_name().unwrap(), "baselines");
        }
    }
}
