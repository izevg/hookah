//! Whole-program checks: `tsc`, dependency-cruiser, and knip.
//!
//! - `tsc --pretty false` prints one `path(line,col): error TSxxxx: message` line per problem,
//!   followed by indented continuation lines that are dropped. A diagnostic without a location
//!   (`error TS5083: Cannot read file ...`) is placed at `tsconfig.json`. All diagnostics are
//!   errors. The compiler checks the whole program, so errors in files other than the edited one
//!   are reported too, by design: an edit can break its callers.
//! - dependency-cruiser (`--output-type json`): the entries of `summary.violations`. Severity
//!   `error` is an error, `warn` a warning, `info` and `ignore` are dropped. The location is the
//!   violating `from` module, with the line of the violating import when the specifier is found
//!   in the file (the JSON has no position), the rule is the rule name, and the message is
//!   `from -> to`, or `from` for rules about a module alone, such as orphans.
//! - knip (`--reporter json`): `{"issues": [{"file": ..., "<type>": [{"name", "line", "col"}]}]}`,
//!   one row per file with one array per issue type (`files`, `exports`, `types`,
//!   `dependencies`, `unlisted`, ...). Every issue is a warning of kind `Unused`, the rule is the
//!   issue type, and the message names the symbol without its position, so the fingerprint
//!   survives line shifts. Dependency issues sit in `package.json`; a file issue has no line.
//!   `duplicates` entries are arrays of the symbols exported twice.

use std::path::Path;

use hookah_application::error::PortError;
use hookah_application::ports::ProcessOutput;
use hookah_domain::finding::{Finding, Location, Severity};
use hookah_domain::toolchain::CheckKind;
use serde::Deserialize;
use serde_json::Value;

use super::{first_line, json_in, relative, strip_ansi, unreadable};
use crate::ecosystems::node::detect::{DEPCRUISE, KNIP, TSC, TSCONFIG};

/// The diagnostic that says a referenced project has not been built.
const NOT_BUILT: &str = "TS6305";

/// Findings from `tsc` output.
///
/// `TS6305` ("Output file ... has not been built from source file ...") is dropped: hookah never
/// builds the referenced projects of the project it checks (see `detect::typecheck`), so every
/// import of one raises it. It says nothing about the edit. A run that only raised it is still
/// understood, not a crash.
pub fn tsc(root: &Path, output: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    let text = strip_ansi(&output.stdout);
    let diagnostics: Vec<Finding> = text.lines().filter_map(|l| tsc_line(root, l)).collect();
    if diagnostics.is_empty() && !output.success() {
        return Err(unreadable(TSC, output));
    }
    Ok(diagnostics
        .into_iter()
        .filter(|f| f.rule.as_deref() != Some(NOT_BUILT))
        .collect())
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
    /// The import specifier as written in `from`, such as `../infra/money`.
    #[serde(rename = "unresolvedTo")]
    unresolved_to: Option<String>,
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
            // The rule name is the finding's rule and shows in the report; the message is the edge.
            let message = match v.to.as_deref() {
                Some(to) if to != v.from => format!("{} -> {to}", v.from),
                _ => v.from.clone(),
            };
            let path = relative(root, &v.from);
            let location = match v
                .unresolved_to
                .as_deref()
                .and_then(|spec| import_line(&root.join(&path), spec))
            {
                Some(line) => Location::line(path, line),
                None => Location::file(path),
            };
            Some(
                Finding::new(CheckKind::Architecture, severity, location, message)
                    .with_rule(v.rule.name),
            )
        })
        .collect())
}

/// How a knip issue type reads in a message; the symbol follows.
fn knip_label(issue_type: &str) -> &str {
    match issue_type {
        "files" => "unused file",
        "dependencies" => "unused dependency",
        "devDependencies" => "unused devDependency",
        "optionalPeerDependencies" => "unused optionalPeerDependency",
        "unlisted" => "unlisted dependency",
        "binaries" => "unlisted binary",
        "unresolved" => "unresolved import",
        "exports" => "unused export",
        "nsExports" => "unused namespace export",
        "types" => "unused exported type",
        "nsTypes" => "unused namespace type",
        "enumMembers" => "unused enum member",
        "namespaceMembers" => "unused namespace member",
        "duplicates" => "duplicate export",
        "subpaths" => "unused subpath export",
        "catalog" => "unused catalog entry",
        "catalogReferences" => "unresolved catalog reference",
        "cycles" => "circular dependency",
        other => other,
    }
}

/// `name`, or `namespace.name` for a member, of one knip symbol.
fn knip_symbol(symbol: &Value) -> Option<String> {
    let name = symbol.get("name")?.as_str()?;
    Some(match symbol.get("namespace").and_then(Value::as_str) {
        Some(namespace) => format!("{namespace}.{name}"),
        None => name.to_owned(),
    })
}

fn knip_line(symbol: &Value) -> Option<u32> {
    u32::try_from(symbol.get("line")?.as_u64()?).ok()
}

