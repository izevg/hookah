//! Reads the active model identifier from a session transcript JSONL file.
//!
//! Some harnesses (such as Claude Code on `PostToolUse` and `Stop`) omit `model` from the hook
//! stdin payload but include `transcript_path`. Assistant records in that JSONL file carry the
//! exact model that produced each turn, so scanning backwards from the end of the file yields the
//! current model even after a mid-session model switch. Everything fails open: missing,
//! unreadable, or malformed files return `None`.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

/// Bytes read per backward chunk. Large enough to cover most recent turns in one read.
const CHUNK_BYTES: u64 = 64 * 1024;
/// Upper bound on total bytes scanned from the tail of a transcript.
const MAX_SCAN_BYTES: u64 = 2 * 1024 * 1024;
/// Placeholder Claude Code writes on synthetic local messages rather than a real LLM turn.
const SYNTHETIC_MODEL: &str = "<synthetic>";

/// The most recent real model record found in a transcript JSONL file, along with its end byte
/// offset and optional entry timestamp so callers can tell whether a newer real model turn was
/// appended after a mid-session model switch (ignoring trailing `user` or `<synthetic>` entries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptModel {
    /// Trimmed model identifier from the latest real model entry.
    pub model: String,
    /// Byte offset immediately after the matching line (before its trailing `\n`, if any).
    pub end_offset: u64,
    /// Parsed RFC 3339 entry timestamp in Unix milliseconds, if present on the JSON record.
    pub timestamp_ms: Option<u128>,
}

/// Returns the most recent model identifier recorded in the JSONL transcript at `path`.
pub fn latest_model(path: &Path) -> Option<String> {
    latest_model_entry(path).map(|entry| entry.model)
}

/// Returns the most recent real model entry recorded in the JSONL transcript at `path`.
pub fn latest_model_entry(path: &Path) -> Option<TranscriptModel> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        return None;
    }

    let mut pos = len;
    let mut scanned = 0u64;
    let mut carry: Vec<u8> = Vec::new();

    while pos > 0 && scanned < MAX_SCAN_BYTES {
        let take = pos.min(CHUNK_BYTES).min(MAX_SCAN_BYTES - scanned);
        pos -= take;
        scanned += take;

        let Ok(take_usize) = usize::try_from(take) else {
            return None;
        };
        let mut buf = vec![0u8; take_usize];
        file.seek(SeekFrom::Start(pos)).ok()?;
        file.read_exact(&mut buf).ok()?;
        buf.extend_from_slice(&carry);

        if pos > 0 {
            if let Some(nl) = buf.iter().position(|&b| b == b'\n') {
                let slice_base = pos.saturating_add(u64::try_from(nl + 1).ok()?);
                if let Some(entry) = scan_lines_reverse(&buf[nl + 1..], slice_base) {
                    return Some(entry);
                }
                // Keep the newline so `buf` stays a contiguous file slice starting at `pos`.
                carry = buf[..=nl].to_vec();
            } else {
                // One line exceeds a chunk: carry it back until we reach its start.
                carry = buf;
            }
        } else {
            return scan_lines_reverse(&buf, 0);
        }
    }

    if !carry.is_empty() {
        return scan_lines_reverse(&carry, pos);
    }
    None
}

fn scan_lines_reverse(bytes: &[u8], base_offset: u64) -> Option<TranscriptModel> {
    let mut end = bytes.len();
    while end > 0 {
        let start = bytes[..end]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |nl| nl + 1);
        let line_bytes = &bytes[start..end];
        let text = String::from_utf8_lossy(line_bytes);
        let trimmed = text.trim();
        if !trimmed.is_empty()
            && trimmed.contains("\"model\"")
            && let Some((model, timestamp_ms)) = model_from_line(trimmed)
        {
            let end_u64 = u64::try_from(end).ok()?;
            return Some(TranscriptModel {
                model,
                end_offset: base_offset.saturating_add(end_u64),
                timestamp_ms,
            });
        }
        end = start.saturating_sub(1);
    }
    None
}

