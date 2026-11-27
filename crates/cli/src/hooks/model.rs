//! Model gate for hook invocations.
//!
//! Resolves the effective [`ModelFilter`](hookah_domain::model::ModelFilter) for the directory a
//! hook targets. When no restriction is configured (the default), every invocation runs without
//! touching the transcript. When a restriction is configured, the invocation runs only if the
//! harness reported a matching model directly or through its session transcript.

use std::io::Write;
use std::path::Path;

use hookah_adapters::config::env::{Host, resolve};
use hookah_adapters::transcript;
use hookah_application::EditRequest;
use hookah_harness::protocol::{Invocation, ModelHint};

/// Whether a hook event targeting `dir` should run for the model described by `hint`.
pub(crate) fn allows(dir: &Path, hint: &ModelHint) -> bool {
    allows_with(dir, hint, &Host::from_process(), |w| {
        let _ = writeln!(std::io::stderr(), "hookah: warning: config: {w}");
    })
}

/// The directory whose config governs `request`.
pub(crate) fn edit_dir(request: &EditRequest) -> &Path {
    request.file.parent().unwrap_or(&request.display_root)
}

/// Drops any part of `invocation` whose target directory's config excludes the model in `hint`.
pub(crate) fn filter_invocation(invocation: Invocation, hint: &ModelHint) -> Invocation {
    filter_invocation_with(invocation, hint, &Host::from_process(), |w| {
        let _ = writeln!(std::io::stderr(), "hookah: warning: config: {w}");
    })
}

fn allows_with(dir: &Path, hint: &ModelHint, host: &Host, mut warn: impl FnMut(&str)) -> bool {
    let found = resolve(dir, host);
    if found.models.filter.is_all() {
        return true;
    }
    let resolved;
    let model = match hint.model.as_deref() {
        Some(m) => Some(m),
        None => {
            resolved = hint
                .transcript
                .as_deref()
                .and_then(transcript::latest_model);
            resolved.as_deref()
        }
    };
    let allowed = found.models.filter.allows(model);
    if !allowed {
        for w in &found.warnings {
            warn(w);
        }
    }
    allowed
}

fn filter_invocation_with(
    invocation: Invocation,
    hint: &ModelHint,
    host: &Host,
    mut warn: impl FnMut(&str),
) -> Invocation {
    match invocation {
        Invocation::Ignore => Invocation::Ignore,
        Invocation::Edit(req) => {
            if allows_with(edit_dir(&req), hint, host, &mut warn) {
                Invocation::Edit(req)
            } else {
                Invocation::Ignore
            }
        }
        Invocation::EditBatch(reqs) => {
            let kept: Vec<_> = reqs
                .into_iter()
                .filter(|r| allows_with(edit_dir(r), hint, host, &mut warn))
                .collect();
            if kept.is_empty() {
                Invocation::Ignore
            } else {
                Invocation::EditBatch(kept)
            }
        }
        Invocation::Stop {
            request,
            already_continuing,
        } => {
            if allows_with(&request.root, hint, host, &mut warn) {
                Invocation::Stop {
                    request,
                    already_continuing,
                }
            } else {
                Invocation::Ignore
            }
        }
        Invocation::Start(req) => {
            if allows_with(&req.cwd, hint, host, &mut warn) {
                Invocation::Start(req)
            } else {
                Invocation::Ignore
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use hookah_application::{StartRequest, StopRequest};

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hookah-model-gate-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        hookah_harness::protocol::canonical_dir(&dir)
    }

    fn quiet_host() -> Host {
        Host {
            var: Box::new(|_| None),
            windows: false,
        }
    }

    #[test]
    fn allows_everything_by_default_and_filters_when_configured() {
        let dir = scratch("basic");
        let file = dir.join("src/lib.rs");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        let req = EditRequest::new(&file, &dir);

        // No config: runs even without a model.
        let kept = filter_invocation_with(
            Invocation::Edit(req.clone()),
            &ModelHint::default(),
            &quiet_host(),
            |_| {},
        );
        assert_eq!(kept, Invocation::Edit(req.clone()));

        // Restricted config: matching model passes, non-matching and unknown become Ignore.
        fs::write(dir.join(".hookah.toml"), "models = [\"claude-sonnet-*\"]\n").unwrap();
        let yes = ModelHint {
            model: Some("claude-sonnet-4-6".into()),
            transcript: None,
        };
        let no = ModelHint {
            model: Some("claude-opus-4-6".into()),
            transcript: None,
        };
        assert_eq!(
            filter_invocation_with(Invocation::Edit(req.clone()), &yes, &quiet_host(), |_| {}),
            Invocation::Edit(req.clone())
        );
        assert_eq!(
            filter_invocation_with(Invocation::Edit(req.clone()), &no, &quiet_host(), |_| {}),
            Invocation::Ignore
        );
        assert_eq!(
            filter_invocation_with(
                Invocation::Edit(req),
                &ModelHint::default(),
                &quiet_host(),
                |_| {}
            ),
            Invocation::Ignore
        );

        // Start and Stop follow the same rule.
        let stop = Invocation::Stop {
            request: StopRequest::new(&dir),
            already_continuing: false,
        };
        assert_eq!(
            filter_invocation_with(stop.clone(), &no, &quiet_host(), |_| {}),
            Invocation::Ignore
        );
        assert_eq!(
            filter_invocation_with(stop.clone(), &yes, &quiet_host(), |_| {}),
            stop
        );
        let start = Invocation::Start(StartRequest::new(&dir, "s"));
        assert_eq!(
            filter_invocation_with(start, &no, &quiet_host(), |_| {}),
            Invocation::Ignore
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn falls_back_to_transcript_when_payload_has_no_model() {
        let dir = scratch("transcript");
        fs::write(dir.join(".hookah.toml"), "models = [\"claude-opus-*\"]\n").unwrap();
        let transcript = dir.join("session.jsonl");
        fs::write(
            &transcript,
            r#"{"type":"assistant","message":{"model":"claude-opus-4-6","content":[]}}"#,
        )
        .unwrap();
        let hint = ModelHint {
            model: None,
            transcript: Some(transcript),
        };
        assert!(allows_with(&dir, &hint, &quiet_host(), |_| {}));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn edit_batch_filters_each_file_against_its_own_project_config() {
        let dir = scratch("batch");
        let a = dir.join("a");
        let b = dir.join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join(".hookah.toml"), "models = [\"gpt-5*\"]\n").unwrap();
        fs::write(b.join(".hookah.toml"), "models = [\"claude-*\"]\n").unwrap();

        let req_a = EditRequest::new(a.join("x.rs"), &dir);
        let req_b = EditRequest::new(b.join("y.rs"), &dir);
        let hint = ModelHint {
            model: Some("gpt-5.3-codex".into()),
            transcript: None,
        };
        let filtered = filter_invocation_with(
            Invocation::EditBatch(vec![req_a.clone(), req_b]),
            &hint,
            &quiet_host(),
            |_| {},
        );
        assert_eq!(filtered, Invocation::EditBatch(vec![req_a]));
        let _ = fs::remove_dir_all(dir);
    }
}
