//! `UpdateStore` on disk, in `<hookah cache>/update/`:
//!
//! - `state.json`: what the updater remembers, replaced atomically (temporary file, rename).
//!   `{"v":1,"checked_at_ms":..,"last_ok":true,"latest":"1.2.3","error":null,"failures":0,
//!   "announced":null,"pending":{"kind":"updated","from":"1.2.2","to":"1.2.3"}}`
//! - `lock`: held with an operating-system advisory lock while an updater runs. The system
//!   releases it when the process exits, however it exits, so no stale lock can block updates.
//!
//! A state file that cannot be read or understood counts as absent.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

use hookah_application::ports::{LockGuard, UpdateError, UpdateStore};
use hookah_domain::update::{Event, UpdateState};
use serde::{Deserialize, Serialize};

use crate::store::cache_dir::update_cache_dir;

const VERSION: u32 = 1;
const STATE_FILE: &str = "state.json";
const LOCK_FILE: &str = "lock";

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Record {
    v: u32,
    checked_at_ms: Option<u64>,
    last_ok: bool,
    latest: Option<String>,
    error: Option<String>,
    failures: u32,
    announced: Option<String>,
    pending: Option<PendingRecord>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PendingRecord {
    Updated { from: String, to: String },
    Available { version: String },
    Failed { reason: String },
}

impl From<&UpdateState> for Record {
    fn from(state: &UpdateState) -> Self {
        Self {
            v: VERSION,
            checked_at_ms: state.checked_at_ms,
            last_ok: state.last_ok,
            latest: state.latest.clone(),
            error: state.error.clone(),
            failures: state.failures,
            announced: state.announced.clone(),
            pending: state.pending.as_ref().map(|event| match event {
                Event::Updated { from, to } => PendingRecord::Updated {
                    from: from.clone(),
                    to: to.clone(),
                },
                Event::Available { version } => PendingRecord::Available {
                    version: version.clone(),
                },
                Event::Failed { reason } => PendingRecord::Failed {
                    reason: reason.clone(),
                },
            }),
        }
    }
}

impl From<Record> for UpdateState {
    fn from(record: Record) -> Self {
        Self {
            checked_at_ms: record.checked_at_ms,
            last_ok: record.last_ok,
            latest: record.latest,
            error: record.error,
            failures: record.failures,
            announced: record.announced,
            pending: record.pending.map(|pending| match pending {
                PendingRecord::Updated { from, to } => Event::Updated { from, to },
                PendingRecord::Available { version } => Event::Available { version },
                PendingRecord::Failed { reason } => Event::Failed { reason },
            }),
        }
    }
}

/// Keeps the updater's state in a directory.
#[derive(Debug, Clone)]
pub struct FileUpdateStore {
    /// `None` until first use: resolving the cache directory creates it.
    dir: Option<PathBuf>,
}

impl FileUpdateStore {
    /// A store under the hookah cache directory, resolved on first use.
    pub fn new() -> Self {
        Self { dir: None }
    }

    /// A store that keeps its files directly in `dir`.
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: Some(dir.into()),
        }
    }

    fn dir(&self) -> Result<PathBuf, UpdateError> {
        match &self.dir {
            Some(dir) => Ok(dir.clone()),
            None => update_cache_dir().map_err(|e| UpdateError::failed(e.message())),
        }
    }
}

impl Default for FileUpdateStore {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdateStore for FileUpdateStore {
    fn load(&self) -> UpdateState {
        let Ok(dir) = self.dir() else {
            return UpdateState::default();
        };
        fs::read_to_string(dir.join(STATE_FILE))
            .ok()
            .and_then(|text| serde_json::from_str::<Record>(&text).ok())
            .filter(|record| record.v == VERSION)
            .map(UpdateState::from)
            .unwrap_or_default()
    }

    fn save(&self, state: &UpdateState) -> Result<(), UpdateError> {
        let dir = self.dir()?;
        let json = serde_json::to_string(&Record::from(state))
            .map_err(|e| UpdateError::failed(format!("cannot encode the update state: {e}")))?;
        write_atomically(&dir, &json)
            .map_err(|e| UpdateError::failed(format!("cannot write the update state: {e}")))
    }

