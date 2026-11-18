//! Writing a multi-file plugin folder, for `hookah plugin orca --out <dir>`.
//!
//! Only the files of the plugin are written; nothing else in the folder is touched or removed.
//! A file that already has the right content is left alone, so running it twice changes nothing.

use std::fs;
use std::path::{Path, PathBuf};

use hookah_harness::orca::{PLUGIN_FILES, PluginFile};

use crate::error::SetupError;
use crate::io::uninstall::{Removal, plugin_steps};

/// What happened to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The file.
    pub path: PathBuf,
    /// False when it already had this content.
    pub wrote: bool,
}

/// Writes `files` below `dir`, creating directories as needed.
pub fn write_plugin(dir: &Path, files: &[PluginFile]) -> Result<Vec<Installed>, SetupError> {
    files
        .iter()
        .map(|file| {
            let path = file
                .path
                .split('/')
                .fold(dir.to_path_buf(), |p, s| p.join(s));
            let io = |source| SetupError::Io {
                path: path.clone(),
                source,
            };
            // Any read error (missing, unreadable) means: write it, and let the write report.
            if fs::read_to_string(&path).is_ok_and(|text| text == file.text) {
                return Ok(Installed { path, wrote: false });
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(io)?;
            }
            fs::write(&path, file.text).map_err(io)?;
            Ok(Installed { path, wrote: true })
        })
        .collect()
}

/// What removing the Orca plugin folder `dir` (written by `hookah plugin orca --out`) would do.
pub fn plan_orca_removal(dir: &Path, force: bool) -> Removal {
    plan_plugin_removal(dir, PLUGIN_FILES, force)
}

/// What removing the plugin folder `dir` would do: each of `files` that still has the content
/// [`write_plugin`] wrote goes, and `dir` goes when it ends up empty. A file the user changed is
/// kept (unless `force`), and so is everything else in the folder.
pub fn plan_plugin_removal(dir: &Path, files: &[PluginFile], force: bool) -> Removal {
    let found: Vec<(PathBuf, &str)> = files
        .iter()
        .map(|f| {
            let path = f.path.split('/').fold(dir.to_path_buf(), |p, s| p.join(s));
            (path, f.text)
        })
        .collect();
    Removal::new(
        "orca",
        None,
        None,
        plugin_steps(&found, force),
        Some(dir.to_path_buf()),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const FILES: &[PluginFile] = &[
        PluginFile {
            path: "m.json",
            text: "{}\n",
        },
        PluginFile {
            path: "src/a.ts",
            text: "a\n",
        },
    ];

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hookah-plugin-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn writes_nested_files_then_reports_nothing_to_do() {
        let d = dir("nested");
        let first = write_plugin(&d, FILES).unwrap();
        assert!(first.iter().all(|f| f.wrote));
        assert_eq!(
            fs::read_to_string(d.join("src").join("a.ts")).unwrap(),
            "a\n"
        );
        let second = write_plugin(&d, FILES).unwrap();
        assert!(second.iter().all(|f| !f.wrote));
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn replaces_a_stale_file_and_leaves_other_files_alone() {
        let d = dir("stale");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("m.json"), "old").unwrap();
        fs::write(d.join("mine.txt"), "keep").unwrap();
        let done = write_plugin(&d, FILES).unwrap();
        assert!(done[0].wrote && done[1].wrote);
        assert_eq!(fs::read_to_string(d.join("m.json")).unwrap(), "{}\n");
        assert_eq!(fs::read_to_string(d.join("mine.txt")).unwrap(), "keep");
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn removal_deletes_matching_files_and_the_empty_folder_and_keeps_changed_ones() {
        use crate::io::uninstall::{Step, remove};
        let d = dir("remove");
        write_plugin(&d, FILES).unwrap();
        let plan = plan_plugin_removal(&d, FILES, false);
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.name, "orca");
        remove(&plan).unwrap();
        assert!(!d.exists());
        assert!(plan_plugin_removal(&d, FILES, false).steps.is_empty());

        write_plugin(&d, FILES).unwrap();
        fs::write(d.join("src").join("a.ts"), "mine").unwrap();
        fs::write(d.join("extra.txt"), "keep").unwrap();
        let plan = plan_plugin_removal(&d, FILES, false);
        assert!(plan.refused());
        assert!(matches!(plan.steps[1], Step::Keep { .. }));
        remove(&plan).unwrap();
        assert!(!d.join("m.json").exists());
        assert_eq!(
            fs::read_to_string(d.join("src").join("a.ts")).unwrap(),
            "mine"
        );
        assert!(d.join("extra.txt").is_file());
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn the_orca_removal_covers_every_orca_plugin_file() {
        let d = dir("orca");
        write_plugin(&d, PLUGIN_FILES).unwrap();
        let plan = plan_orca_removal(&d, false);
        assert_eq!(plan.steps.len(), PLUGIN_FILES.len());
        assert!(!plan.refused());
        crate::io::uninstall::remove(&plan).unwrap();
        assert!(!d.exists());
    }

    #[test]
    fn a_path_that_is_a_file_is_an_error() {
        let d = dir("blocked");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("src"), "file").unwrap();
        assert!(write_plugin(&d, FILES).is_err());
        let _ = fs::remove_dir_all(d);
    }
}
