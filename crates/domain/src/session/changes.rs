//! Changed files and changed lines.
//!
//! `ChangedLines`: which lines of a file differ from the last commit. `ChangeSet`: changed files
//! de-duplicated and grouped by project; the stop phase selects tests for each group.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::project::Project;

/// An inclusive range of 1-based line numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRange {
    start: u32,
    end: u32,
}

impl LineRange {
    /// Builds a range; `None` if `start` is 0 or `end < start`.
    pub fn new(start: u32, end: u32) -> Option<Self> {
        (start >= 1 && end >= start).then_some(Self { start, end })
    }

    /// First line of the range.
    pub fn start(self) -> u32 {
        self.start
    }

    /// Last line of the range, inclusive.
    pub fn end(self) -> u32 {
        self.end
    }

    fn contains(self, line: u32) -> bool {
        (self.start..=self.end).contains(&line)
    }
}

/// Which lines of a file changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangedLines {
    /// The whole file is new or untracked.
    All,
    /// Only these ranges changed. Empty means nothing changed.
    Ranges(Vec<LineRange>),
}

impl ChangedLines {
    /// The number of changed lines, or `None` when the whole file is new and the count is the
    /// file's own length.
    pub fn count(&self) -> Option<usize> {
        match self {
            Self::All => None,
            Self::Ranges(r) => Some(r.iter().map(|r| (r.end - r.start) as usize + 1).sum()),
        }
    }

    /// Whether the 1-based `line` is changed.
    pub fn contains(&self, line: u32) -> bool {
        match self {
            Self::All => true,
            Self::Ranges(r) => r.iter().any(|r| r.contains(line)),
        }
    }
}

/// Changed files of one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectChanges {
    /// The project the files belong to.
    pub project: Project,
    /// Absolute paths of the changed files, sorted and unique.
    pub files: BTreeSet<PathBuf>,
}

/// Changed files grouped by project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeSet {
    groups: BTreeMap<PathBuf, ProjectChanges>,
}

impl ChangeSet {
    /// Creates an empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `file` as changed in `project`. Repeated calls are idempotent.
    pub fn add(&mut self, project: &Project, file: &Path) {
        self.groups
            .entry(project.root.clone())
            .or_insert_with(|| ProjectChanges {
                project: project.clone(),
                files: BTreeSet::new(),
            })
            .files
            .insert(file.to_path_buf());
    }

    /// The groups, ordered by project root.
    pub fn groups(&self) -> impl Iterator<Item = &ProjectChanges> {
        self.groups.values()
    }

    /// Whether no file was recorded.
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::project::Ecosystem;

    #[test]
    fn counts_changed_lines() {
        let r = |a, b| LineRange::new(a, b).unwrap();
        assert_eq!(ChangedLines::All.count(), None);
        assert_eq!(ChangedLines::Ranges(vec![]).count(), Some(0));
        assert_eq!(
            ChangedLines::Ranges(vec![r(2, 4), r(9, 9)]).count(),
            Some(4)
        );
    }

    #[test]
    fn range_validation_and_contains() {
        let r = LineRange::new(2, 7).unwrap();
        assert_eq!((r.start(), r.end()), (2, 7));
        assert!(LineRange::new(0, 3).is_none());
        assert!(LineRange::new(5, 4).is_none());
        let c = ChangedLines::Ranges(vec![
            LineRange::new(2, 3).unwrap(),
            LineRange::new(10, 10).unwrap(),
        ]);
        assert!(!c.contains(1));
        assert!(c.contains(2) && c.contains(3) && c.contains(10));
        assert!(!c.contains(4) && !c.contains(11));
        assert!(ChangedLines::All.contains(999));
        assert!(!ChangedLines::Ranges(vec![]).contains(1));
    }

    #[test]
    fn change_set_groups_and_dedupes() {
        let p = |r: &str| Project {
            root: r.into(),
            ecosystem: Ecosystem::Rust,
            manifests: vec![],
        };
        let mut s = ChangeSet::new();
        assert!(s.is_empty());
        s.add(&p("/a"), Path::new("/a/x.rs"));
        s.add(&p("/a"), Path::new("/a/x.rs"));
        s.add(&p("/a"), Path::new("/a/w.rs"));
        s.add(&p("/b"), Path::new("/b/y.rs"));
        let g: Vec<_> = s.groups().collect();
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].files.len(), 2);
        assert!(!s.is_empty());
    }
}
