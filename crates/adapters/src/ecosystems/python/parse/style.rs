//! Format and lint output: ruff and black.
//!
//! - [`ruff_check`]: the JSON array of `ruff check --output-format json`. Exit 0 and 1 are normal
//!   (clean, violations); anything else is a failure. Ruff has no warnings by default, so every
//!   diagnostic is an `Error`, and its code (`F401`) is the rule. Syntax errors have the code
//!   `invalid-syntax` (older versions: none).
//! - [`ruff_format`]: exit 0 is clean, also when files were rewritten. A file ruff cannot parse
//!   gives `error: Failed to parse path:line:col: message` on stderr, which becomes an `Error`.
//! - [`black`]: exit 0 is clean. A file black cannot parse gives `error: cannot parse:
//!   path:line:col` (newer) or `error: cannot format path: Cannot parse: line:col: text` (older),
//!   exit 123.

use hookah_application::error::PortError;
use hookah_application::ports::ProcessOutput;
use hookah_domain::finding::{Finding, Location, Severity};
use hookah_domain::toolchain::CheckKind;
use serde::Deserialize;

use super::{Root, failure, parse_position};

#[derive(Deserialize)]
struct Diagnostic {
    code: Option<String>,
    message: String,
    filename: String,
    location: Position,
}

#[derive(Deserialize)]
struct Position {
    row: u32,
    column: u32,
}

/// Findings of `ruff check --output-format json`.
pub fn ruff_check(root: &Root, out: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    if out.timed_out || !matches!(out.exit_code, Some(0 | 1)) {
        return Err(failure("ruff", out));
    }
    let text = out.stdout.trim();
    if text.is_empty() {
        return if out.exit_code == Some(0) {
            Ok(vec![])
        } else {
            Err(failure("ruff", out))
        };
    }
    let diagnostics: Vec<Diagnostic> = serde_json::from_str(text)
        .map_err(|e| PortError::new(format!("unreadable ruff output: {e}")))?;
    Ok(diagnostics
        .into_iter()
        .map(|d| {
            let location = Location::at(
                root.relative(&d.filename),
                d.location.row,
                d.location.column,
            );
            let finding = Finding::new(CheckKind::Lint, Severity::Error, location, d.message);
            match d.code {
                Some(code) => finding.with_rule(code),
                None => finding,
            }
        })
        .collect())
}

/// Findings of `ruff format`: files it could not parse.
pub fn ruff_format(root: &Root, out: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    if out.success() {
        return Ok(vec![]);
    }
    let findings: Vec<Finding> = out
        .stderr
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("error: Failed to parse ")?;
            let (position, message) = rest.split_once(": ")?;
            let (path, line, column) = parse_position(position)?;
            Some(format_error(root, path, line, column, message))
        })
        .collect();
    if findings.is_empty() {
        Err(failure("ruff format", out))
    } else {
        Ok(findings)
    }
}

/// Findings of `black`: files it could not parse.
pub fn black(root: &Root, out: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    if out.success() {
        return Ok(vec![]);
    }
    let lines: Vec<&str> = out.stderr.lines().map(str::trim).collect();
    let mut findings = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if let Some(rest) = line.strip_prefix("error: cannot parse: ") {
            // `error: cannot parse: path:line:col`, then the source line, a caret, and the reason.
            if let Some((path, l, c)) = parse_position(rest) {
                // The `error:` line itself never starts with `ParseError: `.
                let reason = lines[i..]
                    .iter()
                    .find_map(|l| l.strip_prefix("ParseError: "))
                    .unwrap_or("cannot parse");
                findings.push(format_error(root, path, l, c, reason));
            }
        } else if let Some(rest) = line.strip_prefix("error: cannot format ") {
            // `error: cannot format path: Cannot parse: line:col: text`
            if let Some((path, tail)) = rest.split_once(": Cannot parse") {
                let tail = tail.trim_start_matches(" for target version Python 3.x");
                let tail = tail.trim_start_matches(':').trim();
                let (pos, text) = tail.split_once(": ").unwrap_or((tail, "cannot parse"));
                let (l, c) = pos.split_once(':').unwrap_or((pos, ""));
                findings.push(format_error(
                    root,
                    path,
                    l.parse().ok(),
                    c.parse().ok(),
                    text,
                ));
            }
        }
    }
    if findings.is_empty() {
        Err(failure("black", out))
    } else {
        Ok(findings)
    }
}

