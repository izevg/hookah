//! `Tool` and `CommandSpec` value objects.
//!
//! A `Tool` is one configured program (`biome`, `ruff`, `clippy`, `pytest`, ...): its name, the
//! checks it serves, and a `CommandSpec` (program, arguments, working directory, timeout). A
//! `CommandSpec` only describes a command; the `CheckRunner` port executes it.