/// Findings from `knip --reporter json`.
pub fn knip(root: &Path, output: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    let Some(value) = json_in(&output.stdout) else {
        return Err(unreadable(KNIP, output));
    };
    let Some(rows) = value.get("issues").and_then(Value::as_array) else {
        return Err(PortError::new("unreadable knip report: no `issues` list"));
    };
    let mut found = Vec::new();
    for row in rows {
        let (Some(file), Some(row)) = (row.get("file").and_then(Value::as_str), row.as_object())
        else {
            continue;
        };
        let path = relative(root, file);
        for (issue_type, items) in row {
            let Some(items) = items.as_array() else {
                continue;
            };
            for item in items {
                // A duplicate is a group of symbols exported under several names.
                let symbols: Vec<&Value> = match item.as_array() {
                    Some(group) => group.iter().collect(),
                    None => vec![item],
                };
                let names: Vec<String> = symbols.iter().filter_map(|s| knip_symbol(s)).collect();
                if names.is_empty() {
                    continue;
                }
                let message = format!("{}: {}", knip_label(issue_type), names.join(", "));
                let location = match symbols.first().and_then(|s| knip_line(s)) {
                    Some(line) => Location::line(path.clone(), line),
                    None => Location::file(path.clone()),
                };
                found.push(
                    Finding::new(CheckKind::Unused, Severity::Warning, location, message)
                        .with_rule(issue_type.as_str()),
                );
            }
        }
    }
    Ok(found)
}

