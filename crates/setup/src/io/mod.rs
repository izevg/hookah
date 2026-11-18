//! Reading, backing up, and writing the config files. See [`env`] for where they live and
//! [`detect`] for the read-only survey.

pub mod detect;
pub mod env;

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::error::SetupError;
use crate::merge::{companions, merge};
use crate::target::Target;
use env::{Env, Scope};

/// What `hookah setup` would do to one config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The harness.
    pub target: Target,
    /// The scope the file belongs to.
    pub scope: Scope,
    /// The config file.
    pub path: PathBuf,
    /// The file as it is now, or `None` if it does not exist.
    pub before: Option<String>,
    /// The file as it should be.
    pub after: String,
    /// The other files the harness needs, written with the config file (Hermes's plugin).
    pub companions: Vec<Companion>,
}

/// A file written next to the config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Companion {
    /// Where it goes.
    pub path: PathBuf,
    /// The file as it is now, or `None` if it does not exist.
    pub before: Option<String>,
    /// The file as it should be.
    pub after: String,
}

impl Companion {
    /// True if applying the plan changes the file.
    pub fn changed(&self) -> bool {
        self.before.as_deref() != Some(self.after.as_str())
    }
}

impl Plan {
    /// True if applying the plan changes any file.
    pub fn changed(&self) -> bool {
        self.before.as_deref() != Some(self.after.as_str())
            || self.companions.iter().any(Companion::changed)
    }
}

/// What [`commit`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// False when the file was already up to date and nothing was written.
    pub wrote: bool,
    /// The backup of the original, when this run created one.
    pub backup: Option<PathBuf>,
    /// The backups of overwritten companion files made by this run.
    pub companion_backups: Vec<PathBuf>,
}

/// Reads the config file and works out its new content. Writes nothing.
///
/// `program` starts hookah in each hook command (see [`merge`]).
pub fn plan(target: Target, scope: Scope, env: &Env, program: &str) -> Result<Plan, SetupError> {
    let path = env.config_path(target, scope)?;
    let before = read(&path)?;
    let after =
        merge(target, before.as_deref(), program).map_err(|reason| SetupError::Invalid {
            path: path.clone(),
            reason,
        })?;
    let mut files = Vec::new();
    for (relative, after) in companions(target) {
        let dir = env.user_dir(target).ok_or(SetupError::NoHome)?;
        let path = dir.join(relative);
        files.push(Companion {
            before: read(&path)?,
            path,
            after: (*after).to_owned(),
        });
    }
    Ok(Plan {
        target,
        scope,
        path,
        before,
        after,
        companions: files,
    })
}

fn read(path: &Path) -> Result<Option<String>, SetupError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(source) => Err(SetupError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// What setup would add to a config file that does not exist yet: the reference snippet.
pub fn snippet(target: Target, program: &str) -> Result<String, String> {
    merge(target, None, program)
}

/// Applies a plan: creates parent directories, backs the original up once to
/// `<file>.hookah.bak`, and writes the new content through a temporary file and a rename, so a
/// crash never leaves half a config.
pub fn commit(plan: &Plan) -> Result<Written, SetupError> {
    if !plan.changed() {
        return Ok(Written {
            wrote: false,
            backup: None,
            companion_backups: vec![],
        });
    }
    let backup = write(&plan.path, plan.before.as_deref(), &plan.after)?;
    let mut companion_backups = Vec::new();
    for file in &plan.companions {
        companion_backups.extend(write(&file.path, file.before.as_deref(), &file.after)?);
    }
    Ok(Written {
        wrote: true,
        backup,
        companion_backups,
    })
}

/// Writes one file if it changes; returns the backup it made.
fn write(path: &Path, before: Option<&str>, after: &str) -> Result<Option<PathBuf>, SetupError> {
    if before == Some(after) {
        return Ok(None);
    }
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| SetupError::Io { path, source }
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io(parent))?;
    }
    let backup = backup_path(path);
    let mut made = None;
    if before.is_some() && !backup.exists() {
        fs::copy(path, &backup).map_err(io(&backup))?;
        made = Some(backup);
    }
    let tmp = sibling(path, ".hookah.tmp");
    fs::write(&tmp, after).map_err(io(&tmp))?;
    fs::rename(&tmp, path).map_err(io(path))?;
    Ok(made)
}

