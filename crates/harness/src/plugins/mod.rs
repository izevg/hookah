//! Plugin harnesses: agents that have no command hooks, only an in-process plugin API. hookah
//! ships a small plugin for each; the plugin calls the `hookah` binary and exchanges one
//! versioned JSON document each way.

pub mod hermes;
pub mod openclaw;
pub mod pi;
