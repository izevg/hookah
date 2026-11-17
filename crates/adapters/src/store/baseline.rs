//! `BaselineStore` on disk.
//!
//! One file per project and source file, holding finding fingerprints. Writes are atomic
//! (write to a temp file, then rename).
