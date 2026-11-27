//! `hookah` binary entry point.
//!
//! Parses arguments, builds the object graph, runs the command, and maps the result to an exit
//! code. In `hook` mode everything runs inside the fail-open guard: a panic, an unreadable
//! payload, or any internal error yields a stderr warning and exit code 0. The only non-zero hook
//! exit is the deliberate 2 of a failing verdict.

mod args;
mod hooks;
mod render;
mod setup;
mod update;
mod wiring;

use std::io::{Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use hookah_adapters::config::env::{Host, resolve as resolve_env};
use hookah_application::{
    CheckEdit, CheckSession, ComputeBaseline, EditRequest, Outcome, ReadStatus, StartSession,
    StopRequest,
};
use hookah_domain::path::{display_path, slashed};
use hookah_domain::project::locate_project;
use hookah_harness::antigravity;
use hookah_harness::claude::Claude;
use hookah_harness::codex::Codex;
use hookah_harness::cursor::Cursor;
use hookah_harness::kimi::Kimi;
use hookah_harness::opencode::{Opencode, PLUGIN_SOURCE};
use hookah_harness::orca::PLUGIN_FILES;
use hookah_harness::plugins::hermes::{Hermes, PLUGIN_SOURCE as HERMES_PLUGIN_SOURCE};
use hookah_harness::plugins::openclaw::{Openclaw, PLUGIN_SOURCE as OPENCLAW_PLUGIN_SOURCE};
use hookah_harness::plugins::pi::{PLUGIN_SOURCE as PI_PLUGIN_SOURCE, Pi};
use hookah_harness::protocol::{Harness, Invocation, Response, canonical_dir, merge_outcomes};
use hookah_harness::qoder::Qoder;
use hookah_harness::zcode::ZCode;

use crate::args::{Cli, Command, HarnessKind, PluginKind};
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
        Command::Check {
            changed: true,
            root,
            ..
        } => exit(check_changed(root.as_deref(), limit)),
        Command::Check { paths, .. } => exit(check(&paths, limit)),
        Command::Status { root, json } => exit(status(root.as_deref(), json)),
        Command::Config { path, model, hook } => exit(config(&path, model.as_deref(), hook)),
        Command::Detect { path } => exit(detect(&path)),
        Command::Baseline { session, root } => exit(baseline(&root, &session)),
        Command::Update { check, background } => exit(update::run(check, background)),
        Command::Hook { harness } => exit(guarded_hook(harness, limit)),
        Command::Plugin { harness, out } => exit(plugin(harness, out.as_deref())),
        Command::Setup(args) => exit(setup::run(&args)),
        Command::Uninstall(args) => exit(setup::uninstall(&args)),
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

/// `hookah baseline`: the background job. Failures are recorded in the store; the exit code is
/// always 0, and nothing is printed because nobody reads the output.
fn baseline(root: &Path, session: &str) -> i32 {
    let adapters = wiring::build();
    let _ = ComputeBaseline::new(adapters.ports()).run(root, session);
    0
}

/// `hookah check --changed`: the stop phase over everything changed under `root`; exit 1 if it
/// fails.
fn check_changed(root: Option<&Path>, limit: usize) -> i32 {
    let request = StopRequest::new(canonical_dir(&absolute_dir(root)));
    let adapters = wiring::build();
    let outcome = CheckSession::new(adapters.ports(), limit)
        .with_budgets(wiring::budgets())
        .run(&request);
    let failed = matches!(outcome.verdict, hookah_domain::verdict::Verdict::Fail(_));
    let shown = display_path(&request.root, &request.display_root);
    let shown = if shown.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        shown
    };
    print_out(&render::outcome(Style::for_stdout(), &shown, &outcome));
    i32::from(failed)
}

/// `hookah status`: the latest recorded outcome per project under `root`. Exit 0 whatever the
/// outcomes are: it reports, it does not judge.
fn status(root: Option<&Path>, json: bool) -> i32 {
    let canonical = canonical_dir(&absolute_dir(root));
    let adapters = wiring::build();
    let found = ReadStatus::new(adapters.ports()).run(&canonical);
    print_out(&if json {
        render::status_json(&canonical, &found)
    } else {
        render::status_text(&found)
    });
    0
}

