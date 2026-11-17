//! Project location rules.
//!
//! "Nearest manifest wins": given the changed file and the manifests found on the way up from
//! it, pick the project. A pure function over candidate paths; the actual directory walk is the
//! `ProjectLocator` port's job.

use std::path::Path;

use super::{Manifest, Project};

/// Picks the project for `file` from candidate manifests.
///
/// Only manifests whose directory is an ancestor of `file` count. The deepest directory wins.
/// If several manifests share that directory, the ecosystem that can check `file` is preferred,
/// then the first in ecosystem order. Returns `None` when no candidate applies.
pub fn locate_project(file: &Path, candidates: &[Manifest]) -> Option<Project> {
    let depth = |m: &Manifest| m.dir().components().count();
    let applicable: Vec<&Manifest> = candidates
        .iter()
        .filter(|m| file.starts_with(m.dir()))
        .collect();
    let nearest = applicable.iter().map(|m| depth(m)).max()?;
    let mut manifests: Vec<Manifest> = applicable
        .into_iter()
        .filter(|m| depth(m) == nearest)
        .cloned()
        .collect();
    manifests.sort();
    let primary = manifests
        .iter()
        .find(|m| m.ecosystem.can_check(file))
        .or(manifests.first())?;
    Some(Project {
        root: primary.dir().to_path_buf(),
        ecosystem: primary.ecosystem,
        manifests: manifests.clone(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::Ecosystem;
    use super::*;

    fn m(p: &str) -> Manifest {
        Manifest::from_path(Path::new(p)).unwrap()
    }

    #[test]
    fn nearest_wins() {
        let c = [m("/r/package.json"), m("/r/pkg/package.json")];
        let p = locate_project(Path::new("/r/pkg/src/a.ts"), &c).unwrap();
        assert_eq!(p.root, Path::new("/r/pkg"));
    }

    #[test]
    fn ignores_non_ancestors() {
        let c = [m("/r/other/Cargo.toml")];
        assert!(locate_project(Path::new("/r/pkg/a.rs"), &c).is_none());
        assert!(locate_project(Path::new("/r/pkg/a.rs"), &[]).is_none());
    }

    #[test]
    fn same_dir_prefers_ecosystem_that_checks_file() {
        let c = [m("/r/package.json"), m("/r/Cargo.toml")];
        let p = locate_project(Path::new("/r/src/a.rs"), &c).unwrap();
        assert_eq!(p.ecosystem, Ecosystem::Rust);
        assert_eq!(p.manifests.len(), 2);
        let p = locate_project(Path::new("/r/src/a.ts"), &c).unwrap();
        assert_eq!(p.ecosystem, Ecosystem::Node);
    }
}
