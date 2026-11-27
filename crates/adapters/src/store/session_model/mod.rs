//! Per-session model state persisted between independent hook processes.
//!
//! Some harnesses (notably Claude Code and Antigravity) report the active model on lifecycle
//! events (`SessionStart`, `PostModelSwitch`, `PreInvocation`) while omitting it on `PostToolUse`
//! or `Stop`, where transcript writes can also lag behind the hook invocation. Keying the stored
//! model by `(harness, session_id)` inside `<hookah cache>/sessions/` keeps it stable even when a
//! session changes working directory, prevents collisions across harnesses, and creates no cache
//! directories during read-only lookups. Everything fails open: cache errors are silently ignored.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::cache_dir::{fnv1a, sessions_cache_dir, sessions_cache_path};
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

/// Stored model record for one `(harness, session_id)` pair.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
struct Record {
    model: String,
    at_ms: u128,
    /// True when written by an explicit mid-session switch (`PostModelSwitch`), so late pre-switch
    /// transcript flushes or untimestamped entries cannot undo it.
    from_switch: bool,
    /// End byte offset of the latest real model entry in the transcript when this record was
    /// written (`Some(0)` if the transcript had no real model entry yet; `None` if no transcript
    /// path was supplied). Trailing `user` or `<synthetic>` entries appended after a switch do
    /// not advance this offset.
    #[serde(default)]
    transcript_model_offset: Option<u64>,
}

/// Records `model` for `(harness, session)` in the session cache. Silently ignores any error.
///
/// When `transcript_path` is provided, also snapshots the end byte offset of the latest real model
/// entry currently in that transcript so later trailing non-model entries (`user`, `<synthetic>`)
/// cannot make an older assistant record override this update.
pub fn record(
    harness: &str,
    session: &str,
    model: &str,
    from_switch: bool,
    transcript_path: Option<&Path>,
) {
    let Ok(dir) = sessions_cache_dir() else {
        return;
    };
    record_in(&dir, harness, session, model, from_switch, transcript_path);
}

fn record_in(
    dir: &Path,
    harness: &str,
    session: &str,
    model: &str,
    from_switch: bool,
    transcript_path: Option<&Path>,
) {
    let model = model.trim();
    if harness.is_empty() || session.is_empty() || model.is_empty() {
        return;
    }
    let path = record_file(dir, harness, session);
    let transcript_model_offset =
        transcript_path.map(|p| transcript::latest_model_entry(p).map_or(0, |e| e.end_offset));
    let _ = write_record(&path, model, now_ms(), from_switch, transcript_model_offset);
}

/// Resolves the model for a hook event that did not include `model` directly.
///
/// Purely read-only: never creates any cache directory or file.
pub fn resolve(
    harness: &str,
    session: Option<&str>,
    transcript_path: Option<&Path>,
    use_session_cache: bool,
) -> Option<String> {
    resolve_with_origin(harness, session, transcript_path, use_session_cache).map(|r| r.model)
}

/// Resolves the model together with the source that supplied it. Purely read-only: creates no
/// cache directories.
///
/// - When `use_session_cache` is true (main session, not a separate subagent), checks any stored
///   session record for `(harness, session)`.
/// - A stored record beats a transcript entry when:
///   - the transcript has not appended any real model record past the snapshotted byte offset
///     (ignoring trailing `user` or `<synthetic>` entries), OR
///   - this record came from an explicit switch (`from_switch`) and the transcript entry's
///     timestamp predates or equals the switch (protecting against late-flushed pre-switch turns),
///     OR
///   - this record came from an explicit switch with no transcript offset snapshot and no entry
///     timestamp to prove a newer post-switch turn (file `mtime` is not trusted because non-model
///     entries bump it).
/// - Otherwise reads the latest real model entry from `transcript`, falling back to any stored
///   session record when the transcript is missing or has no real model entry yet.
pub fn resolve_with_origin(
    harness: &str,
    session: Option<&str>,
    transcript_path: Option<&Path>,
    use_session_cache: bool,
) -> Option<ResolvedModel> {
    let dir = sessions_cache_path().ok();
    resolve_in(
        dir.as_deref(),
        harness,
        session,
        transcript_path,
        use_session_cache,
    )
}

