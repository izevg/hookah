//! PIT (pitest) results: `mutations.xml`.
//!
//! Maven writes `target/pit-reports/mutations.xml` (the command passes `-DtimestampedReports=false`
//! and `-DoutputFormats=XML`). Gradle writes `build/reports/pitest/mutations.xml`, or into a
//! time-stamped subdirectory when the project asks for those reports; the newest one is read.
//!
//! A `SURVIVED` mutant is an error finding at the source line, `mutant survived: <what PIT
//! changed>`, with the mutator as the rule. Mutants without coverage (`NO_COVERAGE`) are not
//! findings: the selected tests are a deliberate subset, so code they do not reach is expected to
//! be uncovered. This is the policy of the Node ecosystem too.

use std::path::{Path, PathBuf};

use hookah_application::error::PortError;
use hookah_domain::finding::{Finding, Location, Severity};
use hookah_domain::toolchain::CheckKind;

use crate::ecosystems::java::build::BuildKind;
use crate::ecosystems::java::detect::PITEST;
use crate::ecosystems::java::parse::{Ctx, Root, number, read, xml};
use crate::ecosystems::java::xml::Node;

/// Mutation findings of a PIT run.
pub fn parse(ctx: &Ctx<'_>) -> Result<Vec<Finding>, PortError> {
    // PIT fails when the selected classes hold nothing it can mutate; that is an empty result.
    if ctx.all().contains("No mutations found") {
        return Ok(vec![]);
    }
    let (goal, task) = match ctx.kind {
        BuildKind::Maven => ("mutationCoverage", ""),
        BuildKind::Gradle => ("", "pitest"),
    };
    if !ctx.ran(goal, task) {
        return Err(ctx.failure(PITEST));
    }
    let path = match ctx.kind {
        BuildKind::Maven => ctx.build_dir().join("pit-reports").join("mutations.xml"),
        BuildKind::Gradle => newest_report(&ctx.build_dir().join("reports").join("pitest"))
            .unwrap_or_else(|| ctx.build_dir().join("reports/pitest/mutations.xml")),
    };
    let report = xml(&read(&path, "PIT report")?, "PIT report")?;
    Ok(survivors(&report, &ctx.root))
}

/// The most recently written `mutations.xml` in `dir` or one of its subdirectories.
fn newest_report(dir: &Path) -> Option<PathBuf> {
    let mut candidates = vec![dir.join("mutations.xml")];
    if let Ok(entries) = std::fs::read_dir(dir) {
        candidates.extend(entries.flatten().map(|e| e.path().join("mutations.xml")));
    }
    candidates
        .into_iter()
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .max()
        .map(|(_, p)| p)
}

