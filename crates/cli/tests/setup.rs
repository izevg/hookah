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

    fn hookah(&self, args: &[&str]) -> Output {
        Command::new(HOOKAH)
            .args(args)
            .current_dir(self.root.join("project"))
            .env("HOME", self.root.join("home"))
            .env("USERPROFILE", self.root.join("home"))
            .env_remove("XDG_CONFIG_HOME")
            .env("PATH", self.root.join("bin"))
            .output()
            .unwrap()
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
    for harness in ["claude", "codex", "qoder", "opencode"] {
        sb.ok(&["setup", harness]);
    }
    sb.ok(&["setup", "kimi"]);
    let files = [
        sb.project(".claude/settings.json"),
        sb.project(".codex/config.toml"),
        sb.project(".qoder/settings.json"),
        sb.project(".opencode/plugins/hookah.ts"),
        sb.home(".kimi-code/config.toml"),
    ];
    let before: Vec<_> = files.iter().map(|f| fs::read(f).unwrap()).collect();
    for harness in ["claude", "codex", "qoder", "opencode", "kimi"] {
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
    for harness in ["claude", "codex", "qoder", "kimi", "opencode"] {
        sb.ok(&["setup", harness, "--scope", "user"]);
    }
    assert!(text(&sb.home(".claude/settings.json")).contains("hook claude"));
    assert!(text(&sb.home(".codex/config.toml")).contains("hook codex"));
    assert!(text(&sb.home(".qoder/settings.json")).contains("hook qoder"));
    assert!(text(&sb.home(".kimi-code/config.toml")).contains("hook kimi"));
    assert!(sb.home(".config/opencode/plugins/hookah.ts").is_file());
    assert!(!sb.project(".claude").exists());
}

#[test]
fn opencode_honors_xdg_config_home() {
    let sb = Sandbox::new("xdg");
    let xdg = sb.root.join("xdg");
    let out = Command::new(HOOKAH)
        .args(["setup", "opencode", "--scope", "user"])
        .current_dir(sb.root.join("project"))
        .env("HOME", sb.root.join("home"))
        .env("XDG_CONFIG_HOME", &xdg)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(xdg.join("opencode/plugins/hookah.ts").is_file());
}

#[test]
fn the_opencode_plugin_matches_hookah_plugin_opencode() {
    let sb = Sandbox::new("plugin");
    sb.ok(&["setup", "opencode"]);
    let printed = sb.ok(&["plugin", "opencode"]);
    assert_eq!(text(&sb.project(".opencode/plugins/hookah.ts")), printed);
    assert_eq!(sb.ok(&["setup", "opencode", "--print"]), printed);
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
}

#[test]
fn detect_reports_installed_and_configured_harnesses() {
    let sb = Sandbox::new("detect");
    fs::write(sb.root.join("bin/claude"), "").unwrap();
    fs::create_dir_all(sb.home(".codex")).unwrap();
    sb.ok(&["setup", "claude"]);
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
