//! Rust mutation testing with `cargo-mutants`.
//!
//! - [`tool`] builds the command: `cargo mutants --package <pkg> {targets} --no-shuffle --output
//!   <workspace>/target/hookah-mutants --iterate -j <n> --timeout-multiplier 3`. Targets are
//!   `--file=<path>` strings, one argv entry each (see [`select`]). `--iterate` makes runs
//!   incremental: mutants caught or unviable in an earlier run are skipped, missed ones are
//!   re-tested. `--in-diff` is not used: mutation is triggered by changed tests, so the targets
//!   are whole files, not changed lines. Timeout 480 s.
//! - The output directory lives under the workspace `target/`, so it is ignored by version
//!   control by default and survives between runs; that is the cache `--iterate` reads.
//! - [`available`] runs `cargo mutants --version` in the project root, so a tool pinned by mise
//!   or any other shim counts. When it is missing and the crate has tests, [`missing`] says so;
//!   hookah never installs it.
//! - [`classify`], [`select`], and [`outcomes`] hold the file-role heuristic, the mapping from
//!   changed tests to source files, and the `outcomes.json` parser.

pub mod classify;
pub mod outcomes;
pub mod select;

use std::path::{Path, PathBuf};
use std::time::Duration;

use hookah_application::ports::CheckRunner;
use hookah_domain::toolchain::{Arg, CheckKind, CommandSpec, MissingTool, Tool};

use super::detect::PackageInfo;

/// Name of the mutation tool.
pub const CARGO_MUTANTS: &str = "cargo-mutants";

const MUTANTS_TIMEOUT: Duration = Duration::from_secs(480);
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Mutants run in parallel at most this many at a time.
const MAX_JOBS: usize = 4;

/// Where `cargo-mutants` keeps its output and the `--iterate` cache for a workspace.
pub fn output_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join("target").join("hookah-mutants")
}

/// The `--jobs` value: half the CPUs, between 1 and [`MAX_JOBS`].
pub fn jobs() -> usize {
    jobs_for(std::thread::available_parallelism().map_or(2, usize::from))
}

fn jobs_for(cpus: usize) -> usize {
    (cpus / 2).clamp(1, MAX_JOBS)
}

/// Builds the mutation tool of the package at `root`.
pub fn tool(info: &PackageInfo, root: &Path, jobs: usize) -> Tool {
    let lit = |s: &str| Arg::Literal(s.into());
    Tool {
        name: CARGO_MUTANTS.into(),
        kind: CheckKind::Mutation,
        command: CommandSpec::new(
            "cargo",
            vec![
                lit("mutants"),
                lit("--package"),
                lit(&info.name),
                Arg::Targets,
                lit("--no-shuffle"),
                lit("--output"),
                Arg::Literal(output_dir(&info.workspace_root).display().to_string()),
                lit("--iterate"),
                lit("-j"),
                Arg::Literal(jobs.to_string()),
                lit("--timeout-multiplier"),
                lit("3"),
            ],
            root,
            MUTANTS_TIMEOUT,
        ),
    }
}

/// Whether `cargo mutants` runs in `root`.
pub fn available(runner: &dyn CheckRunner, root: &Path) -> bool {
    let spec = CommandSpec::new(
        "cargo",
        vec![
            Arg::Literal("mutants".into()),
            Arg::Literal("--version".into()),
        ],
        root,
        PROBE_TIMEOUT,
    );
    runner.run(&spec, &[]).is_ok_and(|o| o.success())
}

/// The notice for a project that has tests but no `cargo-mutants`.
pub fn missing() -> MissingTool {
    MissingTool::new(
        CARGO_MUTANTS,
        CheckKind::Mutation,
        "cargo install --locked cargo-mutants (or pin it in mise)",
    )
}

/// Whether the crate at `root` has tests worth mutating against: a `tests/` directory or a
/// `#[test]` anywhere under `src/`.
pub fn has_tests(root: &Path) -> bool {
    root.join("tests").is_dir() || dir_has_test(&root.join("src"))
}

