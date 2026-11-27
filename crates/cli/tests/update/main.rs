//! End-to-end tests of the self-update through the `hookah` binary, against a local fake release
//! server. The binary under test is always a copy in a temporary directory, never the one under
//! `target/`: an update replaces the executable it runs from. Every variable that decides where
//! hookah looks for config, cache, or releases is set, so nothing of the machine leaks in.
//!
//! Unix and debug builds only: the fake release holds shell scripts, and the test server override
//! does not exist in release builds.
#![cfg(all(unix, debug_assertions))]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hookah_adapters::update::install::sha256_hex;

const TRIPLE: &str = "x86_64-unknown-linux-musl";

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Everything one test owns: directories for the binary copies, the cache, and the config.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let root = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "hookah-e2e-upd-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        let _ = fs::remove_dir_all(&root);
        for dir in ["bin", "reader", "cache", "home", "cwd"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        let real = Path::new(env!("CARGO_BIN_EXE_hookah"));
        for dir in ["bin", "reader"] {
            let copy = root.join(dir).join("hookah");
            fs::copy(real, &copy).unwrap();
            fs::set_permissions(&copy, fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { root }
    }

    /// The copy that gets replaced.
    fn exe(&self) -> PathBuf {
        self.root.join("bin").join("hookah")
    }

    /// A second copy, which only reads the shared state.
    fn reader(&self) -> PathBuf {
        self.root.join("reader").join("hookah")
    }

    fn config(&self) -> PathBuf {
        self.root.join("home").join("config.toml")
    }

    /// `exe args...` with a clean, sandboxed environment that pretends to be version `1.0.0`.
    fn command(&self, exe: &Path, server: &Server) -> Command {
        let mut command = Command::new(exe);
        command
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("home").join("xdg"))
            .env("XDG_CACHE_HOME", self.root.join("home").join("xdgcache"))
            .env("APPDATA", self.root.join("home").join("appdata"))
            .env("LOCALAPPDATA", self.root.join("home").join("localappdata"))
            .env("HOOKAH_CONFIG", self.config())
            .env("HOOKAH_CACHE_DIR", self.root.join("cache"))
            .env("HOOKAH_UPDATE_TEST_BASE", server.base())
            .env("HOOKAH_UPDATE_TEST_VERSION", "1.0.0")
            .env("HOOKAH_UPDATE_TEST_TARGET", TRIPLE)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn hookah(&self, exe: &Path, server: &Server, args: &[&str]) -> Output {
        self.command(exe, server).args(args).output().unwrap()
    }

    /// A session start through `exe`, as Claude Code would send it.
    fn session_start(&self, exe: &Path, server: &Server) -> Output {
        let payload = format!(
            r#"{{"session_id":"s","cwd":"{}","hook_event_name":"SessionStart"}}"#,
            self.root.join("cwd").display()
        );
        let mut child = self
            .command(exe, server)
            .args(["hook", "claude"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn exe_text(&self) -> String {
        fs::read_to_string(self.exe()).unwrap_or_default()
    }

    fn state_file(&self) -> PathBuf {
        self.root.join("cache").join("update").join("state.json")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn script(version: &str) -> String {
    format!("#!/bin/sh\necho 'hookah {version}'\n")
}

/// Polls until `condition` holds, for up to ten seconds.
fn wait_for(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Builds `hookah-2.0.0-<triple>.tar.gz` around `binary` with the system `tar`.
fn archive(work: &Path, version: &str, binary: &str) -> Vec<u8> {
    let dir_name = format!("hookah-{version}-{TRIPLE}");
    let dir = work.join(&dir_name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("hookah"), binary).unwrap();
    fs::write(dir.join("LICENSE"), "license").unwrap();
    let out = work.join("release.tar.gz");
    let status = Command::new("tar")
        .arg("-czf")
        .arg(&out)
        .arg("-C")
        .arg(work)
        .arg(&dir_name)
        .status()
        .unwrap();
    assert!(status.success(), "tar failed");
    fs::read(out).unwrap()
}

/// A fake release `version` served on loopback.
fn release_server(sandbox: &Sandbox, version: &str) -> Server {
    let archive_name = format!("hookah-{version}-{TRIPLE}.tar.gz");
    let bytes = archive(&sandbox.root, version, &script(version));
    let sums = format!("{}  {archive_name}\n", sha256_hex(&bytes));
    let api = format!(
        r#"{{"tag_name":"v{version}","draft":false,"prerelease":false,"assets":[{{"name":"{archive_name}"}},{{"name":"SHA256SUMS"}}]}}"#
    );
    Server::start(vec![
        ("/latest".into(), api.into_bytes()),
        (
            format!("/download/v{version}/SHA256SUMS"),
            sums.into_bytes(),
        ),
        (format!("/download/v{version}/{archive_name}"), bytes),
    ])
}

struct Server {
    addr: SocketAddr,
    log: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl Server {
    fn start(routes: Vec<(String, Vec<u8>)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_log, thread_stop) = (Arc::clone(&log), Arc::clone(&stop));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if thread_stop.load(Ordering::SeqCst) {
                    return;
                }
                if let Ok(stream) = stream {
                    serve(stream, &routes, &thread_log);
                }
            }
        });
        Self { addr, log, stop }
    }

    fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn requests(&self) -> usize {
        self.log.lock().unwrap().len()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
    }
}

fn serve(mut stream: TcpStream, routes: &[(String, Vec<u8>)], log: &Mutex<Vec<String>>) {
    let mut request = Vec::new();
    let mut buf = [0u8; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => request.extend_from_slice(&buf[..n]),
        }
    }
    let request = String::from_utf8_lossy(&request).into_owned();
    let path = request
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_string();
    log.lock().unwrap().push(path.clone());
    let found = routes.iter().find(|(p, _)| *p == path);
    let (status, body) = found.map_or((404, b"nope".as_slice()), |(_, b)| (200, b.as_slice()));
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

#[test]
fn update_installs_the_latest_release_in_place() {
    let sandbox = Sandbox::new();
    let server = release_server(&sandbox, "2.0.0");
    let out = sandbox.hookah(&sandbox.exe(), &server, &["update"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    assert!(stdout.contains("checking and installing"), "{stdout}");
    assert!(
        stdout.contains("updated hookah from 1.0.0 to 2.0.0"),
        "{stdout}"
    );
    assert_eq!(sandbox.exe_text(), script("2.0.0"));
    let mode = fs::metadata(sandbox.exe()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);
    // The new binary answers `--version`, which is what the update verified.
    let version = Command::new(sandbox.exe())
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(text(&version.stdout).trim(), "hookah 2.0.0");
    // A manual update is not announced again at the next session start.
    let start = sandbox.session_start(&sandbox.reader(), &server);
    assert!(start.stdout.is_empty(), "{}", text(&start.stdout));
}

#[test]
fn update_check_only_reports() {
    let sandbox = Sandbox::new();
    let server = release_server(&sandbox, "2.0.0");
    let before = sandbox.exe_text();
    let out = sandbox.hookah(&sandbox.exe(), &server, &["update", "--check"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        text(&out.stdout).contains("hookah 2.0.0 is available"),
        "{}",
        text(&out.stdout)
    );
    assert_eq!(sandbox.exe_text(), before);
    assert_eq!(server.requests(), 1, "only the release document");
}

#[test]
fn update_with_nothing_newer_says_so() {
    let sandbox = Sandbox::new();
    let server = release_server(&sandbox, "1.0.0");
    let out = sandbox.hookah(&sandbox.exe(), &server, &["update"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out.stdout).contains("up to date"));
}

#[test]
fn update_refuses_in_a_development_build_and_when_switched_off() {
    let sandbox = Sandbox::new();
    let server = release_server(&sandbox, "2.0.0");
    let mut dev = sandbox.command(&sandbox.exe(), &server);
    dev.env_remove("HOOKAH_UPDATE_TEST_VERSION").arg("update");
    let out = dev.output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("development build"),
        "{}",
        text(&out.stderr)
    );
    let mut off = sandbox.command(&sandbox.exe(), &server);
    off.env("HOOKAH_NO_UPDATE", "1").arg("update");
    let out = off.output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("HOOKAH_NO_UPDATE"));
    assert_eq!(server.requests(), 0);
    fs::write(sandbox.config(), "[update]\ncheck = false\n").unwrap();
    let out = sandbox.hookah(&sandbox.exe(), &server, &["update", "--check"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(server.requests(), 0);
}

#[test]
fn update_failure_is_a_clear_message_and_exit_one() {
    let sandbox = Sandbox::new();
    let server = Server::start(vec![]);
    let before = sandbox.exe_text();
    let out = sandbox.hookah(&sandbox.exe(), &server, &["update"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("update failed"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(sandbox.exe_text(), before);
}

#[test]
fn a_session_start_updates_in_the_background_and_the_next_one_says_so_once() {
    let sandbox = Sandbox::new();
    let server = release_server(&sandbox, "2.0.0");
    let started = Instant::now();
    let first = sandbox.session_start(&sandbox.exe(), &server);
    assert_eq!(first.status.code(), Some(0));
    assert!(
        first.stdout.is_empty(),
        "nothing yet: {}",
        text(&first.stdout)
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    wait_for("the background update", || {
        sandbox.exe_text() == script("2.0.0")
    });
    wait_for("the state file", || {
        fs::read_to_string(sandbox.state_file()).is_ok_and(|s| s.contains("\"updated\""))
    });
    let requests = server.requests();

    let second = sandbox.session_start(&sandbox.reader(), &server);
    assert_eq!(second.status.code(), Some(0));
    let json: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(
        json["systemMessage"],
        "hookah: updated hookah from 1.0.0 to 2.0.0; new sessions use it"
    );
    assert!(json.get("hookSpecificOutput").is_none());

    let third = sandbox.session_start(&sandbox.reader(), &server);
    assert!(
        third.stdout.is_empty(),
        "shown once: {}",
        text(&third.stdout)
    );
    assert!(third.stderr.is_empty());
    // And within a day nothing is fetched again.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(server.requests(), requests);
}

#[test]
fn with_auto_off_a_session_start_only_announces_the_new_version_once() {
    let sandbox = Sandbox::new();
    fs::write(sandbox.config(), "[update]\nauto = false\n").unwrap();
    let server = release_server(&sandbox, "2.0.0");
    let before = sandbox.exe_text();
    sandbox.session_start(&sandbox.exe(), &server);
    wait_for("the state file", || {
        fs::read_to_string(sandbox.state_file()).is_ok_and(|s| s.contains("\"available\""))
    });
    assert_eq!(sandbox.exe_text(), before, "nothing installed");
    let second = sandbox.session_start(&sandbox.reader(), &server);
    let json: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(
        json["systemMessage"],
        "hookah: hookah 2.0.0 is available; run `hookah update` to install it"
    );
    assert!(
        sandbox
            .session_start(&sandbox.reader(), &server)
            .stdout
            .is_empty()
    );
}

#[test]
fn the_opt_outs_start_nothing() {
    let sandbox = Sandbox::new();
    let server = release_server(&sandbox, "2.0.0");
    let mut off = sandbox.command(&sandbox.exe(), &server);
    off.env("HOOKAH_NO_UPDATE", "1")
        .args(["hook", "claude"])
        .stdin(Stdio::piped());
    let mut child = off.spawn().unwrap();
    let payload = format!(
        r#"{{"session_id":"s","cwd":"{}","hook_event_name":"SessionStart"}}"#,
        sandbox.root.join("cwd").display()
    );
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    assert_eq!(child.wait_with_output().unwrap().status.code(), Some(0));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(server.requests(), 0);
    assert!(!sandbox.state_file().exists());
}

#[test]
fn a_development_build_never_checks_at_a_session_start() {
    let sandbox = Sandbox::new();
    let server = release_server(&sandbox, "2.0.0");
    let mut dev = sandbox.command(&sandbox.exe(), &server);
    dev.env_remove("HOOKAH_UPDATE_TEST_VERSION")
        .args(["hook", "claude"])
        .stdin(Stdio::piped());
    let mut child = dev.spawn().unwrap();
    let payload = format!(
        r#"{{"session_id":"s","cwd":"{}","hook_event_name":"SessionStart"}}"#,
        sandbox.root.join("cwd").display()
    );
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    assert_eq!(child.wait_with_output().unwrap().status.code(), Some(0));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(server.requests(), 0);
}

#[test]
fn a_model_filter_does_not_suppress_session_start_update_notices() {
    let sandbox = Sandbox::new();
    fs::write(sandbox.config(), "models = []\n[update]\nauto = false\n").unwrap();
    let server = release_server(&sandbox, "2.0.0");
    let first = sandbox.session_start(&sandbox.exe(), &server);
    assert_eq!(first.status.code(), Some(0));
    wait_for("the state file", || {
        fs::read_to_string(sandbox.state_file()).is_ok_and(|s| s.contains("\"available\""))
    });
    let second = sandbox.session_start(&sandbox.reader(), &server);
    let json: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(
        json["systemMessage"],
        "hookah: hookah 2.0.0 is available; run `hookah update` to install it"
    );
}
