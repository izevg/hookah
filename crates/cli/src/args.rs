//! Command-line interface.
//!
//! - `hookah check <path>...`: run the edit phase on files directly (for humans and CI).
//! - `hookah hook <harness>`: read a harness hook payload from stdin and answer in that
//!   harness's protocol.
//! - `hookah detect <path>`: print the detected project and toolchain, for debugging.
//! - `--report-limit <N>` (global): cap on findings shown per report.
//!
//! `hookah init`, a post-MVP setup wizard, is not implemented yet.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use hookah_domain::verdict::DEFAULT_LIMIT;

/// Run format, lint, and related tests on files an AI coding agent edits.
#[derive(Debug, Parser)]
#[command(name = "hookah", version, about)]
pub struct Cli {
    /// Maximum number of findings shown per report.
    #[arg(long, global = true, value_name = "N", default_value_t = DEFAULT_LIMIT)]
    pub report_limit: usize,

    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// The subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the edit phase on files; exits 1 if any check fails.
    Check {
        /// Files to check.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// Answer an agent harness hook: payload on stdin, response on stdout/stderr.
    Hook {
        /// The harness that sent the payload.
        harness: HarnessKind,
    },
    /// Print the project and toolchain found for a path.
    Detect {
        /// File or directory to inspect.
        path: PathBuf,
    },
}

/// Supported agent harnesses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum HarnessKind {
    /// Claude Code command hooks.
    Claude,
    /// Codex CLI command hooks.
    Codex,
    /// Qoder CLI command hooks.
    Qoder,
    /// Kimi Code command hooks.
    Kimi,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn parses_check_with_global_limit() {
        let cli = Cli::try_parse_from(["hookah", "check", "a.rs", "b.rs", "--report-limit", "3"])
            .unwrap();
        assert_eq!(cli.report_limit, 3);
        assert!(matches!(cli.command, Command::Check { paths } if paths.len() == 2));
    }

    #[test]
    fn limit_defaults_to_domain_default() {
        let cli = Cli::try_parse_from(["hookah", "hook", "claude"]).unwrap();
        assert_eq!(cli.report_limit, DEFAULT_LIMIT);
        assert!(matches!(
            cli.command,
            Command::Hook {
                harness: HarnessKind::Claude
            }
        ));
    }

    #[test]
    fn check_needs_a_path() {
        assert!(Cli::try_parse_from(["hookah", "check"]).is_err());
    }
}
