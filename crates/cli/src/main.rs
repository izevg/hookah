//! `hookah` binary entry point.
//!
//! Parses arguments, builds the object graph, runs the command, and maps the result to an exit
//! code. In `hook` mode everything runs inside the fail-open guard: a panic, an unreadable
//! payload, or any internal error yields a stderr warning and exit code 0. The only non-zero hook
//! exit is the deliberate 2 of a failing verdict.

mod args;
mod render;
mod wiring;

use std::io::{Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use hookah_application::{CheckEdit, CheckSession, EditRequest, Outcome};
use hookah_domain::path::display_path;
use hookah_domain::project::locate_project;
use hookah_harness::claude::Claude;
use hookah_harness::codex::Codex;
use hookah_harness::kimi::Kimi;
use hookah_harness::protocol::{Harness, Invocation, Response, merge_outcomes};
use hookah_harness::qoder::Qoder;

use crate::args::{Cli, Command, HarnessKind};
use crate::render::Style;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            // A malformed hook command line must not block the agent (clap would exit 2).
            let is_hook = std::env::args().nth(1).as_deref() == Some("hook");
            let code = if is_hook { 0 } else { e.exit_code() };
            let _ = if e.use_stderr() {
                writeln!(std::io::stderr(), "{e}")
            } else {
                writeln!(std::io::stdout(), "{e}")
            };
            return exit(code);
        }
    };
    let limit = cli.report_limit;
    match cli.command {
        Command::Check { paths } => exit(check(&paths, limit)),
        Command::Detect { path } => exit(detect(&path)),
        Command::Hook { harness } => exit(guarded_hook(harness, limit)),
    }
}

fn exit(code: i32) -> ExitCode {
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

/// `hookah check`: exit 1 if any file fails.
fn check(paths: &[PathBuf], limit: usize) -> i32 {
    let adapters = wiring::build();
    let use_case = CheckEdit::new(adapters.ports(), limit);
    let style = Style::for_stdout();
    let mut failed = false;
    for path in paths {
        let file = std::path::absolute(path).unwrap_or_else(|_| path.clone());
        let display_root = std::env::current_dir()
            .unwrap_or_else(|_| file.parent().map(Path::to_path_buf).unwrap_or_default());
        let request = match Invocation::Edit(EditRequest::new(file, display_root)).canonical() {
            Invocation::Edit(request) => request,
            _ => continue,
        };
        let outcome = use_case.run(&request);
        failed |= matches!(outcome.verdict, hookah_domain::verdict::Verdict::Fail(_));
        let shown = display_path(&request.file, &request.display_root);
        print_out(&render::outcome(style, &shown, &outcome));
    }
    i32::from(failed)
}

/// `hookah detect`: print what would run for `path`.
fn detect(path: &Path) -> i32 {
    let adapters = wiring::build();
    let ports = adapters.ports();
    let file = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let style = Style::for_stdout();
    let found = match ports.locator.manifests_above(&file) {
        Ok(manifests) => locate_project(&file, &manifests),
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "hookah: locating project: {e}");
            return 1;
        }
    };
    let Some(project) = found else {
        print_out(&render::detection(style, None));
        return 0;
    };
    match ports.detector.detect(&project) {
        Ok(toolchain) => {
            print_out(&render::detection(style, Some((&project, &toolchain))));
            0
        }
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "hookah: detecting toolchain: {e}");
            1
        }
    }
}

/// Runs the hook and converts any panic into a non-blocking warning.
fn guarded_hook(harness: HarnessKind, limit: usize) -> i32 {
    panic::set_hook(Box::new(|_| {}));
    let result = panic::catch_unwind(AssertUnwindSafe(|| hook(harness, limit)));
    let response = result.unwrap_or_else(|_| Response {
        exit_code: 0,
        stdout: String::new(),
        stderr: "hookah: internal error, skipping checks\n".into(),
    });
    let _ = write!(std::io::stdout(), "{}", response.stdout);
    let _ = write!(std::io::stderr(), "{}", response.stderr);
    response.exit_code
}

/// `hookah hook <harness>`: payload on stdin, `Response` out. Errors fail open.
fn hook(harness: HarnessKind, limit: usize) -> Response {
    let mut stdin = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut stdin) {
        return warning(format!("cannot read hook payload: {e}"));
    }
    match harness {
        HarnessKind::Claude => run_harness(&Claude, &stdin, limit),
        HarnessKind::Codex => run_harness(&Codex, &stdin, limit),
        HarnessKind::Qoder => run_harness(&Qoder, &stdin, limit),
        HarnessKind::Kimi => run_harness(&Kimi, &stdin, limit),
    }
}

fn run_harness(harness: &dyn Harness, stdin: &str, limit: usize) -> Response {
    let invocation = match harness.parse(stdin) {
        Ok(i) => i.canonical(),
        Err(e) => return warning(e.to_string()),
    };
    let adapters = wiring::build();
    let ports = adapters.ports();
    let outcome: Outcome = match &invocation {
        Invocation::Edit(request) => CheckEdit::new(ports, limit).run(request),
        Invocation::EditBatch(requests) => {
            let use_case = CheckEdit::new(ports, limit);
            merge_outcomes(requests.iter().map(|r| use_case.run(r)).collect(), limit)
        }
        Invocation::Stop { request, .. } => CheckSession::new(ports, limit).run(request),
        Invocation::Ignore => Outcome::pass(),
    };
    harness.render(&invocation, &outcome)
}

fn warning(message: String) -> Response {
    Response {
        exit_code: 0,
        stdout: String::new(),
        stderr: format!("hookah: warning: {message}; skipping checks\n"),
    }
}

fn print_out(text: &str) {
    let _ = write!(std::io::stdout(), "{text}");
}
