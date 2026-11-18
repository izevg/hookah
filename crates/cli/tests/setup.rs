//! End-to-end tests of `hookah setup`. Every run gets a private project directory, home, and
//! `PATH`, so nothing here can touch the real user config.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const HOOKAH: &str = env!("CARGO_BIN_EXE_hookah");

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "setup-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        for dir in ["project", "home", "bin"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        Self { root }
    }

    fn project(&self, rel: &str) -> PathBuf {
        self.root.join("project").join(rel)
    }

    fn home(&self, rel: &str) -> PathBuf {
        self.root.join("home").join(rel)
    }

    /// A `hookah` command whose every config location is inside the sandbox, on every OS: each
    /// variable `hookah setup` reads is cleared first, then the ones a harness needs are pointed
    /// at the sandbox. A test that wants another value sets it on the returned command.
    fn command(&self, args: &[&str]) -> Command {
        let home = self.root.join("home");
        let mut cmd = Command::new(HOOKAH);
        cmd.args(args).current_dir(self.root.join("project"));
        for var in hookah_setup::Env::VARS {
            cmd.env_remove(var);
        }
        cmd.env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("LOCALAPPDATA", home.join("AppData").join("Local"))
            .env("APPDATA", home.join("AppData").join("Roaming"))
            .env("HERMES_HOME", home.join(".hermes"))
            .env("PI_CODING_AGENT_DIR", home.join(".pi").join("agent"))
            .env("OPENCLAW_STATE_DIR", home.join(".openclaw"))
            .env("OPENCLAW_HOME", home.join("openclaw-home"))
            .env("PATH", self.root.join("bin"));
        cmd
    }

    fn hookah(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.hookah(args);
        assert!(
            out.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
}

fn text(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

#[test]
fn claude_project_setup_writes_hooks_pointing_at_this_binary() {
    let sb = Sandbox::new("claude");
    let out = sb.ok(&["setup", "claude"]);
    let file = sb.project(".claude/settings.json");
    let written = text(&file);
    // The command as written into JSON: forward slashes on Windows, quoted when it has spaces.
    let program = hookah_setup::shell_program(HOOKAH);
    let json_program = program.replace('\\', "\\\\").replace('"', "\\\"");
    assert!(
        written.contains(&format!("{json_program} hook claude")),
        "expected `{json_program} hook claude` in:\n{written}"
    );
    for event in ["SessionStart", "PostToolUse", "Stop", "SubagentStop"] {
        assert!(written.contains(event), "{event}");
    }
    assert!(
        out.starts_with("hookah setup: claude, project scope\ncreated .claude/settings.json\n")
    );
    assert!(out.contains("verify:") && out.contains("next: Restart Claude Code"));
    assert!(!sb.project(".claude/settings.json.hookah.bak").exists());
}

#[test]
fn a_second_run_is_byte_identical_and_says_so() {
    let sb = Sandbox::new("idempotent");
    for harness in ["claude", "codex", "qoder", "cursor", "opencode", "pi"] {
        sb.ok(&["setup", harness]);
    }
    sb.ok(&["setup", "kimi"]);
    let files = [
        sb.project(".claude/settings.json"),
        sb.project(".codex/config.toml"),
        sb.project(".qoder/settings.json"),
        sb.project(".cursor/hooks.json"),
        sb.project(".opencode/plugins/hookah.ts"),
        sb.project(".pi/extensions/hookah.ts"),
        sb.home(".kimi-code/config.toml"),
    ];
    let before: Vec<_> = files.iter().map(|f| fs::read(f).unwrap()).collect();
    for harness in [
        "claude", "codex", "qoder", "cursor", "opencode", "pi", "kimi",
    ] {
        let out = sb.ok(&["setup", harness]);
        assert!(out.contains("already up to date, nothing written"), "{out}");
    }
    let after: Vec<_> = files.iter().map(|f| fs::read(f).unwrap()).collect();
    assert_eq!(before, after);
}

#[test]
fn existing_settings_survive_and_the_original_is_backed_up_once() {
    let sb = Sandbox::new("merge");
    let file = sb.project(".claude/settings.json");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let original = "{\"permissions\": {\"allow\": [\"Bash(ls)\"]}, \"model\": \"x\"}\n";
    fs::write(&file, original).unwrap();
    let out = sb.ok(&["setup", "claude"]);
    assert!(out.contains("updated .claude/settings.json"));
    assert!(out.contains("backup of the original: .claude/settings.json.hookah.bak"));
    let merged = text(&file);
    assert!(merged.starts_with("{\n  \"permissions\""));
    assert!(merged.contains("Bash(ls)") && merged.contains("\"model\": \"x\""));
    let backup = sb.project(".claude/settings.json.hookah.bak");
    assert_eq!(text(&backup), original);
    sb.ok(&["setup", "claude", "--command", "mise exec -- hookah"]);
    assert_eq!(text(&backup), original);
    let updated = text(&file);
    assert!(updated.contains("mise exec -- hookah hook claude"));
    assert!(!updated.contains(&hookah_setup::shell_program(HOOKAH)));
    assert_eq!(updated.matches("hook claude").count(), 4);
}

#[test]
fn a_malformed_file_is_refused_and_left_untouched() {
    let sb = Sandbox::new("malformed");
    let file = sb.project(".codex/config.toml");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "[[hooks\n").unwrap();
    let out = sb.hookah(&["setup", "codex"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("hookah: setup: cannot merge into"),
        "{stderr}"
    );
    assert!(stderr.contains("not valid TOML"), "{stderr}");
    assert!(out.stdout.is_empty());
    assert_eq!(text(&file), "[[hooks\n");
    assert!(!sb.project(".codex/config.toml.hookah.bak").exists());
}

#[test]
fn codex_toml_keeps_comments() {
    let sb = Sandbox::new("codex");
    let file = sb.project(".codex/config.toml");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "# mine\nmodel = \"gpt\" # keep\n").unwrap();
    sb.ok(&["setup", "codex"]);
    let merged = text(&file);
    assert!(merged.starts_with("# mine\nmodel = \"gpt\" # keep\n"));
    assert!(merged.contains("[[hooks.PostToolUse.hooks]]"));
    assert!(merged.contains("matcher = \"^apply_patch$\""));
}

#[test]
fn user_scope_writes_under_home_for_every_harness() {
    let sb = Sandbox::new("user");
    for harness in [
        "claude", "codex", "qoder", "kimi", "cursor", "opencode", "pi",
    ] {
        sb.ok(&["setup", harness, "--scope", "user"]);
    }
    assert!(text(&sb.home(".claude/settings.json")).contains("hook claude"));
    assert!(text(&sb.home(".codex/config.toml")).contains("hook codex"));
    assert!(text(&sb.home(".qoder/settings.json")).contains("hook qoder"));
    assert!(text(&sb.home(".kimi-code/config.toml")).contains("hook kimi"));
    assert!(text(&sb.home(".cursor/hooks.json")).contains("hook cursor"));
    assert!(sb.home(".config/opencode/plugins/hookah.ts").is_file());
    assert!(sb.home(".pi/agent/extensions/hookah.ts").is_file());
    assert!(!sb.project(".claude").exists());
}

#[test]
fn opencode_honors_xdg_config_home() {
    let sb = Sandbox::new("xdg");
    let xdg = sb.root.join("xdg");
    let out = sb
        .command(&["setup", "opencode", "--scope", "user"])
        .env("XDG_CONFIG_HOME", &xdg)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(xdg.join("opencode/plugins/hookah.ts").is_file());
}

#[test]
fn pi_honors_pi_coding_agent_dir() {
    let sb = Sandbox::new("pidir");
    let dir = sb.root.join("pi-cfg");
    let out = sb
        .command(&["setup", "pi", "--scope", "user"])
        .env("PI_CODING_AGENT_DIR", &dir)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(dir.join("extensions/hookah.ts").is_file());
    assert!(!sb.home(".pi").exists());
}

#[test]
fn the_pi_extension_matches_hookah_plugin_pi() {
    let sb = Sandbox::new("piplugin");
    let dry = sb.ok(&["setup", "pi", "--dry-run"]);
    assert!(dry.contains("would create .pi/extensions/hookah.ts"));
    assert!(!sb.project(".pi").exists());
    sb.ok(&["setup", "pi"]);
    let printed = sb.ok(&["plugin", "pi"]);
    assert_eq!(text(&sb.project(".pi/extensions/hookah.ts")), printed);
    assert_eq!(sb.ok(&["setup", "pi", "--print"]), printed);
}

#[test]
fn rerunning_pi_setup_backs_up_a_changed_extension_once() {
    let sb = Sandbox::new("pibak");
    let file = sb.project(".pi/extensions/hookah.ts");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "// old\n").unwrap();
    sb.ok(&["setup", "pi"]);
    assert_eq!(
        text(&sb.project(".pi/extensions/hookah.ts.hookah.bak")),
        "// old\n"
    );
    assert_eq!(text(&file), sb.ok(&["plugin", "pi"]));
}

