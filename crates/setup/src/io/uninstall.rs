//! `hookah uninstall`: working out what setup wrote and taking exactly that out again.
//!
//! The inverse of [`plan`](super::plan) and [`commit`](super::commit), with the same safety rules:
//! only hookah's own entries leave a config file (see [`unmerge`]), a file hookah wrote leaves
//! only while it still has the content hookah wrote, and a file that cannot be handled safely is
//! reported and left alone. The `.hookah.bak` that setup made is never restored: the user may have
//! changed the file since, so the backup stays where it is.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::env::{Env, Scope};
use super::{backup_path, read, sibling};
use crate::error::SetupError;
use crate::merge::{Unmerged, companions, plugin_text, unmerge};
use crate::target::Target;

/// One thing uninstall does to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Write the file again without hookah's entries.
    Rewrite {
        /// The config file.
        path: PathBuf,
        /// The file as it should be.
        text: String,
    },
    /// Delete a file hookah wrote.
    Delete {
        /// The file.
        path: PathBuf,
    },
    /// Leave the file as it is, and say why.
    Keep {
        /// The file.
        path: PathBuf,
        /// Why it cannot be removed safely, and what to do instead.
        reason: String,
    },
}

impl Step {
    /// The file the step is about.
    pub fn path(&self) -> &Path {
        match self {
            Self::Rewrite { path, .. } | Self::Delete { path } | Self::Keep { path, .. } => path,
        }
    }
}

/// What `hookah uninstall` would do for one harness at one scope. Empty when nothing is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    /// The harness name as typed on the command line.
    pub name: &'static str,
    /// The scope; `None` for the Orca plugin, which has none.
    pub scope: Option<Scope>,
    /// The harness, when it is one `hookah setup` knows.
    pub target: Option<Target>,
    /// The files to change, in order.
    pub steps: Vec<Step>,
    /// Backups from setup that exist next to a file of the steps. They stay.
    pub backups: Vec<PathBuf>,
    /// The directory that is removed too once it is empty (the harness's own, or the plugin's).
    pub root: Option<PathBuf>,
}

impl Removal {
    /// True if applying the removal changes a file.
    pub fn changed(&self) -> bool {
        self.steps.iter().any(|s| !matches!(s, Step::Keep { .. }))
    }

    /// True if some file had to be left in place.
    pub fn refused(&self) -> bool {
        self.steps.iter().any(|s| matches!(s, Step::Keep { .. }))
    }

    /// A removal of `steps`; the backups next to their files are looked up.
    pub fn new(
        name: &'static str,
        scope: Option<Scope>,
        target: Option<Target>,
        steps: Vec<Step>,
        root: Option<PathBuf>,
    ) -> Self {
        let backups = steps
            .iter()
            .map(|s| backup_path(s.path()))
            .filter(|b| b.is_file())
            .collect();
        Self {
            name,
            scope,
            target,
            steps,
            backups,
            root,
        }
    }
}

/// Works out the removal for `target` at `scope`. Reads files and changes nothing.
///
/// With `force`, a plugin file is deleted even when its content is not what hookah writes.
pub fn plan_removal(
    target: Target,
    scope: Scope,
    env: &Env,
    force: bool,
) -> Result<Removal, SetupError> {
    let path = env.config_path(target, scope)?;
    let mut files = Vec::new();
    for (relative, text) in companions(target) {
        let dir = env.user_dir(target).ok_or(SetupError::NoHome)?;
        files.push((dir.join(relative), *text));
    }
    let mut steps = Vec::new();
    if let Some(text) = plugin_text(target) {
        files.insert(0, (path, text));
    } else if let Some(step) = config_step(target, &path)? {
        // A config that cannot be edited leaves the plugin enabled, so its files stay too.
        if matches!(step, Step::Keep { .. }) {
            files.clear();
        }
        steps.push(step);
    }
    steps.extend(plugin_steps(&files, force));
    Ok(Removal::new(
        target.name(),
        Some(scope),
        Some(target),
        steps,
        env.harness_root(target, scope),
    ))
}

