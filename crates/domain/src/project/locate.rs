//! Project location rules.
//!
//! "Nearest manifest wins": given the changed file and the manifests found on the way up to the
//! filesystem or VCS boundary, pick the project root. A pure function over candidate paths; the
//! actual directory walk is the `ProjectLocator` port's job.
