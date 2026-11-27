//! End-to-end tests: the built binary against throwaway Rust crates, each in its own git repo.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

const CLEAN_LIB: &str = "pub fn answer() -> i32 {\n    42\n}\n";

/// A temp crate with its own git repository and one commit.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    /// Commits a crate whose `src/lib.rs` is `lib`.
    fn new(name: &str, lib: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "e2e-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        // The empty `[workspace]` keeps cargo from adopting the enclosing hookah workspace.
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
        )
        .unwrap();
        fs::write(root.join(".gitignore"), "target\n").unwrap();
        let fixture = Self { root };
        fixture.write("src/lib.rs", lib);
        fixture.git(&["init", "-q"]);
        fixture.git(&["add", "-A"]);
        fixture.git(&["commit", "-q", "-m", "init"]);
        fixture
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn write(&self, rel: &str, content: &str) {
        fs::write(self.path(rel), content).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.path(rel)).unwrap()
    }

    fn git(&self, args: &[&str]) {
        let status = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} failed");
    }

    /// Where `hookah` keeps its cache for this fixture: next to the crate, not inside it.
    fn cache(&self) -> PathBuf {
        let mut name = self.root.file_name().unwrap().to_os_string();
        name.push("-cache");
        self.root.with_file_name(name)
    }

    /// Runs `hookah` in the crate root; a baseline job runs to completion before it returns.
    fn hookah(&self, args: &[&str], stdin: &str) -> Output {
        self.hookah_with(args, stdin, &[])
    }

    /// Runs `hookah` in the crate root with extra environment variables, which override the
    /// defaults. One cache directory per fixture serves the detection cache and the baselines, so
    /// nothing reaches the user's cache. A baseline job runs to completion before `hookah`
    /// returns, unless `HOOKAH_BASELINE_FOREGROUND` is overridden with an empty value.
    fn hookah_with(&self, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hookah"))
            .args(args)
            .current_dir(&self.root)
            .env("HOOKAH_CACHE_DIR", self.cache())
            .env("HOOKAH_BASELINE_FOREGROUND", "1")
            // Never the developer's own user config or model override; a test overrides explicitly.
            .env("HOOKAH_CONFIG", self.cache().join("no-user-config.toml"))
            .env_remove("HOOKAH_MODELS")
            .envs(env.iter().copied())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn hookah");
        let _ = child
            .stdin
            .take()
            .expect("stdin")
            .write_all(stdin.as_bytes());
        child.wait_with_output().expect("wait")
    }

    /// An edit event of a session-less harness payload: no baseline is involved.
    fn edit_payload(&self, rel: &str) -> String {
        format!(
            r#"{{"cwd":{cwd},"hook_event_name":"PostToolUse","tool_name":"Edit","tool_input":{{"file_path":{file}}}}}"#,
            cwd = json_str(&self.root),
            file = json_str(&self.path(rel)),
        )
    }

    fn session_edit_payload(&self, rel: &str, session: &str) -> String {
        format!(
            r#"{{"session_id":"{session}","cwd":{cwd},"hook_event_name":"PostToolUse","tool_name":"Edit","tool_input":{{"file_path":{file}}}}}"#,
            cwd = json_str(&self.root),
            file = json_str(&self.path(rel)),
        )
    }

    fn start_payload(&self, session: &str) -> String {
        format!(
            r#"{{"session_id":"{session}","cwd":{cwd},"hook_event_name":"SessionStart","source":"startup"}}"#,
            cwd = json_str(&self.root),
        )
    }

    fn stop_payload(&self, active: bool) -> String {
        format!(
            r#"{{"cwd":{cwd},"hook_event_name":"Stop","stop_hook_active":{active}}}"#,
            cwd = json_str(&self.root),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
        let _ = fs::remove_dir_all(self.cache());
    }
}

