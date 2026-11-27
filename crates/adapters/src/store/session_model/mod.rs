//! Per-session model state persisted between independent hook processes.
//!
//! Some harnesses (notably Claude Code) report `model` on `SessionStart` and `PostModelSwitch`
//! while omitting it on `PostToolUse` and `Stop`, where transcript writes can also lag slightly
//! behind the hook invocation. Recording the session's model in the project cache directory lets
//! subsequent hooks in the same session resolve the active model reliably without mixing sessions
//! or subagents. Everything fails open: cache errors are silently ignored.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::cache_dir::{fnv1a, project_cache_dir};
use crate::transcript;

/// Stored model record for one `(cwd, session_id)` pair.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
struct Record {
    model: String,
    at_ms: u128,
    /// True when written by an explicit mid-session switch (`PostModelSwitch`), so it takes
    /// precedence over a transcript whose modification time predates the switch.
    from_switch: bool,
}

/// Records `model` for `(cwd, session)` in the project cache. Silently ignores any error.
pub fn record(cwd: &Path, session: &str, model: &str, from_switch: bool) {
    let model = model.trim();
    if session.is_empty() || model.is_empty() {
        return;
    }
    let Ok(path) = record_path(cwd, session) else {
        return;
    };
    let _ = write_record(&path, model, now_ms(), from_switch);
}

/// Resolves the model for a hook event that did not include `model` directly.
///
/// - When `use_session_cache` is true (main session, not a separate subagent), checks any stored
///   `SessionStart` / `PostModelSwitch` record for `(cwd, session)`.
/// - An explicit `PostModelSwitch` record that is at least as new as `transcript`'s modification
///   time beats a potentially stale asynchronous transcript write.
/// - Otherwise reads the latest assistant entry from `transcript`, falling back to any stored
///   session record when the transcript is missing or has no assistant message yet.
pub fn resolve(
    cwd: &Path,
    session: Option<&str>,
    transcript_path: Option<&Path>,
    use_session_cache: bool,
) -> Option<String> {
    let cached = session
        .filter(|s| use_session_cache && !s.is_empty())
        .and_then(|s| record_path(cwd, s).ok())
        .and_then(|p| read_record(&p));

    if let Some(ref rec) = cached
        && rec.from_switch
        && transcript_is_not_newer(transcript_path, rec.at_ms)
    {
        return Some(rec.model.clone());
    }

    if let Some(model) = transcript_path.and_then(transcript::latest_model) {
        return Some(model);
    }

    cached.map(|rec| rec.model)
}

fn record_path(cwd: &Path, session: &str) -> Result<PathBuf, hookah_application::error::PortError> {
    let dir = project_cache_dir(cwd)?.join("session-models");
    let key = fnv1a(session.as_bytes());
    Ok(dir.join(format!("{key:016x}.json")))
}

fn write_record(path: &Path, model: &str, at_ms: u128, from_switch: bool) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let record = Record {
        model: model.to_owned(),
        at_ms,
        from_switch,
    };
    let bytes = serde_json::to_vec(&record)?;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let saved = fs::write(&tmp, bytes).and_then(|()| fs::rename(&tmp, path));
    if saved.is_err() {
        let _ = fs::remove_file(tmp);
    }
    saved
}

fn read_record(path: &Path) -> Option<Record> {
    let bytes = fs::read(path).ok()?;
    let record: Record = serde_json::from_slice(&bytes).ok()?;
    let trimmed = record.model.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(Record {
        model: trimmed.to_owned(),
        ..record
    })
}

fn transcript_is_not_newer(transcript_path: Option<&Path>, switch_at_ms: u128) -> bool {
    let Some(path) = transcript_path else {
        return true;
    };
    let Some(mtime_ms) = fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
    else {
        return true;
    };
    mtime_ms <= switch_at_ms
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hookah-session-model-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn switch_record_beats_stale_transcript_while_newer_transcript_wins_over_start() {
        let dir = scratch("precedence");
        let rec_path = dir.join("rec.json");
        let transcript = dir.join("t.jsonl");
        fs::write(
            &transcript,
            r#"{"type":"assistant","message":{"model":"claude-sonnet-4-6","content":[]}}"#,
        )
        .unwrap();
        let t_mtime = fs::metadata(&transcript)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();

        // Explicit switch at or after transcript mtime wins over stale transcript.
        write_record(&rec_path, "claude-opus-4-6", t_mtime + 1000, true).unwrap();
        let rec = read_record(&rec_path).unwrap();
        assert!(rec.from_switch);
        assert!(transcript_is_not_newer(Some(&transcript), rec.at_ms));

        // Switch older than transcript mtime yields to the newer transcript.
        write_record(
            &rec_path,
            "claude-haiku-4-5",
            t_mtime.saturating_sub(5000),
            true,
        )
        .unwrap();
        let older = read_record(&rec_path).unwrap();
        assert!(!transcript_is_not_newer(Some(&transcript), older.at_ms));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn falls_back_to_cached_session_start_when_transcript_is_empty_and_keeps_subagents_separate() {
        let dir = scratch("fallback");
        let empty_transcript = dir.join("empty.jsonl");
        fs::write(&empty_transcript, "").unwrap();

        let rec_path = record_path(&dir, "ses-1").unwrap();
        write_record(&rec_path, "claude-sonnet-4-6", now_ms(), false).unwrap();

        // Main session falls back to SessionStart record.
        assert_eq!(
            resolve(&dir, Some("ses-1"), Some(&empty_transcript), true).as_deref(),
            Some("claude-sonnet-4-6")
        );
        // Subagent does not borrow the parent session's model.
        assert_eq!(
            resolve(&dir, Some("ses-1"), Some(&empty_transcript), false),
            None
        );

        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_file(rec_path);
    }
}
