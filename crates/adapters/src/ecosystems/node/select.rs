//! Node file roles and test and mutation selection.
//!
//! - [`classify`]: a file is a test when its name has a `.test.` or `.spec.` segment
//!   (`foo.test.ts`, `foo.spec.tsx`) or when a directory between the project root and the file is
//!   called `__tests__`, `test`, or `tests`. Everything else is source.
//! - [`related_tests`]: the changed JavaScript and TypeScript files under the project, as project-relative
//!   paths. The runner's own import graph (`vitest related`, `jest --findRelatedTests`) finds the
//!   tests that import them, so no test file is guessed here. No runner or no such file selects
//!   nothing.
//! - [`mutants`]: the sources to mutate. A changed test maps to its source by naming convention
//!   (see [`sources_of_test`]); changed sources count directly. Only files that exist survive.
//!   Stryker takes one comma-separated `--mutate` list, so the selection is a single target.

use std::path::{Path, PathBuf};

use hookah_domain::project::{Ecosystem, Project};
use hookah_domain::session::FileRole;
use hookah_domain::toolchain::{MutationSelection, TestSelection};

/// Extensions a source file can have, in the order candidates are tried.
const SOURCE_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs"];
/// Directory names that mark test files.
const TEST_DIRS: &[&str] = &["__tests__", "test", "tests"];
/// The most files one mutation run takes; Stryker is slow, so the selection stays narrow.
const MAX_MUTANT_FILES: usize = 10;

/// The role of `file`, by file name and by the directories below the project root.
pub fn classify(project: &Project, file: &Path) -> FileRole {
    let Ok(rel) = project.relative(file) else {
        return FileRole::Source;
    };
    if file_name_marks_test(&rel) || test_dir_index(&rel).is_some() {
        FileRole::Test
    } else {
        FileRole::Source
    }
}

/// `foo.test.ts` and `foo.spec.js`: a `test` or `spec` segment between the stem and the extension.
fn file_name_marks_test(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let parts: Vec<&str> = name.split('.').collect();
    parts.len() > 2
        && parts[1..parts.len() - 1]
            .iter()
            .any(|p| matches!(*p, "test" | "spec"))
}

/// The index, among the directory components of `rel`, of the innermost test directory.
fn test_dir_index(rel: &Path) -> Option<usize> {
    let dirs: Vec<_> = rel.parent()?.components().collect();
    dirs.iter().rposition(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|c| TEST_DIRS.contains(&c))
    })
}

/// The changed files the runner should relate to tests, relative to the project root.
pub fn related_tests(project: &Project, changed: &[PathBuf], has_runner: bool) -> TestSelection {
    if !has_runner {
        return TestSelection::None;
    }
    let targets = changed
        .iter()
        .filter(|f| Ecosystem::Node.can_check(f))
        .filter_map(|f| project.relative(f).ok())
        .filter(|rel| !in_node_modules(rel))
        .map(|rel| rel.to_string_lossy().into_owned())
        .collect();
    TestSelection::from_targets(targets)
}

fn in_node_modules(rel: &Path) -> bool {
    rel.components().any(|c| c.as_os_str() == "node_modules")
}

/// The existing sources a test file most likely covers, relative to the project root. For
/// `foo.test.ts` (or `foo.spec.ts`, or a plain `foo.ts` inside a test directory) the stem is
/// `foo`. The candidate directories are:
///
/// - always the test's own directory (`src/foo.test.ts` covers `src/foo.ts`);
/// - for a test directory `D/__tests__/rel/` the parent `D/rel/`;
/// - for `D/test/rel/` or `D/tests/rel/` the sibling `D/src/rel/` and `D/lib/rel/`, then `D/rel/`.
///
/// Each directory tries every extension in [`SOURCE_EXTENSIONS`]. The innermost test directory
/// decides. A file counts only when it exists on disk.
pub fn sources_of_test(project: &Project, test: &Path) -> Vec<PathBuf> {
    let Ok(rel) = project.relative(test) else {
        return vec![];
    };
    let Some(name) = rel.file_name().and_then(|n| n.to_str()) else {
        return vec![];
    };
    let mut stem = name.rsplit_once('.').map_or(name, |(s, _)| s).to_owned();
    for marker in [".test", ".spec"] {
        if let Some(s) = stem.strip_suffix(marker) {
            stem = s.to_owned();
            break;
        }
    }

    let dir = rel.parent().unwrap_or(Path::new(""));
    let mut dirs = vec![dir.to_path_buf()];
    if let Some(i) = test_dir_index(&rel) {
        let comps: Vec<_> = dir.components().collect();
        let outer: PathBuf = comps[..i].iter().collect();
        let inner: PathBuf = comps[i + 1..].iter().collect();
        if comps[i].as_os_str() == "__tests__" {
            dirs.push(outer.join(&inner));
        } else {
            dirs.extend(["src", "lib", ""].map(|top| outer.join(top).join(&inner)));
        }
    }

    let mut found = Vec::new();
    for d in dirs {
        for ext in SOURCE_EXTENSIONS {
            let candidate = d.join(format!("{stem}.{ext}"));
            let abs = project.root.join(&candidate);
            if abs != test && !found.contains(&candidate) && abs.is_file() {
                found.push(candidate);
            }
        }
    }
    found
}

