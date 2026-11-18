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

    /// Runs `hookah` in the crate root.
    fn hookah(&self, args: &[&str], stdin: &str) -> Output {
        self.hookah_with(args, stdin, &[])
    }

    /// Runs `hookah` in the crate root with extra environment variables. The detection cache
    /// lives under the target directory, not in the user's cache.
    fn hookah_with(&self, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hookah"))
            .args(args)
            .current_dir(&self.root)
            .env(
                "XDG_CACHE_HOME",
                Path::new(env!("CARGO_TARGET_TMPDIR")).join("xdg-cache"),
            )
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

    fn edit_payload(&self, rel: &str) -> String {
        format!(
            r#"{{"session_id":"s","cwd":{cwd},"hook_event_name":"PostToolUse","tool_name":"Edit","tool_input":{{"file_path":{file}}}}}"#,
            cwd = json_str(&self.root),
            file = json_str(&self.path(rel)),
        )
    }

    fn stop_payload(&self, active: bool) -> String {
        format!(
            r#"{{"session_id":"s","cwd":{cwd},"hook_event_name":"Stop","stop_hook_active":{active}}}"#,
            cwd = json_str(&self.root),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
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
    assert!(err.contains("after editing"), "{err}");
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