fn model_from_line(line: &str) -> Option<(String, Option<u128>)> {
    let value: Value = serde_json::from_str(line).ok()?;
    let candidate = match value.get("type").and_then(Value::as_str) {
        Some("assistant") => value
            .get("message")
            .and_then(|m| m.get("model"))
            .and_then(Value::as_str)
            .or_else(|| value.get("model").and_then(Value::as_str)),
        Some("turn_context") => value
            .get("payload")
            .and_then(|p| p.get("model"))
            .and_then(Value::as_str),
        _ => None,
    }?;
    let trimmed = candidate.trim();
    if trimmed.is_empty() || trimmed == SYNTHETIC_MODEL {
        return None;
    }
    let timestamp_ms = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_rfc3339_ms);
    Some((trimmed.to_owned(), timestamp_ms))
}

/// Parses an RFC 3339 timestamp (`YYYY-MM-DDTHH:MM:SS[.fff](Z|+HH:MM|-HH:MM)`) into Unix
/// milliseconds. Returns `None` for any unexpected format.
fn parse_rfc3339_ms(raw: &str) -> Option<u128> {
    let bytes = raw.trim().as_bytes();
    if bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year: i64 = std::str::from_utf8(&bytes[0..4]).ok()?.parse().ok()?;
    let month: u32 = std::str::from_utf8(&bytes[5..7]).ok()?.parse().ok()?;
    let day: u32 = std::str::from_utf8(&bytes[8..10]).ok()?.parse().ok()?;
    let hour: u32 = std::str::from_utf8(&bytes[11..13]).ok()?.parse().ok()?;
    let min: u32 = std::str::from_utf8(&bytes[14..16]).ok()?.parse().ok()?;
    let sec: u32 = std::str::from_utf8(&bytes[17..19]).ok()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 60 {
        return None;
    }

    let mut idx = 19;
    let mut millis: i64 = 0;
    if bytes.get(idx) == Some(&b'.') {
        idx += 1;
        let frac_start = idx;
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
        if idx == frac_start {
            return None;
        }
        let frac = &bytes[frac_start..idx];
        let mut scale = 100i64;
        for &digit in frac.iter().take(3) {
            millis += i64::from(digit - b'0') * scale;
            scale /= 10;
        }
    }

    let offset_secs: i64 = match bytes.get(idx)? {
        b'Z' | b'z' if idx + 1 == bytes.len() => 0,
        sign @ (b'+' | b'-') if idx + 6 == bytes.len() && bytes[idx + 3] == b':' => {
            let oh: i64 = std::str::from_utf8(&bytes[idx + 1..idx + 3])
                .ok()?
                .parse()
                .ok()?;
            let om: i64 = std::str::from_utf8(&bytes[idx + 4..idx + 6])
                .ok()?
                .parse()
                .ok()?;
            if oh > 23 || om > 59 {
                return None;
            }
            let total = oh * 3600 + om * 60;
            if *sign == b'+' { total } else { -total }
        }
        _ => return None,
    };

    let days = days_from_civil(year, month, day);
    let secs = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3600 + i64::from(min) * 60 + i64::from(sec))?
        .checked_sub(offset_secs)?;
    let total_ms = secs.checked_mul(1_000)?.checked_add(millis)?;
    u128::try_from(total_ms).ok()
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let d = i64::from(day);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn temp_file(name: &str, content: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "hookah-transcript-{name}-{}-{}.jsonl",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn reads_latest_assistant_model_and_skips_synthetic_entries() {
        let path = temp_file(
            "claude",
            concat!(
                r#"{"type":"user","message":{"role":"user","content":"model: fake"}}"#,
                "\n",
                r#"{"type":"assistant","message":{"model":"claude-sonnet-4-6","content":[]}}"#,
                "\n",
                r#"{"type":"assistant","message":{"model":"claude-opus-4-6","content":[]}}"#,
                "\n",
                r#"{"type":"assistant","message":{"model":"<synthetic>","content":[]}}"#,
                "\n",
                r#"{"type":"user","message":{"role":"user","content":"{\"model\":\"not-this\"}"}}"#,
                "\n",
            ),
        );
        assert_eq!(latest_model(&path).as_deref(), Some("claude-opus-4-6"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn reads_turn_context_model() {
        let path = temp_file(
            "codex",
            concat!(
                r#"{"type":"turn_context","payload":{"model":"gpt-5.3-codex"}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"text":"hi"}}"#,
                "\n",
            ),
        );
        assert_eq!(latest_model(&path).as_deref(), Some("gpt-5.3-codex"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn handles_multi_chunk_transcripts_and_long_lines() {
        let padding = "x".repeat(80 * 1024);
        let content = format!(
            "{}\n{{\"type\":\"user\",\"text\":\"{padding}\"}}\n",
            r#"{"type":"assistant","message":{"model":"claude-haiku-4-5"}}"#
        );
        let path = temp_file("large", &content);
        assert_eq!(latest_model(&path).as_deref(), Some("claude-haiku-4-5"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn missing_empty_or_malformed_files_return_none() {
        assert_eq!(
            latest_model(Path::new("/nonexistent/hookah-transcript.jsonl")),
            None
        );
        let empty = temp_file("empty", "");
        assert_eq!(latest_model(&empty), None);
        let _ = fs::remove_file(empty);

        let garbage = temp_file("garbage", "not json\n{\"model\":\"bare\"}\n");
        assert_eq!(latest_model(&garbage), None);
        let _ = fs::remove_file(garbage);
    }

    #[test]
    fn end_offset_stays_stable_when_trailing_non_model_entries_are_appended() {
        let assistant = r#"{"type":"assistant","timestamp":"2026-04-01T12:00:00.500Z","message":{"model":"claude-opus-4-6","content":[]}}"#;
        let path = temp_file("offset", &format!("{assistant}\n"));
        let first = latest_model_entry(&path).unwrap();
        assert_eq!(first.model, "claude-opus-4-6");
        assert_eq!(first.end_offset, assistant.len() as u64);
        assert_eq!(
            first.timestamp_ms,
            parse_rfc3339_ms("2026-04-01T12:00:00.500Z")
        );
        assert!(first.timestamp_ms.is_some());

        // Appending user and synthetic entries does not advance the real model entry's offset.
        let mut content = format!("{assistant}\n");
        content.push_str(
            r#"{"type":"user","timestamp":"2026-04-01T12:05:00Z","message":{"role":"user","content":"hi"}}"#,
        );
        content.push('\n');
        content.push_str(
            r#"{"type":"assistant","timestamp":"2026-04-01T12:05:01Z","message":{"model":"<synthetic>","content":[]}}"#,
        );
        content.push('\n');
        fs::write(&path, &content).unwrap();
        let after_non_model = latest_model_entry(&path).unwrap();
        assert_eq!(after_non_model, first);

        // Appending a real assistant entry strictly advances the end offset.
        content.push_str(
            r#"{"type":"assistant","timestamp":"2026-04-01T12:06:00Z","message":{"model":"claude-sonnet-4-6","content":[]}}"#,
        );
        content.push('\n');
        fs::write(&path, &content).unwrap();
        let after_real = latest_model_entry(&path).unwrap();
        assert_eq!(after_real.model, "claude-sonnet-4-6");
        assert!(after_real.end_offset > first.end_offset);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn parses_rfc3339_timestamps_with_utc_and_offsets() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:01.25Z"), Some(1_250));
        assert_eq!(parse_rfc3339_ms("1970-01-01T03:00:00+03:00"), Some(0));
        assert_eq!(
            parse_rfc3339_ms("1970-01-01T00:00:00-01:00"),
            Some(3_600_000)
        );
        assert_eq!(parse_rfc3339_ms("not-a-timestamp"), None);
    }
}
