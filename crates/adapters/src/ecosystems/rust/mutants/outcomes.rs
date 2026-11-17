//! Reading what `cargo-mutants` did.
//!
//! The tool writes `<output>/mutants.out/outcomes.json` and exits with a code: 0 (all caught),
//! 2 (some missed), 3 (some timed out), 4 (the unmutated baseline failed), anything else is a
//! tool failure. Findings come from the file, not from the exit code:
//!
//! - `MissedMutant` is an `Error`: `mutant survived: <description>` at the mutated line.
//! - `Timeout` is a `Warning`: `mutant timed out: <description>`. A mutant that hangs the tests
//!   is often an endless loop, which is a kind of catch, so it does not block.
//! - Caught, unviable, and successful outcomes produce nothing.
//! - A baseline failure (exit 4, or a baseline outcome that is not `Success`), a timeout of the
//!   whole run, an unknown exit code, or an unreadable file is a `PortError`. With `--iterate`
//!   the file lists only the mutants tested in this run, never stale ones.
//!
//! File paths in the output are workspace-relative and are made project-relative like every
//! other Rust finding.

use hookah_application::error::PortError;
use hookah_application::ports::ProcessOutput;
use hookah_domain::finding::{Finding, Location, Severity};
use hookah_domain::toolchain::CheckKind;
use serde::Deserialize;

use super::super::parse::{Paths, tail};

#[derive(Deserialize)]
struct Outcomes {
    outcomes: Vec<Outcome>,
}

#[derive(Deserialize)]
struct Outcome {
    scenario: Scenario,
    summary: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Scenario {
    Baseline(String),
    Mutant {
        #[serde(rename = "Mutant")]
        mutant: Mutant,
    },
}

#[derive(Deserialize)]
struct Mutant {
    name: String,
    file: String,
    span: Span,
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

/// Turns a finished `cargo-mutants` run into findings. `outcomes_json` is the content of
/// `outcomes.json`, `None` if the file does not exist.
pub fn parse(
    paths: &Paths,
    output: &ProcessOutput,
    outcomes_json: Option<&str>,
) -> Result<Vec<Finding>, PortError> {
    if output.timed_out {
        return Err(PortError::new("cargo-mutants timed out"));
    }
    match output.exit_code {
        Some(0 | 2 | 3) => {}
        Some(4) => {
            return Err(PortError::new(format!(
                "cargo-mutants: the unmutated baseline fails to build or pass its tests: {}",
                tail_of(output)
            )));
        }
        code => {
            return Err(PortError::new(format!(
                "cargo-mutants failed (exit {}): {}",
                code.map_or_else(|| "signal".into(), |c| c.to_string()),
                tail_of(output)
            )));
        }
    }
    let Some(json) = outcomes_json else {
        // No mutants matched the selection, so no outcomes were written.
        return if output.exit_code == Some(0) {
            Ok(vec![])
        } else {
            Err(PortError::new("cargo-mutants wrote no outcomes.json"))
        };
    };
    let parsed: Outcomes = serde_json::from_str(json)
        .map_err(|e| PortError::new(format!("unreadable cargo-mutants outcomes: {e}")))?;
    let mut findings = Vec::new();
    for outcome in parsed.outcomes {
        match outcome.scenario {
            Scenario::Baseline(name) if outcome.summary != "Success" => {
                return Err(PortError::new(format!(
                    "cargo-mutants: the unmutated {} did not pass ({})",
                    name.to_lowercase(),
                    outcome.summary
                )));
            }
            Scenario::Baseline(_) => {}
            Scenario::Mutant { mutant } => {
                let (severity, verb) = match outcome.summary.as_str() {
                    "MissedMutant" => (Severity::Error, "survived"),
                    "Timeout" => (Severity::Warning, "timed out"),
                    _ => continue,
                };
                let Some(path) = paths.relative(&mutant.file) else {
                    continue;
                };
                let finding = Finding::new(
                    CheckKind::Mutation,
                    severity,
                    Location::line(path, mutant.span.start.line),
                    format!("mutant {verb}: {}", description(&mutant)),
                );
                if !findings.contains(&finding) {
                    findings.push(finding);
                }
            }
        }
    }
    Ok(findings)
}

/// The last lines of what the tool printed.
fn tail_of(output: &ProcessOutput) -> String {
    if output.stderr.trim().is_empty() {
        tail(&output.stdout)
    } else {
        tail(&output.stderr)
    }
}

/// The mutant's name without its `file:line:col: ` prefix, such as `replace + with - in add`.
fn description(mutant: &Mutant) -> &str {
    let prefix = format!(
        "{}:{}:{}: ",
        mutant.file, mutant.span.start.line, mutant.span.start.column
    );
    mutant.name.strip_prefix(&prefix).unwrap_or(&mutant.name)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // Trimmed from `cargo-mutants 27.1.0` runs on a one-function crate in the workspace
    // member `crates/a`. The `Timeout` entry follows the same shape with that summary.
    const RUN: &str = r#"{"outcomes":[
      {"scenario":"Baseline","summary":"Success","log_path":"log/baseline.log","diff_path":null,"phase_results":[]},
      {"scenario":{"Mutant":{"name":"crates/a/src/lib.rs:2:5: replace add -> i32 with 0","package":"scratch","file":"crates/a/src/lib.rs","function":{"function_name":"add","return_type":"-> i32","span":{"start":{"line":1,"column":1},"end":{"line":3,"column":2}}},"span":{"start":{"line":2,"column":5},"end":{"line":2,"column":10}},"replacement":"0","genre":"FnValue"}},"summary":"MissedMutant","log_path":"x","diff_path":"y","phase_results":[]},
      {"scenario":{"Mutant":{"name":"crates/a/src/lib.rs:2:5: replace add -> i32 with 1","package":"scratch","file":"crates/a/src/lib.rs","function":null,"span":{"start":{"line":2,"column":5},"end":{"line":2,"column":10}},"replacement":"1","genre":"FnValue"}},"summary":"CaughtMutant","log_path":"x","diff_path":"y","phase_results":[]},
      {"scenario":{"Mutant":{"name":"crates/a/src/lib.rs:2:7: replace + with - in add","package":"scratch","file":"crates/a/src/lib.rs","function":null,"span":{"start":{"line":2,"column":7},"end":{"line":2,"column":8}},"replacement":"-","genre":"BinaryOperator"}},"summary":"MissedMutant","log_path":"x","diff_path":"y","phase_results":[]},
      {"scenario":{"Mutant":{"name":"crates/a/src/lib.rs:7:9: replace spin with ()","package":"scratch","file":"crates/a/src/lib.rs","function":null,"span":{"start":{"line":7,"column":9},"end":{"line":7,"column":12}},"replacement":"()","genre":"FnValue"}},"summary":"Timeout","log_path":"x","diff_path":"y","phase_results":[]},
      {"scenario":{"Mutant":{"name":"crates/a/src/lib.rs:9:1: replace f -> u8 with 2","package":"scratch","file":"crates/a/src/lib.rs","function":null,"span":{"start":{"line":9,"column":1},"end":{"line":9,"column":2}},"replacement":"2","genre":"FnValue"}},"summary":"Unviable","log_path":"x","diff_path":"y","phase_results":[]}
    ],"total_mutants":5,"missed":2,"caught":1,"timeout":1,"unviable":1,"success":0,"cargo_mutants_version":"27.1.0"}"#;

