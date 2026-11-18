//! End to end: detect, run, and parse with real Python tools on a small src-layout project.
//!
//! These tests need a virtual environment holding ruff, mypy, import-linter, pytest, and mutmut 3.
//! They look for one in `$HOOKAH_TEST_VENV`, then in `target/py/.venv` of this workspace, and
//! skip (passing) when there is none, so machines and CI runs without Python tooling stay green.
//! To create it:
//!
//! ```sh
//! uv venv target/py/.venv
//! uv pip install --python target/py/.venv ruff mypy import-linter pytest mutmut
//! ```
//!
//! The venv is symlinked as `.venv` into a scratch project, which is how hookah finds it.

#![cfg(test)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;

use hookah_application::ports::{CheckRunner, ToolchainDetector};
use hookah_domain::finding::Finding;
use hookah_domain::project::{Ecosystem, Manifest, Project};
use hookah_domain::toolchain::{Arg, CheckKind, MutationSelection, TestSelection};

use super::PythonEcosystem;
use super::fixture::Scratch;
use crate::process::ProcessRunner;

const PYPROJECT: &str = r#"[project]
name = "shop"
version = "0.1.0"

[tool.mypy]
strict = true

[tool.importlinter]
root_package = "shop"

[[tool.importlinter.contracts]]
name = "Domain must not import infra"
type = "forbidden"
source_modules = ["shop.domain"]
forbidden_modules = ["shop.infra"]

[tool.mutmut]
source_paths = ["src"]

[tool.pytest.ini_options]
pythonpath = ["src"]
"#;

const CART: &str = "import os\nfrom shop.infra.db import connect\n\n\ndef total(items: list[int]) -> int:\n    x: int = \"a\"\n    return sum(items) + 1 if connect() else 0\n";

const TESTS: &str = "from shop.domain.cart import total\n\n\ndef test_total():\n    assert total([1, 2]) == 4\n\n\ndef test_empty():\n    assert total([]) == 2\n";

fn real_venv() -> Option<PathBuf> {
    let venv = std::env::var_os("HOOKAH_TEST_VENV")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/py/.venv"));
    let venv = dunce::canonicalize(venv).ok()?;
    ["ruff", "mypy", "lint-imports", "pytest", "mutmut"]
        .iter()
        .all(|t| venv.join("bin").join(t).is_file())
        .then_some(venv)
}

struct Fixture {
    scratch: Scratch,
    project: Project,
    eco: PythonEcosystem,
}

fn fixture(name: &str) -> Option<Fixture> {
    let Some(venv) = real_venv() else {
        eprintln!("skipping {name}: no Python tool venv (see the module docs)");
        return None;
    };
    let scratch = Scratch::new(name);
    symlink(venv, scratch.root().join(".venv")).unwrap();
    scratch.write("pyproject.toml", PYPROJECT);
    for init in ["shop", "shop/domain", "shop/infra"] {
        scratch.write(&format!("src/{init}/__init__.py"), "");
    }
    scratch.write(
        "src/shop/infra/db.py",
        "def connect() -> str:\n    return \"db\"\n",
    );
    scratch.write("src/shop/domain/cart.py", CART);
    scratch.write("tests/test_cart.py", TESTS);
    let project = Project {
        root: scratch.root().to_path_buf(),
        ecosystem: Ecosystem::Python,
        manifests: vec![Manifest::from_path(&scratch.root().join("pyproject.toml")).unwrap()],
    };
    Some(Fixture {
        scratch,
        project,
        eco: PythonEcosystem::new(),
    })
}

impl Fixture {
    /// Runs the tool named `name` on `targets` and parses its output.
    fn check(&self, name: &str, targets: &[&str]) -> Vec<Finding> {
        let toolchain = self.eco.detect(&self.project).unwrap();
        let tool = toolchain
            .tools()
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("no tool {name}: {:?}", toolchain.tools()));
        let targets: Vec<String> = targets.iter().map(|t| (*t).to_string()).collect();
        let out = ProcessRunner.run(&tool.command, &targets).unwrap();
        self.eco
            .parse(&self.project, tool, &out)
            .unwrap_or_else(|e| panic!("{name}: {e}"))
    }
}

fn lines(findings: &[Finding]) -> Vec<String> {
    findings
        .iter()
        .map(|f| match &f.rule {
            Some(r) => format!("{}: {} [{r}]", f.location, f.message),
            None => format!("{}: {}", f.location, f.message),
        })
        .collect()
}

#[test]
fn detects_the_full_toolchain_in_the_venv() {
    let Some(f) = fixture("py-e2e-detect") else {
        return;
    };
    let tc = f.eco.detect(&f.project).unwrap();
    let names: Vec<&str> = tc.tools().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "ruff-format",
            "ruff",
            "mypy",
            "import-linter",
            "pytest",
            "mutmut"
        ]
    );
    assert!(tc.missing().is_empty());
}

