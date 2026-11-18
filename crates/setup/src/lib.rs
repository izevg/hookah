//! Writes and merges the hook configuration of each supported agent harness.
//!
//! Placement: this is a driven adapter. It owns the *file formats* of the harness configs
//! (JSON settings, TOML config, a plugin file), not a business rule of the domain, so it cannot
//! live in `domain` (no external crates, no I/O) or in `application` (ports only). It is a
//! separate crate rather than part of `harness` because `harness` translates hook payloads at
//! run time, while this crate edits files once, at install time, with its own JSON and TOML
//! editors; keeping them apart keeps those dependencies out of the hook-time crates. `cli` only
//! wires it.
//!
//! - [`merge`]: pure `existing text -> new text` functions, one per config format.
//! - [`io`]: where each config lives, reading, backing up, writing, and detection.
//!
//! Safety rules, binding for every harness: refuse a file that does not parse and leave it
//! untouched; add hookah's entries only when absent and update them in place otherwise; never drop
//! or reorder unrelated settings; the result is stable, so a second run changes nothing.

pub mod error;
pub mod io;
pub mod merge;
pub mod plugin;
pub mod target;

pub use error::SetupError;
pub use io::detect::{Detection, detect};
pub use io::env::{Env, Scope};
pub use io::{Plan, Written, commit, plan, snippet};
pub use plugin::{Installed, write_plugin};
pub use target::{Target, quote_program, shell_program};
