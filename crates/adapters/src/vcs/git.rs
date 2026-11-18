//! `ChangeSource` backed by the `git` command line.
//!
//! Every call runs `git -C <dir>` with the pager, colors and path quoting disabled. Changes are
//! measured as worktree versus `HEAD`, so staged and unstaged edits both count. Without a
//! repository or without a first commit there is no baseline: a file then counts as wholly
//! changed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use hookah_application::error::PortError;
use hookah_application::ports::ChangeSource;
use hookah_domain::session::ChangedLines;

use super::hunk::parse_hunks;

/// Reports changes by shelling out to `git`.
#[derive(Debug, Clone, Copy, Default)]
pub struct GitChanges;

impl GitChanges {
    /// Creates the adapter.
    pub fn new() -> Self {
        Self
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<Output, PortError> {
    Command::new("git")
        .arg("--no-pager")
        .arg("--literal-pathspecs")
        .args(["-c", "core.quotepath=off", "-c", "color.ui=never"])
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| PortError::new(format!("cannot run git: {e}")))
}

/// Runs git and returns stdout, treating a non-zero exit as an error.
fn git_ok(dir: &Path, args: &[&str]) -> Result<Vec<u8>, PortError> {
    let out = git(dir, args)?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        let why = String::from_utf8_lossy(&out.stderr);
        Err(PortError::new(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or(""),
            why.trim()
        )))
    }
}

fn succeeds(dir: &Path, args: &[&str]) -> bool {
    git(dir, args).is_ok_and(|o| o.status.success())
}

fn has_head(dir: &Path) -> bool {
    succeeds(dir, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
}

/// A path as git prints it. Git always writes `/` separators, which Windows accepts too. Unix
/// keeps the raw bytes (a file name need not be UTF-8); Windows has no byte paths, so a name
/// that is not valid UTF-8 is converted lossily.
fn path_from_git(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    return PathBuf::from(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(bytes));
    #[cfg(not(unix))]
    return PathBuf::from(String::from_utf8_lossy(bytes).into_owned());
}

fn nul_paths(bytes: &[u8]) -> impl Iterator<Item = PathBuf> + '_ {
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(path_from_git)
}

impl ChangeSource for GitChanges {
    fn changed_lines(&self, project_root: &Path, file: &Path) -> Result<ChangedLines, PortError> {
        if !succeeds(project_root, &["rev-parse", "--is-inside-work-tree"])
            || !has_head(project_root)
        {
            return Ok(ChangedLines::All);
        }
        let file_arg = file.to_string_lossy();
        let tracked = git_ok(project_root, &["ls-files", "-z", "--", &file_arg])?;
        if tracked.is_empty() {
            return Ok(ChangedLines::All);
        }
        let diff = git_ok(
            project_root,
            &[
                "diff",
                "--no-ext-diff",
                "--no-color",
                "-U0",
                "HEAD",
                "--",
                &file_arg,
            ],
        )?;
        Ok(ChangedLines::Ranges(parse_hunks(&String::from_utf8_lossy(
            &diff,
        ))))
    }

    fn changed_files(&self, root: &Path) -> Result<Vec<PathBuf>, PortError> {
        let top = git_ok(root, &["rev-parse", "--show-toplevel"])?;
        let top = top.strip_suffix(b"\n").unwrap_or(&top);
        let top = path_from_git(top);
        let mut rel: BTreeSet<PathBuf> = BTreeSet::new();
        if has_head(root) {
            let diff = git_ok(
                root,
                &[
                    "diff",
                    "--no-ext-diff",
                    "--name-only",
                    "-z",
                    "HEAD",
                    "--",
                    ".",
                ],
            )?;
            rel.extend(nul_paths(&diff));
        } else {
            let all = git_ok(root, &["ls-files", "-z", "--full-name", "--", "."])?;
            rel.extend(nul_paths(&all));
        }
        let others = git_ok(
            root,
            &[
                "ls-files",
                "-z",
                "--full-name",
                "--others",
                "--exclude-standard",
                "--",
                ".",
            ],
        )?;
        rel.extend(nul_paths(&others));
        Ok(rel
            .into_iter()
            .map(|p| top.join(p))
            .filter(|p| p.exists())
            .collect())
    }
}
