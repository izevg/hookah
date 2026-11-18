//! Reading, backing up, and writing the config files. See [`env`] for where they live and
//! [`detect`] for the read-only survey.

pub mod detect;
pub mod env;

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::error::SetupError;
use crate::merge::merge;
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
}

impl Plan {
    /// True if applying the plan changes the file.
    pub fn changed(&self) -> bool {
        self.before.as_deref() != Some(self.after.as_str())
    }
}

/// What [`commit`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// False when the file was already up to date and nothing was written.
    pub wrote: bool,
    /// The backup of the original, when this run created one.
    pub backup: Option<PathBuf>,
}

/// Reads the config file and works out its new content. Writes nothing.
///
/// `program` starts hookah in each hook command (see [`merge`]).
pub fn plan(target: Target, scope: Scope, env: &Env, program: &str) -> Result<Plan, SetupError> {
    let path = env.config_path(target, scope)?;
    let before = match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(source) => return Err(SetupError::Io { path, source }),
    };
    let after =
        merge(target, before.as_deref(), program).map_err(|reason| SetupError::Invalid {
            path: path.clone(),
            reason,
        })?;
    Ok(Plan {
        target,
        scope,
        path,
        before,
        after,
    })
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
        });
    }
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| SetupError::Io { path, source }
    };
    if let Some(parent) = plan.path.parent() {
        fs::create_dir_all(parent).map_err(io(parent))?;
    }
    let backup = backup_path(&plan.path);
    let mut made = None;
    if plan.before.is_some() && !backup.exists() {
        fs::copy(&plan.path, &backup).map_err(io(&backup))?;
        made = Some(backup);
    }
    let tmp = sibling(&plan.path, ".hookah.tmp");
    fs::write(&tmp, &plan.after).map_err(io(&tmp))?;
    fs::rename(&tmp, &plan.path).map_err(io(&plan.path))?;
    Ok(Written {
        wrote: true,
        backup: made,
    })
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
            path: vec![],
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
                backup: None
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
}
