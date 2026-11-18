//! Format and lint output: Biome, ESLint, and Prettier.
//!
//! - Biome (`--reporter=json`, stdout): `{"diagnostics": [...]}`. Biome 2 gives `message`,
//!   `location.path` (a string), and 1-based `start.line/column`. Biome 1.9 gives `description`,
//!   `location.path.file`, and a byte `span` into `location.sourceCode`, from which the line is
//!   computed. Severity `fatal` and `error` are errors, `warning` is a warning, `information` and
//!   `hint` are dropped. The rule is the category (`lint/style/useConst`). Lint keeps syntax
//!   errors (`parse`); format drops them and the "formatting aborted" follow-up, since lint
//!   already reports the syntax error.
//! - ESLint (`--format json`, stdout): an array of files with `messages`. Severity 2 is an error
//!   (also a fatal parse error), 1 a warning. "File ignored" notices are not findings.
//! - Prettier (`--write`): exit 0 is clean. `[error] path: SyntaxError: msg (line:col)` on
//!   stderr becomes an error finding; any other failure is a [`PortError`].

use std::path::Path;

use hookah_application::error::PortError;
use hookah_application::ports::ProcessOutput;
use hookah_domain::finding::{Finding, Location, Severity};
use hookah_domain::toolchain::CheckKind;
use serde::Deserialize;
use serde_json::Value;

use super::{first_line, json_in, relative, strip_ansi, unreadable};
use crate::ecosystems::node::detect::{BIOME, ESLINT, PRETTIER};

#[derive(Deserialize)]
struct BiomeReport {
    #[serde(default)]
    diagnostics: Vec<BiomeDiagnostic>,
}

#[derive(Deserialize)]
struct BiomeDiagnostic {
    category: Option<String>,
    severity: Option<String>,
    /// Biome 2: the text. Biome 1 puts rich text segments here instead.
    message: Option<Value>,
    /// Biome 1: the text.
    description: Option<String>,
    location: Option<BiomeLocation>,
}

#[derive(Deserialize)]
struct BiomeLocation {
    /// Biome 2: a string. Biome 1: `{"file": "..."}`.
    path: Option<Value>,
    start: Option<BiomePosition>,
    span: Option<[usize; 2]>,
    #[serde(rename = "sourceCode")]
    source_code: Option<String>,
}

#[derive(Deserialize)]
struct BiomePosition {
    line: u32,
    column: u32,
}

impl BiomeLocation {
    fn file(&self) -> Option<&str> {
        match self.path.as_ref()? {
            Value::String(s) => Some(s),
            Value::Object(o) => o.get("file")?.as_str(),
            _ => None,
        }
    }

    /// 1-based line and column; line 0 (Biome 2 for file-level notes) has no position.
    fn position(&self) -> Option<(u32, u32)> {
        if let Some(p) = &self.start {
            return (p.line > 0).then_some((p.line, p.column));
        }
        let (source, [offset, _]) = (self.source_code.as_deref()?, self.span?);
        let before = source.get(..offset)?;
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next()?.chars().count() + 1;
        Some((u32::try_from(line).ok()?, u32::try_from(column).ok()?))
    }
}

fn biome_diagnostics(output: &ProcessOutput) -> Result<Vec<BiomeDiagnostic>, PortError> {
    match json_in(&output.stdout) {
        Some(value) => serde_json::from_value::<BiomeReport>(value)
            .map(|r| r.diagnostics)
            .map_err(|e| PortError::new(format!("unreadable biome report: {e}"))),
        None if output.success() && output.stdout.trim().is_empty() => Ok(vec![]),
        None => Err(unreadable(BIOME, output)),
    }
}

