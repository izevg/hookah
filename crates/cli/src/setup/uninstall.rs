//! `hookah uninstall`: wiring and plain-text output. The removal itself lives in `hookah-setup`;
//! this module picks what to remove, applies it, and reports for an agent to read: one fact per
//! line, no color.

use std::io::Write;
use std::path::{Path, PathBuf};

use hookah_adapters::store::cache_dir::{CachePurge, cache_purge};
use hookah_domain::path::display_path;
use hookah_setup::{
    Env, Removal, Scope, SetupError, Step, Target, detect, plan_orca_removal, plan_removal, remove,
};

use super::target;
use crate::args::{ScopeKind, UninstallArgs};

/// Where `hookah plugin orca --out` is documented to write, relative to the home directory.
const ORCA_DIR: &str = ".hookah-orca-plugin";

/// What a run produced: the text for stdout, and whether some file had to stay.
struct Outcome {
    text: String,
    refused: bool,
}

/// Runs the command; returns the process exit code.
pub fn run(args: &UninstallArgs) -> i32 {
    let env = Env::from_process();
    let cache = args.purge_cache.then(cache_purge);
    let (code, out) = match execute(args, &env, cache) {
        Ok(out) => (i32::from(out.refused), out.text),
        Err(message) => {
            let _ = writeln!(std::io::stderr(), "hookah: uninstall: {message}");
            (1, String::new())
        }
    };
    let _ = write!(std::io::stdout(), "{out}");
    code
}

fn execute(
    args: &UninstallArgs,
    env: &Env,
    cache: Option<Option<CachePurge>>,
) -> Result<Outcome, String> {
    let removals = removals(args, env)?;
    if !args.dry_run {
        for removal in &removals {
            remove(removal).map_err(|e| e.to_string())?;
        }
    }
    let mut text = String::new();
    for removal in &removals {
        text.push_str(&removal_report(removal, &env.cwd, args.dry_run));
    }
    if !removals.is_empty() {
        text.push_str(
            "note: the hookah binary is not removed by this command; see docs/setup/uninstall.md\n",
        );
    } else if args.all {
        text.push_str("nothing to remove: hookah is not configured for any harness\n");
    }
    if let Some(cache) = cache {
        text.push_str(&cache_report(cache, args.dry_run)?);
    }
    Ok(Outcome {
        refused: removals.iter().any(Removal::refused),
        text,
    })
}

/// The removals the arguments ask for, planned and not applied.
fn removals(args: &UninstallArgs, env: &Env) -> Result<Vec<Removal>, String> {
    let plan = |t: Target, scope: Scope| plan_removal(t, scope, env, args.force);
    let fail = |e: SetupError| e.to_string();
    if args.all {
        return configured(env, args.scope)
            .into_iter()
            .map(|(t, scope)| plan(t, scope).map_err(fail))
            .collect();
    }
    let Some(kind) = args.harness else {
        return Ok(vec![]);
    };
    let Some(harness) = kind.harness() else {
        if args.scope.is_some() {
            return Err("the Orca plugin has no scope; use --dir to name its folder".into());
        }
        return Ok(vec![plan_orca_removal(&orca_dir(args, env)?, args.force)]);
    };
    if args.dir.is_some() {
        return Err("--dir only applies to `orca`".into());
    }
    let t = target(harness);
    let scope = scope_of(args.scope).unwrap_or_else(|| Scope::default_for(t));
    Ok(vec![plan(t, scope).map_err(fail)?])
}

fn orca_dir(args: &UninstallArgs, env: &Env) -> Result<PathBuf, String> {
    match (&args.dir, &env.home) {
        (Some(dir), _) => Ok(dir.clone()),
        (None, Some(home)) => Ok(home.join(ORCA_DIR)),
        (None, None) => Err(SetupError::NoHome.to_string()),
    }
}

fn scope_of(kind: Option<ScopeKind>) -> Option<Scope> {
    kind.map(|k| match k {
        ScopeKind::Project => Scope::Project,
        ScopeKind::User => Scope::User,
    })
}

