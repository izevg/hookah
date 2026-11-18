//! Rust test selection.
//!
//! Tests are selected at package level. If any changed file is a `.rs` file under the project
//! root or the project's `Cargo.toml`, the selection is this package plus the workspace members
//! that depend on it by path. Each target is a `--package=<name>` string, so the test command
//! can splice the targets in as one argv entry each. Anything else selects nothing.
//!
//! **Light tests only.** `cargo test --package=<name>` runs every test binary of the package:
//! unit tests of the library and binaries, each `tests/*.rs` integration target, and the
//! doctests. Heavy integration targets (`tests/integration_*.rs`, `tests/e2e/main.rs`, see
//! [`TestWeight`]) must not run, and cargo has no flag to skip a target, only to pick targets.
//! So when the package has a heavy target, the selection becomes explicit: `--lib` and `--bins`
//! (when the package has them) plus one `--test=<name>` per light integration target. The flags
//! apply to the dependents in the selection as well. Two costs, accepted: doctests do not run
//! in that mode (cargo cannot combine `--doc` with other target flags), and only the package's
//! own `tests/` directory is inspected, not the dependents'. Without a heavy target the
//! selection stays the plain `--package` list, so nothing changes for most crates. Benches are
//! never run: `cargo test` skips `benches/` unless a bench sets `test = true`, and the explicit
//! mode names no bench either.

use std::path::{Path, PathBuf};

use hookah_domain::light::TestWeight;
use hookah_domain::project::Project;
use hookah_domain::toolchain::TestSelection;

use super::detect::PackageInfo;

/// The `cargo test` target string for one package.
pub fn package_target(name: &str) -> String {
    format!("--package={name}")
}

/// Chooses the packages to test for `changed` (absolute paths).
pub fn select(info: &PackageInfo, project: &Project, changed: &[PathBuf]) -> TestSelection {
    let manifest = project.root.join("Cargo.toml");
    let relevant = changed.iter().any(|p| {
        project.contains(p) && (p.extension().is_some_and(|e| e == "rs") || *p == manifest)
    });
    if !relevant {
        return TestSelection::None;
    }
    let mut targets = vec![package_target(&info.name)];
    targets.extend(info.dependents.iter().map(|d| package_target(d)));
    let layout = Layout::read(&project.root);
    if layout.tests.iter().any(|(_, weight)| weight.is_heavy()) {
        match layout.light_flags() {
            Some(flags) => targets.extend(flags),
            None => return TestSelection::None,
        }
    }
    TestSelection::Targets(targets)
}

/// What the package has to test, read from the file system.
struct Layout {
    lib: bool,
    bins: bool,
    /// The integration test targets: name and weight.
    tests: Vec<(String, TestWeight)>,
}

impl Layout {
    fn read(root: &Path) -> Self {
        let mut tests = Vec::new();
        for entry in std::fs::read_dir(root.join("tests"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            let (name, is_target) = if path.is_dir() {
                (entry.file_name(), path.join("main.rs").is_file())
            } else {
                let is_rs = path.extension().is_some_and(|e| e == "rs");
                (path.file_stem().unwrap_or_default().to_owned(), is_rs)
            };
            if is_target {
                let weight = TestWeight::of_path(&Path::new("tests").join(entry.file_name()));
                tests.push((name.to_string_lossy().into_owned(), weight));
            }
        }
        tests.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            lib: root.join("src/lib.rs").is_file(),
            bins: root.join("src/main.rs").is_file() || root.join("src/bin").is_dir(),
            tests,
        }
    }

    /// The flags that run the light targets only; `None` when there are none.
    fn light_flags(&self) -> Option<Vec<String>> {
        let mut flags = Vec::new();
        if self.lib {
            flags.push("--lib".to_string());
        }
        if self.bins {
            flags.push("--bins".to_string());
        }
        flags.extend(
            self.tests
                .iter()
                .filter(|(_, weight)| !weight.is_heavy())
                .map(|(name, _)| format!("--test={name}")),
        );
        (!flags.is_empty()).then_some(flags)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use hookah_domain::project::Ecosystem;

    use super::*;

    fn project() -> Project {
        Project {
            root: "/ws/crates/core".into(),
            ecosystem: Ecosystem::Rust,
            manifests: vec![],
        }
    }

    fn info() -> PackageInfo {
        PackageInfo {
            name: "core-lib".into(),
            edition: "2024".into(),
            workspace_root: "/ws".into(),
            dependents: vec!["app".into()],
            members: vec![],
        }
    }

    #[test]
    fn rust_files_and_manifest_select_package_and_dependents() {
        for f in ["/ws/crates/core/src/a.rs", "/ws/crates/core/Cargo.toml"] {
            assert_eq!(
                select(&info(), &project(), &[f.into()]),
                TestSelection::Targets(vec!["--package=core-lib".into(), "--package=app".into()])
            );
        }
    }

    /// A package directory with the given files, empty, in a fresh temporary directory.
    fn package(name: &str, files: &[&str]) -> (PathBuf, Project) {
        let root = std::env::temp_dir().join(format!("hookah-rs-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for f in files {
            let path = root.join(f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        let project = Project {
            root: root.clone(),
            ecosystem: Ecosystem::Rust,
            manifests: vec![],
        };
        (root, project)
    }

    fn targets(info: &PackageInfo, project: &Project) -> Vec<String> {
        let changed = [project.root.join("src/a.rs")];
        match select(info, project, &changed) {
            TestSelection::Targets(t) => t,
            TestSelection::None => vec![],
        }
    }

    #[test]
    fn light_integration_targets_keep_the_plain_package_selection() {
        let (_r, p) = package("light", &["src/lib.rs", "tests/api.rs", "tests/db/main.rs"]);
        assert_eq!(
            targets(&info(), &p),
            ["--package=core-lib", "--package=app"]
        );
        let (_r, p) = package("none", &["src/lib.rs"]);
        assert_eq!(
            targets(&info(), &p),
            ["--package=core-lib", "--package=app"]
        );
    }

    #[test]
    fn a_heavy_integration_target_makes_the_selection_explicit() {
        let (_r, p) = package(
            "heavy",
            &[
                "src/lib.rs",
                "src/main.rs",
                "tests/api.rs",
                "tests/integration_db.rs",
                "tests/e2e/main.rs",
                "tests/checkout_e2e.rs",
                "tests/helpers/mod.rs",
                "tests/notes.txt",
                "tests/unit/main.rs",
            ],
        );
        assert_eq!(
            targets(&info(), &p),
            [
                "--package=core-lib",
                "--package=app",
                "--lib",
                "--bins",
                "--test=api",
                "--test=unit",
            ]
        );
    }

    #[test]
    fn explicit_mode_names_only_what_the_package_has() {
        let (_r, p) = package("bin-only", &["src/bin/x.rs", "tests/e2e.rs"]);
        assert_eq!(
            targets(&info(), &p),
            ["--package=core-lib", "--package=app", "--bins"]
        );
        let (_r, p) = package("lib-only", &["src/lib.rs", "tests/e2e/main.rs"]);
        assert_eq!(
            targets(&info(), &p),
            ["--package=core-lib", "--package=app", "--lib"]
        );
        // Nothing light to run at all.
        let (_r, p) = package("nothing", &["tests/e2e.rs"]);
        assert!(targets(&info(), &p).is_empty());
    }

    #[test]
    fn other_files_select_nothing() {
        let changed = [
            "/ws/crates/core/README.md".into(),
            "/ws/crates/app/src/a.rs".into(),
        ];
        assert_eq!(select(&info(), &project(), &changed), TestSelection::None);
    }
}
