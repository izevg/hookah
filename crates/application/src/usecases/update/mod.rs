//! Self-update use cases.
//!
//! - [`ScheduleUpdate`]: the session-start side. Hands back the one thing the user has not been
//!   told yet and starts a background check when one is due. Never waits for the network.
//! - [`RunUpdate`]: the check itself, in the background (`hookah update --background`) or by hand
//!   (`hookah update`, `hookah update --check`).

mod run;
mod schedule;

use hookah_domain::update::{Policy, Target, Version};

pub use run::{Mode, Report, RunUpdate};
pub use schedule::ScheduleUpdate;

/// The facts about the running binary and the user's wishes, resolved by the composition root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateEnv {
    /// The version of the running binary.
    pub current: Version,
    /// The release target of the running binary; `None` when no official build exists for it.
    pub target: Option<Target>,
    /// What the user allowed.
    pub policy: Policy,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod fakes {
    //! One in-memory world for every update port.

    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use hookah_domain::update::{UpdateState, Version};

    use super::*;
    use crate::ports::{
        Clock, Installer, LockGuard, Package, Release, ReleaseSource, UpdateError, UpdatePorts,
        UpdateSpawner, UpdateStore,
    };

    pub struct World {
        pub now: Cell<u64>,
        pub state: RefCell<UpdateState>,
        pub saves: Cell<u32>,
        pub save_fails: bool,
        pub locked: Rc<Cell<bool>>,
        pub lock_fails: bool,
        pub release: RefCell<Result<Release, UpdateError>>,
        /// Asset contents by name; a missing name fails the download.
        pub assets: RefCell<Vec<(String, Vec<u8>)>>,
        pub downloads: RefCell<Vec<String>>,
        pub preflight: RefCell<Result<(), UpdateError>>,
        pub install: RefCell<Result<(), UpdateError>>,
        /// `(sha256, entry, version)` of every install.
        pub installed: RefCell<Vec<(String, String, String)>>,
        pub spawns: Cell<u32>,
        pub spawn_fails: bool,
        /// Whether the lock was held while installing.
        pub lock_held_during_install: Cell<bool>,
    }

    struct Held(Rc<Cell<bool>>);

    impl Drop for Held {
        fn drop(&mut self) {
            self.0.set(false);
        }
    }

    pub fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    pub const SUMS_HASH: &str = "ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12";

    impl World {
        /// A world whose latest release is `latest`, with every asset of `target` present.
        pub fn serving(latest: &str, target: Target) -> Self {
            let version = v(latest);
            let archive = target.archive_name(&version);
            let sums = format!("{SUMS_HASH}  {archive}\n");
            Self {
                now: Cell::new(1_000_000),
                state: RefCell::new(UpdateState::default()),
                saves: Cell::new(0),
                save_fails: false,
                locked: Rc::new(Cell::new(false)),
                lock_fails: false,
                release: RefCell::new(Ok(Release {
                    version,
                    tag: format!("v{latest}"),
                    assets: vec![archive.clone(), "SHA256SUMS".into()],
                })),
                assets: RefCell::new(vec![
                    ("SHA256SUMS".into(), sums.into_bytes()),
                    (archive, b"archive".to_vec()),
                ]),
                downloads: RefCell::new(vec![]),
                preflight: RefCell::new(Ok(())),
                install: RefCell::new(Ok(())),
                installed: RefCell::new(vec![]),
                spawns: Cell::new(0),
                spawn_fails: false,
                lock_held_during_install: Cell::new(false),
            }
        }

        pub fn ports(&self) -> UpdatePorts<'_> {
            UpdatePorts {
                releases: self,
                installer: self,
                store: self,
                clock: self,
                spawner: self,
            }
        }
    }

    impl ReleaseSource for World {
        fn latest(&self) -> Result<Release, UpdateError> {
            self.release.borrow().clone()
        }

        fn asset(&self, tag: &str, name: &str) -> Result<Vec<u8>, UpdateError> {
            self.downloads.borrow_mut().push(format!("{tag}/{name}"));
            self.assets
                .borrow()
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, b)| b.clone())
                .ok_or_else(|| UpdateError::failed(format!("download of {name} failed")))
        }
    }

    impl Installer for World {
        fn preflight(&self) -> Result<(), UpdateError> {
            self.preflight.borrow().clone()
        }

        fn install(&self, package: &Package<'_>) -> Result<(), UpdateError> {
            self.lock_held_during_install.set(self.locked.get());
            self.installed.borrow_mut().push((
                package.sha256.to_string(),
                package.entry.to_string(),
                package.version.to_string(),
            ));
            self.install.borrow().clone()
        }
    }

    impl UpdateStore for World {
        fn load(&self) -> UpdateState {
            self.state.borrow().clone()
        }

        fn save(&self, state: &UpdateState) -> Result<(), UpdateError> {
            if self.save_fails {
                return Err(UpdateError::failed("store unavailable"));
            }
            self.saves.set(self.saves.get() + 1);
            *self.state.borrow_mut() = state.clone();
            Ok(())
        }

        fn try_lock(&self) -> Result<Option<LockGuard>, UpdateError> {
            if self.lock_fails {
                return Err(UpdateError::failed("no lock"));
            }
            if self.locked.get() {
                return Ok(None);
            }
            self.locked.set(true);
            Ok(Some(Box::new(Held(Rc::clone(&self.locked)))))
        }
    }

    impl Clock for World {
        fn now_ms(&self) -> u64 {
            self.now.get()
        }
    }

    impl UpdateSpawner for World {
        fn spawn_check(&self) -> Result<(), UpdateError> {
            if self.spawn_fails {
                return Err(UpdateError::failed("cannot spawn"));
            }
            self.spawns.set(self.spawns.get() + 1);
            Ok(())
        }
    }
}
