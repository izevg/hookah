//! Test results from surefire (Maven) and Gradle: JUnit XML.
//!
//! - Maven writes `target/surefire-reports/TEST-<class>.xml` and never deletes old ones, so only
//!   the classes the console says were run (`[INFO] Running shop.CartTest`) are read.
//! - Gradle deletes the old results of a task and writes `build/test-results/test/TEST-*.xml`, so
//!   all of them belong to this run.
//!
//! A failed or errored test case is an error finding at the test source, on the line of the first
//! stack frame in the test class (the file when there is none): `CartTest.totalsItems failed:
//! expected: <1001> but was: <1000>`. A build that failed before any test ran because the tests
//! do not compile yields the compiler errors as test findings.

use std::path::PathBuf;

use hookah_application::error::PortError;
use hookah_domain::finding::{Finding, Location, Severity};
use hookah_domain::toolchain::CheckKind;

use crate::ecosystems::java::build::BuildKind;
use crate::ecosystems::java::detect::{GRADLE_TEST, SUREFIRE};
use crate::ecosystems::java::parse::{Ctx, Root, javac, number, read, xml};
use crate::ecosystems::java::xml::Node;

/// Test findings for a Maven or Gradle test run.
pub fn parse(ctx: &Ctx<'_>) -> Result<Vec<Finding>, PortError> {
    let (tool, files) = match ctx.kind {
        BuildKind::Maven => (SUREFIRE, maven_reports(ctx)),
        BuildKind::Gradle => (GRADLE_TEST, gradle_reports(ctx)),
    };
    let mut findings = Vec::new();
    for path in files {
        let Ok(text) = read(&path, "test report") else {
            continue;
        };
        findings.extend(junit(&xml(&text, "test report")?, &ctx.root));
    }
    if !findings.is_empty() || ctx.out.success() {
        return Ok(findings);
    }
    let all = ctx.all();
    // The tests did not compile: the compiler's errors are the findings.
    let compile = javac::scan(&ctx.root, &all, CheckKind::Test, "compile error: ");
    if !compile.is_empty() {
        return Ok(compile);
    }
    // A selection that matched nothing is not a failure of the tests.
    if all.contains("No tests found for given includes") {
        return Ok(vec![]);
    }
    Err(ctx.failure(tool))
}

fn maven_reports(ctx: &Ctx<'_>) -> Vec<PathBuf> {
    let dir = ctx.build_dir().join("surefire-reports");
    let mut classes: Vec<String> = ctx
        .all()
        .lines()
        .filter_map(|l| {
            let l = l
                .strip_prefix("[INFO] ")
                .or_else(|| l.strip_prefix("[ERROR] "))?;
            l.strip_prefix("Running ").map(|c| c.trim().to_string())
        })
        .collect();
    let mut seen = std::collections::HashSet::new();
    classes.retain(|c| seen.insert(c.clone()));
    classes
        .into_iter()
        .map(|c| dir.join(format!("TEST-{c}.xml")))
        .collect()
}

fn gradle_reports(ctx: &Ctx<'_>) -> Vec<PathBuf> {
    let dir = ctx.build_dir().join("test-results").join("test");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("TEST-") && n.ends_with(".xml"))
        })
        .collect();
    files.sort();
    files
}

/// The failed and errored test cases of a JUnit XML report (`<testsuite>` or `<testsuites>`).
fn junit(report: &Node, root: &Root) -> Vec<Finding> {
    let mut cases: Vec<&Node> = report
        .descendants()
        .into_iter()
        .filter(|n| n.name == "testcase")
        .collect();
    if report.name == "testcase" {
        cases.push(report);
    }
    cases
        .into_iter()
        .filter_map(|case| {
            let (problem, verb) = case
                .child("failure")
                .map(|f| (f, "failed"))
                .or_else(|| case.child("error").map(|e| (e, "errored")))?;
            Some(finding(case, problem, verb, root))
        })
        .collect()
}

