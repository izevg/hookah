//! opencode plugin payload: the JSON the bundled plugin sends on stdin.
//!
//! ```json
//! {"v": 1, "event": "edit" | "stop", "cwd": "/abs/dir", "files": ["a.ts"],
//!  "session_id": "ses_...", "already_continuing": false}
//! ```
//!
//! - `v` must be 1; any other version is rejected so a stale plugin fails open with a warning.
//! - `edit` becomes one edit request per file (several files make a batch); relative paths
//!   resolve against `cwd`. An edit without files is ignored.
//! - `stop` becomes a stop request rooted at the absolute `cwd`. `already_continuing` is true
//!   when this stop attempt follows a feedback message the plugin injected itself.
//! - `session_id` is informational; the plugin owns the per-session loop state.

use std::path::PathBuf;

use hookah_application::{EditRequest, StopRequest};
use serde::Deserialize;

use crate::protocol::{Invocation, PayloadError, display_root, resolve_path};

/// The contract version this build understands.
pub const VERSION: u32 = 1;

#[derive(Deserialize)]
struct Payload {
    v: u32,
    event: Event,
    cwd: Option<PathBuf>,
    #[serde(default)]
    files: Vec<PathBuf>,
    #[serde(default)]
    already_continuing: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Event {
    Edit,
    Stop,
}

/// Parses an opencode plugin payload.
pub fn parse(stdin: &str) -> Result<Invocation, PayloadError> {
    let p: Payload = serde_json::from_str(stdin).map_err(|e| PayloadError(e.to_string()))?;
    if p.v != VERSION {
        return Err(PayloadError(format!(
            "unsupported plugin contract version {} (expected {VERSION}); reinstall the plugin \
             with `hookah plugin opencode`",
            p.v
        )));
    }
    match p.event {
        Event::Edit => edit(&p),
        Event::Stop => {
            let root = p
                .cwd
                .filter(|c| c.is_absolute())
                .ok_or_else(|| PayloadError("stop event without an absolute cwd".into()))?;
            Ok(Invocation::Stop {
                request: StopRequest::new(root),
                already_continuing: p.already_continuing,
            })
        }
    }
}

fn edit(p: &Payload) -> Result<Invocation, PayloadError> {
    let mut requests = Vec::new();
    for path in &p.files {
        let file = resolve_path(path, p.cwd.as_deref())?;
        let root = display_root(&file, p.cwd.as_deref());
        requests.push(EditRequest::new(file, root));
    }
    Ok(match requests.len() {
        0 => Invocation::Ignore,
        1 => Invocation::Edit(requests.remove(0)),
        _ => Invocation::EditBatch(requests),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn edit_files(json: &str) -> Vec<PathBuf> {
        match parse(json).unwrap() {
            Invocation::Edit(r) => vec![r.file],
            Invocation::EditBatch(rs) => rs.into_iter().map(|r| r.file).collect(),
            other => panic!("expected an edit, got {other:?}"),
        }
    }

    #[test]
    fn single_file_edit() {
        let json = r#"{"v":1,"event":"edit","cwd":"/w/app","files":["/w/app/src/a.ts"],
            "session_id":"ses_1","already_continuing":false}"#;
        match parse(json).unwrap() {
            Invocation::Edit(r) => {
                assert_eq!(r.file, PathBuf::from("/w/app/src/a.ts"));
                assert_eq!(r.display_root, PathBuf::from("/w/app"));
            }
            other => panic!("expected edit, got {other:?}"),
        }
    }

    #[test]
    fn several_files_make_a_batch_and_relative_paths_resolve_against_cwd() {
        let json = r#"{"v":1,"event":"edit","cwd":"/w","files":["a.ts","/x/b.ts"]}"#;
        assert_eq!(
            edit_files(json),
            [PathBuf::from("/w/a.ts"), PathBuf::from("/x/b.ts")]
        );
    }

    #[test]
    fn relative_path_without_cwd_is_malformed() {
        assert!(parse(r#"{"v":1,"event":"edit","files":["a.ts"]}"#).is_err());
        assert!(parse(r#"{"v":1,"event":"edit","cwd":"rel","files":["a.ts"]}"#).is_err());
    }

    #[test]
    fn edit_without_files_is_ignored() {
        assert_eq!(
            parse(r#"{"v":1,"event":"edit","cwd":"/w"}"#).unwrap(),
            Invocation::Ignore
        );
        assert_eq!(
            parse(r#"{"v":1,"event":"edit","cwd":"/w","files":[]}"#).unwrap(),
            Invocation::Ignore
        );
    }

    #[test]
    fn stop_carries_cwd_and_loop_flag() {
        let json = r#"{"v":1,"event":"stop","cwd":"/w/app","session_id":"s",
            "already_continuing":true}"#;
        assert_eq!(
            parse(json).unwrap(),
            Invocation::Stop {
                request: StopRequest::new(PathBuf::from("/w/app")),
                already_continuing: true,
            }
        );
        let json = r#"{"v":1,"event":"stop","cwd":"/w/app"}"#;
        assert!(matches!(
            parse(json).unwrap(),
            Invocation::Stop {
                already_continuing: false,
                ..
            }
        ));
    }

    #[test]
    fn stop_needs_an_absolute_cwd() {
        assert!(parse(r#"{"v":1,"event":"stop"}"#).is_err());
        assert!(parse(r#"{"v":1,"event":"stop","cwd":"rel"}"#).is_err());
    }

    #[test]
    fn other_versions_events_and_garbage_are_errors() {
        let err = parse(r#"{"v":2,"event":"edit","cwd":"/w","files":["a.ts"]}"#).unwrap_err();
        assert!(err.0.contains("unsupported plugin contract version 2"));
        assert!(parse(r#"{"v":1,"event":"idle","cwd":"/w"}"#).is_err());
        assert!(parse(r#"{"event":"edit","cwd":"/w"}"#).is_err());
        assert!(parse("not json").is_err());
    }
}