/// A path as a JSON string; temp paths hold no control characters.
fn json_str(path: &Path) -> String {
    let s = path.to_string_lossy();
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn check_passes_after_a_harmless_edit() {
    let f = Fixture::new("harmless", CLEAN_LIB);
    f.write("src/lib.rs", "pub fn answer() -> i32 {\n    41 + 1\n}\n");
    let out = f.hookah(&["check", "src/lib.rs"], "");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(stdout(&out).starts_with("ok "), "{}", stdout(&out));
}

#[test]
fn check_fails_on_a_compile_error() {
    let f = Fixture::new("broken", CLEAN_LIB);
    f.write("src/lib.rs", "pub fn answer() -> i32 {\n    \"nope\"\n}\n");
    let out = f.hookah(&["check", "src/lib.rs"], "");
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(stdout(&out).contains("src/lib.rs:2"), "{}", stdout(&out));
}

#[test]
fn check_reports_a_new_warning_on_a_changed_line() {
    let f = Fixture::new("newwarn", CLEAN_LIB);
    f.write(
        "src/lib.rs",
        "pub fn answer() -> i32 {\n    return 42;\n}\n",
    );
    let out = f.hookah(&["check", "src/lib.rs"], "");
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(stdout(&out).contains("src/lib.rs:2"), "{}", stdout(&out));
}

#[test]
fn check_ignores_a_warning_on_an_untouched_line() {
    let lib = "pub fn old() -> i32 {\n    return 1;\n}\n\npub fn fresh() -> i32 {\n    2\n}\n";
    let f = Fixture::new("oldwarn", lib);
    f.write("src/lib.rs", &lib.replace("    2\n", "    3\n"));
    let out = f.hookah(&["check", "src/lib.rs"], "");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(!stdout(&out).contains("src/lib.rs:2"), "{}", stdout(&out));
}

#[test]
fn claude_edit_hook_blocks_on_a_broken_file() {
    let f = Fixture::new("hookbroken", CLEAN_LIB);
    f.write("src/lib.rs", "pub fn answer() -> i32 {\n    \"nope\"\n}\n");
    let out = f.hookah(&["hook", "claude"], &f.edit_payload("src/lib.rs"));
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let err = stderr(&out);
    assert!(err.starts_with("hookah: "), "{err}");
    assert!(err.contains("Fix before continuing."), "{err}");
    assert!(err.contains("src/lib.rs:2"), "{err}");
}

#[test]
fn claude_edit_hook_reports_a_formatter_rewrite() {
    let f = Fixture::new("hookfmt", CLEAN_LIB);
    f.write(
        "src/lib.rs",
        "pub fn answer() -> i32 {\n            42\n}\n",
    );
    let out = f.hookah(&["hook", "claude"], &f.edit_payload("src/lib.rs"));
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let json = stdout(&out);
    assert!(json.contains("additionalContext"), "{json}");
    assert!(json.to_lowercase().contains("re-read"), "{json}");
    assert_eq!(f.read("src/lib.rs"), CLEAN_LIB);
}

const FAILING_TEST_LIB: &str = "pub fn answer() -> i32 {\n    42\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn answers() {\n        assert_eq!(super::answer(), 41);\n    }\n}\n";

#[test]
fn claude_stop_hook_blocks_on_a_failing_test() {
    let f = Fixture::new("stopfail", CLEAN_LIB);
    f.write("src/lib.rs", FAILING_TEST_LIB);
    let out = f.hookah(&["hook", "claude"], &f.stop_payload(false));
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
}

#[test]
fn a_zero_test_budget_from_the_environment_runs_no_tests_at_stop() {
    let f = Fixture::new("stopbudget", CLEAN_LIB);
    f.write("src/lib.rs", FAILING_TEST_LIB);
    let env = [("HOOKAH_TEST_BUDGET", "0")];
    let out = f.hookah_with(&["hook", "claude"], &f.stop_payload(false), &env);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let out = f.hookah_with(&["check", "--changed"], "", &env);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    // A value that is not a number keeps the default budget, so the failing test is found.
    let env = [("HOOKAH_TEST_BUDGET", "lots")];
    let out = f.hookah_with(&["check", "--changed"], "", &env);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
}

#[test]
fn claude_stop_hook_never_blocks_twice() {
    let f = Fixture::new("stopactive", CLEAN_LIB);
    f.write("src/lib.rs", FAILING_TEST_LIB);
    let out = f.hookah(&["hook", "claude"], &f.stop_payload(true));
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(stdout(&out).contains("systemMessage"), "{}", stdout(&out));
}

#[test]
fn cursor_edit_hook_reports_to_the_user_and_never_blocks() {
    let f = Fixture::new("cursoredit", CLEAN_LIB);
    f.write("src/lib.rs", "pub fn answer() -> i32 {\n    \"nope\"\n}\n");
    let payload = format!(
        r#"{{"conversation_id":"c1","hook_event_name":"afterFileEdit","workspace_roots":[{root}],"file_path":{file},"edits":[]}}"#,
        root = json_str(&f.root),
        file = json_str(&f.path("src/lib.rs")),
    );
    let out = f.hookah(&["hook", "cursor"], &payload);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
    let err = stderr(&out);
    assert!(err.contains("after editing"), "{err}");
    assert!(err.contains("src/lib.rs:2"), "{err}");
}

#[test]
fn cursor_stop_hook_continues_the_agent_once_with_a_followup_message() {
    let f = Fixture::new("cursorstop", CLEAN_LIB);
    f.write("src/lib.rs", FAILING_TEST_LIB);
    let stop = |loop_count: u32| {
        format!(
            r#"{{"hook_event_name":"stop","workspace_roots":[{root}],"status":"completed","loop_count":{loop_count}}}"#,
            root = json_str(&f.root),
        )
    };
    let out = f.hookah(&["hook", "cursor"], &stop(0));
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let json = stdout(&out);
    assert!(
        json.starts_with("{\"followup_message\":\"hookah: "),
        "{json}"
    );
    let again = f.hookah(&["hook", "cursor"], &stop(1));
    assert_eq!(again.status.code(), Some(0));
    assert!(stdout(&again).is_empty(), "{}", stdout(&again));
    assert!(stderr(&again).contains("not asking the agent to continue again"));
}

#[test]
fn detect_prints_the_crate_and_its_tools() {
    let f = Fixture::new("detect", CLEAN_LIB);
    let out = f.hookah(&["detect", "src/lib.rs"], "");
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    assert!(text.contains("ecosystem: rust"), "{text}");
    for tool in ["rustfmt", "clippy", "cargo-test"] {
        assert!(text.contains(tool), "{text}");
    }
}

#[test]
fn detect_says_whether_the_toolchain_came_from_the_cache() {
    let f = Fixture::new("detect-cache", CLEAN_LIB);
    let cache_line = |out: &Output| {
        stdout(out)
            .lines()
            .find(|l| l.starts_with("cache: "))
            .map(str::to_owned)
            .unwrap_or_default()
    };
    let detect = |env: &[(&str, &str)]| f.hookah_with(&["detect", "src/lib.rs"], "", env);
    assert_eq!(cache_line(&detect(&[])), "cache: miss");
    let warm = detect(&[]);
    assert_eq!(cache_line(&warm), "cache: hit");
    assert!(stdout(&warm).contains("clippy"));
    assert_eq!(
        cache_line(&detect(&[("HOOKAH_NO_CACHE", "1")])),
        "cache: off"
    );
    // Editing the manifest invalidates the entry.
    f.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.2.0\"\nedition = \"2024\"\n\n[workspace]\n",
    );
    assert_eq!(cache_line(&detect(&[])), "cache: miss");
    assert_eq!(cache_line(&detect(&[])), "cache: hit");
}

/// A crate whose untouched `src/other.rs` already has a compile error (a clippy error).
const OLD_ERROR_LIB: &str = "mod other;\n\npub fn answer() -> i32 {\n    42\n}\n";
const OLD_ERROR_OTHER: &str = "pub fn broken() -> i32 {\n    \"old\"\n}\n";

fn old_error_fixture(name: &str) -> Fixture {
    let f = Fixture::new(name, OLD_ERROR_LIB);
    f.write("src/other.rs", OLD_ERROR_OTHER);
    f.git(&["add", "-A"]);
    f.git(&["commit", "-q", "-m", "pre-existing error"]);
    f
}

#[test]
fn without_a_baseline_a_pre_existing_error_in_another_file_is_reported() {
    let f = old_error_fixture("nobase");
    f.write("src/lib.rs", &OLD_ERROR_LIB.replace("42", "43"));
    let out = f.hookah(&["hook", "claude"], &f.edit_payload("src/lib.rs"));
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(stderr(&out).contains("src/other.rs:2"), "{}", stderr(&out));
}

#[test]
fn a_session_baseline_hides_old_errors_and_shows_new_ones() {
    let f = old_error_fixture("baseline");
    let start = f.hookah(&["hook", "claude"], &f.start_payload("sess-1"));
    assert_eq!(start.status.code(), Some(0), "{}", stderr(&start));
    assert!(start.stdout.is_empty() && start.stderr.is_empty());
    let records = fs::read_dir(f.cache())
        .unwrap()
        .flat_map(|dir| fs::read_dir(dir.unwrap().path().join("baselines")).unwrap())
        .count();
    assert_eq!(records, 1);

    // An edit elsewhere: the old error in the untouched file stays quiet.
    f.write("src/lib.rs", &OLD_ERROR_LIB.replace("42", "43"));
    let edit = f.session_edit_payload("src/lib.rs", "sess-1");
    let out = f.hookah(&["hook", "claude"], &edit);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(out.stdout.is_empty() && out.stderr.is_empty());

    // A newly introduced error is shown, and still not the old one.
    f.write("src/lib.rs", &OLD_ERROR_LIB.replace("42", "\"new\""));
    let out = f.hookah(&["hook", "claude"], &edit);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let err = stderr(&out);
    assert!(err.contains("src/lib.rs:4"), "{err}");
    assert!(!err.contains("src/other.rs"), "{err}");

    // Another session has no baseline and gets the fallback: the old error shows up again. Its
    // first edit starts the job lazily (inline here), which does not filter that edit.
    let other = f.session_edit_payload("src/lib.rs", "sess-2");
    let out = f.hookah(&["hook", "claude"], &other);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("src/other.rs:2"), "{}", stderr(&out));
}

#[test]
fn a_lazy_baseline_from_the_first_edit_filters_the_next_one() {
    let f = old_error_fixture("lazy");
    f.write("src/lib.rs", &OLD_ERROR_LIB.replace("42", "43"));
    let edit = f.session_edit_payload("src/lib.rs", "sess-lazy");
    let first = f.hookah(&["hook", "claude"], &edit);
    assert_eq!(first.status.code(), Some(2));
    let second = f.hookah(&["hook", "claude"], &edit);
    assert_eq!(second.status.code(), Some(0), "{}", stderr(&second));
}

