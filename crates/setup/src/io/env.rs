//! Where each harness keeps its config, and the environment that decides it.

use std::ffi::OsString;
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
            Target::Kimi | Target::Hermes | Target::Openclaw => Self::User,
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
    /// `PI_CODING_AGENT_DIR` (Pi's config directory), when set and not empty.
    pub pi_agent_dir: Option<PathBuf>,
    /// OpenClaw's state directory when the environment moves it: `OPENCLAW_STATE_DIR`, else
    /// `OPENCLAW_HOME/.openclaw`. `None` means `~/.openclaw`.
    pub openclaw_state: Option<PathBuf>,
    /// The directories of `PATH`.
    pub path: Vec<PathBuf>,
    /// Hermes's home directory when the environment names it: `HERMES_HOME`, else on Windows
    /// `%LOCALAPPDATA%\hermes`. `None` means `~/.hermes`.
    pub hermes_home: Option<PathBuf>,
}

impl Env {
    /// Every environment variable that decides where a config file goes. A test sandbox must
    /// override all of them, or `hookah setup` writes into the real home; [`Env::from_process`]
    /// reads no other variable (it asserts so in debug builds).
    pub const VARS: &'static [&'static str] = &[
        "HOME",
        "USERPROFILE",
        "XDG_CONFIG_HOME",
        "PATH",
        "HERMES_HOME",
        "LOCALAPPDATA",
        "PI_CODING_AGENT_DIR",
        "OPENCLAW_STATE_DIR",
        "OPENCLAW_HOME",
    ];

    /// Reads the real process environment.
    pub fn from_process() -> Self {
        let var = |name: &str| {
            debug_assert!(
                Self::VARS.contains(&name),
                "{name} is missing from Env::VARS"
            );
            std::env::var_os(name).filter(|v| !v.is_empty())
        };
        Self {
            cwd: std::env::current_dir().unwrap_or_default(),
            home: var("HOME")
                .or_else(|| var("USERPROFILE"))
                .map(PathBuf::from),
            xdg_config: var("XDG_CONFIG_HOME").map(PathBuf::from),
            pi_agent_dir: var("PI_CODING_AGENT_DIR").map(PathBuf::from),
            openclaw_state: var("OPENCLAW_STATE_DIR")
                .map(PathBuf::from)
                .or_else(|| var("OPENCLAW_HOME").map(|h| PathBuf::from(h).join(".openclaw"))),
            path: var("PATH")
                .map(|p| std::env::split_paths(&p).collect())
                .unwrap_or_default(),
            hermes_home: hermes_home_from(var("HERMES_HOME"), var("LOCALAPPDATA"), cfg!(windows)),
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

    /// Hermes's home directory: [`Env::hermes_home`], else `~/.hermes`.
    fn hermes_dir(&self) -> Result<PathBuf, SetupError> {
        match &self.hermes_home {
            Some(dir) => Ok(dir.clone()),
            None => Ok(self.home()?.join(".hermes")),
        }
    }

    /// OpenClaw's state directory: [`Env::openclaw_state`], else `~/.openclaw`.
    fn openclaw_dir(&self) -> Result<PathBuf, SetupError> {
        match &self.openclaw_state {
            Some(dir) => Ok(dir.clone()),
            None => Ok(self.home()?.join(".openclaw")),
        }
    }

    /// Pi's user config directory: `$PI_CODING_AGENT_DIR`, else `~/.pi/agent`.
    fn pi_dir(&self) -> Result<PathBuf, SetupError> {
        match &self.pi_agent_dir {
            Some(dir) => Ok(dir.clone()),
            None => Ok(self.home()?.join(".pi").join("agent")),
        }
    }

    /// The harness's user-level config directory, for detection. `None` without a home.
    pub fn user_dir(&self, target: Target) -> Option<PathBuf> {
        match target {
            Target::Opencode => self.opencode_dir().ok(),
            Target::Hermes => self.hermes_dir().ok(),
            Target::Pi => self.pi_dir().ok(),
            Target::Openclaw => self.openclaw_dir().ok(),
            Target::Zcode => self.home().ok().map(|h| h.join(".zcode/cli")),
            _ => self.home().ok().map(|h| h.join(dir_name(target))),
        }
    }

    /// The directory that holds everything setup writes for `target` at `scope`: `<project>/.claude`,
    /// `~/.codex`, `~/.config/opencode`. Uninstall removes it when it ends up empty.
    pub fn harness_root(&self, target: Target, scope: Scope) -> Option<PathBuf> {
        match scope {
            Scope::Project => Some(self.cwd.join(dir_name(target))),
            Scope::User => self.user_dir(target),
        }
    }

    /// The two project config locations supported by ZCode, in setup preference order.
    pub(crate) fn zcode_project_configs(&self) -> [PathBuf; 2] {
        [
            self.cwd.join(".zcode/config.json"),
            self.cwd.join("zcode.json"),
        ]
    }

    /// The config file `hookah setup <target> --scope <scope>` edits.
    pub fn config_path(&self, target: Target, scope: Scope) -> Result<PathBuf, SetupError> {
        match (target, scope) {
            (Target::Kimi, Scope::Project) => Err(SetupError::UnsupportedScope {
                target: "kimi",
                scope: "project",
                hint: "use --scope user (~/.kimi-code/config.toml)",
            }),
            (Target::Hermes, Scope::Project) => Err(SetupError::UnsupportedScope {
                target: "hermes",
                scope: "project",
                hint: "Hermes loads plugins from its home directory only; use --scope user (~/.hermes)",
            }),
            (Target::Openclaw, Scope::Project) => Err(SetupError::UnsupportedScope {
                target: "openclaw",
                scope: "project",
                hint: "OpenClaw loads project plugins only from its agent workspace; use --scope user (~/.openclaw)",
            }),
            (Target::Zcode, Scope::User) => Ok(self.home()?.join(".zcode/cli/config.json")),
            (Target::Openclaw, Scope::User) => Ok(self.openclaw_dir()?.join(OPENCLAW_PLUGIN_PATH)),
            (Target::Hermes, Scope::User) => Ok(self.hermes_dir()?.join("config.yaml")),
            (Target::Opencode, Scope::Project) => Ok(self.cwd.join(".opencode").join(PLUGIN_PATH)),
            (Target::Opencode, Scope::User) => Ok(self.opencode_dir()?.join(PLUGIN_PATH)),
            (Target::Pi, Scope::Project) => Ok(self.cwd.join(".pi").join(PI_EXTENSION_PATH)),
            (Target::Pi, Scope::User) => Ok(self.pi_dir()?.join(PI_EXTENSION_PATH)),
            (_, Scope::Project) => Ok(self.cwd.join(dir_name(target)).join(file_name(target))),
            (_, Scope::User) => Ok(self.home()?.join(dir_name(target)).join(file_name(target))),
        }
    }
}

/// Hermes's home directory from the environment: `HERMES_HOME`, else `%LOCALAPPDATA%\hermes` on
/// Windows, else `None` (`~/.hermes`).
fn hermes_home_from(
    home: Option<OsString>,
    local_appdata: Option<OsString>,
    windows: bool,
) -> Option<PathBuf> {
    home.map(PathBuf::from).or_else(|| {
        local_appdata
            .filter(|_| windows)
            .map(|dir| PathBuf::from(dir).join("hermes"))
    })
}

const PLUGIN_PATH: &str = "plugins/hookah.ts";
const PI_EXTENSION_PATH: &str = "extensions/hookah.ts";
/// OpenClaw loads plugin directories from `extensions/`; its manifest sits next to this file.
const OPENCLAW_PLUGIN_PATH: &str = "extensions/hookah/index.js";

fn dir_name(target: Target) -> &'static str {
    match target {
        Target::Claude => ".claude",
        Target::Codex => ".codex",
        Target::Qoder => ".qoder",
        Target::Zcode => ".zcode",
        Target::Kimi => ".kimi-code",
        Target::Cursor => ".cursor",
        Target::Opencode => ".opencode",
        Target::Hermes => ".hermes",
        Target::Pi => ".pi",
        Target::Openclaw => ".openclaw",
    }
}

