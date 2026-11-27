//! Plain-text output of `hookah setup`, written for an agent to read: short, no color, one fact
//! per line.

use std::path::Path;

use hookah_domain::path::display_path;
use hookah_setup::{Detection, Plan, Target, Written};

/// The result of an applied plan.
pub fn applied(plan: &Plan, written: &Written, cwd: &Path, program: &str) -> String {
    let path = shown(&plan.path, cwd);
    let mut out = header(plan, "");
    out.push_str(&match (&plan.before, written.wrote) {
        (None, _) => format!("created {path}\n"),
        (Some(_), true) => format!("updated {path}\n"),
        (Some(_), false) => format!("already up to date, nothing written: {path}\n"),
    });
    for file in &plan.companions {
        out.push_str(&match (&file.before, file.changed()) {
            (None, _) => format!("created {}\n", shown(&file.path, cwd)),
            (Some(_), true) => format!("updated {}\n", shown(&file.path, cwd)),
            (Some(_), false) => format!("already up to date: {}\n", shown(&file.path, cwd)),
        });
    }
    for backup in written.backup.iter().chain(&written.companion_backups) {
        out.push_str(&format!("backup of the original: {}\n", shown(backup, cwd)));
    }
    out.push_str(&what(plan.target, program));
    out.push_str(&format!(
        "verify: run `{program} setup --detect` (it should list {} as configured), then `{program} detect <a source file>`\n",
        plan.target.name()
    ));
    out.push_str(&format!("next: {}\n", plan.target.restart_hint()));
    out
}

/// The result of `--dry-run`: the file as it would be, and nothing written.
pub fn dry_run(plan: &Plan, cwd: &Path) -> String {
    let path = shown(&plan.path, cwd);
    let mut out = header(plan, " (dry run, nothing written)");
    out.push_str(&match (&plan.before, plan.changed()) {
        (None, _) => format!("would create {path} with this content:\n"),
        (Some(_), true) => format!("would update {path}; it would read:\n"),
        (Some(_), false) => format!("{path} is already up to date and would stay:\n"),
    });
    out.push_str(&plan.after);
    for file in &plan.companions {
        let action = match (&file.before, file.changed()) {
            (None, _) => "would create",
            (Some(_), true) => "would update",
            (Some(_), false) => "would keep",
        };
        out.push_str(&format!("{action} {}\n", shown(&file.path, cwd)));
    }
    out
}

/// The result of `--detect`.
pub fn detection(found: &[Detection]) -> String {
    let mut out = String::new();
    for d in found {
        let installed = match (&d.binary, d.config_dir) {
            (Some(bin), _) => format!("installed (binary {})", bin.display()),
            (None, true) => "installed (config directory found, no binary on PATH)".to_owned(),
            (None, false) => "not found".to_owned(),
        };
        let project = match d.project {
            Some(true) => "configured",
            Some(false) => "not configured",
            None => "no project config",
        };
        let user = if d.user {
            "configured"
        } else {
            "not configured"
        };
        out.push_str(&format!(
            "{}: {installed}; project: {project}; user: {user}\n",
            d.target.name()
        ));
    }
    out.push_str(
        "next: for each installed harness you use that is not configured, run `hookah setup <name>`.\n",
    );
    out
}

fn header(plan: &Plan, suffix: &str) -> String {
    format!(
        "hookah setup: {}, {} scope{suffix}\n",
        plan.target.name(),
        plan.scope.name()
    )
}

fn what(target: Target, program: &str) -> String {
    if target == Target::Hermes {
        return "installed the bundled Hermes plugin (the same text as `hookah plugin hermes`) and enabled it in plugins.enabled\n"
            .to_owned();
    }
    if target.hooks().is_empty() {
        return format!(
            "installed the bundled {name} plugin (the same text as `hookah plugin {name}`)\n",
            name = target.name()
        );
    }
    let events: Vec<_> = target.hooks().iter().map(|h| h.event).collect();
    format!(
        "hooks: {} run `{program} hook {}`\n",
        events.join(", "),
        target.name()
    )
}

