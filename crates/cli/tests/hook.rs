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

#[test]
fn edit_payload_outside_any_project_passes_silently() {
    let payload = r#"{"session_id":"s","cwd":"/nonexistent","hook_event_name":"PostToolUse",
        "tool_name":"Edit","tool_input":{"file_path":"/nonexistent/a.rs"}}"#;
    let out = run(&["hook", "claude"], payload);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
}

#[test]
fn stop_payload_passes() {
    let payload = r#"{"session_id":"s","cwd":"/nonexistent","hook_event_name":"Stop",
        "stop_hook_active":false}"#;
    assert_eq!(run(&["hook", "claude"], payload).status.code(), Some(0));
}

#[test]
fn garbage_payload_fails_open_with_a_warning() {
    let out = run(&["hook", "claude"], "not json at all");
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("hookah: warning"));
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
