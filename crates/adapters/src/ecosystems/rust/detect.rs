//! Rust toolchain detection.
//!
//! The project root is the directory of the nearest `Cargo.toml`, normally a crate inside a
//! workspace. `cargo metadata --no-deps` tells which package owns that manifest, its edition, the
//! workspace root, and which sibling workspace members depend on it by path. From that the
//! toolchain is built:
//!
//! - `rustfmt --edition <edition> {targets}` (format). Targets are project-relative file paths.
//!   rustfmt also formats the out-of-line child modules (`mod foo;`) of the file it is given, so
//!   a formatted file may rewrite more files than the one passed.
//! - `cargo clippy -p <package> --all-targets --message-format=json --quiet` (lint). It takes no
//!   targets: clippy checks the whole crate, and the use case's reporting filter decides what
//!   to show. Timeout 120 s.
//! - `cargo test --message-format=json --quiet {targets} --no-fail-fast` (test). Targets are
//!   `--package=<name>` strings, one argv entry each, so `Arg::Targets` can expand them in place
//!   and cargo still sees one valid flag per entry. Timeout 300 s.
//!
//! A virtual workspace manifest (no `[package]`) has no toolchain.

use std::path::{Path, PathBuf};
use std::time::Duration;

use hookah_application::error::PortError;
use hookah_application::ports::CheckRunner;
use hookah_domain::toolchain::{Arg, CheckKind, CommandSpec, Tool, Toolchain};
use serde::Deserialize;

use super::parse::tail;

/// Name of the formatter tool.
pub const RUSTFMT: &str = "rustfmt";
/// Name of the lint tool.
pub const CLIPPY: &str = "clippy";
/// Name of the test tool.
pub const CARGO_TEST: &str = "cargo-test";

const METADATA_TIMEOUT: Duration = Duration::from_secs(30);
const FORMAT_TIMEOUT: Duration = Duration::from_secs(30);
const LINT_TIMEOUT: Duration = Duration::from_secs(120);
const TEST_TIMEOUT: Duration = Duration::from_secs(300);

/// What `cargo metadata` says about the package at a project root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageInfo {
    /// Package name.
    pub name: String,
    /// Rust edition, such as `2024`.
    pub edition: String,
    /// Workspace root; compiler paths are relative to it.
    pub workspace_root: PathBuf,
    /// Names of workspace members that depend on this package by path.
    pub dependents: Vec<String>,
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_root: PathBuf,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    edition: String,
    manifest_path: PathBuf,
    #[serde(default)]
    dependencies: Vec<Dependency>,
}

#[derive(Deserialize)]
struct Dependency {
    path: Option<PathBuf>,
}

pub(super) fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Runs `cargo metadata` at `root` and returns the package owning `root/Cargo.toml`, or `None`
/// for a virtual workspace manifest.
pub fn package_info(
    runner: &dyn CheckRunner,
    root: &Path,
) -> Result<Option<PackageInfo>, PortError> {
    let spec = CommandSpec::new(
        "cargo",
        ["metadata", "--no-deps", "--format-version", "1"]
            .map(|a| Arg::Literal(a.into()))
            .to_vec(),
        root,
        METADATA_TIMEOUT,
    );
    let out = runner.run(&spec, &[])?;
    if out.timed_out {
        return Err(PortError::new("cargo metadata timed out"));
    }
    if !out.success() {
        return Err(PortError::new(format!(
            "cargo metadata failed: {}",
            tail(&out.stderr)
        )));
    }
    parse_metadata(&out.stdout, &root.join("Cargo.toml"))
}

/// Finds the package of `manifest` in `cargo metadata` JSON.
pub fn parse_metadata(json: &str, manifest: &Path) -> Result<Option<PackageInfo>, PortError> {
    let meta: Metadata = serde_json::from_str(json)
        .map_err(|e| PortError::new(format!("unreadable cargo metadata: {e}")))?;
    let manifest = canonical(manifest);
    let Some(pkg) = meta
        .packages
        .iter()
        .find(|p| canonical(&p.manifest_path) == manifest)
    else {
        return Ok(None);
    };
    let dir = canonical(manifest.parent().unwrap_or(Path::new("")));
    let mut dependents: Vec<String> = meta
        .packages
        .iter()
        .filter(|p| p.name != pkg.name)
        .filter(|p| {
            p.dependencies
                .iter()
                .any(|d| d.path.as_deref().is_some_and(|d| canonical(d) == dir))
        })
        .map(|p| p.name.clone())
        .collect();
    dependents.sort();
    dependents.dedup();
    Ok(Some(PackageInfo {
        name: pkg.name.clone(),
        edition: pkg.edition.clone(),
        workspace_root: canonical(&meta.workspace_root),
        dependents,
    }))
}

