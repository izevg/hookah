//! The self-update against a local fake release server: the real HTTP client, archive reading,
//! checksum, version check, and swap, with only the server faked. Never touches the network
//! beyond loopback. Debug builds only: the test server override does not exist in release builds.
#![cfg(all(debug_assertions, unix))]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod server;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use hookah_adapters::update::SystemClock;
use hookah_adapters::update::github::GithubReleases;
use hookah_adapters::update::install::{SelfInstaller, sha256_hex};
use hookah_adapters::update::store::FileUpdateStore;
use hookah_application::ports::{UpdateError, UpdatePorts, UpdateSpawner, UpdateStore};
use hookah_application::{RunUpdate, UpdateEnv, UpdateMode, UpdateReport};
use hookah_domain::update::{Event, Policy, Target, Version};
use server::{Route, Server};

const TARGET: Target = Target::LinuxX64;

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "hookah-upd-it-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("state")).unwrap();
        let sandbox = Self { root };
        fs::write(sandbox.exe(), script("1.0.0")).unwrap();
        fs::set_permissions(sandbox.exe(), fs::Permissions::from_mode(0o700)).unwrap();
        sandbox
    }

    fn exe(&self) -> PathBuf {
        self.root.join("bin").join("hookah")
    }

    fn state(&self) -> PathBuf {
        self.root.join("state")
    }

    fn bin_files(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.root.join("bin"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::set_permissions(self.root.join("bin"), fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn script(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\necho 'hookah {version}'\n").into_bytes()
}

fn tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, content) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append_data(&mut header, path, *content).unwrap();
    }
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    std::io::Write::write_all(&mut gz, &builder.into_inner().unwrap()).unwrap();
    gz.finish().unwrap()
}

fn v(text: &str) -> Version {
    Version::parse(text).unwrap()
}

/// A release `version` on the fake server: the API document and the two assets. `binary` is what
/// the archive holds as `hookah`; `listed_hash` overrides the checksum the release lists.
struct Release {
    version: &'static str,
    binary: Vec<u8>,
    listed_hash: Option<String>,
    with_archive_listed: bool,
    prerelease: bool,
}

impl Release {
    fn new(version: &'static str) -> Self {
        Self {
            version,
            binary: script(version),
            listed_hash: None,
            with_archive_listed: true,
            prerelease: false,
        }
    }

