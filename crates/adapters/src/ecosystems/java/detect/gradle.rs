//! Gradle commands. They run at the build root with `--console=plain --offline`, which makes the
//! task lines (`> Task :app:test FAILED`) and the javac errors parseable, and never downloads.
//! The Gradle daemon is on, as it is by default: the first call of a session pays the JVM start,
//! the next ones do not.
//!
//! | Tool          | Tasks (after `gradle --console=plain --offline [--continue]`)                              |
//! |---------------|---------------------------------------------------------------------------------------------|
//! | `spotless`    | `:m:spotlessApply -PspotlessIdeHook=<abs file>` (Spotless formats just that file)           |
//! | `checkstyle`  | `:m:checkstyleMain :m:checkstyleTest`, then `<m>/build/reports/checkstyle/{main,test}.xml`  |
//! | `pmd`         | `:m:pmdMain :m:pmdTest`, then `<m>/build/reports/pmd/{main,test}.xml`                       |
//! | `spotbugs`    | `:m:spotbugsMain`, then `<m>/build/reports/spotbugs/main.xml` (needs the XML report)        |
//! | `javac`       | `:m:compileTestJava` and `:d:compileTestJava` of every dependent project `d`                |
//! | `gradle-test` | `:m:test --tests=<class>...`                                                                |
//! | `pitest`      | `:m:pitest --targetTests=<classes>`, then `<m>/build/reports/pitest/**/mutations.xml`       |
//!
//! Gradle cannot take PIT's `targetClasses` or the XML output format on the command line, so those
//! come from the project's own `pitest {}` block; the command narrows the tests only. Tag
//! exclusion (`excludeTags`) is likewise a build-script setting: the test selection leaves out the
//! classes whose source carries a heavy `@Tag` instead.

use hookah_domain::toolchain::{Arg, CheckKind, Tool};

use super::{
    CHECKSTYLE, COMPILE_TIMEOUT, FORMAT_TIMEOUT, GRADLE_TEST, JAVAC, LINT_TIMEOUT, PIT_TIMEOUT,
    PITEST, PMD, SPOTBUGS, SPOTBUGS_TIMEOUT, SPOTLESS, TEST_TIMEOUT, lit, tool,
};
use crate::ecosystems::java::build::Build;