fn resolve_in(
    dir: Option<&Path>,
    harness: &str,
    session: Option<&str>,
    transcript_path: Option<&Path>,
    use_session_cache: bool,
) -> Option<ResolvedModel> {
    let cached = dir
        .filter(|_| use_session_cache && !harness.is_empty())
        .zip(session.filter(|s| !s.is_empty()))
        .and_then(|(d, s)| read_record(&record_file(d, harness, s)));

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
        if !truncated && entry.end_offset <= snapshot_offset {
            return true;
        }
        // Even at a larger offset, a late-flushed assistant record timestamped at or before an
        // explicit switch must not undo that switch.
        if rec.from_switch
            && let Some(entry_ts) = entry.timestamp_ms
            && entry_ts <= rec.at_ms
        {
            return true;
        }
        return false;
    }
    if !rec.from_switch {
        return false;
    }
    // Explicit switch without an offset snapshot: only yield when the real model entry proves via
    // its timestamp that it was produced strictly after the switch.
    match entry.timestamp_ms {
        Some(entry_ts) => entry_ts <= rec.at_ms,
        None => true,
    }
}

fn record_file(dir: &Path, harness: &str, session: &str) -> PathBuf {
    let mut key_bytes = Vec::with_capacity(harness.len() + 1 + session.len());
    key_bytes.extend_from_slice(harness.as_bytes());
    key_bytes.push(0);
    key_bytes.extend_from_slice(session.as_bytes());
    let key = fnv1a(&key_bytes);
    dir.join(format!("{key:016x}.json"))
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
    fn switch_record_beats_trailing_non_model_and_late_pre_switch_entries() {
        let dir = scratch("precedence");
        let sessions = dir.join("sessions");
        let transcript = dir.join("t.jsonl");
        let old_assistant = concat!(
            r#"{"type":"assistant","timestamp":"2020-01-01T00:00:00Z","message":{"model":"claude-opus-4-6","content":[]}}"#,
            "\n"
        );
        fs::write(&transcript, old_assistant).unwrap();

        // Explicit switch snapshots the transcript's latest real model offset.
        record_in(
            &sessions,
            "claude",
            "ses-offset",
            "claude-sonnet-4-6",
            true,
            Some(&transcript),
        );

        // Appending user and <synthetic> entries bumps mtime, yet the switch still wins.
        let mut content = old_assistant.to_owned();
        content.push_str(
            r#"{"type":"user","timestamp":"2020-01-01T00:01:00Z","message":{"role":"user","content":"hello"}}"#,
        );
        content.push('\n');
        content.push_str(
            r#"{"type":"assistant","timestamp":"2020-01-01T00:01:01Z","message":{"model":"<synthetic>","content":[]}}"#,
        );
        content.push('\n');
        fs::write(&transcript, &content).unwrap();
        let resolved = resolve_in(
            Some(&sessions),
            "claude",
            Some("ses-offset"),
            Some(&transcript),
            true,
        );
        assert_eq!(
            resolved,
            Some(ResolvedModel {
                model: "claude-sonnet-4-6".into(),
                origin: ModelOrigin::SessionCache,
            })
        );

        // A late-flushed assistant turn timestamped BEFORE the switch (even at a larger byte
        // offset) must not undo the explicit switch.
        content.push_str(
            r#"{"type":"assistant","timestamp":"2020-01-01T00:02:00Z","message":{"model":"claude-opus-4-6","content":[]}}"#,
        );
        content.push('\n');
        fs::write(&transcript, &content).unwrap();
        let after_late_flush = resolve_in(
            Some(&sessions),
            "claude",
            Some("ses-offset"),
            Some(&transcript),
            true,
        );
        assert_eq!(
            after_late_flush,
            Some(ResolvedModel {
                model: "claude-sonnet-4-6".into(),
                origin: ModelOrigin::SessionCache,
            })
        );

        // Once a genuinely post-switch assistant turn is appended, the transcript takes over.
        content.push_str(
            r#"{"type":"assistant","timestamp":"2099-01-01T00:00:00Z","message":{"model":"claude-haiku-4-5","content":[]}}"#,
        );
        content.push('\n');
        fs::write(&transcript, &content).unwrap();
        let after_real = resolve_in(
            Some(&sessions),
            "claude",
            Some("ses-offset"),
            Some(&transcript),
            true,
        );
        assert_eq!(
            after_real,
            Some(ResolvedModel {
                model: "claude-haiku-4-5".into(),
                origin: ModelOrigin::Transcript,
            })
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn switch_without_transcript_snapshot_retains_switch_for_untimestamped_or_older_entries() {
        let dir = scratch("no-snapshot");
        let sessions = dir.join("sessions");
        let rec_path = record_file(&sessions, "claude", "ses-ts");
        let transcript = dir.join("t.jsonl");

        // Untimestamped older assistant entry followed by a trailing user entry that bumps mtime.
        fs::write(
            &transcript,
            concat!(
                r#"{"type":"assistant","message":{"model":"claude-opus-4-6","content":[]}}"#,
                "\n",
                r#"{"type":"user","message":{"role":"user","content":"hi"}}"#,
                "\n",
            ),
        )
        .unwrap();
        write_record(&rec_path, "claude-sonnet-4-6", now_ms(), true, None).unwrap();
        assert_eq!(
            resolve_in(
                Some(&sessions),
                "claude",
                Some("ses-ts"),
                Some(&transcript),
                true,
            )
            .map(|r| r.model)
            .as_deref(),
            Some("claude-sonnet-4-6")
        );

        // Post-switch timestamped assistant entry supersedes the switch.
        fs::write(
            &transcript,
            concat!(
                r#"{"type":"assistant","timestamp":"2099-01-01T00:00:00Z","message":{"model":"claude-haiku-4-5","content":[]}}"#,
                "\n"
            ),
        )
        .unwrap();
        assert_eq!(
            resolve_in(
                Some(&sessions),
                "claude",
                Some("ses-ts"),
                Some(&transcript),
                true,
            )
            .map(|r| r.model)
            .as_deref(),
            Some("claude-haiku-4-5")
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn falls_back_to_cached_session_start_keeps_subagents_and_harnesses_separate_and_is_readonly() {
        let dir = scratch("fallback");
        let missing_sessions = dir.join("never-created");
        let empty_transcript = dir.join("empty.jsonl");
        fs::write(&empty_transcript, "").unwrap();

        // Read-only lookup creates nothing on disk.
        assert_eq!(
            resolve_in(
                Some(&missing_sessions),
                "claude",
                Some("ses-1"),
                Some(&empty_transcript),
                true,
            ),
            None
        );
        assert!(!missing_sessions.exists());

        let sessions = dir.join("sessions");
        record_in(
            &sessions,
            "claude",
            "ses-1",
            "claude-sonnet-4-6",
            false,
            None,
        );

        // Main session falls back to SessionStart record.
        assert_eq!(
            resolve_in(
                Some(&sessions),
                "claude",
                Some("ses-1"),
                Some(&empty_transcript),
                true,
            )
            .map(|r| r.model)
            .as_deref(),
            Some("claude-sonnet-4-6")
        );
        // Another harness with the same session id does not collide.
        assert_eq!(
            resolve_in(
                Some(&sessions),
                "antigravity",
                Some("ses-1"),
                Some(&empty_transcript),
                true,
            ),
            None
        );
        // Subagent does not borrow the parent session's model.
        assert_eq!(
            resolve_in(
                Some(&sessions),
                "claude",
                Some("ses-1"),
                Some(&empty_transcript),
                false,
            ),
            None
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
