//! `Installer`: verify, extract, check, and swap in a new `hookah`.
//!
//! Steps of [`SelfInstaller::install`], each of which leaves the installed binary untouched when
//! it fails:
//!
//! 1. the archive's SHA-256 must equal the one the release lists;
//! 2. the executable is read out of the archive in memory ([`archive`]);
//! 3. it is written next to the running one (same filesystem, so the final rename is atomic),
//!    mode 0755 on Unix;
//! 4. it is run with `--version` and must report exactly the expected version;
//! 5. it is renamed over the running executable ([`replace`]).
//!
//! The running executable is `std::env::current_exe`, resolved through symlinks, so a symlinked
//! install updates the file it points to. A binary under a package manager's directory is not
//! touched (see [`hookah_domain::update::managed_by`]).

pub mod archive;
pub mod replace;

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use hookah_application::ports::{Installer, Package, UpdateError};
use hookah_domain::update::{Version, managed_by};
use sha2::{Digest, Sha256};

/// How long the new executable gets to print its version.
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// Replaces one executable with a release's.
#[derive(Debug, Clone)]
pub struct SelfInstaller {
    exe: Result<PathBuf, String>,
}

impl SelfInstaller {
    /// Updates the running executable.
    pub fn current() -> Self {
        Self {
            exe: std::env::current_exe()
                .and_then(dunce::canonicalize)
                .map_err(|e| format!("cannot locate the running hookah: {e}")),
        }
    }

    /// Updates the executable at `exe` (tests).
    pub fn at(exe: impl Into<PathBuf>) -> Self {
        Self {
            exe: Ok(exe.into()),
        }
    }

    /// Deletes what an earlier Windows update left next to the executable. Best effort.
    pub fn clean_up(&self) {
        if let Ok(exe) = &self.exe {
            replace::remove_leftover(exe);
        }
    }

    fn exe(&self) -> Result<&Path, UpdateError> {
        self.exe
            .as_deref()
            .map_err(|e| UpdateError::failed(e.clone()))
    }
}

impl Installer for SelfInstaller {
    fn preflight(&self) -> Result<(), UpdateError> {
        let exe = self.exe()?;
        if let Some(manager) = managed_by(exe) {
            return Err(UpdateError::blocked(format!(
                "hookah at {} is managed by {manager}; update it with {manager}",
                exe.display()
            )));
        }
        let dir = parent(exe)?;
        let probe = dir.join(format!(".hookah-write-probe-{}", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&probe) {
            Ok(_) => {
                let _ = fs::remove_file(&probe);
                Ok(())
            }
            Err(e) => Err(UpdateError::blocked(format!(
                "{} is not writable ({e}); run the installer again to update hookah",
                dir.display()
            ))),
        }
    }

    fn install(&self, package: &Package<'_>) -> Result<(), UpdateError> {
        let exe = self.exe()?;
        let actual = sha256_hex(package.bytes);
        if actual != package.sha256 {
            return Err(UpdateError::failed(format!(
                "checksum mismatch for the downloaded archive: SHA256SUMS lists {}, the download is {actual}",
                package.sha256
            )));
        }
        let binary = archive::extract(package.bytes, package.kind, package.entry)?;
        let staged = Staged::write(parent(exe)?, package.entry, &binary)?;
        reports_version(&staged.path, package.version)?;
        replace::swap(exe, &staged.path).map_err(|e| {
            let message = format!("cannot replace {}: {e}", exe.display());
            if blocked_by_permissions(&e) {
                UpdateError::blocked(message)
            } else {
                UpdateError::failed(message)
            }
        })?;
        Ok(())
    }
}

fn parent(exe: &Path) -> Result<&Path, UpdateError> {
    exe.parent()
        .ok_or_else(|| UpdateError::failed(format!("{} has no directory", exe.display())))
}

fn blocked_by_permissions(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
    )
}

/// Lowercase hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The new executable, written next to the old one. Deleted on drop: after a successful swap the
/// path no longer exists, so that is a no-op, and after any failure it removes the leftover.
struct Staged {
    path: PathBuf,
}

impl Staged {
    fn write(dir: &Path, entry: &str, binary: &[u8]) -> Result<Self, UpdateError> {
        let suffix = if entry.ends_with(".exe") { ".exe" } else { "" };
        let path = dir.join(format!(".hookah-update-{}{suffix}", std::process::id()));
        let _ = fs::remove_file(&path);
        let staged = Self { path };
        let write = || -> io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged.path)?;
            file.write_all(binary)?;
            file.sync_all()?;
            #[cfg(unix)]
            file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
            Ok(())
        };
        write().map_err(|e| {
            let message = format!("cannot write {}: {e}", staged.path.display());
            if blocked_by_permissions(&e) {
                UpdateError::blocked(message)
            } else {
                UpdateError::failed(message)
            }
        })?;
        Ok(staged)
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Runs `<path> --version` and requires it to print `hookah <version>`.
fn reports_version(path: &Path, version: &Version) -> Result<(), UpdateError> {
    let expected = format!("hookah {version}");
    let output = run_version(path, VERSION_TIMEOUT)
        .map_err(|e| UpdateError::failed(format!("the downloaded hookah does not run: {e}")))?;
    let printed = output.trim();
    if printed == expected {
        Ok(())
    } else {
        Err(UpdateError::failed(format!(
            "the downloaded hookah reports {printed:?}, expected {expected:?}"
        )))
    }
}

