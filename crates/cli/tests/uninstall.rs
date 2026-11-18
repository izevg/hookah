//! End-to-end tests of `hookah uninstall`. Every run gets a private project directory, home,
//! `PATH`, and cache, so nothing here can touch the real user config or cache.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const HOOKAH: &str = env!("CARGO_BIN_EXE_hookah");

/// The harnesses, where `hookah setup` writes by default, and an unrelated config of the user's
/// in the form setup writes back byte for byte (`None`: the harness has no merged config).
struct Case {
    harness: &'static str,
    scope: &'static str,
    /// The config file, relative to the project (`project`) or the home (`user`).
    file: &'static str,
    original: Option<&'static str>,
}

const CASES: &[Case] = &[
    Case {
        harness: "claude",
        scope: "project",
        file: ".claude/settings.json",
        original: Some(
            "{\n  \"model\": \"x\",\n  \"hooks\": {\n    \"Stop\": [\n      {\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"mine.sh\"\n          }\n        ]\n      }\n    ]\n  }\n}\n",
        ),
    },
    Case {
        harness: "qoder",
        scope: "project",
        file: ".qoder/settings.json",
        original: Some(
            "{\n  \"hooks\": {\n    \"PostToolUse\": [\n      {\n        \"matcher\": \"Bash\",\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"lint.sh\"\n          }\n        ]\n      }\n    ]\n  },\n  \"zeta\": [\n    1\n  ]\n}\n",
        ),
    },
    Case {
        harness: "cursor",
        scope: "project",
        file: ".cursor/hooks.json",
        original: Some(
            "{\n  \"version\": 1,\n  \"hooks\": {\n    \"stop\": [\n      {\n        \"command\": \"mine.sh\"\n      }\n    ]\n  }\n}\n",
        ),
    },
    Case {
        harness: "codex",
        scope: "project",
        file: ".codex/config.toml",
        original: Some(
            "# mine\nmodel = \"gpt\"   # inline\n\n[[hooks.Stop]]\n\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"mine.sh\"\n",
        ),
    },
    Case {
        harness: "kimi",
        scope: "user",
        file: ".kimi-code/config.toml",
        original: Some(
            "# mine\nmodel = \"k\"\n\n[[hooks]]\nevent = \"Stop\"\ncommand = \"mine.sh\"\n",
        ),
    },
    Case {
        harness: "hermes",
        scope: "user",
        file: ".hermes/config.yaml",
        original: Some("# mine\nmodel: x\nplugins:\n  enabled:\n    - kanban  # keep\n"),
    },
    Case {
        harness: "opencode",
        scope: "project",
        file: ".opencode/plugins/hookah.ts",
        original: None,
    },
    Case {
        harness: "pi",
        scope: "project",
        file: ".pi/extensions/hookah.ts",
        original: None,
    },
    Case {
        harness: "openclaw",
        scope: "user",
        file: ".openclaw/extensions/hookah/index.js",
        original: None,
    },
];

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "uninstall-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        for dir in ["project", "home", "bin", "cache"] {
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

    fn file(&self, case: &Case) -> PathBuf {
        match case.scope {
            "project" => self.project(case.file),
            _ => self.home(case.file),
        }
    }

    /// A `hookah` command whose every config location and its cache are inside the sandbox.
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
            .env("HOOKAH_CACHE_DIR", self.root.join("cache"))
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
            "{args:?} failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// Every file and directory below the sandbox, with the bytes of the files, minus the
    /// backups setup leaves (which uninstall keeps on purpose).
    fn snapshot(&self) -> Vec<(String, Option<Vec<u8>>)> {
        let mut found = Vec::new();
        walk(&self.root, &self.root, &mut found);
        found.retain(|(path, _)| !path.ends_with(".hookah.bak"));
        found.sort();
        found
    }
}

fn walk(root: &Path, dir: &Path, found: &mut Vec<(String, Option<Vec<u8>>)>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if path.is_dir() {
            found.push((rel, None));
            walk(root, &path, found);
        } else {
            found.push((rel, Some(fs::read(&path).unwrap())));
        }
    }
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn setup_then_uninstall_leaves_no_trace_when_setup_created_everything() {
    for case in CASES {
        let sb = Sandbox::new(case.harness);
        let before = sb.snapshot();
        sb.ok(&["setup", case.harness]);
        assert_ne!(before, sb.snapshot(), "{}", case.harness);
        let out = sb.ok(&["uninstall", case.harness]);
        assert!(
            out.starts_with(&format!(
                "hookah uninstall: {}, {} scope\n",
                case.harness, case.scope
            )),
            "{out}"
        );
        assert!(out.contains("next:"), "{out}");
        assert_eq!(before, sb.snapshot(), "{}: {out}", case.harness);
    }
}

