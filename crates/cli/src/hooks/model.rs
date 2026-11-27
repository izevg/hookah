//! Model gate for hook invocations.
//!
//! Resolves the effective [`ModelFilter`] per target directory and caches both directory configs
//! and the active model for the duration of a single hook invocation.
//!
//! - When a directory has no model restriction (`ModelFilter::All`), `allows` returns `true`
//!   without reading the transcript or session-model cache.
//! - When a directory explicitly disables hooks (`models = []`), `allows` returns `false` without
//!   reading the transcript or session-model cache.
//! - Otherwise the active model is resolved lazily once per hook invocation (direct `model` field,
//!   else session-model / transcript fallback) and reused across every file or nested project.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use hookah_adapters::config::env::{Host, resolve};
use hookah_adapters::store::session_model;
use hookah_adapters::transcript;
use hookah_application::EditRequest;
use hookah_domain::model::ModelFilter;
use hookah_harness::protocol::{Invocation, ModelHint, canonical_dir};

/// Where a hook's active model identifier came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModelSource {
    /// Directly reported in the hook payload.
    Payload,
    /// Cached from an earlier session event (`SessionStart`, `PostModelSwitch`, `PreInvocation`).
    SessionCache,
    /// Read from the latest real model entry in the session transcript JSONL file.
    Transcript(PathBuf),
}

/// Cached per-invocation model evaluator.
pub(crate) struct ModelGate<'a> {
    hint: &'a ModelHint,
    host: Host,
    filters: RefCell<HashMap<PathBuf, (ModelFilter, Vec<String>)>>,
    resolved_model: RefCell<Option<Option<String>>>,
    warned: RefCell<HashSet<String>>,
    any_allowed: Cell<bool>,
    any_restricted: Cell<bool>,
    warn_stderr: bool,
}

impl<'a> ModelGate<'a> {
    /// Builds a gate backed by the current process environment.
    pub(crate) fn new(hint: &'a ModelHint) -> Self {
        Self::with_host(hint, Host::from_process(), true)
    }

    fn with_host(hint: &'a ModelHint, host: Host, warn_stderr: bool) -> Self {
        Self {
            hint,
            host,
            filters: RefCell::new(HashMap::new()),
            resolved_model: RefCell::new(None),
            warned: RefCell::new(HashSet::new()),
            any_allowed: Cell::new(false),
            any_restricted: Cell::new(false),
            warn_stderr,
        }
    }

    /// True if at least one `allows` check on this gate returned `true`.
    pub(crate) fn any_allowed(&self) -> bool {
        self.any_allowed.get()
    }

    /// True if any directory checked by this gate had a model restriction configured.
    pub(crate) fn any_restricted(&self) -> bool {
        self.any_restricted.get()
    }

    /// True if `dir` has no model restriction (`ModelFilter::All`).
    pub(crate) fn is_all(&self, dir: &Path) -> bool {
        let is_all = self.filter_for(dir).0.is_all();
        if !is_all {
            self.any_restricted.set(true);
        }
        is_all
    }

    /// Whether a hook event targeting `dir` should run for the active model.
    pub(crate) fn allows(&self, dir: &Path) -> bool {
        let (filter, warnings) = self.filter_for(dir);
        if filter.is_all() {
            self.any_allowed.set(true);
            return true;
        }
        self.any_restricted.set(true);
        if filter.is_empty() {
            self.emit_warnings(&warnings);
            return false;
        }
        let model = self.active_model();
        let allowed = filter.allows(model.as_deref());
        if allowed {
            self.any_allowed.set(true);
        } else {
            self.emit_warnings(&warnings);
        }
        allowed
    }

    fn filter_for(&self, dir: &Path) -> (ModelFilter, Vec<String>) {
        if let Some(cached) = self.filters.borrow().get(dir) {
            return cached.clone();
        }
        let found = resolve(dir, &self.host);
        let entry = (found.models.filter, found.warnings);
        self.filters
            .borrow_mut()
            .insert(dir.to_path_buf(), entry.clone());
        entry
    }

