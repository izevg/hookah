//! Test runs: Vitest, Jest, and Stryker.
//!
//! Vitest and Jest share the report shape `testResults[].assertionResults[]`:
//!
//! - Every failed assertion becomes one `Error` finding `<full name>: <first line of the failure>`
//!   at the test file, on the line of the first stack frame inside that file. Without such a
//!   frame the finding is file-level.
//! - A suite that failed without a failed assertion (import error, syntax error) becomes one
//!   finding from its `message`, with a `(line:col)` suffix used as the position when present.
//! - A non-zero exit without any finding (a crash, a coverage threshold, an error outside the
//!   tests) is a [`PortError`], never a pass.
//!
//! - A Storybook stories file (see [`is_story`]) whose suite failed with no assertion and no
//!   message never ran: Vitest could not open the browser of the Storybook project (Playwright
//!   without its browser, usually). That is an environment problem, not a test failure, so it is
//!   a [`PortError`], which the session shows as a non-blocking notice. When real findings exist
//!   they win and the notice waits for the next run.
//! - A failed story is a finding at the line of its `export const` in the stories file: the
//!   browser's stack frames point into code the dev server transformed, not into the file.
//!
//! Vitest 5 writes its JSON report to a file even without `--outputFile` and prints
//! `JSON report written to <path>`; older versions print the JSON to stdout. The parser follows
//! that message when it is there and reads stdout otherwise. Jest prints the JSON to stdout.
//!
//! Stryker writes `mutation.json` through its JSON reporter and logs
//! `Your report can be found at: file://<path>`; the parser reads the path from that line, so a
//! reporter file name set in the project's config works and a stale report is never read. Only
//! `Survived` mutants are findings (see the ecosystem docs for why `NoCoverage` is not).

use std::collections::BTreeMap;
use std::path::Path;

use hookah_application::error::PortError;
use hookah_application::ports::ProcessOutput;
use hookah_domain::finding::{Finding, Location, Severity};
use hookah_domain::toolchain::CheckKind;
use serde::Deserialize;
use serde_json::Value;

use super::{cap, first_line, json_in, relative, strip_ansi, unreadable};
use crate::ecosystems::node::detect::{JEST, STRYKER, VITEST};
use crate::ecosystems::node::select::is_story;

#[derive(Deserialize)]
struct Report {
    #[serde(rename = "testResults", default)]
    test_results: Vec<Suite>,
}

#[derive(Deserialize)]
struct Suite {
    name: String,
    status: Option<String>,
    message: Option<String>,
    #[serde(rename = "assertionResults", default)]
    assertions: Vec<Assertion>,
}

#[derive(Deserialize)]
struct Assertion {
    status: String,
    title: Option<String>,
    #[serde(rename = "fullName")]
    full_name: Option<String>,
    #[serde(rename = "failureMessages", default)]
    failure_messages: Vec<String>,
}

/// Vitest findings.
pub fn vitest(root: &Path, output: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    let all = strip_ansi(&format!("{}\n{}", output.stdout, output.stderr));
    let value = match written_to(&all, "JSON report written to ") {
        Some(path) => {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| PortError::new(format!("cannot read vitest report {path}: {e}")))?;
            serde_json::from_str(&text)
                .map_err(|e| PortError::new(format!("unreadable vitest report: {e}")))?
        }
        None => match json_in(&output.stdout) {
            Some(v) => v,
            None if output.success() => return Ok(vec![]),
            None => return Err(unreadable(VITEST, output)),
        },
    };
    findings(root, value, VITEST, output)
}

/// Jest findings.
pub fn jest(root: &Path, output: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    match json_in(&output.stdout) {
        Some(value) => findings(root, value, JEST, output),
        None if output.success() => Ok(vec![]),
        None => Err(unreadable(JEST, output)),
    }
}