#[test]
fn the_background_job_outlives_the_hook_and_finishes_the_baseline() {
    let f = old_error_fixture("background");
    let env: &[(&str, &str)] = &[("HOOKAH_BASELINE_FOREGROUND", "")];
    let start = f.hookah_with(&["hook", "claude"], &f.start_payload("sess-bg"), env);
    assert_eq!(start.status.code(), Some(0));
    let edit = f.session_edit_payload("src/lib.rs", "sess-bg");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    loop {
        let out = f.hookah_with(&["hook", "claude"], &edit, env);
        if out.status.code() == Some(0) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "no baseline in time");
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

#[test]
fn the_hidden_baseline_command_records_a_failure_for_a_missing_project() {
    let f = Fixture::new("nobaseproj", CLEAN_LIB);
    let empty = std::env::temp_dir().join(format!("hookah-noproj-{}", std::process::id()));
    fs::create_dir_all(&empty).unwrap();
    let out = f.hookah(
        &["baseline", "--session=x", "--root", empty.to_str().unwrap()],
        "",
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
    let record = fs::read_dir(f.cache())
        .unwrap()
        .map(|dir| dir.unwrap().path().join("baselines/x.json"))
        .find(|p| p.exists())
        .expect("a record");
    let text = fs::read_to_string(record).unwrap();
    assert!(
        text.contains("\"failed\"") && text.contains("no project"),
        "{text}"
    );
    let _ = fs::remove_dir_all(&empty);
}

fn status_doc(f: &Fixture) -> serde_json::Value {
    let out = f.hookah(&["status", "--json"], "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    serde_json::from_str(&stdout(&out)).unwrap()
}

#[test]
fn status_follows_the_latest_check_of_the_project() {
    let f = Fixture::new("status", CLEAN_LIB);
    let doc = status_doc(&f);
    assert_eq!(doc["v"], 1);
    assert_eq!(doc["overall"], serde_json::Value::Null);
    assert_eq!(doc["projects"].as_array().unwrap().len(), 1);
    assert_eq!(doc["projects"][0]["status"], serde_json::Value::Null);

    f.write("src/lib.rs", "pub fn answer() -> i32 {\n    \"nope\"\n}\n");
    assert_eq!(
        f.hookah(&["check", "src/lib.rs"], "").status.code(),
        Some(1)
    );
    let doc = status_doc(&f);
    assert_eq!(doc["overall"], "fail");
    let status = &doc["projects"][0]["status"];
    assert_eq!(status["phase"], "edit");
    assert!(status["problems"].as_u64().unwrap() >= 1);
    assert!(
        status["lines"][0]
            .as_str()
            .unwrap()
            .contains("src/lib.rs:2"),
        "{status}"
    );
    assert!(status["at_ms"].as_u64().unwrap() > 1_600_000_000_000);

    f.write("src/lib.rs", CLEAN_LIB);
    assert_eq!(
        f.hookah(&["check", "src/lib.rs"], "").status.code(),
        Some(0)
    );
    assert_eq!(status_doc(&f)["overall"], "pass");
}

#[test]
fn status_in_text_form_and_from_a_subdirectory() {
    let f = Fixture::new("statustext", CLEAN_LIB);
    f.write("src/lib.rs", "pub fn answer() -> i32 {\n    \"nope\"\n}\n");
    f.hookah(&["check", "src/lib.rs"], "");
    let out = f.hookah(&["status", "--root", "src"], "");
    let text = stdout(&out);
    assert!(
        text.contains("problem") && text.contains("after the last edit check"),
        "{text}"
    );
    assert!(text.contains("src/lib.rs:2"), "{text}");
}

#[test]
fn check_changed_runs_the_stop_phase_and_records_it() {
    let f = Fixture::new("changed", CLEAN_LIB);
    f.write("src/lib.rs", "pub fn answer() -> i32 {\n    \"nope\"\n}\n");
    let out = f.hookah(&["check", "--changed"], "");
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(stdout(&out).starts_with("FAIL "), "{}", stdout(&out));
    assert!(stdout(&out).contains("src/lib.rs:2"), "{}", stdout(&out));
    let doc = status_doc(&f);
    assert_eq!(doc["projects"][0]["status"]["phase"], "stop");
    assert_eq!(doc["overall"], "fail");

    // Nothing changed any more: the clean tree replaces the old failure.
    f.git(&["checkout", "--", "src/lib.rs"]);
    let out = f.hookah(&["check", "--changed", "--root", "."], "");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(stdout(&out).starts_with("ok "), "{}", stdout(&out));
    assert_eq!(status_doc(&f)["overall"], "pass");
}

#[test]
fn the_orca_plugin_is_written_to_a_folder_and_rewritten_without_changes() {
    let f = Fixture::new("orcaplugin", CLEAN_LIB);
    let out = f.hookah(&["plugin", "orca"], "");
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("--out"), "{}", stderr(&out));

    let first = f.hookah(&["plugin", "orca", "--out", "orca-plugin"], "");
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    assert!(stdout(&first).contains("wrote "), "{}", stdout(&first));
    let manifest: serde_json::Value =
        serde_json::from_str(&f.read("orca-plugin/orca-plugin.json")).unwrap();
    assert_eq!(manifest["id"], "hookah");
    assert!(f.path("orca-plugin/src/worker.ts").is_file());
    assert!(f.path("orca-plugin/src/panel.tsx").is_file());

    let second = f.hookah(&["plugin", "orca", "--out", "orca-plugin"], "");
    assert_eq!(second.status.code(), Some(0));
    assert!(!stdout(&second).contains("wrote "), "{}", stdout(&second));
    assert!(
        stdout(&second).contains("unchanged "),
        "{}",
        stdout(&second)
    );

    let opencode = f.hookah(&["plugin", "opencode", "--out", "x"], "");
    assert_eq!(opencode.status.code(), Some(1));
}

#[test]
#[cfg(unix)]
fn status_and_check_changed_resolve_an_aliased_root() {
    let f = Fixture::new("alias", CLEAN_LIB);
    f.write("src/lib.rs", "pub fn answer() -> i32 {\n    \"nope\"\n}\n");
    f.hookah(&["check", "src/lib.rs"], "");
    let alias = f.root.with_file_name(format!(
        "{}-alias",
        f.root.file_name().unwrap().to_string_lossy()
    ));
    let _ = fs::remove_file(&alias);
    std::os::unix::fs::symlink(&f.root, &alias).unwrap();
    let out = f.hookah(&["status", "--json", "--root", alias.to_str().unwrap()], "");
    let doc: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(doc["overall"], "fail", "{doc}");
    let out = f.hookah(
        &["check", "--changed", "--root", alias.to_str().unwrap()],
        "",
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let _ = fs::remove_file(&alias);
}

/// The tool environment of a project comes from its config, not from the hook command.
const ENV_SENSITIVE_LIB: &str = "pub fn answer() -> i32 {\n    42\n}\n\npub const FLAG: [u8; 1] = [0; if option_env!(\"HOOKAH_E2E_VAR\").is_some() { 2 } else { 1 }];\n";

#[test]
fn a_local_config_file_sets_the_environment_of_the_tools() {
    let f = Fixture::new("envlocal", ENV_SENSITIVE_LIB);
    let out = f.hookah(&["check", "src/lib.rs"], "");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );

    f.write(".hookah.local.toml", "[env]\nHOOKAH_E2E_VAR = \"1\"\n");
    f.write("src/lib.rs", &format!("{ENV_SENSITIVE_LIB}// edit\n"));
    let out = f.hookah(&["check", "src/lib.rs"], "");
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
}

#[test]
fn the_user_config_reaches_the_tools_of_a_matching_project() {
    let f = Fixture::new("envuser", ENV_SENSITIVE_LIB);
    let root = f.root.to_string_lossy().replace('\\', "/");
    let user = f.cache().with_extension("user.toml");
    fs::write(
        &user,
        format!("[[project]]\npath = \"{root}\"\n[project.env]\nHOOKAH_E2E_VAR = \"1\"\n"),
    )
    .unwrap();
    f.write("src/lib.rs", &format!("{ENV_SENSITIVE_LIB}// edit\n"));
    let user = user.to_string_lossy().into_owned();
    let out = f.hookah_with(&["check", "src/lib.rs"], "", &[("HOOKAH_CONFIG", &user)]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
}

#[test]
fn a_malformed_config_is_a_warning_and_the_check_still_runs() {
    let f = Fixture::new("envbad", CLEAN_LIB);
    f.write(".hookah.toml", "[env\n");
    let out = f.hookah(&["check", "src/lib.rs"], "");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("warning: config:"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn config_prints_the_effective_environment_and_where_it_came_from() {
    let f = Fixture::new("envshow", CLEAN_LIB);
    f.write(".hookah.toml", "[env]\nA = \"{root}/a\"\nB = \"shared\"\n");
    f.write(".hookah.local.toml", "[env]\nB = \"mine\"\n");
    let out = f.hookah(&["config"], "");
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    assert!(text.contains("  B=mine  ("), "{text}");
    assert!(text.contains(".hookah.local.toml)"), "{text}");
    assert!(text.contains("  A="), "{text}");
    assert!(text.contains("/a  ("), "{text}");
    assert!(text.contains("user config: "), "{text}");
    assert!(text.contains("(not found)"), "{text}");

    let none = Fixture::new("envnone", CLEAN_LIB);
    assert!(stdout(&none.hookah(&["config", "src/lib.rs"], "")).contains("env: none"));
}

#[test]
fn zcode_apply_patch_routes_check_findings_into_post_tool_context() {
    let f = Fixture::new("zcode-edit", CLEAN_LIB);
    f.write("src/lib.rs", "pub fn answer() -> i32 {\n    \"nope\"\n}\n");
    let payload = serde_json::json!({
        "cwd": f.root,
        "hook_event_name": "PostToolUse",
        "tool_name": "ApplyPatch",
        "tool_input": {"patch_text": "*** Begin Patch\n*** Update File: src/lib.rs\n*** End Patch"}
    });

    let out = f.hookah(&["hook", "zcode"], &payload.to_string());

    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json.get("decision").is_none());
    assert!(
        json["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("src/lib.rs:2")
    );
}

#[test]
fn zcode_stop_asks_the_agent_to_fix_a_failing_test_once() {
    let f = Fixture::new("zcode-stop", CLEAN_LIB);
    f.write("src/lib.rs", FAILING_TEST_LIB);

    let out = f.hookah(&["hook", "zcode"], &f.stop_payload(false));
    let continuation = f.hookah(&["hook", "zcode"], &f.stop_payload(true));

    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["decision"], "block");
    assert!(json["reason"].as_str().unwrap().contains("tests::answers"));
    assert_eq!(continuation.status.code(), Some(0));
    assert!(continuation.stdout.is_empty());
    assert!(stderr(&continuation).contains("tests::answers"));
}

#[test]
fn antigravity_delivers_format_feedback_once_to_its_conversation() {
    let fixture = Fixture::new("antigravity-format", CLEAN_LIB);
    let file = "src/lib.rs";
    fixture.write(file, "pub fn answer()->i32{43}\n");
    let event = |session: &str, fields: serde_json::Value| {
        let mut json = serde_json::json!({
            "conversationId": session,
            "workspacePaths": [fixture.root],
            "lastUserInput": "Update the answer.",
        });
        json.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        fixture.hookah(&["hook", "antigravity"], &json.to_string())
    };
    let args = serde_json::json!({"name": "edit_file", "args": {"Path": file}});
    let edit = event("s", serde_json::json!({"stepIdx": 1, "toolCall": args}));
    assert!(edit.status.success());
    assert_eq!(stdout(&edit).trim(), "{}");
    assert_eq!(fixture.read(file), "pub fn answer() -> i32 {\n    43\n}\n");

    let before_model = serde_json::json!({"invocationNum": 1, "initialNumSteps": 2});
    let other = event("other", before_model.clone());
    assert_eq!(stdout(&other).trim(), "{}");
    let feedback = event("s", before_model.clone());
    let value: serde_json::Value = serde_json::from_str(&stdout(&feedback)).unwrap();
    let message = value["injectSteps"][0]["systemMessage"]["systemMessage"]
        .as_str()
        .unwrap();
    assert!(message.contains("src/lib.rs") && message.contains("Re-read"));
    let repeat = event("s", before_model);
    assert_eq!(stdout(&repeat).trim(), "{}");
}

#[test]
fn antigravity_only_continues_a_fully_idle_stop_once_per_prompt() {
    let fixture = Fixture::new("antigravity-stop", CLEAN_LIB);
    fixture.write("src/lib.rs", FAILING_TEST_LIB);
    let stop = |idle: bool, reason: &str, prompt: &str| {
        let payload = serde_json::json!({
            "conversationId": "stop-session",
            "workspacePaths": [fixture.root],
            "lastUserInput": prompt,
            "executionNum": 0,
            "terminationReason": reason,
            "fullyIdle": idle,
        });
        let out = fixture.hookah(&["hook", "antigravity"], &payload.to_string());
        assert!(out.status.success());
        (
            serde_json::from_str::<serde_json::Value>(&stdout(&out)).unwrap(),
            stderr(&out),
        )
    };

    assert_eq!(stop(false, "model_stop", "First").0["decision"], "allow");
    assert_eq!(stop(true, "error", "First").0["decision"], "allow");
    let (first, _) = stop(true, "NO_TOOL_CALL", "First");
    assert_eq!(first["decision"], "continue");
    assert!(first["reason"].as_str().unwrap().contains("answers"));
    let repeated = stop(true, "model_stop", "First");
    assert_eq!(repeated.0["decision"], "allow");
    assert!(repeated.1.contains("answers"));
    assert_eq!(stop(true, "model_stop", "Second").0["decision"], "continue");

    fixture.write("src/lib.rs", CLEAN_LIB);
    assert_eq!(stop(true, "model_stop", "Second").0["decision"], "allow");
    fixture.write("src/lib.rs", FAILING_TEST_LIB);
    assert_eq!(stop(true, "model_stop", "Second").0["decision"], "continue");
}

#[test]
fn antigravity_warns_and_allows_progress_when_feedback_storage_fails() {
    let fixture = Fixture::new("antigravity-cache-error", CLEAN_LIB);
    let invoke = |fields: serde_json::Value| {
        let mut payload = serde_json::json!({
            "conversationId": "cache-test", "workspacePaths": [fixture.root]
        });
        payload
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        fixture.hookah(&["hook", "antigravity"], &payload.to_string())
    };
    let edit = serde_json::json!({
        "toolCall": {"name": "edit_file", "args": {"Path": "src/lib.rs"}}
    });
    let before = serde_json::json!({"invocationNum": 1});
    fixture.write("src/lib.rs", "pub fn answer() -> i32 {\n    43\n}\n");
    assert_eq!(stdout(&invoke(edit.clone())).trim(), "{}");
    assert_eq!(stdout(&invoke(before.clone())).trim(), "{}");

    fixture.write("src/lib.rs", "pub fn answer()->i32{43}\n");
    assert_eq!(stdout(&invoke(edit.clone())).trim(), "{}");
    let cache_file = fs::read_dir(fixture.cache())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|p| p.is_dir())
        .flat_map(|p| fs::read_dir(p).unwrap().map(|entry| entry.unwrap().path()))
        .find(|p| {
            p.extension().is_some_and(|e| e == "json")
                && p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("antigravity-")
        })
        .unwrap();
    fs::remove_file(&cache_file).unwrap();
    fs::create_dir(&cache_file).unwrap();

    fixture.write("src/lib.rs", "pub fn answer()->i32{44}\n");
    for fields in [edit, before] {
        let result = invoke(fields);
        assert!(result.status.success());
        assert_eq!(stdout(&result).trim(), "{}");
        assert!(stderr(&result).contains("antigravity could not relay feedback"));
    }

    fixture.write("src/lib.rs", FAILING_TEST_LIB);
    let result = invoke(serde_json::json!({
        "fullyIdle": true, "executionNum": 0, "terminationReason": "NO_TOOL_CALL"
    }));
    assert!(result.status.success());
    assert_eq!(stdout(&result).trim(), r#"{"decision":"allow"}"#);
    assert!(stderr(&result).contains("antigravity could not relay feedback"));
}

#[test]
fn model_filter_gates_hooks_and_shows_in_config() {
    let f = Fixture::new("models", CLEAN_LIB);
    f.write("src/lib.rs", "pub fn answer() -> i32 {\n    \"nope\"\n}\n");

    // Default: hookah config shows `models: all`, and manual `hookah check` always runs.
    let cfg = stdout(&f.hookah(&["config"], ""));
    assert!(cfg.contains("models: all\n"), "{cfg}");

    f.write(
        ".hookah.toml",
        "models = [\"gpt-5*\", \"claude-sonnet-*\"]\n",
    );
    let cfg = stdout(&f.hookah(&["config"], ""));
    assert!(
        cfg.contains("models: gpt-5*, claude-sonnet-*  (") && cfg.contains(".hookah.toml)"),
        "{cfg}"
    );

    // Manual `hookah check` is never gated by `models`.
    assert_eq!(
        f.hookah(&["check", "src/lib.rs"], "").status.code(),
        Some(1)
    );

    // Codex edit hook: non-matching model is skipped silently; matching model blocks.
    let codex_payload = |model: &str| {
        serde_json::json!({
            "cwd": f.root,
            "hook_event_name": "PostToolUse",
            "model": model,
            "tool_name": "apply_patch",
            "tool_input": {"command": "*** Begin Patch\n*** Update File: src/lib.rs\n*** End Patch"}
        })
        .to_string()
    };
    let skipped = f.hookah(&["hook", "codex"], &codex_payload("o4-mini"));
    assert_eq!(skipped.status.code(), Some(0));
    assert!(skipped.stdout.is_empty() && skipped.stderr.is_empty());

    let matched = f.hookah(&["hook", "codex"], &codex_payload("gpt-5.3-codex"));
    assert_eq!(matched.status.code(), Some(0));
    let json: serde_json::Value = serde_json::from_slice(&matched.stdout).unwrap();
    assert_eq!(json["decision"], "block");

    // Claude Code edit hook without top-level `model`: reads latest model from `transcript_path`.
    let transcript = f.cache().join("claude.jsonl");
    fs::create_dir_all(f.cache()).unwrap();
    fs::write(
        &transcript,
        concat!(
            r#"{"type":"assistant","message":{"model":"claude-sonnet-4-6","content":[]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"model":"claude-opus-4-6","content":[]}}"#,
            "\n",
        ),
    )
    .unwrap();
    let claude_payload = serde_json::json!({
        "cwd": f.root,
        "hook_event_name": "PostToolUse",
        "transcript_path": transcript,
        "tool_name": "Edit",
        "tool_input": {"file_path": f.path("src/lib.rs")}
    })
    .to_string();
    let opus = f.hookah(&["hook", "claude"], &claude_payload);
    assert_eq!(opus.status.code(), Some(0));
    assert!(opus.stdout.is_empty() && opus.stderr.is_empty());

    // Switch model in transcript to claude-sonnet-4-6 -> now blocks.
    fs::write(
        &transcript,
        r#"{"type":"assistant","message":{"model":"claude-sonnet-4-6","content":[]}}"#,
    )
    .unwrap();
    let sonnet = f.hookah(&["hook", "claude"], &claude_payload);
    assert_eq!(sonnet.status.code(), Some(2));
    assert!(stderr(&sonnet).contains("src/lib.rs:2"));

    // Plugin harness (pi): skipped model returns valid quiet JSON; provider-prefixed model matches bare pattern.
    let pi_payload = |model: &str| {
        serde_json::json!({
            "v": 1,
            "event": "edit",
            "cwd": f.root,
            "files": ["src/lib.rs"],
            "model": model
        })
        .to_string()
    };
    let pi_skip = f.hookah(&["hook", "pi"], &pi_payload("anthropic/claude-opus-4-6"));
    assert_eq!(pi_skip.status.code(), Some(0));
    assert_eq!(stdout(&pi_skip).trim(), r#"{"v":1,"block":false}"#);

    let pi_hit = f.hookah(&["hook", "pi"], &pi_payload("anthropic/claude-sonnet-4-6"));
    assert_eq!(pi_hit.status.code(), Some(0));
    let pi_json: serde_json::Value = serde_json::from_slice(&pi_hit.stdout).unwrap();
    assert_eq!(pi_json["block"], true);

    // HOOKAH_MODELS="*" overrides the restrictive file, even when no model is reported.
    let override_out = f.hookah_with(
        &["hook", "claude"],
        &f.edit_payload("src/lib.rs"),
        &[("HOOKAH_MODELS", "*")],
    );
    assert_eq!(override_out.status.code(), Some(2));
}

#[test]
fn nested_project_model_filters_apply_at_start_and_stop() {
    let f = Fixture::new("nested-models", CLEAN_LIB);
    fs::create_dir_all(f.path("child/src")).unwrap();
    f.write(
        "child/Cargo.toml",
        "[package]\nname = \"child\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
    );
    f.write("child/src/lib.rs", CLEAN_LIB);
    f.git(&["add", "-A"]);
    f.git(&["commit", "-q", "-m", "add child crate"]);

    let broken = "pub fn answer() -> i32 {\n    \"nope\"\n}\n";
    let baseline_count = |f: &Fixture| -> usize {
        fs::read_dir(f.cache())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|dir| fs::read_dir(dir.path().join("baselines")).ok())
            .flatten()
            .count()
    };

    // Direction 1: unrestricted root, disabled child (`models = []`).
    f.write("child/.hookah.toml", "models = []\n");
    f.write("child/src/lib.rs", broken);
    let start = f.hookah(&["hook", "claude"], &f.start_payload("sess-dir1"));
    assert_eq!(start.status.code(), Some(0), "{}", stderr(&start));
    // Only the root project got a baseline; the disabled child did not.
    assert_eq!(baseline_count(&f), 1);

    let stop = f.hookah(&["hook", "claude"], &f.stop_payload(false));
    assert_eq!(stop.status.code(), Some(0), "{}", stderr(&stop));

    // Direction 2: disabled root (`models = []`), enabled child (`models = ["*"]`).
    let _ = fs::remove_dir_all(f.cache());
    f.write(".hookah.toml", "models = []\n");
    f.write("child/.hookah.toml", "models = [\"*\"]\n");
    f.write("src/lib.rs", broken);
    f.write("child/src/lib.rs", broken);

    let start = f.hookah(&["hook", "claude"], &f.start_payload("sess-dir2"));
    assert_eq!(start.status.code(), Some(0), "{}", stderr(&start));
    // Only the enabled child got a baseline; the disabled root did not.
    assert_eq!(baseline_count(&f), 1);

    let stop = f.hookah(&["hook", "claude"], &f.stop_payload(false));
    assert_eq!(stop.status.code(), Some(2), "{}", stderr(&stop));
    let err = stderr(&stop);
    assert!(err.contains("child/src/lib.rs:2"), "{err}");
    assert!(!err.contains("\nsrc/lib.rs:2"), "{err}");
}

#[test]
fn claude_post_model_switch_overrides_a_stale_transcript_until_flushed() {
    let f = Fixture::new("claude-switch", CLEAN_LIB);
    f.write(".hookah.toml", "models = [\"claude-sonnet-*\"]\n");
    f.write("src/lib.rs", "pub fn answer() -> i32 {\n    \"nope\"\n}\n");

    let transcript = f.cache().join("switch.jsonl");
    fs::create_dir_all(f.cache()).unwrap();
    let old_assistant = concat!(
        r#"{"type":"assistant","timestamp":"2026-04-01T12:00:00Z","message":{"model":"claude-opus-4-6","content":[]}}"#,
        "\n"
    );
    fs::write(&transcript, old_assistant).unwrap();

    // Before the switch, the stale transcript says opus -> skipped.
    let edit_payload = serde_json::json!({
        "session_id": "ses-sw",
        "cwd": f.root,
        "hook_event_name": "PostToolUse",
        "transcript_path": transcript,
        "tool_name": "Edit",
        "tool_input": {"file_path": f.path("src/lib.rs")}
    })
    .to_string();
    assert_eq!(
        f.hookah(&["hook", "claude"], &edit_payload).status.code(),
        Some(0)
    );

    // Native PostModelSwitch carries `to_model` and `transcript_path`.
    let switch_payload = serde_json::json!({
        "session_id": "ses-sw",
        "cwd": f.root,
        "hook_event_name": "PostModelSwitch",
        "from_model": "claude-opus-4-6",
        "to_model": "claude-sonnet-4-6",
        "transcript_path": transcript
    })
    .to_string();
    let sw = f.hookah(&["hook", "claude"], &switch_payload);
    assert_eq!(sw.status.code(), Some(0));
    assert!(sw.stdout.is_empty() && sw.stderr.is_empty());

    // Appending non-model entries (user + <synthetic>) bumps the transcript file mtime, yet the
    // older opus assistant entry must NOT override the newer PostModelSwitch.
    let mut updated = old_assistant.to_owned();
    updated.push_str(
        r#"{"type":"user","timestamp":"2026-04-01T12:05:00Z","message":{"role":"user","content":"continue"}}"#,
    );
    updated.push('\n');
    updated.push_str(
        r#"{"type":"assistant","timestamp":"2026-04-01T12:05:01Z","message":{"model":"<synthetic>","content":[]}}"#,
    );
    updated.push('\n');
    fs::write(&transcript, &updated).unwrap();

    let after = f.hookah(&["hook", "claude"], &edit_payload);
    assert_eq!(after.status.code(), Some(2), "{}", stderr(&after));
    assert!(stderr(&after).contains("src/lib.rs:2"));

    // Once a real assistant turn with a different model is appended after the switch, it takes over.
    updated.push_str(
        r#"{"type":"assistant","timestamp":"2026-04-01T12:06:00Z","message":{"model":"claude-opus-4-6","content":[]}}"#,
    );
    updated.push('\n');
    fs::write(&transcript, &updated).unwrap();
    let superseded = f.hookah(&["hook", "claude"], &edit_payload);
    assert_eq!(superseded.status.code(), Some(0), "{}", stderr(&superseded));
}

#[test]
fn antigravity_model_name_filters_start_edit_feedback_and_stop() {
    let f = Fixture::new("agy-model-name", CLEAN_LIB);
    f.write(".hookah.toml", "models = [\"gemini-4-*\"]\n");

    let invoke = |session: &str, fields: serde_json::Value| {
        let mut payload = serde_json::json!({
            "conversationId": session,
            "workspacePaths": [f.root],
            "lastUserInput": "Fix lib.rs",
        });
        payload
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        f.hookah(&["hook", "antigravity"], &payload.to_string())
    };

    // Allowed native `modelName` on SessionStart starts the baseline and caches the model.
    let start = invoke(
        "agy-ok",
        serde_json::json!({"modelName": "gemini-4-neon-low"}),
    );
    assert!(start.status.success(), "{}", stderr(&start));

    // PostToolUse without `modelName` resolves the cached modelName from SessionStart.
    f.write("src/lib.rs", "pub fn answer()->i32{43}\n");
    let edit = invoke(
        "agy-ok",
        serde_json::json!({
            "stepIdx": 1,
            "toolCall": {"name": "edit_file", "args": {"Path": "src/lib.rs"}}
        }),
    );
    assert!(edit.status.success());
    assert_eq!(
        f.read("src/lib.rs"),
        "pub fn answer() -> i32 {\n    43\n}\n"
    );

    // PreInvocation with matching `modelName` delivers queued edit feedback.
    let before = invoke(
        "agy-ok",
        serde_json::json!({"invocationNum": 1, "modelName": "gemini-4-neon-low"}),
    );
    assert!(before.status.success());
    let before_json: serde_json::Value = serde_json::from_str(&stdout(&before)).unwrap();
    let msg = before_json["injectSteps"][0]["systemMessage"]["systemMessage"]
        .as_str()
        .unwrap();
    assert!(
        msg.contains("src/lib.rs") && msg.contains("Re-read"),
        "{msg}"
    );

    // Stop with matching `modelName` blocks (continues) on a failing test.
    f.write("src/lib.rs", FAILING_TEST_LIB);
    let stop_ok = invoke(
        "agy-ok",
        serde_json::json!({
            "executionNum": 0,
            "fullyIdle": true,
            "terminationReason": "model_stop",
            "modelName": "gemini-4-neon-low"
        }),
    );
    assert!(stop_ok.status.success());
    let stop_ok_json: serde_json::Value = serde_json::from_str(&stdout(&stop_ok)).unwrap();
    assert_eq!(stop_ok_json["decision"], "continue");

    // Non-matching `modelName` skips stop checks silently.
    let stop_skip = invoke(
        "agy-skip",
        serde_json::json!({
            "executionNum": 0,
            "fullyIdle": true,
            "terminationReason": "model_stop",
            "modelName": "gemini-3-flash"
        }),
    );
    assert!(stop_skip.status.success());
    let stop_skip_json: serde_json::Value = serde_json::from_str(&stdout(&stop_skip)).unwrap();
    assert_eq!(stop_skip_json["decision"], "allow");
}

#[test]
fn unfiltered_session_start_writes_no_session_model_cache() {
    let f = Fixture::new("no-session-model-cache", CLEAN_LIB);
    let start = serde_json::json!({
        "session_id": "ses-unfiltered",
        "cwd": f.root,
        "hook_event_name": "SessionStart",
        "model": "claude-sonnet-4-6"
    })
    .to_string();
    assert_eq!(f.hookah(&["hook", "claude"], &start).status.code(), Some(0));

    let sw = serde_json::json!({
        "session_id": "ses-unfiltered",
        "cwd": f.root,
        "hook_event_name": "PostModelSwitch",
        "to_model": "claude-opus-4-6"
    })
    .to_string();
    assert_eq!(f.hookah(&["hook", "claude"], &sw).status.code(), Some(0));

    let has_session_model_file = || {
        fs::read_dir(f.cache())
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("session-model-")
            })
    };
    assert!(
        !has_session_model_file(),
        "unfiltered sessions must not create session-model cache files"
    );

    // An unrelated [[project]] section in the user config must NOT trigger cache writes here.
    let user_cfg = f.cache().join("unrelated-user.toml");
    fs::create_dir_all(f.cache()).unwrap();
    fs::write(
        &user_cfg,
        "[[project]]\npath = \"/somewhere/unrelated\"\nmodels = [\"claude-sonnet-*\"]\n",
    )
    .unwrap();
    let with_unrelated = f.hookah_with(
        &["hook", "claude"],
        &sw,
        &[("HOOKAH_CONFIG", user_cfg.to_str().unwrap())],
    );
    assert_eq!(with_unrelated.status.code(), Some(0));
    assert!(
        !has_session_model_file(),
        "unrelated user config [[project]] must not create session cache files"
    );

    // Even when a project file restricts models, HOOKAH_MODELS="*" disables the filter and
    // prevents creating or rewriting a session-model file on PostModelSwitch.
    f.write(".hookah.toml", "models = [\"claude-sonnet-*\"]\n");
    let sw_override = f.hookah_with(&["hook", "claude"], &sw, &[("HOOKAH_MODELS", "*")]);
    assert_eq!(sw_override.status.code(), Some(0));
    assert!(
        !has_session_model_file(),
        "HOOKAH_MODELS=* must not create session cache files"
    );
}

#[test]
fn claude_switch_survives_cwd_change_and_subagent_uses_child_transcript() {
    let f = Fixture::new("claude-cwd-subagent", CLEAN_LIB);
    f.write(".hookah.toml", "models = [\"claude-sonnet-*\"]\n");

    let parent_transcript = f.cache().join("main-session.jsonl");
    let subagents_dir = f.cache().join("main-session").join("subagents");
    fs::create_dir_all(&subagents_dir).unwrap();
    fs::write(
        &parent_transcript,
        r#"{"type":"assistant","timestamp":"2020-01-01T00:00:00Z","message":{"model":"claude-opus-4-6","content":[]}}"#,
    )
    .unwrap();

    // Start the session on the clean repo so the session baseline is clean.
    let start = serde_json::json!({
        "session_id": "ses-cwd",
        "cwd": f.root,
        "hook_event_name": "SessionStart",
        "model": "claude-sonnet-4-6",
        "transcript_path": parent_transcript
    })
    .to_string();
    assert_eq!(f.hookah(&["hook", "claude"], &start).status.code(), Some(0));

    f.write("src/lib.rs", "pub fn answer() -> i32 {\n    \"nope\"\n}\n");

    // Switch to sonnet from the repo root.
    let sw = serde_json::json!({
        "session_id": "ses-cwd",
        "cwd": f.root,
        "hook_event_name": "PostModelSwitch",
        "to_model": "claude-sonnet-4-6",
        "transcript_path": parent_transcript
    })
    .to_string();
    assert_eq!(f.hookah(&["hook", "claude"], &sw).status.code(), Some(0));

    // Subsequent edit from a subdirectory still resolves the recorded switch.
    let sub_edit = serde_json::json!({
        "session_id": "ses-cwd",
        "cwd": f.path("src"),
        "hook_event_name": "PostToolUse",
        "transcript_path": parent_transcript,
        "tool_name": "Edit",
        "tool_input": {"file_path": f.path("src/lib.rs")}
    })
    .to_string();
    let from_subdir = f.hookah(&["hook", "claude"], &sub_edit);
    assert_eq!(
        from_subdir.status.code(),
        Some(2),
        "{}",
        stderr(&from_subdir)
    );

    // Subagent PostToolUse with agent_id and no agent_transcript_path must NOT use the parent
    // session's transcript or cached switch when its own child transcript is absent.
    let subagent_missing = serde_json::json!({
        "session_id": "ses-cwd",
        "cwd": f.root,
        "hook_event_name": "PostToolUse",
        "agent_id": "child99",
        "transcript_path": parent_transcript,
        "tool_name": "Edit",
        "tool_input": {"file_path": f.path("src/lib.rs")}
    })
    .to_string();
    assert_eq!(
        f.hookah(&["hook", "claude"], &subagent_missing)
            .status
            .code(),
        Some(0)
    );

    // Once the child transcript exists at <session>/subagents/agent-<id>.jsonl with sonnet, it
    // runs checks using the child's own model even though the parent transcript has opus.
    fs::write(
        subagents_dir.join("agent-child99.jsonl"),
        r#"{"type":"assistant","timestamp":"2020-01-01T00:00:00Z","message":{"model":"claude-sonnet-4-6","content":[]}}"#,
    )
    .unwrap();
    let subagent_allowed = f.hookah(&["hook", "claude"], &subagent_missing);
    assert_eq!(
        subagent_allowed.status.code(),
        Some(2),
        "{}",
        stderr(&subagent_allowed)
    );

    // Switching from an unrestricted sibling directory updates the existing session record so
    // editing back in the filtered project does NOT see the stale prior model.
    let unrestricted_sibling = f.cache().join("unrestricted-sibling");
    fs::create_dir_all(&unrestricted_sibling).unwrap();
    let sw_from_sibling = serde_json::json!({
        "session_id": "ses-cwd",
        "cwd": unrestricted_sibling,
        "hook_event_name": "PostModelSwitch",
        "to_model": "claude-opus-4-6",
        "transcript_path": parent_transcript
    })
    .to_string();
    assert_eq!(
        f.hookah(&["hook", "claude"], &sw_from_sibling)
            .status
            .code(),
        Some(0)
    );
    let back_in_filtered = f.hookah(&["hook", "claude"], &sub_edit);
    assert_eq!(
        back_in_filtered.status.code(),
        Some(0),
        "switch from unrestricted sibling must overwrite stale cached model"
    );
}

#[test]
fn antigravity_caches_model_for_deeply_nested_filtered_project() {
    let f = Fixture::new("agy-deep-filter", CLEAN_LIB);
    // Nest 16 levels deep so both StartSession (depth 3) and the config scan depth cap are tested.
    let deep_rel = "d/d/d/d/d/d/d/d/d/d/d/d/d/d/d/d";
    fs::create_dir_all(f.path(&format!("{deep_rel}/src"))).unwrap();
    f.write(
        &format!("{deep_rel}/Cargo.toml"),
        "[package]\nname = \"deep\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
    );
    f.write(
        &format!("{deep_rel}/.hookah.toml"),
        "models = [\"gemini-4-*\"]\n",
    );
    f.write(
        &format!("{deep_rel}/src/lib.rs"),
        "pub fn answer()->i32{43}\n",
    );

    // Unrestricted root; deep project is 16 levels down.
    let start = serde_json::json!({
        "conversationId": "agy-deep",
        "workspacePaths": [f.root],
        "modelName": "gemini-4-neon-low"
    });
    assert!(
        f.hookah(&["hook", "antigravity"], &start.to_string())
            .status
            .success()
    );

    // Model-less PostToolUse in the deep child still resolves the cached gemini model.
    let edit = serde_json::json!({
        "conversationId": "agy-deep",
        "workspacePaths": [f.root],
        "stepIdx": 1,
        "toolCall": {"name": "edit_file", "args": {"Path": format!("{deep_rel}/src/lib.rs")}}
    });
    let out = f.hookah(&["hook", "antigravity"], &edit.to_string());
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        f.read(&format!("{deep_rel}/src/lib.rs")),
        "pub fn answer() -> i32 {\n    43\n}\n"
    );
}

#[test]
fn config_verifies_model_patterns_and_hook_payloads() {
    let f = Fixture::new("config-model-check", CLEAN_LIB);
    f.write(
        ".hookah.toml",
        "models = [\"claude-sonnet-*\", \"gemini-4-*\"]\n",
    );

    let allowed = stdout(&f.hookah(&["config", "--model", "claude-sonnet-4-6"], ""));
    assert!(
        allowed.contains(
            r#"model check: claude-sonnet-4-6 -> allow (matches pattern "claude-sonnet-*")"#
        ),
        "{allowed}"
    );

    let skipped = stdout(&f.hookah(&["config", "--model", "claude-opus-4-6"], ""));
    assert!(
        skipped.contains(
            "model check: claude-opus-4-6 -> skip (does not match configured models: claude-sonnet-*, gemini-4-*)"
        ),
        "{skipped}"
    );

    let missing = stdout(&f.hookah(&["config", "--model", ""], ""));
    assert!(
        missing.contains("model check: <none> -> skip (no model reported;"),
        "{missing}"
    );

    // Payload smoke test for Antigravity native `modelName` (and must write nothing to cache).
    let _ = fs::remove_dir_all(f.cache());
    let agy_payload = serde_json::json!({
        "conversationId": "s",
        "workspacePaths": [f.root],
        "modelName": "gemini-4-neon-low"
    })
    .to_string();
    let agy_out = stdout(&f.hookah(&["config", "--hook", "antigravity"], &agy_payload));
    assert!(agy_out.contains("harness: antigravity\n"), "{agy_out}");
    assert!(agy_out.contains("event: start\n"), "{agy_out}");
    assert!(
        agy_out.contains("model: gemini-4-neon-low (payload)\n"),
        "{agy_out}"
    );
    assert!(
        agy_out.contains(r#"decision: allow (matches pattern "gemini-4-*")"#),
        "{agy_out}"
    );
    assert!(
        !f.cache().exists(),
        "config --hook must not create any cache directories"
    );

    // Monorepo diagnostic: disabled root (`models = []`) + enabled child (`models = ["*"]`)
    // shows the child's `allow` decision on both Start and Stop.
    fs::create_dir_all(f.path("child/src")).unwrap();
    f.write(
        "child/Cargo.toml",
        "[package]\nname = \"child\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
    );
    f.write("child/src/lib.rs", CLEAN_LIB);
    f.git(&["add", "-A"]);
    f.git(&["commit", "-q", "-m", "add child crate"]);
    f.write(".hookah.toml", "models = []\n");
    f.write("child/.hookah.toml", "models = [\"*\"]\n");
    f.write("child/src/lib.rs", "pub fn answer() -> i32 { 43 }\n");

    let start_diag = stdout(&f.hookah(&["config", "--hook", "claude"], &f.start_payload("diag")));
    assert!(
        start_diag.contains("child\nmodels: all") && start_diag.contains("decision: allow"),
        "{start_diag}"
    );
    let stop_diag = stdout(&f.hookah(&["config", "--hook", "claude"], &f.stop_payload(false)));
    assert!(
        stop_diag.contains("child/src\nmodels: all") && stop_diag.contains("decision: allow"),
        "{stop_diag}"
    );
    assert!(
        !f.cache().exists(),
        "monorepo config --hook must not create any cache directories"
    );

    // Even with 9 sibling projects where the first 8 are disabled and the 9th is enabled,
    // Start diagnostic must NOT truncate before filtering and omit the 9th project.
    for i in 0..9 {
        let name = format!("p{i:02}");
        fs::create_dir_all(f.path(&format!("{name}/src"))).unwrap();
        f.write(
            &format!("{name}/Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n"),
        );
        f.write(&format!("{name}/src/lib.rs"), CLEAN_LIB);
        let cfg = if i < 8 {
            "models = []\n"
        } else {
            "models = [\"*\"]\n"
        };
        f.write(&format!("{name}/.hookah.toml"), cfg);
    }
    let many_diag = stdout(&f.hookah(&["config", "--hook", "claude"], &f.start_payload("many")));
    assert!(
        many_diag.contains("p08\nmodels: all") && many_diag.contains("decision: allow"),
        "{many_diag}"
    );

    // Payload smoke test for Claude transcript resolution.
    f.write(
        ".hookah.toml",
        "models = [\"claude-sonnet-*\", \"gemini-4-*\"]\n",
    );
    let transcript = f.cache().join("diag.jsonl");
    fs::create_dir_all(f.cache()).unwrap();
    fs::write(
        &transcript,
        r#"{"type":"assistant","message":{"model":"claude-opus-4-6","content":[]}}"#,
    )
    .unwrap();
    let claude_payload = serde_json::json!({
        "session_id": "s",
        "cwd": f.root,
        "hook_event_name": "PostToolUse",
        "transcript_path": transcript,
        "tool_name": "Edit",
        "tool_input": {"file_path": f.path("src/lib.rs")}
    })
    .to_string();
    let claude_out = stdout(&f.hookah(&["config", "--hook", "claude"], &claude_payload));
    assert!(claude_out.contains("harness: claude\n"), "{claude_out}");
    assert!(claude_out.contains("event: edit\n"), "{claude_out}");
    assert!(
        claude_out.contains("model: claude-opus-4-6 (transcript:"),
        "{claude_out}"
    );
    assert!(
        claude_out.contains("decision: skip (does not match configured models:"),
        "{claude_out}"
    );
}

#[test]
fn antigravity_delivers_feedback_for_an_allowed_secondary_workspace() {
    let primary = Fixture::new("agy-primary", CLEAN_LIB);
    let secondary = Fixture::new("agy-secondary", CLEAN_LIB);
    primary.write(".hookah.toml", "models = []\n");
    secondary.write(".hookah.toml", "models = [\"*\"]\n");
    secondary.write("src/lib.rs", "pub fn answer()->i32{43}\n");

    let roots = [primary.root.clone(), secondary.root.clone()];
    let edit = serde_json::json!({
        "conversationId": "multi-root",
        "workspacePaths": roots,
        "stepIdx": 1,
        "toolCall": {"name": "edit_file", "args": {"Path": secondary.path("src/lib.rs")}}
    });
    let edit_out = primary.hookah(&["hook", "antigravity"], &edit.to_string());
    assert!(edit_out.status.success());

    let before = serde_json::json!({
        "conversationId": "multi-root",
        "workspacePaths": [primary.root, secondary.root],
        "invocationNum": 1
    });
    let before_out = primary.hookah(&["hook", "antigravity"], &before.to_string());
    assert!(before_out.status.success());
    let value: serde_json::Value = serde_json::from_str(&stdout(&before_out)).unwrap();
    let message = value["injectSteps"][0]["systemMessage"]["systemMessage"]
        .as_str()
        .unwrap();
    assert!(message.contains("src/lib.rs") && message.contains("Re-read"));
}