    fn active_model(&self) -> Option<String> {
        if let Some(cached) = self.resolved_model.borrow().as_ref() {
            return cached.clone();
        }
        let model = resolve_model_with_source(self.hint).map(|(model, _)| model);
        *self.resolved_model.borrow_mut() = Some(model.clone());
        model
    }

    fn emit_warnings(&self, warnings: &[String]) {
        if !self.warn_stderr {
            return;
        }
        let mut seen = self.warned.borrow_mut();
        for w in warnings {
            if seen.insert(w.clone()) {
                let _ = writeln!(std::io::stderr(), "hookah: warning: config: {w}");
            }
        }
    }
}

/// Persists the session's active model when a main-session `SessionStart` or `PostModelSwitch`
/// event carries one and a model filter is active (or a session-model record already exists).
///
/// Writes nothing when `models` is unset across the workspace so unfiltered sessions do not create
/// extra cache files.
pub(crate) fn record_session_model(
    invocation: &Invocation,
    hint: &ModelHint,
    gate: &ModelGate<'_>,
) {
    if hint.is_subagent {
        return;
    }
    let Some(model) = hint.model.as_deref() else {
        return;
    };
    let Some(session) = hint.session.as_deref() else {
        return;
    };
    let cwd = match invocation {
        Invocation::Start(req) => Some(req.cwd.as_path()),
        _ if hint.model_switch => hint.cwd.as_deref(),
        _ => None,
    };
    let Some(cwd) = cwd else {
        return;
    };
    let canonical = canonical_dir(cwd);
    let needed = gate.any_restricted()
        || !gate.is_all(&canonical)
        || (hint.model_switch && session_model::exists(&canonical, session));
    if needed {
        session_model::record(
            &canonical,
            session,
            model,
            hint.model_switch,
            hint.transcript.as_deref(),
        );
    }
}

/// Persists Antigravity's reported `modelName` for the conversation when any workspace has a model
/// filter configured (or when updating an existing session record on `PreInvocation`/`Stop`), so
/// `PostToolUse` events that omit `modelName` still resolve the active model.
pub(crate) fn record_antigravity_model(
    roots: &[PathBuf],
    hint: &ModelHint,
    gate: &ModelGate<'_>,
    is_update: bool,
) {
    let Some(primary) = roots.first() else {
        return;
    };
    let Some(model) = hint.model.as_deref() else {
        return;
    };
    let Some(session) = hint.session.as_deref() else {
        return;
    };
    let canonical = canonical_dir(primary);
    let needed = gate.any_restricted()
        || roots.iter().any(|root| !gate.is_all(root))
        || (is_update && session_model::exists(&canonical, session));
    if needed {
        session_model::record(
            &canonical,
            session,
            model,
            is_update,
            hint.transcript.as_deref(),
        );
    }
}

/// The directory whose config governs `request`.
pub(crate) fn edit_dir(request: &EditRequest) -> &Path {
    request.file.parent().unwrap_or(&request.display_root)
}

/// Filters file-level edit invocations through `gate`.
///
/// `Start` and `Stop` are not gated here because they discover nested projects below `cwd`/`root`
/// and apply `gate` per discovered project inside `StartSession` and `CheckSession`.
pub(crate) fn filter_edits(invocation: Invocation, gate: &ModelGate<'_>) -> Invocation {
    match invocation {
        Invocation::Edit(req) => {
            if gate.allows(edit_dir(&req)) {
                Invocation::Edit(req)
            } else {
                Invocation::Ignore
            }
        }
        Invocation::EditBatch(reqs) => {
            let kept: Vec<_> = reqs
                .into_iter()
                .filter(|r| gate.allows(edit_dir(r)))
                .collect();
            if kept.is_empty() {
                Invocation::Ignore
            } else {
                Invocation::EditBatch(kept)
            }
        }
        other => other,
    }
}

