//! Process runner.
//!
//! Implements `CheckRunner` with `std::process`. Each command starts in its own process group; on
//! timeout the whole group is killed, so child processes of a test runner do not linger. Stdout
//! and stderr are drained on separate threads so a full pipe can never block the child. Does not
//! interpret the output; ecosystem parsers do.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use std::os::unix::process::CommandExt;

use hookah_application::error::PortError;
use hookah_application::ports::{CheckRunner, ProcessOutput};
use hookah_domain::toolchain::CommandSpec;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

const POLL_INTERVAL: Duration = Duration::from_millis(5);
/// How long to wait for the pipes to close after the process is gone (a stray grandchild may
/// keep them open).
const DRAIN_GRACE: Duration = Duration::from_secs(1);

/// Runs commands as child processes.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessRunner;

impl CheckRunner for ProcessRunner {
    fn run(&self, spec: &CommandSpec, targets: &[String]) -> Result<ProcessOutput, PortError> {
        let mut child = Command::new(&spec.program)
            .args(spec.expand_args(targets))
            .current_dir(&spec.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|e| PortError::new(format!("cannot start `{}`: {e}", spec.program)))?;

        let out = drain(child.stdout.take());
        let err = drain(child.stderr.take());

        let (status, timed_out) = wait(&mut child, spec.timeout)
            .map_err(|e| PortError::new(format!("waiting for `{}`: {e}", spec.program)))?;

        Ok(ProcessOutput {
            exit_code: if timed_out { None } else { status },
            stdout: collect(out),
            stderr: collect(err),
            timed_out,
        })
    }
}

/// Reads a pipe to the end on its own thread.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut p) = pipe {
            let _ = p.read_to_end(&mut buf);
        }
        let _ = tx.send(buf);
    });
    rx
}

fn collect(rx: mpsc::Receiver<Vec<u8>>) -> String {
    let bytes = rx.recv_timeout(DRAIN_GRACE).unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Waits for the child; kills its process group when `timeout` passes. Returns the exit code and
/// whether the timeout fired.
fn wait(child: &mut Child, timeout: Duration) -> std::io::Result<(Option<i32>, bool)> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status.code(), false));
        }
        if Instant::now() >= deadline {
            kill_group(child);
            let _ = child.wait();
            return Ok((None, true));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn kill_group(child: &mut Child) {
    kill_with(child, |pid| killpg(pid, Signal::SIGKILL).is_ok());
}

/// Kills the child's process group with `group_kill`; falls back to killing the child alone
/// when that fails.
fn kill_with(child: &mut Child, group_kill: impl FnOnce(Pid) -> bool) {
    let killed = i32::try_from(child.id())
        .ok()
        .is_some_and(|pid| group_kill(Pid::from_raw(pid)));
    if !killed {
        let _ = child.kill();
    }
}

#[cfg(test)]
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
