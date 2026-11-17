//! Common harness protocol.
//!
//! - `Harness`: the trait every adapter implements: `parse(event, stdin) -> Request` and
//!   `render(outcome) -> Response`.
//! - `Response`: what the CLI writes and returns (exit code, stdout, stderr).
//!
//! Also holds the shared fail-open mapping: a degraded outcome always becomes a non-blocking
//! response.