/// `root`, or the current directory, as an absolute path.
fn absolute_dir(root: Option<&Path>) -> PathBuf {
    let dir = root.map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    std::path::absolute(&dir).unwrap_or(dir)
}

/// `hookah plugin <harness>`: print the embedded plugin source, or write the plugin's files.
fn plugin(harness: PluginKind, out: Option<&Path>) -> i32 {
    match (harness, out) {
        (PluginKind::Opencode, None) => print_out(PLUGIN_SOURCE),
        (PluginKind::Opencode, Some(_)) => {
            let _ = writeln!(
                std::io::stderr(),
                "hookah: the opencode plugin is one file: `hookah plugin opencode > .opencode/plugins/hookah.ts`"
            );
            return 1;
        }
        (PluginKind::Hermes, None) => print_out(HERMES_PLUGIN_SOURCE),
        (PluginKind::Hermes, Some(_)) => {
            let _ = writeln!(
                std::io::stderr(),
                "hookah: the Hermes plugin is installed by `hookah setup hermes`"
            );
            return 1;
        }
        (PluginKind::Openclaw, None) => print_out(OPENCLAW_PLUGIN_SOURCE),
        (PluginKind::Openclaw, Some(_)) => {
            let _ = writeln!(
                std::io::stderr(),
                "hookah: the OpenClaw plugin is installed by `hookah setup openclaw`"
            );
            return 1;
        }
        (PluginKind::Pi, None) => print_out(PI_PLUGIN_SOURCE),
        (PluginKind::Pi, Some(_)) => {
            let _ = writeln!(
                std::io::stderr(),
                "hookah: the Pi extension is one file: `hookah plugin pi > .pi/extensions/hookah.ts`"
            );
            return 1;
        }
        (PluginKind::Orca, None) => {
            let _ = writeln!(
                std::io::stderr(),
                "hookah: the Orca plugin has several files: pass --out <dir>"
            );
            return 1;
        }
        (PluginKind::Orca, Some(dir)) => return install_orca_plugin(dir),
    }
    0
}

fn install_orca_plugin(dir: &Path) -> i32 {
    match hookah_setup::write_plugin(dir, PLUGIN_FILES) {
        Ok(done) => {
            let shown = |p: &Path| display_path(p, &absolute_dir(None)).display().to_string();
            for file in &done {
                let verb = if file.wrote { "wrote" } else { "unchanged" };
                print_out(&format!("{verb} {}\n", shown(&file.path)));
            }
            print_out(&format!(
                "next: load it in Orca with `orca plugin dev load {}` (development) or `orca plugin install {}`; see docs/setup/plugins/orca.md\n",
                shown(dir),
                shown(dir)
            ));
            0
        }
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "hookah: plugin: {e}");
            1
        }
    }
}

/// `hookah config`: the config files and tool environment in effect for `path`, or with
/// `--hook <harness>`, a dry-run explanation of how a hook payload's model is resolved and filtered.
fn config(path: &Path, check_model: Option<&str>, hook: Option<HarnessKind>) -> i32 {
    let target = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = if target.is_dir() {
        target
    } else {
        target.parent().map(Path::to_path_buf).unwrap_or(target)
    };
    if let Some(harness) = hook {
        return config_hook(harness, &dir);
    }
    let found = resolve_env(&dir, &Host::from_process());
    print_out(&render::config(&found, check_model));
    0
}

