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

/// Returns the most recent model identifier recorded in the JSONL transcript at `path`.
pub fn latest_model(path: &Path) -> Option<String> {
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
                if let Some(model) = scan_lines_reverse(&buf[nl + 1..]) {
                    return Some(model);
                }
                carry = buf[..nl].to_vec();
            } else {
                // One line exceeds a chunk: carry it back until we reach its start.
                carry = buf;
            }
        } else {
            return scan_lines_reverse(&buf);
        }
    }

    if !carry.is_empty() {
        return scan_lines_reverse(&carry);
    }
    None
}

fn scan_lines_reverse(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    for line in text.lines().rev() {
        let line = line.trim();
        if line.is_empty() || !line.contains("\"model\"") {
            continue;
        }
        if let Some(model) = model_from_line(line) {
            return Some(model);
        }
    }
    None
}

fn model_from_line(line: &str) -> Option<String> {
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
    Some(trimmed.to_owned())
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
}