/// Builds the toolchain of the package at `root`.
pub fn toolchain(info: &PackageInfo, root: &Path) -> Toolchain {
    let lit = |s: &str| Arg::Literal(s.into());
    let format = Tool {
        name: RUSTFMT.into(),
        kind: CheckKind::Format,
        command: CommandSpec::new(
            "rustfmt",
            vec![lit("--edition"), lit(&info.edition), Arg::Targets],
            root,
            FORMAT_TIMEOUT,
        ),
    };
    let lint = Tool {
        name: CLIPPY.into(),
        kind: CheckKind::Lint,
        command: CommandSpec::new(
            "cargo",
            vec![
                lit("clippy"),
                lit("-p"),
                lit(&info.name),
                lit("--all-targets"),
                lit("--message-format=json"),
                lit("--quiet"),
            ],
            root,
            LINT_TIMEOUT,
        ),
    };
    let test = Tool {
        name: CARGO_TEST.into(),
        kind: CheckKind::Test,
        command: CommandSpec::new(
            "cargo",
            vec![
                lit("test"),
                lit("--message-format=json"),
                lit("--quiet"),
                Arg::Targets,
                lit("--no-fail-fast"),
            ],
            root,
            TEST_TIMEOUT,
        ),
    };
    Toolchain::new(vec![format, lint, test])
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use hookah_application::ports::ProcessOutput;

    use super::*;

    const META: &str = r#"{"packages":[
      {"name":"app","edition":"2021","manifest_path":"/ws/crates/app/Cargo.toml","dependencies":[{"name":"serde","path":null},{"name":"core-lib","path":"/ws/crates/core"}]},
      {"name":"core-lib","edition":"2024","manifest_path":"/ws/crates/core/Cargo.toml","dependencies":[]}
    ],"workspace_root":"/ws","target_directory":"/ws/target","version":1}"#;

    #[test]
    fn finds_package_and_reverse_dependents() {
        let core = parse_metadata(META, Path::new("/ws/crates/core/Cargo.toml"))
            .unwrap()
            .unwrap();
        assert_eq!(core.name, "core-lib");
        assert_eq!(core.edition, "2024");
        assert_eq!(core.workspace_root, Path::new("/ws"));
        assert_eq!(core.dependents, ["app"]);
        let app = parse_metadata(META, Path::new("/ws/crates/app/Cargo.toml"))
            .unwrap()
            .unwrap();
        assert!(app.dependents.is_empty());
    }

    #[test]
    fn virtual_workspace_has_no_package() {
        assert_eq!(
            parse_metadata(META, Path::new("/ws/Cargo.toml")).unwrap(),
            None
        );
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(parse_metadata("nope", Path::new("/ws/Cargo.toml")).is_err());
    }

    struct Fake(Result<ProcessOutput, PortError>);

    impl CheckRunner for Fake {
        fn run(&self, _: &CommandSpec, _: &[String]) -> Result<ProcessOutput, PortError> {
            self.0.clone()
        }
    }

    #[test]
    fn missing_cargo_and_failures_are_port_errors() {
        let r = Fake(Err(PortError::new("cannot start `cargo`")));
        assert!(package_info(&r, Path::new("/ws")).is_err());
        let r = Fake(Ok(ProcessOutput {
            exit_code: Some(101),
            stderr: "error: could not find `Cargo.toml`\n".into(),
            ..ProcessOutput::default()
        }));
        let e = package_info(&r, Path::new("/ws")).unwrap_err();
        assert!(e.message().contains("could not find"));
    }

    #[test]
    fn builds_exact_commands() {
        let info = PackageInfo {
            name: "core-lib".into(),
            edition: "2024".into(),
            workspace_root: "/ws".into(),
            dependents: vec![],
        };
        let tc = toolchain(&info, Path::new("/ws/crates/core"));
        let argv = |i: usize| tc.tools()[i].command.expand_args(&["X".into()]);
        assert_eq!(tc.tools()[0].command.program, "rustfmt");
        assert_eq!(argv(0), ["--edition", "2024", "X"]);
        assert_eq!(
            argv(1),
            [
                "clippy",
                "-p",
                "core-lib",
                "--all-targets",
                "--message-format=json",
                "--quiet"
            ]
        );
        assert_eq!(
            argv(2),
            [
                "test",
                "--message-format=json",
                "--quiet",
                "X",
                "--no-fail-fast"
            ]
        );
        assert_eq!(tc.tools()[1].command.timeout, Duration::from_secs(120));
        assert_eq!(tc.tools()[2].command.timeout, Duration::from_secs(300));
        assert_eq!(tc.tools()[2].command.cwd, Path::new("/ws/crates/core"));
    }
}
