//! Java file roles and related-test selection.
//!
//! - [`classify`]: a file is a test when it lies under `src/test`, `src/it`, or
//!   `src/integrationTest`, or its name ends in `Test`, `Tests`, or `IT`. Everything else is
//!   source.
//! - [`tests`]: the related *light* test classes of the changed files. A changed test selects
//!   itself; a changed source `src/main/java/a/b/Foo.java` selects `src/test/java/a/b/FooTest.java`
//!   and, when they exist, `FooTests`, `TestFoo`, and `FooTestCase`. The result is one target
//!   string per Maven or Gradle option, see [`TestOption`]. Nothing related selects nothing; the
//!   whole suite never runs.
//! - [`mutants`]: PIT targets, see there.
//!
//! **Light by default.** Integration and other heavy tests never run: classes named `*IT`,
//! `*ITCase`, `*IntegrationTest(s)`, or `*E2E(Test)`, anything under `src/it` or
//! `src/integrationTest`, and classes whose source carries `@Tag("integration")`, `@Tag("slow")`,
//! `@Tag("e2e")`, `@SpringBootTest`, or `@Testcontainers`. Maven also passes
//! `-DexcludedGroups=integration,slow,e2e` for JUnit 5 projects, which covers tags on methods.
//! The failsafe plugin is never invoked.

pub mod mutants;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use hookah_domain::project::Project;
use hookah_domain::session::FileRole;
use hookah_domain::toolchain::TestSelection;

use super::build::BuildKind;
use super::detect::HEAVY_TAGS;

/// The most test classes one run selects.
const MAX_TESTS: usize = 30;
/// Source sets whose files are tests.
const TEST_SETS: [&str; 3] = ["test", "it", "integrationTest"];
/// Class name suffixes that make a test a test.
const TEST_SUFFIXES: [&str; 3] = ["Test", "Tests", "IT"];

/// A Java file below `src/<set>/java`, taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Class {
    /// The source set: `main`, `test`, ...
    pub set: String,
    /// The package, dot separated; empty for the default package.
    pub package: String,
    /// The class name: the file name without `.java`.
    pub name: String,
}

impl Class {
    /// Reads `src/<set>/java/<package path>/<Name>.java` from a project-relative path.
    pub fn of(rel: &Path) -> Option<Self> {
        let parts: Vec<&str> = rel
            .components()
            .map(|c| c.as_os_str().to_str())
            .collect::<Option<_>>()?;
        let [src, set, java, package @ .., file] = parts.as_slice() else {
            return None;
        };
        if *src != "src" || *java != "java" {
            return None;
        }
        Some(Self {
            set: (*set).to_string(),
            package: package.join("."),
            name: file.strip_suffix(".java")?.to_string(),
        })
    }

    /// The fully qualified name.
    pub fn qualified(&self) -> String {
        if self.package.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.package, self.name)
        }
    }

    /// The project-relative path of the class `name` in the same package of source set `set`.
    pub fn path_in(&self, set: &str, name: &str) -> PathBuf {
        let mut path: PathBuf = ["src", set, "java"].iter().collect();
        path.extend(self.package.split('.').filter(|p| !p.is_empty()));
        path.join(format!("{name}.java"))
    }
}

/// The role of `file`.
pub fn classify(project: &Project, file: &Path) -> FileRole {
    let Ok(rel) = project.relative(file) else {
        return FileRole::Source;
    };
    if is_test(&rel) {
        FileRole::Test
    } else {
        FileRole::Source
    }
}

pub(super) fn is_test(rel: &Path) -> bool {
    if Class::of(rel).is_some_and(|c| TEST_SETS.contains(&c.set.as_str())) {
        return true;
    }
    let stem = rel.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    rel.extension().is_some_and(|e| e == "java") && TEST_SUFFIXES.iter().any(|s| stem.ends_with(s))
}