fn config_hook(harness: HarnessKind, fallback_dir: &Path) -> i32 {
    let mut stdin = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut stdin) {
        let _ = writeln!(std::io::stderr(), "hookah: config: cannot read stdin: {e}");
        return 1;
    }
    let harness_name = harness
        .to_possible_value()
        .map_or_else(|| "hook".to_owned(), |v| v.get_name().to_owned());
    let parsed = match harness {
        HarnessKind::Antigravity => match antigravity::parse(&stdin) {
            Ok(payload) => {
                let (event, scopes) = match payload.event {
                    antigravity::Event::Start => {
                        let adapters = wiring::build();
                        let ports = adapters.ports();
                        ("start", discover_start_scopes(&ports, &payload.roots))
                    }
                    antigravity::Event::Edit(req) => {
                        ("edit", vec![hooks::model::edit_dir(&req).to_path_buf()])
                    }
                    antigravity::Event::BeforeModel => ("before-model", payload.roots),
                    antigravity::Event::Stop { eligible: true } => {
                        let adapters = wiring::build();
                        let ports = adapters.ports();
                        ("stop", discover_stop_scopes(&ports, &payload.roots))
                    }
                    antigravity::Event::Stop { eligible: false } => {
                        ("stop (ineligible)", payload.roots)
                    }
                    antigravity::Event::Ignore => ("ignore", payload.roots),
                };
                Ok((event, payload.model, scopes))
            }
            Err(e) => Err(e),
        },
        other => {
            let adapter: &dyn Harness = match other {
                HarnessKind::Claude => &Claude,
                HarnessKind::Codex => &Codex,
                HarnessKind::Qoder => &Qoder,
                HarnessKind::Zcode => &ZCode,
                HarnessKind::Kimi => &Kimi,
                HarnessKind::Cursor => &Cursor,
                HarnessKind::Opencode => &Opencode,
                HarnessKind::Hermes => &Hermes,
                HarnessKind::Pi => &Pi,
                HarnessKind::Openclaw => &Openclaw,
                HarnessKind::Antigravity => unreachable!(),
            };
            adapter.parse_event(&stdin).map(|parsed| {
                let invocation = parsed.invocation.canonical();
                let (event, scopes) = match &invocation {
                    Invocation::Start(req) => {
                        let adapters = wiring::build();
                        let ports = adapters.ports();
                        (
                            "start",
                            discover_start_scopes(&ports, std::slice::from_ref(&req.cwd)),
                        )
                    }
                    Invocation::Edit(req) => {
                        ("edit", vec![hooks::model::edit_dir(req).to_path_buf()])
                    }
                    Invocation::EditBatch(reqs) => {
                        let mut dirs = Vec::new();
                        for req in reqs {
                            let dir = hooks::model::edit_dir(req).to_path_buf();
                            if !dirs.contains(&dir) {
                                dirs.push(dir);
                            }
                        }
                        ("edit", dirs)
                    }
                    Invocation::Stop { request, .. } => {
                        let adapters = wiring::build();
                        let ports = adapters.ports();
                        (
                            "stop",
                            discover_stop_scopes(&ports, std::slice::from_ref(&request.root)),
                        )
                    }
                    Invocation::Ignore if parsed.model.model_switch => (
                        "model-switch",
                        parsed.model.cwd.clone().into_iter().collect(),
                    ),
                    Invocation::Ignore => {
                        ("ignore", parsed.model.cwd.clone().into_iter().collect())
                    }
                };
                (event, parsed.model, scopes)
            })
        }
    };
    let (event, hint, mut scopes) = match parsed {
        Ok(parts) => parts,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "hookah: config: {e}");
            return 1;
        }
    };
    if scopes.is_empty() {
        scopes.push(fallback_dir.to_path_buf());
    }
    let resolved =
        hooks::model::resolve_model_with_source(&harness_name, &hint).map(|(id, source)| {
            let label = match source {
                hooks::model::ModelSource::Payload => "payload".to_owned(),
                hooks::model::ModelSource::SessionCache => "session cache".to_owned(),
                hooks::model::ModelSource::Transcript(path) => {
                    format!("transcript: {}", slashed(&path))
                }
            };
            (id, label)
        });
    let host = Host::from_process();
    let evaluated: Vec<(&Path, _)> = scopes
        .iter()
        .map(|scope| (scope.as_path(), resolve_env(scope, &host)))
        .collect();
    let model_ref = resolved
        .as_ref()
        .map(|(id, source)| (id.as_str(), source.as_str()));
    print_out(&render::hook_model_check(
        &harness_name,
        event,
        model_ref,
        &evaluated,
    ));
    0
}

