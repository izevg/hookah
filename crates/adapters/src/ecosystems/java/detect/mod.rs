//! Java toolchain detection.
//!
//! The project root is the directory of the nearest `pom.xml`, `build.gradle(.kts)`, or
//! `settings.gradle(.kts)`. Commands run at the root of the build (the Maven reactor, the Gradle
//! build), addressing the module with `-pl <module>` or `:<module>:<task>`, through the project's
//! wrapper (`mvnw`, `gradlew`) when it has one, else `mvn`/`gradle` from `PATH`. Everything runs
//! offline (`mvn -o`, `gradle --offline`): hookah never downloads anything, so a project whose
//! dependencies are not resolved yet is reported as degraded, not blocked.
//!
//! Only tools the build files configure run (see [`crate::ecosystems::java::build`]). Maven and
//! Gradle commands are in [`maven`] and [`gradle`]; both produce the same tools:
//!
//! | Check     | Tool         | Used when                       | Scope                                  | Timeout |
//! |-----------|--------------|---------------------------------|----------------------------------------|---------|
//! | format    | `spotless`   | the Spotless plugin             | the edited file                        | 60 s    |
//! | lint      | `checkstyle` | the Checkstyle plugin           | the module, from its XML report        | 120 s   |
//! | lint      | `pmd`        | the PMD plugin                  | the module, from its XML report        | 120 s   |
//! | lint      | `spotbugs`   | the SpotBugs plugin             | the module, from its XML report        | 180 s   |
//! | typecheck | `javac`      | always                          | main and test sources, plus dependents | 180 s   |
//! | test      | `surefire` / `gradle-test` | always            | related light test classes             | 480 s   |
//! | mutation  | `pitest`     | the PIT plugin                  | changed sources and their light tests  | 600 s   |
//!
//! Architecture rules (ArchUnit) are ordinary tests, so they run with the tests; there is no
//! separate architecture tool. A project with tests but without PIT gets a missing-tool notice
//! carrying the plugin coordinates.

pub mod gradle;
pub mod maven;

use std::path::Path;
use std::time::Duration;

use hookah_domain::toolchain::{Arg, CheckKind, CommandSpec, MissingTool, Tool, Toolchain};

use super::build::{Build, BuildKind};

/// Name of the Spotless formatter.
pub const SPOTLESS: &str = "spotless";
/// Name of the Checkstyle linter.
pub const CHECKSTYLE: &str = "checkstyle";
/// Name of the PMD linter.
pub const PMD: &str = "pmd";
/// Name of the SpotBugs linter.
pub const SPOTBUGS: &str = "spotbugs";
/// Name of the compile check.
pub const JAVAC: &str = "javac";
/// Name of the Maven test tool.
pub const SUREFIRE: &str = "surefire";
/// Name of the Gradle test tool.
pub const GRADLE_TEST: &str = "gradle-test";
/// Name of the PIT mutation tool.
pub const PITEST: &str = "pitest";

pub(super) const FORMAT_TIMEOUT: Duration = Duration::from_secs(60);
pub(super) const LINT_TIMEOUT: Duration = Duration::from_secs(120);
pub(super) const SPOTBUGS_TIMEOUT: Duration = Duration::from_secs(180);
pub(super) const COMPILE_TIMEOUT: Duration = Duration::from_secs(180);
pub(super) const TEST_TIMEOUT: Duration = Duration::from_secs(480);
pub(super) const PIT_TIMEOUT: Duration = Duration::from_secs(600);

/// JUnit 5 tags that mark a test as heavy; their tests never run.
pub const HEAVY_TAGS: [&str; 3] = ["integration", "slow", "e2e"];

pub(super) fn lit(s: &str) -> Arg {
    Arg::Literal(s.to_string())
}

pub(super) fn tool(
    name: &str,
    kind: CheckKind,
    build: &Build,
    args: Vec<Arg>,
    timeout: Duration,
) -> Tool {
    Tool {
        name: name.to_string(),
        kind,
        command: CommandSpec::new(build.program.clone(), args, build.root.clone(), timeout),
    }
}

/// The toolchain of the module `build` describes.
pub fn toolchain(build: &Build) -> Toolchain {
    let tools = match build.kind {
        BuildKind::Maven => maven::tools(build),
        BuildKind::Gradle => gradle::tools(build),
    };
    Toolchain::new(tools).with_missing(missing(build))
}