/// Whether the test class `name` is heavy by its name alone.
fn heavy_name(name: &str) -> bool {
    name.ends_with("IT")
        || name.ends_with("ITCase")
        || name.contains("IntegrationTest")
        || name.ends_with("E2E")
        || name.ends_with("E2ETest")
}

/// Whether the source of a test class carries a marker of a heavy test.
fn heavy_source(text: &str) -> bool {
    let squeezed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    squeezed.contains("@SpringBootTest")
        || squeezed.contains("@Testcontainers")
        || HEAVY_TAGS
            .iter()
            .any(|tag| squeezed.contains(&format!("@Tag(\"{tag}\")")))
}

/// Whether the test file at project-relative `rel` is light: a unit test in `src/test`.
pub(super) fn light(project: &Project, rel: &Path) -> bool {
    let Some(class) = Class::of(rel) else {
        return false;
    };
    class.set == "test"
        && !heavy_name(&class.name)
        && std::fs::read_to_string(project.root.join(rel)).is_ok_and(|t| !heavy_source(&t))
}

/// The light test files, project-relative, that `rel` (a changed file) relates to: itself when it
/// is a light test, else the conventionally named tests of the source that exist.
pub(super) fn related_tests(project: &Project, rel: &Path) -> Vec<PathBuf> {
    if is_test(rel) {
        return if light(project, rel) {
            vec![rel.to_path_buf()]
        } else {
            vec![]
        };
    }
    let Some(class) = Class::of(rel).filter(|c| c.set == "main") else {
        return vec![];
    };
    let n = &class.name;
    [
        format!("{n}Test"),
        format!("{n}Tests"),
        format!("Test{n}"),
        format!("{n}TestCase"),
    ]
    .iter()
    .map(|name| class.path_in("test", name))
    .filter(|p| project.root.join(p).is_file() && light(project, p))
    .collect()
}

/// The way a build tool takes test class names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestOption {
    /// Surefire: one `-Dtest=a.A,b.B`.
    Surefire,
    /// Gradle: `--tests=a.A` for each class.
    Gradle,
}

impl TestOption {
    /// The option for a build tool.
    pub fn of(kind: BuildKind) -> Self {
        match kind {
            BuildKind::Maven => Self::Surefire,
            BuildKind::Gradle => Self::Gradle,
        }
    }

    fn targets(self, classes: &[String]) -> Vec<String> {
        match self {
            Self::Surefire => vec![format!("-Dtest={}", classes.join(","))],
            Self::Gradle => classes.iter().map(|c| format!("--tests={c}")).collect(),
        }
    }
}