    fn try_lock(&self) -> Result<Option<LockGuard>, UpdateError> {
        let dir = self.dir()?;
        let path = dir.join(LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| UpdateError::failed(format!("cannot open {}: {e}", path.display())))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Box::new(Held { _file: file }))),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => Err(UpdateError::failed(format!(
                "cannot lock {}: {e}",
                path.display()
            ))),
        }
    }
}

/// The lock file, locked for as long as this lives.
struct Held {
    _file: File,
}

fn write_atomically(dir: &Path, text: &str) -> std::io::Result<()> {
    let tmp = dir.join(format!("{STATE_FILE}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, dir.join(STATE_FILE))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use hookah_domain::update::Version;

    use super::*;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hookah-upd-store-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn full_state() -> UpdateState {
        let v = |t: &str| Version::parse(t).unwrap();
        let mut s = UpdateState::default();
        s.record_failure(5, "offline");
        s.record_checked(10, &v("1.2.3"));
        s.record_available(&v("1.2.3"));
        s.error = Some("boom".into());
        s.failures = 2;
        s
    }

    #[test]
    fn a_state_round_trips_with_every_pending_kind() {
        let dir = scratch();
        let store = FileUpdateStore::in_dir(&dir);
        let v = |t: &str| Version::parse(t).unwrap();
        let mut states = vec![UpdateState::default(), full_state()];
        let mut updated = full_state();
        updated.record_updated(&v("1.0.0"), &v("1.2.3"));
        states.push(updated);
        let mut failed = UpdateState::default();
        failed.record_blocked(1, &v("2.0.0"), "read-only");
        states.push(failed);
        for state in states {
            store.save(&state).unwrap();
            assert_eq!(store.load(), state);
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_or_unreadable_record_is_the_default() {
        let dir = scratch();
        let store = FileUpdateStore::in_dir(&dir);
        assert_eq!(store.load(), UpdateState::default());
        fs::write(dir.join(STATE_FILE), "not json").unwrap();
        assert_eq!(store.load(), UpdateState::default());
        fs::write(dir.join(STATE_FILE), r#"{"v":2,"checked_at_ms":5}"#).unwrap();
        assert_eq!(store.load(), UpdateState::default(), "another version");
        fs::write(
            dir.join(STATE_FILE),
            r#"{"v":1,"checked_at_ms":5,"future":true}"#,
        )
        .unwrap();
        assert_eq!(store.load().checked_at_ms, Some(5));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn saving_leaves_only_the_state_file() {
        let dir = scratch();
        let store = FileUpdateStore::in_dir(&dir);
        store.save(&full_state()).unwrap();
        store.save(&full_state()).unwrap();
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [STATE_FILE]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unwritable_directory_is_an_error_not_a_panic() {
        let dir = scratch().join("missing");
        let store = FileUpdateStore::in_dir(&dir);
        assert!(store.save(&UpdateState::default()).is_err());
        assert!(store.try_lock().is_err());
        assert_eq!(store.load(), UpdateState::default());
    }

    #[test]
    fn the_lock_is_exclusive_and_released_on_drop() {
        let dir = scratch();
        let store = FileUpdateStore::in_dir(&dir);
        let first = store.try_lock().unwrap().expect("free lock");
        assert!(store.try_lock().unwrap().is_none(), "held");
        // A second store (another process, in effect) is locked out too.
        assert!(FileUpdateStore::in_dir(&dir).try_lock().unwrap().is_none());
        drop(first);
        // A process another test spawns at this moment holds a duplicate of the descriptor
        // between its fork and exec, which keeps the lock for that instant: retry briefly.
        let freed = (0..200).any(|_| {
            if store.try_lock().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            false
        });
        assert!(freed, "released on drop");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_default_store_resolves_the_cache_directory_lazily() {
        let store = FileUpdateStore::default();
        assert!(store.dir.is_none());
    }
}
