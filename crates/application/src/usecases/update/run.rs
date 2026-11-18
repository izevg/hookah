//! `RunUpdate`: look for a newer release and install it.

use hookah_domain::update::state::UpdateState;
use hookah_domain::update::target::CHECKSUMS_ASSET;
use hookah_domain::update::{Decision, Policy, Version, checksum_of, decide};

use super::UpdateEnv;
use crate::ports::{Package, Release, UpdateError, UpdatePorts};

/// Who asked for the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The detached job a session start spawned: throttled, follows the user's `auto` setting,
    /// and leaves a notice for the next session.
    Background,
    /// `hookah update`: now, and installs whatever the policy's `auto` says not to.
    Install,
    /// `hookah update --check`: now, and only reports.
    Check,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    /// Updates are switched off.
    Disabled,
    /// This is a development build, which never updates itself.
    DevBuild,
    /// A background run found the last check recent enough.
    NotDue,
    /// Another updater holds the lock.
    Busy,
    /// Already on the latest release (or newer).
    UpToDate {
        /// The latest release.
        latest: Version,
    },
    /// A newer release exists and was not installed.
    Available {
        /// The newer release.
        latest: Version,
    },
    /// The binary was replaced.
    Updated {
        /// The version that was running.
        from: Version,
        /// The version installed.
        to: Version,
    },
    /// The check or the install did not work.
    Failed(UpdateError),
}

/// Checks the release server and installs a newer release when the mode and policy allow.
pub struct RunUpdate<'a> {
    ports: UpdatePorts<'a>,
    env: &'a UpdateEnv,
}

impl<'a> RunUpdate<'a> {
    /// Creates the use case.
    pub fn new(ports: UpdatePorts<'a>, env: &'a UpdateEnv) -> Self {
        Self { ports, env }
    }

    /// Runs one update attempt. The state is recorded, so the next session start can tell the
    /// user about the result.
    pub fn run(&self, mode: Mode) -> Report {
        if !self.env.policy.check {
            return Report::Disabled;
        }
        if self.env.current.is_dev() {
            return Report::DevBuild;
        }
        let _lock = match self.ports.store.try_lock() {
            Ok(Some(lock)) => lock,
            Ok(None) => return Report::Busy,
            Err(e) => return Report::Failed(e),
        };
        let mut state = self.ports.store.load();
        let now = self.ports.clock.now_ms();
        if mode == Mode::Background && !state.due(now) {
            return Report::NotDue;
        }
        let report = self.attempt(mode, &mut state, now);
        let _ = self.ports.store.save(&state);
        report
    }

    fn attempt(&self, mode: Mode, state: &mut UpdateState, now: u64) -> Report {
        let release = match self.ports.releases.latest() {
            Ok(release) => release,
            Err(e) => {
                state.record_failure(now, e.message());
                return Report::Failed(e);
            }
        };
        let latest = release.version;
        let policy = Policy {
            check: true,
            auto: match mode {
                Mode::Background => self.env.policy.auto,
                Mode::Install => true,
                Mode::Check => false,
            },
        };
        match decide(&self.env.current, &latest, policy) {
            Decision::DevBuild => Report::DevBuild,
            Decision::UpToDate => {
                state.record_checked(now, &latest);
                Report::UpToDate { latest }
            }
            Decision::Notify => {
                state.record_checked(now, &latest);
                if mode == Mode::Background {
                    state.record_available(&latest);
                }
                Report::Available { latest }
            }
            Decision::Install => match self.install(&release) {
                Ok(()) => {
                    state.record_checked(now, &latest);
                    state.record_updated(&self.env.current, &latest);
                    if mode != Mode::Background {
                        // The user is reading the result right now.
                        state.take_pending();
                    }
                    Report::Updated {
                        from: self.env.current,
                        to: latest,
                    }
                }
                Err(e @ UpdateError::Blocked(_)) => {
                    state.record_blocked(now, &latest, e.message());
                    if mode != Mode::Background {
                        state.take_pending();
                    }
                    Report::Failed(e)
                }
                Err(e) => {
                    state.record_failure(now, e.message());
                    Report::Failed(e)
                }
            },
        }
    }

