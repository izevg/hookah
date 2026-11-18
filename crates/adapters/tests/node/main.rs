//! End to end for the Node ecosystem: detect, run, and parse through the real process runner.
//!
//! Two kinds of tests:
//!
//! - hermetic ones put small shell scripts, which print output captured from the real tools, in
//!   `node_modules/.bin`. They exercise detection, `.bin` resolution, target expansion, process
//!   running, and parsing without Node installed;
//! - real-tool ones run the actual tools. They need Node and a directory that holds a
//!   `node_modules` with the tools. Set `HOOKAH_NODE_MODULES` to such a directory (for example the
//!   result of `npm i -D typescript eslint prettier @biomejs/biome vitest` in a scratch folder);
//!   without it they print a note and pass, so CI without network stays green.

// The fake tools are `sh` scripts, so this suite is Unix only; on Windows the shim lookup is
// covered by unit tests in `ecosystems::node::detect::package`.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use hookah_adapters::ecosystems::Ecosystems;
use hookah_adapters::ecosystems::cache::CacheConfig;
use hookah_adapters::process::ProcessRunner;
use hookah_adapters::store::cache_dir::project_cache_dir;
use hookah_application::ports::{CheckRunner, ToolchainDetector};
use hookah_domain::finding::{Finding, Severity};
use hookah_domain::project::{Ecosystem, Manifest, Project};
use hookah_domain::session::{ChangedLines, FileRole};
use hookah_domain::toolchain::{CheckKind, MutationSelection, TestSelection};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("hookah-node-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dunce::canonicalize(dir).unwrap())
    }

    fn write(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.0.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    /// A `node_modules/.bin/<name>` script that prints `stdout`, then exits with `code`.
    fn fake_bin(&self, package: &str, name: &str, stdout: &str, code: i32) {
        self.write(
            &format!("node_modules/{package}/package.json"),
            &format!(r#"{{"name":"{package}","version":"1.0.0"}}"#),
        );
        let script = format!("#!/bin/sh\ncat <<'HOOKAH_EOF'\n{stdout}\nHOOKAH_EOF\nexit {code}\n");
        let path = self.write(&format!("node_modules/.bin/{name}"), &script);
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn project(&self) -> Project {
        Project {
            root: self.0.clone(),
            ecosystem: Ecosystem::Node,
            manifests: vec![Manifest::from_path(&self.0.join("package.json")).unwrap()],
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Detection created a cache directory for this project; do not leave it behind.
        if let Ok(cache) = project_cache_dir(&self.0) {
            let _ = fs::remove_dir_all(cache);
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Detects the toolchain, runs the tool `name` of `kind` on `targets`, and parses its output.
fn check(
    project: &Project,
    kind: CheckKind,
    name: &str,
    targets: &[&str],
) -> Result<Vec<Finding>, hookah_application::error::PortError> {
    let eco = Ecosystems::with_cache(CacheConfig::disabled());
    let toolchain = eco.detect(project).unwrap();
    let tool = toolchain
        .tools()
        .iter()
        .find(|t| t.name == name && t.kind == kind)
        .unwrap_or_else(|| panic!("no {name} {kind:?} tool in {toolchain:?}"));
    let targets: Vec<String> = targets.iter().map(|t| (*t).to_owned()).collect();
    let out = ProcessRunner.run(&tool.command, &targets).unwrap();
    eco.parse(project, tool, &out)
}

fn at(f: &Finding) -> String {
    f.location.to_string()
}

const ESLINT_JSON: &str = r#"[{"filePath":"/x/src/c.js","messages":[{"ruleId":"no-var","severity":2,"message":"Unexpected var, use let or const instead.","line":1,"column":1}]}]"#;

#[test]
fn hermetic_lint_typecheck_and_test_through_fake_bins() {
    let s = Scratch::new("fake");
    s.write(
        "package.json",
        r#"{"devDependencies":{"eslint":"9","typescript":"5","vitest":"2"}}"#,
    );
    s.write("eslint.config.js", "export default [];\n");
    s.write("tsconfig.json", "{}\n");
    s.fake_bin(
        "eslint",
        "eslint",
        &ESLINT_JSON.replace("/x/", &format!("{}/", s.0.display())),
        1,
    );
    s.fake_bin(
        "typescript",
        "tsc",
        "src/a.ts(6,14): error TS2322: Type 'number' is not assignable to type 'string'.",
        2,
    );
    let report = s.write(
        "report.json",
        r#"{"success":false,"testResults":[{"name":"/x/src/a.test.ts","status":"failed","assertionResults":[{"status":"failed","fullName":"adds","failureMessages":["AssertionError: boom\n    at /x/src/a.test.ts:4:10"]}]}]}"#,
    );
    let report_text = fs::read_to_string(&report)
        .unwrap()
        .replace("/x/", &format!("{}/", s.0.display()));
    fs::write(&report, report_text).unwrap();
    s.fake_bin(
        "vitest",
        "vitest",
        &format!("JSON report written to {}", report.display()),
        1,
    );
    let project = s.project();

    let lint = check(&project, CheckKind::Lint, "eslint", &["src/c.js"]).unwrap();
    assert_eq!(lint.len(), 1, "{lint:?}");
    assert_eq!(at(&lint[0]), "src/c.js:1:1");
    assert_eq!(lint[0].rule.as_deref(), Some("no-var"));

    let types = check(&project, CheckKind::Typecheck, "tsc", &[]).unwrap();
    assert_eq!(at(&types[0]), "src/a.ts:6:14");
    assert_eq!(types[0].severity, Severity::Error);

    let eco = Ecosystems::with_cache(CacheConfig::disabled());
    let changed = [s.0.join("src/a.ts")];
    let TestSelection::Targets(targets) = eco.select_tests(&project, &changed).unwrap() else {
        panic!("expected targets");
    };
    assert_eq!(targets, [s.0.join("src/a.ts").to_string_lossy()]);
    let refs: Vec<&str> = targets.iter().map(String::as_str).collect();
    let tests = check(&project, CheckKind::Test, "vitest", &refs).unwrap();
    assert_eq!(tests.len(), 1, "{tests:?}");
    assert_eq!(at(&tests[0]), "src/a.test.ts:4:10");
    assert_eq!(tests[0].kind, CheckKind::Test);
}

#[test]
fn crashing_and_missing_tools_never_pass_silently() {
    let s = Scratch::new("crash");
    s.write("package.json", r#"{"devDependencies":{"typescript":"5"}}"#);
    s.write("tsconfig.json", "{}\n");
    s.fake_bin("typescript", "tsc", "internal compiler error", 134);
    let err = check(&s.project(), CheckKind::Typecheck, "tsc", &[]).unwrap_err();
    assert!(err.message().contains("internal compiler error"), "{err}");

    // Declared but not installed: no tool at all, and no complaint either.
    let s = Scratch::new("absent");
    s.write("package.json", r#"{"devDependencies":{"typescript":"5"}}"#);
    s.write("tsconfig.json", "{}\n");
    assert!(
        Ecosystems::with_cache(CacheConfig::disabled())
            .detect(&s.project())
            .unwrap()
            .tools()
            .is_empty()
    );
}

#[test]
fn roles_mutation_targets_and_missing_stryker() {
    let s = Scratch::new("roles");
    s.write("package.json", r#"{"devDependencies":{"jest":"29"}}"#);
    s.fake_bin("jest", "jest", "{}", 0);
    s.write("src/foo.ts", "export const x = 1;\n");
    let test = s.write("src/foo.test.ts", "");
    let project = s.project();
    let eco = Ecosystems::with_cache(CacheConfig::disabled());

    assert_eq!(
        eco.classify(&project, &test, &ChangedLines::All),
        FileRole::Test
    );
    assert_eq!(
        eco.classify(&project, &s.0.join("src/foo.ts"), &ChangedLines::All),
        FileRole::Source
    );
    assert_eq!(
        eco.select_mutants(&project, &[test], &[]),
        MutationSelection::Targets(vec!["src/foo.ts".into()])
    );
    let toolchain = eco.detect(&project).unwrap();
    assert_eq!(toolchain.missing().len(), 1);
    assert_eq!(toolchain.missing()[0].kind, CheckKind::Mutation);
    assert!(
        toolchain.missing()[0]
            .hint
            .ends_with("@stryker-mutator/jest-runner")
    );
}

/// The node_modules to use for the real-tool tests, or `None` (with a note) to skip them.
fn real_modules() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("HOOKAH_NODE_MODULES")?).join("node_modules");
    if !dir.join(".bin").is_dir() || !Path::new("/usr/bin/env").exists() {
        eprintln!("skipping: HOOKAH_NODE_MODULES has no node_modules/.bin");
        return None;
    }
    let node_ok = std::process::Command::new("node")
        .arg("--version")
        .output()
        .is_ok();
    if !node_ok {
        eprintln!("skipping: node is not on PATH");
        return None;
    }
    Some(dunce::canonicalize(dir).unwrap())
}

/// A project that reuses the tools of the shared `node_modules`.
fn real_project(name: &str, modules: &Path, manifest: &str) -> Scratch {
    let s = Scratch::new(name);
    s.write("package.json", manifest);
    symlink(modules, s.0.join("node_modules")).unwrap();
    s
}

#[test]
fn real_tsc_eslint_prettier_and_vitest() {
    let Some(modules) = real_modules() else {
        eprintln!("real_tsc_eslint_prettier_and_vitest: set HOOKAH_NODE_MODULES to run");
        return;
    };
    let has = |bin: &str| modules.join(".bin").join(bin).exists();
    let s = real_project(
        "real",
        &modules,
        r#"{"type":"module","devDependencies":{"typescript":"*","eslint":"*","prettier":"*","vitest":"*"}}"#,
    );
    s.write("tsconfig.json", r#"{"compilerOptions":{"strict":true,"module":"esnext","target":"es2022","moduleResolution":"bundler","types":[],"noEmit":true},"include":["src"]}"#);
    s.write(
        "eslint.config.js",
        "export default [{ rules: { 'no-var': 'error' } }];\n",
    );
    s.write("src/a.ts", "export const s: string = 5;\n");
    s.write("src/c.js", "var x = 1;\nexport const y = x;\n");
    s.write("src/ugly.ts", "export const a = {\n  b: 1 }\n");
    s.write(
        "src/add.ts",
        "export function add(a: number, b: number): number {\n  return a + b;\n}\n",
    );
    s.write(
        "src/add.test.ts",
        "import { expect, test } from 'vitest';\nimport { add } from './add';\ntest('adds', () => { expect(add(1, 2)).toBe(3); });\ntest('fails', () => { expect(add(1, 1)).toBe(3); });\n",
    );
    let project = s.project();

    if has("tsc") {
        let found = check(&project, CheckKind::Typecheck, "tsc", &[]).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(at(&found[0]), "src/a.ts:1:14");
        assert_eq!(found[0].rule.as_deref(), Some("TS2322"));
    }
    if has("eslint") {
        let found = check(&project, CheckKind::Lint, "eslint", &["src/c.js"]).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(at(&found[0]), "src/c.js:1:1");
        assert_eq!(found[0].rule.as_deref(), Some("no-var"));
    }
    if has("prettier") {
        let found = check(&project, CheckKind::Format, "prettier", &["src/ugly.ts"]).unwrap();
        assert!(found.is_empty(), "{found:?}");
        assert_eq!(
            fs::read_to_string(s.0.join("src/ugly.ts")).unwrap(),
            "export const a = {\n  b: 1,\n};\n"
        );
    }
    if has("vitest") {
        let found = check(&project, CheckKind::Test, "vitest", &["src/add.ts"]).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(at(&found[0]), "src/add.test.ts:4:41");
        assert_eq!(found[0].kind, CheckKind::Test);
    }
}

#[test]
fn real_biome_lints_and_formats() {
    let Some(modules) = real_modules() else {
        eprintln!("real_biome_lints_and_formats: set HOOKAH_NODE_MODULES to run");
        return;
    };
    if !modules.join(".bin/biome").exists() {
        eprintln!("skipping: no biome in HOOKAH_NODE_MODULES");
        return;
    }
    let s = real_project(
        "biome",
        &modules,
        r#"{"devDependencies":{"@biomejs/biome":"*"}}"#,
    );
    s.write("biome.json", r#"{"linter":{"enabled":true}}"#);
    s.write(
        "src/a.ts",
        "export function f() {\n  let unused = 1;\n  var x = 2\n  return x;\n}\n",
    );
    let project = s.project();
    let lint = check(&project, CheckKind::Lint, "biome", &["src/a.ts"]).unwrap();
    assert!(
        lint.iter().any(
            |f| f.rule.as_deref() == Some("lint/correctness/noUnusedVariables")
                && at(f).starts_with("src/a.ts:2:7")
        ),
        "{lint:?}"
    );
    let fmt = check(&project, CheckKind::Format, "biome", &["src/a.ts"]).unwrap();
    assert!(fmt.is_empty(), "{fmt:?}");
    assert!(
        fs::read_to_string(s.0.join("src/a.ts"))
            .unwrap()
            .contains("\tlet unused")
    );
}
