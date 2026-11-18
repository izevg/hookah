//! Rust output parsing.
//!
//! - [`clippy`]: cargo JSON lines (`--message-format=json`). Only `compiler-message` lines with
//!   level `error` or `warning` and at least one span count; the primary span gives the
//!   location, `message.message` the text, and `code.code` the rule. Repeats (the same warning
//!   for the lib and test targets) collapse into one finding. Spans in crates outside the
//!   workspace (absolute paths elsewhere) are skipped.
//! - [`rustfmt`]: exit 0 is clean. A failure with `error: ...` plus `--> path:line:col` on stderr
//!   becomes `Error` findings; a failure nobody can read is a `PortError`.
//! - [`cargo_test`]: stdout mixes cargo JSON (handled like clippy, so compile errors in tests are
//!   reported) with libtest text. Each `test <name> ... FAILED` yields one `Error` finding, placed
//!   at its `panicked at path:line:col` location, or at `Cargo.toml` without one.
//!
//! Paths are made relative to the project root, which may give `../sibling/src/x.rs` for a
//! sibling workspace member.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use hookah_application::error::PortError;
use hookah_application::ports::ProcessOutput;
use hookah_domain::finding::{Finding, Location, Severity};
use hookah_domain::path::normalize;
use hookah_domain::toolchain::CheckKind;
use serde::Deserialize;

/// The two roots paths are resolved against.
#[derive(Debug, Clone)]
pub struct Paths {
    project_root: PathBuf,
    workspace_root: PathBuf,
}

impl Paths {
    /// `project_root`: the crate directory; `workspace_root`: what compiler paths are relative to.
    pub fn new(project_root: PathBuf, workspace_root: PathBuf) -> Self {
        Self {
            project_root,
            workspace_root,
        }
    }

    /// Makes a compiler-reported `file` project-relative. `None` for pseudo files (`<...>`) and
    /// absolute paths outside the workspace (registry and toolchain sources).
    pub fn relative(&self, file: &str) -> Option<PathBuf> {
        resolve(&self.project_root, &self.workspace_root, file)
    }
}

fn resolve(project_root: &Path, base: &Path, file: &str) -> Option<PathBuf> {
    if file.starts_with('<') {
        return None;
    }
    let path = Path::new(file);
    let abs = if path.is_absolute() {
        let path = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if !path.starts_with(base) {
            return None;
        }
        path
    } else {
        base.join(path)
    };
    Some(diff(project_root, &normalize(&abs)))
}

/// `to` relative to `from`, using `..` where needed. Both are absolute and normalized.
fn diff(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for c in &to[common..] {
        out.push(c);
    }
    out
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

#[derive(Deserialize)]
struct CargoLine {
    reason: Option<String>,
    message: Option<Diagnostic>,
}

#[derive(Deserialize)]
struct Diagnostic {
    message: String,
    level: String,
    code: Option<Code>,
    #[serde(default)]
    spans: Vec<Span>,
}

#[derive(Deserialize)]
struct Code {
    code: String,
}

#[derive(Deserialize)]
struct Span {
    file_name: String,
    line_start: u32,
    column_start: u32,
    is_primary: bool,
}

/// Findings from the compiler messages in cargo JSON output; other lines are ignored.
fn compiler_findings(paths: &Paths, kind: CheckKind, stdout: &str) -> Vec<Finding> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in stdout.lines().filter(|l| l.starts_with('{')) {
        let Ok(CargoLine {
            reason: Some(reason),
            message: Some(msg),
        }) = serde_json::from_str(line)
        else {
            continue;
        };
        if reason != "compiler-message" {
            continue;
        }
        let severity = match msg.level.as_str() {
            "error" => Severity::Error,
            "warning" => Severity::Warning,
            _ => continue,
        };
        let Some(span) = msg.spans.iter().find(|s| s.is_primary) else {
            continue;
        };
        let Some(path) = paths.relative(&span.file_name) else {
            continue;
        };
        let text = msg.message.lines().next().unwrap_or_default();
        let mut finding = Finding::new(
            kind,
            severity,
            Location::at(path, span.line_start, span.column_start),
            text,
        );
        if let Some(code) = msg.code {
            finding = finding.with_rule(code.code);
        }
        if seen.insert(finding.clone()) {
            out.push(finding);
        }
    }
    out
}

fn failure(tool: &str, out: &ProcessOutput) -> PortError {
    if out.timed_out {
        PortError::new(format!("{tool} timed out"))
    } else {
        PortError::new(format!(
            "{tool} failed without findings: {}",
            tail(&out.stderr)
        ))
    }
}

