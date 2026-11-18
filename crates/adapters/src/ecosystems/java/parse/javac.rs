//! javac diagnostics, as Maven and Gradle print them.
//!
//! - Maven: `[ERROR] /abs/Cart.java:[29,12] cannot find symbol`, each error twice (once in the
//!   `COMPILATION ERROR` list and once in the summary of the failed goal), so duplicates are
//!   dropped. `[WARNING]` lines are warnings.
//! - Gradle: `/abs/Cart.java:29: error: cannot find symbol`, then the source line and a caret
//!   line, which gives the column. The summary repeats each error indented, which is skipped.
//!
//! Both follow a diagnostic with `symbol:`, `location:`, `required:`, `found:`, and `reason:`
//! lines. They are folded into the message: `cannot find symbol: method totl() (location: class
//! Cart)`.

use std::collections::HashSet;

use hookah_application::error::PortError;
use hookah_domain::finding::{Finding, Location, Severity};
use hookah_domain::toolchain::CheckKind;

use super::{Ctx, Root, number};
use crate::ecosystems::java::detect::JAVAC;

const NOTES: [&str; 5] = ["symbol:", "location:", "required:", "found:", "reason:"];

/// Where the lines after a diagnostic are: Gradle prints the source line and a caret line before
/// the notes, Maven goes straight to them. Anything else ends the diagnostic, so the indented
/// repeat in Gradle's failure summary never feeds notes to the last real one.
#[derive(Clone, Copy)]
enum Stage {
    AfterDiagnostic,
    AfterSource,
    Open,
    Closed,
}

/// A diagnostic while its notes are still being collected.
struct Draft {
    severity: Severity,
    path: String,
    line: u32,
    column: Option<u32>,
    message: String,
    symbol: Option<String>,
    notes: Vec<String>,
}

/// The compile check: findings, or an error when the build failed without any diagnostic.
pub fn typecheck(ctx: &Ctx<'_>) -> Result<Vec<Finding>, PortError> {
    let found = scan(&ctx.root, &ctx.all(), CheckKind::Typecheck, "");
    if found.is_empty() && !ctx.out.success() {
        return Err(ctx.failure(JAVAC));
    }
    Ok(found)
}

