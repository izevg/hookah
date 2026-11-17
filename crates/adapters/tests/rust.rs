//! End to end: detect, run, and parse on a tiny temporary crate with real cargo.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use hookah_adapters::ecosystems::Ecosystems;
use hookah_adapters::process::ProcessRunner;
use hookah_application::ports::{CheckRunner, ToolchainDetector};
use hookah_domain::finding::{Finding, Severity};
use hookah_domain::project::{Ecosystem, Manifest, Project};
use hookah_domain::toolchain::{CheckKind, TestSelection};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("hookah-rust-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"scratch\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
        )
        .unwrap();
        Self(dir.canonicalize().unwrap())
    }

    fn write(&self, rel: &str, text: &str) {
        fs::write(self.0.join(rel), text).unwrap();
    }

    fn project(&self) -> Project {
        Project {
            root: self.0.clone(),
            ecosystem: Ecosystem::Rust,
            manifests: vec![Manifest::from_path(&self.0.join("Cargo.toml")).unwrap()],
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Detects the toolchain, runs the tool named `name` on `targets`, and parses its output.
fn check(
    project: &Project,
    name: &str,
    targets: &[String],
) -> Result<Vec<Finding>, hookah_application::error::PortError> {
    let eco = Ecosystems::new();
    let toolchain = eco.detect(project).unwrap();
    let tool = toolchain.tools().iter().find(|t| t.name == name).unwrap();
    let out = ProcessRunner.run(&tool.command, targets).unwrap();
    eco.parse(project, tool, &out)
}

fn at(f: &Finding) -> String {
    f.location.to_string()
}

#[test]
fn warning_and_failing_test() {
    let s = Scratch::new("warn");
    s.write(
        "src/lib.rs",
        "pub fn f() -> i32 {\n    return 1;\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn fails() {\n        assert_eq!(super::f(), 2);\n    }\n}\n",
    );
    let project = s.project();

    let lint = check(&project, "clippy", &[]).unwrap();
    assert_eq!(lint.len(), 1, "{lint:?}");
    assert_eq!(at(&lint[0]), "src/lib.rs:2:5");
    assert_eq!(lint[0].severity, Severity::Warning);
    assert_eq!(lint[0].rule.as_deref(), Some("clippy::needless_return"));

    let eco = Ecosystems::new();
    let sel = eco
        .select_tests(&project, &[s.0.join("src/lib.rs")])
        .unwrap();
    let TestSelection::Targets(targets) = sel else {
        panic!("expected targets");
    };
    assert_eq!(targets, ["--package=scratch"]);
    let tests = check(&project, "cargo-test", &targets).unwrap();
    assert_eq!(tests.len(), 1, "{tests:?}");
    assert_eq!(at(&tests[0]), "src/lib.rs:9:9");
    assert_eq!(tests[0].kind, CheckKind::Test);
    assert!(tests[0].message.starts_with("test tests::fails failed: "));

    assert_eq!(
        eco.select_tests(&project, &[s.0.join("README.md")])
            .unwrap(),
        TestSelection::None
    );
}

#[test]
fn compile_error() {
    let s = Scratch::new("err");
    s.write("src/lib.rs", "pub fn f() -> i32 {\n    missing\n}\n");
    let project = s.project();
    let lint = check(&project, "clippy", &[]).unwrap();
    assert_eq!(lint.len(), 1, "{lint:?}");
    assert_eq!(at(&lint[0]), "src/lib.rs:2:5");
    assert_eq!(lint[0].severity, Severity::Error);
    assert_eq!(lint[0].rule.as_deref(), Some("E0425"));
    let tests = check(&project, "cargo-test", &["--package=scratch".into()]).unwrap();
    assert_eq!(tests.len(), 1, "{tests:?}");
    assert_eq!(tests[0].kind, CheckKind::Test);
}

#[test]
fn rustfmt_formats_and_reports_syntax_errors() {
    let s = Scratch::new("fmt");
    s.write("src/lib.rs", "pub fn   f( )->i32{1}\n");
    let project = s.project();
    let found = check(&project, "rustfmt", &["src/lib.rs".into()]).unwrap();
    assert!(found.is_empty());
    assert_eq!(
        fs::read_to_string(s.0.join("src/lib.rs")).unwrap(),
        "pub fn f() -> i32 {\n    1\n}\n"
    );

    s.write("src/lib.rs", "pub fn f( {\n");
    let found = check(&project, "rustfmt", &["src/lib.rs".into()]).unwrap();
    assert!(!found.is_empty());
    assert_eq!(found[0].severity, Severity::Error);
    assert_eq!(found[0].location.path, Path::new("src/lib.rs"));
}

#[test]
fn virtual_workspace_has_no_tools() {
    let s = Scratch::new("virt");
    s.write("Cargo.toml", "[workspace]\nmembers = []\n");
    let toolchain = Ecosystems::new().detect(&s.project()).unwrap();
    assert!(toolchain.is_empty());
}
