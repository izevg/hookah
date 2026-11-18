//! Mutation targets for changed tests.
//!
//! The targets are source files, each as one `--file=<path>` string, so `Arg::Targets` expands to
//! one valid `cargo mutants` flag per entry. `cargo-mutants` resolves `--file` against the
//! workspace root, so paths are workspace-relative (`crates/core/src/a.rs`).
//!
//! - A changed test under `src/` (an inline test module) selects its own file.
//! - A changed `tests/foo.rs` (or `tests/foo/...`) selects `src/foo.rs` or `src/foo/mod.rs` when
//!   present. If it has no such twin, it selects the changed sources of the package, and if there
//!   are none, `src/lib.rs` and `src/main.rs` (whichever exist).
//! - A changed heavy test (`tests/integration_*.rs`, `tests/e2e/...`) selects nothing.
//! - Changed sources under `src/` are always included.
//! - Changed files under `benches/` and `examples/` select nothing; non-Rust files and
//!   `build.rs` are ignored; files that do not exist are dropped.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use hookah_domain::light::TestWeight;

/// The `cargo mutants` target string for one workspace-relative file. `--file` takes a glob, so
/// the path is written with `/` on every OS: a `\` would escape the next character.
pub fn file_target(path: &Path) -> String {
    let parts: Vec<_> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect();
    format!("--file={}", parts.join("/"))
}

/// Chooses `--file=` targets. `crate_dir` is the project root relative to the workspace root;
/// `exists` tells whether a project-relative path is a file.
pub fn select(
    crate_dir: &Path,
    project_root: &Path,
    changed_tests: &[PathBuf],
    changed_sources: &[PathBuf],
    exists: &dyn Fn(&Path) -> bool,
) -> Vec<String> {
    let rel = |p: &PathBuf| p.strip_prefix(project_root).ok().map(Path::to_path_buf);
    let is_src = |p: &Path| p.starts_with("src") && p.extension().is_some_and(|e| e == "rs");

    let sources: BTreeSet<PathBuf> = changed_sources
        .iter()
        .filter_map(rel)
        .filter(|p| is_src(p))
        .collect();
    let mut chosen = sources.clone();
    let mut unmatched = false;
    for test in changed_tests
        .iter()
        .filter_map(rel)
        .filter(|t| t.extension().is_some_and(|e| e == "rs"))
    {
        if is_src(&test) {
            chosen.insert(test);
        } else if test.starts_with("tests") && !TestWeight::of_path(&test).is_heavy() {
            match twin(&test, exists) {
                Some(src) => {
                    chosen.insert(src);
                }
                None => unmatched = true,
            }
        }
    }
    if unmatched && sources.is_empty() {
        chosen.extend(
            ["src/lib.rs", "src/main.rs"]
                .map(PathBuf::from)
                .into_iter()
                .filter(|p| exists(p)),
        );
    }
    chosen
        .into_iter()
        .filter(|p| exists(p))
        .map(|p| file_target(&crate_dir.join(p)))
        .collect()
}

/// The source file a `tests/<name>...` file is named after.
fn twin(test: &Path, exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    let second = test.components().nth(1)?;
    let name = Path::new(second.as_os_str()).file_stem()?;
    [
        Path::new("src").join(name).with_extension("rs"),
        Path::new("src").join(name).join("mod.rs"),
    ]
    .into_iter()
    .find(|p| exists(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/ws/crates/core";

    fn run(tests: &[&str], sources: &[&str], present: &[&str]) -> Vec<String> {
        let p = |s: &&str| PathBuf::from(ROOT).join(s);
        let tests: Vec<_> = tests.iter().map(p).collect();
        let sources: Vec<_> = sources.iter().map(p).collect();
        let exists = |rel: &Path| present.iter().any(|f| Path::new(f) == rel);
        select(
            Path::new("crates/core"),
            Path::new(ROOT),
            &tests,
            &sources,
            &exists,
        )
    }

    #[test]
    fn inline_test_selects_its_own_file() {
        assert_eq!(
            run(&["src/a.rs"], &[], &["src/a.rs"]),
            ["--file=crates/core/src/a.rs"]
        );
    }

    #[test]
    fn integration_test_selects_its_twin_and_changed_sources() {
        let present = ["src/foo.rs", "src/bar/mod.rs", "src/lib.rs", "src/baz.rs"];
        assert_eq!(
            run(&["tests/foo.rs"], &[], &present),
            ["--file=crates/core/src/foo.rs"]
        );
        assert_eq!(
            run(&["tests/bar/cases.rs"], &["src/baz.rs"], &present),
            [
                "--file=crates/core/src/bar/mod.rs",
                "--file=crates/core/src/baz.rs"
            ]
        );
    }

    #[test]
    fn integration_test_without_twin_falls_back() {
        let present = ["src/lib.rs", "src/main.rs", "src/x.rs"];
        assert_eq!(
            run(&["tests/api.rs"], &["src/x.rs"], &present),
            ["--file=crates/core/src/x.rs"]
        );
        assert_eq!(
            run(&["tests/api.rs"], &[], &present),
            [
                "--file=crates/core/src/lib.rs",
                "--file=crates/core/src/main.rs"
            ]
        );
        assert!(run(&["tests/api.rs"], &[], &[]).is_empty());
    }

    #[test]
    fn heavy_integration_tests_select_nothing() {
        let present = ["src/foo.rs", "src/lib.rs", "src/integration_flow.rs"];
        for test in [
            "tests/integration_flow.rs",
            "tests/foo_e2e.rs",
            "tests/e2e/main.rs",
            "tests/integration/foo.rs",
        ] {
            assert!(run(&[test], &[], &present).is_empty(), "{test}");
        }
        assert_eq!(
            run(&["tests/integration_flow.rs"], &["src/foo.rs"], &present),
            ["--file=crates/core/src/foo.rs"]
        );
    }

    #[test]
    fn benches_examples_and_foreign_files_select_nothing() {
        let present = ["src/lib.rs", "build.rs"];
        assert!(run(&["benches/b.rs", "examples/e.rs"], &[], &present).is_empty());
        assert!(run(&["tests/t.txt"], &["build.rs", "README.md"], &present).is_empty());
        assert!(
            run(&["benches/b.rs"], &["build.rs", "README.md"], &present).is_empty(),
            "only src/**/*.rs sources count"
        );
    }

    #[test]
    fn paths_outside_the_project_are_ignored() {
        let exists = |_: &Path| true;
        let got = select(
            Path::new("crates/core"),
            Path::new(ROOT),
            &["/elsewhere/src/a.rs".into()],
            &[],
            &exists,
        );
        assert!(got.is_empty());
    }
}