fn findings(
    root: &Path,
    value: Value,
    tool: &str,
    output: &ProcessOutput,
) -> Result<Vec<Finding>, PortError> {
    let report: Report = serde_json::from_value(value)
        .map_err(|e| PortError::new(format!("unreadable {tool} report: {e}")))?;
    let mut found = Vec::new();
    let mut stories_not_run = false;
    for suite in report.test_results {
        let path = relative(root, &suite.name);
        let mut failed = false;
        for a in suite.assertions.iter().filter(|a| a.status == "failed") {
            failed = true;
            let stack = strip_ansi(&a.failure_messages.join("\n"));
            let title = a
                .full_name
                .as_deref()
                .or(a.title.as_deref())
                .unwrap_or("test");
            let message = format!("{title}: {}", first_line(&stack));
            let location = if is_story(&path) {
                // A story runs in the browser, whose stack frames point into transformed code,
                // so the line is the story's own export instead.
                match story_line(root, &path, title) {
                    Some(line) => Location::line(path.clone(), line),
                    None => Location::file(path.clone()),
                }
            } else {
                match frame_position(&stack, &suite.name) {
                    Some((line, column)) => Location::at(path.clone(), line, column),
                    None => Location::file(path.clone()),
                }
            };
            found.push(Finding::new(
                CheckKind::Test,
                Severity::Error,
                location,
                message,
            ));
        }
        if !failed
            && suite.status.as_deref() == Some("failed")
            && suite.assertions.is_empty()
            && suite.message.as_deref().is_none_or(|m| m.trim().is_empty())
            && is_story(&path)
        {
            stories_not_run = true;
        } else if !failed && suite.status.as_deref() == Some("failed") {
            let text = suite_failure(&strip_ansi(suite.message.as_deref().unwrap_or("")));
            let location = match trailing_position(&text) {
                Some((line, column)) => Location::at(path, line, column),
                None => Location::file(path),
            };
            found.push(Finding::new(
                CheckKind::Test,
                Severity::Error,
                location,
                if text.is_empty() {
                    "test suite failed to run".to_owned()
                } else {
                    text
                },
            ));
        }
    }
    if found.is_empty() && stories_not_run {
        return Err(PortError::new(stories_notice(&strip_ansi(&format!(
            "{}\n{}",
            output.stdout, output.stderr
        )))));
    }
    if found.is_empty() && !output.success() {
        return Err(unreadable(tool, output));
    }
    Ok(found)
}

/// The line of the `export const <name>` of the story called `title` (Storybook turns
/// `LoggedIn` into `Logged In`, so spaces and case are ignored), in the stories file `rel`.
fn story_line(root: &Path, rel: &Path, title: &str) -> Option<u32> {
    let wanted: String = title
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    let text = std::fs::read_to_string(root.join(rel)).ok()?;
    let line = text.lines().position(|l| {
        let Some(rest) = l.trim_start().strip_prefix("export ") else {
            return false;
        };
        let rest = rest
            .strip_prefix("const ")
            .or_else(|| rest.strip_prefix("let "))
            .or_else(|| rest.strip_prefix("function "));
        rest.is_some_and(|r| {
            let name: String = r
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                .flat_map(char::to_lowercase)
                .collect();
            name == wanted
        })
    })?;
    u32::try_from(line + 1).ok()
}

/// Why story tests did not run, for the notice: Playwright's own missing-browser message when
/// the output has it.
fn stories_notice(output: &str) -> String {
    if [
        "playwright install",
        "Executable doesn't exist",
        "browserType.launch",
    ]
    .iter()
    .any(|marker| output.contains(marker))
    {
        "story tests skipped: Playwright could not launch its browser (run `npx playwright install`)"
            .to_owned()
    } else {
        "story tests skipped: the Storybook browser project did not start, run vitest on the story yourself"
            .to_owned()
    }
}

/// `line:col` of the first stack frame that points into `file` (an absolute path).
fn frame_position(stack: &str, file: &str) -> Option<(u32, u32)> {
    stack.lines().find_map(|l| {
        let rest = l[l.find(file)? + file.len()..].strip_prefix(':')?;
        let (line, rest) = rest.split_once(':')?;
        let column: String = rest.chars().take_while(char::is_ascii_digit).collect();
        Some((line.parse().ok()?, column.parse().ok()?))
    })
}

/// The one line that says why a suite failed to run. Jest wraps the cause in a long banner and
/// puts it after `Details:`; otherwise the first line that is not a `●` heading is used.
fn suite_failure(message: &str) -> String {
    let lines: Vec<&str> = message
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let after_details = lines
        .iter()
        .position(|l| *l == "Details:")
        .and_then(|i| lines.get(i + 1));
    let line = after_details.or_else(|| lines.iter().find(|l| !l.starts_with('●')));
    cap(line.copied().unwrap_or(""), 200)
}

