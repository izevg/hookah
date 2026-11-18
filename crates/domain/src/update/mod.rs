//! Self-update rules: which release counts as newer, which archive belongs to this binary, when
//! the next check is due, and what the user is told once. Pure: no clock, no network, no files.
//!
//! - [`version`]: strict `X.Y.Z` versions and their order.
//! - [`target`]: the six official release targets, archive names, and `SHA256SUMS` lookup.
//! - [`state`]: what the updater remembers between runs, and the throttle.
//!
//! Policy in one place:
//!
//! - A development build (`0.0.0`, what a local or `cargo install --git` build reports) never
//!   updates itself.
//! - Only a strictly greater stable version is installed. A prerelease is never a candidate.
//! - A 0.x minor bump installs like any other bump. Below 1.0 semver treats a minor as breaking,
//!   but hookah's hook protocol stays backward compatible, so an installed hook keeps working.
//! - A binary that a package manager owns (Cargo, Homebrew, mise, Nix) is left alone.

pub mod state;
pub mod target;
pub mod version;

use std::path::Path;

pub use state::{Event, UpdateState};
pub use target::{ArchiveKind, Target, checksum_of};
pub use version::{Version, VersionError};

/// The repository releases come from. Fixed: nothing redirects an update to another source.
pub const REPOSITORY: &str = "izevg/hookah";

/// What the user allowed. Both on by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Look for new releases at all. Off disables everything, the manual command included.
    pub check: bool,
    /// Install a newer release without asking. Off means check and notify only.
    pub auto: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            check: true,
            auto: true,
        }
    }
}

/// What to do about a release found online.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// This build is a development build.
    DevBuild,
    /// The release is not newer than the running version.
    UpToDate,
    /// Newer, but the policy asks only for a notice.
    Notify,
    /// Newer: install it.
    Install,
}

/// Compares the running version with the latest stable release under `policy`.
pub fn decide(current: &Version, latest: &Version, policy: Policy) -> Decision {
    if current.is_dev() {
        Decision::DevBuild
    } else if latest <= current {
        Decision::UpToDate
    } else if policy.auto {
        Decision::Install
    } else {
        Decision::Notify
    }
}

/// The package manager that owns the binary at `exe`, if its path says so. Such a binary is
/// updated by that tool, and replacing it behind its back would break its bookkeeping.
pub fn managed_by(exe: &Path) -> Option<&'static str> {
    let parts: Vec<String> = exe
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect();
    let has = |name: &str| parts.iter().any(|p| p == name);
    let adjacent = |a: &str, b: &str| parts.windows(2).any(|w| w[0] == a && w[1] == b);
    if adjacent(".cargo", "bin") {
        Some("Cargo")
    } else if has("cellar") || has("homebrew") || has("linuxbrew") {
        Some("Homebrew")
    } else if has("mise") && has("installs") || adjacent("mise", "shims") {
        Some("mise")
    } else if adjacent("nix", "store") {
        Some("Nix")
    } else {
        None
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn a_dev_build_never_updates() {
        assert_eq!(
            decide(&v("0.0.0"), &v("9.9.9"), Policy::default()),
            Decision::DevBuild
        );
    }

    #[test]
    fn only_a_strictly_greater_version_is_a_candidate() {
        let p = Policy::default();
        assert_eq!(decide(&v("1.2.3"), &v("1.2.3"), p), Decision::UpToDate);
        assert_eq!(decide(&v("1.2.3"), &v("1.2.2"), p), Decision::UpToDate);
        assert_eq!(decide(&v("1.2.3"), &v("1.2.4"), p), Decision::Install);
        assert_eq!(decide(&v("0.4.0"), &v("0.5.0"), p), Decision::Install);
        assert_eq!(decide(&v("0.4.0"), &v("1.0.0"), p), Decision::Install);
    }

    #[test]
    fn auto_off_only_notifies() {
        let p = Policy {
            check: true,
            auto: false,
        };
        assert_eq!(decide(&v("1.0.0"), &v("1.0.1"), p), Decision::Notify);
        assert_eq!(decide(&v("1.0.1"), &v("1.0.1"), p), Decision::UpToDate);
    }

    #[test]
    fn package_managed_paths_are_recognized() {
        let by = |p: &str| managed_by(Path::new(p));
        assert_eq!(by("/home/u/.cargo/bin/hookah"), Some("Cargo"));
        assert_eq!(
            by("/opt/homebrew/Cellar/hookah/1.0.0/bin/hookah"),
            Some("Homebrew")
        );
        assert_eq!(
            by("/home/linuxbrew/.linuxbrew/bin/hookah"),
            Some("Homebrew")
        );
        assert_eq!(
            by("/home/u/.local/share/mise/installs/hookah/1.0.0/hookah"),
            Some("mise")
        );
        assert_eq!(by("/home/u/.local/share/mise/shims/hookah"), Some("mise"));
        assert_eq!(by("/nix/store/abc-hookah/bin/hookah"), Some("Nix"));
        assert_eq!(by("/opt/homebrew/bin/hookah"), Some("Homebrew"));
        assert_eq!(
            by("/usr/local/Cellar/hookah/1.0.0/bin/hookah"),
            Some("Homebrew")
        );
        assert_eq!(by("/home/u/.local/bin/hookah"), None);
        assert_eq!(by("/home/u/mise/other/hookah"), None);
        assert_eq!(by("/usr/local/bin/hookah"), None);
    }
}
