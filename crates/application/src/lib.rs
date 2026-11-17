//! Application layer of `hookah`: use cases and the ports they need.
//!
//! - [`ports`]: traits the outside world implements (filesystem, processes, storage).
//! - [`usecases`]: entry points that driving adapters (harnesses, CLI) call.
//!
//! Depends only on `hookah-domain`. Never touches the filesystem or spawns processes directly.

pub mod error;
pub mod ports;
pub mod usecases;