/// The 1-based line of the first quoted occurrence of `specifier` in `file`. The report carries
/// no position, so this is a best effort: the first line with the specifier in single, double,
/// or back quotes, which is the import, re-export, or `require` that causes the violation. An
/// unreadable file or an unquoted specifier gives `None`.
fn import_line(file: &Path, specifier: &str) -> Option<u32> {
    let text = std::fs::read_to_string(file).ok()?;
    let quoted = ['"', '\'', '`'].map(|q| format!("{q}{specifier}{q}"));
    let index = text
        .lines()
        .position(|line| quoted.iter().any(|q| line.contains(q.as_str())))?;
    u32::try_from(index + 1).ok()
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
    fn tsc_diagnostics_without_a_position_and_with_parenthesized_paths() {
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
    fn tsc_drops_unbuilt_references_but_keeps_the_rest() {
        let text = "src/m.ts(1,17): error TS6305: Output file '/ws/lib/dist/index.d.ts' has not been built from source file '/ws/lib/src/index.ts'.\nsrc/m.ts(2,1): error TS2322: Bad.\n";
        let found = tsc(Path::new(ROOT), &out(2, text)).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rule.as_deref(), Some("TS2322"));
        // A run that only raised it is understood, and reports nothing.
        let only = text.lines().next().unwrap();
        assert!(tsc(Path::new(ROOT), &out(2, only)).unwrap().is_empty());
        // A crash without any diagnostic is still an error.
        assert!(tsc(Path::new(ROOT), &out(2, "")).is_err());
    }

    #[test]
    fn tsc_line_without_a_position_needs_the_error_prefix() {
        let root = Path::new("/ws");
        assert!(tsc_line(root, "error foo: bar").is_none());
        assert!(tsc_line(root, "warning TS1: bar").is_none());
        let f = tsc_line(root, "error TS5083: Cannot read file.").unwrap();
        assert_eq!(f.rule.as_deref(), Some("TS5083"));
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
        assert_eq!(found[0].message, "src/ui/view.ts -> src/db/q.ts");
        assert_eq!(found[0].rule.as_deref(), Some("no-ui-to-db"));
        assert_eq!(found[0].kind, CheckKind::Architecture);
        assert_eq!(found[1].severity, Severity::Warning);
        assert_eq!(found[1].message, "src/db/q.ts -> src/ui/view.ts");
        assert_eq!(found[2].message, "src/a.ts");
    }

    #[test]
    fn depcruise_violations_get_the_line_of_the_import() {
        let dir = std::env::temp_dir().join(format!("hookah-cruise-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src/ui")).unwrap();
        std::fs::write(
            dir.join("src/ui/view.ts"),
            "import { a } from \"./a\";\n\nimport { q } from '../db/q';\n",
        )
        .unwrap();
        let found = depcruise(&dir, &out(1, DEPCRUISE_OUT)).unwrap();
        assert_eq!(found[0].location, Location::line("src/ui/view.ts", 3));
        // No such file, or a specifier that does not appear, leaves the line out.
        assert_eq!(found[1].location, Location::file("src/db/q.ts"));
        assert_eq!(found[2].location, Location::file("src/a.ts"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_lines_by_quote_style() {
        let dir = std::env::temp_dir().join(format!("hookah-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.ts");
        std::fs::write(
            &file,
            "// ./x mentioned\nconst a = require(\"./x\");\nimport('./y');\nimport(`./z`);\n",
        )
        .unwrap();
        assert_eq!(import_line(&file, "./x"), Some(2));
        assert_eq!(import_line(&file, "./y"), Some(3));
        assert_eq!(import_line(&file, "./z"), Some(4));
        assert_eq!(import_line(&file, "./w"), None);
        assert_eq!(import_line(&dir.join("missing.ts"), "./x"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `knip --reporter json --no-progress --no-exit-code --no-config-hints` with knip 6.45.1 in
    /// a package with one unused file, dependency, devDependency, export, type, enum member, and
    /// namespace member, an unlisted binary and dependency, and an unresolved import; rows with
    /// only empty arrays and the empty arrays themselves removed. The ordering of the rows is
    /// knip's.
    const KNIP_OUT: &str = r#"{"issues":[{"file":"src/orphan.ts","files":[{"name":"src/orphan.ts"}]},{"file":"package.json","binaries":[{"name":"eslint"}],"dependencies":[{"name":"left-pad","line":8,"col":6,"pos":130}],"devDependencies":[{"name":"lodash","line":13,"col":6,"pos":229}]},{"file":"src/index.ts","unlisted":[{"name":"chalk","line":2,"col":19,"pos":60}],"unresolved":[{"name":"./missing.ts","line":3,"col":21,"pos":89}]},{"file":"src/a.ts","enumMembers":[{"namespace":"Color","name":"Green","line":4,"col":26,"pos":116}],"exports":[{"name":"unusedFn","line":2,"col":14,"pos":42},{"name":"default","line":7,"col":16,"pos":174}],"namespaceMembers":[{"namespace":"ns","name":"two","line":8,"col":58,"pos":234}],"types":[{"name":"Unused","line":3,"col":13,"pos":74}]}]}"#;

    fn knip_lines(found: &[Finding]) -> Vec<String> {
        found
            .iter()
            .map(|f| {
                format!(
                    "{} {} {}",
                    f.location,
                    f.rule.as_deref().unwrap_or(""),
                    f.message
                )
            })
            .collect()
    }

    #[test]
    fn knip_issues_are_unused_warnings_without_positions_in_the_message() {
        let found = knip(Path::new(ROOT), &out(0, KNIP_OUT)).unwrap();
        assert!(
            found
                .iter()
                .all(|f| f.kind == CheckKind::Unused && f.severity == Severity::Warning)
        );
        assert_eq!(
            knip_lines(&found),
            [
                "src/orphan.ts files unused file: src/orphan.ts",
                "package.json binaries unlisted binary: eslint",
                "package.json:8 dependencies unused dependency: left-pad",
                "package.json:13 devDependencies unused devDependency: lodash",
                "src/index.ts:2 unlisted unlisted dependency: chalk",
                "src/index.ts:3 unresolved unresolved import: ./missing.ts",
                "src/a.ts:4 enumMembers unused enum member: Color.Green",
                "src/a.ts:2 exports unused export: unusedFn",
                "src/a.ts:7 exports unused export: default",
                "src/a.ts:8 namespaceMembers unused namespace member: ns.two",
                "src/a.ts:3 types unused exported type: Unused",
            ]
        );
    }

    #[test]
    fn knip_duplicates_are_groups_of_names_at_the_first_position() {
        let report = r#"{"issues":[{"file":"src/a.ts","duplicates":[[{"name":"d","line":6,"col":10,"pos":1},{"name":"e","line":6,"col":18,"pos":2}]]}]}"#;
        let found = knip(Path::new(ROOT), &out(0, report)).unwrap();
        assert_eq!(
            knip_lines(&found),
            ["src/a.ts:6 duplicates duplicate export: d, e"]
        );
    }

    #[test]
    fn knip_unknown_issue_types_keep_their_own_name_and_odd_rows_are_skipped() {
        let report = r#"{"issues":[{"file":"a.ts","future":[{"name":"x"}]},{"file":"b.ts","exports":[{"line":1}]},{"nofile":true},{"file":"c.ts","owners":[{"name":"@me"}],"exports":"no"}]}"#;
        let found = knip(Path::new(ROOT), &out(0, report)).unwrap();
        assert_eq!(
            knip_lines(&found),
            ["a.ts future future: x", "c.ts owners owners: @me"]
        );
    }

    #[test]
    fn knip_clean_run_and_unreadable_reports() {
        assert!(
            knip(Path::new(ROOT), &out(0, r#"{"issues":[]}"#))
                .unwrap()
                .is_empty()
        );
        let failed = ProcessOutput {
            exit_code: Some(2),
            stderr: "ERROR: Error loading knip.json\n".into(),
            ..ProcessOutput::default()
        };
        let e = knip(Path::new(ROOT), &failed).unwrap_err();
        assert!(e.message().starts_with("knip failed (exit 2)"));
        assert!(knip(Path::new(ROOT), &out(0, r#"{"issues":5}"#)).is_err());
        assert!(knip(Path::new(ROOT), &out(0, "")).is_err());
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
