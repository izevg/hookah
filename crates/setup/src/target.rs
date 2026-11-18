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
    /// opencode: a plugin file.
    Opencode,
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

impl Target {
    /// Every harness, in the order `hookah setup --detect` lists them.
    pub const ALL: [Self; 5] = [
        Self::Claude,
        Self::Codex,
        Self::Qoder,
        Self::Kimi,
        Self::Opencode,
    ];

    /// The name used on the command line and in `hookah hook <name>`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Qoder => "qoder",
            Self::Kimi => "kimi",
            Self::Opencode => "opencode",
        }
    }

    /// The hooks hookah registers. Empty for opencode, whose plugin carries them itself.
    pub fn hooks(self) -> &'static [HookSpec] {
        match self {
            Self::Claude => CLAUDE,
            Self::Codex => CODEX,
            Self::Qoder => QODER,
            Self::Kimi => KIMI,
            Self::Opencode => &[],
        }
    }

    /// Executable names that mean the harness is installed.
    pub fn binaries(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["claude"],
            Self::Codex => &["codex"],
            Self::Qoder => &["qodercli", "qoder"],
            Self::Kimi => &["kimi", "kimi-code"],
            Self::Opencode => &["opencode"],
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
            Self::Opencode => {
                "Restart opencode. The plugin runs `hookah` from PATH, or from HOOKAH_BIN if set."
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
    fn names_and_hooks_line_up_with_the_setup_docs() {
        let names: Vec<_> = Target::ALL.iter().map(|t| t.name()).collect();
        assert_eq!(names, ["claude", "codex", "qoder", "kimi", "opencode"]);
        assert_eq!(Target::Claude.hooks().len(), 4);
        assert_eq!(Target::Codex.hooks().len(), 3);
        assert_eq!(Target::Qoder.hooks().len(), 3);
        assert_eq!(Target::Kimi.hooks().len(), 2);
        assert!(Target::Opencode.hooks().is_empty());
        for t in Target::ALL {
            assert!(!t.binaries().is_empty());
            assert!(!t.restart_hint().is_empty());
        }
    }
}
