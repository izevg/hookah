//! Whole-program checks: `tsc` and dependency-cruiser.
//!
//! - `tsc --pretty false` prints one `path(line,col): error TSxxxx: message` line per problem,
//!   followed by indented continuation lines that are dropped. A diagnostic without a location
//!   (`error TS5083: Cannot read file ...`) is placed at `tsconfig.json`. All diagnostics are
//!   errors. The compiler checks the whole program, so errors in files other than the edited one
//!   are reported too, by design: an edit can break its callers.
//! - dependency-cruiser (`--output-type json`): the entries of `summary.violations`. Severity
//!   `error` is an error, `warn` a warning, `info` and `ignore` are dropped. The location is the
//!   violating `from` module (the JSON has no line), the rule is the rule name, and the message is
//!   `rule: from -> to`, or `rule: from` for rules about a module alone, such as orphans.

use std::path::Path;

use hookah_application::error::PortError;
use hookah_application::ports::ProcessOutput;
use hookah_domain::finding::{Finding, Location, Severity};
use hookah_domain::toolchain::CheckKind;
use serde::Deserialize;

use super::{first_line, json_in, relative, strip_ansi, unreadable};
use crate::ecosystems::node::detect::{DEPCRUISE, TSC, TSCONFIG};

/// Findings from `tsc` (also `tsc -b`) output.
pub fn tsc(root: &Path, output: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    let text = strip_ansi(&output.stdout);
    let findings: Vec<Finding> = text.lines().filter_map(|l| tsc_line(root, l)).collect();
    if findings.is_empty() && !output.success() {
        return Err(unreadable(TSC, output));
    }
    Ok(findings)
}

/// `src/a.ts(6,14): error TS2322: Type 'number' is not assignable to type 'string'.`
fn tsc_line(root: &Path, line: &str) -> Option<Finding> {
    let (location, diagnostic) = match line.find("): error TS") {
        Some(i) => {
            let (path, position) = line[..i].rsplit_once('(')?;
            let (l, c) = position.split_once(',')?;
            let location = Location::at(relative(root, path), l.parse().ok()?, c.parse().ok()?);
            (location, &line[i + 3..])
        }
        None if line.starts_with("error TS") => (Location::file(TSCONFIG), line),
        None => return None,
    };
    let (head, message) = diagnostic.split_once(": ")?;
    let code = head.strip_prefix("error ")?;
    Some(
        Finding::new(
            CheckKind::Typecheck,
            Severity::Error,
            location,
            first_line(message),
        )
        .with_rule(code),
    )
}

#[derive(Deserialize)]
struct Cruise {
    summary: Summary,
}

#[derive(Deserialize)]
struct Summary {
    #[serde(default)]
    violations: Vec<Violation>,
}

#[derive(Deserialize)]
struct Violation {
    from: String,
    to: Option<String>,
    rule: Rule,
}

#[derive(Deserialize)]
struct Rule {
    name: String,
    severity: String,
}

