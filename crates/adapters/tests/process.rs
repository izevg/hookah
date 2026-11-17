//! Integration tests of the process runner with real short commands.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use hookah_adapters::process::runner::ProcessRunner;
use hookah_application::ports::CheckRunner;
use hookah_domain::toolchain::{Arg, CommandSpec};

fn sh(script: &str, timeout: Duration) -> CommandSpec {
    CommandSpec::new(
        "sh",
        vec![Arg::Literal("-c".into()), Arg::Literal(script.into())],
        ".",
        timeout,
    )
}

#[test]
fn captures_output_and_zero_exit() {
    let out = ProcessRunner
        .run(&sh("echo out; echo err >&2", Duration::from_secs(5)), &[])
        .unwrap();
    assert!(out.success());
    assert_eq!(out.stdout, "out\n");
    assert_eq!(out.stderr, "err\n");
}

#[test]
fn reports_non_zero_exit() {
    let out = ProcessRunner
        .run(&sh("exit 3", Duration::from_secs(5)), &[])
        .unwrap();
    assert_eq!(out.exit_code, Some(3));
    assert!(!out.success() && !out.timed_out);
}

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
