//! End-to-end tests of the `hookah` binary in hook mode.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::process::{Command, Output, Stdio};

fn run(args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hookah"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hookah");
    // The child may exit before reading everything; ignore a broken pipe.
    let _ = child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes());
    child.wait_with_output().expect("wait")
}

/// An absolute directory that does not exist. A Windows path needs a drive to count as absolute.
const NOWHERE: &str = if cfg!(windows) {
    "C:/nonexistent"
} else {
    "/nonexistent"
};

#[test]
fn edit_payload_outside_any_project_passes_silently() {
    let payload = format!(
        r#"{{"session_id":"s","cwd":"{NOWHERE}","hook_event_name":"PostToolUse",
        "tool_name":"Edit","tool_input":{{"file_path":"{NOWHERE}/a.rs"}}}}"#
    );
    let out = run(&["hook", "claude"], &payload);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
}

#[test]
fn stop_payload_passes() {
    let payload = format!(
        r#"{{"session_id":"s","cwd":"{NOWHERE}","hook_event_name":"Stop",
        "stop_hook_active":false}}"#
    );
    assert_eq!(run(&["hook", "claude"], &payload).status.code(), Some(0));
}

#[test]
fn garbage_payload_fails_open_with_a_warning() {
    let out = run(&["hook", "claude"], "not json at all");
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("hookah: warning"));
}

#[test]
fn codex_garbage_fails_open_and_unrelated_tool_is_ignored() {
    let out = run(&["hook", "codex"], "not json at all");
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("hookah: warning"));
    let bash = r#"{"cwd":"/nonexistent","hook_event_name":"PostToolUse","tool_name":"Bash",
        "tool_input":{"command":"ls"}}"#;
    let out = run(&["hook", "codex"], bash);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
}

#[test]
fn qoder_garbage_fails_open_and_unrelated_tool_is_ignored() {
    let out = run(&["hook", "qoder"], "not json at all");
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("hookah: warning"));
    let bash = r#"{"cwd":"/nonexistent","hook_event_name":"PostToolUse","tool_name":"Bash",
        "tool_input":{"command":"ls"}}"#;
    let out = run(&["hook", "qoder"], bash);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
}

#[test]
fn kimi_garbage_fails_open_and_unrelated_tool_is_ignored() {
    let out = run(&["hook", "kimi"], "not json at all");
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("hookah: warning"));
    let bash = r#"{"cwd":"/nonexistent","hook_event_name":"PostToolUse","tool_name":"Bash",
        "tool_input":{"command":"ls"}}"#;
    let out = run(&["hook", "kimi"], bash);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
}

#[test]
fn opencode_garbage_and_stale_versions_fail_open() {
    let out = run(&["hook", "opencode"], "not json at all");
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("hookah: warning"));
    let stale = r#"{"v":9,"event":"edit","cwd":"/nonexistent","files":["a.ts"]}"#;
    let out = run(&["hook", "opencode"], stale);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("reinstall the plugin"));
}

#[test]
fn opencode_edit_and_stop_answer_with_json() {
    let edit = format!(
        r#"{{"v":1,"event":"edit","cwd":"{NOWHERE}","files":["a.ts"],
        "session_id":"s","already_continuing":false}}"#
    );
    let out = run(&["hook", "opencode"], &edit);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        r#"{"v":1,"block":false}"#
    );
    let stop = format!(r#"{{"v":1,"event":"stop","cwd":"{NOWHERE}","already_continuing":true}}"#);
    let out = run(&["hook", "opencode"], &stop);
    assert_eq!(out.status.code(), Some(0));
    // Not a repository: the stop phase degrades, which warns the user and never blocks.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.starts_with(r#"{"v":1,"block":false,"user_text":"hookah: checks incomplete"#));
}

#[test]
fn plugin_opencode_prints_the_embedded_source() {
    let out = run(&["plugin", "opencode"], "");
    assert_eq!(out.status.code(), Some(0));
    let source = String::from_utf8_lossy(&out.stdout);
    assert_eq!(source, include_str!("../../harness/src/opencode/plugin.ts"));
    assert!(run(&["plugin", "nonsense"], "").status.code() != Some(0));
}

#[test]
fn bad_hook_arguments_fail_open() {
    assert_eq!(run(&["hook", "nonsense"], "").status.code(), Some(0));
}

#[test]
fn detect_reports_no_project() {
    let out = run(&["detect", "/nonexistent/a.rs"], "");
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("no project found"));
}

#[test]
fn check_passes_when_nothing_is_wrong() {
    let out = run(&["check", "/nonexistent/a.rs"], "");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn bad_check_arguments_exit_nonzero() {
    assert_ne!(run(&["check"], "").status.code(), Some(0));
}
