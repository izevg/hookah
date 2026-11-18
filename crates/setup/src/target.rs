//! The supported harnesses and the hooks each one gets, exactly as documented in `docs/setup/`.

/// A harness `hookah setup` can configure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Claude Code: `settings.json`.
    Claude,
    /// Codex CLI: `config.toml`.
    Codex,
    /// Qoder CLI: `settings.json`.
    Qoder,
    /// Kimi Code: `config.toml`, user-level only.
    Kimi,
    /// Cursor (the IDE agent and `cursor-agent`): `hooks.json`.
    Cursor,
    /// opencode: a plugin file.
    Opencode,
    /// Hermes Agent: a plugin directory, enabled in `config.yaml`, user-level only.
    Hermes,
    /// Pi coding agent: an extension file.
    Pi,
    /// OpenClaw: a plugin directory (entry point and manifest).
    Openclaw,
}

/// One hook entry: the agent event, an optional tool matcher, and the timeout in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookSpec {
    /// The agent event, for example `PostToolUse`.
    pub event: &'static str,
    /// Which sessions or tools the hook applies to.
    pub matcher: Option<&'static str>,
    /// Seconds before the agent gives up on the hook.
    pub timeout: i64,
}

const fn hook(event: &'static str, matcher: Option<&'static str>, timeout: i64) -> HookSpec {
    HookSpec {
        event,
        matcher,
        timeout,
    }
}

const CLAUDE: &[HookSpec] = &[
    hook("SessionStart", Some("startup|resume"), 30),
    hook("PostToolUse", Some("Edit|Write|MultiEdit|NotebookEdit"), 30),
    hook("Stop", None, 120),
    hook("SubagentStop", None, 120),
];
const CODEX: &[HookSpec] = &[
    hook("SessionStart", Some("startup|resume"), 30),
    hook("PostToolUse", Some("^apply_patch$"), 30),
    hook("Stop", None, 120),
];
const QODER: &[HookSpec] = &[
    hook("SessionStart", None, 30),
    hook("PostToolUse", Some("Write|Edit"), 30),
    hook("Stop", None, 120),
];
const KIMI: &[HookSpec] = &[
    hook("SessionStart", Some("startup|resume"), 30),
    hook("PostToolUse", Some("Write|Edit"), 30),
];

const CURSOR: &[HookSpec] = &[
    hook("sessionStart", None, 30),
    hook("afterFileEdit", None, 30),
    hook("stop", None, 120),
];

impl Target {
    /// Every harness, in the order `hookah setup --detect` lists them.
    pub const ALL: [Self; 9] = [
        Self::Claude,
        Self::Codex,
        Self::Qoder,
        Self::Kimi,
        Self::Cursor,
        Self::Opencode,
        Self::Hermes,
        Self::Pi,
        Self::Openclaw,
    ];