/// What to do with the merged config file, `None` when it holds nothing of hookah's.
fn config_step(target: Target, path: &Path) -> Result<Option<Step>, SetupError> {
    let Some(text) = read(path)? else {
        return Ok(None);
    };
    Ok(match unmerge(target, &text) {
        Err(reason) => Some(Step::Keep {
            path: path.to_path_buf(),
            reason: format!("{reason}; remove hookah's entries by hand"),
        }),
        Ok(Unmerged::Unchanged) => None,
        // An empty file that setup created is deleted. When setup backed up an original, the
        // file existed before, so it stays, empty.
        Ok(Unmerged::Removed { empty: true, .. }) if !backup_path(path).exists() => {
            Some(Step::Delete {
                path: path.to_path_buf(),
            })
        }
        Ok(Unmerged::Removed { text, .. }) => Some(Step::Rewrite {
            path: path.to_path_buf(),
            text,
        }),
    })
}

/// The steps for files that hookah wrote whole: delete each that still has the content `want`.
pub(crate) fn plugin_steps(files: &[(PathBuf, &str)], force: bool) -> Vec<Step> {
    let mut steps = Vec::new();
    for (path, want) in files {
        let keep = |reason: String| Step::Keep {
            path: path.clone(),
            reason,
        };
        match fs::read_to_string(path) {
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => steps.push(keep(format!("cannot read it ({e})"))),
            Ok(text) if force || same(&text, want) => {
                steps.push(Step::Delete { path: path.clone() })
            }
            Ok(_) => steps.push(keep(
                "it differs from the plugin this hookah installs (you edited it, or another \
                 hookah version wrote it); delete it by hand, or run uninstall again with --force"
                    .to_owned(),
            )),
        }
    }
    steps
}

/// Equal up to line endings: git may have rewritten them on Windows.
fn same(text: &str, want: &str) -> bool {
    text.replace("\r\n", "\n") == want.replace("\r\n", "\n")
}

/// Applies a removal. Directories left empty are removed up to `root`.
pub fn remove(removal: &Removal) -> Result<(), SetupError> {
    for step in &removal.steps {
        match step {
            Step::Rewrite { path, text } => {
                let tmp = sibling(path, ".hookah.tmp");
                let io = |path: &Path| {
                    let path = path.to_path_buf();
                    move |source| SetupError::Io { path, source }
                };
                fs::write(&tmp, text).map_err(io(&tmp))?;
                fs::rename(&tmp, path).map_err(io(path))?;
            }
            Step::Delete { path } => {
                match fs::remove_file(path) {
                    Err(e) if e.kind() != ErrorKind::NotFound => {
                        return Err(SetupError::Io {
                            path: path.clone(),
                            source: e,
                        });
                    }
                    _ => {}
                }
                if let Some(root) = &removal.root {
                    prune(path, root);
                }
            }
            Step::Keep { .. } => {}
        }
    }
    Ok(())
}

