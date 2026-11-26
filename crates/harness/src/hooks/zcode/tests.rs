use hookah_application::{EditRequest, Outcome, StartRequest, StopRequest};
use hookah_domain::finding::{Finding, Location, Severity};
use hookah_domain::toolchain::CheckKind;
use hookah_domain::verdict::{Notice, Report, Verdict};
use serde_json::{Value, json};

use super::ZCode;
use crate::protocol::{Harness, Invocation, Response};
use crate::testing::{abs, os_json};

fn parse(payload: &str) -> Invocation {
    ZCode.parse(&os_json(payload)).unwrap()
}

#[test]
fn a_successful_write_keeps_the_session_and_resolves_the_path() {
    let invocation = parse(
        r#"{"hookEventName":"PostToolUse","hook_event_name":"PostToolUse",
        "sessionId":"s","session_id":"s","cwd":"/p","toolName":"Write","tool_name":"Write",
        "toolInput":{"file_path":"src/a.rs"},"tool_input":{"file_path":"src/a.rs"}}"#,
    );
    assert_eq!(
        invocation,
        Invocation::Edit(
            EditRequest::new(abs("/p/src/a.rs"), abs("/p")).with_session(Some("s".into()))
        )
    );
}

#[test]
fn apply_patch_checks_targets_and_ignores_deleted_files() {
    let invocation = parse(
        r#"{"hook_event_name":"PostToolUse","session_id":"s","cwd":"/p","tool_name":"ApplyPatch",
        "tool_input":{"patch_text":"*** Begin Patch\n*** Add File: a.rs\n+fn a() {}\n*** Update File: from.rs\n*** Move to: to.rs\n*** Delete File: gone.rs\n*** End Patch"}}"#,
    );
    assert_eq!(
        invocation,
        Invocation::EditBatch(
            ["a.rs", "to.rs"]
                .into_iter()
                .map(|name| EditRequest::new(abs("/p").join(name), abs("/p"))
                    .with_session(Some("s".into())))
                .collect()
        )
    );
}

#[test]
fn unhandled_events_and_unrelated_tool_payloads_are_ignored() {
    assert_eq!(
        parse(
            r#"{"hook_event_name":"PostToolUseFailure","cwd":"/p","tool_name":"Edit","tool_input":{"file_path":"a.rs"}}"#
        ),
        Invocation::Ignore
    );
    assert_eq!(
        parse(
            r#"{"hook_event_name":"PostToolUse","cwd":"/p","tool_name":"Read","tool_input":{"file_path":"a.rs"}}"#
        ),
        Invocation::Ignore
    );
}

#[test]
fn a_session_start_passes_cwd_and_session_to_the_baseline() {
    assert_eq!(
        parse(
            r#"{"hook_event_name":"SessionStart","session_id":"s","cwd":"/p","source":"startup"}"#
        ),
        Invocation::Start(StartRequest::new(abs("/p"), "s"))
    );
    assert_eq!(
        ZCode.render(
            &Invocation::Start(StartRequest::new(abs("/p"), "s")),
            &Outcome::pass()
        ),
        Response::silent()
    );
}

fn failure() -> Outcome {
    let finding = Finding::new(
        CheckKind::Lint,
        Severity::Error,
        Location::line("a.rs", 5),
        "unused import",
    );
    Outcome {
        verdict: Verdict::Fail(Report::new(vec![finding], 20).unwrap()),
        notices: vec![Notice::FileRewritten(abs("/p/a.rs"))],
    }
}

#[test]
fn edit_feedback_uses_context_and_includes_re_read_hint() {
    let edit = Invocation::EditBatch(vec![EditRequest::new(abs("/p/a.rs"), abs("/p"))]);
    let response = ZCode.render(&edit, &failure());
    assert_eq!(response.exit_code, 0);
    let output: Value = serde_json::from_str(&response.stdout).unwrap();
    assert!(output.get("decision").is_none());
    let payload = &output["hookSpecificOutput"];
    assert_eq!(payload["hookEventName"], "PostToolUse");
    let context = payload["additionalContext"].as_str().unwrap();
    assert!(context.contains("a.rs:5: unused import"));
    assert!(context.to_ascii_lowercase().contains("re-read"));
}

#[test]
fn stop_requests_one_continuation_and_warns_on_the_next_attempt() {
    let stop = |active| {
        parse(&format!(
            r#"{{"hook_event_name":"Stop","cwd":"/p","session_id":"s","stop_hook_active":{active}}}"#
        ))
    };
    assert_eq!(
        stop(false),
        Invocation::Stop {
            request: StopRequest::new(abs("/p")).with_session(Some("s".into())),
            already_continuing: false,
        }
    );
    let response = ZCode.render(&stop(false), &failure());
    assert_eq!(response.exit_code, 0);
    let actual: Value = serde_json::from_str(&response.stdout).unwrap();
    assert_eq!(actual["decision"], "block");
    assert!(
        actual["reason"]
            .as_str()
            .unwrap()
            .contains("a.rs:5: unused import")
    );

    let response = ZCode.render(&stop(true), &failure());
    assert_eq!(response.exit_code, 0);
    assert!(response.stdout.is_empty());
    assert!(response.stderr.contains("a.rs:5: unused import"));
}

#[test]
fn a_degraded_stop_prints_a_warning_and_leaves_stdout_empty() {
    let stop = parse(r#"{"hook_event_name":"Stop","cwd":"/p","session_id":"s"}"#);
    let response = ZCode.render(&stop, &Outcome::degraded("timeout"));
    assert_eq!(response.exit_code, 0);
    assert!(response.stdout.is_empty());
    assert!(response.stderr.contains("timeout"));
}

#[test]
fn a_passing_edit_explains_that_the_formatter_rewrote_the_file() {
    let edit = Invocation::Edit(EditRequest::new(abs("/p/a.rs"), abs("/p")));
    let mut outcome = Outcome::pass();
    outcome.notices.push(Notice::FileRewritten(abs("/p/a.rs")));
    let response = ZCode.render(&edit, &outcome);
    let actual = serde_json::from_str::<Value>(&response.stdout).unwrap();
    assert_eq!(
        actual["hookSpecificOutput"]["hookEventName"],
        json!("PostToolUse")
    );
    assert!(
        actual["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("Re-read")
    );
}