fn file_name(target: Target) -> &'static str {
    match target {
        Target::Claude | Target::Qoder => "settings.json",
        Target::Codex | Target::Kimi => "config.toml",
        Target::Zcode => "config.json",
        Target::Cursor => "hooks.json",
        Target::Opencode => "plugins/hookah.ts",
        Target::Hermes => "config.yaml",
        Target::Pi => PI_EXTENSION_PATH,
        Target::Openclaw => OPENCLAW_PLUGIN_PATH,
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
            pi_agent_dir: None,
            openclaw_state: None,
            path: vec![],
            hermes_home: None,
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
        assert_eq!(p(Target::Cursor), Path::new("/p/.cursor/hooks.json"));
        assert_eq!(
            p(Target::Opencode),
            Path::new("/p/.opencode/plugins/hookah.ts")
        );
        assert_eq!(p(Target::Pi), Path::new("/p/.pi/extensions/hookah.ts"));
    }

    #[test]
    fn user_paths_follow_the_setup_docs() {
        let u = |t| path(t, Scope::User);
        assert_eq!(u(Target::Claude), Path::new("/h/.claude/settings.json"));
        assert_eq!(u(Target::Codex), Path::new("/h/.codex/config.toml"));
        assert_eq!(u(Target::Qoder), Path::new("/h/.qoder/settings.json"));
        assert_eq!(u(Target::Kimi), Path::new("/h/.kimi-code/config.toml"));
        assert_eq!(u(Target::Cursor), Path::new("/h/.cursor/hooks.json"));
        assert_eq!(env().user_dir(Target::Cursor), Some("/h/.cursor".into()));
        assert_eq!(Scope::default_for(Target::Cursor), Scope::Project);
        assert_eq!(
            u(Target::Opencode),
            Path::new("/h/.config/opencode/plugins/hookah.ts")
        );
        assert_eq!(
            u(Target::Pi),
            Path::new("/h/.pi/agent/extensions/hookah.ts")
        );
    }

    #[test]
    fn pi_honors_pi_coding_agent_dir() {
        let mut e = env();
        e.pi_agent_dir = Some("/cfg".into());
        assert_eq!(
            e.config_path(Target::Pi, Scope::User).unwrap(),
            Path::new("/cfg/extensions/hookah.ts")
        );
        assert_eq!(e.user_dir(Target::Pi), Some("/cfg".into()));
        assert_eq!(env().user_dir(Target::Pi), Some("/h/.pi/agent".into()));
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
    fn openclaw_is_user_level_and_follows_its_state_directory() {
        assert_eq!(
            path(Target::Openclaw, Scope::User),
            Path::new("/h/.openclaw/extensions/hookah/index.js")
        );
        assert_eq!(
            env().user_dir(Target::Openclaw),
            Some("/h/.openclaw".into())
        );
        let err = env()
            .config_path(Target::Openclaw, Scope::Project)
            .unwrap_err();
        assert!(err.to_string().contains("--scope user"));
        assert_eq!(Scope::default_for(Target::Openclaw), Scope::User);
        let mut e = env();
        e.openclaw_state = Some("/s".into());
        assert_eq!(
            e.config_path(Target::Openclaw, Scope::User).unwrap(),
            Path::new("/s/extensions/hookah/index.js")
        );
        assert_eq!(e.user_dir(Target::Openclaw), Some("/s".into()));
        e.home = None;
        assert!(e.config_path(Target::Openclaw, Scope::User).is_ok());
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
    fn hermes_is_user_level_and_honors_its_home_variable() {
        assert_eq!(
            path(Target::Hermes, Scope::User),
            Path::new("/h/.hermes/config.yaml")
        );
        assert_eq!(env().user_dir(Target::Hermes), Some("/h/.hermes".into()));
        let err = env()
            .config_path(Target::Hermes, Scope::Project)
            .unwrap_err();
        assert!(err.to_string().contains("--scope user"));
        assert_eq!(Scope::default_for(Target::Hermes), Scope::User);
        let mut e = env();
        e.hermes_home = Some("/x/hermes".into());
        assert_eq!(
            e.config_path(Target::Hermes, Scope::User).unwrap(),
            Path::new("/x/hermes/config.yaml")
        );
        e.home = None;
        assert_eq!(e.user_dir(Target::Hermes), Some("/x/hermes".into()));
        e.hermes_home = None;
        assert_eq!(e.user_dir(Target::Hermes), None);
    }

    #[test]
    fn hermes_home_comes_from_its_variable_then_local_app_data_on_windows() {
        let var = |v: &str| Some(OsString::from(v));
        assert_eq!(
            hermes_home_from(var("/x"), var("/l"), true),
            Some("/x".into())
        );
        assert_eq!(
            hermes_home_from(None, var("/l"), true),
            Some(PathBuf::from("/l").join("hermes"))
        );
        assert_eq!(hermes_home_from(None, var("/l"), false), None);
        assert_eq!(hermes_home_from(None, None, true), None);
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
        assert!(matches!(
            e.config_path(Target::Pi, Scope::User),
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

    #[test]
    fn the_process_environment_is_read() {
        let env = Env::from_process();
        assert_eq!(env.cwd, std::env::current_dir().unwrap());
        assert!(!env.path.is_empty());
        let has_home = ["HOME", "USERPROFILE"]
            .iter()
            .any(|v| std::env::var_os(v).is_some_and(|h| !h.is_empty()));
        assert_eq!(env.home.is_some(), has_home);
    }
}