/// Every javac diagnostic in `text` as a finding of `kind`, with `prefix` in front of the message.
pub fn scan(root: &Root, text: &str, kind: CheckKind, prefix: &str) -> Vec<Finding> {
    let mut drafts: Vec<Draft> = Vec::new();
    let mut stage = Stage::Closed;
    for line in text.lines() {
        if let Some(draft) = diagnostic(line) {
            drafts.push(draft);
            stage = Stage::AfterDiagnostic;
            continue;
        }
        let Some(last) = drafts.last_mut() else {
            continue;
        };
        let column = caret_column(line);
        let note = note(line);
        stage = match (stage, column, note) {
            (Stage::AfterSource, Some(c), _) => {
                last.column = last.column.or(Some(c));
                Stage::Open
            }
            (
                Stage::AfterDiagnostic | Stage::AfterSource | Stage::Open,
                _,
                Some((label, value)),
            ) => {
                if label == "symbol:" {
                    last.symbol.get_or_insert(value);
                } else {
                    last.notes.push(format!("{label} {value}"));
                }
                Stage::Open
            }
            (Stage::AfterDiagnostic, ..) if !line.trim().is_empty() => Stage::AfterSource,
            _ => Stage::Closed,
        };
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for d in drafts {
        let mut message = d.message;
        if let Some(symbol) = d.symbol {
            message = format!("{message}: {symbol}");
        }
        if !d.notes.is_empty() {
            message = format!("{message} ({})", d.notes.join("; "));
        }
        let path = root.relative(&d.path);
        if !seen.insert((path.clone(), d.line, d.column, message.clone())) {
            continue;
        }
        let location = match d.column {
            Some(c) => Location::at(path, d.line, c),
            None => Location::line(path, d.line),
        };
        out.push(Finding::new(
            kind,
            d.severity,
            location,
            format!("{prefix}{message}"),
        ));
    }
    out
}

/// One diagnostic line in either spelling; `None` for anything else, indented summary lines
/// included.
fn diagnostic(line: &str) -> Option<Draft> {
    let (severity, rest) = if let Some(r) = line.strip_prefix("[ERROR] ") {
        (Severity::Error, r)
    } else if let Some(r) = line.strip_prefix("[WARNING] ") {
        (Severity::Warning, r)
    } else {
        (Severity::Error, line)
    };
    let at = rest.find(".java:")?;
    let path = &rest[..at + ".java".len()];
    let tail = &rest[at + ".java:".len()..];
    if path.starts_with(char::is_whitespace) {
        return None;
    }
    if let Some(bracket) = tail.strip_prefix('[') {
        // Maven: `[line,col] message`.
        let (position, message) = bracket.split_once("] ")?;
        let (l, c) = position.split_once(',')?;
        return Some(Draft {
            severity,
            path: path.to_string(),
            line: number(Some(l))?,
            column: number(Some(c)),
            message: message.trim().to_string(),
            symbol: None,
            notes: vec![],
        });
    }
    // Gradle: `line: error: message`, or `line: warning: message`.
    let (l, after) = tail.split_once(": ")?;
    let (severity, message) = match after.strip_prefix("error: ") {
        Some(m) => (Severity::Error, m),
        None => (Severity::Warning, after.strip_prefix("warning: ")?),
    };
    Some(Draft {
        severity,
        path: path.to_string(),
        line: number(Some(l))?,
        column: None,
        message: message.trim().to_string(),
        symbol: None,
        notes: vec![],
    })
}

/// The column of javac's caret line (`    ^`): its position plus one. Only a line made of spaces
/// and one caret counts.
fn caret_column(line: &str) -> Option<u32> {
    let at = line.find('^')?;
    if line[..at].chars().all(|c| c == ' ' || c == '\t') && line[at + 1..].trim().is_empty() {
        u32::try_from(at + 1).ok()
    } else {
        None
    }
}

/// A `symbol:`/`location:`/... line, with an optional Maven prefix.
fn note(line: &str) -> Option<(&'static str, String)> {
    let line = line.strip_prefix("[ERROR] ").unwrap_or(line);
    let text = line.trim_start();
    if text.len() == line.len() {
        return None;
    }
    NOTES.iter().find_map(|label| {
        text.strip_prefix(label)
            .map(|v| (*label, v.trim().to_string()))
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use hookah_application::ports::ProcessOutput;

    use super::super::testing::{ctx, out};
    use super::*;
    use crate::ecosystems::java::build::BuildKind;
    use crate::ecosystems::testing::{abs, rooted};

    /// Captured from Maven 3.9 (`mvn -q -o compile`) on a call of a missing method and an int
    /// initialized with a string.
    const MAVEN: &str = r#"[ERROR] COMPILATION ERROR : 
[ERROR] /ws/app/src/main/java/shop/Cart.java:[29,12] cannot find symbol
  symbol:   method totl()
  location: class shop.Cart
[ERROR] /ws/app/src/main/java/shop/Cart.java:[29,50] incompatible types: java.lang.String cannot be converted to int
[ERROR] Failed to execute goal org.apache.maven.plugins:maven-compiler-plugin:3.15.0:compile (default-compile) on project shop: Compilation failure: Compilation failure: 
[ERROR] /ws/app/src/main/java/shop/Cart.java:[29,12] cannot find symbol
[ERROR]   symbol:   method totl()
[ERROR]   location: class shop.Cart
[ERROR] /ws/app/src/main/java/shop/Cart.java:[29,50] incompatible types: java.lang.String cannot be converted to int
[ERROR] -> [Help 1]
"#;

    /// Captured from Gradle 9.5 (`gradle --console=plain :compileJava`) on the same code.
    const GRADLE: &str = r#"
> Task :compileJava FAILED
/ws/app/src/main/java/shop/Cart.java:29: error: cannot find symbol
    return totl() >= FREE_SHIPPING_FROM; int x = "s";
           ^
  symbol:   method totl()
  location: class Cart
/ws/app/src/main/java/shop/Cart.java:29: error: incompatible types: String cannot be converted to int
    return totl() >= FREE_SHIPPING_FROM; int x = "s";
                                                 ^
2 errors

FAILURE: Build failed with an exception.

* What went wrong:
Execution failed for task ':compileJava'.
> Compilation failed; see the compiler output below.
  /ws/app/src/main/java/shop/Cart.java:29: error: incompatible types: String cannot be converted to int
      return totl() >= FREE_SHIPPING_FROM; int x = "s";
                                                   ^
  /ws/app/src/main/java/shop/Cart.java:29: error: cannot find symbol
      return totl() >= FREE_SHIPPING_FROM; int x = "s";
             ^
    symbol:   method totl()
    location: class Cart
  2 errors
"#;

    fn show(found: &[Finding]) -> Vec<String> {
        found
            .iter()
            .map(|f| format!("{} {:?} {}", f.location, f.severity, f.message))
            .collect()
    }

    fn run(kind: BuildKind, text: &str, code: i32) -> Result<Vec<Finding>, PortError> {
        let root = abs("/ws/app");
        let o: ProcessOutput = out(code, &rooted(text), "");
        typecheck(&ctx(&root, kind, None, &o))
    }

    #[test]
    fn maven_errors_are_deduplicated_and_carry_their_notes() {
        let found = run(BuildKind::Maven, MAVEN, 1).unwrap();
        assert_eq!(
            show(&found),
            [
                "src/main/java/shop/Cart.java:29:12 Error cannot find symbol: method totl() (location: class shop.Cart)",
                "src/main/java/shop/Cart.java:29:50 Error incompatible types: java.lang.String cannot be converted to int",
            ]
        );
        assert!(found.iter().all(|f| f.kind == CheckKind::Typecheck));
    }

    #[test]
    fn gradle_errors_take_their_column_from_the_caret_and_skip_the_indented_summary() {
        let found = run(BuildKind::Gradle, GRADLE, 1).unwrap();
        assert_eq!(
            show(&found),
            [
                "src/main/java/shop/Cart.java:29:12 Error cannot find symbol: method totl() (location: class Cart)",
                "src/main/java/shop/Cart.java:29:50 Error incompatible types: String cannot be converted to int",
            ]
        );
    }

    #[test]
    fn warnings_in_both_spellings() {
        let maven = "[WARNING] /ws/app/src/A.java:[3,1] [deprecation] f() has been deprecated\n";
        let found = run(BuildKind::Maven, maven, 0).unwrap();
        assert_eq!(found[0].severity, Severity::Warning);
        assert_eq!(found[0].location.line, Some(3));
        let gradle = "/ws/app/src/A.java:7: warning: [removal] g() has been deprecated and marked for removal\n   x();\n   ^\n";
        let found = run(BuildKind::Gradle, gradle, 0).unwrap();
        assert_eq!(found[0].severity, Severity::Warning);
        assert_eq!(found[0].location, Location::at("src/A.java", 7, 4));
    }

    #[test]
    fn clean_builds_and_unexplained_failures() {
        assert!(run(BuildKind::Maven, "", 0).unwrap().is_empty());
        let e = run(
            BuildKind::Maven,
            "[ERROR] Failed to execute goal X: Could not resolve dependencies\n",
            1,
        )
        .unwrap_err();
        assert!(e.message().contains("javac exited with 1"));
        assert!(e.message().contains("Could not resolve dependencies"));
        assert!(run(BuildKind::Gradle, "BUILD FAILED\n", 1).is_err());
        // Findings with a failing exit code are findings, not a failure.
        assert_eq!(run(BuildKind::Maven, MAVEN, 1).unwrap().len(), 2);
    }

    #[test]
    fn non_diagnostic_lines_are_ignored() {
        let text = "Note: Some input files use unchecked operations.\n\
                    /ws/app/Notes.java is mentioned here\n\
                    /ws/app/A.java:xx: error: not a number\n\
                    /ws/app/A.java:5: info: something else\n\
                    /ws/app/A.java:0: error: line zero\n\
                    [ERROR] /ws/app/A.java:[x,2] bad\n\
                    [ERROR] /ws/app/A.java:[4,2]no space\n\
                    [ERROR] /ws/app/A.java:[4] only a line\n\
                    \x20 /ws/app/B.java:9: error: indented summary\n";
        assert!(
            scan(
                &Root::new(&abs("/ws/app")),
                &rooted(text),
                CheckKind::Typecheck,
                ""
            )
            .is_empty()
        );
    }

    #[test]
    fn a_maven_error_without_a_column_and_paths_outside_the_module() {
        let text = "[ERROR] /ws/app/src/A.java:[8,0] msg\n[ERROR] /ws/other/src/B.java:[2,3] in a dependent\n";
        let found = run(BuildKind::Maven, text, 1).unwrap();
        assert_eq!(found[0].location, Location::line("src/A.java", 8));
        assert_eq!(found[1].location.path, abs("/ws/other/src/B.java"));
    }

    #[test]
    fn the_test_kind_and_prefix_are_applied() {
        let found = scan(
            &Root::new(&abs("/ws/app")),
            &rooted(MAVEN),
            CheckKind::Test,
            "compile error: ",
        );
        assert_eq!(found[1].kind, CheckKind::Test);
        assert!(
            found[1]
                .message
                .starts_with("compile error: incompatible types")
        );
    }

    #[test]
    fn caret_lines_and_notes() {
        assert_eq!(caret_column("   ^"), Some(4));
        assert_eq!(caret_column("\t^  "), Some(2));
        assert_eq!(caret_column("^"), Some(1));
        assert_eq!(caret_column("  a^"), None);
        assert_eq!(caret_column("  ^ x"), None);
        assert_eq!(caret_column("no caret"), None);
        assert_eq!(note("  symbol:   var x"), Some(("symbol:", "var x".into())));
        assert_eq!(note("[ERROR]   found: int"), Some(("found:", "int".into())));
        assert_eq!(note("symbol: not indented"), None);
        assert_eq!(note("  other: x"), None);
    }

    #[test]
    fn a_blank_line_ends_the_diagnostic() {
        let text = "/ws/app/A.java:3: error: first\n\n   ^\n  symbol: late\n";
        let found = run(BuildKind::Gradle, text, 1).unwrap();
        assert_eq!(found[0].location, Location::line("A.java", 3));
        assert_eq!(found[0].message, "first");
    }

    #[test]
    fn required_and_found_notes_are_kept() {
        let text = "/ws/app/A.java:3: error: method f in class A cannot be applied to given types;\n  f(1);\n  ^\n  required: String\n  found:    int\n  reason: argument mismatch\n";
        let found = run(BuildKind::Gradle, text, 1).unwrap();
        assert_eq!(
            found[0].message,
            "method f in class A cannot be applied to given types; (required: String; found: int; reason: argument mismatch)"
        );
    }
}
