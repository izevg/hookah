//! `hookah` binary entry point.
//!
//! Parses arguments, builds the object graph, runs the command, and maps the result to an exit
//! code. Wraps everything in the fail-open guard: a panic, an internal error, or the global
//! deadline yields a warning and a non-blocking exit, never a stuck or blocked agent.

mod args;
mod render;
mod wiring;

fn main() {}
