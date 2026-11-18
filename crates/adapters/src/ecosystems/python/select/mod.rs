//! Python file roles and test selection.
//!
//! **Roles.** A file is a test (`FileRole::Test`) when its name is `test_*.py`, `*_test.py`, or
//! `conftest.py`, or when any directory between the project root and the file is named `tests`
//! or `test`. Everything else is source. Only the path below the project root counts, so a
//! project that lives in `/home/me/test/app` is not all tests.
//!
//! **Selection.** Tests run as files, relative to the project root:
//!
//! - With `pytest-testmon`, the targets are the changed Python files; the pytest command takes no
//!   file arguments, and testmon itself picks the affected tests (its first run executes the whole
//!   suite to build `.testmondata`).
//! - Without it: a changed test file selects itself. A changed source file `pkg/foo.py` selects
//!   every `test_foo.py` and `foo_test.py` in the project (`pkg/__init__.py` selects
//!   `test_pkg.py`). A changed `conftest.py` or a helper inside a test directory selects the test
//!   files in its own directory.
//!
//! Heavy tests (see [`TestWeight`]: `integration/`, `e2e/`, `*_integration.py`, ...) are dropped
//! from every selection, and under testmon `heavy_ignores` makes pytest skip them itself.
//!
//! Without a match nothing is selected: the whole suite is never run instead. Hidden directories,
//! virtual environments, `build/`, `dist/`, and `mutants/` are not searched.

pub mod mutants;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use hookah_domain::light::TestWeight;
use hookah_domain::project::Project;
use hookah_domain::session::FileRole;
use hookah_domain::toolchain::TestSelection;

use super::layout;

/// Whether a file name is a pytest test module name: `test_*.py` or `*_test.py`.
pub fn is_test_module(name: &str) -> bool {
    name.ends_with(".py")
        && name != "test_.py"
        && (name.starts_with("test_") || name.ends_with("_test.py"))
}

/// Whether the project-relative path is test code.
pub fn is_test_path(rel: &Path) -> bool {
    let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if is_test_module(name) || name == "conftest.py" {
        return true;
    }
    rel.parent().is_some_and(|dir| {
        dir.components()
            .any(|c| matches!(c.as_os_str().to_str(), Some("tests" | "test")))
    })
}

/// The role of `file` (absolute) in `project`.
pub fn classify(project: &Project, file: &Path) -> FileRole {
    match project.relative(file) {
        Ok(rel) if is_test_path(&rel) => FileRole::Test,
        _ => FileRole::Source,
    }
}

/// The existing project-relative `.py` files among the absolute `paths`.
pub fn existing_python(project: &Project, paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut rels: Vec<PathBuf> = paths
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "py"))
        .filter_map(|p| project.relative(p).ok())
        .filter(|rel| project.root.join(rel).is_file())
        .collect();
    rels.sort();
    rels.dedup();
    rels
}

/// The test names a source file `foo.py` is tested by: `test_foo.py` and `foo_test.py`.
fn test_names(rel: &Path) -> [String; 2] {
    let stem = rel.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let stem = if stem == "__init__" {
        rel.parent()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
            .unwrap_or("")
    } else {
        stem
    };
    [format!("test_{stem}.py"), format!("{stem}_test.py")]
}

/// Chooses the tests for the changed files (absolute paths). `testmon` says pytest runs with
/// `--testmon`.
pub fn tests(project: &Project, testmon: bool, changed: &[PathBuf]) -> TestSelection {
    let mut rels = existing_python(project, changed);
    rels.retain(|rel| !TestWeight::of_path(rel).is_heavy());
    if testmon {
        return TestSelection::from_targets(path_strings(rels));
    }
    let all = layout::python_files(&project.root);
    let mut picked: BTreeSet<PathBuf> = BTreeSet::new();
    for rel in &rels {
        let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if is_test_module(name) {
            picked.insert(rel.clone());
        } else if is_test_path(rel) {
            // conftest.py or a helper: the tests next to it.
            let dir = rel.parent();
            picked.extend(
                all.iter()
                    .filter(|f| {
                        f.parent() == dir
                            && f.file_name()
                                .is_some_and(|n| is_test_module(&n.to_string_lossy()))
                    })
                    .cloned(),
            );
        } else {
            let wanted = test_names(rel);
            picked.extend(
                all.iter()
                    .filter(|f| {
                        f.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| wanted.iter().any(|w| w == n))
                    })
                    .cloned(),
            );
        }
    }
    picked.retain(|rel| !TestWeight::of_path(rel).is_heavy());
    TestSelection::from_targets(path_strings(picked))
}

