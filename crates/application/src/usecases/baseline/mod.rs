//! The session baseline: the static findings that existed when the agent session started, so
//! later phases report only what the agent introduced.
//!
//! - [`StartSession`] (session-start event): claims the baseline of the session's project and
//!   starts the background job.
//! - [`ComputeBaseline`] (the background job): runs the Lint, Typecheck, and Architecture tools
//!   over the whole project and stores the fingerprints.
//! - [`lookup`] (edit and stop phases): the baseline when it is ready. When a harness has no
//!   session-start event, the first edit of a session starts the job instead (lazily); that
//!   first edit is not baseline-filtered.
//!
//! Every failure is fail-open: a baseline that is missing, still running, or failed makes the
//! phases fall back to the changed-lines filter.

mod compute;
mod start;

use std::path::Path;

use hookah_domain::finding::baseline::Baseline;
use hookah_domain::project::Project;

pub use compute::ComputeBaseline;
pub use start::StartSession;

use crate::ports::{BaselineState, Ports};

/// A file name that is never read: it only gives the locator a starting directory.
const PROBE: &str = ".hookah-probe";

/// The probe path inside `dir`, for `ProjectLocator::manifests_above`.
fn probe(dir: &Path) -> std::path::PathBuf {
    dir.join(PROBE)
}

/// Claims the baseline of (`root`, `session`) and starts its job. `Ok(true)` when a job was
/// started, `Ok(false)` when one exists already. A job that cannot start is recorded as failed.
fn ensure(ports: &Ports<'_>, root: &Path, session: &str) -> Result<bool, String> {
    match ports.baselines.begin(root, session) {
        Err(e) => Err(format!("baseline store: {e}")),
        Ok(false) => Ok(false),
        Ok(true) => match ports.spawner.spawn(root, session) {
            Ok(()) => Ok(true),
            Err(e) => {
                let reason = format!("cannot start the baseline job: {e}");
                let _ = ports.baselines.fail(root, session, &reason);
                Err(reason)
            }
        },
    }
}

/// The ready baseline of the project for `session`, if any. Without a session id there is none.
/// With `start_if_absent`, an absent baseline gets its job started (and is still not available
/// to this call).
pub(crate) fn lookup(
    ports: &Ports<'_>,
    project: &Project,
    session: Option<&str>,
    start_if_absent: bool,
) -> Option<Baseline> {
    let session = session.filter(|s| !s.is_empty())?;
    match ports.baselines.state(&project.root, session) {
        BaselineState::Ready(baseline) => Some(baseline),
        BaselineState::InProgress | BaselineState::Failed(_) => None,
        BaselineState::Absent => {
            if start_if_absent {
                let _ = ensure(ports, &project.root, session);
            }
            None
        }
    }
}