/// The `(line:col)` a message ends with, as Babel and Prettier print it.
fn trailing_position(text: &str) -> Option<(u32, u32)> {
    let inner = text.strip_suffix(')')?;
    let (_, pos) = inner.rsplit_once('(')?;
    let (line, column) = pos.split_once(':')?;
    Some((line.parse().ok()?, column.parse().ok()?))
}

/// The text after `marker` on the first line that contains it.
fn written_to(text: &str, marker: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.split_once(marker))
        .map(|(_, path)| path.trim().to_owned())
        .filter(|p| !p.is_empty())
}

#[derive(Deserialize)]
struct MutationReport {
    #[serde(default)]
    files: BTreeMap<String, MutatedFile>,
}

#[derive(Deserialize)]
struct MutatedFile {
    #[serde(default)]
    mutants: Vec<Mutant>,
}

#[derive(Deserialize)]
struct Mutant {
    #[serde(rename = "mutatorName")]
    mutator: String,
    replacement: Option<String>,
    status: String,
    location: Span,
}

#[derive(Deserialize)]
struct Span {
    start: Position,
}

#[derive(Deserialize)]
struct Position {
    line: u32,
    column: u32,
}

/// Stryker findings: one error per surviving mutant, read from the JSON report.
pub fn stryker(root: &Path, output: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    let all = strip_ansi(&format!("{}\n{}", output.stdout, output.stderr));
    let url = written_to(&all, "Your report can be found at: ");
    let Some(path) = url.as_deref().and_then(file_url_path) else {
        return Err(stryker_failure(&all, output));
    };
    let path = percent_decode(path);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| PortError::new(format!("cannot read stryker report {path}: {e}")))?;
    let report: MutationReport = serde_json::from_str(&text)
        .map_err(|e| PortError::new(format!("unreadable stryker report: {e}")))?;
    let mut found = Vec::new();
    for (file, data) in report.files {
        for m in data.mutants.into_iter().filter(|m| m.status == "Survived") {
            let replacement = m
                .replacement
                .as_deref()
                .map(|r| cap(r, 60))
                .unwrap_or_default();
            found.push(Finding::new(
                CheckKind::Mutation,
                Severity::Error,
                Location::at(
                    relative(root, &file),
                    m.location.start.line,
                    m.location.start.column,
                ),
                format!("mutant survived: {} {replacement}", m.mutator)
                    .trim_end()
                    .to_owned(),
            ));
        }
    }
    found.sort_by(|a, b| a.location.cmp(&b.location));
    Ok(found)
}

/// Stryker's own `ERROR` log lines say what went wrong; fall back to the output tail.
fn stryker_failure(all: &str, output: &ProcessOutput) -> PortError {
    let errors: Vec<&str> = all
        .lines()
        .filter(|l| l.contains(" ERROR "))
        .map(str::trim)
        .collect();
    if errors.is_empty() {
        return unreadable(STRYKER, output);
    }
    let start = errors.len().saturating_sub(2);
    PortError::new(format!(
        "{STRYKER} failed: {}",
        cap(&errors[start..].join(" | "), 300)
    ))
}