    /// The name used on the command line and in `hookah hook <name>`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Qoder => "qoder",
            Self::Kimi => "kimi",
            Self::Cursor => "cursor",
            Self::Opencode => "opencode",
            Self::Hermes => "hermes",
            Self::Pi => "pi",
            Self::Openclaw => "openclaw",
        }
    }

    /// The hooks hookah registers. Empty for the plugin harnesses (opencode, Hermes, Pi,
    /// OpenClaw), whose plugins carry them themselves.
    pub fn hooks(self) -> &'static [HookSpec] {
        match self {
            Self::Claude => CLAUDE,
            Self::Codex => CODEX,
            Self::Qoder => QODER,
            Self::Kimi => KIMI,
            Self::Cursor => CURSOR,
            Self::Opencode | Self::Hermes | Self::Pi | Self::Openclaw => &[],
        }
    }

    /// Executable names that mean the harness is installed.
    pub fn binaries(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["claude"],
            Self::Codex => &["codex"],
            Self::Qoder => &["qodercli", "qoder"],
            Self::Kimi => &["kimi", "kimi-code"],
            Self::Cursor => &["cursor-agent", "cursor"],
            Self::Opencode => &["opencode"],
            Self::Hermes => &["hermes"],
            Self::Pi => &["pi"],
            Self::Openclaw => &["openclaw"],
        }
    }

    /// The harness as its users call it, for messages.
    pub fn title(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Qoder => "Qoder",
            Self::Kimi => "Kimi Code",
            Self::Cursor => "Cursor",
            Self::Opencode => "opencode",
            Self::Hermes => "Hermes",
            Self::Pi => "Pi",
            Self::Openclaw => "OpenClaw",
        }
    }

    /// What the user must do after the config is written.
    pub fn restart_hint(self) -> &'static str {
        match self {
            Self::Claude => {
                "Restart Claude Code, or run /hooks in it to review and approve the new hooks."
            }
            Self::Codex => {
                "Restart Codex, then run /hooks in it to review and trust the new hooks."
            }
            Self::Qoder => "Restart Qoder so it loads the new hooks.",
            Self::Kimi => "Restart Kimi Code so it loads the new hooks.",
            Self::Cursor => {
                "Restart Cursor (or the cursor-agent session), then check the Hooks tab in Cursor Settings."
            }
            Self::Opencode => {
                "Restart opencode. The plugin runs `hookah` from PATH, or from HOOKAH_BIN if set."
            }
            Self::Hermes => {
                "Restart Hermes so it loads the plugin. The plugin runs `hookah` from PATH, or from HOOKAH_BIN if set."
            }
            Self::Pi => {
                "Restart Pi, or run /reload in it. The extension runs `hookah` from PATH, or from HOOKAH_BIN if set."
            }
            Self::Openclaw => {
                "Run `openclaw plugins enable hookah` and `openclaw config set \
                 plugins.entries.hookah.hooks.allowConversationAccess true`, then restart the \
                 OpenClaw Gateway. The plugin runs `hookah` from PATH, or from HOOKAH_BIN if set."
            }
        }
    }
}

/// Quotes a program path for a shell command line when it contains whitespace.
pub fn quote_program(program: &str) -> String {
    if program.contains(char::is_whitespace) {
        format!("\"{program}\"")
    } else {
        program.to_owned()
    }
}

