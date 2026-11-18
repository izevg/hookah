//! Process execution adapters: the command runner and the background baseline spawner.

pub mod background;
pub mod runner;

pub use background::BackgroundSpawner;
pub use runner::ProcessRunner;
