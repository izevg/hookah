//! mypy output.
//!
//! Two formats are read, line by line, so one parser serves every mypy version:
//!
//! - JSON (`-O json`, mypy 1.11+): one object per line with `file`, `line`, `column` (0-based),
//!   `message`, `code`, and `severity`.
//! - Text: `path:line:col: error: message  [code]`, or `path: error: message` when there is no
//!   position (an unreadable file).
//!
//! `error` becomes `Error`, `warning` becomes `Warning`, and `note` lines are dropped. mypy also
//! checks the modules the edited file imports, so findings may belong to other files; they are
//! reported on purpose. Exit 0 is clean, 1 means errors were found, and 2 means mypy stopped
//! (a syntax error is reported as a finding with exit 2). A run that fails without any finding
//! is a [`PortError`].

use hookah_application::error::PortError;
use hookah_application::ports::ProcessOutput;
use hookah_domain::finding::{Finding, Location, Severity};
use hookah_domain::toolchain::CheckKind;
use serde::Deserialize;

use super::{Root, failure, parse_position};

#[derive(Deserialize)]
struct JsonLine {
    file: String,
    line: Option<i64>,
    column: Option<i64>,
    message: String,
    code: Option<String>,
    severity: String,
}

/// Findings of one mypy run.
pub fn parse(root: &Root, out: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    if out.timed_out || out.exit_code.is_none() {
        return Err(failure("mypy", out));
    }
    let findings: Vec<Finding> = out
        .stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('{') {
                json_line(root, line)
            } else {
                text_line(root, line)
            }
        })
        .collect();
    match out.exit_code {
        Some(0) => Ok(findings),
        _ if findings.is_empty() => Err(failure("mypy", out)),
        _ => Ok(findings),
    }
}

fn severity(name: &str) -> Option<Severity> {
    match name {
        "error" => Some(Severity::Error),
        "warning" => Some(Severity::Warning),
        _ => None,
    }
}

fn positive(n: Option<i64>) -> Option<u32> {
    n.filter(|n| *n > 0).and_then(|n| u32::try_from(n).ok())
}

fn location(root: &Root, path: &str, line: Option<u32>, column: Option<u32>) -> Location {
    let file = root.relative(path);
    match (line, column) {
        (Some(l), Some(c)) => Location::at(file, l, c),
        (Some(l), None) => Location::line(file, l),
        _ => Location::file(file),
    }
}

fn finding(severity: Severity, location: Location, message: &str, code: Option<&str>) -> Finding {
    let f = Finding::new(CheckKind::Typecheck, severity, location, message);
    match code {
        Some(code) => f.with_rule(code),
        None => f,
    }
}

fn json_line(root: &Root, line: &str) -> Option<Finding> {
    let j: JsonLine = serde_json::from_str(line).ok()?;
    let severity = severity(&j.severity)?;
    let line = positive(j.line);
    // JSON columns are 0-based; findings are 1-based.
    let column = j
        .column
        .filter(|c| *c >= 0)
        .and_then(|c| u32::try_from(c + 1).ok());
    let column = line.and(column);
    Some(finding(
        severity,
        location(root, &j.file, line, column),
        &j.message,
        j.code.as_deref(),
    ))
}

fn text_line(root: &Root, line: &str) -> Option<Finding> {
    // The earliest level marker ends the `path[:line[:col]]` prefix.
    let (at, level) = ["error", "warning", "note"]
        .iter()
        .filter_map(|l| line.find(&format!(": {l}: ")).map(|i| (i, *l)))
        .min_by_key(|(i, _)| *i)?;
    let severity = severity(level)?;
    let (path, line_no, column) = parse_position(&line[..at])?;
    let message = &line[at + level.len() + 4..];
    let (message, code) = split_code(message);
    Some(finding(
        severity,
        location(root, path, line_no, column),
        message,
        code,
    ))
}

