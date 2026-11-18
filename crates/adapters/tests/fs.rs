//! Integration tests of the filesystem locator and the content hasher on real temp dirs.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use hookah_adapters::fs::{FileHasher, FsLocator};
use hookah_application::ports::{ContentHasher, ProjectLocator};

fn temp_dir(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "fs-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(dir).unwrap()
}

fn touch(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "").unwrap();
}

fn paths(found: Vec<hookah_domain::project::Manifest>) -> Vec<PathBuf> {
    found.into_iter().map(|m| m.path).collect()
}

#[test]
fn nested_roots_finds_manifest_directories_shallowest_first_within_the_depth() {
    let root = temp_dir("nested");
    touch(&root.join("Cargo.toml")); // the root itself is not nested
    touch(&root.join("b/package.json"));
    touch(&root.join("a/Cargo.toml"));
    touch(&root.join("a/deep/inner/Cargo.toml"));
    touch(&root.join("a/deep/inner/more/pyproject.toml")); // 4 levels down
    touch(&root.join("c/src/readme.md"));
    touch(&root.join("c/src/pkg/pyproject.toml")); // 3 levels down
    for skipped in ["node_modules", "target", ".hidden", "venv", "vendor"] {
        touch(&root.join(skipped).join("Cargo.toml"));
    }

    let loc = FsLocator::with_home(None);
    let found = loc.nested_roots(&root, 3).unwrap();
    assert_eq!(
        found,
        vec![
            root.join("a"),
            root.join("b"),
            root.join("a/deep/inner"),
            root.join("c/src/pkg"),
        ]
    );
    assert_eq!(
        loc.nested_roots(&root, 1).unwrap(),
        vec![root.join("a"), root.join("b")]
    );
    assert!(loc.nested_roots(&root, 0).unwrap().is_empty());
    assert!(
        loc.nested_roots(&root.join("missing"), 3)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn collects_manifests_up_to_the_git_root_inclusive() {
    let root = temp_dir("git");
    fs::create_dir_all(root.join("outer/.git")).unwrap();
    touch(&root.join("outer/package.json"));
    touch(&root.join("outer/pkg/pyproject.toml"));
    touch(&root.join("outer/pkg/src/Cargo.toml"));
    touch(&root.join("outer/pkg/src/lib.rs"));
    touch(&root.join("Cargo.toml")); // above the boundary

    let loc = FsLocator::with_home(None);
    let found = paths(
        loc.manifests_above(&root.join("outer/pkg/src/lib.rs"))
            .unwrap(),
    );
    assert_eq!(
        found,
        vec![
            root.join("outer/pkg/src/Cargo.toml"),
            root.join("outer/pkg/pyproject.toml"),
            root.join("outer/package.json"),
        ]
    );
}

#[test]
fn stops_before_home() {
    let home = temp_dir("home");
    touch(&home.join("package.json"));
    touch(&home.join("proj/Cargo.toml"));
    touch(&home.join("proj/a.rs"));

    let loc = FsLocator::with_home(Some(home.clone()));
    let found = paths(loc.manifests_above(&home.join("proj/a.rs")).unwrap());
    assert_eq!(found, vec![home.join("proj/Cargo.toml")]);
}

#[test]
fn deleted_file_walks_from_nearest_existing_ancestor() {
    let home = temp_dir("gone");
    touch(&home.join("proj/Cargo.toml"));

    let loc = FsLocator::with_home(Some(home.clone()));
    let found = paths(
        loc.manifests_above(&home.join("proj/removed/dir/x.rs"))
            .unwrap(),
    );
    assert_eq!(found, vec![home.join("proj/Cargo.toml")]);
}

#[test]
fn directory_named_like_a_manifest_is_ignored() {
    let home = temp_dir("dirname");
    fs::create_dir_all(home.join("proj/package.json")).unwrap();
    touch(&home.join("proj/a.ts"));

    let loc = FsLocator::with_home(Some(home.clone()));
    assert!(
        loc.manifests_above(&home.join("proj/a.ts"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn hash_is_stable_and_tracks_content() {
    let dir = temp_dir("hash");
    let file = dir.join("a.txt");
    fs::write(&file, "one").unwrap();
    let h = FileHasher.content_hash(&file).unwrap();
    assert_eq!(h, FileHasher.content_hash(&file).unwrap());
    fs::write(&file, "two").unwrap();
    assert_ne!(h, FileHasher.content_hash(&file).unwrap());
    fs::write(&file, "one").unwrap();
    assert_eq!(h, FileHasher.content_hash(&file).unwrap());
}

#[test]
fn hash_of_missing_file_is_an_error() {
    let dir = temp_dir("hash-missing");
    assert!(FileHasher.content_hash(&dir.join("nope")).is_err());
}
