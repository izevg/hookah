//! Process runner.
//!
//! Spawns a `CommandSpec` with a hard timeout, kills the whole process group when time runs out,
//! and captures exit status, stdout, and stderr. Does not interpret the output; ecosystem parsers
//! do.
