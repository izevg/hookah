//! `Fingerprint`: a stable identity for a finding. Post-MVP.
//!
//! Built from path, rule id, and normalized message, without the line number, so the same
//! pre-existing problem still matches after unrelated edits shift its line. Needed only once a
//! baseline exists.
