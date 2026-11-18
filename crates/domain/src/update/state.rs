//! What the updater remembers between runs: when it last checked, what it saw, and the one thing
//! the user has not been told yet.
//!
//! The background updater has no channel to the agent, so it leaves a pending [`Event`] here and
//! the next session start shows it, once.

use std::fmt;

use super::Version;

/// One check a day (24 hours, in milliseconds).
pub const CHECK_INTERVAL_MS: u64 = 86_400_000;
/// After a failed check, try again after an hour instead of a day (in milliseconds).
pub const RETRY_INTERVAL_MS: u64 = 3_600_000;
/// A failing check (network, rate limit) is shown to the user only on its third failure in a row.
pub const FAILURES_BEFORE_NOTICE: u32 = 3;

/// Something the user should hear about at the next session start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The binary was replaced.
    Updated {
        /// The version that was running.
        from: String,
        /// The version installed.
        to: String,
    },
    /// A newer version exists and was not installed (automatic updates are off).
    Available {
        /// The newer version.
        version: String,
    },
    /// Updating does not work and will not by itself.
    Failed {
        /// Why, in one line.
        reason: String,
    },
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Updated { from, to } => {
                write!(f, "updated hookah from {from} to {to}; new sessions use it")
            }
            Self::Available { version } => write!(
                f,
                "hookah {version} is available; run `hookah update` to install it"
            ),
            Self::Failed { reason } => write!(f, "cannot update: {reason}"),
        }
    }
}

/// The updater's memory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateState {
    /// When the last check ran, in milliseconds since the Unix epoch.
    pub checked_at_ms: Option<u64>,
    /// Whether that check reached the release server and understood the answer.
    pub last_ok: bool,
    /// The newest stable version seen.
    pub latest: Option<String>,
    /// The last error, for `hookah update --check`.
    pub error: Option<String>,
    /// Checks that failed in a row.
    pub failures: u32,
    /// The version the user was last told about (as available, or as blocked), so it is told once.
    pub announced: Option<String>,
    /// Not shown yet.
    pub pending: Option<Event>,
}

impl UpdateState {
    /// Whether a background check is due at `now_ms`. A clock that went backwards counts as due:
    /// waiting for a stored time in the future would stall the check indefinitely.
    pub fn due(&self, now_ms: u64) -> bool {
        match self.checked_at_ms {
            None => true,
            Some(at) if now_ms < at => true,
            Some(at) => {
                let wait = if self.last_ok {
                    CHECK_INTERVAL_MS
                } else {
                    RETRY_INTERVAL_MS
                };
                now_ms - at >= wait
            }
        }
    }

    /// A check reached the server and found `latest` as the newest stable version.
    pub fn record_checked(&mut self, now_ms: u64, latest: &Version) {
        self.checked_at_ms = Some(now_ms);
        self.last_ok = true;
        self.latest = Some(latest.to_string());
        self.error = None;
        self.failures = 0;
    }

    /// A check or install failed for a passing reason (network, server, a broken download). The
    /// user hears about it only when it keeps failing.
    pub fn record_failure(&mut self, now_ms: u64, reason: &str) {
        self.checked_at_ms = Some(now_ms);
        self.last_ok = false;
        self.error = Some(reason.to_string());
        self.failures = self.failures.saturating_add(1);
        if self.failures == FAILURES_BEFORE_NOTICE {
            self.pending = Some(Event::Failed {
                reason: format!("{reason} ({} checks in a row failed)", self.failures),
            });
        }
    }

    /// `version` is out but cannot be installed here and will not become installable by waiting
    /// (the directory is not writable, no release for this platform). Told once per version.
    pub fn record_blocked(&mut self, now_ms: u64, version: &Version, reason: &str) {
        self.record_checked(now_ms, version);
        self.error = Some(reason.to_string());
        self.announce(
            version,
            Event::Failed {
                reason: reason.to_string(),
            },
        );
    }

    /// `version` is out and, with automatic updates off, was left alone. Told once per version.
    pub fn record_available(&mut self, version: &Version) {
        self.announce(
            version,
            Event::Available {
                version: version.to_string(),
            },
        );
    }

    /// The binary was replaced.
    pub fn record_updated(&mut self, from: &Version, to: &Version) {
        self.latest = Some(to.to_string());
        self.error = None;
        self.announced = None;
        self.pending = Some(Event::Updated {
            from: from.to_string(),
            to: to.to_string(),
        });
    }

    /// The pending event, once: it is cleared.
    pub fn take_pending(&mut self) -> Option<Event> {
        self.pending.take()
    }