/// `<file>.hookah.bak`.
pub fn backup_path(path: &Path) -> PathBuf {
    sibling(path, ".hookah.bak")
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn temp_env(name: &str) -> Env {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "hookah-setup-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Env {
            cwd: root.join("project"),
            home: Some(root.join("home")),
            xdg_config: None,
            pi_agent_dir: None,
            openclaw_state: None,
            path: vec![],
            hermes_home: None,
        }
    }

    fn apply(env: &Env, target: Target, scope: Scope) -> Result<(Plan, Written), SetupError> {
        let plan = plan(target, scope, env, "hookah")?;
        let written = commit(&plan)?;
        Ok((plan, written))
    }

    #[test]
    fn a_missing_file_is_created_with_its_parents_and_needs_no_backup() {
        let env = temp_env("create");
        let (plan, written) = apply(&env, Target::Claude, Scope::Project).unwrap();
        assert!(plan.before.is_none());
        assert!(written.wrote);
        assert_eq!(written.backup, None);
        assert_eq!(fs::read_to_string(&plan.path).unwrap(), plan.after);
        assert!(!backup_path(&plan.path).exists());
        assert!(!sibling(&plan.path, ".hookah.tmp").exists());
    }

    #[test]
    fn the_original_is_backed_up_once_and_the_second_run_changes_nothing() {
        let env = temp_env("backup");
        let path = env.config_path(Target::Claude, Scope::Project).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{\"model\": \"x\"}\n").unwrap();
        let (_, first) = apply(&env, Target::Claude, Scope::Project).unwrap();
        assert!(first.wrote);
        let backup = first.backup.unwrap();
        assert_eq!(backup, backup_path(&path));
        assert_eq!(fs::read_to_string(&backup).unwrap(), "{\"model\": \"x\"}\n");
        let after_first = fs::read(&path).unwrap();
        let (plan, second) = apply(&env, Target::Claude, Scope::Project).unwrap();
        assert!(!plan.changed());
        assert_eq!(
            second,
            Written {
                wrote: false,
                backup: None,
                companion_backups: vec![]
            }
        );
        assert_eq!(fs::read(&path).unwrap(), after_first);
        assert_eq!(fs::read_to_string(&backup).unwrap(), "{\"model\": \"x\"}\n");
    }

    #[test]
    fn an_existing_backup_is_never_overwritten() {
        let env = temp_env("keepbak");
        let path = env.config_path(Target::Codex, Scope::Project).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "a = 1\n").unwrap();
        fs::write(backup_path(&path), "oldest").unwrap();
        let (_, written) = apply(&env, Target::Codex, Scope::Project).unwrap();
        assert!(written.wrote);
        assert_eq!(written.backup, None);
        assert_eq!(fs::read_to_string(backup_path(&path)).unwrap(), "oldest");
    }

    #[test]
    fn a_malformed_file_is_refused_and_left_untouched() {
        let env = temp_env("bad");
        let path = env.config_path(Target::Qoder, Scope::Project).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ nope").unwrap();
        let err = plan(Target::Qoder, Scope::Project, &env, "hookah").unwrap_err();
        let shown = err.to_string();
        assert!(shown.contains("not valid JSON"), "{shown}");
        assert!(shown.contains("left untouched"), "{shown}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ nope");
        assert!(!backup_path(&path).exists());
    }

    #[test]
    fn a_directory_in_place_of_the_file_is_an_io_error() {
        let env = temp_env("dir");
        let path = env.config_path(Target::Claude, Scope::Project).unwrap();
        fs::create_dir_all(&path).unwrap();
        let err = plan(Target::Claude, Scope::Project, &env, "hookah").unwrap_err();
        assert!(matches!(err, SetupError::Io { .. }));
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn hermes_writes_the_plugin_files_and_enables_them_and_a_second_run_changes_nothing() {
        let env = temp_env("hermes");
        let (plan, written) = apply(&env, Target::Hermes, Scope::User).unwrap();
        let home = env.home.clone().unwrap().join(".hermes");
        assert_eq!(plan.path, home.join("config.yaml"));
        assert_eq!(plan.after, "plugins:\n  enabled:\n    - hookah\n");
        assert!(written.wrote && written.backup.is_none());
        assert!(written.companion_backups.is_empty());
        let manifest = fs::read_to_string(home.join("plugins/hookah/plugin.yaml")).unwrap();
        assert_eq!(manifest, hookah_harness::plugins::hermes::PLUGIN_MANIFEST);
        let source = fs::read_to_string(home.join("plugins/hookah/__init__.py")).unwrap();
        assert_eq!(source, hookah_harness::plugins::hermes::PLUGIN_SOURCE);
        let (again, written) = apply(&env, Target::Hermes, Scope::User).unwrap();
        assert!(!again.changed());
        assert!(!written.wrote);
    }

    #[test]
    fn a_stale_companion_alone_is_rewritten_and_backed_up_once_without_touching_the_config() {
        let env = temp_env("hermes-stale");
        apply(&env, Target::Hermes, Scope::User).unwrap();
        let home = env.home.clone().unwrap().join(".hermes");
        let config = fs::read(home.join("config.yaml")).unwrap();
        let init = home.join("plugins/hookah/__init__.py");
        fs::write(&init, "old plugin").unwrap();
        let (plan, written) = apply(&env, Target::Hermes, Scope::User).unwrap();
        assert!(plan.changed());
        assert!(written.wrote);
        assert_eq!(written.backup, None);
        assert_eq!(written.companion_backups, [backup_path(&init)]);
        assert_eq!(
            fs::read_to_string(backup_path(&init)).unwrap(),
            "old plugin"
        );
        assert_eq!(
            fs::read_to_string(&init).unwrap(),
            hookah_harness::plugins::hermes::PLUGIN_SOURCE
        );
        assert_eq!(fs::read(home.join("config.yaml")).unwrap(), config);
        assert!(!backup_path(&home.join("config.yaml")).exists());
    }

    #[test]
    fn a_refused_config_writes_no_plugin_files() {
        let env = temp_env("hermes-refused");
        let home = env.home.clone().unwrap().join(".hermes");
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("config.yaml"), "plugins: {}\n").unwrap();
        let err = plan(Target::Hermes, Scope::User, &env, "hookah").unwrap_err();
        assert!(err.to_string().contains("by hand"), "{err}");
        assert!(!home.join("plugins").exists());
        assert!(plan(Target::Hermes, Scope::Project, &env, "hookah").is_err());
    }

    #[test]
    fn hermes_needs_a_home_directory_for_its_plugin_files() {
        let mut env = temp_env("hermes-nohome");
        env.home = None;
        assert!(matches!(
            plan(Target::Hermes, Scope::User, &env, "hookah"),
            Err(SetupError::NoHome)
        ));
    }

    #[test]
    fn an_unreadable_companion_is_an_io_error() {
        let env = temp_env("hermes-dir");
        let home = env.home.clone().unwrap().join(".hermes");
        fs::create_dir_all(home.join("plugins/hookah/__init__.py")).unwrap();
        let err = plan(Target::Hermes, Scope::User, &env, "hookah").unwrap_err();
        assert!(matches!(err, SetupError::Io { .. }));
    }

    #[test]
    fn openclaw_writes_the_plugin_and_its_manifest_and_repairs_either() {
        let env = temp_env("openclaw");
        let (plan, written) = apply(&env, Target::Openclaw, Scope::User).unwrap();
        assert!(written.wrote);
        assert_eq!(plan.after, hookah_harness::plugins::openclaw::PLUGIN_SOURCE);
        let manifest = plan.path.with_file_name("openclaw.plugin.json");
        assert_eq!(plan.companions.len(), 1);
        assert_eq!(plan.companions[0].path, manifest);
        assert_eq!(
            fs::read_to_string(&manifest).unwrap(),
            hookah_harness::plugins::openclaw::MANIFEST
        );
        let (again, written) = apply(&env, Target::Openclaw, Scope::User).unwrap();
        assert!(!again.changed() && !written.wrote);
        // Only the manifest is stale: it is rewritten and backed up, the entry point is not.
        fs::write(&manifest, "{}").unwrap();
        let (_, written) = apply(&env, Target::Openclaw, Scope::User).unwrap();
        assert!(written.wrote);
        assert_eq!(written.backup, None);
        assert_eq!(written.companion_backups, [backup_path(&manifest)]);
        assert_eq!(
            fs::read_to_string(&manifest).unwrap(),
            hookah_harness::plugins::openclaw::MANIFEST
        );
        assert!(!backup_path(&plan.path).exists());
    }

    #[test]
    fn user_scope_writes_under_home() {
        let env = temp_env("user");
        let (plan, _) = apply(&env, Target::Kimi, Scope::User).unwrap();
        assert!(plan.path.starts_with(env.home.as_ref().unwrap()));
        assert!(plan.after.contains("hookah hook kimi"));
    }

    #[test]
    fn the_snippet_is_the_merge_into_nothing() {
        let text = snippet(Target::Kimi, "hookah").unwrap();
        assert!(text.contains("[[hooks]]"));
        assert_eq!(
            snippet(Target::Opencode, "ignored").unwrap(),
            hookah_harness::opencode::PLUGIN_SOURCE
        );
        assert_eq!(
            snippet(Target::Pi, "ignored").unwrap(),
            hookah_harness::plugins::pi::PLUGIN_SOURCE
        );
    }

    #[test]
    fn the_pi_file_is_the_embedded_extension_and_stable() {
        let env = temp_env("pi");
        let (plan, _) = apply(&env, Target::Pi, Scope::Project).unwrap();
        assert_eq!(plan.after, hookah_harness::plugins::pi::PLUGIN_SOURCE);
        assert!(plan.path.ends_with(".pi/extensions/hookah.ts"));
        let (again, written) = apply(&env, Target::Pi, Scope::Project).unwrap();
        assert!(!again.changed());
        assert!(!written.wrote);
    }

    #[test]
    fn the_opencode_file_is_the_embedded_plugin_and_stable() {
        let env = temp_env("plugin");
        let (plan, _) = apply(&env, Target::Opencode, Scope::Project).unwrap();
        assert_eq!(plan.after, hookah_harness::opencode::PLUGIN_SOURCE);
        let (again, written) = apply(&env, Target::Opencode, Scope::Project).unwrap();
        assert!(!again.changed());
        assert!(!written.wrote);
    }

    #[test]
    fn windows_commands_are_rendered_per_harness_and_replace_the_old_form_in_place() {
        use crate::shell_program;
        let exe = r"C:\Users\a b\AppData\Local\hookah\bin\hookah.exe";
        let program = shell_program(exe);
        let new_form = "C:/Users/a b/AppData/Local/hookah/bin/hookah.exe";
        for (target, needle) in [
            (
                Target::Claude,
                r#""command": "\"C:/Users/a b/AppData/Local/hookah/bin/hookah.exe\" hook claude""#,
            ),
            (
                Target::Qoder,
                r#""command": "\"C:/Users/a b/AppData/Local/hookah/bin/hookah.exe\" hook qoder""#,
            ),
            (
                Target::Codex,
                r#"command = '"C:/Users/a b/AppData/Local/hookah/bin/hookah.exe" hook codex'"#,
            ),
            (
                Target::Kimi,
                r#"command = '"C:/Users/a b/AppData/Local/hookah/bin/hookah.exe" hook kimi'"#,
            ),
        ] {
            let out = snippet(target, &program).unwrap();
            assert!(out.contains(needle), "{target:?}: {out}");
            // The old unquoted backslash form is recognized and replaced, not duplicated.
            let old = snippet(target, exe).unwrap();
            let merged = crate::merge::merge(target, Some(&old), &program).unwrap();
            assert_eq!(merged, out, "{target:?}");
            assert!(!merged.contains("AppData\\"), "{target:?}: {merged}");
            assert!(merged.contains(new_form));
        }
    }
}
