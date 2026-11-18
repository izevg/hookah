//! Process runner.
//!
//! Implements `CheckRunner` with `std::process`. Each command starts as the root of a killable process tree (a
//! process group on Unix, a Job Object on Windows, see `tree`); on timeout the whole tree is killed, so child processes of a test runner do not linger. Stdout
//! and stderr are drained on separate threads so a full pipe can never block the child. Does not
//! interpret the output; ecosystem parsers do.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use hookah_application::error::PortError;
use hookah_application::ports::{CheckRunner, ProcessOutput};
use hookah_domain::toolchain::CommandSpec;

use super::tree::{Tree, spawn};

const POLL_INTERVAL: Duration = Duration::from_millis(5);
/// How long to wait for the pipes to close after the process is gone (a stray grandchild may
/// keep them open).
const DRAIN_GRACE: Duration = Duration::from_secs(1);

/// Runs commands as child processes.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessRunner;

impl CheckRunner for ProcessRunner {
    fn run(&self, spec: &CommandSpec, targets: &[String]) -> Result<ProcessOutput, PortError> {
        let mut command = Command::new(&spec.program);
        command
            .args(spec.expand_args(targets))
            .envs(spec.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&spec.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let (mut child, tree) = spawn(&mut command)
            .map_err(|e| PortError::new(format!("cannot start `{}`: {e}", spec.program)))?;

        let out = drain(child.stdout.take());
        let err = drain(child.stderr.take());

        let (status, timed_out) = wait(&mut child, tree, spec.timeout)
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

/// Waits for the child; kills its process tree when `timeout` passes. Returns the exit code and
/// whether the timeout fired.
fn wait(
    child: &mut Child,
    mut tree: Tree,
    timeout: Duration,
) -> std::io::Result<(Option<i32>, bool)> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status.code(), false));
        }
        if Instant::now() >= deadline {
            tree.kill(child);
            let _ = child.wait();
            return Ok((None, true));
        }
        thread::sleep(POLL_INTERVAL);
    }
}
