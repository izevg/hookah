//! Process execution adapters: the command runner and the background baseline spawner.

pub mod background;
pub mod runner;
mod tree;

pub use background::BackgroundSpawner;
pub use runner::ProcessRunner;
