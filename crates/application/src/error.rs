//! Application errors.
//!
//! `PortError`: a port could not do its job (tool missing, I/O failure, unparseable output).
//! Use cases never turn it into a failing verdict: it becomes a degraded, non-blocking outcome.

use std::fmt;

/// A port failed. Carries a human-readable reason and nothing else; use cases only display it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortError {
    message: String,
}

impl PortError {
    /// Creates an error from a reason.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The reason.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for PortError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PortError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_and_display_carry_the_reason() {
        let e = PortError::new("tool missing");
        assert_eq!(e.message(), "tool missing");
        assert_eq!(e.to_string(), "tool missing");
    }
}