fn missing(build: &Build) -> Vec<MissingTool> {
    let f = &build.features;
    let mut out = Vec::new();
    if has_tests(&build.module_dir) {
        match (f.pitest, f.pitest_xml, build.kind) {
            (false, _, BuildKind::Maven) => out.push(MissingTool::new(
                PITEST,
                CheckKind::Mutation,
                "add org.pitest:pitest-maven (and org.pitest:pitest-junit5-plugin for JUnit 5) to <build><plugins> in pom.xml",
            )),
            (false, _, BuildKind::Gradle) => out.push(MissingTool::new(
                PITEST,
                CheckKind::Mutation,
                "add the info.solidsoft.pitest plugin (id(\"info.solidsoft.pitest\")) to the build script",
            )),
            (true, false, BuildKind::Gradle) => out.push(MissingTool::new(
                PITEST,
                CheckKind::Mutation,
                "set outputFormats = [\"XML\"] in the pitest block: hookah reads build/reports/pitest/mutations.xml",
            )),
            _ => {}
        }
    }
    if f.spotbugs && !f.spotbugs_xml {
        out.push(MissingTool::new(
            SPOTBUGS,
            CheckKind::Lint,
            "enable the XML report: tasks.spotbugsMain { reports { xml.required = true } }",
        ));
    }
    out
}

/// Whether `module_dir` has Java test sources.
fn has_tests(module_dir: &Path) -> bool {
    has_java(&module_dir.join("src").join("test").join("java"), 8)
}

fn has_java(dir: &Path, depth: usize) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        let path = e.path();
        if path.is_dir() {
            depth > 0 && has_java(&path, depth - 1)
        } else {
            path.extension().is_some_and(|x| x == "java")
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::build::Features;
    use super::super::build::tests::Tree;
    use super::*;

    pub(super) fn build(kind: BuildKind, dir: &Path, features: Features) -> Build {
        Build {
            kind,
            root: dir.to_path_buf(),
            module_dir: dir.to_path_buf(),
            selector: None,
            program: "prog".into(),
            dependents: vec![],
            features,
        }
    }

    #[test]
    fn tests_are_found_at_any_depth_but_only_java() {
        let t = Tree::new("det-tests");
        assert!(!has_tests(&t.0));
        t.write("src/test/resources/x.txt", "");
        assert!(!has_tests(&t.0));
        t.write("src/test/java/a/b/c/FooTest.java", "");
        assert!(has_tests(&t.0));
        let deep = Tree::new("det-deep");
        deep.write("src/test/java/a/b/c/d/e/f/g/h/i/j/FooTest.java", "");
        assert!(!has_tests(&deep.0), "the search is bounded");
    }

    #[test]
    fn a_missing_pit_is_reported_only_with_tests() {
        let t = Tree::new("det-missing");
        for kind in [BuildKind::Maven, BuildKind::Gradle] {
            let b = build(kind, &t.0, Features::default());
            assert!(missing(&b).is_empty());
        }
        t.write("src/test/java/FooTest.java", "");
        let m = missing(&build(BuildKind::Maven, &t.0, Features::default()));
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].name, PITEST);
        assert_eq!(m[0].kind, CheckKind::Mutation);
        assert!(m[0].hint.contains("org.pitest:pitest-maven"));
        let g = missing(&build(BuildKind::Gradle, &t.0, Features::default()));
        assert!(g[0].hint.contains("info.solidsoft.pitest"));
    }

    #[test]
    fn gradle_pit_and_spotbugs_without_xml_are_reported() {
        let t = Tree::new("det-xml");
        t.write("src/test/java/FooTest.java", "");
        let f = Features {
            pitest: true,
            spotbugs: true,
            ..Features::default()
        };
        let m = missing(&build(BuildKind::Gradle, &t.0, f.clone()));
        let names: Vec<_> = m.iter().map(|m| (m.name.as_str(), m.kind)).collect();
        assert_eq!(
            names,
            [(PITEST, CheckKind::Mutation), (SPOTBUGS, CheckKind::Lint)]
        );
        assert!(m[0].hint.contains("outputFormats"));
        // Maven asks for the reports itself.
        let f = Features {
            pitest: true,
            pitest_xml: true,
            spotbugs: true,
            spotbugs_xml: true,
            ..Features::default()
        };
        assert!(missing(&build(BuildKind::Maven, &t.0, f.clone())).is_empty());
        assert!(missing(&build(BuildKind::Gradle, &t.0, f)).is_empty());
    }
}