/// Every harness and scope `hookah setup --detect` reports as configured, project scope first.
fn configured(env: &Env, scope: Option<ScopeKind>) -> Vec<(Target, Scope)> {
    let wanted = scope_of(scope);
    let mut found = Vec::new();
    for scope in [Scope::Project, Scope::User] {
        if wanted.is_some_and(|w| w != scope) {
            continue;
        }
        for d in detect(env) {
            let on = match scope {
                Scope::Project => d.project == Some(true),
                Scope::User => d.user,
            };
            if on {
                found.push((d.target, scope));
            }
        }
    }
    found
}

/// One removal as text.
fn removal_report(removal: &Removal, cwd: &Path, dry_run: bool) -> String {
    let shown = |p: &Path| display_path(p, cwd).display().to_string();
    let scope = removal
        .scope
        .map_or(String::new(), |s| format!(", {} scope", s.name()));
    let mut out = format!(
        "hookah uninstall: {}{scope}{}\n",
        removal.name,
        if dry_run {
            " (dry run, nothing written)"
        } else {
            ""
        }
    );
    if removal.steps.is_empty() {
        out.push_str(&format!(
            "nothing to remove: hookah is not set up for {}{}\n",
            removal.name,
            removal
                .scope
                .map_or(String::new(), |s| format!(" at {} scope", s.name()))
        ));
        return out;
    }
    let would = if dry_run { "would " } else { "" };
    for step in &removal.steps {
        out.push_str(&match step {
            Step::Rewrite { path, .. } => format!(
                "{would}{} hookah's entries from {}\n",
                if dry_run { "remove" } else { "removed" },
                shown(path)
            ),
            Step::Delete { path } => format!(
                "{would}{} {}\n",
                if dry_run { "delete" } else { "deleted" },
                shown(path)
            ),
            Step::Keep { path, reason } => format!("kept {}: {reason}\n", shown(path)),
        });
    }
    for backup in &removal.backups {
        out.push_str(&format!(
            "kept {}: the copy of the original that setup made; hookah never restores it, delete it when you no longer need it\n",
            shown(backup)
        ));
    }
    if removal.changed() {
        out.push_str(&next(removal));
    }
    out
}

/// What the user does after files were removed.
fn next(removal: &Removal) -> String {
    let mut out = String::new();
    match removal.target {
        Some(Target::Openclaw) => out.push_str(
            "next: run `openclaw plugins disable hookah` (hookah does not edit OpenClaw's own config), then restart the OpenClaw Gateway. An entry plugins.entries.hookah may remain in OpenClaw's config; delete it there if you want it gone.\n",
        ),
        Some(t) => out.push_str(&format!(
            "next: restart {} so it stops loading hookah.\n",
            t.title()
        )),
        None => out.push_str(
            "next: remove the plugin from Orca too: `orca plugin uninstall hookah` if you installed it, or `orca plugin dev unload <folder>` if you loaded it for development.\n",
        ),
    }
    out
}

