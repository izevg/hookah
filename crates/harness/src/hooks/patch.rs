//! Converts apply-patch file headers shared by command-hook adapters.

use std::path::Path;

use hookah_application::EditRequest;

use crate::protocol::{Invocation, PayloadError, display_root, resolve_path};

/// Converts Codex-style patch headers into requests for the files left by the patch.
pub(super) fn invocation(
    patch: &str,
    cwd: Option<&Path>,
    session: Option<&str>,
) -> Result<Invocation, PayloadError> {
    let mut requests = Vec::new();
    for path in patched_paths(patch) {
        let file = resolve_path(Path::new(&path), cwd)?;
        let root = display_root(&file, cwd);
        requests.push(EditRequest::new(file, root).with_session(session.map(str::to_owned)));
    }
    Ok(if requests.is_empty() {
        Invocation::Ignore
    } else {
        Invocation::EditBatch(requests)
    })
}

/// The paths a patch leaves files at: added, updated (or moved to). Deletions are skipped.
fn patched_paths(patch: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    // Whether the last header was an update, so a following `*** Move to:` replaces its path.
    let mut last_is_update = false;
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            paths.push(path.trim().to_string());
            last_is_update = false;
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            paths.push(path.trim().to_string());
            last_is_update = true;
        } else if let Some(path) = line.strip_prefix("*** Move to: ") {
            if last_is_update && let Some(last) = paths.last_mut() {
                *last = path.trim().to_string();
            }
            last_is_update = false;
        } else if line.starts_with("*** Delete File: ") {
            last_is_update = false;
        }
    }
    paths.dedup();
    paths
}