fn biome_findings(
    root: &Path,
    diagnostics: Vec<BiomeDiagnostic>,
    kind: CheckKind,
    skip: &[&str],
) -> Vec<Finding> {
    diagnostics
        .into_iter()
        .filter_map(|d| {
            let category = d.category.as_deref().unwrap_or("");
            if skip.contains(&category) {
                return None;
            }
            let severity = match d.severity.as_deref()? {
                "fatal" | "error" => Severity::Error,
                "warning" => Severity::Warning,
                _ => return None,
            };
            let loc = d.location.as_ref()?;
            let path = relative(root, loc.file()?);
            let location = match loc.position() {
                Some((line, column)) => Location::at(path, line, column),
                None => Location::file(path),
            };
            let text = d
                .description
                .or_else(|| d.message.as_ref()?.as_str().map(str::to_owned))
                .unwrap_or_else(|| category.to_owned());
            let finding = Finding::new(kind, severity, location, first_line(&text));
            Some(if category.is_empty() {
                finding
            } else {
                finding.with_rule(category)
            })
        })
        .collect()
}

/// Biome lint findings, syntax errors included.
pub fn biome_lint(root: &Path, output: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    Ok(biome_findings(
        root,
        biome_diagnostics(output)?,
        CheckKind::Lint,
        &[],
    ))
}

/// Biome format findings: anything but syntax errors, which lint reports.
pub fn biome_format(root: &Path, output: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    Ok(biome_findings(
        root,
        biome_diagnostics(output)?,
        CheckKind::Format,
        &["parse", "format"],
    ))
}

#[derive(Deserialize)]
struct EslintFile {
    #[serde(rename = "filePath")]
    file_path: String,
    #[serde(default)]
    messages: Vec<EslintMessage>,
}

#[derive(Deserialize)]
struct EslintMessage {
    #[serde(rename = "ruleId")]
    rule_id: Option<String>,
    severity: u8,
    message: String,
    line: Option<u32>,
    column: Option<u32>,
}

/// ESLint findings. Without a JSON report the run failed (bad config, crash).
pub fn eslint(root: &Path, output: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    let Some(value) = json_in(&output.stdout) else {
        return Err(unreadable(ESLINT, output));
    };
    let files: Vec<EslintFile> = serde_json::from_value(value)
        .map_err(|e| PortError::new(format!("unreadable eslint report: {e}")))?;
    let mut findings = Vec::new();
    for file in files {
        let path = relative(root, &file.file_path);
        for m in file.messages {
            if m.rule_id.is_none() && m.message.starts_with("File ignored") {
                continue;
            }
            let severity = match m.severity {
                2 => Severity::Error,
                1 => Severity::Warning,
                _ => continue,
            };
            let location = match (m.line, m.column) {
                (Some(line), Some(column)) => Location::at(path.clone(), line, column),
                (Some(line), None) => Location::line(path.clone(), line),
                _ => Location::file(path.clone()),
            };
            let finding = Finding::new(CheckKind::Lint, severity, location, first_line(&m.message));
            findings.push(match m.rule_id {
                Some(rule) => finding.with_rule(rule),
                None => finding,
            });
        }
    }
    Ok(findings)
}

/// Prettier syntax errors. A clean run, or one that only formatted files, has none.
pub fn prettier(root: &Path, output: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    if output.success() {
        return Ok(vec![]);
    }
    let stderr = strip_ansi(&output.stderr);
    let findings: Vec<Finding> = stderr
        .lines()
        .filter_map(|l| l.strip_prefix("[error] "))
        .filter_map(|l| prettier_error(root, l))
        .collect();
    if findings.is_empty() {
        Err(unreadable(PRETTIER, output))
    } else {
        Ok(findings)
    }
}