    const BASELINE_FAILED: &str = r#"{"outcomes":[{"scenario":"Baseline","summary":"Failure","log_path":"log/baseline.log","diff_path":null,"phase_results":[{"phase":"Test","duration":0.06,"process_status":{"Failure":101},"argv":["cargo","test"]}]}],"total_mutants":0,"missed":0,"caught":0,"timeout":0,"unviable":0,"success":0,"cargo_mutants_version":"27.1.0"}"#;

    fn paths() -> Paths {
        Paths::new("/ws/crates/a".into(), "/ws".into())
    }

    fn exit(code: i32) -> ProcessOutput {
        ProcessOutput {
            exit_code: Some(code),
            ..ProcessOutput::default()
        }
    }

    #[test]
    fn missed_mutants_are_errors_and_timeouts_are_warnings() {
        let found = parse(&paths(), &exit(2), Some(RUN)).unwrap();
        let shown: Vec<_> = found
            .iter()
            .map(|f| format!("{} {:?} {}", f.location, f.severity, f.message))
            .collect();
        assert_eq!(
            shown,
            [
                "src/lib.rs:2 Error mutant survived: replace add -> i32 with 0",
                "src/lib.rs:2 Error mutant survived: replace + with - in add",
                "src/lib.rs:7 Warning mutant timed out: replace spin with ()",
            ]
        );
        assert!(found.iter().all(|f| f.kind == CheckKind::Mutation));
    }

    #[test]
    fn clean_run_and_no_mutants_are_empty() {
        let clean = r#"{"outcomes":[{"scenario":"Baseline","summary":"Success"}]}"#;
        assert!(parse(&paths(), &exit(0), Some(clean)).unwrap().is_empty());
        assert!(parse(&paths(), &exit(0), None).unwrap().is_empty());
    }

    #[test]
    fn baseline_failure_is_a_port_error() {
        let e = parse(&paths(), &exit(4), Some(BASELINE_FAILED)).unwrap_err();
        assert!(e.message().contains("baseline"), "{}", e.message());
        // Even if the exit code were lost, the outcome file says it.
        let e = parse(&paths(), &exit(2), Some(BASELINE_FAILED)).unwrap_err();
        assert!(e.message().contains("baseline"));
    }

    #[test]
    fn tool_failures_are_port_errors() {
        assert!(parse(&paths(), &exit(1), None).is_err());
        assert!(parse(&paths(), &exit(70), Some(RUN)).is_err());
        assert!(parse(&paths(), &exit(2), None).is_err());
        assert!(parse(&paths(), &exit(2), Some("nope")).is_err());
        let timed_out = ProcessOutput {
            timed_out: true,
            ..ProcessOutput::default()
        };
        assert!(parse(&paths(), &timed_out, Some(RUN)).is_err());
        assert!(parse(&paths(), &ProcessOutput::default(), None).is_err());
    }
}
