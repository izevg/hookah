//! Execution ports.
//!
//! - `CheckRunner`: run one command with its targets inside its timeout and capture the output.
//! - `ContentHasher`: hash file content, to detect that a formatter rewrote a file.

use std::path::Path;

use hookah_domain::toolchain::CommandSpec;

use crate::error::PortError;

/// What a finished (or killed) process produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProcessOutput {
    /// Exit code; `None` if the process was killed by a signal or by the timeout.
    pub exit_code: Option<i32>,
    /// Captured standard output, lossily decoded.
    pub stdout: String,
    /// Captured standard error, lossily decoded.
    pub stderr: String,
    /// The process ran out of time and its process group was killed.
    pub timed_out: bool,
}

impl ProcessOutput {
    /// Whether the process exited with code 0 in time.
    pub fn success(&self) -> bool {
        !self.timed_out && self.exit_code == Some(0)
    }
}

/// Runs external commands.
pub trait CheckRunner {
    /// Runs `spec` with [`CommandSpec::expand_args`]`(targets)`, honoring `spec.timeout`. A
    /// non-zero exit or a timeout is a normal [`ProcessOutput`]; an error means the process could
    /// not be started or observed (for example, the program is missing).
    fn run(&self, spec: &CommandSpec, targets: &[String]) -> Result<ProcessOutput, PortError>;
}

/// Hashes file content.
pub trait ContentHasher {
    /// A hash of the file's bytes; equal content gives an equal hash within one process.
    fn content_hash(&self, file: &Path) -> Result<u64, PortError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(exit_code: Option<i32>, timed_out: bool) -> ProcessOutput {
        ProcessOutput {
            exit_code,
            timed_out,
            ..ProcessOutput::default()
        }
    }

    #[test]
    fn success_needs_exit_zero_in_time() {
        assert!(output(Some(0), false).success());
        assert!(!output(Some(1), false).success());
        assert!(!output(None, false).success());
        assert!(!output(Some(0), true).success());
        assert!(!output(None, true).success());
    }
}
