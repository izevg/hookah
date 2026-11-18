//! `hookah setup`: wiring only. The merging lives in `hookah-setup`; this module picks the
//! scope and the command, calls it, and prints.

mod render;

use std::io::Write;

use hookah_setup::{Env, Scope, SetupError, Target, commit, detect, plan, shell_program, snippet};

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
    let program = args.command.clone().unwrap_or_else(|| shell_program(exe));
    if args.print {
        return snippet(target, &program);
    }
    let scope = match args.scope {
        Some(ScopeKind::Project) => Scope::Project,
        Some(ScopeKind::User) => Scope::User,
        None => Scope::default_for(target),
    };
    let planned = plan(target, scope, env, &program).map_err(|e: SetupError| e.to_string())?;
    if args.dry_run {
        return Ok(render::dry_run(&planned, &env.cwd));
    }
    let written = commit(&planned).map_err(|e| e.to_string())?;
    let mut out = render::applied(&planned, &written, &env.cwd, &program);
    if target == Target::Opencode && args.command.is_some() {
        out.push_str("note: --command is ignored for opencode; the plugin runs `hookah` from PATH or HOOKAH_BIN\n");
    }
    Ok(out)
}

fn target(kind: HarnessKind) -> Target {
    match kind {
        HarnessKind::Claude => Target::Claude,
        HarnessKind::Codex => Target::Codex,
        HarnessKind::Qoder => Target::Qoder,
        HarnessKind::Kimi => Target::Kimi,
        HarnessKind::Cursor => Target::Cursor,
        HarnessKind::Opencode => Target::Opencode,
    }
}