fn survivors(report: &Node, root: &Root) -> Vec<Finding> {
    report
        .children_named("mutation")
        .filter(|m| m.attr("status") == Some("SURVIVED"))
        .map(|m| {
            let class = m.child_text("mutatedClass").unwrap_or_default();
            let outer = class.split('$').next().unwrap_or(class);
            let package = outer.rsplit_once('.').map_or("", |(p, _)| p);
            let file = m.child_text("sourceFile").unwrap_or("Unknown.java");
            let mut path: PathBuf = ["src", "main", "java"].iter().collect();
            path.extend(package.split('.').filter(|p| !p.is_empty()));
            path.push(file);
            let path = root.existing(&[path]);
            let location = match number(m.child_text("lineNumber")) {
                Some(line) => Location::line(path, line),
                None => Location::file(path),
            };
            let what = m.child_text("description").unwrap_or("mutation");
            let mut finding = Finding::new(
                CheckKind::Mutation,
                Severity::Error,
                location,
                format!("mutant survived: {what}"),
            );
            if let Some(mutator) = m.child_text("mutator") {
                let short = mutator.rsplit('.').next().unwrap_or(mutator);
                finding = finding.with_rule(short.strip_suffix("Mutator").unwrap_or(short));
            }
            finding
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::ecosystems::java::build::tests::Tree;
    use crate::ecosystems::java::parse::testing::{ctx, out};
    use crate::ecosystems::java::xml::parse as parse_xml;

    /// Captured from pitest-maven 1.20.0 (`-DoutputFormats=XML`), trimmed to a survivor of each
    /// status and an inner class.
    const MUTATIONS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<mutations partial="true">
<mutation detected='false' status='SURVIVED' numberOfTestsRun='3'><sourceFile>Cart.java</sourceFile><mutatedClass>shop.Cart</mutatedClass><mutatedMethod>add</mutatedMethod><methodDescription>(I)V</methodDescription><lineNumber>12</lineNumber><mutator>org.pitest.mutationtest.engine.gregor.mutators.ConditionalsBoundaryMutator</mutator><indexes><index>4</index></indexes><blocks><block>0</block></blocks><killingTest/><description>changed conditional boundary</description></mutation>
<mutation detected='true' status='KILLED' numberOfTestsRun='1'><sourceFile>Cart.java</sourceFile><mutatedClass>shop.Cart</mutatedClass><mutatedMethod>add</mutatedMethod><methodDescription>(I)V</methodDescription><lineNumber>12</lineNumber><mutator>org.pitest.mutationtest.engine.gregor.mutators.NegateConditionalsMutator</mutator><indexes><index>4</index></indexes><blocks><block>0</block></blocks><killingTest>shop.CartTest.[engine:junit-jupiter]/[class:shop.CartTest]/[method:totalsItems()]</killingTest><description>negated conditional</description></mutation>
<mutation detected='false' status='NO_COVERAGE' numberOfTestsRun='0'><sourceFile>Pricing.java</sourceFile><mutatedClass>shop.Pricing</mutatedClass><mutatedMethod>discount</mutatedMethod><methodDescription>(II)I</methodDescription><lineNumber>8</lineNumber><mutator>org.pitest.mutationtest.engine.gregor.mutators.MathMutator</mutator><indexes><index>9</index></indexes><blocks><block>0</block></blocks><killingTest/><description>Replaced integer subtraction with addition</description></mutation>
<mutation detected='false' status='SURVIVED' numberOfTestsRun='2'><sourceFile>Cart.java</sourceFile><mutatedClass>shop.Cart$Item</mutatedClass><mutatedMethod>price</mutatedMethod><methodDescription>()I</methodDescription><lineNumber>31</lineNumber><mutator>org.pitest.mutationtest.engine.gregor.mutators.returns.PrimitiveReturnsMutator</mutator><indexes><index>2</index></indexes><blocks><block>0</block></blocks><killingTest/><description>replaced int return with 0 for shop/Cart$Item::price</description></mutation>
<mutation detected='false' status='SURVIVED'><sourceFile>Top.java</sourceFile><mutatedClass>Top</mutatedClass><description>no line, no package</description></mutation>
</mutations>"#;

    const MAVEN_BANNER: &str =
        "[INFO] --- pitest:1.20.0:mutationCoverage (default-cli) @ shop ---\n";

    fn show(found: &[Finding]) -> Vec<String> {
        found
            .iter()
            .map(|f| {
                format!(
                    "{} {} [{}]",
                    f.location,
                    f.message,
                    f.rule.as_deref().unwrap_or("-")
                )
            })
            .collect()
    }

    #[test]
    fn only_survivors_are_findings() {
        let root = Root::new(&crate::ecosystems::testing::abs("/ws/app"));
        let found = survivors(&parse_xml(MUTATIONS).unwrap(), &root);
        assert_eq!(
            show(&found),
            [
                "src/main/java/shop/Cart.java:12 mutant survived: changed conditional boundary [ConditionalsBoundary]",
                "src/main/java/shop/Cart.java:31 mutant survived: replaced int return with 0 for shop/Cart$Item::price [PrimitiveReturns]",
                "src/main/java/Top.java mutant survived: no line, no package [-]",
            ]
        );
        assert!(
            found
                .iter()
                .all(|f| f.kind == CheckKind::Mutation && f.severity == Severity::Error)
        );
    }

    #[test]
    fn maven_reads_the_untimestamped_report() {
        let t = Tree::new("pit-maven");
        t.write("target/pit-reports/mutations.xml", MUTATIONS);
        let o = out(0, MAVEN_BANNER, "");
        let found = parse(&ctx(&t.0, BuildKind::Maven, None, &o)).unwrap();
        assert_eq!(found.len(), 3);
    }

    #[test]
    fn gradle_reads_the_newest_report_even_in_a_timestamped_directory() {
        let t = Tree::new("pit-gradle");
        let o = out(0, "> Task :pitest\n", "");
        t.write(
            "build/reports/pitest/202611171910/mutations.xml",
            "<mutations/>",
        );
        let found = parse(&ctx(&t.0, BuildKind::Gradle, None, &o)).unwrap();
        assert!(found.is_empty());
        std::thread::sleep(std::time::Duration::from_millis(30));
        t.write("build/reports/pitest/202611171920/mutations.xml", MUTATIONS);
        assert_eq!(
            parse(&ctx(&t.0, BuildKind::Gradle, None, &o))
                .unwrap()
                .len(),
            3
        );
        // A report directly in the directory, written last, wins.
        std::thread::sleep(std::time::Duration::from_millis(30));
        t.write("build/reports/pitest/mutations.xml", "<mutations/>");
        assert!(
            parse(&ctx(&t.0, BuildKind::Gradle, None, &o))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn newest_report_ignores_directories_without_one() {
        let t = Tree::new("pit-newest");
        assert_eq!(newest_report(&t.path("missing")), None);
        t.write("r/a/index.html", "");
        assert_eq!(newest_report(&t.path("r")), None);
        t.write("r/b/mutations.xml", "<mutations/>");
        assert_eq!(
            newest_report(&t.path("r")),
            Some(t.path("r/b/mutations.xml"))
        );
    }

    #[test]
    fn nothing_to_mutate_is_an_empty_result() {
        let t = Tree::new("pit-none");
        let o = out(
            1,
            "[ERROR] ... failed: No mutations found. This probably means there is an issue with either the supplied classpath or filters.\n",
            "",
        );
        assert!(
            parse(&ctx(&t.0, BuildKind::Maven, None, &o))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn pit_that_did_not_run_or_wrote_nothing_is_an_error() {
        let t = Tree::new("pit-fail");
        let o = out(
            1,
            "[ERROR] Failed to execute goal compiler: Compilation failure\n",
            "",
        );
        let e = parse(&ctx(&t.0, BuildKind::Maven, None, &o)).unwrap_err();
        assert!(e.message().starts_with("pitest exited with 1"));
        let o = out(0, MAVEN_BANNER, "");
        let e = parse(&ctx(&t.0, BuildKind::Maven, None, &o)).unwrap_err();
        assert!(e.message().contains("cannot read PIT report"));
        let o = out(1, "> Task :pitest FAILED\n", "");
        assert!(parse(&ctx(&t.0, BuildKind::Gradle, None, &o)).is_err());
        t.write("target/pit-reports/mutations.xml", "<mutations");
        let o = out(0, MAVEN_BANNER, "");
        let e = parse(&ctx(&t.0, BuildKind::Maven, None, &o)).unwrap_err();
        assert!(e.message().contains("unreadable PIT report"));
    }
}