/// Resolves the active model for `hint` together with the source that supplied it.
pub(crate) fn resolve_model_with_source(hint: &ModelHint) -> Option<(String, ModelSource)> {
    if let Some(model) = hint.model.clone() {
        return Some((model, ModelSource::Payload));
    }
    if let Some(cwd) = hint.cwd.as_deref() {
        let canonical = canonical_dir(cwd);
        let resolved = session_model::resolve_with_origin(
            &canonical,
            hint.session.as_deref(),
            hint.transcript.as_deref(),
            !hint.is_subagent,
        )?;
        let source = match resolved.origin {
            session_model::ModelOrigin::SessionCache => ModelSource::SessionCache,
            session_model::ModelOrigin::Transcript => {
                ModelSource::Transcript(hint.transcript.clone().unwrap_or_default())
            }
        };
        return Some((resolved.model, source));
    }
    let path = hint.transcript.as_deref()?;
    let model = transcript::latest_model(path)?;
    Some((model, ModelSource::Transcript(path.to_path_buf())))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hookah-model-gate-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        canonical_dir(&dir)
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

        let empty_hint = ModelHint::default();
        let gate = ModelGate::with_host(&empty_hint, quiet_host(), false);
        assert_eq!(
            filter_edits(Invocation::Edit(req.clone()), &gate),
            Invocation::Edit(req.clone())
        );

        fs::write(dir.join(".hookah.toml"), "models = [\"claude-sonnet-*\"]\n").unwrap();
        let yes = ModelHint {
            model: Some("claude-sonnet-4-6".into()),
            ..ModelHint::default()
        };
        let no = ModelHint {
            model: Some("claude-opus-4-6".into()),
            ..ModelHint::default()
        };
        let yes_gate = ModelGate::with_host(&yes, quiet_host(), false);
        assert_eq!(
            filter_edits(Invocation::Edit(req.clone()), &yes_gate),
            Invocation::Edit(req.clone())
        );
        let no_gate = ModelGate::with_host(&no, quiet_host(), false);
        assert_eq!(
            filter_edits(Invocation::Edit(req.clone()), &no_gate),
            Invocation::Ignore
        );
        let unknown_gate = ModelGate::with_host(&empty_hint, quiet_host(), false);
        assert_eq!(
            filter_edits(Invocation::Edit(req), &unknown_gate),
            Invocation::Ignore
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn falls_back_to_transcript_once_and_skips_it_for_empty_filter() {
        let dir = scratch("transcript");
        fs::write(dir.join(".hookah.toml"), "models = []\n").unwrap();
        let transcript = dir.join("session.jsonl");
        fs::write(
            &transcript,
            r#"{"type":"assistant","message":{"model":"claude-opus-4-6","content":[]}}"#,
        )
        .unwrap();
        let hint = ModelHint {
            transcript: Some(transcript.clone()),
            ..ModelHint::default()
        };

        // Empty filter short-circuits before touching the transcript.
        let empty_gate = ModelGate::with_host(&hint, quiet_host(), false);
        assert!(!empty_gate.allows(&dir));
        assert!(empty_gate.resolved_model.borrow().is_none());

        // Restrictive filter resolves the transcript once and caches it for later files.
        fs::write(dir.join(".hookah.toml"), "models = [\"claude-opus-*\"]\n").unwrap();
        let gate = ModelGate::with_host(&hint, quiet_host(), false);
        assert!(gate.allows(&dir));
        let _ = fs::remove_file(&transcript);
        assert!(
            gate.allows(&dir),
            "reuses cached model instead of rereading"
        );

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
            ..ModelHint::default()
        };
        let gate = ModelGate::with_host(&hint, quiet_host(), false);
        let filtered = filter_edits(Invocation::EditBatch(vec![req_a.clone(), req_b]), &gate);
        assert_eq!(filtered, Invocation::EditBatch(vec![req_a]));
        let _ = fs::remove_dir_all(dir);
    }
}
