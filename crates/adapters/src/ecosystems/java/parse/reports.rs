//! The XML reports of Checkstyle, PMD, and SpotBugs.
//!
//! The tools run per module and report every violation of it, so the findings are not scoped
//! to the edited file; the use case's reporting filter (or the session baseline) decides what
//! shows. A report counts only when the build ran the tool (see [`super::Ctx::ran`]).
//!
//! | Tool       | Maven                      | Gradle                                          |
//! |------------|----------------------------|-------------------------------------------------|
//! | Checkstyle | `target/checkstyle-result.xml` | `build/reports/checkstyle/{main,test}.xml`  |
//! | PMD        | `target/pmd.xml`           | `build/reports/pmd/{main,test}.xml`             |
//! | SpotBugs   | `target/spotbugsXml.xml`   | `build/reports/spotbugs/main.xml`               |
//!
//! Severities: Checkstyle's `error` is an error and `warning` and `info` are warnings (`ignore`
//! is dropped). PMD priorities 1 and 2 are errors, the rest warnings. SpotBugs priority 1 (high)
//! is an error, the rest warnings.

use std::path::{Path, PathBuf};

use hookah_application::error::PortError;
use hookah_domain::finding::{Finding, Location, Severity};
use hookah_domain::toolchain::CheckKind;

use super::{Ctx, Root, javac, number, read, xml};
use crate::ecosystems::java::build::BuildKind;
use crate::ecosystems::java::detect::{CHECKSTYLE, PMD, SPOTBUGS};
use crate::ecosystems::java::xml::Node;

/// Checkstyle findings.
pub fn checkstyle(ctx: &Ctx<'_>) -> Result<Vec<Finding>, PortError> {
    let (files, goal, task) = match ctx.kind {
        BuildKind::Maven => (vec!["checkstyle-result.xml"], "checkstyle", ""),
        BuildKind::Gradle => (
            vec!["reports/checkstyle/main.xml", "reports/checkstyle/test.xml"],
            "",
            "checkstyleMain",
        ),
    };
    collect(ctx, CHECKSTYLE, goal, task, &files, |node, root| {
        checkstyle_xml(node, root)
    })
}

/// PMD findings.
pub fn pmd(ctx: &Ctx<'_>) -> Result<Vec<Finding>, PortError> {
    let (files, goal, task) = match ctx.kind {
        BuildKind::Maven => (vec!["pmd.xml"], "pmd", ""),
        BuildKind::Gradle => (
            vec!["reports/pmd/main.xml", "reports/pmd/test.xml"],
            "",
            "pmdMain",
        ),
    };
    collect(ctx, PMD, goal, task, &files, |node, root| {
        pmd_xml(node, root)
    })
}

/// SpotBugs findings.
pub fn spotbugs(ctx: &Ctx<'_>) -> Result<Vec<Finding>, PortError> {
    let (files, goal, task) = match ctx.kind {
        BuildKind::Maven => (vec!["spotbugsXml.xml"], "spotbugs", ""),
        BuildKind::Gradle => (vec!["reports/spotbugs/main.xml"], "", "spotbugsMain"),
    };
    collect(ctx, SPOTBUGS, goal, task, &files, |node, root| {
        spotbugs_xml(node, root)
    })
}

/// Reads the reports `files` (relative to the build directory) of a tool that ran, and turns each
/// into findings. A report that is missing is an error, unless Gradle had no sources to check or
/// another report of the same tool was read.
fn collect(
    ctx: &Ctx<'_>,
    tool: &str,
    goal: &str,
    task: &str,
    files: &[&str],
    parse: impl Fn(&Node, &Root) -> Vec<Finding>,
) -> Result<Vec<Finding>, PortError> {
    if !ctx.ran(goal, task) {
        // The build stopped before the tool: when the code does not compile, the compile check
        // reports it, and this one has nothing to add.
        let broken = !javac::scan(&ctx.root, &ctx.all(), CheckKind::Typecheck, "").is_empty();
        return if broken {
            Ok(vec![])
        } else {
            Err(ctx.failure(tool))
        };
    }
    let mut findings = Vec::new();
    let mut read_any = false;
    let mut last_error = None;
    for file in files {
        let path = ctx.build_dir().join(file);
        match read(&path, &format!("{tool} report")) {
            Ok(text) => {
                findings.extend(parse(&xml(&text, &format!("{tool} report"))?, &ctx.root));
                read_any = true;
            }
            Err(e) => last_error = Some(e),
        }
    }
    match (read_any, last_error) {
        (false, Some(_)) if ctx.no_source(task) => Ok(vec![]),
        (false, Some(e)) => Err(e),
        _ => Ok(findings),
    }
}