fn run_version(path: &Path, timeout: Duration) -> io::Result<String> {
    let mut child = spawn_retrying(path)?;
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(io::ErrorKind::TimedOut, "no answer in time"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!("exited with {}", output.status)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Spawns the executable, retrying while it is busy: a freshly written file can be briefly
/// "text file busy" (ETXTBSY) when another thread forked while it was open for writing.
fn spawn_retrying(path: &Path) -> io::Result<std::process::Child> {
    retry_busy(Duration::from_millis(20), || {
        Command::new(path)
            .arg("--version")
            .env(super::settings::NO_UPDATE_VAR, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
    })
}

/// Calls `attempt` until it stops failing with "executable file busy", with at most 100 retries.
fn retry_busy<T>(delay: Duration, mut attempt: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    for _ in 0..100 {
        match attempt() {
            Err(e) if e.kind() == io::ErrorKind::ExecutableFileBusy => std::thread::sleep(delay),
            other => return other,
        }
    }
    attempt()
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use hookah_domain::update::ArchiveKind;

    use super::archive::fixture::tar_gz as tar_gz_with;
    use super::*;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hookah-upd-inst-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn script(version: &str) -> Vec<u8> {
        format!("#!/bin/sh\necho 'hookah {version}'\n").into_bytes()
    }

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn installed(dir: &Path) -> PathBuf {
        let exe = dir.join("hookah");
        fs::write(&exe, "#!/bin/sh\necho 'hookah 1.0.0'\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
        exe
    }

    fn package<'a>(archive: &'a [u8], sha: &'a str, version: &'a Version) -> Package<'a> {
        Package {
            bytes: archive,
            kind: ArchiveKind::TarGz,
            sha256: sha,
            entry: "hookah",
            version,
        }
    }

    #[test]
    fn sha256_matches_the_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_good_package_replaces_the_executable_with_mode_0755() {
        let dir = scratch();
        let exe = installed(&dir);
        let archive = tar_gz_with(&[("hookah-2.0.0-x/hookah", &script("2.0.0"))]);
        let sha = sha256_hex(&archive);
        let version = v("2.0.0");
        SelfInstaller::at(&exe)
            .install(&package(&archive, &sha, &version))
            .unwrap();
        assert_eq!(fs::read(&exe).unwrap(), script("2.0.0"));
        assert_eq!(
            fs::metadata(&exe).unwrap().permissions().mode() & 0o777,
            0o755
        );
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["hookah"], "no staged file left behind");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_checksum_mismatch_changes_nothing() {
        let dir = scratch();
        let exe = installed(&dir);
        let before = fs::read(&exe).unwrap();
        let archive = tar_gz_with(&[("d/hookah", &script("2.0.0"))]);
        let wrong = "0".repeat(64);
        let version = v("2.0.0");
        let e = SelfInstaller::at(&exe)
            .install(&package(&archive, &wrong, &version))
            .unwrap_err();
        assert!(e.message().contains("checksum mismatch"), "{e}");
        assert!(matches!(e, UpdateError::Failed(_)));
        assert_eq!(fs::read(&exe).unwrap(), before);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_binary_reporting_another_version_is_not_installed() {
        let dir = scratch();
        let exe = installed(&dir);
        let before = fs::read(&exe).unwrap();
        let archive = tar_gz_with(&[("d/hookah", &script("9.9.9"))]);
        let sha = sha256_hex(&archive);
        let version = v("2.0.0");
        let e = SelfInstaller::at(&exe)
            .install(&package(&archive, &sha, &version))
            .unwrap_err();
        assert!(
            e.message().contains("9.9.9") && e.message().contains("2.0.0"),
            "{e}"
        );
        assert_eq!(fs::read(&exe).unwrap(), before);
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            1,
            "staged file removed"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_binary_that_does_not_run_is_not_installed() {
        let dir = scratch();
        let exe = installed(&dir);
        for body in [&b"not an executable"[..], &b"#!/bin/sh\nexit 3\n"[..]] {
            let archive = tar_gz_with(&[("d/hookah", body)]);
            let sha = sha256_hex(&archive);
            let version = v("2.0.0");
            let e = SelfInstaller::at(&exe)
                .install(&package(&archive, &sha, &version))
                .unwrap_err();
            assert!(e.message().contains("does not run"), "{e}");
        }
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn an_archive_without_the_executable_is_not_installed() {
        let dir = scratch();
        let exe = installed(&dir);
        let archive = tar_gz_with(&[("d/README.md", b"x")]);
        let sha = sha256_hex(&archive);
        let version = v("2.0.0");
        let e = SelfInstaller::at(&exe)
            .install(&package(&archive, &sha, &version))
            .unwrap_err();
        assert!(e.message().contains("no hookah"), "{e}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_busy_executable_is_retried_a_bounded_number_of_times() {
        let busy = || io::Error::from(io::ErrorKind::ExecutableFileBusy);
        let mut calls = 0;
        let result = retry_busy(Duration::ZERO, || {
            calls += 1;
            if calls <= 3 { Err(busy()) } else { Ok(calls) }
        });
        assert_eq!(result.unwrap(), 4);
        let mut calls = 0;
        let result: io::Result<()> = retry_busy(Duration::ZERO, || {
            calls += 1;
            Err(busy())
        });
        assert_eq!(
            result.unwrap_err().kind(),
            io::ErrorKind::ExecutableFileBusy
        );
        assert_eq!(calls, 101, "the first try and 100 retries");
        // Any other error is final at once.
        let mut calls = 0;
        let result: io::Result<()> = retry_busy(Duration::ZERO, || {
            calls += 1;
            Err(io::Error::from(io::ErrorKind::NotFound))
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(calls, 1);
    }

    #[test]
    fn only_permission_errors_block() {
        for kind in [
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::ReadOnlyFilesystem,
        ] {
            assert!(blocked_by_permissions(&io::Error::from(kind)), "{kind:?}");
        }
        for kind in [
            io::ErrorKind::NotFound,
            io::ErrorKind::TimedOut,
            io::ErrorKind::Other,
        ] {
            assert!(!blocked_by_permissions(&io::Error::from(kind)), "{kind:?}");
        }
    }

    #[test]
    fn writing_into_a_read_only_directory_is_blocked_and_leaves_nothing() {
        let dir = scratch();
        let exe = installed(&dir);
        let archive = tar_gz_with(&[("d/hookah", &script("2.0.0"))]);
        let sha = sha256_hex(&archive);
        let version = v("2.0.0");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        let result = SelfInstaller::at(&exe).install(&package(&archive, &sha, &version));
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        // A superuser writes anywhere; the case cannot be staged then.
        if let Err(e) = result {
            assert!(matches!(e, UpdateError::Blocked(_)), "{e}");
            assert!(e.message().contains("cannot write"), "{e}");
        }
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn preflight_accepts_a_writable_directory_and_leaves_nothing() {
        let dir = scratch();
        let exe = installed(&dir);
        SelfInstaller::at(&exe).preflight().unwrap();
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn preflight_blocks_a_directory_that_is_not_writable() {
        let dir = scratch();
        let exe = installed(&dir);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        let result = SelfInstaller::at(&exe).preflight();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        // Root ignores directory permissions; nothing to assert then.
        if let Err(e) = result {
            assert!(matches!(e, UpdateError::Blocked(_)));
            assert!(e.message().contains("not writable") && e.message().contains("installer"));
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn preflight_blocks_a_package_managed_path() {
        let e = SelfInstaller::at("/home/u/.cargo/bin/hookah")
            .preflight()
            .unwrap_err();
        assert!(matches!(e, UpdateError::Blocked(_)));
        assert!(e.message().contains("Cargo"));
    }

    #[test]
    fn an_unlocatable_executable_is_a_failure() {
        let installer = SelfInstaller {
            exe: Err("cannot locate the running hookah: gone".into()),
        };
        assert!(matches!(installer.preflight(), Err(UpdateError::Failed(_))));
        installer.clean_up();
    }

    #[test]
    fn current_resolves_to_a_real_file() {
        let installer = SelfInstaller::current();
        let exe = installer.exe().unwrap();
        assert!(exe.is_file() && exe.is_absolute());
    }

    #[test]
    fn a_hanging_binary_is_killed_at_the_timeout() {
        let dir = scratch();
        let exe = dir.join("hang");
        fs::write(&exe, "#!/bin/sh\nexec sleep 20\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        let started = Instant::now();
        let e = run_version(&exe, Duration::from_millis(300)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(10));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_quick_binary_is_read_in_full() {
        let dir = scratch();
        let exe = dir.join("quick");
        fs::write(&exe, script("1.2.3")).unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        reports_version(&exe, &v("1.2.3")).unwrap();
        let _ = fs::remove_dir_all(dir);
    }
}