#[test]
fn setup_then_uninstall_restores_a_preexisting_config_byte_for_byte() {
    for case in CASES {
        let sb = Sandbox::new(case.harness);
        // The user's own files: the config where there is one, a neighbor of the plugin otherwise.
        match case.original {
            Some(text) => write(&sb.file(case), text),
            None => {
                let neighbor = sb.file(case).parent().unwrap().join("mine.txt");
                write(&neighbor, "mine");
            }
        }
        let before = sb.snapshot();
        sb.ok(&["setup", case.harness]);
        let out = sb.ok(&["uninstall", case.harness]);
        assert_eq!(before, sb.snapshot(), "{}: {out}", case.harness);
        if let Some(original) = case.original {
            // Setup's backup stays, and the output says so.
            let backup = format!("{}.hookah.bak", sb.file(case).display());
            assert_eq!(fs::read_to_string(&backup).unwrap(), original);
            assert!(out.contains(".hookah.bak"), "{out}");
            assert!(out.contains("never restores"), "{out}");
        }
    }
}

#[test]
fn a_hookah_hook_in_a_shared_group_goes_and_its_neighbor_stays() {
    let sb = Sandbox::new("shared");
    let file = sb.project(".claude/settings.json");
    sb.ok(&["setup", "claude"]);
    // The user adds a hook to the group hookah created, and an unrelated key.
    let text = fs::read_to_string(&file)
        .unwrap()
        .replace("\"hooks\": {", "\"zzz\": true,\n  \"hooks\": {");
    let mut json: serde_json::Value = serde_json::from_str(&text).unwrap();
    json["hooks"]["Stop"][0]["hooks"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"type": "command", "command": "mine.sh"}));
    fs::write(&file, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    let out = sb.ok(&["uninstall", "claude"]);
    assert!(
        out.contains("removed hookah's entries from .claude/settings.json"),
        "{out}"
    );
    let left = fs::read_to_string(&file).unwrap();
    assert!(
        !left.contains("hook claude") && !left.contains("hookah"),
        "{left}"
    );
    let json: serde_json::Value = serde_json::from_str(&left).unwrap();
    assert_eq!(json["zzz"], true);
    assert_eq!(json["hooks"]["Stop"][0]["hooks"][0]["command"], "mine.sh");
    assert!(json["hooks"].get("PostToolUse").is_none());
}

#[test]
fn a_modified_plugin_is_refused_with_the_reason_and_force_removes_it() {
    for harness in ["opencode", "pi"] {
        let sb = Sandbox::new(harness);
        sb.ok(&["setup", harness]);
        let case = CASES.iter().find(|c| c.harness == harness).unwrap();
        let file = sb.file(case);
        fs::write(&file, "// my own version\n").unwrap();
        let out = sb.hookah(&["uninstall", harness]);
        assert_eq!(out.status.code(), Some(1), "{harness}");
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(
            text.contains("kept ") && text.contains("differs from the plugin"),
            "{text}"
        );
        assert!(!text.contains("next:"), "{text}");
        assert_eq!(fs::read_to_string(&file).unwrap(), "// my own version\n");
        let forced = sb.ok(&["uninstall", harness, "--force"]);
        assert!(forced.contains("deleted "), "{forced}");
        assert!(!file.exists());
    }
}

#[test]
fn a_config_that_cannot_be_edited_safely_is_kept_and_reported() {
    let sb = Sandbox::new("badconfig");
    let file = sb.project(".claude/settings.json");
    write(&file, "{ nope");
    let out = sb.hookah(&["uninstall", "claude"]);
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("not valid JSON") && text.contains("by hand"),
        "{text}"
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "{ nope");
}