#[test]
fn the_opencode_plugin_matches_hookah_plugin_opencode() {
    let sb = Sandbox::new("plugin");
    sb.ok(&["setup", "opencode"]);
    let printed = sb.ok(&["plugin", "opencode"]);
    assert_eq!(text(&sb.project(".opencode/plugins/hookah.ts")), printed);
    assert_eq!(sb.ok(&["setup", "opencode", "--print"]), printed);
}

/// The sandbox is the only place `hookah setup` may write, for every harness at every scope, on
/// every OS: each path the command reports is inside it, and nothing appears next to it.
#[test]
fn setup_never_writes_outside_the_sandbox() {
    let sb = Sandbox::new("guard");
    for harness in [
        "claude", "codex", "qoder", "kimi", "cursor", "opencode", "hermes", "pi", "openclaw",
    ] {
        for scope in ["user", "project"] {
            let out = sb.hookah(&["setup", harness, "--scope", scope]);
            let stdout = String::from_utf8(out.stdout).unwrap();
            for line in stdout.lines() {
                let Some(path) = ["created ", "updated ", "backup of the original: "]
                    .iter()
                    .find_map(|verb| line.strip_prefix(verb))
                else {
                    continue;
                };
                // Canonical on both sides: a Windows temp dir may be spelled with 8.3 short names.
                let path = sb.root.join("project").join(path);
                let path = fs::canonicalize(&path).unwrap_or(path);
                let root = fs::canonicalize(&sb.root).unwrap();
                assert!(
                    path.starts_with(&root),
                    "{harness} --scope {scope} wrote {path:?}, outside {root:?}"
                );
            }
        }
    }
}

