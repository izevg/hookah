//! Use cases: one per agent event `hookah` reacts to.

pub mod baseline;
pub mod check_edit;
pub mod check_session;
pub mod request;
pub mod status;

pub use baseline::{ComputeBaseline, StartSession};
pub use check_edit::CheckEdit;
pub use check_session::CheckSession;
pub use request::{EditRequest, Outcome, StartRequest, StopRequest};
pub use status::{ProjectStatus, ReadStatus};
