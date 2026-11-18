//! Java output parsing.
//!
//! Each tool's result becomes findings whose paths are relative to the project root (the module
//! directory); a path outside it (a dependent module's source) stays absolute.
//!
//! - [`javac`]: compiler errors, in the Maven (`path:[line,col] message`) and the Gradle
//!   (`path:line: error: message`) spelling.
//! - [`reports`]: the XML reports of Checkstyle, PMD, and SpotBugs, read from the module's build
//!   directory.
//! - [`dynamic`]: surefire and Gradle test results (JUnit XML) and PIT's `mutations.xml`.
//!
//! The tools that write reports are trusted only when they ran: a report from an earlier run
//! stays on disk when the build fails before the tool starts, so the parsers look for the tool's
//! own line in the console output first (Maven's `--- checkstyle:3.6.0:checkstyle ---`, Gradle's
//! `> Task :checkstyleMain`).
//!
//! Every parser follows the fail-open rule: output it cannot understand, or a tool that fails
//! without reporting anything, is a [`PortError`] carrying a short tail of the output. It is never
//! read as a pass.

pub mod dynamic;
pub mod javac;
pub mod reports;

use std::path::{Path, PathBuf};

use hookah_application::error::PortError;
use hookah_application::ports::ProcessOutput;
use hookah_domain::finding::Finding;
use hookah_domain::path::is_driveless_root;
use hookah_domain::project::Project;
use hookah_domain::toolchain::Tool;

use super::build::{Build, BuildKind};
use super::detect::{CHECKSTYLE, GRADLE_TEST, JAVAC, PITEST, PMD, SPOTBUGS, SPOTLESS, SUREFIRE};

/// Parses the output of `tool`, run for `project`.
pub fn parse(
    project: &Project,
    tool: &Tool,
    out: &ProcessOutput,
) -> Result<Vec<Finding>, PortError> {
    let Some(build) = Build::load(&project.root) else {
        return Err(PortError::new("no Maven or Gradle build found"));
    };
    let ctx = Ctx {
        root: Root::new(&project.root),
        kind: build.kind,
        selector: build.selector.as_deref(),
        out,
    };
    match tool.name.as_str() {
        SPOTLESS => spotless(&ctx),
        JAVAC => javac::typecheck(&ctx),
        CHECKSTYLE => reports::checkstyle(&ctx),
        PMD => reports::pmd(&ctx),
        SPOTBUGS => reports::spotbugs(&ctx),
        SUREFIRE | GRADLE_TEST => dynamic::junit::parse(&ctx),
        PITEST => dynamic::pit::parse(&ctx),
        other => Err(PortError::new(format!("unknown Java tool `{other}`"))),
    }
}

/// What a parser needs: the module, the build tool, and the output.
pub struct Ctx<'a> {
    /// The module directory.
    pub root: Root,
    /// The build tool that ran.
    pub kind: BuildKind,
    /// Gradle: the project path (`:app`), `None` for the root project.
    pub selector: Option<&'a str>,
    /// What the command printed.
    pub out: &'a ProcessOutput,
}

impl Ctx<'_> {
    /// Standard output and standard error, one after the other.
    pub fn all(&self) -> String {
        format!("{}\n{}", self.out.stdout, self.out.stderr)
    }

    /// Whether the build ran the tool that writes the report: Maven's banner for goal `goal`
    /// (`--- checkstyle:3.6.0:checkstyle (default-cli) @ app ---`), Gradle's task line for `task`
    /// (`> Task :app:checkstyleMain`).
    pub fn ran(&self, goal: &str, task: &str) -> bool {
        let text = self.all();
        match self.kind {
            BuildKind::Maven => {
                let marker = format!(":{goal} (");
                text.lines()
                    .any(|l| l.contains("--- ") && l.contains(&marker))
            }
            BuildKind::Gradle => {
                let wanted = format!("{}:{task}", self.selector.unwrap_or_default());
                text.lines().any(|l| {
                    l.strip_prefix("> Task ")
                        .and_then(|t| t.split_whitespace().next())
                        == Some(wanted.as_str())
                })
            }
        }
    }

    /// Whether Gradle skipped the task for lack of sources (`> Task :checkstyleMain NO-SOURCE`).
    pub fn no_source(&self, task: &str) -> bool {
        let wanted = format!("{}:{task}", self.selector.unwrap_or_default());
        self.all().lines().any(|l| {
            let mut words = l
                .strip_prefix("> Task ")
                .unwrap_or_default()
                .split_whitespace();
            words.next() == Some(wanted.as_str()) && words.next() == Some("NO-SOURCE")
        })
    }

    /// The error for a tool that failed without a result we can use.
    pub fn failure(&self, tool: &str) -> PortError {
        failure(tool, self.out)
    }

    /// The module's build output directory: `target` for Maven, `build` for Gradle.
    pub fn build_dir(&self) -> PathBuf {
        self.root.path().join(match self.kind {
            BuildKind::Maven => "target",
            BuildKind::Gradle => "build",
        })
    }
}

