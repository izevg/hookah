//! import-linter output (`lint-imports`).
//!
//! Exit 0 means every contract is kept. Otherwise the report lists each contract's status, then a
//! "Broken contracts" section:
//!
//! ```text
//! Layers
//! ------
//!
//! shop.infra is not allowed to import shop.app:
//!
//! - shop.infra.db2 -> shop.util.helpers (l.1)
//!   shop.util.helpers -> shop.app.svc (l.1)
//! ```
//!
//! Each illegal import chain (a `-` line plus its indented continuation lines) becomes one
//! `Error` finding of kind `Architecture`. The message is the chain as `a.b -> c.d -> e.f`, the
//! rule is the contract name, and the location is the first module of the chain, at the line of
//! its import (`(l.N)`). Module names are mapped to files in the src or flat layout. A broken
//! contract without any readable chain is reported at the project's configuration instead.
//! Output that shows no broken contract at all, such as a missing root package, is a
//! [`PortError`].

use std::path::PathBuf;

use hookah_application::error::PortError;
use hookah_application::ports::ProcessOutput;
use hookah_domain::finding::{Finding, Location, Severity};
use hookah_domain::toolchain::CheckKind;

use super::{Root, failure};
use crate::ecosystems::python::layout;

/// One import in a chain: `importer -> imported (l.N)`.
#[derive(Debug, PartialEq, Eq)]
struct Hop {
    importer: String,
    imported: String,
    line: Option<u32>,
}

/// One illegal chain of a contract.
#[derive(Debug, PartialEq, Eq)]
struct Chain {
    contract: String,
    hops: Vec<Hop>,
}

/// Findings of one `lint-imports` run.
pub fn parse(root: &Root, out: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    if out.timed_out || out.exit_code.is_none() {
        return Err(failure("lint-imports", out));
    }
    if out.exit_code == Some(0) {
        return Ok(vec![]);
    }
    let (chains, broken) = read_report(&out.stdout);
    if broken.is_empty() {
        return Err(failure("lint-imports", out));
    }
    let mut findings: Vec<Finding> = chains.iter().map(|c| chain_finding(root, c)).collect();
    for contract in &broken {
        if !chains.iter().any(|c| &c.contract == contract) {
            let location = Location::file(config_file(root));
            findings.push(
                Finding::new(
                    CheckKind::Architecture,
                    Severity::Error,
                    location,
                    "contract broken",
                )
                .with_rule(contract.clone()),
            );
        }
    }
    Ok(findings)
}

fn config_file(root: &Root) -> PathBuf {
    [".importlinter", "setup.cfg"]
        .iter()
        .find(|f| root.path().join(f).is_file() && !root.path().join("pyproject.toml").is_file())
        .map_or_else(|| PathBuf::from("pyproject.toml"), PathBuf::from)
}

fn chain_finding(root: &Root, chain: &Chain) -> Finding {
    let message = {
        let mut names: Vec<&str> = chain.hops.iter().map(|h| h.importer.as_str()).collect();
        if let Some(last) = chain.hops.last() {
            names.push(&last.imported);
        }
        names.join(" -> ")
    };
    let first = chain.hops.first();
    let location = match first {
        Some(hop) => {
            let file = layout::module_file(root.path(), &hop.importer).unwrap_or_else(|| {
                PathBuf::from(hop.importer.replace('.', "/")).with_extension("py")
            });
            match hop.line {
                Some(line) => Location::line(file, line),
                None => Location::file(file),
            }
        }
        None => Location::file(config_file(root)),
    };
    Finding::new(CheckKind::Architecture, Severity::Error, location, message)
        .with_rule(chain.contract.clone())
}

/// Reads the report: the illegal chains, and the names of all contracts listed as broken.
fn read_report(stdout: &str) -> (Vec<Chain>, Vec<String>) {
    let lines: Vec<&str> = stdout.lines().map(str::trim_end).collect();
    let broken: Vec<String> = lines
        .iter()
        .filter_map(|l| l.strip_suffix(" BROKEN").map(|n| n.trim().to_string()))
        .collect();
    let Some(start) = lines.iter().position(|l| l.trim() == "Broken contracts") else {
        return (vec![], broken);
    };

    let mut chains: Vec<Chain> = Vec::new();
    let mut contract: Option<String> = None;
    // Whether the last chain may still grow: set by a hop, cleared by a heading, a blank line, or
    // a line ending in a colon.
    let mut open = false;
    let mut rest = lines[start..].iter().skip(1).peekable();
    while let Some(&line) = rest.next() {
        let next_is_rule = rest.peek().is_some_and(|n| is_rule(n));
        if !line.trim().is_empty() && !is_rule(line) && next_is_rule {
            contract = Some(line.trim().to_string());
            open = false;
            rest.next(); // the rule under the heading
            continue;
        }
        if let (Some(name), Some((starts_chain, hop))) = (&contract, hop(line)) {
            match chains.last_mut() {
                Some(last) if !starts_chain && open => last.hops.push(hop),
                _ => {
                    chains.push(Chain {
                        contract: name.clone(),
                        hops: vec![hop],
                    });
                    open = true;
                }
            }
        } else if line.trim().is_empty() || line.trim().ends_with(':') {
            open = false;
        }
    }
    (chains, broken)
}

