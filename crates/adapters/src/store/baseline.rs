//! `BaselineStore` on disk. Post-MVP.
//!
//! One file per project and source file, holding finding fingerprints. Writes are atomic
//! (write to a temp file, then rename).