/// Removes the empty directories above `file`, up to and including `root`.
fn prune(file: &Path, root: &Path) {
    let mut dir = file.parent();
    while let Some(current) = dir {
        if !current.starts_with(root) || fs::remove_dir(current).is_err() || current == root {
            return;
        }
        dir = current.parent();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::{commit, plan};

    fn temp_env(name: &str) -> Env {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "hookah-uninstall-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Env {
            cwd: root.join("project"),
            home: Some(root.join("home")),
            ..Env::default()
        }
    }

    fn setup(env: &Env, target: Target, scope: Scope) {
        commit(&plan(target, scope, env, "hookah").unwrap()).unwrap();
    }

    fn undo(env: &Env, target: Target, scope: Scope) -> Removal {
        let removal = plan_removal(target, scope, env, false).unwrap();
        remove(&removal).unwrap();
        removal
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn nothing_installed_is_an_empty_removal_for_every_harness() {
        let env = temp_env("none");
        for target in Target::ALL {
            let scope = Scope::default_for(target);
            let removal = plan_removal(target, scope, &env, false).unwrap();
            assert!(removal.steps.is_empty(), "{target:?}");
            assert!(!removal.changed() && !removal.refused());
            assert!(removal.backups.is_empty());
        }
    }

    #[test]
    fn a_file_setup_created_is_deleted_with_its_directory_and_a_second_run_is_a_no_op() {
        let env = temp_env("created");
        setup(&env, Target::Claude, Scope::Project);
        let file = env.cwd.join(".claude/settings.json");
        assert!(file.is_file());
        let first = undo(&env, Target::Claude, Scope::Project);
        assert_eq!(first.steps, [Step::Delete { path: file }]);
        assert!(!env.cwd.join(".claude").exists());
        assert!(env.cwd.exists());
        let second = plan_removal(Target::Claude, Scope::Project, &env, false).unwrap();
        assert!(second.steps.is_empty());
    }

    #[test]
    fn a_preexisting_config_is_edited_and_its_backup_is_kept_untouched() {
        let env = temp_env("edit");
        let file = env.cwd.join(".claude/settings.json");
        let original = "{\n  \"model\": \"x\"\n}\n";
        write(&file, original);
        setup(&env, Target::Claude, Scope::Project);
        assert_ne!(fs::read_to_string(&file).unwrap(), original);
        let removal = undo(&env, Target::Claude, Scope::Project);
        assert_eq!(fs::read_to_string(&file).unwrap(), original);
        assert_eq!(removal.backups, [backup_path(&file)]);
        assert_eq!(fs::read_to_string(backup_path(&file)).unwrap(), original);
    }

    #[test]
    fn an_original_that_held_nothing_else_stays_as_an_empty_file_because_a_backup_exists() {
        let env = temp_env("emptyorig");
        let file = env.cwd.join(".qoder/settings.json");
        write(&file, "{}\n");
        setup(&env, Target::Qoder, Scope::Project);
        let removal = undo(&env, Target::Qoder, Scope::Project);
        assert!(matches!(removal.steps[0], Step::Rewrite { .. }));
        assert_eq!(fs::read_to_string(&file).unwrap(), "{}\n");
    }

    #[test]
    fn a_config_that_does_not_parse_is_kept_with_the_reason() {
        let env = temp_env("bad");
        let file = env.cwd.join(".codex/config.toml");
        write(&file, "model = [");
        let removal = plan_removal(Target::Codex, Scope::Project, &env, false).unwrap();
        let [Step::Keep { path, reason }] = removal.steps.as_slice() else {
            panic!("{removal:?}");
        };
        assert_eq!(path, &file);
        assert!(reason.contains("not valid TOML"), "{reason}");
        assert!(reason.contains("by hand"), "{reason}");
        assert!(removal.refused() && !removal.changed());
        remove(&removal).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "model = [");
    }

    #[test]
    fn a_directory_in_place_of_the_config_is_an_io_error() {
        let env = temp_env("dirconfig");
        fs::create_dir_all(env.cwd.join(".claude/settings.json")).unwrap();
        let err = plan_removal(Target::Claude, Scope::Project, &env, false).unwrap_err();
        assert!(matches!(err, SetupError::Io { .. }));
    }

    #[test]
    fn a_plugin_that_matches_is_deleted_and_the_empty_directories_go_up_to_the_root() {
        let env = temp_env("pi");
        setup(&env, Target::Pi, Scope::Project);
        let file = env.cwd.join(".pi/extensions/hookah.ts");
        assert!(file.is_file());
        let removal = undo(&env, Target::Pi, Scope::Project);
        assert_eq!(removal.steps, [Step::Delete { path: file }]);
        assert!(!env.cwd.join(".pi").exists());
        assert!(env.cwd.exists());
    }

    #[test]
    fn pruning_stops_at_a_directory_that_still_holds_something() {
        let env = temp_env("keepdir");
        setup(&env, Target::Opencode, Scope::Project);
        write(&env.cwd.join(".opencode/plugins/mine.ts"), "mine");
        undo(&env, Target::Opencode, Scope::Project);
        assert!(env.cwd.join(".opencode/plugins/mine.ts").is_file());
        assert!(!env.cwd.join(".opencode/plugins/hookah.ts").exists());
        // A file next to the plugin directory keeps the harness directory too.
        let env = temp_env("keepdir2");
        setup(&env, Target::Opencode, Scope::Project);
        write(&env.cwd.join(".opencode/opencode.json"), "{}");
        undo(&env, Target::Opencode, Scope::Project);
        assert!(!env.cwd.join(".opencode/plugins").exists());
        assert!(env.cwd.join(".opencode/opencode.json").is_file());
    }

    #[test]
    fn an_edited_plugin_is_refused_unless_forced() {
        let env = temp_env("edited");
        setup(&env, Target::Opencode, Scope::Project);
        let file = env.cwd.join(".opencode/plugins/hookah.ts");
        write(&file, "// my own plugin\n");
        let removal = undo(&env, Target::Opencode, Scope::Project);
        let [Step::Keep { reason, .. }] = removal.steps.as_slice() else {
            panic!("{removal:?}");
        };
        assert!(
            reason.contains("differs") && reason.contains("--force"),
            "{reason}"
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), "// my own plugin\n");
        let forced = plan_removal(Target::Opencode, Scope::Project, &env, true).unwrap();
        remove(&forced).unwrap();
        assert!(!file.exists());
    }

    #[test]
    fn crlf_line_endings_still_count_as_the_same_plugin() {
        let env = temp_env("crlf");
        setup(&env, Target::Pi, Scope::Project);
        let file = env.cwd.join(".pi/extensions/hookah.ts");
        let text = fs::read_to_string(&file).unwrap().replace('\n', "\r\n");
        fs::write(&file, text).unwrap();
        let removal = undo(&env, Target::Pi, Scope::Project);
        assert!(matches!(removal.steps[0], Step::Delete { .. }));
        assert!(!file.exists());
    }

    #[test]
    fn an_unreadable_plugin_file_is_kept_with_the_error() {
        let env = temp_env("unreadable");
        fs::create_dir_all(env.cwd.join(".pi/extensions/hookah.ts")).unwrap();
        let removal = plan_removal(Target::Pi, Scope::Project, &env, false).unwrap();
        let [Step::Keep { reason, .. }] = removal.steps.as_slice() else {
            panic!("{removal:?}");
        };
        assert!(reason.starts_with("cannot read it"), "{reason}");
    }

    #[test]
    fn hermes_loses_its_enabled_entry_and_its_plugin_files() {
        let env = temp_env("hermes");
        let config = env.home.clone().unwrap().join(".hermes/config.yaml");
        let original = "# mine\nmodel: x\nplugins:\n  enabled:\n    - kanban  # keep\n";
        write(&config, original);
        setup(&env, Target::Hermes, Scope::User);
        let removal = undo(&env, Target::Hermes, Scope::User);
        assert_eq!(removal.steps.len(), 3);
        assert_eq!(fs::read_to_string(&config).unwrap(), original);
        let plugin = config.with_file_name("plugins");
        assert!(!plugin.exists());
        assert!(backup_path(&config).is_file());
        assert!(
            plan_removal(Target::Hermes, Scope::User, &env, false)
                .unwrap()
                .steps
                .is_empty()
        );
    }

    #[test]
    fn hermes_created_from_nothing_leaves_no_trace() {
        let env = temp_env("hermes-nothing");
        setup(&env, Target::Hermes, Scope::User);
        undo(&env, Target::Hermes, Scope::User);
        assert!(!env.home.clone().unwrap().join(".hermes").exists());
    }

    #[test]
    fn hermes_keeps_the_plugin_when_the_config_cannot_be_edited() {
        let env = temp_env("hermes-inline");
        let config = env.home.clone().unwrap().join(".hermes/config.yaml");
        write(&config, "plugins:\n  enabled: [a, hookah]\n");
        write(&config.with_file_name("plugins/hookah/__init__.py"), "x");
        let removal = plan_removal(Target::Hermes, Scope::User, &env, false).unwrap();
        assert_eq!(removal.steps.len(), 1);
        assert!(
            matches!(&removal.steps[0], Step::Keep { path, reason } if path == &config && reason.contains("inline"))
        );
    }

    #[test]
    fn openclaw_removes_the_entry_point_and_the_manifest_and_their_directory() {
        let env = temp_env("openclaw");
        setup(&env, Target::Openclaw, Scope::User);
        let dir = env
            .home
            .clone()
            .unwrap()
            .join(".openclaw/extensions/hookah");
        assert!(dir.join("index.js").is_file() && dir.join("openclaw.plugin.json").is_file());
        let removal = undo(&env, Target::Openclaw, Scope::User);
        assert_eq!(removal.steps.len(), 2);
        assert!(!env.home.clone().unwrap().join(".openclaw").exists());
    }

    #[test]
    fn a_stale_companion_with_a_backup_reports_the_backup() {
        let env = temp_env("stale");
        setup(&env, Target::Openclaw, Scope::User);
        let dir = env
            .home
            .clone()
            .unwrap()
            .join(".openclaw/extensions/hookah");
        fs::write(dir.join("openclaw.plugin.json"), "{}").unwrap();
        setup(&env, Target::Openclaw, Scope::User);
        let removal = undo(&env, Target::Openclaw, Scope::User);
        assert_eq!(
            removal.backups,
            [backup_path(&dir.join("openclaw.plugin.json"))]
        );
        assert!(dir.join("openclaw.plugin.json.hookah.bak").is_file());
        assert!(dir.is_dir());
    }

    #[test]
    fn user_scope_without_a_home_is_an_error() {
        let env = Env::default();
        assert!(matches!(
            plan_removal(Target::Claude, Scope::User, &env, false),
            Err(SetupError::NoHome)
        ));
        let mut env = temp_env("nohome");
        env.home = None;
        assert!(matches!(
            plan_removal(Target::Hermes, Scope::User, &env, false),
            Err(SetupError::NoHome)
        ));
    }

    #[test]
    fn project_scope_of_a_user_only_harness_is_unsupported() {
        let env = temp_env("unsupported");
        assert!(matches!(
            plan_removal(Target::Kimi, Scope::Project, &env, false),
            Err(SetupError::UnsupportedScope { .. })
        ));
    }

    #[test]
    fn deleting_a_file_that_vanished_in_between_is_fine() {
        let removal = Removal::new(
            "x",
            None,
            None,
            vec![Step::Delete {
                path: std::env::temp_dir().join("hookah-uninstall-never-existed"),
            }],
            None,
        );
        remove(&removal).unwrap();
    }

    #[test]
    fn deleting_a_directory_as_a_file_is_an_io_error() {
        let env = temp_env("deldir");
        let dir = env.cwd.join("a-dir");
        fs::create_dir_all(dir.join("inner")).unwrap();
        let removal = Removal::new("x", None, None, vec![Step::Delete { path: dir }], None);
        assert!(matches!(remove(&removal), Err(SetupError::Io { .. })));
    }

    #[test]
    fn pruning_never_leaves_the_root() {
        let env = temp_env("prune");
        let root = env.cwd.join("root");
        fs::create_dir_all(root.join("a/b")).unwrap();
        write(&root.join("a/b/f"), "x");
        let removal = Removal::new(
            "x",
            None,
            None,
            vec![Step::Delete {
                path: root.join("a/b/f"),
            }],
            Some(root.join("a")),
        );
        remove(&removal).unwrap();
        // `a` is the root of the removal: it goes, its parent stays.
        assert!(!root.join("a").exists());
        assert!(root.exists());
    }
}