    fn routes(&self) -> Vec<Route> {
        let archive_name = TARGET.archive_name(&v(self.version));
        let archive = tar_gz(&[
            (&format!("hookah-{}-x/hookah", self.version), &self.binary),
            (&format!("hookah-{}-x/LICENSE", self.version), b"license"),
        ]);
        let hash = self
            .listed_hash
            .clone()
            .unwrap_or_else(|| sha256_hex(&archive));
        let sums = format!("{hash}  {archive_name}\n");
        let assets = if self.with_archive_listed {
            format!(r#"[{{"name":"{archive_name}"}},{{"name":"SHA256SUMS"}}]"#)
        } else {
            r#"[{"name":"SHA256SUMS"}]"#.to_string()
        };
        let api = format!(
            r#"{{"tag_name":"v{}","draft":false,"prerelease":{},"assets":{assets}}}"#,
            self.version, self.prerelease
        );
        vec![
            Route::ok("/latest", api.into_bytes()),
            Route::ok(
                &format!("/download/v{}/SHA256SUMS", self.version),
                sums.into_bytes(),
            ),
            Route::ok(
                &format!("/download/v{}/{archive_name}", self.version),
                archive,
            ),
        ]
    }
}

struct NoSpawn;

impl UpdateSpawner for NoSpawn {
    fn spawn_check(&self) -> Result<(), UpdateError> {
        Ok(())
    }
}

fn run(sandbox: &Sandbox, server: &Server, current: &str, mode: UpdateMode) -> UpdateReport {
    let releases = GithubReleases::at(&server.base());
    let installer = SelfInstaller::at(sandbox.exe());
    let store = FileUpdateStore::in_dir(sandbox.state());
    let ports = UpdatePorts {
        releases: &releases,
        installer: &installer,
        store: &store,
        clock: &SystemClock,
        spawner: &NoSpawn,
    };
    let env = UpdateEnv {
        current: v(current),
        target: Some(TARGET),
        policy: Policy::default(),
    };
    RunUpdate::new(ports, &env).run(mode)
}

fn exe_text(sandbox: &Sandbox) -> String {
    fs::read_to_string(sandbox.exe()).unwrap()
}

#[test]
fn a_newer_release_is_downloaded_verified_and_swapped_in() {
    let sandbox = Sandbox::new();
    let server = Server::start(Release::new("2.0.0").routes());
    let report = run(&sandbox, &server, "1.0.0", UpdateMode::Background);
    assert_eq!(
        report,
        UpdateReport::Updated {
            from: v("1.0.0"),
            to: v("2.0.0")
        }
    );
    assert_eq!(
        exe_text(&sandbox),
        String::from_utf8(script("2.0.0")).unwrap()
    );
    assert_eq!(
        fs::metadata(sandbox.exe()).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(sandbox.bin_files(), ["hookah"]);
    assert_eq!(
        server.requests(),
        [
            "/latest".to_string(),
            "/download/v2.0.0/SHA256SUMS".into(),
            format!("/download/v2.0.0/{}", TARGET.archive_name(&v("2.0.0")))
        ]
    );
    let state = FileUpdateStore::in_dir(sandbox.state()).load();
    assert_eq!(state.latest.as_deref(), Some("2.0.0"));
    assert_eq!(
        state.pending,
        Some(Event::Updated {
            from: "1.0.0".into(),
            to: "2.0.0".into()
        })
    );
}

#[test]
fn a_checksum_mismatch_leaves_the_binary_alone() {
    let sandbox = Sandbox::new();
    let mut release = Release::new("2.0.0");
    release.listed_hash = Some("0".repeat(64));
    let server = Server::start(release.routes());
    let UpdateReport::Failed(e) = run(&sandbox, &server, "1.0.0", UpdateMode::Install) else {
        panic!("expected a failure");
    };
    assert!(e.message().contains("checksum mismatch"), "{e}");
    assert_eq!(
        exe_text(&sandbox),
        String::from_utf8(script("1.0.0")).unwrap()
    );
    assert_eq!(sandbox.bin_files(), ["hookah"]);
}

#[test]
fn a_release_without_the_platform_archive_downloads_nothing() {
    let sandbox = Sandbox::new();
    let mut release = Release::new("2.0.0");
    release.with_archive_listed = false;
    let server = Server::start(release.routes());
    let UpdateReport::Failed(e) = run(&sandbox, &server, "1.0.0", UpdateMode::Install) else {
        panic!("expected a failure");
    };
    assert!(
        e.message()
            .contains("has no hookah-2.0.0-x86_64-unknown-linux-musl.tar.gz"),
        "{e}"
    );
    assert_eq!(server.requests(), ["/latest"]);
    assert_eq!(
        exe_text(&sandbox),
        String::from_utf8(script("1.0.0")).unwrap()
    );
}

#[test]
fn a_binary_that_reports_the_wrong_version_is_not_installed() {
    let sandbox = Sandbox::new();
    let mut release = Release::new("2.0.0");
    release.binary = script("2.0.1");
    let server = Server::start(release.routes());
    let UpdateReport::Failed(e) = run(&sandbox, &server, "1.0.0", UpdateMode::Install) else {
        panic!("expected a failure");
    };
    assert!(e.message().contains("2.0.1"), "{e}");
    assert_eq!(
        exe_text(&sandbox),
        String::from_utf8(script("1.0.0")).unwrap()
    );
    assert_eq!(sandbox.bin_files(), ["hookah"]);
}

#[test]
fn a_directory_that_is_not_writable_blocks_before_any_download() {
    let sandbox = Sandbox::new();
    let bin = sandbox.root.join("bin");
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o555)).unwrap();
    // A superuser writes anywhere; the case cannot be staged then.
    if fs::write(bin.join(".probe"), "").is_ok() {
        return;
    }
    let server = Server::start(Release::new("2.0.0").routes());
    let report = run(&sandbox, &server, "1.0.0", UpdateMode::Background);
    let UpdateReport::Failed(UpdateError::Blocked(reason)) = report else {
        panic!("expected blocked, got {report:?}");
    };
    assert!(
        reason.contains("not writable") && reason.contains("installer"),
        "{reason}"
    );
    assert_eq!(server.requests(), ["/latest"], "nothing downloaded");
    let state = FileUpdateStore::in_dir(sandbox.state()).load();
    assert!(matches!(state.pending, Some(Event::Failed { .. })));
}

