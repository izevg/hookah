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
//! - `cargo clippy -p <package> [-p <dependent>]... --all-targets --message-format=json --quiet`
//!   (lint). The dependents are the workspace members that depend on the package by path, so an
//!   API change that breaks them is caught in the edit phase (clippy is incremental, only their
//!   check is added). It takes no targets: clippy checks the whole crates, and the use case's
//!   reporting filter decides what to show. Timeout 120 s.
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
use serde::{Deserialize, Serialize};

use super::parse::tail;
use crate::ecosystems::cache::Input;
use crate::ecosystems::cache::fingerprint::{executables_in, files_in};

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageInfo {
    /// Package name.
    pub name: String,
    /// Rust edition, such as `2024`.
    pub edition: String,
    /// Workspace root; compiler paths are relative to it.
    pub workspace_root: PathBuf,
    /// Names of workspace members that depend on this package by path.
    pub dependents: Vec<String>,
    /// Manifests of all workspace members, the package itself included.
    pub members: Vec<PathBuf>,
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
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
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
        members: meta
            .packages
            .iter()
            .map(|p| canonical(&p.manifest_path))
            .collect(),
    }))
}

/// Files, looked up in the project directory and every ancestor, that decide what `cargo` and the
/// toolchain behind it do: manifests and the lockfile, toolchain pins, and `cargo` configuration.
const CONFIG_FILES: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain",
    "rust-toolchain.toml",
    ".mise.toml",
    "mise.toml",
    ".tool-versions",
    ".cargo/config.toml",
    ".cargo/config",
];

/// Executables whose presence on `PATH` changes the result: `cargo` itself and `cargo-mutants`.
const PATH_TOOLS: &[&str] = &["cargo", "cargo-mutants"];

/// The inputs of `cargo metadata` for the package at `root`: the configuration files of the
/// project and its ancestors. The other workspace members are *derived* (see [`derived_inputs`]).
pub fn manifest_inputs(root: &Path) -> Vec<Input> {
    let dirs: Vec<PathBuf> = root.ancestors().map(Path::to_path_buf).collect();
    files_in(&dirs, CONFIG_FILES)
}

/// The inputs of toolchain detection for the package at `root`: those of `cargo metadata`, the
/// `cargo` and `cargo-mutants` executables in the `PATH` directories `path_dirs`, and the `tests`
/// directory, which decides whether a missing `cargo-mutants` is worth reporting.
pub fn detection_inputs(root: &Path, path_dirs: &[PathBuf]) -> Vec<Input> {
    let mut inputs = manifest_inputs(root);
    inputs.extend(executables_in(path_dirs, PATH_TOOLS));
    inputs.push(Input::Exists(root.join("tests")));
    inputs
}