/// The light tests related to the `changed` files (absolute paths).
pub fn tests(project: &Project, option: TestOption, changed: &[PathBuf]) -> TestSelection {
    let classes: BTreeSet<String> = changed
        .iter()
        .filter(|f| f.extension().is_some_and(|e| e == "java"))
        .filter_map(|f| project.relative(f).ok())
        .flat_map(|rel| related_tests(project, &rel))
        .filter_map(|p| Class::of(&p))
        .map(|c| c.qualified())
        .collect();
    let classes: Vec<String> = classes.into_iter().take(MAX_TESTS).collect();
    if classes.is_empty() {
        return TestSelection::None;
    }
    TestSelection::Targets(option.targets(&classes))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(super) mod fixture {
    use hookah_domain::project::Ecosystem;

    use super::*;
    use crate::ecosystems::java::build::tests::Tree;

    /// A project with a source, light and heavy tests.
    pub fn project() -> (Tree, Project) {
        let t = Tree::new("select");
        t.write("src/main/java/a/b/Foo.java", "package a.b; class Foo {}");
        t.write("src/main/java/a/b/Bar.java", "package a.b; class Bar {}");
        t.write("src/main/java/Root.java", "class Root {}");
        t.write("src/test/java/a/b/FooTest.java", "class FooTest {}");
        t.write("src/test/java/a/b/FooTests.java", "class FooTests {}");
        t.write("src/test/java/a/b/TestFoo.java", "class TestFoo {}");
        t.write("src/test/java/a/b/FooIT.java", "class FooIT {}");
        t.write(
            "src/test/java/a/b/BarTest.java",
            "@Tag(\"integration\")\nclass BarTest {}",
        );
        t.write("src/test/java/RootTest.java", "class RootTest {}");
        t.write(
            "src/integrationTest/java/a/b/SlowTest.java",
            "class SlowTest {}",
        );
        let project = Project {
            root: t.0.clone(),
            ecosystem: Ecosystem::Java,
            manifests: vec![],
        };
        (t, project)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::fixture::project;
    use super::*;

    #[test]
    fn splits_standard_layout_paths() {
        let c = Class::of(Path::new("src/main/java/a/b/Foo.java")).unwrap();
        assert_eq!(
            c,
            Class {
                set: "main".into(),
                package: "a.b".into(),
                name: "Foo".into()
            }
        );
        assert_eq!(c.qualified(), "a.b.Foo");
        assert_eq!(
            c.path_in("test", "FooTest"),
            Path::new("src/test/java/a/b/FooTest.java")
        );
        let root = Class::of(Path::new("src/test/java/RootTest.java")).unwrap();
        assert_eq!(root.package, "");
        assert_eq!(root.qualified(), "RootTest");
        assert_eq!(
            root.path_in("main", "Root"),
            Path::new("src/main/java/Root.java")
        );
        assert_eq!(Class::of(Path::new("src/main/kotlin/A.java")), None);
        assert_eq!(Class::of(Path::new("lib/main/java/A.java")), None);
        assert_eq!(Class::of(Path::new("src/main/java/A.kt")), None);
        assert_eq!(Class::of(Path::new("src/main/A.java")), None);
        assert_eq!(Class::of(Path::new("A.java")), None);
    }

    #[test]
    fn roles_follow_the_source_set_and_the_name() {
        let (_t, p) = project();
        let role = |rel: &str| classify(&p, &p.root.join(rel));
        assert_eq!(role("src/main/java/a/b/Foo.java"), FileRole::Source);
        assert_eq!(role("src/test/java/a/b/Helper.java"), FileRole::Test);
        assert_eq!(role("src/it/java/Helper.java"), FileRole::Test);
        assert_eq!(role("src/integrationTest/java/Helper.java"), FileRole::Test);
        assert_eq!(role("src/main/java/a/FooTest.java"), FileRole::Test);
        assert_eq!(role("src/main/java/a/FooTests.java"), FileRole::Test);
        assert_eq!(role("src/main/java/a/FooIT.java"), FileRole::Test);
        assert_eq!(role("src/main/java/a/Contest.java"), FileRole::Source);
        assert_eq!(role("src/main/java/a/Testing.java"), FileRole::Source);
        assert_eq!(role("src/main/resources/FooTest.xml"), FileRole::Source);
        assert_eq!(
            classify(&p, Path::new("/elsewhere/FooTest.java")),
            FileRole::Source
        );
    }

    #[test]
    fn heavy_names_and_sources_are_recognized() {
        for name in [
            "FooIT",
            "FooITCase",
            "OrderIntegrationTest",
            "OrderIntegrationTests",
            "FlowE2E",
            "FlowE2ETest",
        ] {
            assert!(heavy_name(name), "{name}");
        }
        for name in ["FooTest", "Edit", "Item", "Unit"] {
            assert!(!heavy_name(name), "{name}");
        }
        assert!(heavy_source("@SpringBootTest\nclass A {}"));
        assert!(heavy_source("@Testcontainers class A {}"));
        assert!(heavy_source("@Tag( \"slow\" )\nclass A {}"));
        assert!(heavy_source("@Tag(\"e2e\") class A {}"));
        assert!(heavy_source("@Tag(\"integration\") class A {}"));
        assert!(!heavy_source("@Tag(\"fast\") class A {}"));
        assert!(!heavy_source("class A {}"));
    }

    #[test]
    fn only_unit_tests_in_the_test_source_set_are_light() {
        let (_t, p) = project();
        let l = |rel: &str| light(&p, Path::new(rel));
        assert!(l("src/test/java/a/b/FooTest.java"));
        assert!(!l("src/test/java/a/b/FooIT.java"));
        assert!(!l("src/test/java/a/b/BarTest.java"));
        assert!(!l("src/integrationTest/java/a/b/SlowTest.java"));
        assert!(!l("src/test/java/a/b/Missing.java"));
        assert!(!l("src/main/java/a/b/Foo.java"));
        assert!(!l("elsewhere/FooTest.java"));
    }

    #[test]
    fn a_source_relates_to_its_conventionally_named_light_tests() {
        let (_t, p) = project();
        let related = |rel: &str| related_tests(&p, Path::new(rel));
        assert_eq!(
            related("src/main/java/a/b/Foo.java"),
            [
                Path::new("src/test/java/a/b/FooTest.java"),
                Path::new("src/test/java/a/b/FooTests.java"),
                Path::new("src/test/java/a/b/TestFoo.java"),
            ]
        );
        // BarTest is tagged heavy.
        assert!(related("src/main/java/a/b/Bar.java").is_empty());
        assert_eq!(
            related("src/main/java/Root.java"),
            [Path::new("src/test/java/RootTest.java")]
        );
        assert!(related("src/main/resources/x.java").is_empty());
        assert!(related("src/test/resources/x.java").is_empty());
    }

    #[test]
    fn a_changed_test_selects_itself_when_light() {
        let (_t, p) = project();
        assert_eq!(
            related_tests(&p, Path::new("src/test/java/a/b/FooTest.java")),
            [Path::new("src/test/java/a/b/FooTest.java")]
        );
        assert!(related_tests(&p, Path::new("src/test/java/a/b/FooIT.java")).is_empty());
    }

    #[test]
    fn surefire_gets_one_option_and_gradle_one_per_class() {
        let (_t, p) = project();
        let changed = [
            p.root.join("src/main/java/a/b/Foo.java"),
            p.root.join("src/test/java/RootTest.java"),
            p.root.join("README.md"),
            p.root.join("src/main/java/a/b/Bar.java"),
        ];
        assert_eq!(
            tests(&p, TestOption::Surefire, &changed),
            TestSelection::Targets(vec![
                "-Dtest=RootTest,a.b.FooTest,a.b.FooTests,a.b.TestFoo".into()
            ])
        );
        assert_eq!(
            tests(&p, TestOption::Gradle, &changed),
            TestSelection::Targets(
                ["RootTest", "a.b.FooTest", "a.b.FooTests", "a.b.TestFoo"]
                    .map(|c| format!("--tests={c}"))
                    .to_vec()
            )
        );
        assert_eq!(TestOption::of(BuildKind::Maven), TestOption::Surefire);
        assert_eq!(TestOption::of(BuildKind::Gradle), TestOption::Gradle);
    }

    #[test]
    fn nothing_related_selects_nothing() {
        let (_t, p) = project();
        let changed = [
            p.root.join("src/main/java/a/b/Bar.java"),
            p.root.join("src/test/java/a/b/FooIT.java"),
            PathBuf::from("/elsewhere/FooTest.java"),
        ];
        assert_eq!(
            tests(&p, TestOption::Surefire, &changed),
            TestSelection::None
        );
        assert_eq!(tests(&p, TestOption::Gradle, &[]), TestSelection::None);
    }

    #[test]
    fn the_selection_is_capped() {
        let (t, p) = project();
        let mut changed = Vec::new();
        for i in 0..(MAX_TESTS + 5) {
            let rel = format!("src/test/java/many/T{i:02}Test.java");
            changed.push(t.write(&rel, "class T {}"));
        }
        let TestSelection::Targets(targets) = tests(&p, TestOption::Gradle, &changed) else {
            panic!("expected targets");
        };
        assert_eq!(targets.len(), MAX_TESTS);
    }
}