/// A line of three or more dashes.
fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3 && t.chars().all(|c| c == '-')
}

/// Reads `[- ]importer -> imported [(l.N[, l.M])]`. The flag tells whether the line starts a new
/// chain (it has the `-` marker).
fn hop(line: &str) -> Option<(bool, Hop)> {
    let t = line.trim_start();
    let (starts, body) = match t.strip_prefix("- ") {
        Some(rest) => (true, rest.trim_start()),
        None => (false, t),
    };
    let (importer, rest) = body.split_once(" -> ")?;
    if importer.contains(char::is_whitespace) {
        return None;
    }
    let (imported, tail) = match rest.split_once(' ') {
        Some((a, b)) => (a, b),
        None => (rest, ""),
    };
    if imported.is_empty() {
        return None;
    }
    let line_no = tail
        .strip_prefix("(l.")
        .and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|n| n.parse().ok());
    Some((
        starts,
        Hop {
            importer: importer.to_string(),
            imported: imported.to_string(),
            line: line_no,
        },
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::Path;

    use crate::ecosystems::python::fixture::Scratch;

    use super::super::testing::out;
    use super::*;

    /// Captured from import-linter 2.16 on a src-layout project with three broken contracts.
    const REPORT: &str = "\
---------
Contracts
---------

Analyzed 10 files, 6 dependencies.
----------------------------------

Domain must not import infra BROKEN
Layers BROKEN
Util is independent BROKEN

Contracts: 0 kept, 3 broken.


----------------
Broken contracts
----------------

Domain must not import infra
----------------------------

shop.domain is not allowed to import shop.infra:

-   shop.domain.cart -> shop.infra.db (l.2)


Layers
------

shop.infra is not allowed to import shop.app:

- shop.infra.db2 -> shop.util.helpers (l.1)
  shop.util.helpers -> shop.app.svc (l.1)


Util is independent
-------------------

shop.util is not allowed to import shop.app:

- shop.util.helpers -> shop.app.svc (l.1)


shop.app is not allowed to import shop.util:

- shop.app.svc -> shop.util.helpers (l.1)


";

    fn project() -> Scratch {
        let s = Scratch::new("imports");
        for f in [
            "src/shop/domain/cart.py",
            "src/shop/infra/db2.py",
            "src/shop/util/helpers.py",
            "src/shop/app/svc.py",
        ] {
            s.write(f, "");
        }
        s
    }

    #[test]
    fn broken_contracts_become_located_chain_findings() {
        let s = project();
        let f = parse(&Root::new(s.root()), &out(1, REPORT, "")).unwrap();
        let lines: Vec<String> = f
            .iter()
            .map(|f| {
                format!(
                    "{}: {} [{}]",
                    f.location,
                    f.message,
                    f.rule.as_deref().unwrap()
                )
            })
            .collect();
        assert_eq!(
            lines,
            [
                "src/shop/domain/cart.py:2: shop.domain.cart -> shop.infra.db [Domain must not import infra]",
                "src/shop/infra/db2.py:1: shop.infra.db2 -> shop.util.helpers -> shop.app.svc [Layers]",
                "src/shop/util/helpers.py:1: shop.util.helpers -> shop.app.svc [Util is independent]",
                "src/shop/app/svc.py:1: shop.app.svc -> shop.util.helpers [Util is independent]",
            ]
        );
        assert!(
            f.iter()
                .all(|f| f.kind == CheckKind::Architecture && f.severity == Severity::Error)
        );
    }

    #[test]
    fn unresolvable_modules_fall_back_to_a_guessed_path() {
        let s = Scratch::new("imports-guess");
        let report = REPORT.replace("shop.domain.cart ->", "other.mod ->");
        let f = parse(&Root::new(s.root()), &out(1, &report, "")).unwrap();
        assert_eq!(f[0].location, Location::line("other/mod.py", 2));
    }

    #[test]
    fn flat_layout_modules_resolve() {
        let s = Scratch::new("imports-flat");
        s.write("shop/domain/cart.py", "");
        let f = parse(&Root::new(s.root()), &out(1, REPORT, "")).unwrap();
        assert_eq!(f[0].location, Location::line("shop/domain/cart.py", 2));
    }

    #[test]
    fn kept_contracts_and_crashes() {
        let root = Root::new(Path::new("/p"));
        let kept = "Contracts: 1 kept, 0 broken.\n";
        assert!(parse(&root, &out(0, kept, "")).unwrap().is_empty());
        // Captured: a root package that cannot be imported.
        let e = parse(
            &root,
            &out(
                1,
                "╔══╗\nCould not find package 'shop' in your Python path.\n",
                "",
            ),
        )
        .unwrap_err();
        assert!(e.message().contains("Could not find package 'shop'"));
        assert!(parse(&root, &ProcessOutput::default()).is_err());
    }

    #[test]
    fn broken_contract_without_a_readable_chain_is_reported_at_the_config() {
        let s = Scratch::new("imports-nochain");
        s.write("pyproject.toml", "");
        let report = "My contract BROKEN\n\nContracts: 0 kept, 1 broken.\n\n----------------\nBroken contracts\n----------------\n\nMy contract\n-----------\n\nsomething unexpected\n";
        let f = parse(&Root::new(s.root()), &out(1, report, "")).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].location, Location::file("pyproject.toml"));
        assert_eq!(f[0].rule.as_deref(), Some("My contract"));
    }

    fn chains_of(report: &str) -> Vec<(String, Vec<String>)> {
        read_report(report)
            .0
            .into_iter()
            .map(|c| (c.contract, c.hops.into_iter().map(|h| h.importer).collect()))
            .collect()
    }

    fn one(contract: &str, hops: &[&str]) -> (String, Vec<String>) {
        (
            contract.to_string(),
            hops.iter().map(|h| (*h).to_string()).collect(),
        )
    }

    #[test]
    fn chain_continuation_rules() {
        let head = "Broken contracts\n----------------\n\nC\n---\n";
        let with = |body: &str| chains_of(&format!("{head}{body}"));
        // An unmarked line right after a hop continues the chain.
        assert_eq!(with("- a -> b\n  b -> c\n"), [one("C", &["a", "b"])]);
        // A new marker starts a new chain even without a blank line.
        assert_eq!(
            with("- a -> b\n- c -> d\n"),
            [one("C", &["a"]), one("C", &["c"])]
        );
        // A blank line ends the chain; an unmarked hop after it starts another.
        assert_eq!(
            with("- a -> b\n\n  c -> d\n"),
            [one("C", &["a"]), one("C", &["c"])]
        );
        // So does a line ending in a colon.
        assert_eq!(
            with("- a -> b\nx is not allowed to import y:\n  c -> d\n"),
            [one("C", &["a"]), one("C", &["c"])]
        );
        // A heading starts over: the first hop of the next contract is its own chain.
        assert_eq!(
            with("- a -> b\nD\n---\n  c -> d\n"),
            [one("C", &["a"]), one("D", &["c"])]
        );
    }

    #[test]
    fn hops_before_the_broken_contracts_section_are_ignored() {
        let report = "Preamble\nAnalyzed 1 files.\n-----------------\n  a -> b (l.1)\n\nBroken contracts\n----------------\n\nC\n---\n- x -> y\n";
        assert_eq!(chains_of(report), [one("C", &["x"])]);
    }

    #[test]
    fn timeouts_and_kills_are_failures_even_with_output() {
        let root = Root::new(Path::new("/p"));
        let mut o = out(0, "Contracts: 1 kept, 0 broken.\n", "");
        o.timed_out = true;
        assert!(parse(&root, &o).is_err());
        let mut o = out(0, "Contracts: 1 kept, 0 broken.\n", "");
        o.exit_code = None;
        assert!(parse(&root, &o).is_err());
    }

    #[test]
    fn only_contracts_without_a_chain_are_reported_at_the_config() {
        let s = Scratch::new("imports-partial");
        s.write("pyproject.toml", "");
        let report = "A BROKEN\nB BROKEN\n\n----------------\nBroken contracts\n----------------\n\nA\n---\n\n- p.q -> r.s (l.3)\n";
        let f = parse(&Root::new(s.root()), &out(1, report, "")).unwrap();
        let rules: Vec<_> = f.iter().map(|f| f.rule.as_deref().unwrap()).collect();
        assert_eq!(rules, ["A", "B"]);
        assert_eq!(f[0].location, Location::line("p/q.py", 3));
        assert_eq!(f[1].location, Location::file("pyproject.toml"));
    }

    #[test]
    fn config_file_prefers_pyproject_then_the_dedicated_files() {
        let cfg = |files: &[&str]| {
            let s = Scratch::new("imports-cfg");
            for f in files {
                s.write(f, "");
            }
            config_file(&Root::new(s.root()))
        };
        assert_eq!(cfg(&[]), Path::new("pyproject.toml"));
        assert_eq!(cfg(&[".importlinter"]), Path::new(".importlinter"));
        assert_eq!(cfg(&["setup.cfg"]), Path::new("setup.cfg"));
        assert_eq!(
            cfg(&[".importlinter", "setup.cfg"]),
            Path::new(".importlinter")
        );
        assert_eq!(
            cfg(&[".importlinter", "pyproject.toml"]),
            Path::new("pyproject.toml")
        );
        assert_eq!(
            cfg(&["setup.cfg", "pyproject.toml"]),
            Path::new("pyproject.toml")
        );
    }

    #[test]
    fn hops() {
        let (starts, h) = hop("-   a.b -> c.d (l.7, l.9)").unwrap();
        assert!(starts);
        assert_eq!(
            (h.importer.as_str(), h.imported.as_str(), h.line),
            ("a.b", "c.d", Some(7))
        );
        let (starts, h) = hop("  c.d -> e.f").unwrap();
        assert!(!starts && h.line.is_none());
        assert!(hop("shop.domain is not allowed to import shop.infra:").is_none());
        assert!(hop("see the docs -> here").is_none());
        assert!(hop("a -> ").is_none());
    }
}