/// The pytest flags that skip heavy test paths below `root`, for runs where pytest picks the
/// tests itself (testmon). `--ignore-glob` is an `fnmatch` against the file's absolute path, so
/// the patterns start with the escaped project root: a project that lives under a directory
/// called `e2e` is not ignored whole. Matching is by directory (`<root>*/e2e/*`) and by file
/// stem word (`*_integration.py`, `integration_*.py`).
pub fn heavy_ignores(root: &Path) -> Vec<String> {
    let base: String = root
        .to_string_lossy()
        .chars()
        .map(|c| match c {
            '*' | '?' | '[' => format!("[{c}]"),
            c => c.to_string(),
        })
        .collect();
    let mut globs: Vec<String> = TestWeight::DIRS
        .iter()
        .map(|d| format!("{base}*/{d}/*"))
        .collect();
    for w in TestWeight::WORDS {
        globs.push(format!("{base}*/{w}.py"));
        globs.push(format!("{base}*/{w}[_-]*.py"));
        globs.push(format!("{base}*[_-]{w}.py"));
        globs.push(format!("{base}*[_-]{w}[_-]*.py"));
    }
    globs
        .into_iter()
        .map(|g| format!("--ignore-glob={g}"))
        .collect()
}

fn path_strings(paths: impl IntoIterator<Item = PathBuf>) -> Vec<String> {
    paths
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use hookah_domain::project::Ecosystem;

    use crate::ecosystems::python::fixture::Scratch;

    use super::*;

    pub(super) fn project(s: &Scratch) -> Project {
        Project {
            root: s.root().to_path_buf(),
            ecosystem: Ecosystem::Python,
            manifests: vec![],
        }
    }

    fn abs(s: &Scratch, rels: &[&str]) -> Vec<PathBuf> {
        rels.iter().map(|r| s.root().join(r)).collect()
    }

    #[test]
    fn roles() {
        let s = Scratch::new("roles");
        let p = project(&s);
        let role = |rel: &str| classify(&p, &s.root().join(rel));
        for test in [
            "tests/helpers.py",
            "test/x.py",
            "pkg/tests/sub/data.py",
            "test_a.py",
            "pkg/a_test.py",
            "conftest.py",
            "tests/conftest.py",
        ] {
            assert_eq!(role(test), FileRole::Test, "{test}");
        }
        for source in [
            "src/pkg/a.py",
            "pkg/testing.py",
            "pkg/contest.py",
            "latest.py",
            "test_.py",
        ] {
            assert_eq!(role(source), FileRole::Source, "{source}");
        }
        // Only the path below the root counts.
        let inside_test_dir = Project {
            root: "/home/me/test/app".into(),
            ..p
        };
        assert_eq!(
            classify(&inside_test_dir, Path::new("/home/me/test/app/pkg/a.py")),
            FileRole::Source
        );
        assert_eq!(
            classify(&inside_test_dir, Path::new("/elsewhere/a.py")),
            FileRole::Source
        );
    }

    fn layout(s: &Scratch) {
        for f in [
            "src/shop/cart.py",
            "src/shop/__init__.py",
            "src/shop/pricing/rules.py",
            "tests/test_cart.py",
            "tests/unit/test_rules.py",
            "tests/unit/conftest.py",
            "tests/unit/test_other.py",
            "tests/helpers.py",
            "src/shop/pricing/rules_test.py",
            "tests/test_shop.py",
            "README.md",
        ] {
            s.write(f, "");
        }
    }

    fn select(s: &Scratch, testmon: bool, changed: &[&str]) -> TestSelection {
        tests(&project(s), testmon, &abs(s, changed))
    }

    /// The selected targets, written with `/` whatever the platform separator is.
    fn targets(sel: TestSelection) -> Vec<String> {
        match sel {
            TestSelection::Targets(t) => t.iter().map(|p| p.replace('\\', "/")).collect(),
            TestSelection::None => vec![],
        }
    }

    #[test]
    fn a_changed_test_selects_itself() {
        let s = Scratch::new("sel-self");
        layout(&s);
        assert_eq!(
            targets(select(&s, false, &["tests/test_cart.py"])),
            ["tests/test_cart.py"]
        );
    }

    #[test]
    fn a_changed_source_selects_tests_by_name_anywhere() {
        let s = Scratch::new("sel-source");
        layout(&s);
        assert_eq!(
            targets(select(&s, false, &["src/shop/cart.py"])),
            ["tests/test_cart.py"]
        );
        assert_eq!(
            targets(select(&s, false, &["src/shop/pricing/rules.py"])),
            ["src/shop/pricing/rules_test.py", "tests/unit/test_rules.py"]
        );
        // A package's __init__.py maps to test_<package>.py.
        assert_eq!(
            targets(select(&s, false, &["src/shop/__init__.py"])),
            ["tests/test_shop.py"]
        );
    }

    #[test]
    fn conftest_and_helpers_select_their_sibling_tests() {
        let s = Scratch::new("sel-conftest");
        layout(&s);
        assert_eq!(
            targets(select(&s, false, &["tests/unit/conftest.py"])),
            ["tests/unit/test_other.py", "tests/unit/test_rules.py"]
        );
        assert_eq!(
            targets(select(&s, false, &["tests/helpers.py"])),
            ["tests/test_cart.py", "tests/test_shop.py"]
        );
    }

    #[test]
    fn no_match_selects_nothing_never_the_suite() {
        let s = Scratch::new("sel-none");
        layout(&s);
        s.write("src/shop/orphan.py", "");
        assert_eq!(
            select(&s, false, &["src/shop/orphan.py"]),
            TestSelection::None
        );
        assert_eq!(select(&s, false, &["README.md"]), TestSelection::None);
        assert_eq!(select(&s, false, &[]), TestSelection::None);
    }

    #[test]
    fn deleted_and_foreign_files_are_ignored() {
        let s = Scratch::new("sel-gone");
        layout(&s);
        assert_eq!(
            select(&s, false, &["tests/test_gone.py"]),
            TestSelection::None
        );
        let outside = tests(
            &project(&s),
            false,
            &[PathBuf::from("/elsewhere/test_a.py")],
        );
        assert_eq!(outside, TestSelection::None);
    }

    #[test]
    fn junk_directories_are_not_searched() {
        let s = Scratch::new("sel-junk");
        s.write("src/shop/cart.py", "");
        s.write(".venv/lib/test_cart.py", "");
        s.write("mutants/tests/test_cart.py", "");
        assert_eq!(
            select(&s, false, &["src/shop/cart.py"]),
            TestSelection::None
        );
    }

    #[test]
    fn testmon_targets_are_the_changed_python_files() {
        let s = Scratch::new("sel-testmon");
        layout(&s);
        assert_eq!(
            targets(select(
                &s,
                true,
                &[
                    "src/shop/orphan_missing.py",
                    "src/shop/cart.py",
                    "README.md"
                ]
            )),
            ["src/shop/cart.py"]
        );
        assert_eq!(select(&s, true, &["README.md"]), TestSelection::None);
    }

    #[test]
    fn heavy_tests_are_never_selected() {
        let s = Scratch::new("sel-heavy");
        layout(&s);
        for f in [
            "tests/integration/test_cart.py",
            "tests/e2e/test_cart.py",
            "tests/test_cart_integration.py",
            "tests/integration/conftest.py",
            "tests/integration/test_other.py",
        ] {
            s.write(f, "");
        }
        // A changed source picks only the light test of its name.
        assert_eq!(
            targets(select(&s, false, &["src/shop/cart.py"])),
            ["tests/test_cart.py"]
        );
        // A changed heavy test, or a helper next to heavy tests, picks nothing.
        for changed in [
            "tests/integration/test_cart.py",
            "tests/test_cart_integration.py",
            "tests/integration/conftest.py",
        ] {
            assert_eq!(
                select(&s, false, &[changed]),
                TestSelection::None,
                "{changed}"
            );
            assert_eq!(
                select(&s, true, &[changed]),
                TestSelection::None,
                "{changed}"
            );
        }
    }

    #[test]
    fn heavy_ignores_are_anchored_at_the_escaped_root() {
        let flags = heavy_ignores(Path::new("/w/e2e/app[1]"));
        assert!(
            flags
                .iter()
                .all(|f| f.starts_with("--ignore-glob=/w/e2e/app[[]1]"))
        );
        assert!(flags.contains(&"--ignore-glob=/w/e2e/app[[]1]*/integration/*".to_string()));
        assert!(flags.contains(&"--ignore-glob=/w/e2e/app[[]1]*[_-]e2e.py".to_string()));
        assert!(flags.contains(&"--ignore-glob=/w/e2e/app[[]1]*/cypress[_-]*.py".to_string()));
        assert_eq!(flags.len(), 5 + 4 * 4);
    }

    #[test]
    fn module_names() {
        assert!(is_test_module("test_a.py") && is_test_module("a_test.py"));
        assert!(
            !is_test_module("a.py") && !is_test_module("test_a.txt") && !is_test_module("testa.py")
        );
    }
}
