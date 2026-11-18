//! Integration tests of the git change source on real temporary repositories.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use hookah_adapters::vcs::GitChanges;
use hookah_application::ports::ChangeSource;
use hookah_domain::session::{ChangedLines, LineRange};

fn fresh_dir(base: &Path, name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = base.join(format!(
        "vcs-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dunce::canonicalize(dir).unwrap()
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn repo(name: &str) -> PathBuf {
    let dir = fresh_dir(Path::new(env!("CARGO_TARGET_TMPDIR")), name);
    git(&dir, &["init", "-q"]);
    dir
}

fn commit_all(dir: &Path) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "c"]);
}

fn range(a: u32, b: u32) -> LineRange {
    LineRange::new(a, b).unwrap()
}

const TEN: &str = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";

#[test]
fn modified_tracked_file_reports_new_side_ranges() {
    let dir = repo("mod");
    let f = dir.join("a.txt");
    fs::write(&f, TEN).unwrap();
    commit_all(&dir);
    fs::write(&f, "1\nTWO\n3\n4\n5\n6\n7\n8\nNINE\nTEN\nextra\n").unwrap();

    let got = GitChanges::new().changed_lines(&dir, &f).unwrap();
    assert_eq!(got, ChangedLines::Ranges(vec![range(2, 2), range(9, 11)]));
}

#[test]
fn staged_changes_count_and_clean_file_is_empty() {
    let dir = repo("staged");
    let f = dir.join("a.txt");
    fs::write(&f, TEN).unwrap();
    fs::write(dir.join("b.txt"), "x\n").unwrap();
    commit_all(&dir);
    fs::write(&f, TEN.replace("5\n", "five\n")).unwrap();
    git(&dir, &["add", "a.txt"]);

    let src = GitChanges::new();
    assert_eq!(
        src.changed_lines(&dir, &f).unwrap(),
        ChangedLines::Ranges(vec![range(5, 5)])
    );
    assert_eq!(
        src.changed_lines(&dir, &dir.join("b.txt")).unwrap(),
        ChangedLines::Ranges(vec![])
    );
}

#[test]
fn pure_deletion_gives_no_range() {
    let dir = repo("del");
    let f = dir.join("a.txt");
    fs::write(&f, TEN).unwrap();
    commit_all(&dir);
    fs::write(&f, "1\n2\n3\n4\n5\n6\n7\n8\n9\n").unwrap();
    assert_eq!(
        GitChanges::new().changed_lines(&dir, &f).unwrap(),
        ChangedLines::Ranges(vec![])
    );
}

#[test]
fn untracked_file_is_all() {
    let dir = repo("untracked");
    fs::write(dir.join("a.txt"), "a\n").unwrap();
    commit_all(&dir);
    let f = dir.join("new.txt");
    fs::write(&f, "n\n").unwrap();
    assert_eq!(
        GitChanges::new().changed_lines(&dir, &f).unwrap(),
        ChangedLines::All
    );
}

#[test]
fn repo_without_head_is_all() {
    let dir = repo("nohead");
    let f = dir.join("a.txt");
    fs::write(&f, "a\n").unwrap();
    assert_eq!(
        GitChanges::new().changed_lines(&dir, &f).unwrap(),
        ChangedLines::All
    );
    // Staged but never committed: tracked, yet there is no HEAD to diff against.
    git(&dir, &["add", "a.txt"]);
    assert_eq!(
        GitChanges::new().changed_lines(&dir, &f).unwrap(),
        ChangedLines::All
    );
}

#[test]
fn outside_a_repo_is_all_for_lines_and_error_for_files() {
    let dir = fresh_dir(&std::env::temp_dir(), "norepo");
    let f = dir.join("a.txt");
    fs::write(&f, "a\n").unwrap();
    let src = GitChanges::new();
    assert_eq!(src.changed_lines(&dir, &f).unwrap(), ChangedLines::All);
    assert!(src.changed_files(&dir).is_err());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn spaces_and_non_ascii_paths() {
    let dir = repo("unicode");
    let f = dir.join("dir with space/файл é.txt");
    fs::create_dir_all(f.parent().unwrap()).unwrap();
    fs::write(&f, "a\nb\n").unwrap();
    commit_all(&dir);
    fs::write(&f, "a\nB\n").unwrap();
    let new = dir.join("dir with space/новый.txt");
    fs::write(&new, "n\n").unwrap();

    let src = GitChanges::new();
    assert_eq!(
        src.changed_lines(&dir, &f).unwrap(),
        ChangedLines::Ranges(vec![range(2, 2)])
    );
    assert_eq!(src.changed_lines(&dir, &new).unwrap(), ChangedLines::All);
    let mut files = src.changed_files(&dir).unwrap();
    files.sort();
    let mut want = vec![f, new];
    want.sort();
    assert_eq!(files, want);
}

#[test]
fn changed_files_lists_modified_staged_untracked_and_drops_deleted() {
    let dir = repo("files");
    for n in ["keep", "edit", "stage", "gone"] {
        fs::write(dir.join(n), "x\n").unwrap();
    }
    fs::create_dir(dir.join("sub")).unwrap();
    fs::write(dir.join("sub/inner"), "x\n").unwrap();
    commit_all(&dir);
    fs::write(dir.join("edit"), "y\n").unwrap();
    fs::write(dir.join("stage"), "y\n").unwrap();
    git(&dir, &["add", "stage"]);
    fs::remove_file(dir.join("gone")).unwrap();
    fs::write(dir.join("fresh"), "z\n").unwrap();
    fs::write(dir.join("sub/inner"), "y\n").unwrap();

    let src = GitChanges::new();
    let mut all = src.changed_files(&dir).unwrap();
    all.sort();
    assert_eq!(
        all,
        vec![
            dir.join("edit"),
            dir.join("fresh"),
            dir.join("stage"),
            dir.join("sub/inner")
        ]
    );
    // Scoped to a subdirectory.
    assert_eq!(
        src.changed_files(&dir.join("sub")).unwrap(),
        vec![dir.join("sub/inner")]
    );
}

#[test]
fn changed_files_without_head_lists_tracked_and_untracked() {
    let dir = repo("files-nohead");
    fs::write(dir.join("a"), "x\n").unwrap();
    fs::write(dir.join("b"), "x\n").unwrap();
    git(&dir, &["add", "a"]);
    let mut got = GitChanges::new().changed_files(&dir).unwrap();
    got.sort();
    assert_eq!(got, vec![dir.join("a"), dir.join("b")]);
}

#[test]
fn line_count_counts_the_lines_of_a_file() {
    let dir = repo("line-count");
    let vcs = GitChanges::new();
    let count = |name: &str, text: &str| {
        let file = dir.join(name);
        fs::write(&file, text).unwrap();
        vcs.line_count(&file).unwrap()
    };
    assert_eq!(count("empty.txt", ""), 0);
    assert_eq!(count("ten.txt", TEN), 10);
    assert_eq!(count("open.txt", "a\nb"), 2);
    assert_eq!(count("blank.txt", "\n"), 1);
    assert!(vcs.line_count(&dir.join("missing.txt")).is_err());
}