/// A path relative to the project when it is inside it, absolute otherwise, written with `/`
/// on every OS like the paths of a report.
fn shown(path: &Path, cwd: &Path) -> String {
    display_path(path, cwd).display().to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use hookah_setup::{Companion, Scope};

    use super::*;

    fn plan(target: Target, before: Option<&str>, after: &str) -> Plan {
        Plan {
            target,
            scope: Scope::Project,
            path: "/p/.claude/settings.json".into(),
            before: before.map(str::to_owned),
            after: after.to_owned(),
            companions: vec![],
        }
    }

    const CWD: &str = "/p";

    #[test]
    fn a_created_file_is_reported_with_hooks_verification_and_restart() {
        let p = plan(Target::Claude, None, "{}");
        let w = Written {
            wrote: true,
            backup: None,
            companion_backups: vec![],
        };
        let out = applied(&p, &w, Path::new(CWD), "/b/hookah");
        assert_eq!(
            out,
            "hookah setup: claude, project scope\n\
             created .claude/settings.json\n\
             hooks: SessionStart, PostModelSwitch, PostToolUse, Stop, SubagentStop run `/b/hookah hook claude`\n\
             verify: run `/b/hookah setup --detect` (it should list claude as configured), then `/b/hookah detect <a source file>`\n\
             next: Restart Claude Code, or run /hooks in it to review and approve the new hooks.\n"
        );
    }

    #[test]
    fn an_update_names_the_backup_and_an_unchanged_file_says_so() {
        let p = plan(Target::Claude, Some("{}"), "{ }");
        let updated = applied(
            &p,
            &Written {
                wrote: true,
                backup: Some("/p/.claude/settings.json.hookah.bak".into()),
                companion_backups: vec![],
            },
            Path::new(CWD),
            "hookah",
        );
        assert!(updated.contains("updated .claude/settings.json\n"));
        assert!(updated.contains("backup of the original: .claude/settings.json.hookah.bak\n"));
        let same = applied(
            &p,
            &Written {
                wrote: false,
                backup: None,
                companion_backups: vec![],
            },
            Path::new(CWD),
            "hookah",
        );
        assert!(same.contains("already up to date, nothing written: .claude/settings.json\n"));
        assert!(!same.contains("backup"));
    }

    #[test]
    fn paths_outside_the_project_stay_absolute() {
        let p = plan(Target::Claude, None, "{}");
        let w = Written {
            wrote: true,
            backup: None,
            companion_backups: vec![],
        };
        let out = applied(&p, &w, Path::new("/elsewhere"), "hookah");
        assert!(out.contains("created /p/.claude/settings.json\n"));
    }

    #[test]
    fn opencode_says_it_installed_the_plugin() {
        let p = plan(Target::Opencode, None, "// plugin");
        let w = Written {
            wrote: true,
            backup: None,
            companion_backups: vec![],
        };
        let out = applied(&p, &w, Path::new(CWD), "hookah");
        assert!(out.contains("installed the bundled opencode plugin"));
        assert!(!out.contains("hooks:"));
    }

    #[test]
    fn hermes_lists_the_plugin_files_and_their_backups() {
        let mut p = plan(Target::Hermes, Some("model: x\n"), "model: x\nplugins:\n");
        p.path = "/h/.hermes/config.yaml".into();
        p.companions = vec![
            Companion {
                path: "/h/.hermes/plugins/hookah/plugin.yaml".into(),
                before: None,
                after: "name: hookah\n".into(),
            },
            Companion {
                path: "/h/.hermes/plugins/hookah/__init__.py".into(),
                before: Some("old".into()),
                after: "new".into(),
            },
            Companion {
                path: "/h/.hermes/plugins/hookah/same.py".into(),
                before: Some("same".into()),
                after: "same".into(),
            },
        ];
        let w = Written {
            wrote: true,
            backup: Some("/h/.hermes/config.yaml.hookah.bak".into()),
            companion_backups: vec!["/h/.hermes/plugins/hookah/__init__.py.hookah.bak".into()],
        };
        let out = applied(&p, &w, Path::new("/h"), "hookah");
        assert!(out.contains("updated .hermes/config.yaml\ncreated .hermes/plugins/hookah/plugin.yaml\nupdated .hermes/plugins/hookah/__init__.py\nalready up to date: .hermes/plugins/hookah/same.py\n"));
        assert!(out.contains("backup of the original: .hermes/config.yaml.hookah.bak\nbackup of the original: .hermes/plugins/hookah/__init__.py.hookah.bak\n"));
        assert!(out.contains("installed the bundled Hermes plugin"));
        assert!(!out.contains("hooks:"));
        let dry = dry_run(&p, Path::new("/h"));
        assert!(dry.contains("would create .hermes/plugins/hookah/plugin.yaml\nwould update .hermes/plugins/hookah/__init__.py\nwould keep .hermes/plugins/hookah/same.py\n"));
    }

    #[test]
    fn pi_says_it_installed_the_plugin() {
        let p = plan(Target::Pi, None, "// plugin");
        let w = Written {
            wrote: true,
            backup: None,
            companion_backups: vec![],
        };
        let out = applied(&p, &w, Path::new(CWD), "hookah");
        assert!(
            out.contains("installed the bundled pi plugin (the same text as `hookah plugin pi`)")
        );
    }

    #[test]
    fn openclaw_lists_the_manifest_and_says_what_to_run_next() {
        let mut p = plan(Target::Openclaw, None, "// plugin");
        p.path = "/h/.openclaw/extensions/hookah/index.js".into();
        p.companions = vec![Companion {
            path: "/h/.openclaw/extensions/hookah/openclaw.plugin.json".into(),
            before: None,
            after: "{}".into(),
        }];
        let w = Written {
            wrote: true,
            backup: None,
            companion_backups: vec![],
        };
        let out = applied(&p, &w, Path::new("/h"), "hookah");
        assert!(out.contains(
            "created .openclaw/extensions/hookah/index.js\ncreated .openclaw/extensions/hookah/openclaw.plugin.json\n"
        ));
        assert!(out.contains(
            "installed the bundled openclaw plugin (the same text as `hookah plugin openclaw`)"
        ));
        assert!(out.contains("next: Run `openclaw plugins enable hookah`"));
    }

    #[test]
    fn dry_run_shows_the_file_and_the_action() {
        let create = dry_run(&plan(Target::Claude, None, "NEW\n"), Path::new(CWD));
        assert_eq!(
            create,
            "hookah setup: claude, project scope (dry run, nothing written)\n\
             would create .claude/settings.json with this content:\nNEW\n"
        );
        let update = dry_run(&plan(Target::Claude, Some("OLD"), "NEW\n"), Path::new(CWD));
        assert!(update.contains("would update .claude/settings.json; it would read:\nNEW\n"));
        let same = dry_run(&plan(Target::Claude, Some("X"), "X"), Path::new(CWD));
        assert!(same.contains("is already up to date and would stay:\nX"));
    }

    #[test]
    fn detection_lists_every_state() {
        let found = [
            Detection {
                target: Target::Claude,
                binary: Some("/bin/claude".into()),
                config_dir: true,
                project: Some(true),
                user: false,
            },
            Detection {
                target: Target::Kimi,
                binary: None,
                config_dir: true,
                project: None,
                user: true,
            },
            Detection {
                target: Target::Codex,
                binary: None,
                config_dir: false,
                project: Some(false),
                user: false,
            },
        ];
        let out = detection(&found);
        assert!(out.contains(
            "claude: installed (binary /bin/claude); project: configured; user: not configured\n"
        ));
        assert!(out.contains(
            "kimi: installed (config directory found, no binary on PATH); project: no project config; user: configured\n"
        ));
        assert!(out.contains("codex: not found; project: not configured; user: not configured\n"));
        assert!(out.ends_with("run `hookah setup <name>`.\n"));
    }
}
