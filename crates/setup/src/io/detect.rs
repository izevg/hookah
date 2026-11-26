//! `hookah setup --detect`: which harnesses are here and which are already configured. Read-only.

use std::fs;
use std::path::{Path, PathBuf};

use super::env::{Env, Scope};
use crate::merge::{yaml, zcode};
use crate::target::{Target, mentions_hookah};

/// What is known about one harness on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    /// The harness.
    pub target: Target,
    /// Where its executable was found on `PATH`.
    pub binary: Option<PathBuf>,
    /// True if its user-level config directory exists.
    pub config_dir: bool,
    /// Whether this project's config has hookah's hooks; `None` if the harness has no project config.
    pub project: Option<bool>,
    /// Whether the user-level config has hookah's hooks.
    pub user: bool,
}

impl Detection {
    /// True if the harness looks installed.
    pub fn installed(&self) -> bool {
        self.binary.is_some() || self.config_dir
    }
}

/// Surveys every harness.
pub fn detect(env: &Env) -> Vec<Detection> {
    Target::ALL
        .into_iter()
        .map(|target| Detection {
            target,
            binary: find_binary(target, &env.path),
            config_dir: env.user_dir(target).is_some_and(|d| d.is_dir()),
            project: if target == Target::Zcode {
                Some(
                    env.zcode_project_configs()
                        .iter()
                        .any(|p| configured(target, p)),
                )
            } else {
                env.config_path(target, Scope::Project)
                    .ok()
                    .map(|p| configured(target, &p))
            },
            user: env
                .config_path(target, Scope::User)
                .ok()
                .is_some_and(|p| configured(target, &p)),
        })
        .collect()
}

/// True if the file holds hookah's hooks (for opencode, Pi, and OpenClaw: the plugin file exists; for Hermes: the
/// plugin is installed next to the config file and enabled in it).
fn configured(target: Target, path: &Path) -> bool {
    match fs::read_to_string(path) {
        Ok(text) => match target {
            Target::Antigravity => crate::merge::agent_hooks::antigravity::configured(&text),
            Target::Opencode | Target::Pi | Target::Openclaw => true,
            Target::Hermes => {
                let plugin = path.with_file_name("plugins/hookah/__init__.py");
                plugin.is_file() && yaml::enabled(&text)
            }
            Target::Zcode => zcode::configured(&text),
            _ => mentions_hookah(&text, target.name()),
        },
        Err(_) => false,
    }
}

