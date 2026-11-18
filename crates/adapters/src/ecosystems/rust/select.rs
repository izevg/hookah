//! Rust test selection.
//!
//! Tests are selected at package level. If any changed file is a `.rs` file under the project
//! root or the project's `Cargo.toml`, the selection is this package plus the workspace members
//! that depend on it by path. Each target is a `--package=<name>` string, so the test command
//! can splice the targets in as one argv entry each. Anything else selects nothing.

use std::path::PathBuf;

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
    TestSelection::Targets(targets)
}

#[cfg(test)]
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

    #[test]
    fn other_files_select_nothing() {
        let changed = [
            "/ws/crates/core/README.md".into(),
            "/ws/crates/app/src/a.rs".into(),
        ];
        assert_eq!(select(&info(), &project(), &changed), TestSelection::None);
    }
}
