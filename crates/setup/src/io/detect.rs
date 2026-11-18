//! `hookah setup --detect`: which harnesses are here and which are already configured. Read-only.

use std::fs;
use std::path::{Path, PathBuf};

use super::env::{Env, Scope};
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
            project: env
                .config_path(target, Scope::Project)
                .ok()
                .map(|p| configured(target, &p)),
            user: env
                .config_path(target, Scope::User)
                .ok()
                .is_some_and(|p| configured(target, &p)),
        })
        .collect()
}

/// True if the file holds hookah's hooks (for opencode: the plugin file exists).
fn configured(target: Target, path: &Path) -> bool {
    match fs::read_to_string(path) {
        Ok(text) => target == Target::Opencode || mentions_hookah(&text, target.name()),
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
            path: vec![root.join("bin")],
        }
    }

    fn get(found: &[Detection], t: Target) -> &Detection {
        found.iter().find(|d| d.target == t).unwrap()
    }

    #[test]
    fn an_empty_machine_has_nothing() {
        let root = root("none");
        let found = detect(&env(&root));
        assert_eq!(found.len(), 5);
        for d in &found {
            assert!(!d.installed());
            assert!(!d.user);
            assert_ne!(d.project, Some(true));
        }
        assert_eq!(get(&found, Target::Kimi).project, None);
        assert_eq!(get(&found, Target::Claude).project, Some(false));
    }

    #[test]
    fn binaries_and_config_dirs_mean_installed() {
        let root = root("installed");
        write(&root.join("bin/claude"), "");
        write(&root.join("bin/qodercli.exe"), "");
        fs::create_dir_all(root.join("home/.codex")).unwrap();
        fs::create_dir_all(root.join("home/.config/opencode")).unwrap();
        let found = detect(&env(&root));
        let claude = get(&found, Target::Claude);
        assert_eq!(claude.binary, Some(root.join("bin/claude")));
        assert!(!claude.config_dir);
        assert!(claude.installed());
        assert!(get(&found, Target::Qoder).installed());
        let codex = get(&found, Target::Codex);
        assert!(codex.binary.is_none() && codex.config_dir && codex.installed());
        assert!(get(&found, Target::Opencode).installed());
        assert!(!get(&found, Target::Kimi).installed());
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
