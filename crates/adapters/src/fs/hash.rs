//! `ContentHasher` over file bytes.

use std::hash::{DefaultHasher, Hasher};
use std::path::Path;

use hookah_application::error::PortError;
use hookah_application::ports::ContentHasher;

/// Hashes file content with the std hasher. Values are only comparable within one process.
#[derive(Debug, Clone, Copy, Default)]
pub struct FileHasher;

impl ContentHasher for FileHasher {
    fn content_hash(&self, file: &Path) -> Result<u64, PortError> {
        let bytes = std::fs::read(file)
            .map_err(|e| PortError::new(format!("cannot read {}: {e}", file.display())))?;
        let mut hasher = DefaultHasher::new();
        hasher.write(&bytes);
        Ok(hasher.finish())
    }
}
