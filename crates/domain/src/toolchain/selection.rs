//! Test selection.
//!
//! `TestSelection`: either the specific tests related to a set of changed files, or `None`. The
//! rule: if no related test can be found, run nothing. Never fall back to the full suite.
