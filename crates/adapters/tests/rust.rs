//! End to end: detect, run, and parse on a tiny temporary crate with real cargo.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use hookah_adapters::ecosystems::Ecosystems;
use hookah_adapters::ecosystems::cache::CacheConfig;
use hookah_adapters::process::ProcessRunner;
use hookah_application::ports::{CheckRunner, ToolchainDetector};
use hookah_domain::finding::{Finding, Severity};
use hookah_domain::project::{Ecosystem, Manifest, Project};
use hookah_domain::session::{ChangedLines, FileRole, LineRange};
use hookah_domain::toolchain::{CheckKind, MutationSelection, TestSelection};

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
        Self(dunce::canonicalize(dir).unwrap())
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
    let eco = Ecosystems::with_cache(CacheConfig::disabled());
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
    assert_eq!(lint[0].kind, CheckKind::Lint);

    let eco = Ecosystems::with_cache(CacheConfig::disabled());
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
    let toolchain = Ecosystems::with_cache(CacheConfig::disabled())
        .detect(&s.project())
        .unwrap();
    assert!(toolchain.is_empty());
}

fn mutants_installed(root: &Path) -> bool {
    std::process::Command::new("cargo")
        .args(["mutants", "--version"])
        .current_dir(root)
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn weak_test_lets_a_mutant_survive() {
    let s = Scratch::new("mutants");
    if !mutants_installed(&s.0) {
        eprintln!("skipped: cargo-mutants is not installed");
        return;
    }
    s.write(
        "src/lib.rs",
        "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn weak() {\n        assert_eq!(super::add(0, 0), 0);\n    }\n}\n",
    );
    let project = s.project();
    let eco = Ecosystems::with_cache(CacheConfig::disabled());

    let toolchain = eco.detect(&project).unwrap();
    assert!(toolchain.tools().iter().any(|t| t.name == "cargo-mutants"));
    assert!(toolchain.missing().is_empty());

    let lib = s.0.join("src/lib.rs");
    let role = eco.classify(
        &project,
        &lib,
        &ChangedLines::Ranges(vec![LineRange::new(8, 10).unwrap()]),
    );
    assert_eq!(role, FileRole::Test);

    let MutationSelection::Targets(targets) = eco.select_mutants(&project, &[lib], &[]) else {
        panic!("expected targets");
    };
    assert_eq!(targets, ["--file=src/lib.rs"]);

    let found = check(&project, "cargo-mutants", &targets).unwrap();
    assert!(!found.is_empty(), "{found:?}");
    assert!(found.iter().all(|f| f.kind == CheckKind::Mutation));
    assert!(found.iter().all(|f| f.severity == Severity::Error));
    assert!(found.iter().all(|f| at(f) == "src/lib.rs:2"), "{found:?}");
    assert!(
        found
            .iter()
            .any(|f| f.message == "mutant survived: replace + with - in add"),
        "{found:?}"
    );

    // A strong test catches everything; the iterated run re-tests only what is left.
    s.write(
        "src/lib.rs",
        "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn strong() {\n        assert_eq!(super::add(2, 3), 5);\n        assert_eq!(super::add(0, 0), 0);\n    }\n}\n",
    );
    let found = check(&project, "cargo-mutants", &targets).unwrap();
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn missing_cargo_mutants_is_reported_only_for_crates_with_tests() {
    let s = Scratch::new("nomutants");
    s.write("src/lib.rs", "pub fn f() {}\n");
    let project = s.project();
    let eco = Ecosystems::with_cache(CacheConfig::disabled());
    let toolchain = eco.detect(&project).unwrap();
    assert!(toolchain.missing().is_empty());
    s.write("src/lib.rs", "pub fn f() {}\n\n#[test]\nfn t() {}\n");
    let toolchain = eco.detect(&project).unwrap();
    let has_tool = toolchain.tools().iter().any(|t| t.name == "cargo-mutants");
    // Either the tool is installed (and offered) or it is reported missing, never both.
    assert_eq!(has_tool, toolchain.missing().is_empty());
}

/// The detection cache with real `cargo metadata`: warm calls hit, and a manifest edit misses.
#[test]
fn detection_is_cached_until_the_manifest_changes() {
    use hookah_adapters::ecosystems::cache::Origin;

    let s = Scratch::new("cache");
    s.write("src/lib.rs", "pub fn f() {}\n");
    let cache = std::env::temp_dir().join(format!("hookah-rust-entries-{}", std::process::id()));
    let _ = fs::remove_dir_all(&cache);
    fs::create_dir_all(&cache).unwrap();
    let lookup = |key: &str| {
        (key != "HOOKAH_NO_CACHE")
            .then(|| std::env::var_os(key))
            .flatten()
    };
    let eco = || Ecosystems::with_cache(CacheConfig::from_lookup(lookup).in_dir(cache.clone()));
    let project = s.project();

    let (cold, origin) = eco().detect_traced(&project).unwrap();
    assert_eq!(origin, Origin::Miss);
    // The workspace members are stored with the entry as derived inputs.
    let stored = fs::read_to_string(cache.join("detect-rust.json")).unwrap();
    assert!(
        stored.contains(&format!("{:?}", s.0.join("Cargo.toml"))),
        "{stored}"
    );
    assert!(cache.join("metadata.json").is_file());
    assert!(cache.join("detect-rust.json").is_file());
    let (warm, origin) = eco().detect_traced(&project).unwrap();
    assert_eq!(origin, Origin::Hit);
    assert_eq!(warm, cold);
    let edition =
        |tc: &hookah_domain::toolchain::Toolchain| tc.tools()[0].command.expand_args(&[]).join(" ");
    assert_eq!(edition(&warm), "--edition 2021");

    // Without the detection entry, `cargo metadata` still comes from its own entry.
    fs::remove_file(cache.join("detect-rust.json")).unwrap();
    let (again, origin) = eco().detect_traced(&project).unwrap();
    assert_eq!((origin, &again), (Origin::Miss, &cold));
    // A corrupt metadata entry is recomputed.
    fs::write(cache.join("metadata.json"), "{ not json").unwrap();
    fs::remove_file(cache.join("detect-rust.json")).unwrap();
    assert_eq!(eco().detect_traced(&project).unwrap().1, Origin::Miss);
    assert_eq!(eco().detect_traced(&project).unwrap().1, Origin::Hit);

    // Editing the manifest invalidates both entries; the new edition shows.
    s.write(
        "Cargo.toml",
        "[package]\nname = \"scratch\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
    );
    let (edited, origin) = eco().detect_traced(&project).unwrap();
    assert_eq!(origin, Origin::Miss);
    assert_eq!(edition(&edited), "--edition 2024");
    assert_eq!(eco().detect_traced(&project).unwrap().1, Origin::Hit);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn rust_inputs_name_the_manifest_the_members_and_the_path_tools() {
    use hookah_adapters::ecosystems::cache::Input;
    use hookah_adapters::ecosystems::rust::RustEcosystem;

    let s = Scratch::new("inputs");
    s.write("src/lib.rs", "pub fn f() {}\n");
    let project = s.project();
    let rust = RustEcosystem::with_cache(CacheConfig::from_lookup(|key| {
        (key == "PATH").then(|| "/some/bin".into())
    }));
    let manifest = Input::File(s.0.join("Cargo.toml"));
    let detection = rust.detection_inputs(&project);
    assert!(detection.contains(&manifest));
    assert!(detection.contains(&Input::File(PathBuf::from("/some/bin/cargo-mutants"))));
    assert!(rust.derived_inputs(&project).contains(&manifest));
}