fn format_error(
    root: &Root,
    path: &str,
    line: Option<u32>,
    column: Option<u32>,
    message: &str,
) -> Finding {
    let file = root.relative(path);
    let location = match (line, column) {
        (Some(l), Some(c)) => Location::at(file, l, c),
        (Some(l), None) => Location::line(file, l),
        _ => Location::file(file),
    };
    Finding::new(
        CheckKind::Format,
        Severity::Error,
        location,
        format!("cannot format: {message}"),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::testing::out as plain;
    use super::*;
    use crate::ecosystems::testing::{abs, rooted};

    fn out(code: i32, stdout: &str, stderr: &str) -> ProcessOutput {
        plain(code, &rooted(stdout), stderr)
    }

    fn root() -> Root {
        Root::new(&abs("/ws/proj"))
    }

    /// Captured from ruff 0.17.5 on a file with an unused import, an unsorted import block, and
    /// an unused variable.
    const RUFF_JSON: &str = r#"[
  {"cell": null, "code": "I001", "end_location": {"column": 34, "row": 2},
   "filename": "/ws/proj/src/shop/domain/cart.py",
   "fix": {"applicability": "safe", "edits": [], "message": "Organize imports"},
   "location": {"column": 1, "row": 1}, "message": "Import block is un-sorted or un-formatted",
   "name": "unsorted-imports", "noqa_row": 1, "severity": "error",
   "url": "https://docs.astral.sh/ruff/rules/unsorted-imports"},
  {"cell": null, "code": "F401", "end_location": {"column": 10, "row": 1},
   "filename": "/ws/proj/src/shop/domain/cart.py", "fix": null,
   "location": {"column": 8, "row": 1}, "message": "`os` imported but unused",
   "name": "unused-import", "noqa_row": 1, "severity": "error", "url": null}
]"#;

    /// Captured from ruff 0.17.5 on `def f(:`.
    const RUFF_SYNTAX: &str = r#"[
  {"cell": null, "code": "invalid-syntax", "end_location": {"column": 8, "row": 1},
   "filename": "/ws/proj/src/shop/bad.py", "fix": null,
   "location": {"column": 7, "row": 1},
   "message": "Expected a parameter or the end of the parameter list",
   "name": "invalid-syntax", "noqa_row": null, "severity": "error", "url": null}
]"#;

    #[test]
    fn ruff_check_maps_codes_and_locations() {
        let f = ruff_check(&root(), &out(1, RUFF_JSON, "")).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f[1].location, Location::at("src/shop/domain/cart.py", 1, 8));
        assert_eq!(f[1].message, "`os` imported but unused");
        assert_eq!(f[1].rule.as_deref(), Some("F401"));
        assert_eq!(f[1].severity, Severity::Error);
        assert_eq!(f[1].kind, CheckKind::Lint);
        assert_eq!(f[0].rule.as_deref(), Some("I001"));
    }

    #[test]
    fn ruff_check_reports_syntax_errors_and_old_null_codes() {
        let f = ruff_check(&root(), &out(1, RUFF_SYNTAX, "")).unwrap();
        assert_eq!(f[0].rule.as_deref(), Some("invalid-syntax"));
        let old = RUFF_SYNTAX.replace(
            "\"invalid-syntax\", \"end_location\"",
            "null, \"end_location\"",
        );
        let f = ruff_check(&root(), &out(1, &old, "")).unwrap();
        assert_eq!(f[0].rule, None);
        assert_eq!(f[0].location, Location::at("src/shop/bad.py", 1, 7));
    }

    #[test]
    fn ruff_check_clean_and_failures() {
        assert!(ruff_check(&root(), &out(0, "[]", "")).unwrap().is_empty());
        assert!(ruff_check(&root(), &out(0, "", "")).unwrap().is_empty());
        let e = ruff_check(
            &root(),
            &out(2, "", "error: Failed to load configuration\n"),
        )
        .unwrap_err();
        assert!(e.message().contains("Failed to load configuration"));
        assert!(ruff_check(&root(), &out(1, "garbage", "")).is_err());
        assert!(ruff_check(&root(), &out(1, "", "")).is_err());
        assert!(ruff_check(&root(), &ProcessOutput::default()).is_err());
    }

    #[test]
    fn ruff_format_clean_parse_error_and_crash() {
        assert!(
            ruff_format(&root(), &out(0, "1 file reformatted\n", ""))
                .unwrap()
                .is_empty()
        );
        // Captured from ruff 0.17.5.
        let stderr = "error: Failed to parse src/shop/bad.py:1:7: Expected a parameter or the end of the parameter list\n";
        let f = ruff_format(&root(), &out(2, "", stderr)).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].location, Location::at("src/shop/bad.py", 1, 7));
        assert_eq!(
            f[0].message,
            "cannot format: Expected a parameter or the end of the parameter list"
        );
        assert_eq!(f[0].kind, CheckKind::Format);
        assert!(ruff_format(&root(), &out(2, "", "error: invalid value\n")).is_err());
    }

    #[test]
    fn black_parse_errors_in_both_formats() {
        // Captured from black 26.
        let new = "error: cannot parse: bad.py:1:6\n    def f(:\n         ^\nParseError: bad input\n\nOh no! 💥 💔 💥\n1 file failed to reformat.\n";
        let f = black(&root(), &out(123, "", new)).unwrap();
        assert_eq!(f[0].location, Location::at("bad.py", 1, 6));
        assert_eq!(f[0].message, "cannot format: bad input");
        let old = "error: cannot format pkg/bad.py: Cannot parse: 3:4: x = = 1\nOh no!\n";
        let f = black(&root(), &out(123, "", old)).unwrap();
        assert_eq!(f[0].location, Location::at("pkg/bad.py", 3, 4));
        assert_eq!(f[0].message, "cannot format: x = = 1");
        let older =
            "error: cannot format a.py: Cannot parse for target version Python 3.x: 1:6: def f(:\n";
        assert_eq!(
            black(&root(), &out(123, "", older)).unwrap()[0].location,
            Location::at("a.py", 1, 6)
        );
    }

    #[test]
    fn black_error_with_a_line_but_no_column() {
        let f = black(
            &root(),
            &out(123, "", "error: cannot format a.py: Cannot parse: 3: bad\n"),
        )
        .unwrap();
        assert_eq!(f[0].location, Location::line("a.py", 3));
    }

    #[test]
    fn ruff_timeouts_and_unknown_exit_codes_are_failures() {
        let mut o = out(0, "[]", "");
        o.timed_out = true;
        assert!(ruff_check(&root(), &o).is_err());
        assert!(ruff_check(&root(), &out(2, "[]", "")).is_err());
        assert!(ruff_check(&root(), &out(0, "[]", "")).unwrap().is_empty());
    }

    #[test]
    fn black_clean_and_unreadable_failure() {
        assert!(
            black(&root(), &out(0, "", "All done!\n1 file left unchanged.\n"))
                .unwrap()
                .is_empty()
        );
        assert!(black(&root(), &out(123, "", "error: cannot format a.py: boom\n")).is_err());
    }
}
