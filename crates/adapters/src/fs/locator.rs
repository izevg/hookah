//! `ProjectLocator` over the real filesystem.
//!
//! Walks parent directories from the changed file and stops at the VCS root, the home directory,
//! or the filesystem root. Results are cached by manifest mtime so repeated hook calls skip the
//! walk.
