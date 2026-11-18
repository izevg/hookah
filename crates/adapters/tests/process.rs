//! Integration tests of the process runner with real short commands.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

use hookah_adapters::process::runner::ProcessRunner;
use hookah_application::ports::CheckRunner;
use hookah_domain::toolchain::{Arg, CommandSpec};

#[cfg(unix)]
fn sh(script: &str, timeout: Duration) -> CommandSpec {
    CommandSpec::new(
        "sh",
        vec![Arg::Literal("-c".into()), Arg::Literal(script.into())],
        ".",
        timeout,
    )
}

#[cfg(unix)]
#[test]
fn captures_output_and_zero_exit() {
    let out = ProcessRunner
        .run(&sh("echo out; echo err >&2", Duration::from_secs(5)), &[])
        .unwrap();
    assert!(out.success());
    assert_eq!(out.stdout, "out\n");
    assert_eq!(out.stderr, "err\n");
}

#[cfg(unix)]
#[test]
fn reports_non_zero_exit() {
    let out = ProcessRunner
        .run(&sh("exit 3", Duration::from_secs(5)), &[])
        .unwrap();
    assert_eq!(out.exit_code, Some(3));
    assert!(!out.success() && !out.timed_out);
}

#[cfg(unix)]
#[test]
fn expands_targets() {
    let spec = CommandSpec::new(
        "sh",
        vec![
            Arg::Literal("-c".into()),
            Arg::Literal("echo \"$@\"".into()),
            Arg::Literal("sh".into()),
            Arg::Targets,
        ],
        ".",
        Duration::from_secs(5),
    );
    let out = ProcessRunner.run(&spec, &["a".into(), "b".into()]).unwrap();
    assert_eq!(out.stdout, "a b\n");
}

#[cfg(unix)]
#[test]
fn large_output_does_not_deadlock() {
    let out = ProcessRunner
        .run(
            &sh("head -c 1000000 /dev/zero | tr '\\0' x; head -c 1000000 /dev/zero | tr '\\0' y >&2", Duration::from_secs(10)),
            &[],
        )
        .unwrap();
    assert_eq!(out.stdout.len(), 1_000_000);
    assert_eq!(out.stderr.len(), 1_000_000);
}

