//! The finding entity.
//!
//! `Finding`: one problem reported by a check. It has a location, severity, message, optional
//! rule id, and the check kind that produced it. This is the only shape tool output takes once it
//! leaves an adapter.

pub mod baseline;
pub mod fingerprint;
pub mod location;