fn find_binary(target: Target, path: &[PathBuf]) -> Option<PathBuf> {
    let suffixes = ["", ".exe", ".cmd", ".bat"];
    path.iter().find_map(|dir| {
        target.binaries().iter().find_map(|bin| {
            suffixes
                .iter()
                .map(|s| dir.join(format!("{bin}{s}")))
                .find(|p| p.is_file())
        })
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn root(name: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "hookah-detect-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn env(root: &Path) -> Env {
        Env {
            cwd: root.join("project"),
            home: Some(root.join("home")),
            xdg_config: None,
            pi_agent_dir: None,
            openclaw_state: None,
            path: vec![root.join("bin")],
            hermes_home: None,
        }
    }

    fn get(found: &[Detection], t: Target) -> &Detection {
        found.iter().find(|d| d.target == t).unwrap()
    }

    #[test]
    fn an_empty_machine_has_nothing() {
        let root = root("none");
        let found = detect(&env(&root));
        assert_eq!(found.len(), Target::ALL.len());
        for d in &found {
            assert!(!d.installed());
            assert!(!d.user);
            assert_ne!(d.project, Some(true));
        }
        assert_eq!(get(&found, Target::Kimi).project, None);
        assert_eq!(get(&found, Target::Hermes).project, None);
        assert_eq!(get(&found, Target::Claude).project, Some(false));
    }

    #[test]
    fn zcode_detects_native_hooks_and_the_user_cli_config_directory() {
        let root = root("zcode");
        let env = env(&root);
        let config = crate::snippet(Target::Zcode, crate::Program::Executable("hookah")).unwrap();
        write(&root.join("project/zcode.json"), &config);
        fs::create_dir_all(root.join("home/.zcode")).unwrap();
        let found = detect(&env);
        let zcode = get(&found, Target::Zcode);
        assert_eq!(zcode.project, Some(true));
        assert!(!zcode.config_dir);

        write(&root.join("home/.zcode/cli/config.json"), &config);
        let found = detect(&env);
        let zcode = get(&found, Target::Zcode);
        assert!(zcode.config_dir && zcode.user);
    }

    #[test]
    fn binaries_and_config_dirs_mean_installed() {
        let root = root("installed");
        write(&root.join("bin/claude"), "");
        write(&root.join("bin/qodercli.exe"), "");
        fs::create_dir_all(root.join("home/.codex")).unwrap();
        fs::create_dir_all(root.join("home/.config/opencode")).unwrap();
        fs::create_dir_all(root.join("home/.pi/agent")).unwrap();
        let found = detect(&env(&root));
        let claude = get(&found, Target::Claude);
        assert_eq!(claude.binary, Some(root.join("bin/claude")));
        assert!(!claude.config_dir);
        assert!(claude.installed());
        assert!(get(&found, Target::Qoder).installed());
        let codex = get(&found, Target::Codex);
        assert!(codex.binary.is_none() && codex.config_dir && codex.installed());
        assert!(get(&found, Target::Opencode).installed());
        assert!(get(&found, Target::Pi).installed());
        assert!(!get(&found, Target::Kimi).installed());
    }

    #[test]
    fn cursor_is_found_by_either_binary_or_by_its_config_directory() {
        for bin in ["cursor-agent", "cursor.cmd"] {
            let root = root("cursor-bin");
            write(&root.join("bin").join(bin), "");
            assert!(get(&detect(&env(&root)), Target::Cursor).binary.is_some());
        }
        let root = root("cursor-dir");
        fs::create_dir_all(root.join("home/.cursor")).unwrap();
        let cursor = detect(&env(&root))
            .into_iter()
            .find(|d| d.target == Target::Cursor)
            .unwrap();
        assert!(cursor.binary.is_none() && cursor.config_dir && cursor.installed());
    }

    #[test]
    fn antigravity_detects_its_named_group_in_the_correct_scope() {
        let root = root("antigravity");
        write(
            &root.join("project/.agents/hooks.json"),
            r#"{"other":{"Stop":[{"command":"hookah hook antigravity"}]}}"#,
        );
        write(
            &root.join("home/.gemini/config/hooks.json"),
            r#"{"hookah":{"PostToolUse":[{"matcher":"write_to_file","hooks":[{"command":"/opt/bin/hookah-dev hook antigravity"}]}]}}"#,
        );
        let found = detect(&env(&root));
        let antigravity = get(&found, Target::Antigravity);
        assert_eq!(antigravity.project, Some(false));
        assert!(antigravity.user);
        assert!(antigravity.config_dir);
    }

    #[test]
    fn a_cursor_hooks_json_with_hookah_is_configured() {
        let root = root("cursor-cfg");
        write(
            &root.join("project/.cursor/hooks.json"),
            r#"{"version": 1, "hooks": {"stop": [{"command": "hookah hook cursor"}]}}"#,
        );
        let found = detect(&env(&root));
        let cursor = get(&found, Target::Cursor);
        assert_eq!(cursor.project, Some(true));
        assert!(!cursor.user);
    }

    #[test]
    fn a_directory_named_like_a_binary_does_not_count() {
        let root = root("dirbin");
        fs::create_dir_all(root.join("bin/claude")).unwrap();
        assert!(get(&detect(&env(&root)), Target::Claude).binary.is_none());
    }

    #[test]
    fn configured_scopes_are_reported_separately() {
        let root = root("configured");
        write(
            &root.join("project/.claude/settings.json"),
            r#"{"hooks": {"Stop": [{"hooks": [{"command": "/x/hookah hook claude"}]}]}}"#,
        );
        write(
            &root.join("home/.codex/config.toml"),
            "command = 'hookah hook codex'\n",
        );
        write(&root.join("home/.qoder/settings.json"), r#"{"hooks": {}}"#);
        write(
            &root.join("project/.opencode/plugins/hookah.ts"),
            "// plugin",
        );
        write(&root.join("home/.pi/agent/extensions/hookah.ts"), "// ext");
        let found = detect(&env(&root));
        let claude = get(&found, Target::Claude);
        assert_eq!(claude.project, Some(true));
        assert!(!claude.user);
        let codex = get(&found, Target::Codex);
        assert_eq!(codex.project, Some(false));
        assert!(codex.user);
        assert_eq!(get(&found, Target::Qoder).project, Some(false));
        assert!(!get(&found, Target::Qoder).user);
        assert_eq!(get(&found, Target::Opencode).project, Some(true));
        assert_eq!(get(&found, Target::Pi).project, Some(false));
        assert!(get(&found, Target::Pi).user);
    }

    #[test]
    fn hermes_is_configured_only_with_the_plugin_installed_and_enabled() {
        let root = root("hermes");
        let hermes = |root: &Path| get(&detect(&env(root)), Target::Hermes).clone();
        write(&root.join("home/.hermes/config.yaml"), "model: x\n");
        let found = hermes(&root);
        assert!(found.config_dir && !found.user);
        write(
            &root.join("home/.hermes/config.yaml"),
            "plugins:\n  enabled:\n    - hookah\n",
        );
        assert!(!hermes(&root).user, "enabled but not installed");
        write(&root.join("home/.hermes/plugins/hookah/__init__.py"), "#");
        assert!(hermes(&root).user);
        write(&root.join("home/.hermes/config.yaml"), "model: x\n");
        assert!(!hermes(&root).user, "installed but not enabled");
        write(&root.join("bin/hermes"), "");
        assert!(hermes(&root).binary.is_some());
    }

    #[test]
    fn a_config_that_mentions_another_harness_is_not_configured() {
        let root = root("other");
        write(
            &root.join("project/.claude/settings.json"),
            r#"{"command": "hookah hook codex"}"#,
        );
        assert_eq!(
            get(&detect(&env(&root)), Target::Claude).project,
            Some(false)
        );
    }
}
