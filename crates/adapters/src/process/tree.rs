//! Starting a child so that its whole process tree can be killed.
//!
//! - Unix: the child leads its own process group; killing means `killpg(SIGKILL)`.
//! - Windows: the child is assigned to a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`;
//!   closing the job kills every process in it, grandchildren included. The `win32job` crate
//!   wraps the Win32 calls safely (`unsafe` is forbidden here). The child is assigned right
//!   after it starts, so a grandchild spawned within those microseconds could escape; for a
//!   test runner that is not a practical risk.
//!
//! Either way a failure to set up the group or job degrades to killing the child alone.

use std::io;
use std::process::{Child, Command};

/// Handle on the process tree of one child.
#[derive(Debug)]
pub(super) struct Tree {
    #[cfg(windows)]
    job: Option<win32job::Job>,
}

/// Spawns `command` as the root of a killable process tree.
pub(super) fn spawn(command: &mut Command) -> io::Result<(Child, Tree)> {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
    let child = command.spawn()?;
    #[cfg(windows)]
    let tree = Tree {
        job: contain(&child),
    };
    #[cfg(unix)]
    let tree = Tree {};
    Ok((child, tree))
}

/// Windows only; not covered by mutation testing, which runs on Unix (`.cargo/mutants.toml`).
#[cfg(windows)]
fn contain(child: &Child) -> Option<win32job::Job> {
    let job = win32job::Job::create().ok()?;
    let mut info = job.query_extended_limit_info().ok()?;
    info.limit_kill_on_job_close();
    job.set_extended_limit_info(&info).ok()?;
    let handle = std::os::windows::io::AsRawHandle::as_raw_handle(child) as isize;
    job.assign_process(handle).ok()?;
    Some(job)
}

impl Tree {
    /// Kills the child and everything it started.
    pub(super) fn kill(&mut self, child: &mut Child) {
        #[cfg(unix)]
        kill_with(child, |pid| {
            nix::sys::signal::killpg(pid, nix::sys::signal::Signal::SIGKILL).is_ok()
        });
        #[cfg(windows)]
        {
            // Dropping the job closes its handle, which kills the whole tree.
            drop(self.job.take());
            let _ = child.kill();
        }
    }
}

/// Kills the child's process group with `group_kill`; falls back to killing the child alone
/// when that fails.
#[cfg(unix)]
fn kill_with(child: &mut Child, group_kill: impl FnOnce(nix::unistd::Pid) -> bool) {
    let killed = i32::try_from(child.id())
        .ok()
        .is_some_and(|pid| group_kill(nix::unistd::Pid::from_raw(pid)));
    if !killed {
        let _ = child.kill();
    }
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn sleeper() -> Child {
        Command::new("sleep").arg("30").spawn().unwrap()
    }

    #[test]
    fn falls_back_to_killing_the_child_when_the_group_kill_fails() {
        let mut child = sleeper();
        kill_with(&mut child, |_| false);
        assert!(child.wait().unwrap().code().is_none(), "killed by signal");
    }

    #[test]
    fn leaves_the_child_alone_when_the_group_kill_worked() {
        let mut child = sleeper();
        kill_with(&mut child, |_| true);
        assert!(child.try_wait().unwrap().is_none(), "still running");
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
