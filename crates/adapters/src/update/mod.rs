//! Driven adapters of the self-update.
//!
//! - [`github`]: the official releases, over HTTPS, trusting the system certificate store.
//! - [`install`]: checksum, extraction, version check, and the atomic swap of the executable.
//! - [`store`]: the updater's state and lock in the cache directory.
//! - [`settings`]: `HOOKAH_NO_UPDATE` and the `[update]` table of the user config.
//!
//! The clock is here too: [`SystemClock`].

pub mod github;
pub mod install;
pub mod settings;
pub mod store;

use std::time::{SystemTime, UNIX_EPOCH};

use hookah_application::ports::Clock;

/// The system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_tells_a_recent_time() {
        // After 2024-01-01 and not absurdly far ahead.
        let now = SystemClock.now_ms();
        assert!(now > 1_704_067_200_000);
        assert!(now < 32_503_680_000_000);
    }
}