/// `src/a.ts: SyntaxError: Expression expected. (1:12)`.
fn prettier_error(root: &Path, line: &str) -> Option<Finding> {
    let (path, rest) = line.split_once(": ")?;
    let (name, message) = rest.split_once(": ")?;
    if !name.ends_with("Error") || path.is_empty() {
        return None;
    }
    let path = relative(root, path);
    let located = message.strip_suffix(')').and_then(|m| {
        let (text, pos) = m.rsplit_once(" (")?;
        let (line, column) = pos.split_once(':')?;
        Some((text, line.parse().ok()?, column.parse().ok()?))
    });
    Some(match located {
        Some((text, line, column)) => Finding::new(
            CheckKind::Format,
            Severity::Error,
            Location::at(path, line, column),
            format!("{name}: {text}"),
        ),
        None => Finding::new(
            CheckKind::Format,
            Severity::Error,
            Location::file(path),
            format!("{name}: {message}"),
        ),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::ecosystems::testing::rooted;

    const ROOT: &str = if cfg!(windows) {
        "C:/ws/app"
    } else {
        "/ws/app"
    };

    fn out(exit: i32, stdout: &str, stderr: &str) -> ProcessOutput {
        ProcessOutput {
            exit_code: Some(exit),
            stdout: rooted(stdout),
            stderr: rooted(stderr),
            timed_out: false,
        }
    }

    fn at(path: &str, line: u32, col: u32) -> Location {
        Location::at(path, line, col)
    }

    /// `biome lint --reporter=json src/bad.ts` with Biome 2.6.8, trimmed.
    const BIOME2_LINT: &str = r#"The `json` and `json-pretty` reporters are experimental and may change in patch releases.
{"summary":{"changed":0,"unchanged":1,"matches":0,"errors":1,"warnings":2,"infos":0},"diagnostics":[{"severity":"warning","message":"This let declares a variable that is only assigned once.","category":"lint/style/useConst","location":{"path":"src/bad.ts","start":{"line":1,"column":1},"end":{"line":1,"column":4}},"advices":[{"start":{"line":1,"column":7},"end":{"line":1,"column":8},"text":"Safe fix: Use const instead."}]},{"severity":"error","message":"expected `)` but instead the file ends","category":"parse","location":{"path":"src/bad.ts","start":{"line":2,"column":1},"end":{"line":2,"column":1}},"advices":[]},{"severity":"information","message":"fyi","category":"lint/x","location":{"path":"src/bad.ts","start":{"line":3,"column":1},"end":{"line":3,"column":2}},"advices":[]}],"command":"lint"}
"#;

    /// `biome lint --reporter=json src/a.ts` with Biome 1.9.4: byte spans, advices trimmed.
    const BIOME1_LINT: &str = r#"{"summary":{"changed":0,"unchanged":1,"errors":2,"warnings":0},"diagnostics":[{"category":"lint/style/useConst","severity":"error","description":"This let declares a variable that is only assigned once.","message":[{"elements":[],"content":"This "},{"elements":["Emphasis"],"content":"let"},{"elements":[],"content":" declares a variable that is only assigned once."}],"advices":{"advices":[]},"verboseAdvices":{"advices":[]},"location":{"path":{"file":"src/a.ts"},"span":[54,57],"sourceCode":"export function add(a: number, b: number): number {\n  let unused = 1;\n  var x = 2\n  return a + b + x;\n}\nexport const s: string = 5;\n"},"tags":["fixable"],"source":null},{"category":"lint/style/noVar","severity":"error","description":"Use let or const instead of var.","message":[{"elements":[],"content":"Use let or const instead of var."}],"advices":{"advices":[]},"verboseAdvices":{"advices":[]},"location":{"path":{"file":"src/a.ts"},"span":[72,81],"sourceCode":"export function add(a: number, b: number): number {\n  let unused = 1;\n  var x = 2\n  return a + b + x;\n}\nexport const s: string = 5;\n"},"tags":["fixable"],"source":null}],"command":"lint"}
"#;

    /// `biome format --write --reporter=json src/bad.ts` with Biome 2.6.8: a syntax error.
    const BIOME2_FORMAT_ABORT: &str = r#"{"summary":{"changed":0,"unchanged":1,"errors":1},"diagnostics":[{"severity":"error","message":"expected `)` but instead the file ends","category":"parse","location":{"path":"src/bad.ts","start":{"line":2,"column":1},"end":{"line":2,"column":1}},"advices":[]},{"severity":"error","message":"Code formatting aborted due to parsing errors. To format code with errors, enable the 'formatter.formatWithErrors' option.","category":"format","location":{"path":"src/bad.ts","start":{"line":0,"column":0},"end":{"line":0,"column":0}},"advices":[]}],"command":"format"}
"#;

    #[test]
    fn biome2_lint() {
        let found = biome_lint(Path::new(ROOT), &out(1, BIOME2_LINT, "")).unwrap();
        assert_eq!(found.len(), 2, "information is dropped: {found:?}");
        assert_eq!(found[0].severity, Severity::Warning);
        assert_eq!(found[0].location, at("src/bad.ts", 1, 1));
        assert_eq!(
            found[0].message,
            "This let declares a variable that is only assigned once."
        );
        assert_eq!(found[0].rule.as_deref(), Some("lint/style/useConst"));
        assert_eq!(found[0].kind, CheckKind::Lint);
        assert_eq!(found[1].severity, Severity::Error);
        assert_eq!(found[1].rule.as_deref(), Some("parse"));
    }

    #[test]
    fn biome1_lint_computes_the_line_from_the_byte_span() {
        let found = biome_lint(Path::new(ROOT), &out(1, BIOME1_LINT, "")).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].location, at("src/a.ts", 2, 3));
        assert_eq!(found[0].severity, Severity::Error);
        assert_eq!(found[1].location, at("src/a.ts", 3, 3));
        assert_eq!(found[1].message, "Use let or const instead of var.");
        assert_eq!(found[1].rule.as_deref(), Some("lint/style/noVar"));
    }

    #[test]
    fn biome_format_leaves_syntax_errors_to_lint() {
        let found = biome_format(Path::new(ROOT), &out(1, BIOME2_FORMAT_ABORT, "")).unwrap();
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn biome_format_reports_other_diagnostics_without_a_position_for_line_zero() {
        let report = r#"{"diagnostics":[{"severity":"error","message":"cannot read the file","category":"internalError/io","location":{"path":"src/a.ts","start":{"line":0,"column":0},"end":{"line":0,"column":0}}}]}"#;
        let found = biome_format(Path::new(ROOT), &out(1, report, "")).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].location, Location::file("src/a.ts"));
        assert_eq!(found[0].kind, CheckKind::Format);
        assert_eq!(found[0].rule.as_deref(), Some("internalError/io"));
    }

    #[test]
    fn biome_clean_and_broken_runs() {
        let clean = r#"{"summary":{"changed":1},"diagnostics":[],"command":"format"}"#;
        assert!(
            biome_format(Path::new(ROOT), &out(0, clean, ""))
                .unwrap()
                .is_empty()
        );
        assert!(
            biome_lint(Path::new(ROOT), &out(0, "", ""))
                .unwrap()
                .is_empty()
        );
        let e = biome_lint(
            Path::new(ROOT),
            &out(1, "", "biome.json:1:1 deserialize ━━\n"),
        )
        .unwrap_err();
        assert!(e.message().starts_with("biome failed (exit 1)"));
        assert!(biome_lint(Path::new(ROOT), &out(1, "{\"diagnostics\": 5}", "")).is_err());
    }

    /// `eslint --format json src/c.js dist/i.js` with ESLint 9.x, `source` removed.
    const ESLINT_OUT: &str = r#"[{"filePath":"/ws/app/dist/i.js","messages":[{"ruleId":null,"fatal":false,"severity":1,"message":"File ignored because of a matching ignore pattern. Use \"--no-ignore\" to disable file ignore settings or use \"--no-warn-ignored\" to suppress this warning.","nodeType":null}],"suppressedMessages":[],"errorCount":0,"warningCount":1},{"filePath":"/ws/app/src/c.js","messages":[{"ruleId":"no-var","severity":2,"message":"Unexpected var, use let or const instead.","line":1,"column":1,"nodeType":"VariableDeclaration","messageId":"unexpectedVar","endLine":1,"endColumn":11,"fix":{"range":[0,3],"text":"let"}},{"ruleId":"prefer-const","severity":1,"message":"'y' is never reassigned. Use 'const' instead.","line":2,"column":5,"nodeType":"Identifier","messageId":"useConst","endLine":2,"endColumn":6}],"suppressedMessages":[],"errorCount":1,"warningCount":1},{"filePath":"/ws/app/src/d.ts","messages":[{"ruleId":null,"fatal":true,"severity":2,"message":"Parsing error: Unexpected token :","line":1,"column":22}],"suppressedMessages":[],"errorCount":1,"fatalErrorCount":1,"warningCount":0}]"#;

    #[test]
    fn eslint_report() {
        let found = eslint(Path::new(ROOT), &out(1, ESLINT_OUT, "")).unwrap();
        assert_eq!(found.len(), 3, "ignored-file notice is dropped: {found:?}");
        assert_eq!(found[0].location, at("src/c.js", 1, 1));
        assert_eq!(found[0].severity, Severity::Error);
        assert_eq!(found[0].rule.as_deref(), Some("no-var"));
        assert_eq!(found[1].severity, Severity::Warning);
        assert_eq!(found[1].location, at("src/c.js", 2, 5));
        assert_eq!(found[2].location, at("src/d.ts", 1, 22));
        assert_eq!(found[2].rule, None);
        assert_eq!(found[2].message, "Parsing error: Unexpected token :");
        assert!(found.iter().all(|f| f.kind == CheckKind::Lint));
    }

    #[test]
    fn eslint_message_with_a_line_but_no_column_is_located_at_the_line() {
        let report = r#"[{"filePath":"/ws/app/a.js","messages":[{"ruleId":"x","severity":2,"message":"m","line":7}]}]"#;
        let found = eslint(Path::new(ROOT), &out(1, report, "")).unwrap();
        assert_eq!(found[0].location, Location::line("a.js", 7));
    }

    #[test]
    fn eslint_without_a_report_is_an_error() {
        let e = eslint(
            Path::new(ROOT),
            &out(2, "", "Oops! Something went wrong! :(\nESLint: 9.0.0\n"),
        )
        .unwrap_err();
        assert!(e.message().contains("Something went wrong"));
        assert!(
            eslint(Path::new(ROOT), &out(0, "[]", ""))
                .unwrap()
                .is_empty()
        );
    }

    /// `prettier --write src/bad.ts`, Prettier 3.x and 2.x print the same.
    const PRETTIER_ERR: &str = "[error] src/bad.ts: SyntaxError: Expression expected. (1:12)\n[error] > 1 | let   y = (\n[error]     |            ^\n[error]   2 |\n";

    #[test]
    fn prettier_syntax_errors() {
        let found = prettier(Path::new(ROOT), &out(2, "", PRETTIER_ERR)).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].location, at("src/bad.ts", 1, 12));
        assert_eq!(found[0].message, "SyntaxError: Expression expected.");
        assert_eq!(found[0].severity, Severity::Error);
        assert_eq!(found[0].kind, CheckKind::Format);
    }

    #[test]
    fn prettier_ignores_lines_that_are_not_errors_or_have_no_path() {
        let root = Path::new(ROOT);
        assert!(prettier_error(root, "src/a.ts: Warning: odd (1:2)").is_none());
        assert!(prettier_error(root, ": SyntaxError: odd (1:2)").is_none());
        let f = prettier_error(root, "src/a.ts: SyntaxError: odd").unwrap();
        assert_eq!(f.location, Location::file("src/a.ts"));
    }

    #[test]
    fn prettier_clean_and_unreadable_failures() {
        assert!(
            prettier(Path::new(ROOT), &out(0, "src/a.ts 12ms\n", ""))
                .unwrap()
                .is_empty()
        );
        let e = prettier(
            Path::new(ROOT),
            &out(2, "", "[error] No parser could be inferred\n"),
        )
        .unwrap_err();
        assert!(e.message().contains("No parser could be inferred"));
    }
}