#[test]
fn hermes_setup_installs_the_plugin_and_enables_it_without_touching_the_rest() {
    let sb = Sandbox::new("hermes");
    let config = sb.home(".hermes/config.yaml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let original = "# mine\nmodel: x\nplugins:\n  enabled:\n    - kanban  # keep\n";
    fs::write(&config, original).unwrap();
    let out = sb.ok(&["setup", "hermes"]);
    assert!(
        out.starts_with("hookah setup: hermes, user scope\nupdated "),
        "{out}"
    );
    assert!(out.contains("config.yaml\n"), "{out}");
    assert!(out.contains("hook_callback_timeout"), "{out}");
    assert_eq!(text(&config), format!("{original}    - hookah\n"));
    assert_eq!(text(&sb.home(".hermes/config.yaml.hookah.bak")), original);
    let plugin = sb.home(".hermes/plugins/hookah");
    assert!(text(&plugin.join("plugin.yaml")).starts_with("name: hookah\n"));
    assert_eq!(
        text(&plugin.join("__init__.py")),
        sb.ok(&["plugin", "hermes"])
    );
    // A second run changes nothing.
    let again = sb.ok(&["setup", "hermes"]);
    assert!(
        again.contains("already up to date, nothing written"),
        "{again}"
    );
    assert_eq!(text(&config), format!("{original}    - hookah\n"));
}

#[test]
fn hermes_setup_creates_a_missing_config_and_honors_hermes_home() {
    let sb = Sandbox::new("hermes-home");
    sb.ok(&["setup", "hermes"]);
    assert_eq!(
        text(&sb.home(".hermes/config.yaml")),
        "plugins:\n  enabled:\n    - hookah\n"
    );
    let elsewhere = sb.root.join("elsewhere");
    let out = sb
        .command(&["setup", "hermes"])
        .env("HERMES_HOME", &elsewhere)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(elsewhere.join("config.yaml").is_file());
    assert!(elsewhere.join("plugins/hookah/__init__.py").is_file());
}

#[test]
fn hermes_refuses_what_it_cannot_edit_safely_and_project_scope() {
    let sb = Sandbox::new("hermes-refuse");
    let config = sb.home(".hermes/config.yaml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "plugins:\n  enabled: [a]\n").unwrap();
    let refused = sb.hookah(&["setup", "hermes"]);
    assert_eq!(refused.status.code(), Some(1));
    let stderr = String::from_utf8(refused.stderr).unwrap();
    assert!(
        stderr.contains("add `hookah` to plugins.enabled by hand"),
        "{stderr}"
    );
    assert_eq!(text(&config), "plugins:\n  enabled: [a]\n");
    assert!(!sb.home(".hermes/plugins").exists());
    let project = sb.hookah(&["setup", "hermes", "--scope", "project"]);
    assert_eq!(project.status.code(), Some(1));
    assert!(
        String::from_utf8(project.stderr)
            .unwrap()
            .contains("hermes has no project-level config")
    );
}

#[test]
fn hermes_dry_run_print_and_command_note() {
    let sb = Sandbox::new("hermes-dry");
    let dry = sb.ok(&["setup", "hermes", "--dry-run"]);
    assert!(
        dry.contains("would create ") && dry.contains("config.yaml"),
        "{dry}"
    );
    assert!(dry.contains("plugins/hookah/__init__.py"), "{dry}");
    assert_eq!(
        sb.ok(&["setup", "hermes", "--print"]),
        "plugins:\n  enabled:\n    - hookah\n"
    );
    assert!(!sb.home(".hermes").exists());
    let out = sb.ok(&["setup", "hermes", "--command", "x"]);
    assert!(
        out.contains("note: --command is ignored for Hermes"),
        "{out}"
    );
}

#[test]
fn hermes_is_detected_once_the_plugin_is_installed_and_enabled() {
    let sb = Sandbox::new("hermes-detect");
    fs::write(sb.root.join("bin/hermes"), "").unwrap();
    let before = sb.ok(&["setup", "--detect"]);
    assert!(before.contains("hermes: installed (binary"), "{before}");
    assert!(
        before.contains("project: no project config; user: not configured"),
        "{before}"
    );
    sb.ok(&["setup", "hermes"]);
    let after = sb.ok(&["setup", "--detect"]);
    let line = after.lines().find(|l| l.starts_with("hermes")).unwrap();
    assert!(line.ends_with("user: configured"), "{line}");
}

#[test]
fn kimi_defaults_to_user_scope_and_has_no_project_config() {
    let sb = Sandbox::new("kimi");
    let out = sb.ok(&["setup", "kimi"]);
    assert!(out.starts_with("hookah setup: kimi, user scope\n"));
    assert!(sb.home(".kimi-code/config.toml").is_file());
    let refused = sb.hookah(&["setup", "kimi", "--scope", "project"]);
    assert_eq!(refused.status.code(), Some(1));
    let stderr = String::from_utf8(refused.stderr).unwrap();
    assert!(
        stderr.contains("kimi has no project-level config"),
        "{stderr}"
    );
}

#[test]
fn dry_run_and_print_write_nothing() {
    let sb = Sandbox::new("dry");
    let dry = sb.ok(&["setup", "qoder", "--dry-run"]);
    assert!(dry.contains("(dry run, nothing written)"));
    assert!(dry.contains("would create .qoder/settings.json with this content:"));
    assert!(dry.contains("\"hooks\""));
    let printed = sb.ok(&["setup", "qoder", "--print"]);
    assert!(printed.starts_with("{\n  \"hooks\""));
    assert!(!printed.contains("hookah setup:"));
    assert!(!sb.project(".qoder").exists());
}

#[test]
fn openclaw_installs_the_plugin_directory_into_the_state_directory() {
    let sb = Sandbox::new("openclaw");
    let out = sb.ok(&["setup", "openclaw"]);
    assert!(
        out.starts_with("hookah setup: openclaw, user scope\n"),
        "{out}"
    );
    let dir = sb.home(".openclaw/extensions/hookah");
    let printed = sb.ok(&["plugin", "openclaw"]);
    assert_eq!(text(&dir.join("index.js")), printed);
    assert_eq!(sb.ok(&["setup", "openclaw", "--print"]), printed);
    let manifest = text(&dir.join("openclaw.plugin.json"));
    assert!(manifest.contains("\"id\": \"hookah\""));
    assert!(out.contains("openclaw.plugin.json"), "{out}");
    assert!(
        out.contains("installed the bundled openclaw plugin"),
        "{out}"
    );
    assert!(out.contains("openclaw plugins enable hookah"), "{out}");
    let again = sb.ok(&["setup", "openclaw"]);
    assert!(
        again.contains("already up to date, nothing written"),
        "{again}"
    );
    // A stale manifest alone is repaired without touching the entry point.
    fs::write(dir.join("openclaw.plugin.json"), "{}").unwrap();
    let repaired = sb.ok(&["setup", "openclaw"]);
    assert!(
        repaired.contains("updated") && repaired.contains("openclaw.plugin.json"),
        "{repaired}"
    );
    assert_eq!(text(&dir.join("openclaw.plugin.json")), manifest);
    let refused = sb.hookah(&["setup", "openclaw", "--scope", "project"]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        String::from_utf8(refused.stderr)
            .unwrap()
            .contains("--scope user")
    );
    assert!(!sb.project(".openclaw").exists());
}

#[test]
fn openclaw_honors_its_home_override() {
    let sb = Sandbox::new("openclaw-home");
    let alt = sb.root.join("alt");
    let mut cmd = sb.command(&["setup", "openclaw"]);
    cmd.env_remove("OPENCLAW_STATE_DIR")
        .env("OPENCLAW_HOME", &alt);
    assert!(cmd.output().unwrap().status.success());
    assert!(alt.join(".openclaw/extensions/hookah/index.js").is_file());
    let mut cmd = sb.command(&["setup", "openclaw"]);
    cmd.env_remove("OPENCLAW_STATE_DIR")
        .env_remove("OPENCLAW_HOME");
    assert!(cmd.output().unwrap().status.success());
    assert!(sb.home(".openclaw/extensions/hookah/index.js").is_file());
}

#[test]
fn command_override_is_used_verbatim() {
    let sb = Sandbox::new("command");
    let kimi = sb.ok(&["setup", "kimi", "--command", "mise exec -- hookah"]);
    assert!(!kimi.contains("note:"));
    let written = text(&sb.home(".kimi-code/config.toml"));
    assert!(written.contains("command = \"mise exec -- hookah hook kimi\""));
    let out = sb.ok(&["setup", "opencode", "--command", "x"]);
    assert!(out.contains("note: --command is ignored for opencode"));
    let plain = sb.ok(&["setup", "opencode", "--scope", "user"]);
    assert!(!plain.contains("note:"));
    let out = sb.ok(&["setup", "pi", "--command", "x"]);
    assert!(out.contains("note: --command is ignored for pi"));
    let out = sb.ok(&["setup", "openclaw", "--command", "x"]);
    assert!(out.contains("note: --command is ignored for openclaw"));
}

#[test]
fn detect_reports_installed_and_configured_harnesses() {
    let sb = Sandbox::new("detect");
    fs::write(sb.root.join("bin/claude"), "").unwrap();
    fs::write(sb.root.join("bin/pi"), "").unwrap();
    fs::create_dir_all(sb.home(".codex")).unwrap();
    sb.ok(&["setup", "claude"]);
    sb.ok(&["setup", "pi"]);
    sb.ok(&["setup", "openclaw"]);
    sb.ok(&["setup", "qoder", "--scope", "user"]);
    let out = sb.ok(&["setup", "--detect"]);
    let line = |name: &str| {
        out.lines()
            .find(|l| l.starts_with(name))
            .unwrap()
            .to_owned()
    };
    assert!(line("claude:").contains("installed (binary"));
    assert!(line("claude:").ends_with("project: configured; user: not configured"));
    assert!(line("codex:").contains("config directory found"));
    assert!(line("qoder:").ends_with("project: not configured; user: configured"));
    assert!(line("pi:").contains("installed (binary"));
    assert!(line("pi:").ends_with("project: configured; user: not configured"));
    assert!(line("kimi:").contains("not found; project: no project config"));
    assert!(out.contains("next:"));
    assert!(!sb.project(".codex").exists());
}

#[test]
fn flags_that_do_not_fit_together_are_rejected() {
    let sb = Sandbox::new("flags");
    for args in [
        &["setup"][..],
        &["setup", "claude", "--detect"],
        &["setup", "claude", "--dry-run", "--print"],
        &["setup", "--detect", "--scope", "user"],
    ] {
        let out = sb.hookah(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn cursor_setup_merges_into_an_existing_hooks_json() {
    let sb = Sandbox::new("cursor");
    let file = sb.project(".cursor/hooks.json");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let original = "{\"version\": 1, \"hooks\": {\"stop\": [{\"command\": \"./audit.sh\"}]}}\n";
    fs::write(&file, original).unwrap();
    let out = sb.ok(&["setup", "cursor"]);
    assert!(out.starts_with("hookah setup: cursor, project scope\nupdated .cursor/hooks.json\n"));
    assert!(
        out.contains("hooks: sessionStart, afterFileEdit, stop run "),
        "{out}"
    );
    assert!(out.contains("next: Restart Cursor"), "{out}");
    let merged = text(&file);
    assert!(merged.contains("./audit.sh") && merged.contains("afterFileEdit"));
    assert!(merged.contains("hook cursor"));
    assert_eq!(text(&sb.project(".cursor/hooks.json.hookah.bak")), original);
    let dry = sb.ok(&["setup", "cursor", "--dry-run"]);
    assert!(dry.contains(".cursor/hooks.json is already up to date and would stay:"));
}

#[test]
fn detect_recognizes_cursor_by_binary_and_by_config_directory() {
    let sb = Sandbox::new("detectcursor");
    let line = |out: &str| {
        out.lines()
            .find(|l| l.starts_with("cursor:"))
            .unwrap()
            .to_owned()
    };
    assert!(line(&sb.ok(&["setup", "--detect"])).contains("not found"));
    fs::write(sb.root.join("bin/cursor-agent"), "").unwrap();
    assert!(line(&sb.ok(&["setup", "--detect"])).contains("installed (binary"));
    fs::remove_file(sb.root.join("bin/cursor-agent")).unwrap();
    fs::create_dir_all(sb.home(".cursor")).unwrap();
    let out = sb.ok(&["setup", "--detect"]);
    assert!(line(&out).contains("config directory found"), "{out}");
    sb.ok(&["setup", "cursor", "--scope", "user"]);
    assert!(
        line(&sb.ok(&["setup", "--detect"])).ends_with("project: not configured; user: configured")
    );
}
