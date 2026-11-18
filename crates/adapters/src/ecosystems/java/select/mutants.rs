//! PIT targets.
//!
//! Mutation testing runs only when a test changed (the use case's rule) and after green tests.
//! The sources to mutate are the changed sources plus the sources the changed tests cover, found
//! by the naming convention (`FooTest` covers `Foo`); only sources that exist count. The tests to
//! run against them are the light tests related to those sources, plus the changed light tests.
//! Without a source or without a light test nothing runs.
//!
//! The selection is the options of the build tool, one string each, so the command only places
//! them (see [`super::TestOption`]):
//!
//! - Maven: `-DtargetClasses=a.Foo,a.Foo$*,...` and `-DtargetTests=a.FooTest,...`. The `$*` globs
//!   keep nested classes in.
//! - Gradle: `--targetTests=a.FooTest,...`. The classes to mutate come from the project's own
//!   `pitest {}` block, because Gradle takes no such option on the command line.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use hookah_domain::project::Project;
use hookah_domain::toolchain::MutationSelection;

use super::{Class, TEST_SUFFIXES, TestOption, light, related_tests};

/// The sources a test file most likely covers, project-relative: the class name without its test
/// suffix or prefix, in the same package of `src/main`.
fn sources_of_test(project: &Project, rel: &Path) -> Vec<PathBuf> {
    let Some(class) = Class::of(rel) else {
        return vec![];
    };
    let name = &class.name;
    let stripped = TEST_SUFFIXES
        .iter()
        .find_map(|s| name.strip_suffix(s))
        .or_else(|| name.strip_prefix("Test"))
        .or_else(|| name.strip_suffix("TestCase"));
    let Some(stripped) = stripped.filter(|s| !s.is_empty()) else {
        return vec![];
    };
    let path = class.path_in("main", stripped);
    if project.root.join(&path).is_file() {
        vec![path]
    } else {
        vec![]
    }
}