/// Parses `cargo clippy --message-format=json` output.
pub fn clippy(paths: &Paths, out: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    let findings = compiler_findings(paths, CheckKind::Lint, &out.stdout);
    if findings.is_empty() && !out.success() {
        return Err(failure("clippy", out));
    }
    Ok(findings)
}

/// Parses `rustfmt` output. `project_root` is the (canonical) crate directory.
pub fn rustfmt(project_root: &Path, out: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    if out.success() {
        return Ok(vec![]);
    }
    let mut findings = Vec::new();
    let mut pending: Option<&str> = None;
    for line in out.stderr.lines() {
        if let Some(rest) = line.strip_prefix("error") {
            pending = rest
                .split_once(": ")
                .map(|(_, m)| m.trim())
                .filter(|m| !m.is_empty());
        } else if let (Some(message), Some(at)) = (pending, line.trim_start().strip_prefix("--> "))
            && let Some((file, row, col)) = split_position(at.trim_end_matches(':'))
            && let Some(path) = resolve(project_root, project_root, file)
        {
            findings.push(Finding::new(
                CheckKind::Format,
                Severity::Error,
                Location::at(path, row, col),
                message,
            ));
            pending = None;
        }
    }
    if findings.is_empty() {
        return Err(failure("rustfmt", out));
    }
    Ok(findings)
}

/// Splits `path:line:col`.
fn split_position(s: &str) -> Option<(&str, u32, u32)> {
    let mut parts = s.rsplitn(3, ':');
    let col = parts.next()?.parse().ok()?;
    let row = parts.next()?.parse().ok()?;
    Some((parts.next()?, row, col))
}

/// Parses `cargo test --message-format=json` output.
pub fn cargo_test(paths: &Paths, out: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    let mut findings = compiler_findings(paths, CheckKind::Test, &out.stdout);
    let panics = panics_by_test(&out.stdout);
    let mut seen = HashSet::new();
    for name in out.stdout.lines().filter_map(failed_test) {
        if !seen.insert(name) {
            continue;
        }
        let panic = panics.get(name);
        let location = panic
            .and_then(|p| p.at.as_ref())
            .and_then(|(file, row, col)| Some((paths.relative(file)?, *row, *col)))
            .map_or_else(
                || Location::file("Cargo.toml"),
                |(path, row, col)| Location::at(path, row, col),
            );
        let message = match panic.map(|p| p.message.as_str()) {
            Some(m) if !m.is_empty() => format!("test {name} failed: {m}"),
            _ => format!("test {name} failed"),
        };
        findings.push(Finding::new(
            CheckKind::Test,
            Severity::Error,
            location,
            message,
        ));
    }
    if findings.is_empty() && !out.success() {
        return Err(failure("cargo test", out));
    }
    Ok(findings)
}

/// The test name of a failed-test line: `test <name> ... FAILED`, or `<name> --- FAILED` when
/// libtest runs quiet (cargo's `--quiet` is forwarded to it).
fn failed_test(line: &str) -> Option<&str> {
    if let Some(name) = line.strip_suffix(" --- FAILED") {
        return Some(name);
    }
    line.strip_prefix("test ")?.strip_suffix(" ... FAILED")
}

struct Panic {
    at: Option<(String, u32, u32)>,
    message: String,
}

/// Panic locations and first message lines, from the `---- <name> stdout ----` sections.
fn panics_by_test(stdout: &str) -> HashMap<&str, Panic> {
    let mut out: HashMap<&str, Panic> = HashMap::new();
    let mut current: Option<&str> = None;
    let mut lines = stdout.lines();
    while let Some(line) = lines.next() {
        if let Some(name) = line
            .strip_prefix("---- ")
            .and_then(|l| l.strip_suffix(" stdout ----"))
        {
            current = Some(name);
        } else if line == "failures:" {
            current = None;
        } else if let Some(name) = current
            && !out.contains_key(name)
            && let Some((_, rest)) = line.split_once(" panicked at ")
        {
            let panic = if let Some(loc) = rest.strip_suffix(':') {
                // Rust 1.73+: location on the header line, message on the next line.
                Panic {
                    at: owned_position(loc),
                    message: lines.next().unwrap_or_default().trim().into(),
                }
            } else if let Some((msg, loc)) = rest.rsplit_once(", ") {
                // Older format: `'message', path:line:col`.
                Panic {
                    at: owned_position(loc),
                    message: msg.trim_matches('\'').into(),
                }
            } else {
                continue;
            };
            out.insert(name, panic);
        }
    }
    out
}