/// Splits a trailing `  [code]` off a message.
fn split_code(message: &str) -> (&str, Option<&str>) {
    let message = message.trim_end();
    if let Some(body) = message.strip_suffix(']')
        && let Some(i) = body.rfind("  [")
    {
        let code = &body[i + 3..];
        if !code.is_empty() && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return (&body[..i], Some(code));
        }
    }
    (message, None)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::Path;

    use super::super::testing::out;
    use super::*;

    fn root() -> Root {
        Root::new(Path::new("/w/proj"))
    }

    /// Captured from mypy 2.4.1 (`-O json`).
    const JSON: &str = r#"{"file": "src/shop/domain/cart.py", "line": 6, "column": 13, "end_line": 6, "end_column": 16, "message": "Incompatible types in assignment (expression has type \"str\", variable has type \"int\")", "hint": null, "code": "assignment", "severity": "error"}
{"file": "src/shop/domain/cart.py", "line": 7, "column": 4, "end_line": 7, "end_column": 5, "message": "Revealed type is \"builtins.int\"", "hint": null, "code": null, "severity": "note"}
"#;

    /// Captured from mypy 2.4.1 (text, `--show-column-numbers`).
    const TEXT: &str = "src/shop/domain/cart.py:6:14: error: Incompatible types in assignment (expression has type \"str\", variable has type \"int\")  [assignment]\nsrc/shop/domain/cart.py:6:14: note: Some note\nsrc/shop/other.py:3:1: warning: Unused \"type: ignore\" comment  [unused-ignore]\n";

    #[test]
    fn parses_json_lines() {
        let f = parse(&root(), &out(1, JSON, "")).unwrap();
        assert_eq!(f.len(), 1, "notes are dropped");
        assert_eq!(
            f[0].location,
            Location::at("src/shop/domain/cart.py", 6, 14)
        );
        assert_eq!(f[0].rule.as_deref(), Some("assignment"));
        assert_eq!(f[0].severity, Severity::Error);
        assert_eq!(f[0].kind, CheckKind::Typecheck);
        assert!(f[0].message.starts_with("Incompatible types in assignment"));
    }

    #[test]
    fn parses_text_lines() {
        let f = parse(&root(), &out(1, TEXT, "")).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(
            f[0].location,
            Location::at("src/shop/domain/cart.py", 6, 14)
        );
        assert_eq!(f[0].rule.as_deref(), Some("assignment"));
        assert_eq!(
            f[0].message,
            "Incompatible types in assignment (expression has type \"str\", variable has type \"int\")"
        );
        assert_eq!(f[1].severity, Severity::Warning);
        assert_eq!(f[1].location.path, Path::new("src/shop/other.py"));
        assert_eq!(f[1].rule.as_deref(), Some("unused-ignore"));
    }

    #[test]
    fn text_without_columns_codes_or_positions() {
        let text = "a.py:4: error: Name \"x\" is not defined\nsrc/nonexist.py: error: Cannot read file: No such file or directory\n";
        let f = parse(&root(), &out(2, text, "")).unwrap();
        assert_eq!(f[0].location, Location::line("a.py", 4));
        assert_eq!(f[0].rule, None);
        assert_eq!(f[1].location, Location::file("src/nonexist.py"));
    }

    #[test]
    fn errors_in_other_files_are_kept() {
        let text = "pkg/other.py:9:1: error: Missing return statement  [return]\n";
        let f = parse(&root(), &out(1, text, "")).unwrap();
        assert_eq!(f[0].location.path, Path::new("pkg/other.py"));
    }

    #[test]
    fn syntax_error_exit_two_with_findings_is_a_result() {
        let text = "a.py:1: error: invalid syntax  [syntax]\n";
        let f = parse(&root(), &out(2, text, "")).unwrap();
        assert_eq!(f[0].rule.as_deref(), Some("syntax"));
    }

    #[test]
    fn message_with_brackets_is_not_a_code() {
        let text = "a.py:1:1: error: Incompatible types in assignment  [expected list[int]]\n";
        let f = parse(&root(), &out(1, text, "")).unwrap();
        assert_eq!(f[0].rule, None);
        assert!(f[0].message.ends_with("[expected list[int]]"));
    }

    #[test]
    fn clean_run_and_failures() {
        assert!(parse(&root(), &out(0, "", "")).unwrap().is_empty());
        let e = parse(
            &root(),
            &out(
                2,
                "",
                "usage: mypy [-h]\nmypy: error: unrecognized arguments: -O\n",
            ),
        )
        .unwrap_err();
        assert!(e.message().contains("unrecognized arguments"));
        assert!(parse(&root(), &out(1, "", "")).is_err());
        assert!(parse(&root(), &ProcessOutput::default()).is_err());
    }
}