/// Whether `rel` can go into a comma-separated Stryker `--mutate` list unchanged: no commas and
/// no glob or negation characters, which Stryker would read as patterns.
fn mutate_safe(rel: &Path) -> bool {
    let s = rel.to_string_lossy();
    !s.is_empty()
        && !s.contains(|c: char| {
            matches!(c, ',' | '*' | '?' | '[' | ']' | '{' | '}' | '(' | ')' | '!')
        })
        && !s.ends_with(".d.ts")
        && !in_node_modules(rel)
}

/// The sources to mutate for the changed tests and sources.
pub fn mutants(
    project: &Project,
    changed_tests: &[PathBuf],
    changed_sources: &[PathBuf],
) -> MutationSelection {
    let mut files: Vec<PathBuf> = Vec::new();
    let mapped = changed_tests
        .iter()
        .flat_map(|t| sources_of_test(project, t));
    let direct = changed_sources
        .iter()
        .filter(|f| Ecosystem::Node.can_check(f) && f.is_file())
        .filter_map(|f| project.relative(f).ok());
    for rel in mapped.chain(direct) {
        if mutate_safe(&rel) && !files.contains(&rel) {
            files.push(rel);
        }
    }
    files.truncate(MAX_MUTANT_FILES);
    let list: Vec<String> = files
        .iter()
        .map(|f| f.to_string_lossy().into_owned())
        .collect();
    MutationSelection::from_targets(if list.is_empty() {
        vec![]
    } else {
        vec![list.join(",")]
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::detect::package::scratch::Scratch;
    use super::*;

    fn project(s: &Scratch) -> Project {
        Project {
            root: s.0.clone(),
            ecosystem: Ecosystem::Node,
            manifests: vec![],
        }
    }

    fn role(s: &Scratch, rel: &str) -> FileRole {
        classify(&project(s), &s.path(rel))
    }

    #[test]
    fn classifies_tests_by_name_and_directory() {
        let s = Scratch::new();
        for test in [
            "src/foo.test.ts",
            "src/foo.spec.tsx",
            "src/foo.component.test.js",
            "src/__tests__/foo.ts",
            "test/foo.ts",
            "tests/unit/foo.js",
            "src/a/test/b.mjs",
        ] {
            assert_eq!(role(&s, test), FileRole::Test, "{test}");
        }
        for source in [
            "src/foo.ts",
            "src/test.ts",
            "src/spec.ts",
            "src/testing/foo.ts",
            "src/contest/foo.ts",
            "src/latest.ts",
            "foo.tsx",
        ] {
            assert_eq!(role(&s, source), FileRole::Source, "{source}");
        }
    }

    #[test]
    fn directories_above_the_project_root_do_not_matter() {
        let s = Scratch::new();
        let p = Project {
            root: s.path("tests/app"),
            ecosystem: Ecosystem::Node,
            manifests: vec![],
        };
        assert_eq!(
            classify(&p, &s.path("tests/app/src/a.ts")),
            FileRole::Source
        );
        assert_eq!(
            classify(&p, Path::new("/elsewhere/test/a.ts")),
            FileRole::Source
        );
    }

    #[test]
    fn test_selection_hands_changed_files_to_the_runner() {
        let s = Scratch::new();
        let changed = [
            s.path("src/a.ts"),
            s.path("src/b.test.tsx"),
            s.path("README.md"),
            s.path("node_modules/x/index.js"),
            PathBuf::from("/elsewhere/c.ts"),
        ];
        assert_eq!(
            related_tests(&project(&s), &changed, true),
            TestSelection::Targets(vec!["src/a.ts".into(), "src/b.test.tsx".into()])
        );
        assert_eq!(
            related_tests(&project(&s), &changed, false),
            TestSelection::None
        );
        assert_eq!(
            related_tests(&project(&s), &[s.path("README.md")], true),
            TestSelection::None
        );
    }

    fn mutate(s: &Scratch, tests: &[&str], sources: &[&str]) -> MutationSelection {
        let abs = |v: &[&str]| v.iter().map(|p| s.path(p)).collect::<Vec<_>>();
        mutants(&project(s), &abs(tests), &abs(sources))
    }

    fn files(s: &Scratch, paths: &[&str]) {
        for p in paths {
            s.write(p, "");
        }
    }

    #[test]
    fn maps_tests_next_to_their_sources_across_extensions() {
        let s = Scratch::new();
        files(
            &s,
            &[
                "src/foo.ts",
                "src/bar.tsx",
                "src/baz.js",
                "src/qux.component.ts",
            ],
        );
        assert_eq!(
            mutate(
                &s,
                &[
                    "src/foo.test.ts",
                    "src/bar.spec.ts",
                    "src/baz.test.mjs",
                    "src/qux.component.spec.ts",
                    "src/missing.test.ts"
                ],
                &[]
            ),
            MutationSelection::Targets(vec![
                "src/foo.ts,src/bar.tsx,src/baz.js,src/qux.component.ts".into()
            ])
        );
    }

    #[test]
    fn maps_dunder_tests_to_the_parent_directory() {
        let s = Scratch::new();
        files(&s, &["src/util/foo.ts", "src/util/deep/bar.js"]);
        assert_eq!(
            mutate(
                &s,
                &[
                    "src/util/__tests__/foo.ts",
                    "src/util/__tests__/deep/bar.test.js"
                ],
                &[]
            ),
            MutationSelection::Targets(vec!["src/util/foo.ts,src/util/deep/bar.js".into()])
        );
    }

    #[test]
    fn maps_test_directories_to_src_lib_or_the_project_root() {
        let s = Scratch::new();
        files(
            &s,
            &["src/foo.ts", "lib/baz.js", "bar.ts", "src/unit/deep.ts"],
        );
        assert_eq!(
            mutate(
                &s,
                &[
                    "test/foo.spec.ts",
                    "tests/baz.test.js",
                    "test/bar.test.ts",
                    "test/unit/deep.spec.ts"
                ],
                &[]
            ),
            MutationSelection::Targets(vec![
                "src/foo.ts,lib/baz.js,bar.ts,src/unit/deep.ts".into()
            ])
        );
    }

    #[test]
    fn changed_sources_count_and_duplicates_collapse() {
        let s = Scratch::new();
        files(&s, &["src/foo.ts", "src/other.ts", "src/gone.d.ts"]);
        assert_eq!(
            mutate(
                &s,
                &["src/foo.test.ts"],
                &[
                    "src/foo.ts",
                    "src/other.ts",
                    "src/vanished.ts",
                    "src/gone.d.ts",
                    "README.md"
                ]
            ),
            MutationSelection::Targets(vec!["src/foo.ts,src/other.ts".into()])
        );
    }

    #[test]
    fn nothing_to_mutate_is_none() {
        let s = Scratch::new();
        assert_eq!(
            mutate(&s, &["src/foo.test.ts"], &[]),
            MutationSelection::None
        );
        assert_eq!(mutate(&s, &[], &[]), MutationSelection::None);
    }

    #[test]
    fn unsafe_names_and_long_lists_are_dropped_or_capped() {
        let s = Scratch::new();
        files(&s, &["src/[id].tsx", "src/a,b.ts", "src/(g)/x.ts"]);
        assert_eq!(
            mutate(&s, &[], &["src/[id].tsx", "src/a,b.ts", "src/(g)/x.ts"]),
            MutationSelection::None
        );
        let names: Vec<String> = (0..15).map(|i| format!("src/f{i}.ts")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        files(&s, &refs);
        let MutationSelection::Targets(t) = mutate(&s, &[], &refs) else {
            panic!("expected targets");
        };
        assert_eq!(t[0].split(',').count(), MAX_MUTANT_FILES);
    }
}
