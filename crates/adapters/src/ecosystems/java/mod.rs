//! Java ecosystem adapter.
//!
//! [`JavaEcosystem`] implements [`ToolchainDetector`] for Maven and Gradle projects: Spotless for
//! format, Checkstyle, PMD, and SpotBugs for lint, `javac` (through the build) for typecheck, the
//! project's JUnit tests for tests, and PIT for mutation testing. Every tool runs through the
//! project's own build, offline, from the root of the Maven reactor or the Gradle build; nothing
//! is installed or downloaded. The details are in [`build`], [`detect`], [`parse`], and
//! [`select`], and in `docs/ecosystems.md`.
//!
//! Detection reads a few small build files, so the adapter keeps no state and nothing is cached:
//! every call sees the project as it is now. A JVM start dominates every call anyway.

pub mod build;
pub mod detect;
pub mod parse;
pub mod select;
pub mod xml;

use std::path::{Path, PathBuf};

use hookah_application::error::PortError;
use hookah_application::ports::{ProcessOutput, ToolchainDetector};
use hookah_domain::finding::Finding;
use hookah_domain::project::Project;
use hookah_domain::session::{ChangedLines, FileRole};
use hookah_domain::toolchain::{MutationSelection, TestSelection, Tool, Toolchain};

use build::Build;
use select::TestOption;

/// Detector for Maven and Gradle projects.
#[derive(Debug, Default)]
pub struct JavaEcosystem;

impl JavaEcosystem {
    /// Creates the detector.
    pub fn new() -> Self {
        Self
    }
}

impl ToolchainDetector for JavaEcosystem {
    fn detect(&self, project: &Project) -> Result<Toolchain, PortError> {
        Ok(Build::load(&project.root).map_or_else(Toolchain::default, |b| detect::toolchain(&b)))
    }

    fn classify(&self, project: &Project, file: &Path, _changed: &ChangedLines) -> FileRole {
        select::classify(project, file)
    }

    fn select_mutants(
        &self,
        project: &Project,
        changed_tests: &[PathBuf],
        changed_sources: &[PathBuf],
    ) -> MutationSelection {
        let Some(build) = Build::load(&project.root) else {
            return MutationSelection::None;
        };
        select::mutants::select(
            project,
            TestOption::of(build.kind),
            changed_tests,
            changed_sources,
        )
    }

    fn select_tests(
        &self,
        project: &Project,
        changed: &[PathBuf],
    ) -> Result<TestSelection, PortError> {
        let Some(build) = Build::load(&project.root) else {
            return Ok(TestSelection::None);
        };
        Ok(select::tests(project, TestOption::of(build.kind), changed))
    }

    fn parse(
        &self,
        project: &Project,
        tool: &Tool,
        output: &ProcessOutput,
    ) -> Result<Vec<Finding>, PortError> {
        parse::parse(project, tool, output)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use hookah_domain::project::Ecosystem;
    use hookah_domain::toolchain::CheckKind;

    use super::*;
    use build::tests::Tree;

    fn project(t: &Tree) -> Project {
        Project {
            root: t.0.clone(),
            ecosystem: Ecosystem::Java,
            manifests: vec![],
        }
    }

    fn maven_sample() -> Tree {
        let t = Tree::new("eco-maven");
        t.write(
            "pom.xml",
            "<project><dependencies><dependency><artifactId>junit-jupiter</artifactId></dependency></dependencies>\
             <build><plugins><plugin><artifactId>maven-checkstyle-plugin</artifactId></plugin></plugins></build></project>",
        );
        t.write("src/main/java/a/Foo.java", "package a; class Foo {}");
        t.write(
            "src/test/java/a/FooTest.java",
            "package a; class FooTest {}",
        );
        t
    }

    #[test]
    fn a_project_without_a_build_has_no_toolchain() {
        let t = Tree::new("eco-none");
        let eco = JavaEcosystem::new();
        let p = project(&t);
        assert!(eco.detect(&p).unwrap().is_empty());
        assert_eq!(eco.select_tests(&p, &[]).unwrap(), TestSelection::None);
        let changed = [t.path("src/main/java/a/Foo.java")];
        assert_eq!(
            eco.select_mutants(&p, &changed, &[]),
            MutationSelection::None
        );
    }

    #[test]
    fn detect_select_and_classify_delegate_to_the_modules() {
        let t = maven_sample();
        let eco = JavaEcosystem::new();
        let p = project(&t);
        let toolchain = eco.detect(&p).unwrap();
        let lint: Vec<_> = toolchain
            .of_kind(CheckKind::Lint)
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(lint, ["checkstyle"]);
        assert_eq!(toolchain.missing().len(), 1, "PIT is missing");

        let foo = t.path("src/main/java/a/Foo.java");
        let test = t.path("src/test/java/a/FooTest.java");
        assert_eq!(eco.classify(&p, &test, &ChangedLines::All), FileRole::Test);
        assert_eq!(eco.classify(&p, &foo, &ChangedLines::All), FileRole::Source);
        assert_eq!(
            eco.select_tests(&p, std::slice::from_ref(&foo)).unwrap(),
            TestSelection::Targets(vec!["-Dtest=a.FooTest".into()])
        );
        assert_eq!(
            eco.select_mutants(&p, &[test], &[foo]),
            MutationSelection::Targets(vec![
                "-DtargetClasses=a.Foo,a.Foo$*".into(),
                "-DtargetTests=a.FooTest".into()
            ])
        );
    }

    #[test]
    fn gradle_projects_select_with_gradle_options() {
        let t = Tree::new("eco-gradle");
        t.write("build.gradle", "plugins { id 'java' }");
        t.write("src/main/java/a/Foo.java", "package a; class Foo {}");
        t.write(
            "src/test/java/a/FooTest.java",
            "package a; class FooTest {}",
        );
        let eco = JavaEcosystem::new();
        let p = project(&t);
        assert_eq!(
            eco.select_tests(&p, &[t.path("src/main/java/a/Foo.java")])
                .unwrap(),
            TestSelection::Targets(vec!["--tests=a.FooTest".into()])
        );
        assert_eq!(
            eco.select_mutants(&p, &[t.path("src/test/java/a/FooTest.java")], &[]),
            MutationSelection::Targets(vec!["--targetTests=a.FooTest".into()])
        );
    }

    #[test]
    fn parse_goes_to_the_parsers() {
        let t = maven_sample();
        let eco = JavaEcosystem::new();
        let p = project(&t);
        let toolchain = eco.detect(&p).unwrap();
        let javac = toolchain.of_kind(CheckKind::Typecheck).next().unwrap();
        let out = ProcessOutput {
            exit_code: Some(1),
            stdout: format!(
                "[ERROR] {}:[3,5] boom\n",
                t.path("src/main/java/a/Foo.java").display()
            ),
            stderr: String::new(),
            timed_out: false,
        };
        let found = eco.parse(&p, javac, &out).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].location.path,
            Path::new("src/main/java/a/Foo.java")
        );
    }
}
