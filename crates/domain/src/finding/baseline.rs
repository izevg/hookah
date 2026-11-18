//! `Baseline` and the "only new findings" rule.
//!
//! A baseline is a multiset of fingerprints: the static findings (format, lint, typecheck,
//! architecture, unused) that existed when the agent session started. Comparing a fresh run with it
//! gives the findings the agent introduced. Counts matter: with two occurrences in the baseline
//! and three now, exactly one is new, and it is the last one by position.
//!
//! `Test` and `Mutation` findings are never compared: their scope is already narrow.

use std::collections::BTreeMap;

use super::Finding;
use super::fingerprint::Fingerprint;
use crate::toolchain::CheckKind;

/// Fingerprint counts of the findings that existed before the agent's change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Baseline {
    counts: BTreeMap<Fingerprint, u32>,
}

impl Baseline {
    /// Whether findings of `kind` are compared against a baseline.
    pub fn covers(kind: CheckKind) -> bool {
        matches!(
            kind,
            CheckKind::Format
                | CheckKind::Lint
                | CheckKind::Typecheck
                | CheckKind::Architecture
                | CheckKind::Unused
        )
    }

    /// Counts the fingerprints of the findings it covers.
    pub fn from_findings<'a>(findings: impl IntoIterator<Item = &'a Finding>) -> Self {
        let mut counts = BTreeMap::new();
        for finding in findings.into_iter().filter(|f| Self::covers(f.kind)) {
            *counts.entry(Fingerprint::of(finding)).or_insert(0) += 1;
        }
        Self { counts }
    }

    /// Rebuilds a baseline from stored `(fingerprint, count)` pairs; zero counts are dropped.
    pub fn from_counts(counts: impl IntoIterator<Item = (Fingerprint, u32)>) -> Self {
        let mut map = BTreeMap::new();
        for (fingerprint, count) in counts.into_iter().filter(|(_, c)| *c > 0) {
            *map.entry(fingerprint).or_insert(0) += count;
        }
        Self { counts: map }
    }

    /// The stored form: every fingerprint with its count.
    pub fn counts(&self) -> impl Iterator<Item = (&Fingerprint, u32)> {
        self.counts.iter().map(|(f, c)| (f, *c))
    }

    /// How many times `fingerprint` occurred.
    pub fn count(&self, fingerprint: &Fingerprint) -> u32 {
        self.counts.get(fingerprint).copied().unwrap_or(0)
    }

    /// Whether the baseline holds no findings.
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// The findings this change introduced, in their original order: findings that the baseline
    /// does not cover pass through; for the others, each fingerprint keeps only the occurrences
    /// beyond its baseline count, the last ones by line and column first.
    pub fn introduced(&self, findings: Vec<Finding>) -> Vec<Finding> {
        let mut groups: BTreeMap<Fingerprint, Vec<usize>> = BTreeMap::new();
        for (i, finding) in findings.iter().enumerate() {
            if Self::covers(finding.kind) {
                groups.entry(Fingerprint::of(finding)).or_default().push(i);
            }
        }
        let mut hidden = vec![false; findings.len()];
        for (fingerprint, mut indices) in groups {
            indices.sort_by_key(|&i| (findings[i].location.line, findings[i].location.column, i));
            let known = (self.count(&fingerprint) as usize).min(indices.len());
            for &i in &indices[..known] {
                hidden[i] = true;
            }
        }
        findings
            .into_iter()
            .zip(hidden)
            .filter_map(|(finding, hide)| (!hide).then_some(finding))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::finding::{Location, Severity};

    fn at(kind: CheckKind, path: &str, line: u32, msg: &str) -> Finding {
        Finding::new(kind, Severity::Error, Location::line(path, line), msg)
    }

    fn lint(path: &str, line: u32, msg: &str) -> Finding {
        at(CheckKind::Lint, path, line, msg)
    }

    #[test]
    fn only_static_kinds_are_covered() {
        for kind in [
            CheckKind::Format,
            CheckKind::Lint,
            CheckKind::Typecheck,
            CheckKind::Architecture,
            CheckKind::Unused,
        ] {
            assert!(Baseline::covers(kind));
        }
        assert!(!Baseline::covers(CheckKind::Test));
        assert!(!Baseline::covers(CheckKind::Mutation));
    }

    #[test]
    fn counts_covered_findings_only() {
        let findings = [
            lint("a.rs", 1, "m"),
            lint("a.rs", 9, "m"),
            at(CheckKind::Test, "a.rs", 1, "m"),
        ];
        let base = Baseline::from_findings(&findings);
        assert_eq!(base.count(&Fingerprint::of(&findings[0])), 2);
        assert_eq!(base.count(&Fingerprint::of(&findings[2])), 0);
        assert_eq!(base.counts().count(), 1);
        assert!(!base.is_empty());
        assert!(Baseline::default().is_empty());
    }

    #[test]
    fn from_counts_merges_and_drops_zero() {
        let a = Fingerprint::from_stored("a");
        let b = Fingerprint::from_stored("b");
        let base = Baseline::from_counts([(a.clone(), 1), (a.clone(), 2), (b.clone(), 0)]);
        assert_eq!(base.count(&a), 3);
        assert_eq!(base.count(&b), 0);
        assert_eq!(base.counts().count(), 1);
    }

    #[test]
    fn known_findings_are_hidden_anywhere_and_any_severity() {
        let old = lint("a.rs", 3, "old");
        let base = Baseline::from_findings([&old]);
        let mut shifted = lint("a.rs", 40, "old");
        shifted.severity = Severity::Warning;
        assert!(base.introduced(vec![shifted]).is_empty());
        assert!(base.introduced(vec![old]).is_empty());
    }

    #[test]
    fn new_findings_are_kept_in_order() {
        let base = Baseline::from_findings([&lint("a.rs", 3, "old")]);
        let out = base.introduced(vec![
            lint("b.rs", 1, "new"),
            lint("a.rs", 3, "old"),
            lint("a.rs", 5, "other"),
        ]);
        assert_eq!(out, vec![lint("b.rs", 1, "new"), lint("a.rs", 5, "other")]);
    }

    #[test]
    fn a_new_occurrence_of_a_known_fingerprint_is_reported_as_the_last_by_line() {
        let base = Baseline::from_findings([&lint("a.rs", 1, "m"), &lint("a.rs", 2, "m")]);
        let out = base.introduced(vec![
            lint("a.rs", 30, "m"),
            lint("a.rs", 2, "m"),
            lint("a.rs", 1, "m"),
        ]);
        assert_eq!(out, vec![lint("a.rs", 30, "m")]);
        let out = base.introduced(vec![
            lint("a.rs", 1, "m"),
            lint("a.rs", 5, "m"),
            lint("a.rs", 9, "m"),
        ]);
        assert_eq!(out, vec![lint("a.rs", 9, "m")]);
    }

    #[test]
    fn fewer_occurrences_than_the_baseline_report_nothing() {
        let base = Baseline::from_findings([&lint("a.rs", 1, "m"), &lint("a.rs", 2, "m")]);
        assert!(base.introduced(vec![lint("a.rs", 1, "m")]).is_empty());
    }

    #[test]
    fn ties_by_line_resolve_by_column() {
        let base = Baseline::from_findings([&lint("a.rs", 1, "m")]);
        let mut early = lint("a.rs", 4, "m");
        early.location.column = Some(1);
        let mut late = lint("a.rs", 4, "m");
        late.location.column = Some(9);
        let out = base.introduced(vec![late.clone(), early.clone()]);
        assert_eq!(out, vec![late.clone()]);
        let out = base.introduced(vec![early, late.clone()]);
        assert_eq!(out, vec![late]);
    }

    #[test]
    fn identical_findings_tie_by_input_order() {
        let base = Baseline::from_findings([&lint("a.rs", 1, "m")]);
        let out = base.introduced(vec![lint("a.rs", 1, "m"), lint("a.rs", 1, "m")]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn test_and_mutation_findings_pass_through_even_when_identical() {
        let t = at(CheckKind::Test, "a.rs", 1, "m");
        let m = at(CheckKind::Mutation, "a.rs", 1, "m");
        let base = Baseline::from_findings([&t, &m]);
        assert_eq!(base.introduced(vec![t.clone(), m.clone()]), vec![t, m]);
    }

    #[test]
    fn kinds_are_separate_identities() {
        let base = Baseline::from_findings([&lint("a.rs", 1, "m")]);
        let ty = at(CheckKind::Typecheck, "a.rs", 1, "m");
        assert_eq!(base.introduced(vec![ty.clone()]), vec![ty]);
    }
}