/// The inputs only `cargo metadata` can name: the manifest of every workspace member, and the
/// directories that hold the members, so that a new member (as in `crates/*`) is noticed.
pub fn derived_inputs(info: &PackageInfo) -> Vec<Input> {
    let mut inputs: Vec<Input> = info.members.iter().cloned().map(Input::File).collect();
    for member_dir in info.members.iter().filter_map(|m| m.parent()) {
        if member_dir == info.workspace_root {
            continue;
        }
        if let Some(parent) = member_dir.parent() {
            let dirs = Input::Dirs(parent.to_path_buf());
            if !inputs.contains(&dirs) {
                inputs.push(dirs);
            }
        }
    }
    inputs
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
    // The edited package plus its path dependents: an API change breaks them, not the package.
    let mut lint_args = vec![lit("clippy")];
    for name in std::iter::once(&info.name).chain(&info.dependents) {
        lint_args.push(lit("-p"));
        lint_args.push(lit(name));
    }
    lint_args.extend([
        lit("--all-targets"),
        lit("--message-format=json"),
        lit("--quiet"),
    ]);
    let lint = Tool {
        name: CLIPPY.into(),
        kind: CheckKind::Lint,
        command: CommandSpec::new("cargo", lint_args, root, LINT_TIMEOUT),
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
            members: vec![],
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
        let with_deps = PackageInfo {
            dependents: vec!["app".into(), "cli".into()],
            ..info
        };
        let tc2 = toolchain(&with_deps, Path::new("/ws/crates/core"));
        assert_eq!(
            tc2.tools()[1].command.expand_args(&[]),
            [
                "clippy",
                "-p",
                "core-lib",
                "-p",
                "app",
                "-p",
                "cli",
                "--all-targets",
                "--message-format=json",
                "--quiet"
            ]
        );
        assert_eq!(tc.tools()[1].command.timeout, Duration::from_secs(120));
        assert_eq!(tc.tools()[2].command.timeout, Duration::from_secs(300));
        assert_eq!(tc.tools()[2].command.cwd, Path::new("/ws/crates/core"));
    }

    #[test]
    fn metadata_inputs_cover_the_configuration_of_every_ancestor() {
        let inputs = manifest_inputs(Path::new("/ws/crates/app"));
        for dir in ["/ws/crates/app", "/ws/crates", "/ws", "/"] {
            for name in CONFIG_FILES {
                assert!(
                    inputs.contains(&Input::File(Path::new(dir).join(name))),
                    "{dir} {name}"
                );
            }
        }
        for name in [
            "Cargo.lock",
            "rust-toolchain.toml",
            ".mise.toml",
            ".cargo/config.toml",
        ] {
            assert!(CONFIG_FILES.contains(&name), "{name}");
        }
    }

    #[test]
    fn detection_inputs_add_the_path_executables_and_the_tests_directory() {
        let path = [PathBuf::from("/usr/bin"), PathBuf::from("/opt/bin")];
        let inputs = detection_inputs(Path::new("/ws/app"), &path);
        let base = manifest_inputs(Path::new("/ws/app"));
        assert!(inputs.starts_with(&base));
        for dir in ["/usr/bin", "/opt/bin"] {
            for name in ["cargo", "cargo-mutants"] {
                assert!(inputs.contains(&Input::File(Path::new(dir).join(name))));
            }
        }
        assert_eq!(
            inputs.last(),
            Some(&Input::Exists(Path::new("/ws/app").join("tests")))
        );
        assert!(!base.contains(&Input::Exists(Path::new("/ws/app").join("tests"))));
    }

    #[test]
    fn derived_inputs_are_the_members_and_their_parent_directories() {
        let info = PackageInfo {
            name: "a".into(),
            edition: "2024".into(),
            workspace_root: "/ws".into(),
            dependents: vec![],
            members: vec![
                "/ws/Cargo.toml".into(),
                "/ws/crates/a/Cargo.toml".into(),
                "/ws/crates/b/Cargo.toml".into(),
                "/ws/tools/c/Cargo.toml".into(),
            ],
        };
        let want: Vec<Input> = [
            Input::File("/ws/Cargo.toml".into()),
            Input::File("/ws/crates/a/Cargo.toml".into()),
            Input::File("/ws/crates/b/Cargo.toml".into()),
            Input::File("/ws/tools/c/Cargo.toml".into()),
            Input::Dirs("/ws/crates".into()),
            Input::Dirs("/ws/tools".into()),
        ]
        .into();
        assert_eq!(derived_inputs(&info), want);
        // The workspace root itself has no sibling directories worth watching.
        assert!(!derived_inputs(&info).contains(&Input::Dirs("/".into())));
    }

    #[test]
    fn package_info_records_every_workspace_member() {
        let core = parse_metadata(META, Path::new("/ws/crates/core/Cargo.toml"))
            .unwrap()
            .unwrap();
        assert_eq!(
            core.members,
            [
                PathBuf::from("/ws/crates/app/Cargo.toml"),
                PathBuf::from("/ws/crates/core/Cargo.toml")
            ]
        );
    }

    #[test]
    fn package_info_survives_a_json_round_trip() {
        let core = parse_metadata(META, Path::new("/ws/crates/core/Cargo.toml"))
            .unwrap()
            .unwrap();
        let json = serde_json::to_string(&Some(&core)).unwrap();
        let back: Option<PackageInfo> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Some(core));
        assert_eq!(
            serde_json::from_str::<Option<PackageInfo>>("null").unwrap(),
            None
        );
    }
}
