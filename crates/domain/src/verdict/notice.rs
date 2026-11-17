//! Notices: messages for the agent that are not findings.
//!
//! The main one is `FileRewritten`: a formatter changed the file, so the agent must re-read it
//! before the next edit. Degradation warnings (timeout, missing tool) are notices too.