fn finding(case: &Node, problem: &Node, verb: &str, root: &Root) -> Finding {
    let class = case.attr("classname").unwrap_or_default();
    let outer = class.split('$').next().unwrap_or(class);
    let simple = class.rsplit('.').next().unwrap_or(class);
    // JUnit 5 names methods `name()` in Gradle's reports and `name` in surefire's.
    let method = case.attr("name").unwrap_or("test");
    let method = method.strip_suffix("()").unwrap_or(method);

    let mut path: PathBuf = ["src", "test", "java"].iter().collect();
    path.extend(outer.split('.'));
    path.set_extension("java");
    let mut other = PathBuf::from("src/integrationTest/java");
    other.extend(outer.split('.'));
    other.set_extension("java");
    let path = root.existing(&[path, other]);

    let file = outer.rsplit('.').next().unwrap_or(outer);
    let line = frame_line(&problem.text, file);
    let location = match line {
        Some(l) => Location::line(path, l),
        None => Location::file(path),
    };

    let kind = problem.attr("type").unwrap_or_default();
    let message = problem
        .attr("message")
        .map(|m| m.trim_start_matches(&format!("{kind}: ")))
        .and_then(|m| m.lines().next())
        .unwrap_or_default()
        .trim();
    let text = match (verb, message.is_empty()) {
        ("errored", true) => kind.to_string(),
        ("errored", false) => format!("{kind}: {message}"),
        (_, true) => kind.to_string(),
        (_, false) => message.to_string(),
    };
    let text = if text.is_empty() {
        format!("{simple}.{method} {verb}")
    } else {
        format!("{simple}.{method} {verb}: {text}")
    };
    Finding::new(CheckKind::Test, Severity::Error, location, text)
}

