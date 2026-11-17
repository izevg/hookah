//! Cache directory layout. Post-MVP.
//!
//! Resolves `$XDG_CACHE_HOME/hookah` (or the platform equivalent) and lays out one subdirectory
//! per project, keyed by a hash of the project root. Creates directories with `0700` permissions.
