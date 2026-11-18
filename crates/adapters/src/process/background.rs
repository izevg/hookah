//! `BaselineSpawner` with a detached child process.
//!
//! Runs `<exe> baseline --session=<id> --root <dir>` with all three standard streams on the null
//! device, so a harness that waits for the hook's pipes to close is not held up. The parent never
//! waits for the child.
//!
//! - Unix: the child leads its own process group, so a signal sent to the hook's group (a
//!   harness killing a timed-out hook) does not reach it.
//! - Windows: the child is created `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`, which cuts it
//!   from the hook's console and Ctrl+C group. This path compiles in principle and has not been
//!   run on Windows yet.
//!
//! With `wait`, the call blocks until the child exits; tests and the sample runner use it to get a
//! baseline synchronously.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use hookah_application::error::PortError;
use hookah_application::ports::BaselineSpawner;

/// Starts baseline jobs as child processes of the `hookah` binary.
#[derive(Debug, Clone)]
pub struct BackgroundSpawner {
    exe: PathBuf,
    wait: bool,
}

impl BackgroundSpawner {
    /// Spawns `exe`; with `wait` the call returns only after the child exits.
    pub fn new(exe: impl Into<PathBuf>, wait: bool) -> Self {
        Self {
            exe: exe.into(),
            wait,
        }
    }

    fn command(&self, root: &Path, session: &str) -> Command {
        let mut command = Command::new(&self.exe);
        command
            .arg("baseline")
            .arg(format!("--session={session}"))
            .arg("--root")
            .arg(root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        detach(&mut command);
        #[cfg(windows)]
        detach_from_console(&mut command);
        command
    }
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    std::os::unix::process::CommandExt::process_group(command, 0);
}

/// Windows only; not covered by mutation testing, which runs on Unix (`.cargo/mutants.toml`).
#[cfg(windows)]
fn detach_from_console(command: &mut Command) {
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    std::os::windows::process::CommandExt::creation_flags(
        command,
        DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP,
    );
}

impl BaselineSpawner for BackgroundSpawner {
    fn spawn(&self, root: &Path, session: &str) -> Result<(), PortError> {
        let mut child = self
            .command(root, session)
            .spawn()
            .map_err(|e| PortError::new(format!("cannot run {}: {e}", self.exe.display())))?;
        if self.wait {
            child
                .wait()
                .map_err(|e| PortError::new(format!("cannot wait for the baseline job: {e}")))?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn a_missing_program_is_an_error() {
        let spawner = BackgroundSpawner::new("/nonexistent/hookah", false);
        let err = spawner.spawn(Path::new("/r"), "s").unwrap_err();
        assert!(err.message().contains("/nonexistent/hookah"));
    }

    #[cfg(unix)]
    fn script(tag: &str, body: &str) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("hookah-spawn-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("fake-hookah");
        fs::write(&exe, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        (dir, exe)
    }

    #[cfg(unix)]
    #[test]
    fn passes_the_job_arguments_and_waits_when_told_to() {
        let (dir, exe) = script(
            "wait",
            "sleep 0.3; echo \"$@\" > \"$(dirname \"$0\")/args\"",
        );
        let started = Instant::now();
        BackgroundSpawner::new(&exe, true)
            .spawn(Path::new("/some/root"), "ses 1")
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(300));
        let args = fs::read_to_string(dir.join("args")).unwrap();
        assert_eq!(args.trim(), "baseline --session=ses 1 --root /some/root");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn returns_at_once_without_wait_and_the_child_keeps_running() {
        let (dir, exe) = script("detach", "sleep 1; echo done > \"$(dirname \"$0\")/done\"");
        let started = Instant::now();
        BackgroundSpawner::new(&exe, false)
            .spawn(Path::new("/r"), "s")
            .unwrap();
        assert!(started.elapsed() < Duration::from_millis(800));
        let done = dir.join("done");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(done.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_child_leads_its_own_process_group_with_null_streams() {
        let (dir, exe) = script(
            "group",
            "ps -o pid=,pgid= -p $$ > \"$(dirname \"$0\")/ids\"; [ -t 0 ] || echo notty >> \"$(dirname \"$0\")/ids\"",
        );
        BackgroundSpawner::new(&exe, true)
            .spawn(Path::new("/r"), "s")
            .unwrap();
        let ids = fs::read_to_string(dir.join("ids")).unwrap();
        let mut fields = ids.split_whitespace();
        let (pid, pgid) = (fields.next().unwrap(), fields.next().unwrap());
        assert_eq!(pid, pgid);
        assert_eq!(fields.next(), Some("notty"));
        let _ = fs::remove_dir_all(&dir);
    }
}
