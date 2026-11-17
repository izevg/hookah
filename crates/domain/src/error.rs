//! Domain errors.
//!
//! `DomainError`: invariant violations of the model itself, such as a file outside its project
//! root. Never wraps I/O errors.

use std::fmt;
use std::path::PathBuf;

/// An invariant of the domain model was violated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainError {
    /// A file was asked to be treated as part of a project that does not contain it.
    FileOutsideProject {
        /// The offending file.
        file: PathBuf,
        /// The project root it was expected under.
        root: PathBuf,
    },
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileOutsideProject { file, root } => write!(
                f,
                "file {} is outside project root {}",
                file.display(),
                root.display()
            ),
        }
    }
}

impl std::error::Error for DomainError {}
