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

/// Where a resolved model identifier came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelOrigin {
    /// Cached from an earlier session event (`SessionStart`, `PostModelSwitch`, `PreInvocation`).
    SessionCache,
    /// Read from the latest real model entry in the session's JSONL transcript.
    Transcript,
}

/// Resolved model identifier together with its origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModel {
    /// The active model identifier.
    pub model: String,
    /// Which source produced `model`.
    pub origin: ModelOrigin,
}

/// Stored model record for one `(cwd, session_id)` pair.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
struct Record {
    model: String,
    at_ms: u128,
    /// True when written by an explicit mid-session switch (`PostModelSwitch`), so it takes
    /// precedence over a transcript that predates the switch even if no transcript path was
    /// provided at switch time.
    from_switch: bool,
    /// End byte offset of the latest real model entry in the transcript when this record was
    /// written (`Some(0)` if the transcript had no real model entry yet; `None` if no transcript
    /// path was supplied). Trailing `user` or `<synthetic>` entries appended after a switch do
    /// not advance this offset.
    #[serde(default)]
    transcript_model_offset: Option<u64>,
}

/// True if a cached session-model file already exists for `(cwd, session)`.
pub fn exists(cwd: &Path, session: &str) -> bool {
    !session.is_empty() && record_path(cwd, session).is_ok_and(|p| p.is_file())
}

/// Records `model` for `(cwd, session)` in the project cache. Silently ignores any error.
///
/// When `transcript_path` is provided, also snapshots the end byte offset of the latest real model
/// entry currently in that transcript so later trailing non-model entries (`user`, `<synthetic>`)
/// cannot make an older assistant record override this update.
pub fn record(
    cwd: &Path,
    session: &str,
    model: &str,
    from_switch: bool,
    transcript_path: Option<&Path>,
) {
    let model = model.trim();
    if session.is_empty() || model.is_empty() {
        return;
    }
    let Ok(path) = record_path(cwd, session) else {
        return;
    };
    let transcript_model_offset =
        transcript_path.map(|p| transcript::latest_model_entry(p).map_or(0, |e| e.end_offset));
    let _ = write_record(&path, model, now_ms(), from_switch, transcript_model_offset);
}

/// Resolves the model for a hook event that did not include `model` directly.
pub fn resolve(
    cwd: &Path,
    session: Option<&str>,
    transcript_path: Option<&Path>,
    use_session_cache: bool,
) -> Option<String> {
    resolve_with_origin(cwd, session, transcript_path, use_session_cache).map(|r| r.model)
}

/// Resolves the model together with the source that supplied it.
///
/// - When `use_session_cache` is true (main session, not a separate subagent), checks any stored
///   session record for `(cwd, session)`.
/// - A stored record beats a transcript entry when the transcript has not appended any real model
///   record past the offset snapshotted when the record was written (ignoring trailing `user` or
///   `<synthetic>` entries), or for an explicit switch without a transcript snapshot, when the
///   latest real model entry's timestamp (or file mtime if untimestamped) is not newer than the
///   switch.
/// - Otherwise reads the latest real model entry from `transcript`, falling back to any stored
///   session record when the transcript is missing or has no real model entry yet.
pub fn resolve_with_origin(
    cwd: &Path,
    session: Option<&str>,
    transcript_path: Option<&Path>,
    use_session_cache: bool,
) -> Option<ResolvedModel> {
    let cached = session
        .filter(|s| use_session_cache && !s.is_empty())
        .and_then(|s| record_path(cwd, s).ok())
        .and_then(|p| read_record(&p));

    let entry = transcript_path.and_then(transcript::latest_model_entry);

    match (cached, entry) {
        (Some(rec), Some(entry)) => {
            if record_supersedes_entry(&rec, &entry, transcript_path) {
                Some(ResolvedModel {
                    model: rec.model,
                    origin: ModelOrigin::SessionCache,
                })
            } else {
                Some(ResolvedModel {
                    model: entry.model,
                    origin: ModelOrigin::Transcript,
                })
            }
        }
        (Some(rec), None) => Some(ResolvedModel {
            model: rec.model,
            origin: ModelOrigin::SessionCache,
        }),
        (None, Some(entry)) => Some(ResolvedModel {
            model: entry.model,
            origin: ModelOrigin::Transcript,
        }),
        (None, None) => None,
    }
}