fn dir_has_test(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        let path = e.path();
        if path.is_dir() {
            dir_has_test(&path)
        } else {
            path.extension().is_some_and(|x| x == "rs")
                && std::fs::read_to_string(&path).is_ok_and(|t| t.contains("#[test]"))
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use hookah_application::error::PortError;
    use hookah_application::ports::ProcessOutput;

    use super::*;

    fn info() -> PackageInfo {
        PackageInfo {
            name: "core-lib".into(),
            edition: "2024".into(),
            workspace_root: "/ws".into(),
            dependents: vec![],
        }
    }

    #[test]
    fn jobs_are_half_the_cpus_between_one_and_the_maximum() {
        assert_eq!(jobs_for(1), 1);
        assert_eq!(jobs_for(2), 1);
        assert_eq!(jobs_for(3), 1);
        assert_eq!(jobs_for(4), 2);
        assert_eq!(jobs_for(7), 3);
        assert_eq!(jobs_for(8), 4);
        assert_eq!(jobs_for(64), MAX_JOBS);
        let cpus = std::thread::available_parallelism().map_or(2, usize::from);
        assert_eq!(jobs(), jobs_for(cpus));
    }

    #[test]
    fn builds_the_incremental_command() {
        let t = tool(&info(), Path::new("/ws/crates/core"), 2);
        assert_eq!(t.kind, CheckKind::Mutation);
        assert_eq!(
            t.command
                .expand_args(&["--file=crates/core/src/a.rs".into()]),
            [
                "mutants",
                "--package",
                "core-lib",
                "--file=crates/core/src/a.rs",
                "--no-shuffle",
                "--output",
                "/ws/target/hookah-mutants",
                "--iterate",
                "-j",
                "2",
                "--timeout-multiplier",
                "3"
            ]
        );
        assert_eq!(t.command.timeout, Duration::from_secs(480));
        assert_eq!(t.command.cwd, Path::new("/ws/crates/core"));
        assert!((1..=MAX_JOBS).contains(&jobs()));
    }

    struct Fake(Result<ProcessOutput, PortError>);

    impl CheckRunner for Fake {
        fn run(&self, spec: &CommandSpec, _: &[String]) -> Result<ProcessOutput, PortError> {
            assert_eq!(spec.expand_args(&[]), ["mutants", "--version"]);
            self.0.clone()
        }
    }

    #[test]
    fn availability_follows_the_version_probe() {
        let ok = ProcessOutput {
            exit_code: Some(0),
            stdout: "cargo-mutants 27.1.0\n".into(),
            ..ProcessOutput::default()
        };
        let bad = ProcessOutput {
            exit_code: Some(101),
            stderr: "error: no such command: `mutants`\n".into(),
            ..ProcessOutput::default()
        };
        let root = Path::new("/ws");
        assert!(available(&Fake(Ok(ok)), root));
        assert!(!available(&Fake(Ok(bad)), root));
        assert!(!available(&Fake(Err(PortError::new("no cargo"))), root));
    }

    #[test]
    fn missing_notice_names_the_tool() {
        let m = missing();
        assert_eq!(m.name, "cargo-mutants");
        assert_eq!(m.kind, CheckKind::Mutation);
        assert!(m.hint.contains("cargo install --locked cargo-mutants"));
    }

    #[test]
    fn has_tests_looks_for_a_tests_dir_or_a_test_attribute() {
        let dir = std::env::temp_dir().join(format!("hookah-has-tests-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/inner")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
        assert!(!has_tests(&dir));
        std::fs::write(dir.join("src/inner/x.rs"), "#[test]\nfn t() {}\n").unwrap();
        assert!(has_tests(&dir));
        std::fs::remove_file(dir.join("src/inner/x.rs")).unwrap();
        std::fs::create_dir_all(dir.join("tests")).unwrap();
        assert!(has_tests(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