/// Findings from `depcruise --output-type json`.
pub fn depcruise(root: &Path, output: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    let Some(value) = json_in(&output.stdout) else {
        return Err(unreadable(DEPCRUISE, output));
    };
    let cruise: Cruise = serde_json::from_value(value)
        .map_err(|e| PortError::new(format!("unreadable dependency-cruiser report: {e}")))?;
    Ok(cruise
        .summary
        .violations
        .into_iter()
        .filter_map(|v| {
            let severity = match v.rule.severity.as_str() {
                "error" => Severity::Error,
                "warn" => Severity::Warning,
                _ => return None,
            };
            let message = match v.to.as_deref() {
                Some(to) if to != v.from => format!("{}: {} -> {to}", v.rule.name, v.from),
                _ => format!("{}: {}", v.rule.name, v.from),
            };
            Some(
                Finding::new(
                    CheckKind::Architecture,
                    severity,
                    Location::file(relative(root, &v.from)),
                    message,
                )
                .with_rule(v.rule.name),
            )
        })
        .collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const ROOT: &str = "/ws/app";

    fn out(exit: i32, stdout: &str) -> ProcessOutput {
        ProcessOutput {
            exit_code: Some(exit),
            stdout: stdout.into(),
            ..ProcessOutput::default()
        }
    }

    /// `tsc --noEmit --pretty false -p tsconfig.json` with TypeScript 7.0.2 and 5.9.3: the same
    /// line format, plus a made-up multi-line message in the style of tsc.
    const TSC_OUT: &str = "src/a.ts(6,14): error TS2322: Type 'number' is not assignable to type 'string'.\nsrc/b.ts(10,3): error TS2345: Argument of type '{ a: number; }' is not assignable to parameter of type 'B'.\n  Property 'b' is missing in type '{ a: number; }' but required in type 'B'.\n../lib/src/c.ts(1,1): error TS2304: Cannot find name 'x'.\n";

    #[test]
    fn tsc_diagnostics() {
        let found = tsc(Path::new(ROOT), &out(1, TSC_OUT)).unwrap();
        assert_eq!(found.len(), 3, "continuation lines are dropped: {found:?}");
        assert_eq!(found[0].location, Location::at("src/a.ts", 6, 14));
        assert_eq!(
            found[0].message,
            "Type 'number' is not assignable to type 'string'."
        );
        assert_eq!(found[0].rule.as_deref(), Some("TS2322"));
        assert_eq!(found[0].severity, Severity::Error);
        assert_eq!(found[0].kind, CheckKind::Typecheck);
        assert_eq!(found[1].location, Location::at("src/b.ts", 10, 3));
        assert_eq!(found[2].location, Location::at("../lib/src/c.ts", 1, 1));
    }

    #[test]
    fn tsc_build_mode_without_a_position_and_with_parenthesized_paths() {
        let text = "packages/app/tsconfig.json(1,177): error TS6310: Referenced project '/ws/packages/lib' may not disable emit.\nerror TS18003: No inputs were found in config file.\napp/(group)/page.tsx(2,5): error TS2322: Bad (type): mismatch.\n";
        let found = tsc(Path::new(ROOT), &out(2, text)).unwrap();
        assert_eq!(found.len(), 3);
        assert_eq!(
            found[0].location,
            Location::at("packages/app/tsconfig.json", 1, 177)
        );
        assert_eq!(found[1].location, Location::file("tsconfig.json"));
        assert_eq!(found[1].rule.as_deref(), Some("TS18003"));
        assert_eq!(
            found[2].location,
            Location::at("app/(group)/page.tsx", 2, 5)
        );
        assert_eq!(found[2].message, "Bad (type): mismatch.");
    }

    #[test]
    fn tsc_clean_run_and_unreadable_failure() {
        assert!(tsc(Path::new(ROOT), &out(0, "")).unwrap().is_empty());
        let e = tsc(
            Path::new(ROOT),
            &ProcessOutput {
                exit_code: Some(1),
                stderr: "node: bad option\n".into(),
                ..ProcessOutput::default()
            },
        )
        .unwrap_err();
        assert!(e.message().contains("bad option"));
    }

    /// `depcruise --config .dependency-cruiser.cjs --output-type json src/ui/view.ts` with
    /// dependency-cruiser 18.8.0; `modules`, `optionsUsed`, and `ruleSetUsed` removed. The info
    /// and orphan entries were added by hand to cover every severity.
    const DEPCRUISE_OUT: &str = r#"{"modules":[],"summary":{"violations":[{"type":"dependency","from":"src/ui/view.ts","to":"src/db/q.ts","unresolvedTo":"../db/q","dependencyTypes":["local","import"],"rule":{"severity":"error","name":"no-ui-to-db"}},{"type":"cycle","from":"src/db/q.ts","to":"src/ui/view.ts","unresolvedTo":"../ui/view","dependencyTypes":["local","import"],"rule":{"severity":"warn","name":"no-circular"},"cycle":[{"name":"src/ui/view.ts","dependencyTypes":["local","import"]},{"name":"src/db/q.ts","dependencyTypes":["local","import"]}]},{"type":"module","from":"src/a.ts","to":"src/a.ts","rule":{"severity":"warn","name":"no-orphans"}},{"type":"dependency","from":"src/ui/view.ts","to":"src/add.ts","rule":{"severity":"info","name":"note"}}],"error":1,"warn":2,"info":1,"ignore":0,"totalCruised":3}}"#;

    #[test]
    fn depcruise_violations() {
        let found = depcruise(Path::new(ROOT), &out(1, DEPCRUISE_OUT)).unwrap();
        assert_eq!(found.len(), 3, "info is dropped: {found:?}");
        assert_eq!(found[0].severity, Severity::Error);
        assert_eq!(found[0].location, Location::file("src/ui/view.ts"));
        assert_eq!(
            found[0].message,
            "no-ui-to-db: src/ui/view.ts -> src/db/q.ts"
        );
        assert_eq!(found[0].rule.as_deref(), Some("no-ui-to-db"));
        assert_eq!(found[0].kind, CheckKind::Architecture);
        assert_eq!(found[1].severity, Severity::Warning);
        assert_eq!(
            found[1].message,
            "no-circular: src/db/q.ts -> src/ui/view.ts"
        );
        assert_eq!(found[2].message, "no-orphans: src/a.ts");
    }

    #[test]
    fn depcruise_without_a_report_is_an_error() {
        let e = depcruise(Path::new(ROOT), &out(1, "  ERROR: Can't find a config\n")).unwrap_err();
        assert!(e.message().starts_with("depcruise failed"));
        assert!(
            depcruise(Path::new(ROOT), &out(0, r#"{"summary":{"violations":[]}}"#))
                .unwrap()
                .is_empty()
        );
        assert!(depcruise(Path::new(ROOT), &out(0, r#"{"summary":5}"#)).is_err());
    }
}