#[test]
fn hermes_keeps_its_plugin_while_the_enabled_list_cannot_be_edited() {
    let sb = Sandbox::new("hermes-inline");
    write(
        &sb.home(".hermes/config.yaml"),
        "plugins:\n  enabled: [a, hookah]\n",
    );
    write(&sb.home(".hermes/plugins/hookah/__init__.py"), "x");
    let out = sb.hookah(&["uninstall", "hermes"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(sb.home(".hermes/plugins/hookah/__init__.py").is_file());
}

#[test]
fn uninstalling_twice_changes_nothing_the_second_time_and_exits_zero() {
    for case in CASES {
        let sb = Sandbox::new(case.harness);
        sb.ok(&["setup", case.harness]);
        sb.ok(&["uninstall", case.harness]);
        let after_first = sb.snapshot();
        let out = sb.ok(&["uninstall", case.harness]);
        assert!(
            out.contains(&format!(
                "nothing to remove: hookah is not set up for {} at {} scope\n",
                case.harness, case.scope
            )),
            "{out}"
        );
        assert!(!out.contains("next:"), "{out}");
        assert_eq!(after_first, sb.snapshot());
    }
}

#[test]
fn uninstalling_where_nothing_is_installed_is_a_clean_no_op() {
    let sb = Sandbox::new("nothing");
    let before = sb.snapshot();
    let out = sb.ok(&["uninstall", "--all"]);
    assert!(out.contains("nothing to remove"), "{out}");
    assert_eq!(before, sb.snapshot());
    sb.ok(&["uninstall", "orca"]);
}

#[test]
fn dry_run_writes_nothing_and_says_what_would_happen() {
    let sb = Sandbox::new("dry");
    for case in CASES {
        if let Some(text) = case.original {
            write(&sb.file(case), text);
        }
        sb.ok(&["setup", case.harness]);
    }
    let before = sb.snapshot();
    let out = sb.ok(&["uninstall", "--all", "--dry-run", "--purge-cache"]);
    assert!(out.contains("(dry run, nothing written)"), "{out}");
    assert!(out.contains("would remove hookah's entries from"), "{out}");
    assert!(out.contains("would delete "), "{out}");
    assert!(
        !out.contains("\ndeleted ") && !out.contains("\nremoved "),
        "{out}"
    );
    assert_eq!(before, sb.snapshot());
}

#[test]
fn all_walks_every_configured_harness_at_both_scopes_or_just_the_one_asked_for() {
    let sb = Sandbox::new("all");
    let before = sb.snapshot();
    for case in CASES {
        sb.ok(&["setup", case.harness]);
    }
    sb.ok(&["setup", "claude", "--scope", "user"]);
    // Project scope only: the user-level entries stay.
    let out = sb.ok(&["uninstall", "--all", "--scope", "project"]);
    assert!(
        out.contains("hookah uninstall: claude, project scope\n"),
        "{out}"
    );
    assert!(!out.contains("user scope"), "{out}");
    assert!(sb.home(".claude/settings.json").is_file());
    assert!(sb.home(".kimi-code/config.toml").is_file());
    assert!(!sb.project(".claude").exists());
    let out = sb.ok(&["uninstall", "--all"]);
    assert!(
        out.contains("hookah uninstall: claude, user scope\n"),
        "{out}"
    );
    assert_eq!(before, sb.snapshot());
    // Both scopes of one harness are independent.
    assert!(sb.ok(&["uninstall", "--all"]).contains("nothing to remove"));
}

#[test]
fn purge_cache_deletes_hookahs_cache_only_and_honors_the_override() {
    let sb = Sandbox::new("purge");
    let cache = sb.root.join("cache");
    write(&cache.join("0123456789abcdef/baselines/x"), "b");
    write(&cache.join("fedcba9876543210/status"), "s");
    write(&cache.join("notes.txt"), "mine");
    let out = sb.ok(&["uninstall", "--purge-cache", "--dry-run"]);
    assert!(out.contains("cache: would delete"), "{out}");
    assert!(cache.join("0123456789abcdef").is_dir());
    let out = sb.ok(&["uninstall", "--purge-cache"]);
    assert!(
        out.contains("cache: deleted") && out.contains("0123456789abcdef"),
        "{out}"
    );
    assert!(!cache.join("0123456789abcdef").exists());
    assert!(!cache.join("fedcba9876543210").exists());
    // Whatever else lives in a directory the user chose stays.
    assert_eq!(fs::read_to_string(cache.join("notes.txt")).unwrap(), "mine");
    let out = sb.ok(&["uninstall", "--purge-cache"]);
    assert!(out.contains("cache: nothing to delete"), "{out}");
    // Combined with a harness.
    sb.ok(&["setup", "claude"]);
    let out = sb.ok(&["uninstall", "claude", "--purge-cache"]);
    assert!(
        out.contains("deleted .claude/settings.json") && out.contains("cache:"),
        "{out}"
    );
}

#[test]
fn the_orca_plugin_folder_is_removed_when_untouched_and_kept_when_edited() {
    let sb = Sandbox::new("orca");
    let dir = sb.home(".hookah-orca-plugin");
    sb.ok(&["plugin", "orca", "--out", dir.to_str().unwrap()]);
    let out = sb.ok(&["uninstall", "orca", "--dry-run"]);
    assert!(
        out.contains("would delete") && dir.join("orca-plugin.json").is_file(),
        "{out}"
    );
    let out = sb.ok(&["uninstall", "orca"]);
    assert!(out.starts_with("hookah uninstall: orca\n"), "{out}");
    assert!(out.contains("orca plugin uninstall hookah"), "{out}");
    assert!(!dir.exists());
    // A named folder, with one edited file and one stranger.
    let other = sb.root.join("elsewhere");
    sb.ok(&["plugin", "orca", "--out", other.to_str().unwrap()]);
    fs::write(other.join("src/worker.ts"), "// mine").unwrap();
    write(&other.join("notes.txt"), "mine");
    let out = sb.hookah(&["uninstall", "orca", "--dir", other.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(other.join("src/worker.ts").is_file() && other.join("notes.txt").is_file());
    assert!(!other.join("orca-plugin.json").exists());
}

#[test]
fn openclaw_prints_the_disable_command_instead_of_editing_its_config() {
    let sb = Sandbox::new("openclaw");
    sb.ok(&["setup", "openclaw"]);
    let config = sb.home(".openclaw/openclaw.json5");
    write(&config, "{ plugins: { entries: { hookah: {} } } }\n");
    let out = sb.ok(&["uninstall", "openclaw"]);
    assert!(out.contains("openclaw plugins disable hookah"), "{out}");
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        "{ plugins: { entries: { hookah: {} } } }\n"
    );
}

#[test]
fn inconsistent_arguments_fail_before_anything_is_touched() {
    let sb = Sandbox::new("args");
    sb.ok(&["setup", "claude"]);
    let before = sb.snapshot();
    for args in [
        &["uninstall", "orca", "--scope", "user"][..],
        &["uninstall", "claude", "--dir", "x"],
        &["uninstall", "kimi", "--scope", "project"],
        &["uninstall"],
        &["uninstall", "claude", "--all"],
    ] {
        let out = sb.hookah(args);
        assert!(!out.status.success(), "{args:?}");
    }
    assert_eq!(before, sb.snapshot());
}

/// The sandbox is the only place `hookah uninstall` may touch, for every harness at every scope,
/// with the cache purged too: each path it reports is inside it, and the cache it deletes is the
/// one the environment names.
#[test]
fn uninstall_never_touches_anything_outside_the_sandbox() {
    let sb = Sandbox::new("guard");
    let canary = sb
        .root
        .parent()
        .unwrap()
        .join(format!("uninstall-canary-{}", std::process::id()));
    write(&canary.join("kept"), "x");
    let root = fs::canonicalize(&sb.root).unwrap();
    let inside = |reported: &str| {
        let path = sb.root.join("project").join(reported);
        let path = fs::canonicalize(&path).unwrap_or(path);
        path.starts_with(&root)
    };
    for harness in [
        "claude", "codex", "qoder", "kimi", "cursor", "opencode", "hermes", "pi", "openclaw",
    ] {
        for scope in ["user", "project"] {
            sb.hookah(&["setup", harness, "--scope", scope]);
            for extra in [&["--dry-run"][..], &[]] {
                let mut args = vec!["uninstall", harness, "--scope", scope, "--purge-cache"];
                args.extend_from_slice(extra);
                let out = sb.hookah(&args);
                let stdout = String::from_utf8(out.stdout).unwrap();
                for line in stdout.lines() {
                    let reported = [
                        "deleted ",
                        "would delete ",
                        "removed hookah's entries from ",
                        "would remove hookah's entries from ",
                        "cache: deleted ",
                        "cache: would delete ",
                    ]
                    .iter()
                    .find_map(|verb| line.strip_prefix(verb))
                    .or_else(|| {
                        line.strip_prefix("kept ")
                            .and_then(|rest| rest.split(": ").next())
                    });
                    if let Some(path) = reported {
                        assert!(inside(path), "{harness} --scope {scope}: {line}");
                    }
                }
            }
        }
    }
    let out = sb.ok(&["uninstall", "--all", "--purge-cache"]);
    assert!(
        !out.contains(".cache") || out.contains("uninstall-guard"),
        "{out}"
    );
    assert_eq!(fs::read_to_string(canary.join("kept")).unwrap(), "x");
    let _ = fs::remove_dir_all(canary);
}