#[test]
fn an_up_to_date_binary_downloads_nothing() {
    let sandbox = Sandbox::new();
    let server = Server::start(Release::new("1.0.0").routes());
    assert_eq!(
        run(&sandbox, &server, "1.0.0", UpdateMode::Background),
        UpdateReport::UpToDate { latest: v("1.0.0") }
    );
    assert_eq!(server.requests(), ["/latest"]);
}

#[test]
fn check_only_reports_without_downloading() {
    let sandbox = Sandbox::new();
    let server = Server::start(Release::new("2.0.0").routes());
    assert_eq!(
        run(&sandbox, &server, "1.0.0", UpdateMode::Check),
        UpdateReport::Available { latest: v("2.0.0") }
    );
    assert_eq!(server.requests(), ["/latest"]);
}

#[test]
fn a_prerelease_as_latest_is_ignored() {
    let sandbox = Sandbox::new();
    let mut release = Release::new("2.0.0");
    release.prerelease = true;
    let server = Server::start(release.routes());
    assert!(matches!(
        run(&sandbox, &server, "1.0.0", UpdateMode::Install),
        UpdateReport::Failed(_)
    ));
    assert_eq!(server.requests(), ["/latest"]);
}

#[test]
fn server_errors_and_garbage_are_failures_recorded_for_a_retry() {
    let sandbox = Sandbox::new();
    for routes in [
        vec![],
        vec![Route::status("/latest", 503, b"busy".to_vec())],
        vec![Route::ok(
            "/latest",
            b"<html>captive portal</html>".to_vec(),
        )],
    ] {
        let server = Server::start(routes);
        assert!(matches!(
            run(&sandbox, &server, "1.0.0", UpdateMode::Background),
            UpdateReport::Failed(UpdateError::Failed(_))
        ));
        let state = FileUpdateStore::in_dir(sandbox.state()).load();
        assert!(state.error.is_some() && !state.last_ok);
        // Back-to-back background runs are throttled to an hour after a failure.
        assert_eq!(
            run(&sandbox, &server, "1.0.0", UpdateMode::Background),
            UpdateReport::NotDue
        );
        let _ = fs::remove_file(sandbox.state().join("state.json"));
    }
}

#[test]
fn a_missing_archive_on_the_server_is_a_failure() {
    let sandbox = Sandbox::new();
    let mut routes = Release::new("2.0.0").routes();
    routes.pop();
    let server = Server::start(routes);
    let UpdateReport::Failed(e) = run(&sandbox, &server, "1.0.0", UpdateMode::Install) else {
        panic!("expected a failure");
    };
    assert!(
        e.message().contains("404") || e.message().contains("cannot fetch"),
        "{e}"
    );
    assert_eq!(
        exe_text(&sandbox),
        String::from_utf8(script("1.0.0")).unwrap()
    );
}

#[test]
fn two_updaters_do_not_run_at_once() {
    let sandbox = Sandbox::new();
    let server = Server::start(Release::new("2.0.0").routes());
    let store = FileUpdateStore::in_dir(sandbox.state());
    let held = store.try_lock().unwrap().unwrap();
    assert_eq!(
        run(&sandbox, &server, "1.0.0", UpdateMode::Background),
        UpdateReport::Busy
    );
    assert!(server.requests().is_empty());
    drop(held);
    assert!(matches!(
        run(&sandbox, &server, "1.0.0", UpdateMode::Background),
        UpdateReport::Updated { .. }
    ));
}
