//! Command-hook harnesses: agents that run an external command on a hook event, passing the
//! payload on stdin and reading the verdict from the exit code, stdout, and stderr.

pub mod antigravity;
pub mod claude;
pub mod codex;
pub mod cursor;
pub mod kimi;
pub mod qoder;