/// The command line prefix that runs the executable at `exe` in the shell every harness uses.
///
/// A Windows path is written with `/`, and quoted when it contains whitespace. Git Bash (Claude
/// Code) eats backslashes, and cmd.exe (Codex, and the Node and Python spawners of the others)
/// accepts a drive-qualified path with `/`, so one spelling works in all of them. A `\\?\`
/// verbatim prefix is dropped because shells do not understand it. Other paths are only quoted.
pub fn shell_program(exe: &str) -> String {
    let path = exe.strip_prefix(r"\\?\").unwrap_or(exe);
    let drive = matches!(path.as_bytes(), [d, b':', b'\\' | b'/', ..] if d.is_ascii_alphabetic());
    if drive || path.starts_with(r"\\") {
        quote_program(&path.replace('\\', "/"))
    } else {
        quote_program(exe)
    }
}

/// True if `text` mentions `hookah hook <name>` as a command, whatever the path before it, the
/// quoting, or an `.exe` suffix. This is how an existing hookah entry is recognized.
pub fn mentions_hookah(text: &str, name: &str) -> bool {
    let plain: String = text
        .chars()
        .filter(|c| !matches!(c, '"' | '\'' | '\\'))
        .collect::<String>()
        .replace(".exe", "");
    let needle = format!("hookah hook {name}");
    plain.match_indices(&needle).any(|(at, _)| {
        plain[at + needle.len()..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_'))
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_hookah_commands_in_every_spelling() {
        for c in [
            "hookah hook claude",
            "/home/u/.local/bin/hookah hook claude",
            "mise exec -- hookah hook claude",
            "\"/my dir/hookah\" hook claude",
            "C:\\Program Files\\hookah.exe hook claude",
            "\\\"C:\\\\x\\\\hookah.exe\\\" hook claude",
            "hookah hook claude --report-limit 3",
        ] {
            assert!(mentions_hookah(c, "claude"), "{c}");
        }
    }

    #[test]
    fn does_not_confuse_other_commands() {
        assert!(!mentions_hookah("hookah hook codex", "claude"));
        assert!(!mentions_hookah("hookah hook claudex", "claude"));
        assert!(!mentions_hookah("hookah hook claude_old", "claude"));
        assert!(!mentions_hookah("hookah hook claude-old", "claude"));
        assert!(!mentions_hookah("echo hookah", "claude"));
        assert!(mentions_hookah("hookah hook claude;", "claude"));
    }

    #[test]
    fn quotes_only_paths_with_whitespace() {
        assert_eq!(quote_program("/a/hookah"), "/a/hookah");
        assert_eq!(quote_program("/a b/hookah"), "\"/a b/hookah\"");
    }

    #[test]
    fn windows_paths_become_forward_slash_commands() {
        assert_eq!(
            shell_program(r"C:\Users\x\AppData\Local\hookah\bin\hookah.exe"),
            "C:/Users/x/AppData/Local/hookah/bin/hookah.exe"
        );
        assert_eq!(
            shell_program(r"D:\Program Files\hookah\hookah.exe"),
            "\"D:/Program Files/hookah/hookah.exe\""
        );
        assert_eq!(shell_program(r"c:/a/hookah.exe"), "c:/a/hookah.exe");
        assert_eq!(
            shell_program(r"\\?\C:\a b\hookah.exe"),
            "\"C:/a b/hookah.exe\""
        );
        assert_eq!(
            shell_program(r"\\srv\share\hookah.exe"),
            "//srv/share/hookah.exe"
        );
    }

    #[test]
    fn unix_paths_are_only_quoted() {
        assert_eq!(shell_program("/a/hookah"), "/a/hookah");
        assert_eq!(shell_program("/a b/hookah"), "\"/a b/hookah\"");
        assert_eq!(shell_program(r"/a\b/hookah"), r"/a\b/hookah");
        assert_eq!(shell_program("x:y"), "x:y");
        assert_eq!(shell_program("C:"), "C:");
        assert_eq!(shell_program(r"1:\a"), r"1:\a");
    }

    #[test]
    fn windows_forms_are_still_recognized_as_configured() {
        for c in [
            "C:/Users/x/AppData/Local/hookah/bin/hookah.exe hook claude",
            "\"D:/Program Files/hookah/hookah.exe\" hook codex",
            "\\\"D:/Program Files/hookah/hookah.exe\\\" hook codex",
        ] {
            let name = if c.contains("codex") {
                "codex"
            } else {
                "claude"
            };
            assert!(mentions_hookah(c, name), "{c}");
        }
    }

    #[test]
    fn names_and_hooks_line_up_with_the_setup_docs() {
        let names: Vec<_> = Target::ALL.iter().map(|t| t.name()).collect();
        assert_eq!(
            names,
            [
                "claude", "codex", "qoder", "kimi", "cursor", "opencode", "hermes", "pi",
                "openclaw"
            ]
        );
        assert_eq!(Target::Claude.hooks().len(), 4);
        assert_eq!(Target::Codex.hooks().len(), 3);
        assert_eq!(Target::Qoder.hooks().len(), 3);
        assert_eq!(Target::Kimi.hooks().len(), 2);
        assert_eq!(Target::Cursor.hooks().len(), 3);
        assert!(Target::Opencode.hooks().is_empty());
        assert!(Target::Hermes.hooks().is_empty());
        assert!(Target::Pi.hooks().is_empty());
        assert!(Target::Openclaw.hooks().is_empty());
        assert_eq!(Target::Claude.title(), "Claude Code");
        assert_eq!(Target::Openclaw.title(), "OpenClaw");
        for t in Target::ALL {
            assert!(!t.binaries().is_empty());
            assert!(t.restart_hint().contains("estart"), "{t:?}");
            assert!(!t.title().is_empty());
        }
    }
}
