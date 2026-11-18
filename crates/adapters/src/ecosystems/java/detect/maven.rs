//! Maven commands. They run at the reactor root, in batch mode and offline.
//!
//! | Tool        | Command (after `mvn -B -o`; `-pl` only in a multi-module build)                                  |
//! |-------------|----------------------------------------------------------------------------------------------------|
//! | `spotless`  | `-q spotless:apply -pl <m> -DspotlessFiles=.*/<file>`                                              |
//! | `checkstyle`| `checkstyle:checkstyle` (`test-compile checkstyle:checkstyle -pl <m>,<d>... -am` in a reactor), then `<m>/target/checkstyle-result.xml` |
//! | `pmd`       | `pmd:pmd` (likewise), then `<m>/target/pmd.xml`                                                    |
//! | `spotbugs`  | `test-compile spotbugs:spotbugs -pl <m>,<d>... -am`, then `<m>/target/spotbugsXml.xml`                         |
//! | `javac`     | `-q test-compile -pl <m>,<d>... -am` (the module, its dependents `<d>`, and what they need)         |
//! | `surefire`  | `test -pl <m> -am -Dtest=<classes> -Dsurefire.failIfNoSpecifiedTests=false -DfailIfNoTests=false [-DexcludedGroups=integration,slow,e2e]` |
//! | `pitest`    | `test-compile org.pitest:pitest-maven:mutationCoverage -pl <m> -am -DtargetClasses=<fqcns> -DtargetTests=<fqcns> -DoutputFormats=XML -DtimestampedReports=false -DwithHistory=true -DhistoryInputFile=<m>/target/pit-history.bin -DhistoryOutputFile=<same> -DfailWhenNoMutations=false` |
//!
//! `-am` makes the modules the selected ones need part of the reactor, so they resolve from it and
//! not from `~/.m2`, which `-o` could not fill. The dependents `<d>` are the modules that depend
//! on the module, so an API change that breaks them is caught in the edit phase. They are named in
//! `-pl` and not asked for with `-amd`: `-amd` adds them without what they need besides the
//! module, and that is then looked up in `~/.m2`. hookah computes them from the poms (see
//! `build::reactor`) and keeps them within a budget (`HOOKAH_DOWNSTREAM_BUDGET`, 10): over it, only
//! the module and its upstream compile, with a notice. The compiler plugin sees that a dependency
//! changed only when it changed during the same Maven run, so every command that compiles in a
//! reactor covers the same set of modules (except the test and PIT runs, which come after the
//! compile check):
//! otherwise a lint run that compiled the module first would hide the breakage of its dependents
//! from the compile check.
//!
//! `-DexcludedGroups` is passed for JUnit 5 projects only, because the JUnit 4 provider would read
//! the tag names as category classes.

use hookah_domain::toolchain::{Arg, CheckKind, Tool};

use super::{
    CHECKSTYLE, COMPILE_TIMEOUT, FORMAT_TIMEOUT, HEAVY_TAGS, JAVAC, LINT_TIMEOUT, PIT_TIMEOUT,
    PITEST, PMD, SPOTBUGS, SPOTBUGS_TIMEOUT, SPOTLESS, SUREFIRE, TEST_TIMEOUT, lit, tool,
};
use crate::ecosystems::java::build::{Also, Build};

const NEIGHBORS_NONE: Also = Also {
    upstream: false,
    downstream: false,
};
const UPSTREAM: Also = Also {
    upstream: true,
    downstream: false,
};
const BOTH: Also = Also {
    upstream: true,
    downstream: true,
};