/// The line of the first stack frame in `File.java`.
fn frame_line(stack: &str, file: &str) -> Option<u32> {
    let marker = format!("({file}.java:");
    stack.lines().find_map(|l| {
        let at = l.find(&marker)?;
        let rest = &l[at + marker.len()..];
        number(rest.split(')').next())
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::ecosystems::java::build::tests::Tree;
    use crate::ecosystems::java::parse::testing::{ctx, out};
    use crate::ecosystems::java::xml::parse as parse_xml;

    /// Captured from surefire 3.5.3 (JUnit Jupiter 5.12) with one failing assertion.
    const SUREFIRE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuite xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" version="3.0.2" name="shop.CartTest" time="0.018" tests="3" errors="0" skipped="0" failures="1">
  <properties>
  </properties>
  <testcase name="rejectsNegativePrices" classname="shop.CartTest" time="0.007"/>
  <testcase name="totalsItems" classname="shop.CartTest" time="0.003">
    <failure message="expected: &lt;1001&gt; but was: &lt;1000&gt;" type="org.opentest4j.AssertionFailedError"><![CDATA[org.opentest4j.AssertionFailedError: expected: <1001> but was: <1000>
	at org.junit.jupiter.api.Assertions.assertEquals(Assertions.java:531)
	at shop.CartTest.totalsItems(CartTest.java:16)
	at java.base/java.lang.reflect.Method.invoke(Method.java:580)
]]></failure>
  </testcase>
  <testcase name="freeShippingStartsAtFiftyUnits" classname="shop.CartTest" time="0.0"/>
</testsuite>"#;

    /// Captured from Gradle 9.5 for the same failure; the message repeats the exception type.
    const GRADLE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuite name="shop.CartTest" tests="3" skipped="0" failures="1" errors="0" timestamp="2026-11-18T03:24:21.900Z" hostname="h" time="0.136">
  <properties/>
  <testcase name="rejectsNegativePrices()" classname="shop.CartTest" time="0.083"/>
  <testcase name="totalsItems()" classname="shop.CartTest" time="0.009">
    <failure message="org.opentest4j.AssertionFailedError: expected: &lt;1001&gt; but was: &lt;1000&gt;" type="org.opentest4j.AssertionFailedError">org.opentest4j.AssertionFailedError: expected: &lt;1001&gt; but was: &lt;1000&gt;
	at org.junit.jupiter.api.Assertions.assertEquals(Assertions.java:531)
	at shop.CartTest.totalsItems(CartTest.java:16)
</failure>
  </testcase>
  <system-out><![CDATA[]]></system-out>
</testsuite>"#;

    fn root() -> Root {
        Root::new(&crate::ecosystems::testing::abs("/ws/app"))
    }

    fn show(found: &[Finding]) -> Vec<String> {
        found
            .iter()
            .map(|f| format!("{} {}", f.location, f.message))
            .map(|line| line.replace('\\', "/"))
            .collect()
    }

    #[test]
    fn a_failed_assertion_is_an_error_at_the_test_line() {
        for xml_text in [SUREFIRE_XML, GRADLE_XML] {
            let found = junit(&parse_xml(xml_text).unwrap(), &root());
            assert_eq!(
                show(&found),
                [
                    "src/test/java/shop/CartTest.java:16 CartTest.totalsItems failed: expected: <1001> but was: <1000>"
                ]
            );
            assert_eq!(found[0].severity, Severity::Error);
            assert_eq!(found[0].kind, CheckKind::Test);
        }
    }

    #[test]
    fn errors_name_the_exception_and_nested_classes_map_to_the_outer_file() {
        let text = r#"<testsuites><testsuite name="x"><testcase name="boom" classname="shop.CartTest$Inner">
<error message="bad state" type="java.lang.IllegalStateException">java.lang.IllegalStateException: bad state
	at shop.Cart.add(Cart.java:12)
	at shop.CartTest$Inner.boom(CartTest.java:44)
</error></testcase>
<testcase name="npe" classname="shop.CartTest"><error type="java.lang.NullPointerException"/></testcase>
<testcase name="plain" classname="shop.CartTest"><failure/></testcase>
<testcase name="skipped" classname="shop.CartTest"><skipped/></testcase>
</testsuite></testsuites>"#;
        let found = junit(&parse_xml(text).unwrap(), &root());
        assert_eq!(
            show(&found),
            [
                "src/test/java/shop/CartTest.java:44 CartTest$Inner.boom errored: java.lang.IllegalStateException: bad state",
                "src/test/java/shop/CartTest.java CartTest.npe errored: java.lang.NullPointerException",
                "src/test/java/shop/CartTest.java CartTest.plain failed",
            ]
        );
    }

    #[test]
    fn a_lone_testcase_root_and_multiline_messages() {
        let text = r#"<testcase name="t" classname="a.B"><failure message="first&#10;second" type="E"/></testcase>"#;
        let found = junit(&parse_xml(text).unwrap(), &root());
        assert_eq!(show(&found), ["src/test/java/a/B.java B.t failed: first"]);
    }

    #[test]
    fn integration_test_sources_are_found_when_they_exist() {
        let t = Tree::new("tests-it");
        t.write("src/integrationTest/java/a/B.java", "");
        let text =
            r#"<testcase name="t" classname="a.B"><failure message="m" type="E"/></testcase>"#;
        let found = junit(&parse_xml(text).unwrap(), &Root::new(&t.0));
        assert_eq!(
            found[0].location.path,
            std::path::Path::new("src/integrationTest/java/a/B.java")
        );
    }

    #[test]
    fn frames_must_be_in_the_test_file() {
        let stack =
            "x\n\tat a.Other.m(Other.java:3)\n\tat a.B.t(B.java:9)\n\tat a.B.u(B.java:10)\n";
        assert_eq!(frame_line(stack, "B"), Some(9));
        assert_eq!(frame_line(stack, "Other"), Some(3));
        assert_eq!(frame_line(stack, "C"), None);
        assert_eq!(frame_line("at a.B.t(B.java:x)", "B"), None);
        assert_eq!(frame_line("", "B"), None);
    }

    #[test]
    fn maven_reads_the_reports_of_the_classes_that_ran() {
        let t = Tree::new("tests-maven");
        t.write(
            "target/surefire-reports/TEST-shop.CartTest.xml",
            SUREFIRE_XML,
        );
        // A stale report of a class that did not run this time.
        t.write(
            "target/surefire-reports/TEST-shop.OldTest.xml",
            SUREFIRE_XML,
        );
        let stdout = "[INFO] Running shop.CartTest\n[ERROR] Tests run: 3, Failures: 1 <<< FAILURE! -- in shop.CartTest\n[INFO] Running shop.MissingTest\n[INFO] Running shop.CartTest\n";
        let o = out(1, stdout, "");
        let found = parse(&ctx(&t.0, BuildKind::Maven, None, &o)).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].location,
            Location::line("src/test/java/shop/CartTest.java", 16)
        );
    }

    #[test]
    fn gradle_reads_every_report_of_the_run() {
        let t = Tree::new("tests-gradle");
        t.write("build/test-results/test/TEST-shop.CartTest.xml", GRADLE_XML);
        t.write(
            "build/test-results/test/TEST-shop.Other.xml",
            &GRADLE_XML.replace("shop.CartTest", "shop.Other"),
        );
        t.write("build/test-results/test/binary/results.bin", "x");
        t.write("build/test-results/test/notes.xml", "<broken");
        let o = out(1, "> Task :test FAILED\n", "");
        let found = parse(&ctx(&t.0, BuildKind::Gradle, None, &o)).unwrap();
        assert_eq!(found.len(), 2);
        assert!(found[0].message.starts_with("CartTest."));
        assert!(found[1].message.starts_with("Other."));
    }

    #[test]
    fn passing_runs_are_clean_even_without_reports() {
        let t = Tree::new("tests-pass");
        let o = out(0, "[INFO] Tests run: 3, Failures: 0\n", "");
        assert!(
            parse(&ctx(&t.0, BuildKind::Maven, None, &o))
                .unwrap()
                .is_empty()
        );
        assert!(
            parse(&ctx(&t.0, BuildKind::Gradle, None, &o))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn tests_that_do_not_compile_give_compiler_findings() {
        let t = Tree::new("tests-compile");
        let stdout = "[ERROR] COMPILATION ERROR : \n[ERROR] /ws/app/src/test/java/shop/CartTest.java:[7,9] cannot find symbol\n";
        let o = out(1, &crate::ecosystems::testing::rooted(stdout), "");
        let root = crate::ecosystems::testing::abs("/ws/app");
        let found = parse(&ctx(&root, BuildKind::Maven, None, &o)).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, CheckKind::Test);
        assert!(
            found[0]
                .message
                .starts_with("compile error: cannot find symbol")
        );
        drop(t);
    }

    #[test]
    fn an_empty_gradle_selection_is_not_a_failure_but_other_failures_are() {
        let t = Tree::new("tests-none");
        let o = out(
            1,
            "> No tests found for given includes: [shop.PricingTest](--tests filter)\n",
            "",
        );
        assert!(
            parse(&ctx(&t.0, BuildKind::Gradle, None, &o))
                .unwrap()
                .is_empty()
        );
        let o = out(
            1,
            "[ERROR] Failed to execute goal surefire: could not resolve\n",
            "",
        );
        let e = parse(&ctx(&t.0, BuildKind::Maven, None, &o)).unwrap_err();
        assert!(e.message().starts_with("surefire exited with 1"));
        let o = out(1, "BUILD FAILED\n", "");
        let e = parse(&ctx(&t.0, BuildKind::Gradle, None, &o)).unwrap_err();
        assert!(e.message().starts_with("gradle-test exited with 1"));
    }

    #[test]
    fn a_broken_report_of_a_class_that_ran_is_an_error() {
        let t = Tree::new("tests-broken");
        t.write(
            "target/surefire-reports/TEST-shop.CartTest.xml",
            "<testsuite><testcase",
        );
        let o = out(1, "[INFO] Running shop.CartTest\n", "");
        let e = parse(&ctx(&t.0, BuildKind::Maven, None, &o)).unwrap_err();
        assert!(e.message().contains("unreadable test report"));
    }
}
