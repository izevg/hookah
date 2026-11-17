//! `SessionStore` on disk. Post-MVP.
//!
//! One file per session id, holding its change set. Stale sessions are pruned by age.