/// Chooses what PIT mutates and with which tests.
pub fn select(
    project: &Project,
    option: TestOption,
    changed_tests: &[PathBuf],
    changed_sources: &[PathBuf],
) -> MutationSelection {
    let rel = |files: &[PathBuf]| -> Vec<PathBuf> {
        files
            .iter()
            .filter(|f| f.extension().is_some_and(|e| e == "java"))
            .filter_map(|f| project.relative(f).ok())
            .collect()
    };
    // A changed heavy test says nothing about the light tests: it starts no mutation run.
    let tests: Vec<PathBuf> = rel(changed_tests)
        .into_iter()
        .filter(|t| light(project, t))
        .collect();
    let mut sources: BTreeSet<PathBuf> = rel(changed_sources)
        .into_iter()
        .filter(|p| Class::of(p).is_some_and(|c| c.set == "main"))
        .filter(|p| project.root.join(p).is_file())
        .collect();
    sources.extend(tests.iter().flat_map(|t| sources_of_test(project, t)));
    let sources: Vec<PathBuf> = sources.into_iter().collect();

    let test_classes: BTreeSet<String> = sources
        .iter()
        .chain(tests.iter())
        .flat_map(|p| related_tests(project, p))
        .filter_map(|p| Class::of(&p))
        .map(|c| c.qualified())
        .collect();
    let source_classes: Vec<String> = sources
        .iter()
        .filter_map(|p| Class::of(p))
        .map(|c| c.qualified())
        .collect();
    if source_classes.is_empty() || test_classes.is_empty() {
        return MutationSelection::None;
    }
    let tests = test_classes.into_iter().collect::<Vec<_>>().join(",");
    MutationSelection::Targets(match option {
        TestOption::Surefire => {
            let classes = source_classes
                .iter()
                .flat_map(|c| [c.clone(), format!("{c}$*")])
                .collect::<Vec<_>>()
                .join(",");
            vec![
                format!("-DtargetClasses={classes}"),
                format!("-DtargetTests={tests}"),
            ]
        }
        TestOption::Gradle => vec![format!("--targetTests={tests}")],
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::fixture::project;
    use super::*;

    fn abs(p: &Project, rels: &[&str]) -> Vec<PathBuf> {
        rels.iter().map(|r| p.root.join(r)).collect()
    }

    #[test]
    fn a_changed_test_mutates_its_source_with_the_related_tests() {
        let (_t, p) = project();
        let tests = abs(&p, &["src/test/java/a/b/FooTest.java"]);
        assert_eq!(
            select(&p, TestOption::Surefire, &tests, &[]),
            MutationSelection::Targets(vec![
                "-DtargetClasses=a.b.Foo,a.b.Foo$*".into(),
                "-DtargetTests=a.b.FooTest,a.b.FooTests,a.b.TestFoo".into(),
            ])
        );
        assert_eq!(
            select(&p, TestOption::Gradle, &tests, &[]),
            MutationSelection::Targets(vec![
                "--targetTests=a.b.FooTest,a.b.FooTests,a.b.TestFoo".into()
            ])
        );
    }

    #[test]
    fn changed_sources_and_tests_combine_without_duplicates() {
        let (_t, p) = project();
        let tests = abs(&p, &["src/test/java/RootTest.java"]);
        let sources = abs(
            &p,
            &["src/main/java/a/b/Foo.java", "src/main/java/Root.java"],
        );
        assert_eq!(
            select(&p, TestOption::Surefire, &tests, &sources),
            MutationSelection::Targets(vec![
                "-DtargetClasses=Root,Root$*,a.b.Foo,a.b.Foo$*".into(),
                "-DtargetTests=RootTest,a.b.FooTest,a.b.FooTests,a.b.TestFoo".into(),
            ])
        );
    }

    #[test]
    fn heavy_tests_and_unrelated_files_select_nothing() {
        let (t, p) = project();
        // FooIT is heavy and BarTest is tagged heavy: no light test, nothing to run.
        let it = abs(
            &p,
            &[
                "src/test/java/a/b/FooIT.java",
                "src/test/java/a/b/BarTest.java",
            ],
        );
        assert_eq!(
            select(&p, TestOption::Surefire, &it, &[]),
            MutationSelection::None
        );
        // A test without a source.
        t.write("src/test/java/a/b/OrphanTest.java", "class OrphanTest {}");
        let orphan = abs(&p, &["src/test/java/a/b/OrphanTest.java"]);
        assert_eq!(
            select(&p, TestOption::Surefire, &orphan, &[]),
            MutationSelection::None
        );
        // Sources that do not exist, resources, and files outside the project.
        let gone = abs(
            &p,
            &["src/main/java/a/b/Gone.java", "src/main/resources/x.java"],
        );
        assert_eq!(
            select(&p, TestOption::Surefire, &[], &gone),
            MutationSelection::None
        );
        let outside = vec![PathBuf::from("/elsewhere/FooTest.java")];
        assert_eq!(
            select(&p, TestOption::Surefire, &outside, &[]),
            MutationSelection::None
        );
        assert_eq!(
            select(&p, TestOption::Gradle, &[], &[]),
            MutationSelection::None
        );
    }

    #[test]
    fn test_names_map_to_sources_by_suffix_and_prefix() {
        let (t, p) = project();
        let of = |rel: &str| sources_of_test(&p, Path::new(rel));
        assert_eq!(
            of("src/test/java/a/b/FooTest.java"),
            [Path::new("src/main/java/a/b/Foo.java")]
        );
        assert_eq!(
            of("src/test/java/a/b/FooTests.java"),
            [Path::new("src/main/java/a/b/Foo.java")]
        );
        assert_eq!(
            of("src/test/java/a/b/TestFoo.java"),
            [Path::new("src/main/java/a/b/Foo.java")]
        );
        assert_eq!(
            of("src/test/java/a/b/FooIT.java"),
            [Path::new("src/main/java/a/b/Foo.java")]
        );
        t.write("src/main/java/a/b/Baz.java", "class Baz {}");
        assert_eq!(
            of("src/test/java/a/b/BazTestCase.java"),
            [Path::new("src/main/java/a/b/Baz.java")]
        );
        assert!(of("src/test/java/a/b/Helper.java").is_empty());
        assert!(of("src/test/java/a/b/Test.java").is_empty());
        assert!(of("src/test/java/a/b/Nothing.java").is_empty());
        assert!(of("README.md").is_empty());
    }
}