fn cache_report(cache: Option<CachePurge>, dry_run: bool) -> Result<String, String> {
    let Some(purge) = cache else {
        return Ok("cache: no cache directory could be located; nothing to delete\n".into());
    };
    if purge.paths.is_empty() {
        return Ok(format!(
            "cache: nothing to delete in {}\n",
            purge.base.display()
        ));
    }
    if !dry_run {
        purge
            .run()
            .map_err(|e| format!("cannot delete the cache {}: {e}", purge.base.display()))?;
    }
    let verb = if dry_run { "would delete" } else { "deleted" };
    Ok(purge
        .paths
        .iter()
        .map(|p| format!("cache: {verb} {}\n", p.display()))
        .collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn removal(steps: Vec<Step>, target: Option<Target>) -> Removal {
        let mut r = Removal::new("claude", Some(Scope::Project), target, steps, None);
        r.backups = vec!["/p/.claude/settings.json.hookah.bak".into()];
        r
    }

    #[test]
    fn an_applied_removal_lists_each_file_the_backup_and_the_restart() {
        let r = removal(
            vec![
                Step::Rewrite {
                    path: "/p/.claude/settings.json".into(),
                    text: String::new(),
                },
                Step::Delete {
                    path: "/p/.claude/x".into(),
                },
                Step::Keep {
                    path: "/p/.claude/y".into(),
                    reason: "because".into(),
                },
            ],
            Some(Target::Claude),
        );
        assert_eq!(
            removal_report(&r, Path::new("/p"), false),
            "hookah uninstall: claude, project scope\n\
             removed hookah's entries from .claude/settings.json\n\
             deleted .claude/x\n\
             kept .claude/y: because\n\
             kept .claude/settings.json.hookah.bak: the copy of the original that setup made; hookah never restores it, delete it when you no longer need it\n\
             next: restart Claude Code so it stops loading hookah.\n"
        );
    }

    #[test]
    fn a_dry_run_says_would_and_an_empty_removal_says_nothing_to_remove() {
        let r = removal(
            vec![Step::Delete {
                path: "/p/.claude/x".into(),
            }],
            Some(Target::Claude),
        );
        let out = removal_report(&r, Path::new("/p"), true);
        assert!(out.starts_with(
            "hookah uninstall: claude, project scope (dry run, nothing written)\nwould delete .claude/x\n"
        ));
        let r = Removal::new(
            "claude",
            Some(Scope::User),
            Some(Target::Claude),
            vec![],
            None,
        );
        assert_eq!(
            removal_report(&r, Path::new("/p"), false),
            "hookah uninstall: claude, user scope\nnothing to remove: hookah is not set up for claude at user scope\n"
        );
        let r = removal(
            vec![Step::Rewrite {
                path: "/p/a".into(),
                text: String::new(),
            }],
            None,
        );
        assert!(
            removal_report(&r, Path::new("/p"), true)
                .contains("would remove hookah's entries from a\n")
        );
    }

    #[test]
    fn a_refusal_alone_prints_no_next_step() {
        let r = Removal::new(
            "pi",
            Some(Scope::Project),
            Some(Target::Pi),
            vec![Step::Keep {
                path: "/p/a".into(),
                reason: "edited".into(),
            }],
            None,
        );
        let out = removal_report(&r, Path::new("/p"), false);
        assert!(out.contains("kept a: edited\n") && !out.contains("next:"));
    }

    #[test]
    fn openclaw_and_orca_get_their_own_next_steps() {
        let step = vec![Step::Delete {
            path: "/p/a".into(),
        }];
        let r = Removal::new(
            "openclaw",
            Some(Scope::User),
            Some(Target::Openclaw),
            step.clone(),
            None,
        );
        assert!(
            removal_report(&r, Path::new("/p"), false)
                .contains("next: run `openclaw plugins disable hookah`")
        );
        let r = Removal::new("orca", None, None, step, None);
        let out = removal_report(&r, Path::new("/p"), false);
        assert!(out.starts_with("hookah uninstall: orca\n"), "{out}");
        assert!(
            out.contains("orca plugin uninstall hookah") && out.contains("orca plugin dev unload")
        );
    }

    #[test]
    fn the_cache_report_covers_every_state() {
        assert!(
            cache_report(None, false)
                .unwrap()
                .contains("no cache directory")
        );
        let empty = CachePurge {
            base: "/c".into(),
            paths: vec![],
            shared: false,
        };
        assert_eq!(
            cache_report(Some(empty), false).unwrap(),
            "cache: nothing to delete in /c\n"
        );
        let dir = std::env::temp_dir().join(format!("hookah-cli-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let some = CachePurge {
            base: dir.clone(),
            paths: vec![dir.clone()],
            shared: false,
        };
        let out = cache_report(Some(some.clone()), true).unwrap();
        assert_eq!(out, format!("cache: would delete {}\n", dir.display()));
        assert!(dir.exists());
        let out = cache_report(Some(some.clone()), false).unwrap();
        assert_eq!(out, format!("cache: deleted {}\n", dir.display()));
        assert!(!dir.exists());
        assert!(cache_report(Some(some), false).is_err());
    }
}