    fn announce(&mut self, version: &Version, event: Event) {
        let text = version.to_string();
        if self.announced.as_deref() != Some(text.as_str()) {
            self.announced = Some(text);
            self.pending = Some(event);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn checked(at: u64) -> UpdateState {
        let mut s = UpdateState::default();
        s.record_checked(at, &v("1.0.0"));
        s
    }

    #[test]
    fn a_fresh_state_is_due() {
        assert!(UpdateState::default().due(0));
    }

    #[test]
    fn after_a_good_check_the_next_one_waits_a_day() {
        let s = checked(1_000);
        assert!(!s.due(1_000));
        assert!(!s.due(1_000 + CHECK_INTERVAL_MS - 1));
        assert!(s.due(1_000 + CHECK_INTERVAL_MS));
    }

    #[test]
    fn after_a_failed_check_the_retry_waits_an_hour() {
        let mut s = UpdateState::default();
        s.record_failure(5_000, "offline");
        assert!(!s.due(5_000 + RETRY_INTERVAL_MS - 1));
        assert!(s.due(5_000 + RETRY_INTERVAL_MS));
    }

    #[test]
    fn a_clock_that_went_back_is_due() {
        assert!(checked(10_000).due(9_999));
    }

    #[test]
    fn a_check_records_what_it_saw_and_clears_the_error() {
        let mut s = UpdateState::default();
        s.record_failure(1, "offline");
        s.record_checked(2, &v("1.2.3"));
        assert_eq!(s.latest.as_deref(), Some("1.2.3"));
        assert_eq!(s.checked_at_ms, Some(2));
        assert!(s.last_ok);
        assert_eq!(s.error, None);
        assert_eq!(s.failures, 0);
    }

    #[test]
    fn a_failure_stays_quiet_until_it_keeps_happening() {
        let mut s = UpdateState::default();
        s.record_failure(1, "offline");
        s.record_failure(2, "offline");
        assert_eq!(s.pending, None);
        assert_eq!(s.error.as_deref(), Some("offline"));
        s.record_failure(3, "offline");
        let Some(Event::Failed { reason }) = s.take_pending() else {
            panic!("expected a failure event");
        };
        assert!(reason.contains("offline") && reason.contains('3'));
        // Not repeated on the fourth failure.
        s.record_failure(4, "offline");
        assert_eq!(s.pending, None);
    }

    #[test]
    fn a_success_resets_the_failure_count() {
        let mut s = UpdateState::default();
        s.record_failure(1, "x");
        s.record_failure(2, "x");
        s.record_checked(3, &v("1.0.0"));
        s.record_failure(4, "x");
        s.record_failure(5, "x");
        assert_eq!(s.pending, None);
    }

    #[test]
    fn a_blocked_version_is_announced_once() {
        let mut s = UpdateState::default();
        s.record_blocked(1, &v("2.0.0"), "directory is not writable");
        assert_eq!(
            s.take_pending(),
            Some(Event::Failed {
                reason: "directory is not writable".into()
            })
        );
        assert!(
            s.last_ok,
            "the server was reached, so the next check is a day away"
        );
        s.record_blocked(2, &v("2.0.0"), "directory is not writable");
        assert_eq!(s.pending, None);
        s.record_blocked(3, &v("2.0.1"), "directory is not writable");
        assert!(s.pending.is_some());
    }

    #[test]
    fn an_available_version_is_announced_once() {
        let mut s = UpdateState::default();
        s.record_available(&v("1.1.0"));
        assert_eq!(
            s.take_pending(),
            Some(Event::Available {
                version: "1.1.0".into()
            })
        );
        s.record_available(&v("1.1.0"));
        assert_eq!(s.take_pending(), None);
        s.record_available(&v("1.2.0"));
        assert!(s.take_pending().is_some());
    }

    #[test]
    fn an_update_replaces_a_pending_notice_and_resets_the_announcement() {
        let mut s = UpdateState::default();
        s.record_available(&v("1.1.0"));
        s.record_updated(&v("1.0.0"), &v("1.1.0"));
        assert_eq!(
            s.take_pending(),
            Some(Event::Updated {
                from: "1.0.0".into(),
                to: "1.1.0".into()
            })
        );
        assert_eq!(s.announced, None);
        assert_eq!(s.latest.as_deref(), Some("1.1.0"));
        assert_eq!(s.take_pending(), None, "shown once");
    }

    #[test]
    fn events_read_as_one_short_line() {
        let updated = Event::Updated {
            from: "1.0.0".into(),
            to: "1.1.0".into(),
        };
        assert_eq!(
            updated.to_string(),
            "updated hookah from 1.0.0 to 1.1.0; new sessions use it"
        );
        let available = Event::Available {
            version: "1.1.0".into(),
        };
        assert!(available.to_string().contains("hookah update"));
        let failed = Event::Failed {
            reason: "not writable".into(),
        };
        assert_eq!(failed.to_string(), "cannot update: not writable");
    }
}