/// The Maven tools of the module, in execution order.
pub fn tools(build: &Build) -> Vec<Tool> {
    let f = &build.features;
    let mut tools = Vec::new();
    let args = |quiet: bool, goals: &[&str], also: Also, tail: Vec<Arg>| {
        let mut a = vec![lit("-B")];
        if quiet {
            a.push(lit("-q"));
        }
        a.push(lit("-o"));
        a.extend(goals.iter().map(|g| lit(g)));
        a.extend(build.reactor_args(also).iter().map(|s| lit(s)));
        a.extend(tail);
        a
    };

    if f.spotless {
        let tail = vec![Arg::TargetsJoined {
            prefix: "-DspotlessFiles=.*/".into(),
            separator: ",.*/".into(),
        }];
        tools.push(tool(
            SPOTLESS,
            CheckKind::Format,
            build,
            args(true, &["spotless:apply"], NEIGHBORS_NONE, tail),
            FORMAT_TIMEOUT,
        ));
    }
    // A module of a reactor resolves its siblings from the reactor, which has them only after
    // their compile; a single module needs no compile for a report goal. The phase is
    // `test-compile`, not `compile`: the compiler plugin notices that the main classes changed
    // only within one Maven run, so a lint run that compiled just the main sources would hide a
    // broken test from the compile check that follows it. For the same reason the run covers the
    // dependents too: whatever compiles first sees the API change, and a module that
    // failed to compile is compiled again by the next run.
    let (prepare, upstream): (&[&str], Also) = if build.selector.is_some() {
        (&["test-compile"], BOTH)
    } else {
        (&[], NEIGHBORS_NONE)
    };
    let report = |goal: &str| {
        let mut goals = prepare.to_vec();
        goals.push(goal);
        args(false, &goals, upstream, vec![])
    };
    if f.checkstyle {
        tools.push(tool(
            CHECKSTYLE,
            CheckKind::Lint,
            build,
            report("checkstyle:checkstyle"),
            LINT_TIMEOUT,
        ));
    }
    if f.pmd {
        tools.push(tool(
            PMD,
            CheckKind::Lint,
            build,
            report("pmd:pmd"),
            LINT_TIMEOUT,
        ));
    }
    if f.spotbugs {
        tools.push(tool(
            SPOTBUGS,
            CheckKind::Lint,
            build,
            args(false, &["test-compile", "spotbugs:spotbugs"], BOTH, vec![]),
            SPOTBUGS_TIMEOUT,
        ));
    }
    tools.push(tool(
        JAVAC,
        CheckKind::Typecheck,
        build,
        args(true, &["test-compile"], BOTH, vec![]),
        COMPILE_TIMEOUT,
    ));

    let mut tail = vec![
        lit("-Dsurefire.failIfNoSpecifiedTests=false"),
        lit("-DfailIfNoTests=false"),
    ];
    if f.junit5 {
        tail.push(lit(&format!("-DexcludedGroups={}", HEAVY_TAGS.join(","))));
    }
    tail.push(Arg::TargetsJoined {
        prefix: "-Dtest=".into(),
        separator: ",".into(),
    });
    tools.push(tool(
        SUREFIRE,
        CheckKind::Test,
        build,
        args(false, &["test"], UPSTREAM, tail),
        TEST_TIMEOUT,
    ));

    if f.pitest {
        let history = build
            .module_dir
            .join("target")
            .join("pit-history.bin")
            .to_string_lossy()
            .into_owned();
        let tail = vec![
            lit("-DoutputFormats=XML"),
            lit("-DtimestampedReports=false"),
            lit("-DwithHistory=true"),
            lit(&format!("-DhistoryInputFile={history}")),
            lit(&format!("-DhistoryOutputFile={history}")),
            lit("-DfailWhenNoMutations=false"),
            Arg::Targets,
        ];
        tools.push(tool(
            PITEST,
            CheckKind::Mutation,
            build,
            args(
                false,
                &["test-compile", "org.pitest:pitest-maven:mutationCoverage"],
                UPSTREAM,
                tail,
            ),
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
        let b = build(BuildKind::Maven, Path::new("/r"), Features::default());
        let tools = tools(&b);
        let names: Vec<_> = tools.iter().map(|t| (t.name.as_str(), t.kind)).collect();
        assert_eq!(
            names,
            [(JAVAC, CheckKind::Typecheck), (SUREFIRE, CheckKind::Test)]
        );
        assert_eq!(tools[0].command.program, "prog");
        assert_eq!(tools[0].command.cwd, Path::new("/r"));
        assert_eq!(argv(&tools[0], &[]), ["-B", "-q", "-o", "test-compile"]);
        assert_eq!(
            argv(&tools[1], &["a.B"]),
            [
                "-B",
                "-o",
                "test",
                "-Dsurefire.failIfNoSpecifiedTests=false",
                "-DfailIfNoTests=false",
                "-Dtest=a.B"
            ]
        );
    }

    #[test]
    fn every_configured_tool_runs_in_order() {
        let b = build(BuildKind::Maven, Path::new("/r"), everything());
        let tools = tools(&b);
        let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            [SPOTLESS, CHECKSTYLE, PMD, SPOTBUGS, JAVAC, SUREFIRE, PITEST]
        );
        let kinds: Vec<_> = tools.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            [
                CheckKind::Format,
                CheckKind::Lint,
                CheckKind::Lint,
                CheckKind::Lint,
                CheckKind::Typecheck,
                CheckKind::Test,
                CheckKind::Mutation
            ]
        );
        assert_eq!(
            argv(by_name(&tools, SPOTLESS), &["src/main/java/a/B.java"]),
            [
                "-B",
                "-q",
                "-o",
                "spotless:apply",
                "-DspotlessFiles=.*/src/main/java/a/B.java"
            ]
        );
        assert_eq!(
            argv(by_name(&tools, CHECKSTYLE), &[]),
            ["-B", "-o", "checkstyle:checkstyle"]
        );
        assert_eq!(argv(by_name(&tools, PMD), &[]), ["-B", "-o", "pmd:pmd"]);

        assert_eq!(
            argv(by_name(&tools, SPOTBUGS), &[]),
            ["-B", "-o", "test-compile", "spotbugs:spotbugs"]
        );
        assert!(
            argv(by_name(&tools, SUREFIRE), &[])
                .contains(&"-DexcludedGroups=integration,slow,e2e".to_string())
        );
    }

    #[test]
    fn the_pit_command_scopes_and_records_history_in_the_module() {
        let b = build(BuildKind::Maven, Path::new("/r/m"), everything());
        let tools = tools(&b);
        let history = Path::new("/r/m")
            .join("target")
            .join("pit-history.bin")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            argv(
                by_name(&tools, PITEST),
                &["-DtargetClasses=a.B", "-DtargetTests=a.BTest"]
            ),
            [
                "-B".to_string(),
                "-o".into(),
                "test-compile".into(),
                "org.pitest:pitest-maven:mutationCoverage".into(),
                "-DoutputFormats=XML".into(),
                "-DtimestampedReports=false".into(),
                "-DwithHistory=true".into(),
                format!("-DhistoryInputFile={history}"),
                format!("-DhistoryOutputFile={history}"),
                "-DfailWhenNoMutations=false".into(),
                "-DtargetClasses=a.B".into(),
                "-DtargetTests=a.BTest".into(),
            ]
        );
    }

    #[test]
    fn reactor_modules_compile_their_siblings_before_a_report_goal() {
        let mut b = build(BuildKind::Maven, Path::new("/r"), everything());
        b.selector = Some("m".into());
        b.dependents = vec!["d1".into(), "d2".into()];
        let tools = tools(&b);
        assert_eq!(
            argv(by_name(&tools, CHECKSTYLE), &[]),
            [
                "-B",
                "-o",
                "test-compile",
                "checkstyle:checkstyle",
                "-pl",
                "m,d1,d2",
                "-am"
            ]
        );
        assert_eq!(
            argv(by_name(&tools, PMD), &[]),
            [
                "-B",
                "-o",
                "test-compile",
                "pmd:pmd",
                "-pl",
                "m,d1,d2",
                "-am"
            ]
        );
    }

    #[test]
    fn tags_are_only_excluded_for_junit_5() {
        let mut f = everything();
        f.junit5 = false;
        let b = build(BuildKind::Maven, Path::new("/r"), f);
        let tools = tools(&b);
        assert!(
            !argv(by_name(&tools, SUREFIRE), &[])
                .iter()
                .any(|a| a.starts_with("-DexcludedGroups"))
        );
    }

    #[test]
    fn modules_of_a_reactor_are_selected_with_their_neighbors() {
        let mut b = build(BuildKind::Maven, Path::new("/r"), everything());
        b.selector = Some("services/app".into());
        b.dependents = vec!["web".into()];
        let tools = tools(&b);
        let pl = ["-pl", "services/app,web"];
        let has = |name: &str, extra: &[&str]| {
            let v = argv(by_name(&tools, name), &[]);
            let at = v.iter().position(|a| a == "-pl").unwrap();
            assert_eq!(&v[at..at + 2], pl, "{name}");
            assert_eq!(
                v[at + 2..].iter().take(extra.len()).collect::<Vec<_>>(),
                extra,
                "{name}"
            );
            if extra.is_empty() {
                assert!(
                    !v.contains(&"-am".to_string()) && !v.contains(&"-amd".to_string()),
                    "{name}"
                );
            }
        };
        has(CHECKSTYLE, &["-am"]);
        has(PMD, &["-am"]);
        has(SPOTBUGS, &["-am"]);
        has(JAVAC, &["-am"]);
        for name in [CHECKSTYLE, PMD, SPOTBUGS, JAVAC] {
            assert!(
                !argv(by_name(&tools, name), &[]).contains(&"-amd".to_string()),
                "{name}: -amd misses the upstream of the dependents"
            );
        }
        // Spotless formats one file; the test and PIT runs stay with the module and its upstream.
        let spotless = argv(by_name(&tools, SPOTLESS), &[]);
        assert!(spotless.ends_with(&["-pl".into(), "services/app".into()]));
        for name in [SUREFIRE, PITEST] {
            let v = argv(by_name(&tools, name), &[]);
            let at = v.iter().position(|a| a == "-pl").unwrap();
            assert_eq!(&v[at..at + 3], ["-pl", "services/app", "-am"], "{name}");
        }
    }
}