/// The formatter. A file Spotless cannot parse is a syntax error that the compiler reports with
/// its own location, so it adds nothing here; any other failure is one.
fn spotless(ctx: &Ctx<'_>) -> Result<Vec<Finding>, PortError> {
    if ctx.out.success() {
        return Ok(vec![]);
    }
    let text = ctx.all();
    let syntax = text.contains("FormatterException")
        || (text.contains("found problem in") && text.contains(": error:"));
    if syntax {
        Ok(vec![])
    } else {
        Err(ctx.failure(SPOTLESS))
    }
}

/// The module directory, in the two spellings tools may report paths in.
#[derive(Debug, Clone)]
pub struct Root {
    path: PathBuf,
    canonical: PathBuf,
}

impl Root {
    /// Wraps a module directory.
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            canonical: dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
        }
    }

    /// The directory as given.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Makes a tool-reported `file` module-relative. A relative path is already relative to the
    /// module; an absolute path outside the module stays absolute.
    pub fn relative(&self, file: &str) -> PathBuf {
        let path = Path::new(file);
        if is_driveless_root(path) {
            return path.to_path_buf();
        }
        if !path.is_absolute() {
            return path.strip_prefix(".").unwrap_or(path).to_path_buf();
        }
        let canon = dunce::canonicalize(path).ok();
        for candidate in [Some(path.to_path_buf()), canon].into_iter().flatten() {
            for root in [&self.path, &self.canonical] {
                if let Ok(rel) = candidate.strip_prefix(root) {
                    return rel.to_path_buf();
                }
            }
        }
        path.to_path_buf()
    }

    /// The first of `candidates` (module-relative paths) that exists, else the first.
    pub fn existing(&self, candidates: &[PathBuf]) -> PathBuf {
        candidates
            .iter()
            .find(|c| self.path.join(c).is_file())
            .or(candidates.first())
            .cloned()
            .unwrap_or_default()
    }
}

/// The last few non-empty lines of `text`, joined for a one-line error message.
pub fn tail(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let start = lines.len().saturating_sub(3);
    let joined = lines[start..].join(" | ");
    if joined.is_empty() {
        "no output".into()
    } else {
        joined.chars().take(300).collect()
    }
}

/// The artifacts a build could not resolve because it runs offline and the local repository does
/// not have them, in the order they appear, each once.
///
/// - Maven: `Cannot access central (https://repo.maven.apache.org/maven2) in offline mode and the
///   artifact shop:shop-shared:jar:1.0.0 has not been downloaded from it before.`
/// - Gradle: `No cached version of org.x:y:1.0 available for offline mode.`
fn offline_artifacts(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in text.lines() {
        let artifact = line
            .split_once("and the artifact ")
            .filter(|_| line.contains("in offline mode"))
            .and_then(|(_, rest)| rest.split_once(" has not been downloaded"))
            .map(|(a, _)| a)
            .or_else(|| {
                line.split_once("No cached version of ")
                    .and_then(|(_, rest)| rest.split_once(" available for offline mode"))
                    .map(|(a, _)| a)
            });
        if let Some(a) = artifact
            && !found.iter().any(|f| f == a)
        {
            found.push(a.to_string());
        }
    }
    found
}

/// The one-line explanation of an offline resolution failure, or `None` for any other failure.
/// The cause is that hookah never downloads, so a dependency that the local repository lacks
/// stops the build; it is not a problem in the edited code.
fn offline_notice(text: &str) -> Option<String> {
    let artifacts = offline_artifacts(text);
    let first = artifacts.first()?;
    let more = match artifacts.len() {
        1 => String::new(),
        n => format!(" and {} more", n - 1),
    };
    let online_build = if text.contains("No cached version of ") {
        "`gradle build`"
    } else {
        "`mvn -q install -DskipTests`"
    };
    Some(format!(
        "{first}{more} is not in the local repository and hookah runs offline, so the build could not start; run the build once online ({online_build}), or set the environment of the build in `[env]` (docs/configuration.md)"
    ))
}

