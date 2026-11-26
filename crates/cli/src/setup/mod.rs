//! `hookah setup`: wiring only. The merging lives in `hookah-setup`; this module picks the
//! scope and the command, calls it, and prints.

mod render;
mod uninstall;

pub use uninstall::run as uninstall;

use std::io::Write;

use hookah_setup::{Env, Program, Scope, SetupError, Target, commit, detect, plan, snippet};

use crate::args::{HarnessKind, ScopeKind, SetupArgs};

/// Runs the command; returns the process exit code.
pub fn run(args: &SetupArgs) -> i32 {
    let env = Env::from_process();
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "hookah".into());
    let (code, out) = match execute(args, &env, &exe) {
        Ok(out) => (0, out),
        Err(message) => {
            let _ = writeln!(std::io::stderr(), "hookah: setup: {message}");
            (1, String::new())
        }
    };
    let _ = write!(std::io::stdout(), "{out}");
    code
}

fn execute(args: &SetupArgs, env: &Env, exe: &str) -> Result<String, String> {
    let Some(kind) = args.harness else {
        return Ok(render::detection(&detect(env)));
    };
    let target = target(kind);
    let program = args
        .command
        .as_deref()
        .map_or(Program::Executable(exe), Program::Shell);
    if args.print {
        return snippet(target, program);
    }
    let scope = match args.scope {
        Some(ScopeKind::Project) => Scope::Project,
        Some(ScopeKind::User) => Scope::User,
        None => Scope::default_for(target),
    };
    let planned = plan(target, scope, env, program).map_err(|e: SetupError| e.to_string())?;
    if args.dry_run {
        return Ok(render::dry_run(&planned, &env.cwd));
    }
    let written = commit(&planned).map_err(|e| e.to_string())?;
    let mut out = render::applied(&planned, &written, &env.cwd, &program.shell_command());
    if matches!(target, Target::Opencode | Target::Pi | Target::Openclaw) && args.command.is_some()
    {
        out.push_str(&format!(
            "note: --command is ignored for {}; the plugin runs `hookah` from PATH or HOOKAH_BIN\n",
            target.name()
        ));
    }
    if target == Target::Hermes {
        if args.command.is_some() {
            out.push_str("note: --command is ignored for Hermes; the plugin runs `hookah` from PATH or HOOKAH_BIN\n");
        }
        out.push_str("note: Hermes abandons a plugin hook after plugins.hook_callback_timeout seconds (30 by default); raise it in config.yaml if the stop checks run slow tests\n");
    }
    Ok(out)
}

pub(super) fn target(kind: HarnessKind) -> Target {
    match kind {
        HarnessKind::Claude => Target::Claude,
        HarnessKind::Codex => Target::Codex,
        HarnessKind::Qoder => Target::Qoder,
        HarnessKind::Zcode => Target::Zcode,
        HarnessKind::Kimi => Target::Kimi,
        HarnessKind::Antigravity => Target::Antigravity,
        HarnessKind::Cursor => Target::Cursor,
        HarnessKind::Opencode => Target::Opencode,
        HarnessKind::Hermes => Target::Hermes,
        HarnessKind::Pi => Target::Pi,
        HarnessKind::Openclaw => Target::Openclaw,
    }
}