fn severity_of_checkstyle(name: &str) -> Option<Severity> {
    match name {
        "error" => Some(Severity::Error),
        "warning" | "info" => Some(Severity::Warning),
        _ => None,
    }
}

fn at(path: PathBuf, line: Option<u32>, column: Option<u32>) -> Location {
    match (line, column) {
        (Some(l), Some(c)) => Location::at(path, l, c),
        (Some(l), None) => Location::line(path, l),
        _ => Location::file(path),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn checkstyle_xml(report: &Node, root: &Root) -> Vec<Finding> {
    let mut out = Vec::new();
    for file in report.children_named("file") {
        let path = root.relative(file.attr("name").unwrap_or_default());
        for error in file.children_named("error") {
            let Some(severity) = severity_of_checkstyle(error.attr("severity").unwrap_or("error"))
            else {
                continue;
            };
            let location = at(
                path.clone(),
                number(error.attr("line")),
                number(error.attr("column")),
            );
            let message = one_line(error.attr("message").unwrap_or("checkstyle violation"));
            let mut finding = Finding::new(CheckKind::Lint, severity, location, message);
            if let Some(source) = error.attr("source") {
                let rule = source.rsplit('.').next().unwrap_or(source);
                finding = finding.with_rule(rule.strip_suffix("Check").unwrap_or(rule));
            }
            out.push(finding);
        }
    }
    out
}

fn pmd_xml(report: &Node, root: &Root) -> Vec<Finding> {
    let mut out = Vec::new();
    for file in report.children_named("file") {
        let path = root.relative(file.attr("name").unwrap_or_default());
        for v in file.children_named("violation") {
            let severity = match v.attr("priority").and_then(|p| p.trim().parse::<u8>().ok()) {
                Some(1 | 2) => Severity::Error,
                _ => Severity::Warning,
            };
            let location = at(
                path.clone(),
                number(v.attr("beginline")),
                number(v.attr("begincolumn")),
            );
            let mut finding = Finding::new(CheckKind::Lint, severity, location, one_line(&v.text));
            if let Some(rule) = v.attr("rule") {
                finding = finding.with_rule(rule);
            }
            out.push(finding);
        }
    }
    // Files PMD could not parse, and configuration problems.
    for error in report.children_named("error") {
        let path = root.relative(error.attr("filename").unwrap_or_default());
        let message = one_line(
            error
                .attr("msg")
                .unwrap_or("PMD could not process the file"),
        );
        out.push(Finding::new(
            CheckKind::Lint,
            Severity::Error,
            Location::file(path),
            message,
        ));
    }
    out
}

fn spotbugs_xml(report: &Node, root: &Root) -> Vec<Finding> {
    let mut out = Vec::new();
    for bug in report.children_named("BugInstance") {
        let Some(line) = primary_line(bug) else {
            continue;
        };
        let source = line.attr("sourcepath").unwrap_or_default();
        let path = root.existing(&[
            Path::new("src/main/java").join(source),
            Path::new("src/test/java").join(source),
            PathBuf::from(source),
        ]);
        let severity = if bug.attr("priority") == Some("1") {
            Severity::Error
        } else {
            Severity::Warning
        };
        let message = bug
            .child_text("LongMessage")
            .or_else(|| bug.child_text("ShortMessage"))
            .unwrap_or("SpotBugs finding");
        let mut finding = Finding::new(
            CheckKind::Lint,
            severity,
            at(path, number(line.attr("start")), None),
            one_line(message),
        );
        if let Some(kind) = bug.attr("type") {
            finding = finding.with_rule(kind);
        }
        out.push(finding);
    }
    out
}

/// The source line a bug is reported at: the bug's own primary line, else the primary method's,
/// else the class's.
fn primary_line(bug: &Node) -> Option<&Node> {
    let has_start = |n: &&Node| n.attr("start").is_some();
    bug.children_named("SourceLine")
        .find(|l| l.attr("primary") == Some("true"))
        .or_else(|| {
            bug.children_named("Method")
                .filter(|m| m.attr("primary") == Some("true"))
                .filter_map(|m| m.child("SourceLine"))
                .find(has_start)
        })
        .or_else(|| {
            bug.children_named("Class")
                .filter_map(|c| c.child("SourceLine"))
                .find(has_start)
        })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::testing::{ctx, out};
    use super::*;
    use crate::ecosystems::java::build::tests::Tree;
    use crate::ecosystems::java::xml::parse as parse_xml;

    /// Captured from maven-checkstyle-plugin 3.6.0 with Checkstyle 10.26.1.
    const CHECKSTYLE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<checkstyle version="10.26.1">
<file name="/ws/app/src/main/java/shop/Pricing.java">
<error line="3" column="8" severity="warning" message="Unused import - java.util.List." source="com.puppycrawl.tools.checkstyle.checks.imports.UnusedImportsCheck"/>
<error line="9" column="5" severity="warning" message="&apos;if&apos; construct must use &apos;{}&apos;s." source="com.puppycrawl.tools.checkstyle.checks.blocks.NeedBracesCheck"/>
<error line="9" column="55" severity="error" message="&apos;37&apos; is a magic number." source="com.puppycrawl.tools.checkstyle.checks.coding.MagicNumberCheck"/>
<error line="10" severity="info" message="Line too long" source="com.example.Custom"/>
<error line="11" severity="ignore" message="ignored" source="x.IgnoredCheck"/>
</file>
<file name="/ws/app/src/main/java/shop/Cart.java">
</file>
<file name="/ws/app/src/test/java/shop/CartTest.java">
<error severity="error" message="File-level problem"/>
</file>
</checkstyle>"#;

    /// Captured from maven-pmd-plugin 3.28.0 (PMD 7.17.0); the second file and the `error` element
    /// are what PMD writes for a file it cannot parse.
    const PMD_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<pmd xmlns="http://pmd.sourceforge.net/report/2.0.0" version="7.17.0" timestamp="2026-11-17T19:14:11.902">
<file name="/ws/app/src/main/java/shop/Pricing.java">
<violation beginline="8" endline="8" begincolumn="24" endcolumn="35" rule="UselessPureMethodCall" ruleset="Error Prone" package="shop" class="Pricing" method="discount" priority="3">
Do not call pure method length if the result is not used.
</violation>
<violation beginline="12" endline="12" begincolumn="5" endcolumn="9" rule="AvoidBranching" ruleset="X" priority="1">
  Blocker
    problem
</violation>
<violation beginline="13" begincolumn="5" rule="Second" ruleset="X" priority="2">high</violation>
<violation beginline="14" rule="NoPriority" ruleset="X">none</violation>
</file>
<error filename="/ws/app/src/main/java/shop/Broken.java" msg="ParseException: Encountered &quot;}&quot;"><![CDATA[stack]]></error>
</pmd>"#;

    /// Captured from spotbugs-maven-plugin 4.9.3.2 (SpotBugs 4.9.3), trimmed to one bug.
    const SPOTBUGS_XML: &str = r#"<BugCollection sequence='0' release='' version='4.9.3'><Project projectName='shop'><Jar>/ws/app/target/classes</Jar></Project><BugInstance instanceOccurrenceNum='0' cweid='476' rank='5' abbrev='NP' category='CORRECTNESS' priority='1' type='NP_ALWAYS_NULL'><ShortMessage>Null pointer dereference</ShortMessage><LongMessage>Null pointer dereference of s in shop.Pricing.discount(int, int)</LongMessage><Class classname='shop.Pricing' primary='true'><SourceLine classname='shop.Pricing' start='4' end='11' sourcepath='shop/Pricing.java' sourcefile='Pricing.java'><Message>At Pricing.java:[lines 4-11]</Message></SourceLine><Message>In class shop.Pricing</Message></Class><Method isStatic='true' classname='shop.Pricing' signature='(II)I' name='discount' primary='true'><SourceLine endBytecode='107' classname='shop.Pricing' start='7' end='11' sourcepath='shop/Pricing.java' sourcefile='Pricing.java' startBytecode='0'></SourceLine><Message>In method shop.Pricing.discount(int, int)</Message></Method><SourceLine role='SOURCE_LINE_DEREF' endBytecode='7' classname='shop.Pricing' start='9' end='9' sourcepath='shop/Pricing.java' sourcefile='Pricing.java' startBytecode='7' primary='true'><Message>Dereferenced at Pricing.java:[line 9]</Message></SourceLine></BugInstance><BugCategory category='CORRECTNESS'><Description>Correctness</Description></BugCategory></BugCollection>"#;

    fn root() -> Root {
        Root::new(&crate::ecosystems::testing::abs("/ws/app"))
    }

    fn show(found: &[Finding]) -> Vec<String> {
        found
            .iter()
            .map(|f| {
                format!(
                    "{} {:?} {} [{}]",
                    f.location,
                    f.severity,
                    f.message,
                    f.rule.as_deref().unwrap_or("-")
                )
            })
            .map(|line| line.replace('\\', "/"))
            .collect()
    }

    #[test]
    fn checkstyle_maps_severities_rules_and_locations() {
        let found = checkstyle_xml(
            &parse_xml(&crate::ecosystems::testing::rooted(CHECKSTYLE_XML)).unwrap(),
            &root(),
        );
        assert_eq!(
            show(&found),
            [
                "src/main/java/shop/Pricing.java:3:8 Warning Unused import - java.util.List. [UnusedImports]",
                "src/main/java/shop/Pricing.java:9:5 Warning 'if' construct must use '{}'s. [NeedBraces]",
                "src/main/java/shop/Pricing.java:9:55 Error '37' is a magic number. [MagicNumber]",
                "src/main/java/shop/Pricing.java:10 Warning Line too long [Custom]",
                "src/test/java/shop/CartTest.java Error File-level problem [-]",
            ]
        );
        assert!(found.iter().all(|f| f.kind == CheckKind::Lint));
    }

    #[test]
    fn pmd_maps_priorities_and_reports_unparseable_files() {
        let found = pmd_xml(
            &parse_xml(&crate::ecosystems::testing::rooted(PMD_XML)).unwrap(),
            &root(),
        );
        assert_eq!(
            show(&found),
            [
                "src/main/java/shop/Pricing.java:8:24 Warning Do not call pure method length if the result is not used. [UselessPureMethodCall]",
                "src/main/java/shop/Pricing.java:12:5 Error Blocker problem [AvoidBranching]",
                "src/main/java/shop/Pricing.java:13:5 Error high [Second]",
                "src/main/java/shop/Pricing.java:14 Warning none [NoPriority]",
                "src/main/java/shop/Broken.java Error ParseException: Encountered \"}\" [-]",
            ]
        );
    }

    #[test]
    fn spotbugs_uses_the_primary_line_and_resolves_the_source_path() {
        let t = Tree::new("spotbugs");
        t.write("src/main/java/shop/Pricing.java", "");
        let root = Root::new(&t.0);
        let found = spotbugs_xml(&parse_xml(SPOTBUGS_XML).unwrap(), &root);
        assert_eq!(
            show(&found),
            [
                "src/main/java/shop/Pricing.java:9 Error Null pointer dereference of s in shop.Pricing.discount(int, int) [NP_ALWAYS_NULL]"
            ]
        );
        // A source that is not found stays as SpotBugs spelled it.
        let found = spotbugs_xml(
            &parse_xml(SPOTBUGS_XML).unwrap(),
            &Root::new(&t.path("none")),
        );
        assert_eq!(
            found[0].location.path,
            Path::new("src/main/java/shop/Pricing.java")
        );
    }

    #[test]
    fn spotbugs_falls_back_from_the_bug_line_to_the_method_and_the_class() {
        let no_primary = SPOTBUGS_XML.replace(
            " primary='true'><Message>Dereferenced",
            "><Message>Dereferenced",
        );
        let found = spotbugs_xml(&parse_xml(&no_primary).unwrap(), &root());
        assert_eq!(found[0].location.line, Some(7));
        let no_method = no_primary.replace("name='discount' primary='true'", "name='discount'");
        let found = spotbugs_xml(&parse_xml(&no_method).unwrap(), &root());
        assert_eq!(found[0].location.line, Some(4));
        let none = r#"<BugCollection><BugInstance type='X' priority='2'><ShortMessage>short</ShortMessage></BugInstance></BugCollection>"#;
        assert!(spotbugs_xml(&parse_xml(none).unwrap(), &root()).is_empty());
        let low = SPOTBUGS_XML.replace("priority='1'", "priority='2'").replace(
            "<LongMessage>Null pointer dereference of s in shop.Pricing.discount(int, int)</LongMessage>",
            "",
        );
        let found = spotbugs_xml(&parse_xml(&low).unwrap(), &root());
        assert_eq!(found[0].severity, Severity::Warning);
        assert_eq!(found[0].message, "Null pointer dereference");
    }

    #[test]
    fn a_maven_tool_that_ran_is_read_from_target() {
        let t = Tree::new("rep-maven");
        t.write(
            "target/checkstyle-result.xml",
            &CHECKSTYLE_XML.replace("/ws/app", &t.0.to_string_lossy()),
        );
        let o = out(
            0,
            "[INFO] --- checkstyle:3.6.0:checkstyle (default-cli) @ shop ---\n",
            "",
        );
        let c = ctx(&t.0, BuildKind::Maven, None, &o);
        let found = checkstyle(&c).unwrap();
        assert_eq!(found.len(), 5);
        assert_eq!(
            found[0].location.path,
            Path::new("src/main/java/shop/Pricing.java")
        );
        // The report of an earlier run is not used when the tool did not run.
        let o = out(1, "[ERROR] Failed to execute goal compiler\n", "");
        let e = checkstyle(&ctx(&t.0, BuildKind::Maven, None, &o)).unwrap_err();
        assert!(e.message().contains("checkstyle exited with 1"));
        assert!(e.message().contains("Failed to execute goal compiler"));
        // ... unless the build stopped on a compile error, which the compile check reports.
        let o = out(
            1,
            "[ERROR] /ws/app/src/A.java:[3,5] cannot find symbol\n",
            "",
        );
        assert!(
            checkstyle(&ctx(&t.0, BuildKind::Maven, None, &o))
                .unwrap()
                .is_empty()
        );
        // Ran, but wrote nothing.
        let o = out(
            0,
            "[INFO] --- pmd:3.28.0:pmd (default-cli) @ shop ---\n",
            "",
        );
        let e = pmd(&ctx(&t.0, BuildKind::Maven, None, &o)).unwrap_err();
        assert!(e.message().contains("cannot read pmd report"));
    }

    #[test]
    fn gradle_reports_come_from_both_source_sets_and_no_source_is_clean() {
        let t = Tree::new("rep-gradle");
        let main = r#"<checkstyle><file name="/ws/app/A.java"><error line="1" severity="warning" message="m" source="x.YCheck"/></file></checkstyle>"#;
        t.write(
            "build/reports/checkstyle/main.xml",
            &main.replace("/ws/app", &t.0.to_string_lossy()),
        );
        let o = out(
            0,
            "> Task :checkstyleMain\n> Task :checkstyleTest NO-SOURCE\n",
            "",
        );
        let found = checkstyle(&ctx(&t.0, BuildKind::Gradle, None, &o)).unwrap();
        assert_eq!(found.len(), 1);
        t.write("build/reports/checkstyle/test.xml", "<checkstyle><file name=\"B.java\"><error line=\"2\" severity=\"error\" message=\"t\"/></file></checkstyle>");
        let found = checkstyle(&ctx(&t.0, BuildKind::Gradle, None, &o)).unwrap();
        assert_eq!(found.len(), 2);

        let none = Tree::new("rep-gradle-none");
        let o = out(0, "> Task :pmdMain NO-SOURCE\n", "");
        assert!(
            pmd(&ctx(&none.0, BuildKind::Gradle, None, &o))
                .unwrap()
                .is_empty()
        );
        let o = out(0, "> Task :pmdMain\n", "");
        assert!(pmd(&ctx(&none.0, BuildKind::Gradle, None, &o)).is_err());
        let o = out(1, "> Task :compileJava FAILED\n", "");
        assert!(pmd(&ctx(&none.0, BuildKind::Gradle, None, &o)).is_err());
    }

    #[test]
    fn spotbugs_reads_the_module_report_for_both_build_tools() {
        let t = Tree::new("rep-sb");
        t.write("target/spotbugsXml.xml", SPOTBUGS_XML);
        let o = out(
            0,
            "[INFO] --- spotbugs:4.9.3.2:spotbugs (default-cli) @ shop ---\n",
            "",
        );
        assert_eq!(
            spotbugs(&ctx(&t.0, BuildKind::Maven, None, &o))
                .unwrap()
                .len(),
            1
        );
        let g = Tree::new("rep-sb-gradle");
        g.write("build/reports/spotbugs/main.xml", SPOTBUGS_XML);
        let o = out(0, "> Task :app:spotbugsMain\n", "");
        assert_eq!(
            spotbugs(&ctx(&g.0, BuildKind::Gradle, Some(":app"), &o))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn a_broken_report_is_an_error() {
        let t = Tree::new("rep-broken");
        t.write("target/pmd.xml", "<pmd><file");
        let o = out(
            0,
            "[INFO] --- pmd:3.28.0:pmd (default-cli) @ shop ---\n",
            "",
        );
        let e = pmd(&ctx(&t.0, BuildKind::Maven, None, &o)).unwrap_err();
        assert!(e.message().contains("unreadable pmd report"));
    }

    #[test]
    fn locations_fall_back_to_the_file() {
        assert_eq!(at("a".into(), Some(2), Some(3)), Location::at("a", 2, 3));
        assert_eq!(at("a".into(), Some(2), None), Location::line("a", 2));
        assert_eq!(at("a".into(), None, Some(3)), Location::file("a"));
        assert_eq!(one_line("  a \n  b\tc "), "a b c");
    }
}