/// The error for a tool that failed without a result we can use: the exit status plus the
/// build's own error lines when it printed any, else the tail of stderr (or stdout). A build that
/// could not resolve a dependency offline gets the short explanation of that instead.
pub fn failure(tool: &str, out: &ProcessOutput) -> PortError {
    if let Some(notice) = offline_notice(&format!("{}\n{}", out.stdout, out.stderr)) {
        return PortError::new(notice);
    }
    let status = out
        .exit_code
        .map_or_else(|| "was killed".to_string(), |c| format!("exited with {c}"));
    let all = format!("{}\n{}", out.stdout, out.stderr);
    let errors: Vec<&str> = all
        .lines()
        .filter_map(|l| l.strip_prefix("[ERROR]"))
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("->") && !l.starts_with("[Help"))
        .collect();
    let text = if !errors.is_empty() {
        errors.join("\n")
    } else if out.stderr.trim().is_empty() {
        out.stdout.clone()
    } else {
        out.stderr.clone()
    };
    PortError::new(format!("{tool} {status}: {}", tail(&text)))
}

/// Reads the file at `path`, or says what is missing.
pub fn read(path: &Path, what: &str) -> Result<String, PortError> {
    std::fs::read_to_string(path)
        .map_err(|e| PortError::new(format!("cannot read {what} {}: {e}", path.display())))
}

/// Parses `text` as XML for the report called `what`.
pub fn xml(text: &str, what: &str) -> Result<super::xml::Node, PortError> {
    super::xml::parse(text).map_err(|e| PortError::new(format!("unreadable {what}: {e}")))
}

