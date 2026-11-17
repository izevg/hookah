//! Use case input and output.
//!
//! `EditRequest` (session id, changed file) and `StopRequest` (session id) come in, and an
//! `Outcome` (verdict, report, notices) goes out. Harness adapters translate to and from these.