fn record_supersedes_entry(
    rec: &Record,
    entry: &transcript::TranscriptModel,
    transcript_path: Option<&Path>,
) -> bool {
    if let Some(snapshot_offset) = rec.transcript_model_offset {
        let truncated = transcript_path
            .and_then(|p| fs::metadata(p).ok())
            .is_some_and(|m| m.len() < snapshot_offset);
        return !truncated && entry.end_offset <= snapshot_offset;
    }
    if !rec.from_switch {
        return false;
    }
    if let Some(entry_ts) = entry.timestamp_ms {
        return entry_ts <= rec.at_ms;
    }
    transcript_is_not_newer(transcript_path, rec.at_ms)
}

fn record_path(cwd: &Path, session: &str) -> Result<PathBuf, hookah_application::error::PortError> {
    let dir = project_cache_dir(cwd)?.join("session-models");
    let key = fnv1a(session.as_bytes());
    Ok(dir.join(format!("{key:016x}.json")))
}

fn write_record(
    path: &Path,
    model: &str,
    at_ms: u128,
    from_switch: bool,
    transcript_model_offset: Option<u64>,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let record = Record {
        model: model.to_owned(),
        at_ms,
        from_switch,
        transcript_model_offset,
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
    fn switch_record_beats_trailing_non_model_entries_until_a_new_assistant_turn_arrives() {
        let dir = scratch("precedence");
        let transcript = dir.join("t.jsonl");
        let old_assistant = concat!(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"model":"claude-opus-4-6","content":[]}}"#,
            "\n"
        );
        fs::write(&transcript, old_assistant).unwrap();

        // Explicit switch snapshots the transcript's latest real model offset.
        record(
            &dir,
            "ses-offset",
            "claude-sonnet-4-6",
            true,
            Some(&transcript),
        );
        assert!(exists(&dir, "ses-offset"));

        // Appending user and <synthetic> entries bumps the file's mtime, yet the switch still wins.
        let mut content = old_assistant.to_owned();
        content.push_str(
            r#"{"type":"user","timestamp":"2026-01-01T00:01:00Z","message":{"role":"user","content":"hello"}}"#,
        );
        content.push('\n');
        content.push_str(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:01:01Z","message":{"model":"<synthetic>","content":[]}}"#,
        );
        content.push('\n');
        fs::write(&transcript, &content).unwrap();
        let resolved = resolve_with_origin(&dir, Some("ses-offset"), Some(&transcript), true);
        assert_eq!(
            resolved,
            Some(ResolvedModel {
                model: "claude-sonnet-4-6".into(),
                origin: ModelOrigin::SessionCache,
            })
        );

        // Once a real assistant turn is appended after the switch, the transcript takes over.
        content.push_str(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:02:00Z","message":{"model":"claude-haiku-4-5","content":[]}}"#,
        );
        content.push('\n');
        fs::write(&transcript, &content).unwrap();
        let resolved = resolve_with_origin(&dir, Some("ses-offset"), Some(&transcript), true);
        assert_eq!(
            resolved,
            Some(ResolvedModel {
                model: "claude-haiku-4-5".into(),
                origin: ModelOrigin::Transcript,
            })
        );

        if let Ok(path) = record_path(&dir, "ses-offset") {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn switch_without_transcript_snapshot_compares_assistant_entry_timestamp() {
        let dir = scratch("timestamp");
        let rec_path = record_path(&dir, "ses-ts").unwrap();
        let transcript = dir.join("t.jsonl");
        // Old assistant entry followed by a brand-new user entry that bumps file mtime.
        fs::write(
            &transcript,
            concat!(
                r#"{"type":"assistant","timestamp":"2020-01-01T00:00:00Z","message":{"model":"claude-opus-4-6","content":[]}}"#,
                "\n",
                r#"{"type":"user","timestamp":"2099-01-01T00:00:00Z","message":{"role":"user","content":"hi"}}"#,
                "\n",
            ),
        )
        .unwrap();

        write_record(&rec_path, "claude-sonnet-4-6", now_ms(), true, None).unwrap();
        assert_eq!(
            resolve(&dir, Some("ses-ts"), Some(&transcript), true).as_deref(),
            Some("claude-sonnet-4-6")
        );

        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_file(rec_path);
    }

    #[test]
    fn falls_back_to_cached_session_start_when_transcript_is_empty_and_keeps_subagents_separate() {
        let dir = scratch("fallback");
        let empty_transcript = dir.join("empty.jsonl");
        fs::write(&empty_transcript, "").unwrap();

        let rec_path = record_path(&dir, "ses-1").unwrap();
        write_record(&rec_path, "claude-sonnet-4-6", now_ms(), false, None).unwrap();

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