/// The Gradle tools of the module, in execution order.
pub fn tools(build: &Build) -> Vec<Tool> {
    let f = &build.features;
    let mut tools = Vec::new();
    let args = |continue_: bool, tasks: Vec<String>, tail: Vec<Arg>| {
        let mut a = vec![lit("--console=plain"), lit("--offline")];
        if continue_ {
            a.push(lit("--continue"));
        }
        a.extend(tasks.iter().map(|t| lit(t)));
        a.extend(tail);
        a
    };

    if f.spotless {
        let hook = format!(
            "-PspotlessIdeHook={}{}",
            build.module_dir.display(),
            std::path::MAIN_SEPARATOR
        );
        let tail = vec![Arg::TargetsJoined {
            prefix: hook,
            separator: ",".into(),
        }];
        tools.push(tool(
            SPOTLESS,
            CheckKind::Format,
            build,
            args(false, vec![build.task("spotlessApply")], tail),
            FORMAT_TIMEOUT,
        ));
    }
    if f.checkstyle {
        let tasks = vec![build.task("checkstyleMain"), build.task("checkstyleTest")];
        tools.push(tool(
            CHECKSTYLE,
            CheckKind::Lint,
            build,
            args(true, tasks, vec![]),
            LINT_TIMEOUT,
        ));
    }
    if f.pmd {
        let tasks = vec![build.task("pmdMain"), build.task("pmdTest")];
        tools.push(tool(
            PMD,
            CheckKind::Lint,
            build,
            args(true, tasks, vec![]),
            LINT_TIMEOUT,
        ));
    }
    if f.spotbugs && f.spotbugs_xml {
        tools.push(tool(
            SPOTBUGS,
            CheckKind::Lint,
            build,
            args(true, vec![build.task("spotbugsMain")], vec![]),
            SPOTBUGS_TIMEOUT,
        ));
    }
    let mut compile = vec![build.task("compileTestJava")];
    compile.extend(
        build
            .dependents
            .iter()
            .map(|d| format!("{d}:compileTestJava")),
    );
    tools.push(tool(
        JAVAC,
        CheckKind::Typecheck,
        build,
        args(true, compile, vec![]),
        COMPILE_TIMEOUT,
    ));
    tools.push(tool(
        GRADLE_TEST,
        CheckKind::Test,
        build,
        args(true, vec![build.task("test")], vec![Arg::Targets]),
        TEST_TIMEOUT,
    ));
    if f.pitest && f.pitest_xml {
        tools.push(tool(
            PITEST,
            CheckKind::Mutation,
            build,
            args(false, vec![build.task("pitest")], vec![Arg::Targets]),
            PIT_TIMEOUT,
        ));
    }
    tools
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::Path;

    use super::super::tests::build;
    use super::*;
    use crate::ecosystems::java::build::{BuildKind, Features};

    fn everything() -> Features {
        Features {
            spotless: true,
            checkstyle: true,
            pmd: true,
            spotbugs: true,
            spotbugs_xml: true,
            pitest: true,
            pitest_xml: true,
            junit5: true,
        }
    }

    fn argv(t: &Tool, targets: &[&str]) -> Vec<String> {
        let targets: Vec<String> = targets.iter().map(ToString::to_string).collect();
        t.command.expand_args(&targets)
    }

    fn by_name<'a>(tools: &'a [Tool], name: &str) -> &'a Tool {
        tools.iter().find(|t| t.name == name).unwrap()
    }

    #[test]
    fn a_plain_project_gets_the_compile_check_and_the_tests() {
        let b = build(BuildKind::Gradle, Path::new("/r"), Features::default());
        let tools = tools(&b);
        let names: Vec<_> = tools.iter().map(|t| (t.name.as_str(), t.kind)).collect();
        assert_eq!(
            names,
            [
                (JAVAC, CheckKind::Typecheck),
                (GRADLE_TEST, CheckKind::Test)
            ]
        );
        assert_eq!(
            argv(&tools[0], &[]),
            [
                "--console=plain",
                "--offline",
                "--continue",
                ":compileTestJava"
            ]
        );
        assert_eq!(
            argv(&tools[1], &["--tests=a.B", "--tests=a.C"]),
            [
                "--console=plain",
                "--offline",
                "--continue",
                ":test",
                "--tests=a.B",
                "--tests=a.C"
            ]
        );
        assert_eq!(tools[0].command.cwd, Path::new("/r"));
    }

    #[test]
    fn every_configured_tool_runs_in_order_on_the_module_project() {
        let mut b = build(BuildKind::Gradle, Path::new("/r"), everything());
        b.selector = Some(":app".into());
        b.module_dir = "/r/app".into();
        b.dependents = vec![":web".into(), ":cli".into()];
        let tools = tools(&b);
        let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            [
                SPOTLESS,
                CHECKSTYLE,
                PMD,
                SPOTBUGS,
                JAVAC,
                GRADLE_TEST,
                PITEST
            ]
        );
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            argv(by_name(&tools, SPOTLESS), &["src/A.java"]),
            [
                "--console=plain".to_string(),
                "--offline".into(),
                ":app:spotlessApply".into(),
                format!(
                    "-PspotlessIdeHook={}{sep}src/A.java",
                    Path::new("/r/app").display()
                ),
            ]
        );
        assert_eq!(
            argv(by_name(&tools, CHECKSTYLE), &[]),
            [
                "--console=plain",
                "--offline",
                "--continue",
                ":app:checkstyleMain",
                ":app:checkstyleTest"
            ]
        );
        assert_eq!(
            argv(by_name(&tools, PMD), &[])[3..],
            [":app:pmdMain", ":app:pmdTest"]
        );
        assert_eq!(
            argv(by_name(&tools, SPOTBUGS), &[])[3..],
            [":app:spotbugsMain"]
        );
        assert_eq!(
            argv(by_name(&tools, JAVAC), &[])[3..],
            [
                ":app:compileTestJava",
                ":web:compileTestJava",
                ":cli:compileTestJava"
            ]
        );
        assert_eq!(argv(by_name(&tools, GRADLE_TEST), &[])[3..], [":app:test"]);
        assert_eq!(
            argv(by_name(&tools, PITEST), &["--targetTests=a.BTest"]),
            [
                "--console=plain",
                "--offline",
                ":app:pitest",
                "--targetTests=a.BTest"
            ]
        );
    }

    #[test]
    fn spotbugs_and_pit_need_their_xml_reports() {
        let mut f = everything();
        f.spotbugs_xml = false;
        f.pitest_xml = false;
        let b = build(BuildKind::Gradle, Path::new("/r"), f);
        let names: Vec<_> = tools(&b).iter().map(|t| t.name.clone()).collect();
        assert!(!names.contains(&SPOTBUGS.to_string()));
        assert!(!names.contains(&PITEST.to_string()));
    }
}
