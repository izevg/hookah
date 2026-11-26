//! File-based storage: the per-project cache directory ([`cache_dir`]), the session finding
//! baselines kept in it ([`baseline`]), and the latest check status of a project ([`status`]).

pub mod baseline;
pub mod cache_dir;
pub mod hook_feedback;
pub mod status;
