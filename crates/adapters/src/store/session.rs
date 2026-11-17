//! `SessionStore` on disk.
//!
//! One file per session id, holding its change set. Stale sessions are pruned by age.