    fn install(&self, release: &Release) -> Result<(), UpdateError> {
        let target = self.env.target.ok_or_else(|| {
            UpdateError::blocked("there is no official hookah build for this platform")
        })?;
        let version = &release.version;
        let archive = target.archive_name(version);
        for needed in [archive.as_str(), CHECKSUMS_ASSET] {
            if !release.assets.iter().any(|a| a == needed) {
                return Err(UpdateError::failed(format!(
                    "release {} has no {needed}",
                    release.tag
                )));
            }
        }
        self.ports.installer.preflight()?;
        let sums = self.ports.releases.asset(&release.tag, CHECKSUMS_ASSET)?;
        let sums = String::from_utf8(sums)
            .map_err(|_| UpdateError::failed(format!("{CHECKSUMS_ASSET} is not text")))?;
        let sha256 = checksum_of(&sums, &archive).ok_or_else(|| {
            UpdateError::failed(format!("{CHECKSUMS_ASSET} has no checksum for {archive}"))
        })?;
        let bytes = self.ports.releases.asset(&release.tag, &archive)?;
        self.ports.installer.install(&Package {
            bytes: &bytes,
            kind: target.archive_kind(),
            sha256: &sha256,
            entry: target.binary_name(),
            version,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use hookah_domain::update::state::{CHECK_INTERVAL_MS, RETRY_INTERVAL_MS};
    use hookah_domain::update::{Event, Target};

    use super::super::fakes::{SUMS_HASH, World, v};
    use super::*;

    const TARGET: Target = Target::MacArm64;

    fn env(current: &str, policy: Policy, target: Option<Target>) -> UpdateEnv {
        UpdateEnv {
            current: v(current),
            target,
            policy,
        }
    }

    fn run(w: &World, current: &str, mode: Mode) -> Report {
        run_with(w, &env(current, Policy::default(), Some(TARGET)), mode)
    }

    fn run_with(w: &World, e: &UpdateEnv, mode: Mode) -> Report {
        RunUpdate::new(w.ports(), e).run(mode)
    }

    fn serving(latest: &str) -> World {
        World::serving(latest, TARGET)
    }

    #[test]
    fn a_newer_release_is_verified_and_installed() {
        let w = serving("1.1.0");
        assert_eq!(
            run(&w, "1.0.0", Mode::Background),
            Report::Updated {
                from: v("1.0.0"),
                to: v("1.1.0")
            }
        );
        assert_eq!(
            *w.installed.borrow(),
            [(
                SUMS_HASH.to_string(),
                "hookah".to_string(),
                "1.1.0".to_string()
            )]
        );
        assert_eq!(
            *w.downloads.borrow(),
            [
                "v1.1.0/SHA256SUMS".to_string(),
                "v1.1.0/hookah-1.1.0-aarch64-apple-darwin.tar.gz".to_string()
            ]
        );
        assert!(w.lock_held_during_install.get());
        assert!(!w.locked.get(), "the lock is released");
        let state = w.state.borrow();
        assert_eq!(state.latest.as_deref(), Some("1.1.0"));
        assert_eq!(
            state.pending,
            Some(Event::Updated {
                from: "1.0.0".into(),
                to: "1.1.0".into()
            })
        );
    }

    #[test]
    fn a_windows_target_installs_its_exe_from_a_zip() {
        let w = World::serving("2.0.0", Target::WindowsX64);
        let e = env("1.0.0", Policy::default(), Some(Target::WindowsX64));
        assert!(matches!(
            run_with(&w, &e, Mode::Install),
            Report::Updated { .. }
        ));
        assert_eq!(w.installed.borrow()[0].1, "hookah.exe");
        assert!(w.downloads.borrow()[1].ends_with("x86_64-pc-windows-msvc.zip"));
    }

    #[test]
    fn nothing_is_installed_unless_strictly_newer() {
        for latest in ["1.0.0", "0.9.0"] {
            let w = serving(latest);
            assert_eq!(
                run(&w, "1.0.0", Mode::Background),
                Report::UpToDate { latest: v(latest) }
            );
            assert!(w.installed.borrow().is_empty());
            assert!(w.downloads.borrow().is_empty());
            assert_eq!(w.state.borrow().pending, None);
            assert_eq!(w.state.borrow().latest.as_deref(), Some(latest));
        }
    }

    #[test]
    fn a_minor_bump_below_one_point_zero_installs() {
        let w = serving("0.5.0");
        assert!(matches!(
            run(&w, "0.4.0", Mode::Background),
            Report::Updated { .. }
        ));
    }

    #[test]
    fn opt_out_and_dev_builds_touch_nothing() {
        let w = serving("2.0.0");
        let off = Policy {
            check: false,
            auto: true,
        };
        assert_eq!(
            run_with(&w, &env("1.0.0", off, Some(TARGET)), Mode::Install),
            Report::Disabled
        );
        assert_eq!(run(&w, "0.0.0", Mode::Install), Report::DevBuild);
        assert_eq!(w.saves.get(), 0);
        assert!(w.downloads.borrow().is_empty());
    }

    #[test]
    fn a_background_run_honors_the_throttle() {
        let w = serving("2.0.0");
        w.state
            .borrow_mut()
            .record_checked(w.now.get() - 10, &v("1.0.0"));
        assert_eq!(run(&w, "1.0.0", Mode::Background), Report::NotDue);
        assert_eq!(w.saves.get(), 0);
        w.now.set(w.now.get() + CHECK_INTERVAL_MS);
        assert!(matches!(
            run(&w, "1.0.0", Mode::Background),
            Report::Updated { .. }
        ));
    }

    #[test]
    fn a_manual_run_ignores_the_throttle() {
        let w = serving("2.0.0");
        w.state
            .borrow_mut()
            .record_checked(w.now.get(), &v("1.0.0"));
        assert!(matches!(
            run(&w, "1.0.0", Mode::Install),
            Report::Updated { .. }
        ));
    }

    #[test]
    fn a_failed_check_retries_after_an_hour() {
        let w = serving("2.0.0");
        *w.release.borrow_mut() = Err(UpdateError::failed("offline"));
        assert_eq!(
            run(&w, "1.0.0", Mode::Background),
            Report::Failed(UpdateError::failed("offline"))
        );
        assert_eq!(w.state.borrow().error.as_deref(), Some("offline"));
        assert_eq!(run(&w, "1.0.0", Mode::Background), Report::NotDue);
        w.now.set(w.now.get() + RETRY_INTERVAL_MS);
        assert!(matches!(
            run(&w, "1.0.0", Mode::Background),
            Report::Failed(_)
        ));
        assert_eq!(w.state.borrow().failures, 2);
    }

    #[test]
    fn a_held_lock_means_busy_and_changes_nothing() {
        let w = serving("2.0.0");
        w.locked.set(true);
        assert_eq!(run(&w, "1.0.0", Mode::Background), Report::Busy);
        assert_eq!(w.saves.get(), 0);
        assert!(w.downloads.borrow().is_empty());
    }

    #[test]
    fn a_lock_error_is_a_failure() {
        let mut w = serving("2.0.0");
        w.lock_fails = true;
        assert_eq!(
            run(&w, "1.0.0", Mode::Install),
            Report::Failed(UpdateError::failed("no lock"))
        );
    }

    #[test]
    fn auto_off_notifies_once_in_the_background_and_installs_by_hand() {
        let w = serving("2.0.0");
        let manual_only = Policy {
            check: true,
            auto: false,
        };
        let e = env("1.0.0", manual_only, Some(TARGET));
        assert_eq!(
            run_with(&w, &e, Mode::Background),
            Report::Available { latest: v("2.0.0") }
        );
        assert_eq!(
            w.state.borrow().pending,
            Some(Event::Available {
                version: "2.0.0".into()
            })
        );
        assert!(w.installed.borrow().is_empty());
        w.state.borrow_mut().pending = None;
        w.now.set(w.now.get() + CHECK_INTERVAL_MS);
        run_with(&w, &e, Mode::Background);
        assert_eq!(w.state.borrow().pending, None, "told once");
        // `hookah update` installs even so.
        assert!(matches!(
            run_with(&w, &e, Mode::Install),
            Report::Updated { .. }
        ));
    }

    #[test]
    fn check_only_reports_and_leaves_no_notice() {
        let w = serving("2.0.0");
        assert_eq!(
            run(&w, "1.0.0", Mode::Check),
            Report::Available { latest: v("2.0.0") }
        );
        assert!(w.installed.borrow().is_empty());
        assert!(w.downloads.borrow().is_empty());
        assert_eq!(w.state.borrow().pending, None);
    }

    #[test]
    fn a_manual_install_leaves_no_notice_for_the_next_session() {
        let w = serving("2.0.0");
        run(&w, "1.0.0", Mode::Install);
        assert_eq!(w.state.borrow().pending, None);
        assert_eq!(w.state.borrow().latest.as_deref(), Some("2.0.0"));
    }

    #[test]
    fn a_platform_without_a_release_is_blocked_and_told_once() {
        let w = serving("2.0.0");
        let e = env("1.0.0", Policy::default(), None);
        let Report::Failed(UpdateError::Blocked(reason)) = run_with(&w, &e, Mode::Background)
        else {
            panic!("expected blocked");
        };
        assert!(reason.contains("no official hookah build"));
        assert_eq!(
            w.state.borrow().pending,
            Some(Event::Failed {
                reason: reason.clone()
            })
        );
        assert!(w.downloads.borrow().is_empty());
    }

    #[test]
    fn a_missing_asset_fails_before_any_download() {
        for gone in ["SHA256SUMS", "hookah-2.0.0-aarch64-apple-darwin.tar.gz"] {
            let w = serving("2.0.0");
            if let Ok(release) = &mut *w.release.borrow_mut() {
                release.assets.retain(|a| a != gone);
            }
            let Report::Failed(UpdateError::Failed(reason)) = run(&w, "1.0.0", Mode::Install)
            else {
                panic!("expected failed");
            };
            assert!(reason.contains(gone), "{reason}");
            assert!(w.downloads.borrow().is_empty());
            assert!(w.installed.borrow().is_empty());
        }
    }

    #[test]
    fn a_blocked_preflight_stops_before_downloading() {
        let w = serving("2.0.0");
        *w.preflight.borrow_mut() = Err(UpdateError::blocked("not writable"));
        assert_eq!(
            run(&w, "1.0.0", Mode::Background),
            Report::Failed(UpdateError::blocked("not writable"))
        );
        assert!(w.downloads.borrow().is_empty());
        assert_eq!(
            w.state.borrow().pending,
            Some(Event::Failed {
                reason: "not writable".into()
            })
        );
        assert!(w.state.borrow().last_ok);
    }

    #[test]
    fn a_checksum_list_without_the_archive_or_with_garbage_fails() {
        let w = serving("2.0.0");
        w.assets.borrow_mut()[0].1 = b"deadbeef  other.tar.gz\n".to_vec();
        let Report::Failed(e) = run(&w, "1.0.0", Mode::Install) else {
            panic!("expected failed");
        };
        assert!(e.message().contains("no checksum"));
        w.assets.borrow_mut()[0].1 = vec![0xff, 0xfe];
        let Report::Failed(e) = run(&w, "1.0.0", Mode::Install) else {
            panic!("expected failed");
        };
        assert!(e.message().contains("not text"));
        assert!(w.installed.borrow().is_empty());
    }

    #[test]
    fn download_and_install_failures_are_recorded() {
        let w = serving("2.0.0");
        w.assets.borrow_mut().pop();
        assert!(matches!(run(&w, "1.0.0", Mode::Install), Report::Failed(_)));
        assert_eq!(w.state.borrow().failures, 1);
        let w = serving("2.0.0");
        *w.install.borrow_mut() = Err(UpdateError::failed("checksum mismatch"));
        assert_eq!(
            run(&w, "1.0.0", Mode::Background),
            Report::Failed(UpdateError::failed("checksum mismatch"))
        );
        assert_eq!(w.state.borrow().error.as_deref(), Some("checksum mismatch"));
        assert_eq!(w.state.borrow().pending, None, "not yet noisy");
    }

    #[test]
    fn a_save_failure_does_not_change_the_result() {
        let mut w = serving("2.0.0");
        w.save_fails = true;
        assert!(matches!(
            run(&w, "1.0.0", Mode::Install),
            Report::Updated { .. }
        ));
    }
}