#[cfg(unix)]
#[test]
fn timeout_kills_the_whole_process_group() {
    let start = Instant::now();
    // The sleeping grandchild holds the pipes; killing only the shell would hang the drain.
    let out = ProcessRunner
        .run(&sh("sleep 30 & sleep 30", Duration::from_millis(200)), &[])
        .unwrap();
    assert!(out.timed_out);
    assert_eq!(out.exit_code, None);
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn missing_program_is_an_error() {
    let spec = CommandSpec::new(
        "hookah-no-such-program",
        vec![],
        ".",
        Duration::from_secs(1),
    );
    assert!(ProcessRunner.run(&spec, &[]).is_err());
}

/// `cmd /C <script>`: the Windows counterpart of the `sh` helper.
#[cfg(windows)]
fn cmd(script: &str, timeout: Duration) -> CommandSpec {
    CommandSpec::new(
        "cmd",
        vec![Arg::Literal("/C".into()), Arg::Literal(script.into())],
        ".",
        timeout,
    )
}

#[cfg(windows)]
#[test]
fn windows_captures_output_and_exit_code() {
    let out = ProcessRunner
        .run(&cmd("echo out& exit 3", Duration::from_secs(10)), &[])
        .unwrap();
    assert_eq!(out.exit_code, Some(3));
    assert_eq!(out.stdout.trim(), "out");
}

#[cfg(windows)]
#[test]
fn windows_timeout_kills_the_whole_process_tree() {
    let start = std::time::Instant::now();
    // `ping` is a grandchild of `cmd` and holds the pipes; the job object kills it too.
    let out = ProcessRunner
        .run(
            &cmd("ping -n 30 127.0.0.1 >nul", Duration::from_millis(500)),
            &[],
        )
        .unwrap();
    assert!(out.timed_out);
    assert_eq!(out.exit_code, None);
    assert!(start.elapsed() < Duration::from_secs(10));
}

#[cfg(unix)]
#[test]
fn sets_the_environment_of_the_command() {
    let spec = sh("echo \"$HOOKAH_TEST_VAR\"", Duration::from_secs(5))
        .with_env(vec![("HOOKAH_TEST_VAR".into(), "from-spec".into())]);
    let out = ProcessRunner.run(&spec, &[]).unwrap();
    assert_eq!(out.stdout, "from-spec\n");
}

#[cfg(windows)]
#[test]
fn windows_sets_the_environment_of_the_command() {
    let spec = cmd("echo %HOOKAH_TEST_VAR%", Duration::from_secs(10))
        .with_env(vec![("HOOKAH_TEST_VAR".into(), "from-spec".into())]);
    let out = ProcessRunner.run(&spec, &[]).unwrap();
    assert_eq!(out.stdout.trim(), "from-spec");
}

mod project_env {
    use std::fs;

    use hookah_adapters::config::env::{Host, ProjectEnvRunner};

    use super::*;

    /// A host with an empty environment: the developer's real user config is never read.
    fn sandbox() -> Host {
        Host {
            var: Box::new(|_| None),
            windows: cfg!(windows),
        }
    }

    fn echo(var: &str, cwd: &std::path::Path) -> CommandSpec {
        let (program, args) = if cfg!(windows) {
            ("cmd", vec!["/C".to_string(), format!("echo %{var}%")])
        } else {
            ("sh", vec!["-c".to_string(), format!("echo \"${var}\"")])
        };
        CommandSpec::new(
            program,
            args.into_iter().map(Arg::Literal).collect(),
            cwd,
            Duration::from_secs(10),
        )
    }

    fn project(config: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hookah-projenv-{}-{}",
            std::process::id(),
            config.len()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(".hookah.local.toml"), config).unwrap();
        dir
    }

    #[test]
    fn the_tool_process_receives_the_configured_variables() {
        let dir = project("[env]\nHOOKAH_TEST_VAR = \"from-config {root}\"\n");
        let runner = ProjectEnvRunner::new(ProcessRunner, sandbox());
        let out = runner.run(&echo("HOOKAH_TEST_VAR", &dir), &[]).unwrap();
        let root = dunce::canonicalize(&dir).unwrap();
        assert_eq!(out.stdout.trim(), format!("from-config {}", root.display()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_wins_over_the_variables_the_detector_set() {
        let dir = project("[env]\nHOOKAH_TEST_WINS = \"config\"\n");
        let spec = echo("HOOKAH_TEST_WINS", &dir).with_env(vec![
            ("HOOKAH_TEST_WINS".into(), "detector".into()),
            ("HOOKAH_TEST_KEPT".into(), "kept".into()),
        ]);
        let runner = ProjectEnvRunner::new(ProcessRunner, sandbox());
        assert_eq!(runner.run(&spec, &[]).unwrap().stdout.trim(), "config");
        let kept = echo("HOOKAH_TEST_KEPT", &dir).with_env(spec.env.clone());
        assert_eq!(runner.run(&kept, &[]).unwrap().stdout.trim(), "kept");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_project_without_config_runs_unchanged() {
        let dir = std::env::temp_dir().join(format!("hookah-projenv-none-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let runner = ProjectEnvRunner::new(ProcessRunner, sandbox());
        let spec = echo("HOOKAH_TEST_UNSET_VAR", &dir);
        assert!(runner.run(&spec, &[]).unwrap().success());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_malformed_file_does_not_stop_the_tool() {
        let dir = project("[env\n broken");
        let runner = ProjectEnvRunner::new(ProcessRunner, sandbox());
        let out = runner.run(&echo("HOOKAH_TEST_VAR", &dir), &[]).unwrap();
        assert!(out.success());
        let _ = fs::remove_dir_all(&dir);
    }
}
