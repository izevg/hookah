//! Domain errors.
//!
//! `DomainError`: invariant violations of the model itself (for example, a file outside its
//! project root, or an empty toolchain asked to run a check). Never wraps I/O errors.
