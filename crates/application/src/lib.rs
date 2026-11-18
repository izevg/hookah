//! Application layer of `hookah`: use cases and the ports they need.
//!
//! - [`ports`]: traits the outside world implements (filesystem, processes, version control).
//! - [`usecases`]: entry points that driving adapters (harnesses, CLI) call.
//!
//! Depends only on `hookah-domain`. Never touches the filesystem or spawns processes directly.
//! Every port failure is fail-open: it degrades the outcome and never blocks the agent.

pub mod error;
pub mod ports;
pub mod usecases;

pub use error::PortError;
pub use ports::Ports;
pub use usecases::{
    CheckEdit, CheckSession, ComputeBaseline, EditRequest, Outcome, StartRequest, StartSession,
    StopRequest,
};
