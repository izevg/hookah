//! Where each harness keeps its config, and the environment that decides it.

use std::path::{Path, PathBuf};

use crate::error::SetupError;
use crate::target::Target;

/// Which config file to edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The current project's config.
    Project,
    /// The user's own config, for every project.
    User,
}

impl Scope {
    /// The word used on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::User => "user",
        }
    }

    /// The scope used when none is given: the project's, except where only a user config exists.
    pub fn default_for(target: Target) -> Self {
        match target {
            Target::Kimi => Self::User,
            _ => Self::Project,
        }
    }
}

/// The process environment that locates config files. Passed in so tests never touch the real
/// home directory.
#[derive(Debug, Clone, Default)]
pub struct Env {
    /// The project root: the directory `hookah setup` runs in.
    pub cwd: PathBuf,
    /// The user's home directory (`HOME`, else `USERPROFILE`).
    pub home: Option<PathBuf>,
    /// `XDG_CONFIG_HOME`, when set and not empty.
    pub xdg_config: Option<PathBuf>,
    /// The directories of `PATH`.
    pub path: Vec<PathBuf>,
}

impl Env {
    /// Reads the real process environment.
    pub fn from_process() -> Self {
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
        Self {
            cwd: std::env::current_dir().unwrap_or_default(),
            home: var("HOME")
                .or_else(|| var("USERPROFILE"))
                .map(PathBuf::from),
            xdg_config: var("XDG_CONFIG_HOME").map(PathBuf::from),
            path: var("PATH")
                .map(|p| std::env::split_paths(&p).collect())
                .unwrap_or_default(),
        }
    }

    fn home(&self) -> Result<&Path, SetupError> {
        self.home.as_deref().ok_or(SetupError::NoHome)
    }

    /// opencode's user config directory: `$XDG_CONFIG_HOME/opencode`, else `~/.config/opencode`.
    fn opencode_dir(&self) -> Result<PathBuf, SetupError> {
        match &self.xdg_config {
            Some(xdg) => Ok(xdg.join("opencode")),
            None => Ok(self.home()?.join(".config").join("opencode")),
        }
    }

    /// The harness's user-level config directory, for detection. `None` without a home.
    pub fn user_dir(&self, target: Target) -> Option<PathBuf> {
        match target {
            Target::Opencode => self.opencode_dir().ok(),
            _ => self.home().ok().map(|h| h.join(dir_name(target))),
        }
    }

    /// The config file `hookah setup <target> --scope <scope>` edits.
    pub fn config_path(&self, target: Target, scope: Scope) -> Result<PathBuf, SetupError> {
        match (target, scope) {
            (Target::Kimi, Scope::Project) => Err(SetupError::UnsupportedScope {
                target: "kimi",
                scope: "project",
                hint: "use --scope user (~/.kimi-code/config.toml)",
            }),
            (Target::Opencode, Scope::Project) => Ok(self.cwd.join(".opencode").join(PLUGIN_PATH)),
            (Target::Opencode, Scope::User) => Ok(self.opencode_dir()?.join(PLUGIN_PATH)),
            (_, Scope::Project) => Ok(self.cwd.join(dir_name(target)).join(file_name(target))),
            (_, Scope::User) => Ok(self.home()?.join(dir_name(target)).join(file_name(target))),
        }
    }
}

const PLUGIN_PATH: &str = "plugins/hookah.ts";

fn dir_name(target: Target) -> &'static str {
    match target {
        Target::Claude => ".claude",
        Target::Codex => ".codex",
        Target::Qoder => ".qoder",
        Target::Kimi => ".kimi-code",
        Target::Opencode => ".opencode",
    }
}

fn file_name(target: Target) -> &'static str {
    match target {
        Target::Claude | Target::Qoder => "settings.json",
        Target::Codex | Target::Kimi => "config.toml",
        Target::Opencode => "plugins/hookah.ts",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn env() -> Env {
        Env {
            cwd: "/p".into(),
            home: Some("/h".into()),
            xdg_config: None,
            path: vec![],
        }
    }

    fn path(target: Target, scope: Scope) -> PathBuf {
        env().config_path(target, scope).unwrap()
    }

    #[test]
    fn project_paths_follow_the_setup_docs() {
        let p = |t| path(t, Scope::Project);
        assert_eq!(p(Target::Claude), Path::new("/p/.claude/settings.json"));
        assert_eq!(p(Target::Codex), Path::new("/p/.codex/config.toml"));
        assert_eq!(p(Target::Qoder), Path::new("/p/.qoder/settings.json"));
        assert_eq!(
            p(Target::Opencode),
            Path::new("/p/.opencode/plugins/hookah.ts")
        );
    }

    #[test]
    fn user_paths_follow_the_setup_docs() {
        let u = |t| path(t, Scope::User);
        assert_eq!(u(Target::Claude), Path::new("/h/.claude/settings.json"));
        assert_eq!(u(Target::Codex), Path::new("/h/.codex/config.toml"));
        assert_eq!(u(Target::Qoder), Path::new("/h/.qoder/settings.json"));
        assert_eq!(u(Target::Kimi), Path::new("/h/.kimi-code/config.toml"));
        assert_eq!(
            u(Target::Opencode),
            Path::new("/h/.config/opencode/plugins/hookah.ts")
        );
    }

    #[test]
    fn opencode_honors_xdg_config_home() {
        let mut e = env();
        e.xdg_config = Some("/x".into());
        assert_eq!(
            e.config_path(Target::Opencode, Scope::User).unwrap(),
            Path::new("/x/opencode/plugins/hookah.ts")
        );
        assert_eq!(e.user_dir(Target::Opencode), Some("/x/opencode".into()));
    }

    #[test]
    fn kimi_has_no_project_config_and_defaults_to_user() {
        let err = env().config_path(Target::Kimi, Scope::Project).unwrap_err();
        assert!(err.to_string().contains("--scope user"));
        assert_eq!(Scope::default_for(Target::Kimi), Scope::User);
        assert_eq!(Scope::default_for(Target::Claude), Scope::Project);
        assert_eq!(Scope::Project.name(), "project");
        assert_eq!(Scope::User.name(), "user");
    }

    #[test]
    fn user_scope_needs_a_home_but_project_scope_does_not() {
        let mut e = env();
        e.home = None;
        assert!(matches!(
            e.config_path(Target::Claude, Scope::User),
            Err(SetupError::NoHome)
        ));
        assert!(matches!(
            e.config_path(Target::Opencode, Scope::User),
            Err(SetupError::NoHome)
        ));
        assert!(e.config_path(Target::Claude, Scope::Project).is_ok());
        assert_eq!(e.user_dir(Target::Claude), None);
    }

    #[test]
    fn user_dirs_are_the_parents_of_the_user_configs() {
        assert_eq!(env().user_dir(Target::Kimi), Some("/h/.kimi-code".into()));
        assert_eq!(
            env().user_dir(Target::Opencode),
            Some("/h/.config/opencode".into())
        );
    }
}