fn discover_start_scopes(ports: &hookah_application::Ports<'_>, roots: &[PathBuf]) -> Vec<PathBuf> {
    let start = StartSession::new(*ports);
    let mut scopes = Vec::new();
    for root in roots {
        let found = start.discover_roots(root);
        if found.is_empty() {
            if !scopes.contains(root) {
                scopes.push(root.clone());
            }
        } else {
            for dir in found {
                if !scopes.contains(&dir) {
                    scopes.push(dir);
                }
            }
        }
    }
    scopes
}

fn discover_stop_scopes(ports: &hookah_application::Ports<'_>, roots: &[PathBuf]) -> Vec<PathBuf> {
    let stop = CheckSession::new(*ports, 1);
    let mut scopes = Vec::new();
    for root in roots {
        for dir in stop.discover_scopes(root) {
            if !scopes.contains(&dir) {
                scopes.push(dir);
            }
        }
    }
    scopes
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
    match adapters.detector().detect_traced(&project) {
        Ok((toolchain, origin)) => {
            print_out(&render::detection(
                style,
                Some((&project, &toolchain, origin)),
            ));
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
        HarnessKind::Claude => run_harness("claude", &Claude, &stdin, limit),
        HarnessKind::Codex => run_harness("codex", &Codex, &stdin, limit),
        HarnessKind::Qoder => run_harness("qoder", &Qoder, &stdin, limit),
        HarnessKind::Zcode => run_harness("zcode", &ZCode, &stdin, limit),
        HarnessKind::Kimi => run_harness("kimi", &Kimi, &stdin, limit),
        HarnessKind::Antigravity => hooks::antigravity::run(&stdin, limit),
        HarnessKind::Cursor => run_harness("cursor", &Cursor, &stdin, limit),
        HarnessKind::Opencode => run_harness("opencode", &Opencode, &stdin, limit),
        HarnessKind::Hermes => run_harness("hermes", &Hermes, &stdin, limit),
        HarnessKind::Pi => run_harness("pi", &Pi, &stdin, limit),
        HarnessKind::Openclaw => run_harness("openclaw", &Openclaw, &stdin, limit),
    }
}

fn run_harness(name: &str, harness: &dyn Harness, stdin: &str, limit: usize) -> Response {
    let parsed = match harness.parse_event(stdin) {
        Ok(parsed) => parsed,
        Err(e) => return warning(e.to_string()),
    };
    let invocation = parsed.invocation.canonical();
    let gate = hooks::model::ModelGate::new(name, &parsed.model);
    let invocation = hooks::model::filter_edits(invocation, &gate);
    let outcome = run_invocation(&invocation, limit, &gate);
    hooks::model::record_session_model(name, &invocation, &parsed.model, &gate);
    harness.render(&invocation, &outcome)
}

fn run_invocation(
    invocation: &Invocation,
    limit: usize,
    gate: &hooks::model::ModelGate<'_>,
) -> Outcome {
    if matches!(invocation, Invocation::Ignore) {
        return Outcome::pass();
    }
    let adapters = wiring::build();
    let ports = adapters.ports();
    match invocation {
        Invocation::Edit(request) => CheckEdit::new(ports, limit).run(request),
        Invocation::EditBatch(requests) => {
            let use_case = CheckEdit::new(ports, limit);
            merge_outcomes(requests.iter().map(|r| use_case.run(r)).collect(), limit)
        }
        Invocation::Stop { request, .. } => {
            let allow = |dir: &Path| gate.allows(dir);
            CheckSession::new(ports, limit)
                .with_budgets(wiring::budgets())
                .with_dir_filter(&allow)
                .run(request)
        }
        Invocation::Start(request) => {
            let allow = |dir: &Path| gate.allows(dir);
            let mut outcome = StartSession::new(ports)
                .with_dir_filter(&allow)
                .run(request);
            outcome.notices.extend(update::session_start_notice());
            outcome
        }
        Invocation::Ignore => Outcome::pass(),
    }
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
