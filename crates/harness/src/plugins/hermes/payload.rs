//! Hermes plugin payload: the JSON the bundled plugin sends on stdin.
//!
//! ```json
//! {"v": 1, "event": "start" | "edit" | "stop", "cwd": "/abs/dir", "files": ["a.py"],
//!  "session_id": "20260101_...", "already_continuing": false}
//! ```
//!
//! - `v` must be 1; any other version is rejected so a stale plugin fails open with a warning.
//! - `edit` becomes one edit request per file (several files make a batch); relative paths
//!   resolve against `cwd`. An edit without files is ignored.
//! - `stop` becomes a stop request rooted at the absolute `cwd`. `already_continuing` is true
//!   when this stop attempt follows a continuation the plugin already asked for (Hermes'
//!   `pre_verify` `attempt` is above zero).
//! - `start` (Hermes' `on_session_start`) becomes a start request for the absolute `cwd` and
//!   `session_id`: it starts the baseline.
//! - `session_id` tags every edit and stop request, so they find the session's baseline.

use std::path::PathBuf;

use hookah_application::{EditRequest, StopRequest};
use serde::Deserialize;

use crate::protocol::{Invocation, PayloadError, display_root, resolve_path, start_invocation};

/// The contract version this build understands.
pub const VERSION: u32 = 1;

#[derive(Deserialize)]
struct Payload {
    v: u32,
    event: Event,
    cwd: Option<PathBuf>,
    session_id: Option<String>,
    #[serde(default)]
    files: Vec<PathBuf>,
    #[serde(default)]
    already_continuing: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Event {
    Start,
    Edit,
    Stop,
}

/// Parses a Hermes plugin payload.
pub fn parse(stdin: &str) -> Result<Invocation, PayloadError> {
    let p: Payload = serde_json::from_str(stdin).map_err(|e| PayloadError(e.to_string()))?;
    if p.v != VERSION {
        return Err(PayloadError(format!(
            "unsupported plugin contract version {} (expected {VERSION}); reinstall the plugin \
             with `hookah setup hermes`",
            p.v
        )));
    }
    match p.event {
        Event::Start => Ok(start_invocation(p.cwd, p.session_id)),
        Event::Edit => edit(&p),
        Event::Stop => {
            let root = p
                .cwd
                .filter(|c| c.is_absolute())
                .ok_or_else(|| PayloadError("stop event without an absolute cwd".into()))?;
            Ok(Invocation::Stop {
                request: StopRequest::new(root).with_session(p.session_id),
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
        requests.push(EditRequest::new(file, root).with_session(p.session_id.clone()));
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
    use hookah_application::StartRequest;

    use super::*;
    use crate::testing::{abs, os_json};

    fn parse(json: &str) -> Result<Invocation, PayloadError> {
        super::parse(&os_json(json))
    }

    fn edit_files(json: &str) -> Vec<PathBuf> {
        match parse(json).unwrap() {
            Invocation::Edit(r) => vec![r.file],
            Invocation::EditBatch(rs) => rs.into_iter().map(|r| r.file).collect(),
            other => panic!("expected an edit, got {other:?}"),
        }
    }

    #[test]
    fn single_file_edit_carries_root_and_session() {
        let json = r#"{"v":1,"event":"edit","cwd":"/w/app","files":["/w/app/src/a.py"],
            "session_id":"s1"}"#;
        let Invocation::Edit(r) = parse(json).unwrap() else {
            panic!("expected an edit");
        };
        assert_eq!(r.file, abs("/w/app/src/a.py"));
        assert_eq!(r.display_root, abs("/w/app"));
        assert_eq!(r.session.as_deref(), Some("s1"));
    }

    #[test]
    fn several_files_make_a_batch_and_relative_paths_resolve_against_cwd() {
        let json = r#"{"v":1,"event":"edit","cwd":"/w","files":["a.py","/x/b.py"]}"#;
        assert_eq!(edit_files(json), [abs("/w/a.py"), abs("/x/b.py")]);
    }

    #[test]
    fn relative_path_without_an_absolute_cwd_is_malformed() {
        assert!(parse(r#"{"v":1,"event":"edit","files":["a.py"]}"#).is_err());
        assert!(parse(r#"{"v":1,"event":"edit","cwd":"rel","files":["a.py"]}"#).is_err());
    }

    #[test]
    fn edit_without_files_is_ignored() {
        for json in [
            r#"{"v":1,"event":"edit","cwd":"/w"}"#,
            r#"{"v":1,"event":"edit","cwd":"/w","files":[]}"#,
        ] {
            assert_eq!(parse(json).unwrap(), Invocation::Ignore);
        }
    }

    #[test]
    fn stop_carries_cwd_session_and_loop_flag() {
        let json = r#"{"v":1,"event":"stop","cwd":"/w/app","session_id":"s",
            "already_continuing":true}"#;
        assert_eq!(
            parse(json).unwrap(),
            Invocation::Stop {
                request: StopRequest::new(abs("/w/app")).with_session(Some("s".into())),
                already_continuing: true,
            }
        );
        let json = r#"{"v":1,"event":"stop","cwd":"/w/app"}"#;
        assert_eq!(
            parse(json).unwrap(),
            Invocation::Stop {
                request: StopRequest::new(abs("/w/app")),
                already_continuing: false,
            }
        );
    }

    #[test]
    fn stop_needs_an_absolute_cwd() {
        assert!(parse(r#"{"v":1,"event":"stop"}"#).is_err());
        assert!(parse(r#"{"v":1,"event":"stop","cwd":"rel"}"#).is_err());
    }

    #[test]
    fn other_versions_events_and_garbage_are_errors() {
        let err = parse(r#"{"v":2,"event":"edit","cwd":"/w","files":["a.py"]}"#).unwrap_err();
        assert!(err.0.contains("unsupported plugin contract version 2"));
        assert!(err.0.contains("hookah setup hermes"));
        assert!(parse(r#"{"v":1,"event":"idle","cwd":"/w"}"#).is_err());
        assert!(parse(r#"{"event":"edit","cwd":"/w"}"#).is_err());
        assert!(parse("not json").is_err());
    }

    #[test]
    fn start_needs_a_session_id_and_an_absolute_cwd() {
        let ok = r#"{"v":1,"event":"start","cwd":"/w/app","session_id":"s1"}"#;
        assert_eq!(
            parse(ok).unwrap(),
            Invocation::Start(StartRequest::new(abs("/w/app"), "s1"))
        );
        for json in [
            r#"{"v":1,"event":"start","cwd":"/w/app"}"#,
            r#"{"v":1,"event":"start","session_id":"s"}"#,
            r#"{"v":1,"event":"start","cwd":"rel","session_id":"s"}"#,
        ] {
            assert_eq!(parse(json).unwrap(), Invocation::Ignore, "{json}");
        }
    }
}
