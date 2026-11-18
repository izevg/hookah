//! Putting a new executable in place of the running one.
//!
//! - Unix: `rename(2)` over the old file is atomic, and the running process keeps its old inode.
//! - Windows: the running `.exe` cannot be overwritten or deleted, but it can be renamed. So the
//!   old file is renamed aside to `hookah.exe.old`, then the new one is renamed in; if the second
//!   step fails the old file is renamed back. The `.old` file is deleted by a later run
//!   ([`remove_leftover`]), once the process that ran it has exited.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Replaces `exe` with the file `new`, which must be on the same filesystem.
pub fn swap(exe: &Path, new: &Path) -> io::Result<()> {
    if cfg!(windows) {
        swap_aside(exe, new)
    } else {
        fs::rename(new, exe)
    }
}

/// The rename-aside swap: the old file stays as `<exe>.old`. Used on Windows; written without
/// Windows-only calls, so Unix tests cover it.
pub fn swap_aside(exe: &Path, new: &Path) -> io::Result<()> {
    let old = leftover_path(exe);
    // A leftover from an earlier update that could not be deleted yet; best effort.
    let _ = fs::remove_file(&old);
    fs::rename(exe, &old)?;
    if let Err(e) = fs::rename(new, exe) {
        // Put the old one back, so the install is never left without a binary.
        let _ = fs::rename(&old, exe);
        return Err(e);
    }
    Ok(())
}

/// Where the replaced executable is moved on Windows: `hookah.exe.old`.
pub fn leftover_path(exe: &Path) -> PathBuf {
    let mut name: OsString = exe.as_os_str().to_owned();
    name.push(".old");
    PathBuf::from(name)
}

/// Deletes the file an earlier Windows update moved aside. Best effort: it may still be in use.
pub fn remove_leftover(exe: &Path) {
    let _ = fs::remove_file(leftover_path(exe));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hookah-upd-swap-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn swap_replaces_the_executable() {
        let dir = scratch();
        let (exe, new) = (dir.join("hookah"), dir.join("new"));
        fs::write(&exe, "old").unwrap();
        fs::write(&new, "new").unwrap();
        swap(&exe, &new).unwrap();
        assert_eq!(fs::read_to_string(&exe).unwrap(), "new");
        assert!(!new.exists());
        if cfg!(windows) {
            assert_eq!(fs::read_to_string(leftover_path(&exe)).unwrap(), "old");
        } else {
            assert!(!leftover_path(&exe).exists());
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_aside_swap_keeps_the_old_file_and_clears_a_stale_one() {
        let dir = scratch();
        let (exe, new) = (dir.join("hookah.exe"), dir.join("new.exe"));
        fs::write(&exe, "old").unwrap();
        fs::write(&new, "new").unwrap();
        fs::write(dir.join("hookah.exe.old"), "stale").unwrap();
        swap_aside(&exe, &new).unwrap();
        assert_eq!(fs::read_to_string(&exe).unwrap(), "new");
        assert_eq!(
            fs::read_to_string(dir.join("hookah.exe.old")).unwrap(),
            "old"
        );
        remove_leftover(&exe);
        assert!(!dir.join("hookah.exe.old").exists());
        assert!(exe.exists());
        remove_leftover(&exe); // nothing left: fine
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_aside_swap_restores_the_old_executable() {
        let dir = scratch();
        let exe = dir.join("hookah.exe");
        fs::write(&exe, "old").unwrap();
        let missing = dir.join("never-written.exe");
        assert!(swap_aside(&exe, &missing).is_err());
        assert_eq!(fs::read_to_string(&exe).unwrap(), "old");
        assert!(!dir.join("hookah.exe.old").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_executable_is_an_error_and_nothing_appears() {
        let dir = scratch();
        let new = dir.join("new");
        fs::write(&new, "new").unwrap();
        assert!(swap_aside(&dir.join("hookah.exe"), &new).is_err());
        assert!(new.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_leftover_is_the_executable_name_plus_old() {
        assert_eq!(
            leftover_path(Path::new("/a/hookah.exe")),
            PathBuf::from("/a/hookah.exe.old")
        );
    }
}
