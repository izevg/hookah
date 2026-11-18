//! Ports of the self-update: the release server, the installer, the updater's memory, the clock,
//! and the background launcher.
//!
//! A release is identified by its tag. Assets are fetched by name from that release only; where
//! they come from is the adapter's business and is fixed to the official repository.

use std::any::Any;
use std::fmt;

use hookah_domain::update::{ArchiveKind, UpdateState, Version};

/// An update step failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    /// A passing problem: the network, the server, a bad download. Worth trying again later.
    Failed(String),
    /// It will not work here until the user acts: a directory that is not writable, a platform
    /// without a release, a binary owned by a package manager.
    Blocked(String),
}

impl UpdateError {
    /// A passing problem.
    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }

    /// A problem that needs the user.
    pub fn blocked(message: impl Into<String>) -> Self {
        Self::Blocked(message.into())
    }

    /// The reason, in one line.
    pub fn message(&self) -> &str {
        match self {
            Self::Failed(m) | Self::Blocked(m) => m,
        }
    }
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for UpdateError {}

/// The latest stable release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Its version.
    pub version: Version,
    /// Its git tag, the key to its assets.
    pub tag: String,
    /// Names of the files attached to it.
    pub assets: Vec<String>,
}

/// The official release server.
pub trait ReleaseSource {
    /// The latest stable release: never a draft or a prerelease.
    fn latest(&self) -> Result<Release, UpdateError>;

    /// The content of an asset of the release tagged `tag`.
    fn asset(&self, tag: &str, name: &str) -> Result<Vec<u8>, UpdateError>;
}

/// A downloaded archive, ready to install.
#[derive(Debug, Clone, Copy)]
pub struct Package<'a> {
    /// The archive bytes.
    pub bytes: &'a [u8],
    /// How the archive is packed.
    pub kind: ArchiveKind,
    /// The SHA-256 the release lists for it, lowercase hex.
    pub sha256: &'a str,
    /// The executable to take out of it.
    pub entry: &'a str,
    /// The version the executable must report.
    pub version: &'a Version,
}

/// Replaces the running binary.
pub trait Installer {
    /// Whether this binary can be replaced at all: not owned by a package manager, directory
    /// writable. Checked before anything is downloaded.
    fn preflight(&self) -> Result<(), UpdateError>;

    /// Verifies the checksum, extracts the executable, makes sure it reports the expected
    /// version, and swaps it in. On any error the installed binary is untouched.
    fn install(&self, package: &Package<'_>) -> Result<(), UpdateError>;
}

/// Held while an updater runs; dropping it releases the lock.
pub type LockGuard = Box<dyn Any>;

/// The updater's memory and its lock.
pub trait UpdateStore {
    /// The stored state. An unreadable record is the default state.
    fn load(&self) -> UpdateState;

    /// Stores the state.
    fn save(&self, state: &UpdateState) -> Result<(), UpdateError>;

    /// Takes the updater lock: `None` when another updater holds it.
    fn try_lock(&self) -> Result<Option<LockGuard>, UpdateError>;
}

/// Wall-clock time.
pub trait Clock {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> u64;
}

/// Starts a background update check without waiting for it.
pub trait UpdateSpawner {
    /// Starts `hookah update --background`, detached.
    fn spawn_check(&self) -> Result<(), UpdateError>;
}

/// All update ports, borrowed. Built by the composition root.
#[derive(Clone, Copy)]
pub struct UpdatePorts<'a> {
    /// The release server.
    pub releases: &'a dyn ReleaseSource,
    /// Replaces the binary.
    pub installer: &'a dyn Installer,
    /// The updater's memory.
    pub store: &'a dyn UpdateStore,
    /// The time.
    pub clock: &'a dyn Clock,
    /// Starts background checks.
    pub spawner: &'a dyn UpdateSpawner,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_carry_their_reason() {
        assert_eq!(UpdateError::failed("x").message(), "x");
        assert_eq!(UpdateError::blocked("y").to_string(), "y");
        assert_ne!(UpdateError::failed("x"), UpdateError::blocked("x"));
    }
}