/// The path of a `file://` URL. A Windows URL is `file:///C:/dir/f.json`: the slash before the
/// drive letter is not part of the path.
fn file_url_path(url: &str) -> Option<&str> {
    let path = url.strip_prefix("file://")?;
    match path.as_bytes() {
        [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => Some(&path[1..]),
        _ => Some(path),
    }
}

/// Decodes `%XX` escapes (spaces and non-ASCII in a `file://` URL).
fn percent_decode(text: &str) -> String {
    let mut chunks = text.as_bytes().split(|b| *b == b'%');
    let mut out = chunks.next().unwrap_or_default().to_vec();
    for chunk in chunks {
        match chunk.get(..2).and_then(hex_byte) {
            Some(byte) => {
                out.push(byte);
                out.extend_from_slice(&chunk[2..]);
            }
            None => {
                out.push(b'%');
                out.extend_from_slice(chunk);
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The byte spelled by two hex digits.
fn hex_byte(digits: &[u8]) -> Option<u8> {
    std::str::from_utf8(digits)
        .ok()
        .filter(|d| d.bytes().all(|b| b.is_ascii_hexdigit()))
        .and_then(|d| u8::from_str_radix(d, 16).ok())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::ecosystems::node::detect::package::scratch::Scratch;
    use crate::ecosystems::testing::{WS, rooted};

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

    /// `vitest related --run --reporter=json --outputFile=... src/add.ts` with Vitest 5.1.0; the
    /// frames inside `node_modules` and paths shortened.
    const VITEST_REPORT: &str = r#"{"numTotalTestSuites":1,"numPassedTestSuites":0,"numFailedTestSuites":1,"numTotalTests":3,"numPassedTests":2,"numFailedTests":1,"snapshot":{"added":0,"failure":false},"startTime":1794956344043,"success":false,"testResults":[{"assertionResults":[{"ancestorTitles":[],"fullName":"adds","status":"passed","title":"adds","duration":0.62,"failureMessages":[],"meta":{},"tags":[],"benchmarks":[]},{"ancestorTitles":[],"fullName":"fails","status":"failed","title":"fails","duration":2.2,"failureMessages":["AssertionError: expected 2 to be 3 // Object.is equality\n    at /ws/app/src/add.test.ts:5:41\n    at file:///ws/app/node_modules/vitest/dist/chunks/run.js:1632:35\n    at new Promise (<anonymous>)"],"meta":{},"tags":[],"benchmarks":[]}],"startTime":1794956344108,"endTime":1794956344111.2,"status":"failed","message":"","name":"/ws/app/src/add.test.ts"}]}"#;

    #[test]
    fn vitest_reads_the_report_file_named_in_its_output() {
        let s = Scratch::new();
        let file = s.write("vitest-report.json", &rooted(VITEST_REPORT));
        let stdout = format!("\nJSON report written to {}\n", file.display());
        let found = vitest(Path::new(ROOT), &out(1, &stdout, "")).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].location, Location::at("src/add.test.ts", 5, 41));
        assert_eq!(
            found[0].message,
            "fails: AssertionError: expected 2 to be 3 // Object.is equality"
        );
        assert_eq!(found[0].severity, Severity::Error);
        assert_eq!(found[0].kind, CheckKind::Test);
    }

    /// A failed story from `vitest related --run --reporter=json` with the Storybook addon (10.6,
    /// Vitest 5.1, Playwright Chromium); the browser frame is a URL with a query.
    const STORY_REPORT: &str = r#"{"testResults":[{"assertionResults":[{"ancestorTitles":[],"fullName":"Small","status":"passed","title":"Small","duration":1,"failureMessages":[],"meta":{"storyId":"badge--small","reports":[]}},{"ancestorTitles":[],"fullName":"Capped","status":"failed","title":"Capped","duration":1,"failureMessages":["AssertionError: expected '100' to be '99+' // Object.is equality\n    at play (http://localhost:23315/ws/app/src/badge.stories.js?import&browserv=1795015716165:27:45)"],"meta":{"storyId":"badge--capped"}}],"startTime":1,"endTime":2,"status":"failed","message":"","name":"/ws/app/src/badge.stories.js"}]}"#;

    /// The same run without the Playwright browser: the stories suite fails with no assertion and
    /// no message, and the output says why.
    const NO_BROWSER_REPORT: &str = r#"{"testResults":[{"assertionResults":[{"ancestorTitles":[],"fullName":"caps","status":"passed","title":"caps","duration":1,"failureMessages":[],"meta":{}}],"startTime":1,"endTime":2,"status":"passed","message":"","name":"/ws/app/src/label.test.js"},{"assertionResults":[],"startTime":1,"endTime":1,"status":"failed","message":"","name":"/ws/app/src/badge.stories.js"}]}"#;

    const NO_BROWSER_STDERR: &str = "error during close browserType.launch: Executable doesn't exist at /home/u/.cache/ms-playwright/chromium_headless_shell-1255/chrome-headless-shell\n\u{2551} Please run the following command to download new browsers:\n\u{2551}     npx playwright install\n";

    const STORIES: &str = "import { x } from \"y\";\n\nexport default { title: \"Badge\" };\n\nexport const Small = {};\n  export const Capped = {};\nexport const LoggedIn = {};\n";

    #[test]
    fn a_failed_story_is_a_finding_at_its_export() {
        let s = Scratch::new();
        s.write("src/badge.stories.js", STORIES);
        let found = vitest(&s.0, &out(1, &story_report(&s), "")).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].location, Location::line("src/badge.stories.js", 6));
        assert_eq!(
            found[0].message,
            "Capped: AssertionError: expected '100' to be '99+' // Object.is equality"
        );
    }

    /// [`STORY_REPORT`] with the suite path inside `s`.
    fn story_report(s: &Scratch) -> String {
        STORY_REPORT.replace("/ws/app", &s.0.to_string_lossy().replace('\\', "/"))
    }

    #[test]
    fn story_lines_match_export_names_loosely() {
        let s = Scratch::new();
        s.write("a.stories.js", STORIES);
        let line = |t: &str| story_line(&s.0, Path::new("a.stories.js"), t);
        assert_eq!(line("Small"), Some(5));
        assert_eq!(line("Logged In"), Some(7));
        s.write(
            "b.stories.js",
            "export const With_Under = {};\nexport const $Dollar = {};\n",
        );
        let line_b = |t: &str| story_line(&s.0, Path::new("b.stories.js"), t);
        assert_eq!(line_b("With_Under"), Some(1));
        assert_eq!(line_b("$Dollar"), Some(2));
        assert_eq!(line("Sma"), None);
        assert_eq!(line("Missing"), None);
        assert_eq!(
            story_line(&s.0, Path::new("gone.stories.js"), "Small"),
            None
        );
    }

    #[test]
    fn a_story_without_a_findable_export_is_file_level() {
        let found = vitest(Path::new(ROOT), &out(1, STORY_REPORT, "")).unwrap();
        assert_eq!(found[0].location, Location::file("src/badge.stories.js"));
    }

    #[test]
    fn a_missing_browser_is_a_notice_not_a_finding() {
        let e = vitest(
            Path::new(ROOT),
            &out(1, NO_BROWSER_REPORT, NO_BROWSER_STDERR),
        )
        .unwrap_err();
        assert!(e.to_string().contains("npx playwright install"), "{e}");
        let e = vitest(Path::new(ROOT), &out(1, NO_BROWSER_REPORT, "")).unwrap_err();
        assert!(e.to_string().contains("did not start"), "{e}");
    }

    #[test]
    fn real_findings_win_over_the_story_notice() {
        let both = NO_BROWSER_REPORT.replace(
            r#""status":"passed","message":"","name":"/ws/app/src/label.test.js""#,
            r#""status":"failed","message":"","name":"/ws/app/src/label.test.js""#,
        );
        let both = both.replace(
            r#""fullName":"caps","status":"passed""#,
            r#""fullName":"caps","status":"failed""#,
        );
        let found = vitest(Path::new(ROOT), &out(1, &both, NO_BROWSER_STDERR)).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].location.path, Path::new("src/label.test.js"));
    }

    #[test]
    fn a_story_suite_with_a_message_is_a_finding() {
        let broken = NO_BROWSER_REPORT.replace(
            r#""status":"failed","message":"","name":"/ws/app/src/badge.stories.js""#,
            r#""status":"failed","message":"Error: Cannot find module './nope.js' (3:9)","name":"/ws/app/src/badge.stories.js""#,
        );
        let found = vitest(Path::new(ROOT), &out(1, &broken, "")).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].location,
            Location::at("src/badge.stories.js", 3, 9)
        );
    }

    #[test]
    fn vitest_reads_stdout_when_no_file_was_written() {
        let found = vitest(Path::new(ROOT), &out(1, VITEST_REPORT, "")).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].location.line, Some(5));
    }

    #[test]
    fn vitest_failures_without_a_report_are_errors_but_clean_runs_pass() {
        let e = vitest(
            Path::new(ROOT),
            &out(1, "", "Error: Cannot find module 'x'\n"),
        )
        .unwrap_err();
        assert!(e.message().contains("Cannot find module"));
        assert!(
            vitest(Path::new(ROOT), &out(0, "No test files found\n", ""))
                .unwrap()
                .is_empty()
        );
        let e = vitest(
            Path::new(ROOT),
            &out(0, "JSON report written to /nonexistent/report.json\n", ""),
        )
        .unwrap_err();
        assert!(e.message().contains("cannot read vitest report"));
    }

    #[test]
    fn a_failed_run_without_failed_tests_is_not_a_pass() {
        let report = r#"{"success":false,"testResults":[{"name":"/ws/app/src/a.test.ts","status":"passed","assertionResults":[{"status":"passed","title":"ok","failureMessages":[]}]}]}"#;
        assert!(
            vitest(
                Path::new(ROOT),
                &out(
                    1,
                    report,
                    "ERROR: Coverage for lines (50%) does not meet threshold\n"
                )
            )
            .is_err()
        );
        assert!(
            vitest(Path::new(ROOT), &out(0, report, ""))
                .unwrap()
                .is_empty()
        );
    }

    /// `jest --ci --json --no-watchman --passWithNoTests --findRelatedTests ...` with Jest 29.7,
    /// failure messages shortened; the second suite does not parse.
    const JEST_OUT: &str = r#"{"numFailedTestSuites":2,"numFailedTests":1,"numPassedTests":1,"numTotalTests":2,"success":false,"testResults":[{"assertionResults":[{"ancestorTitles":[],"failureMessages":[],"fullName":"adds","location":null,"status":"passed","title":"adds"},{"ancestorTitles":[],"failureMessages":["Error: expect(received).toBe(expected) // Object.is equality\n\nExpected: 3\nReceived: 2\n    at Object.toBe (/ws/app/src/add.test.js:3:41)\n    at Promise.then.completed (/ws/app/node_modules/jest-circus/build/utils.js:298:28)"],"fullName":"fails","location":null,"status":"failed","title":"fails"}],"message":"  ● fails\n\n    expect(received).toBe(expected) // Object.is equality","name":"/ws/app/src/add.test.js","status":"failed"},{"assertionResults":[],"message":"  ● Test suite failed to run\n\n    Jest encountered an unexpected token\n\n    Jest failed to parse a file.\n\n    Details:\n\n    SyntaxError: /ws/app/src/broken.test.js: Unexpected token (3:0)\n\n      1 | const { add } = require(\"./add\");\n    > 3 |\n        | ^\n\n      at constructor (node_modules/@babel/parser/src/parse-error.ts:96:45)","name":"/ws/app/src/broken.test.js","status":"failed"}]}"#;

    #[test]
    fn jest_assertions_and_suites_that_fail_to_run() {
        let found = jest(Path::new(ROOT), &out(1, JEST_OUT, "")).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].location, Location::at("src/add.test.js", 3, 41));
        assert_eq!(
            found[0].message,
            "fails: Error: expect(received).toBe(expected) // Object.is equality"
        );
        assert_eq!(found[1].location, Location::at("src/broken.test.js", 3, 0));
        assert_eq!(
            found[1].message,
            rooted("SyntaxError: /ws/app/src/broken.test.js: Unexpected token (3:0)")
        );
    }

    #[test]
    fn jest_strips_colors_and_reports_crashes() {
        let colored = JEST_OUT.replace("Error: expect", "\\u001b[31mError: expect");
        let found = jest(Path::new(ROOT), &out(1, &colored, "")).unwrap();
        assert!(found[0].message.starts_with("fails: Error: expect"));
        assert!(
            jest(
                Path::new(ROOT),
                &out(1, "", "No tests found, exiting with code 1\n")
            )
            .is_err()
        );
        assert!(
            jest(Path::new(ROOT), &out(0, r#"{"testResults":[]}"#, ""))
                .unwrap()
                .is_empty()
        );
    }

    /// The tail of a Stryker 10 run with `--reporters json` (ANSI colors and all).
    const STRYKER_LOG: &str = "\u{1b}[32m15:01:34 (17625) INFO JsonReporter\u{1b}[39m Your report can be found at: file://{PATH}\n\u{1b}[32m15:01:34 (17625) INFO MutationTestExecutor\u{1b}[39m Done in 1 second.\n";

    /// `reports/mutation/mutation.json` of that run, without ids, coverage, and test lists.
    const STRYKER_REPORT: &str = r#"{"schemaVersion":"1.0","thresholds":{"high":80,"low":60},"files":{"src/m.js":{"language":"javascript","source":"export function add","mutants":[{"id":"9","mutatorName":"BooleanLiteral","replacement":"true","status":"NoCoverage","static":false,"location":{"end":{"column":15,"line":6},"start":{"column":10,"line":6}}},{"id":"5","mutatorName":"EqualityOperator","replacement":"a <= 0","status":"Survived","static":false,"location":{"end":{"column":12,"line":5},"start":{"column":7,"line":5}}},{"id":"3","mutatorName":"ConditionalExpression","replacement":"true","status":"Survived","static":false,"location":{"end":{"column":12,"line":5},"start":{"column":7,"line":5}}},{"id":"1","mutatorName":"ArithmeticOperator","replacement":"a - b","statusReason":"expected -1 to be 3","status":"Killed","static":false,"location":{"end":{"column":15,"line":2},"start":{"column":10,"line":2}}}]},"src/k.js":{"mutants":[{"id":"0","mutatorName":"BlockStatement","replacement":"{}\n  ","status":"Survived","location":{"end":{"column":2,"line":3},"start":{"column":27,"line":1}}}]}}}"#;

    #[test]
    fn a_file_url_names_a_path_on_every_os() {
        assert_eq!(file_url_path("file:///a/b.json"), Some("/a/b.json"));
        assert_eq!(file_url_path("file:///C:/a/b.json"), Some("C:/a/b.json"));
        assert_eq!(file_url_path("file://C:\\a\\b.json"), Some("C:\\a\\b.json"));
        // Only a letter before the colon is a drive.
        assert_eq!(file_url_path("file:///1:/a"), Some("/1:/a"));
        assert_eq!(file_url_path("/a/b.json"), None);
    }

    #[test]
    fn stryker_reports_survivors_only() {
        let s = Scratch::new();
        let report = s.write("reports/mutation/mutation.json", STRYKER_REPORT);
        let stdout = STRYKER_LOG.replace("{PATH}", &report.to_string_lossy());
        let found = stryker(Path::new(ROOT), &out(0, &stdout, "")).unwrap();
        let shown: Vec<_> = found
            .iter()
            .map(|f| format!("{} {}", f.location, f.message))
            .collect();
        assert_eq!(
            shown,
            [
                "src/k.js:1:27 mutant survived: BlockStatement {}",
                "src/m.js:5:7 mutant survived: EqualityOperator a <= 0",
                "src/m.js:5:7 mutant survived: ConditionalExpression true",
            ]
        );
        assert!(
            found
                .iter()
                .all(|f| f.severity == Severity::Error && f.kind == CheckKind::Mutation)
        );
    }

    #[test]
    fn stryker_report_path_is_percent_decoded() {
        let s = Scratch::new();
        let report = s.write("my reports/mutation.json", STRYKER_REPORT);
        let url = report.to_string_lossy().replace(' ', "%20");
        let stdout = STRYKER_LOG.replace("{PATH}", &url);
        assert_eq!(
            stryker(Path::new(ROOT), &out(0, &stdout, ""))
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn stryker_failures_quote_the_error_log() {
        let log = "\u{1b}[91m15:01:29 (16366) ERROR DryRunExecutor\u{1b}[39m One or more tests failed in the initial test run:\n\t\tnpm error Missing script: \"test\"\n\u{1b}[91m15:01:29 (16366) ERROR Stryker\u{1b}[39m There were failed tests in the initial test run.\n";
        let e = stryker(Path::new(ROOT), &out(1, log, "")).unwrap_err();
        assert!(e.message().starts_with("stryker failed: "));
        assert!(
            e.message()
                .contains("There were failed tests in the initial test run.")
        );
        let e = stryker(Path::new(ROOT), &out(1, "", "boom\n")).unwrap_err();
        assert!(e.message().contains("boom"));
    }

    #[test]
    fn decodes_percent_escapes() {
        assert_eq!(percent_decode("/a%20b/%C3%A9"), "/a b/é");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%41%7e%2F"), "A~/");
        assert_eq!(percent_decode("a%%41"), "a%A");
        assert_eq!(percent_decode("%4"), "%4");
        assert_eq!(percent_decode("%zz%4g"), "%zz%4g");
        // A sign is not a hex digit.
        assert_eq!(percent_decode("%+1"), "%+1");
    }

    #[test]
    fn jest_clean_run_without_a_report_passes() {
        let ok = ProcessOutput {
            exit_code: Some(0),
            stdout: "no json here".into(),
            stderr: String::new(),
            timed_out: false,
        };
        assert!(jest(Path::new(WS), &ok).unwrap().is_empty());
    }

    #[test]
    fn suite_failure_skips_bullet_headings() {
        assert_eq!(
            suite_failure("  ● Test suite failed to run\n\n    Cannot find module 'x'\n"),
            "Cannot find module 'x'"
        );
        assert_eq!(suite_failure("Boom\nmore"), "Boom");
        assert_eq!(suite_failure("● only a heading"), "");
    }
}
