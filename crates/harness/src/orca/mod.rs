//! Orca harness adapter.
//!
//! Orca runs other agents (Claude Code, Codex, ...) in managed worktrees, so per-edit feedback
//! comes from the hooks of the agent it hosts, set up with `hookah setup <harness>`. There is no
//! hook payload to parse here. What this module ships is an Orca plugin, a folder of TypeScript
//! sources that adds what only Orca can do: the latest hookah verdict per worktree (panel and
//! status bar, read with `hookah status --json`), a palette command that checks a worktree
//! (`hookah check --changed`), and optional setup of hookah for an agent right before Orca
//! starts it. The plugin only calls the `hookah` binary; the rules stay in the use cases.
//!
//! The sources live in `plugin/` (with their tests in `plugin/test/`). [`PLUGIN_FILES`] embeds
//! the ones Orca loads, and `hookah plugin orca --out <dir>` writes them. See
//! `docs/setup/plugins/orca.md`.

/// One file of the plugin folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginFile {
    /// The path inside the plugin folder, with `/` separators.
    pub path: &'static str,
    /// The file's text.
    pub text: &'static str,
}

/// The files Orca loads, in the order they are written.
pub const PLUGIN_FILES: &[PluginFile] = &[
    PluginFile {
        path: "orca-plugin.json",
        text: include_str!("plugin/orca-plugin.json"),
    },
    PluginFile {
        path: "src/hookah.ts",
        text: include_str!("plugin/src/hookah.ts"),
    },
    PluginFile {
        path: "src/panel.tsx",
        text: include_str!("plugin/src/panel.tsx"),
    },
    PluginFile {
        path: "src/protocol.ts",
        text: include_str!("plugin/src/protocol.ts"),
    },
    PluginFile {
        path: "src/worker.ts",
        text: include_str!("plugin/src/worker.ts"),
    },
];

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn file(path: &str) -> &'static str {
        PLUGIN_FILES.iter().find(|f| f.path == path).unwrap().text
    }

    fn manifest() -> Value {
        serde_json::from_str(file("orca-plugin.json")).unwrap()
    }

    #[test]
    fn the_manifest_entry_points_are_shipped() {
        let m = manifest();
        let main = m["main"].as_str().unwrap();
        let panel = m["contributes"]["panels"][0]["entry"].as_str().unwrap();
        for path in [main, panel] {
            assert!(PLUGIN_FILES.iter().any(|f| f.path == path), "{path}");
        }
    }

    #[test]
    fn the_manifest_asks_only_for_what_the_worker_uses() {
        let m = manifest();
        assert_eq!(m["execution"], "trusted-worker");
        let kinds: Vec<&str> = m["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["kind"].as_str().unwrap())
            .collect();
        // No terminal access, no workspace changes, no agent launches.
        assert_eq!(
            kinds,
            [
                "workspace:read",
                "agent:read",
                "notifications:show",
                "settings:own"
            ]
        );
    }

    #[test]
    fn every_command_event_hook_and_setting_has_code_behind_it() {
        let m = manifest();
        let worker = file("src/worker.ts");
        for c in m["contributes"]["commands"].as_array().unwrap() {
            let id = c["id"].as_str().unwrap();
            assert!(worker.contains(&format!("register('{id}'")), "{id}");
        }
        for e in m["contributes"]["events"].as_array().unwrap() {
            let on = e["on"].as_str().unwrap();
            assert!(worker.contains(&format!("events.on('{on}'")), "{on}");
        }
        for h in m["contributes"]["hooks"].as_array().unwrap() {
            let event = h["event"].as_str().unwrap();
            assert!(worker.contains(&format!("hooks.on('{event}'")), "{event}");
        }
        for s in m["contributes"]["settings"].as_array().unwrap() {
            let key = s["key"].as_str().unwrap();
            assert!(worker.contains(&format!("all.{key}")), "{key}");
        }
    }

    #[test]
    fn the_plugin_only_reads_and_checks_unless_the_user_allows_setup() {
        let m = manifest();
        let setup = m["contributes"]["settings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["key"] == "setup")
            .unwrap();
        // The default must not write into a repository.
        assert_eq!(setup["default"], "suggest");
    }

    #[test]
    fn the_sources_call_only_commands_hookah_has() {
        let client = file("src/hookah.ts");
        for call in [
            "['status', '--json', '--root', dir]",
            "['setup', '--detect']",
            "['setup', harness, '--scope', 'project']",
            "['check', '--changed', '--root', dir]",
        ] {
            assert!(client.contains(call), "{call}");
        }
    }

    #[test]
    fn the_sources_are_english_ascii_text() {
        for f in PLUGIN_FILES {
            assert!(f.text.is_ascii(), "{}", f.path);
            assert!(f.text.ends_with('\n'), "{}", f.path);
        }
    }
}
