//! Mutation target selection.
//!
//! The targets are mutmut 3 name globs, one per source file: `<module>.x[_ǁ]*`, where `<module>`
//! is mutmut's module name (`src/shop/cart.py` is `shop.cart`) and `x_`/`xǁ` start the mangled
//! name of a function or method. The class of globs matches no sub-module that happens to share
//! the prefix.
//!
//! The sources are the changed sources plus the sources of the changed tests. `test_foo.py` and
//! `foo_test.py` map to the non-test files named `foo.py` anywhere in the project (src layout,
//! flat layout, or nested packages). When several files share the name, only those whose
//! directory shares the most path components with the test's directory are kept, so
//! `tests/domain/test_cart.py` picks `shop/domain/cart.py` over `shop/infra/cart.py`. Files that
//! no longer exist are dropped; an empty result selects nothing.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use hookah_domain::project::Project;
use hookah_domain::toolchain::MutationSelection;

use super::{existing_python, is_test_module, is_test_path};
use crate::ecosystems::python::layout;

/// Components that carry no information about which package a file belongs to.
const GENERIC_DIRS: [&str; 3] = ["src", "tests", "test"];

/// The source file name a test module covers: `test_foo.py` and `foo_test.py` give `foo.py`.
fn covered_name(test: &str) -> Option<String> {
    let stem = test.strip_suffix(".py")?;
    let covered = stem
        .strip_prefix("test_")
        .or_else(|| stem.strip_suffix("_test"))?;
    (!covered.is_empty()).then(|| format!("{covered}.py"))
}

fn dir_parts(rel: &Path) -> BTreeSet<String> {
    rel.parent()
        .into_iter()
        .flat_map(Path::components)
        .filter_map(|c| c.as_os_str().to_str())
        .filter(|c| !GENERIC_DIRS.contains(c))
        .map(str::to_string)
        .collect()
}

/// The mutmut glob selecting every mutant of one source file.
pub fn glob(rel: &Path) -> String {
    format!("{}.x[_ǁ]*", layout::mutmut_module(rel))
}

/// Chooses what to mutate for the changed tests and sources (absolute paths).
pub fn select(
    project: &Project,
    changed_tests: &[PathBuf],
    changed_sources: &[PathBuf],
) -> MutationSelection {
    let mut sources: BTreeSet<PathBuf> = existing_python(project, changed_sources)
        .into_iter()
        .filter(|rel| !is_test_path(rel))
        .collect();

    let tests = existing_python(project, changed_tests);
    if !tests.is_empty() {
        let candidates: Vec<PathBuf> = layout::python_files(&project.root)
            .into_iter()
            .filter(|rel| !is_test_path(rel))
            .collect();
        for test in &tests {
            let name = test.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !is_test_module(name) {
                continue;
            }
            let Some(covered) = covered_name(name) else {
                continue;
            };
            sources.extend(best_matches(test, &covered, &candidates));
        }
    }
    MutationSelection::from_targets(sources.iter().map(|rel| glob(rel)).collect())
}

/// The candidates named `covered` whose directories overlap most with the test's.
fn best_matches(test: &Path, covered: &str, candidates: &[PathBuf]) -> Vec<PathBuf> {
    let wanted = dir_parts(test);
    let named: Vec<(&PathBuf, usize)> = candidates
        .iter()
        .filter(|c| c.file_name().is_some_and(|n| n == covered))
        .map(|c| (c, dir_parts(c).intersection(&wanted).count()))
        .collect();
    let best = named.iter().map(|(_, score)| *score).max().unwrap_or(0);
    named
        .into_iter()
        .filter(|(_, score)| *score == best)
        .map(|(c, _)| c.clone())
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use crate::ecosystems::python::fixture::Scratch;

    use super::super::tests::project;
    use super::*;

    fn abs(s: &Scratch, rels: &[&str]) -> Vec<PathBuf> {
        rels.iter().map(|r| s.root().join(r)).collect()
    }

    fn targets(sel: MutationSelection) -> Vec<String> {
        match sel {
            MutationSelection::Targets(t) => t,
            MutationSelection::None => vec![],
        }
    }

    fn tree(name: &str) -> Scratch {
        let s = Scratch::new(name);
        for f in [
            "src/shop/domain/cart.py",
            "src/shop/infra/cart.py",
            "src/shop/pricing.py",
            "src/shop/other.py",
            "tests/domain/test_cart.py",
            "tests/test_pricing.py",
            "tests/helpers.py",
        ] {
            s.write(f, "");
        }
        s
    }

    #[test]
    fn globs_follow_mutmut_module_names() {
        assert_eq!(glob(Path::new("src/shop/cart.py")), "shop.cart.x[_ǁ]*");
        assert_eq!(glob(Path::new("shop/cart.py")), "shop.cart.x[_ǁ]*");
    }

    #[test]
    fn changed_tests_map_to_their_sources_by_name_and_directory() {
        let s = tree("mut-tests");
        let sel = select(
            &project(&s),
            &abs(&s, &["tests/domain/test_cart.py", "tests/test_pricing.py"]),
            &[],
        );
        assert_eq!(
            targets(sel),
            ["shop.domain.cart.x[_ǁ]*", "shop.pricing.x[_ǁ]*"]
        );
    }

    #[test]
    fn changed_sources_are_included_and_deduplicated() {
        let s = tree("mut-sources");
        let sel = select(
            &project(&s),
            &abs(&s, &["tests/test_pricing.py"]),
            &abs(&s, &["src/shop/pricing.py", "src/shop/other.py"]),
        );
        assert_eq!(targets(sel), ["shop.other.x[_ǁ]*", "shop.pricing.x[_ǁ]*"]);
    }

    #[test]
    fn ties_keep_every_candidate() {
        let s = tree("mut-ties");
        s.write("tests/test_cart.py", "");
        let sel = select(&project(&s), &abs(&s, &["tests/test_cart.py"]), &[]);
        assert_eq!(
            targets(sel),
            ["shop.domain.cart.x[_ǁ]*", "shop.infra.cart.x[_ǁ]*"]
        );
    }

    #[test]
    fn tests_without_a_source_and_helpers_select_nothing() {
        let s = tree("mut-none");
        s.write("tests/test_ghost.py", "");
        let sel = select(
            &project(&s),
            &abs(
                &s,
                &[
                    "tests/test_ghost.py",
                    "tests/helpers.py",
                    "tests/conftest.py",
                ],
            ),
            &[],
        );
        assert_eq!(sel, MutationSelection::None);
    }

    #[test]
    fn missing_files_and_test_files_among_sources_are_dropped() {
        let s = tree("mut-missing");
        let sel = select(
            &project(&s),
            &abs(&s, &["tests/domain/test_gone.py"]),
            &abs(&s, &["src/shop/gone.py", "tests/test_pricing.py"]),
        );
        assert_eq!(sel, MutationSelection::None);
    }

    #[test]
    fn flat_layout() {
        let s = Scratch::new("mut-flat");
        s.write("shop/cart.py", "");
        s.write("test_cart.py", "");
        let sel = select(&project(&s), &abs(&s, &["test_cart.py"]), &[]);
        assert_eq!(targets(sel), ["shop.cart.x[_ǁ]*"]);
    }

    #[test]
    fn covered_names() {
        assert_eq!(covered_name("test_foo.py").as_deref(), Some("foo.py"));
        assert_eq!(covered_name("foo_test.py").as_deref(), Some("foo.py"));
        assert_eq!(covered_name("conftest.py"), None);
    }
}
