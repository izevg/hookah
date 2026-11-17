//! `Report`: the compact, capped rendering policy for findings.
//!
//! One `path:line: message` per finding, ordered by severity, cut off at a limit, followed by a
//! `+N more` line. Agent-agnostic; harness adapters wrap it in their own protocol.