fn owned_position(s: &str) -> Option<(String, u32, u32)> {
    split_position(s).map(|(f, r, c)| (f.to_owned(), r, c))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn paths() -> Paths {
        Paths::new("/ws/crates/core".into(), "/ws".into())
    }

    fn out(code: i32, stdout: &str, stderr: &str) -> ProcessOutput {
        ProcessOutput {
            exit_code: Some(code),
            stdout: stdout.into(),
            stderr: stderr.into(),
            timed_out: false,
        }
    }

    const WARN: &str = r#"{"reason":"compiler-message","package_id":"path+file:///ws/crates/core#core-lib@0.1.0","manifest_path":"/ws/crates/core/Cargo.toml","target":{"kind":["lib"],"name":"core_lib","src_path":"/ws/crates/core/src/lib.rs"},"message":{"rendered":"warning: unneeded `return` statement\n --> crates/core/src/lib.rs:3:5\n","$message_type":"diagnostic","children":[],"code":{"code":"clippy::needless_return","explanation":null},"level":"warning","message":"unneeded `return` statement","spans":[{"byte_end":40,"byte_start":30,"column_end":14,"column_start":5,"file_name":"crates/core/src/lib.rs","is_primary":true,"line_end":3,"line_start":3,"text":[]}]}}"#;
    const ERR: &str = r#"{"reason":"compiler-message","package_id":"x","message":{"rendered":"error[E0425]: cannot find value `y` in this scope\n","children":[],"code":{"code":"E0425","explanation":"..."},"level":"error","message":"cannot find value `y` in this scope\nsecond line","spans":[{"column_start":9,"file_name":"crates/sibling/src/x.rs","is_primary":true,"line_start":7},{"column_start":1,"file_name":"crates/core/src/lib.rs","is_primary":false,"line_start":1}]}}"#;
    const SUMMARY: &str = r#"{"reason":"compiler-message","package_id":"x","message":{"rendered":"error: aborting due to 1 previous error\n","children":[],"code":null,"level":"error","message":"aborting due to 1 previous error","spans":[]}}"#;
    const EXTERNAL: &str = r#"{"reason":"compiler-message","package_id":"x","message":{"rendered":"","children":[],"code":null,"level":"warning","message":"unused","spans":[{"column_start":1,"file_name":"/home/u/.cargo/registry/src/idx/dep-1.0.0/src/lib.rs","is_primary":true,"line_start":1}]}}"#;
    const NOISE: &str =
        r#"{"reason":"compiler-artifact","package_id":"x","target":{"name":"a"},"fresh":false}"#;

    #[test]
    fn clippy_extracts_dedupes_and_relativizes() {
        let stdout = [NOISE, WARN, WARN, ERR, SUMMARY, EXTERNAL, "not json"].join("\n");
        let found = clippy(&paths(), &out(101, &stdout, "")).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].location.to_string(), "src/lib.rs:3:5");
        assert_eq!(found[0].severity, Severity::Warning);
        assert_eq!(found[0].message, "unneeded `return` statement");
        assert_eq!(found[0].rule.as_deref(), Some("clippy::needless_return"));
        assert_eq!(found[0].kind, CheckKind::Lint);
        assert_eq!(found[1].location.to_string(), "../sibling/src/x.rs:7:9");
        assert_eq!(found[1].severity, Severity::Error);
        assert_eq!(found[1].message, "cannot find value `y` in this scope");
        assert_eq!(found[1].rule.as_deref(), Some("E0425"));
    }

    #[test]
    fn clippy_clean_run_is_empty_and_silent_failure_degrades() {
        assert!(clippy(&paths(), &out(0, NOISE, "")).unwrap().is_empty());
        let e = clippy(&paths(), &out(101, SUMMARY, "error: no matching package")).unwrap_err();
        assert!(e.message().contains("no matching package"));
        let t = ProcessOutput {
            timed_out: true,
            ..ProcessOutput::default()
        };
        assert!(
            clippy(&paths(), &t)
                .unwrap_err()
                .message()
                .contains("timed out")
        );
    }

    #[test]
    fn rustfmt_clean_and_parse_errors() {
        assert!(
            rustfmt(Path::new("/p"), &out(0, "", ""))
                .unwrap()
                .is_empty()
        );
        let stderr = "error: expected one of `!` or `::`, found `}`\n --> src/lib.rs:4:1\n  |\n4 | }\n  | ^ expected one of `!` or `::`\n\nerror: expected item, found `}`\n --> /p/src/util.rs:9:2:\n";
        let found = rustfmt(Path::new("/p"), &out(1, "", stderr)).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].location.to_string(), "src/lib.rs:4:1");
        assert_eq!(found[0].message, "expected one of `!` or `::`, found `}`");
        assert_eq!(found[0].kind, CheckKind::Format);
        assert_eq!(found[1].location.to_string(), "src/util.rs:9:2");
    }

    #[test]
    fn rustfmt_unreadable_failure_is_a_port_error() {
        let e = rustfmt(Path::new("/p"), &out(1, "", "boom\n")).unwrap_err();
        assert!(e.message().contains("boom"));
    }

    const TEST_OUT: &str = "running 3 tests\ntest tests::ok ... ok\ntest tests::bad ... FAILED\ntest tests::noloc ... FAILED\n\nfailures:\n\n---- tests::bad stdout ----\n\nthread 'tests::bad' panicked at crates/core/src/lib.rs:12:9:\nassertion `left == right` failed\n  left: 1\n right: 2\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n\n---- tests::noloc stdout ----\nthread 'tests::noloc' panicked at /rustc/abc/library/core/src/option.rs:1:1:\ncalled `Option::unwrap()` on a `None` value\n\n\nfailures:\n    tests::bad\n    tests::noloc\n\ntest result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out\n";

    #[test]
    fn cargo_test_reports_failed_tests() {
        let found = cargo_test(&paths(), &out(101, TEST_OUT, "error: test failed")).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].location.to_string(), "src/lib.rs:12:9");
        assert_eq!(
            found[0].message,
            "test tests::bad failed: assertion `left == right` failed"
        );
        assert_eq!(found[0].severity, Severity::Error);
        assert_eq!(found[0].kind, CheckKind::Test);
        assert_eq!(found[1].location.to_string(), "Cargo.toml");
        assert_eq!(
            found[1].message,
            "test tests::noloc failed: called `Option::unwrap()` on a `None` value"
        );
    }

    #[test]
    fn cargo_test_handles_quiet_libtest_output() {
        let quiet = "\nrunning 1 test\ntests::fails --- FAILED\n\nfailures:\n\n---- tests::fails stdout ----\n\nthread 'tests::fails' (14854604) panicked at crates/core/src/lib.rs:6:9:\nassertion failed\n\nfailures:\n    tests::fails\n";
        let found = cargo_test(&paths(), &out(101, quiet, "")).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].location.to_string(), "src/lib.rs:6:9");
        assert_eq!(
            found[0].message,
            "test tests::fails failed: assertion failed"
        );
    }

    #[test]
    fn cargo_test_handles_old_panic_format_and_compile_errors() {
        let old = "test t ... FAILED\n---- t stdout ----\nthread 't' panicked at 'boom', crates/core/src/lib.rs:5:3\n";
        let found = cargo_test(&paths(), &out(101, old, "")).unwrap();
        assert_eq!(found[0].location.to_string(), "src/lib.rs:5:3");
        assert_eq!(found[0].message, "test t failed: boom");

        let found = cargo_test(&paths(), &out(101, &[ERR, SUMMARY].join("\n"), "")).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, CheckKind::Test);
    }

    #[test]
    fn cargo_test_without_a_panic_message_says_only_that_it_failed() {
        let blank = "test t ... FAILED\n---- t stdout ----\nthread 't' panicked at crates/core/src/lib.rs:5:3:\n\nfailures:\n    t\n";
        let found = cargo_test(&paths(), &out(101, blank, "")).unwrap();
        assert_eq!(found[0].message, "test t failed");
    }

    #[test]
    fn paths_with_parent_segments_are_resolved_lexically() {
        assert_eq!(
            paths().relative("crates/core/../sibling/src/x.rs"),
            Some(PathBuf::from("../sibling/src/x.rs"))
        );
        assert_eq!(
            paths().relative("crates/core/src/../lib.rs"),
            Some(PathBuf::from("lib.rs"))
        );
    }

    #[test]
    fn cargo_test_passes_or_degrades() {
        let ok = "running 1 test\ntest a ... ok\n\ntest result: ok. 1 passed\n";
        assert!(cargo_test(&paths(), &out(0, ok, "")).unwrap().is_empty());
        let e = cargo_test(&paths(), &out(101, "", "error: could not compile\n")).unwrap_err();
        assert!(e.message().contains("could not compile"));
    }

    #[test]
    fn diff_walks_up_and_down() {
        assert_eq!(
            diff(Path::new("/a/b"), Path::new("/a/c/d.rs")),
            Path::new("../c/d.rs")
        );
        assert_eq!(
            diff(Path::new("/a"), Path::new("/a/x.rs")),
            Path::new("x.rs")
        );
    }
}