#[test]
fn lint_and_typecheck_report_real_findings() {
    let Some(f) = fixture("py-e2e-static") else {
        return;
    };
    let file = "src/shop/domain/cart.py";
    let lint = lines(&f.check("ruff", &[file]));
    assert!(
        lint.iter()
            .any(|l| l.starts_with("src/shop/domain/cart.py:1:8: `os` imported but unused [F401]")),
        "{lint:?}"
    );
    let types = f.check("mypy", &[file]);
    assert!(
        types.iter().any(|t| t.kind == CheckKind::Typecheck
            && t.location.path.ends_with("cart.py")
            && t.location.line == Some(6)
            && t.rule.as_deref() == Some("assignment")),
        "{:?}",
        lines(&types)
    );
}

#[test]
fn format_rewrites_the_file_and_reports_parse_errors() {
    let Some(f) = fixture("py-e2e-format") else {
        return;
    };
    assert!(
        f.check("ruff-format", &["src/shop/domain/cart.py"])
            .is_empty()
    );
    let text = fs::read_to_string(f.scratch.root().join("src/shop/domain/cart.py")).unwrap();
    assert!(
        text.contains("\n\n\ndef total") && !text.contains("\n\n\n\n"),
        "{text}"
    );
    f.scratch.write("src/shop/bad.py", "def f(:\n    pass\n");
    let found = f.check("ruff-format", &["src/shop/bad.py"]);
    assert_eq!(found.len(), 1, "{:?}", lines(&found));
    assert_eq!(found[0].kind, CheckKind::Format);
    assert_eq!(found[0].location.line, Some(1));
}

#[test]
fn import_linter_reports_the_illegal_import_at_its_line() {
    let Some(f) = fixture("py-e2e-imports") else {
        return;
    };
    let found = lines(&f.check("import-linter", &["src/shop/domain/cart.py"]));
    assert_eq!(
        found,
        [
            "src/shop/domain/cart.py:2: shop.domain.cart -> shop.infra.db [Domain must not import infra]"
        ]
    );
}

#[test]
fn pytest_failures_are_located_at_the_test_line() {
    let Some(f) = fixture("py-e2e-pytest") else {
        return;
    };
    let selection = f
        .eco
        .select_tests(
            &f.project,
            &[f.scratch.root().join("src/shop/domain/cart.py")],
        )
        .unwrap();
    // With pytest-testmon in the venv, testmon picks the tests and the targets are the changed
    // files; otherwise the test file is chosen by name.
    let testmon = f
        .eco
        .detect(&f.project)
        .unwrap()
        .of_kind(CheckKind::Test)
        .any(|t| t.command.args.contains(&Arg::Literal("--testmon".into())));
    let expected = if testmon {
        "src/shop/domain/cart.py"
    } else {
        "tests/test_cart.py"
    };
    assert_eq!(selection, TestSelection::Targets(vec![expected.into()]));
    let TestSelection::Targets(targets) = selection else {
        panic!("nothing selected");
    };
    let targets: Vec<&str> = targets.iter().map(String::as_str).collect();
    let found = lines(&f.check("pytest", &targets));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].starts_with("tests/test_cart.py:9: test_empty failed: assert 1 == 2"),
        "{found:?}"
    );
}

#[test]
fn mutmut_survivors_are_reported_at_source_lines() {
    let Some(f) = fixture("py-e2e-mutmut") else {
        return;
    };
    let test = f.scratch.root().join("tests/test_cart.py");
    let sel = f.eco.select_mutants(&f.project, &[test], &[]);
    let MutationSelection::Targets(targets) = sel else {
        panic!("nothing selected");
    };
    assert_eq!(targets, ["shop.domain.cart.x[_ǁ]*"]);
    // The baseline must pass for mutmut to run.
    f.scratch
        .write("tests/test_cart.py", &TESTS.replace("== 2", "== 1"));
    let target_refs: Vec<&str> = targets.iter().map(String::as_str).collect();
    let found = f.check("mutmut", &target_refs);
    assert!(!found.is_empty(), "the weak tests leave mutants alive");
    assert!(found.iter().all(|m| m.kind == CheckKind::Mutation));
    // `else 0` -> `else 1` survives because `connect()` is always truthy.
    let survivor = found
        .iter()
        .find(|m| {
            m.message
                .contains("`return sum(items) + 1 if connect() else 0` -> ")
        })
        .unwrap_or_else(|| panic!("{:?}", lines(&found)));
    assert_eq!(survivor.location.line, Some(7));
    assert_eq!(
        survivor.location.path,
        PathBuf::from("src/shop/domain/cart.py")
    );
}
