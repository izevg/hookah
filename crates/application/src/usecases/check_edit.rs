//! `CheckEdit`: the edit phase.
//!
//! Locate the project, detect the toolchain, record the file in the session, take a baseline,
//! then format and lint the file. Return only the new findings, within the edit time budget.
