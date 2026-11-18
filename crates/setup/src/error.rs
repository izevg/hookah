//! Setup failures. Every variant carries what the user needs to fix it.

use std::fmt;
use std::path::PathBuf;

/// Why `hookah setup` could not finish.
#[derive(Debug)]
pub enum SetupError {
    /// The existing config file cannot be merged safely. The file was not touched.
    Invalid {
        /// The config file.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },
    /// The harness has no config at this scope.
    UnsupportedScope {
        /// The harness name.
        target: &'static str,
        /// The scope that was asked for.
        scope: &'static str,
        /// What to do instead.
        hint: &'static str,
    },
    /// The user-level location needs a home directory, and none is set.
    NoHome,
    /// Reading or writing failed.
    Io {
        /// The file or directory.
        path: PathBuf,
        /// The operating-system error.
        source: std::io::Error,
    },
}

impl fmt::Display for SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { path, reason } => write!(
                f,
                "cannot merge into {}: {reason}; the file was left untouched, fix or remove it and run setup again",
                path.display()
            ),
            Self::UnsupportedScope {
                target,
                scope,
                hint,
            } => write!(f, "{target} has no {scope}-level config; {hint}"),
            Self::NoHome => f.write_str(
                "cannot find the home directory (HOME or USERPROFILE is not set); use --scope project",
            ),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for SetupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