/// `line` and `column` attributes as numbers; a missing or zero value is `None`.
pub fn number(value: Option<&str>) -> Option<u32> {
    value?.trim().parse().ok().filter(|n| *n > 0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(super) mod testing {
    use super::*;

    pub fn out(code: i32, stdout: &str, stderr: &str) -> ProcessOutput {
        ProcessOutput {
            exit_code: Some(code),
            stdout: stdout.into(),
            stderr: stderr.into(),
            timed_out: false,
        }
    }

    pub fn ctx<'a>(
        root: &Path,
        kind: BuildKind,
        selector: Option<&'a str>,
        out: &'a ProcessOutput,
    ) -> Ctx<'a> {
        Ctx {
            root: Root::new(root),
            kind,
            selector,
            out,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use hookah_domain::toolchain::{CheckKind, CommandSpec};

    use super::testing::{ctx, out};
    use super::*;
    use crate::ecosystems::java::build::tests::Tree;
    use crate::ecosystems::testing::abs;

    /// Real output of `mvn -B -q -o test-compile -pl core -am -amd` on the `maven-reactor` sample,
    /// whose `app` needs `shared` (and its test-jar), which the local repository does not have.
    const MAVEN_OFFLINE: &str = "[ERROR] Failed to execute goal on project shop-app: Could not resolve dependencies for project shop:shop-app:jar:1.0.0
[ERROR] dependency: shop:shop-shared:jar:1.0.0 (compile)
[ERROR] \tCannot access central (https://repo.maven.apache.org/maven2) in offline mode and the artifact shop:shop-shared:jar:1.0.0 has not been downloaded from it before.
[ERROR] dependency: shop:shop-shared:jar:tests:1.0.0 (test)
[ERROR] \tCannot access central (https://repo.maven.apache.org/maven2) in offline mode and the artifact shop:shop-shared:jar:tests:1.0.0 has not been downloaded from it before.
[ERROR] -> [Help 1]
[ERROR] 
[ERROR] To see the full stack trace of the errors, re-run Maven with the -e switch.
[ERROR]   mvn <args> -rf :shop-app
";

    #[test]
    fn an_offline_resolution_failure_is_one_short_explanation() {
        let e = failure("javac", &out(1, MAVEN_OFFLINE, ""));
        let m = e.message();
        assert!(
            m.starts_with("shop:shop-shared:jar:1.0.0 and 1 more is not in the local repository")
        );
        assert!(m.contains("`mvn -q install -DskipTests`"));
        assert!(!m.contains("gradle"));
        assert!(m.contains("`[env]`"));
        assert!(!m.contains("[ERROR]"), "no wall of Maven text: {m}");
        // On stderr as well, and one artifact alone has no "and N more".
        let one = MAVEN_OFFLINE.lines().take(3).collect::<Vec<_>>().join("\n");
        let e = failure("surefire", &out(1, "", &one));
        assert!(
            e.message()
                .starts_with("shop:shop-shared:jar:1.0.0 is not in the local")
        );
    }

    #[test]
    fn gradle_offline_failures_are_recognised_too() {
        let text = "> Could not resolve org.x:y:1.0.\n     > No cached version of org.x:y:1.0 available for offline mode.\n\
                    > No cached version of org.x:y:1.0 available for offline mode.\n";
        assert_eq!(offline_artifacts(text), ["org.x:y:1.0"]);
        let notice = offline_notice(text).unwrap();
        assert!(notice.contains("`gradle build`"));
        assert!(!notice.contains("mvn"));
    }

    #[test]
    fn other_failures_are_not_offline_failures() {
        assert!(offline_notice("").is_none());
        // Online resolution failure and a stray mention are not the offline case.
        assert!(
            offline_notice(
                "Could not transfer artifact a:b:jar:1 from/to central: the artifact a:b:jar:1 has not been downloaded from it before"
            )
            .is_none()
        );
        assert!(offline_notice("[ERROR] COMPILATION ERROR :").is_none());
        let e = failure("javac", &out(1, "[ERROR] COMPILATION ERROR :\n", ""));
        assert!(e.message().starts_with("javac exited with 1"));
    }

    #[test]
    fn maven_banners_and_gradle_task_lines_say_a_tool_ran() {
        let root = abs("/ws/app");
        let o = out(
            0,
            "[INFO] --- checkstyle:3.6.0:checkstyle (default-cli) @ shop ---\n[INFO] BUILD SUCCESS\n",
            "",
        );
        let c = ctx(&root, BuildKind::Maven, None, &o);
        assert!(c.ran("checkstyle", "checkstyleMain"));
        assert!(!c.ran("pmd", "pmdMain"));
        // The goal name alone, outside a banner, is not enough.
        let o = out(1, "[ERROR] mentions :checkstyle (here)\n", "");
        assert!(!ctx(&root, BuildKind::Maven, None, &o).ran("checkstyle", ""));

        let o = out(
            0,
            "> Task :compileJava\n> Task :checkstyleMain UP-TO-DATE\n> Task :app:pmdMain FAILED\n",
            "",
        );
        let root_project = ctx(&root, BuildKind::Gradle, None, &o);
        assert!(root_project.ran("", "checkstyleMain"));
        assert!(!root_project.ran("", "pmdMain"), "another project's task");
        assert!(!root_project.ran("", "checkstyle"), "whole task names only");
        let app = ctx(&root, BuildKind::Gradle, Some(":app"), &o);
        assert!(app.ran("", "pmdMain"));
        assert!(!app.ran("", "checkstyleMain"));
    }

    #[test]
    fn no_source_is_a_task_state() {
        let root = abs("/ws/app");
        let o = out(0, "> Task :checkstyleMain NO-SOURCE\n> Task :pmdMain\n", "");
        let c = ctx(&root, BuildKind::Gradle, None, &o);
        assert!(c.no_source("checkstyleMain"));
        assert!(!c.no_source("pmdMain"));
        assert!(!c.no_source("other"));
        let o = out(0, "> Task :app:checkstyleMain NO-SOURCE\n", "");
        assert!(ctx(&root, BuildKind::Gradle, Some(":app"), &o).no_source("checkstyleMain"));
        assert!(!ctx(&root, BuildKind::Gradle, None, &o).no_source("checkstyleMain"));
    }

    #[test]
    fn the_build_directory_depends_on_the_build_tool() {
        let root = abs("/ws/app");
        let o = out(0, "", "");
        assert_eq!(
            ctx(&root, BuildKind::Maven, None, &o).build_dir(),
            root.join("target")
        );
        assert_eq!(
            ctx(&root, BuildKind::Gradle, None, &o).build_dir(),
            root.join("build")
        );
    }

    #[test]
    fn paths_become_module_relative() {
        let root = Root::new(&abs("/ws/app"));
        assert_eq!(
            root.relative(&abs("/ws/app/src/A.java").to_string_lossy()),
            Path::new("src/A.java")
        );
        assert_eq!(root.relative("./src/A.java"), Path::new("src/A.java"));
        assert_eq!(root.relative("src/A.java"), Path::new("src/A.java"));
        let outside = abs("/ws/other/B.java");
        assert_eq!(root.relative(&outside.to_string_lossy()), outside);
        assert_eq!(root.path(), abs("/ws/app"));
    }

    #[test]
    fn existing_prefers_a_real_file() {
        let t = Tree::new("parse-existing");
        t.write("src/test/java/A.java", "");
        let root = Root::new(&t.0);
        let main = PathBuf::from("src/main/java/A.java");
        let test = PathBuf::from("src/test/java/A.java");
        assert_eq!(root.existing(&[main.clone(), test.clone()]), test);
        assert_eq!(root.existing(&[test.clone(), main.clone()]), test);
        assert_eq!(root.existing(&[main.clone(), PathBuf::from("x")]), main);
        assert_eq!(root.existing(&[]), PathBuf::new());
    }

    #[test]
    fn tails_and_failures_name_what_went_wrong() {
        assert_eq!(tail(""), "no output");
        assert_eq!(tail("a\n\n b \nc\nd\n"), "b | c | d");
        assert_eq!(tail(&"x".repeat(400)).len(), 300);

        let e = failure("surefire", &out(1, "", "boom\n"));
        assert_eq!(e.message(), "surefire exited with 1: boom");
        let e = failure("surefire", &out(1, "only stdout\n", ""));
        assert_eq!(e.message(), "surefire exited with 1: only stdout");
        let killed = ProcessOutput::default();
        assert_eq!(failure("x", &killed).message(), "x was killed: no output");
        // Maven's own error lines beat the noise around them.
        let stdout = "[INFO] noise\n[ERROR] Failed to execute goal X: boom\n[ERROR] \n[ERROR] -> [Help 1]\n[ERROR] [Help 2] link\n[ERROR] For more information\n[INFO] tail\n";
        let e = failure("mvn", &out(1, stdout, ""));
        assert_eq!(
            e.message(),
            "mvn exited with 1: Failed to execute goal X: boom | [Help 2] link | For more information"
                .replace(" | [Help 2] link", "")
        );
    }

    #[test]
    fn numbers_must_be_positive() {
        assert_eq!(number(Some("12")), Some(12));
        assert_eq!(number(Some(" 3 ")), Some(3));
        assert_eq!(number(Some("0")), None);
        assert_eq!(number(Some("x")), None);
        assert_eq!(number(None), None);
    }

    #[test]
    fn spotless_ignores_syntax_errors_and_reports_other_failures() {
        let root = abs("/ws/app");
        let c = |o: &ProcessOutput| spotless(&ctx(&root, BuildKind::Maven, None, o));
        assert!(c(&out(0, "", "")).unwrap().is_empty());
        let maven = "[ERROR] Failed to execute goal com.diffplug.spotless:spotless-maven-plugin:2.44.5:apply (default-cli) on project shop: Unable to format file /ws/app/A.java: com.google.googlejavaformat.java.FormatterException: 10:17: error: ';' expected -> [Help 1]\n";
        assert!(c(&out(1, maven, "")).unwrap().is_empty());
        let gradle = "> Task :spotlessJava FAILED\nStep 'google-java-format' found problem in 'Pricing.java':\n2:32: error: illegal start of expression\n";
        assert!(c(&out(1, gradle, "")).unwrap().is_empty());
        let e = c(&out(1, "", "Plugin could not be resolved\n")).unwrap_err();
        assert!(e.message().contains("spotless exited with 1"));
        // "found problem in" without an error line is not a syntax error.
        assert!(c(&out(1, "found problem in 'A.java': style\n", "")).is_err());
    }

    #[test]
    fn dispatch_rejects_unknown_tools_and_missing_builds() {
        let t = Tree::new("parse-dispatch");
        let project = Project {
            root: t.0.clone(),
            ecosystem: hookah_domain::project::Ecosystem::Java,
            manifests: vec![],
        };
        let tool = Tool {
            name: "nope".into(),
            kind: CheckKind::Lint,
            command: CommandSpec::new("x", vec![], "/r", std::time::Duration::from_secs(1)),
        };
        let o = out(0, "", "");
        assert!(
            parse(&project, &tool, &o)
                .unwrap_err()
                .message()
                .contains("no Maven or Gradle")
        );
        t.write("pom.xml", "<project/>");
        assert!(
            parse(&project, &tool, &o)
                .unwrap_err()
                .message()
                .contains("unknown Java tool `nope`")
        );
    }
}
