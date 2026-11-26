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
            // Never the developer's own user config; a test that wants one overrides this.
            .env("HOOKAH_CONFIG", self.cache().join("no-user-config.toml"))
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
