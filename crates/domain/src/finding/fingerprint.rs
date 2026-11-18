//! `Fingerprint`: a stable identity for a finding.
//!
//! Built from the check kind, the project-relative path, the rule id, and a normalized message.
//! It carries no line or column, so the same pre-existing problem still matches after unrelated
//! edits shift its line.
//!
//! Message normalization ([`normalize_message`]):
//! - `:<line>` and `:<line>:<column>` references are removed;
//! - the word pairs `line N`, `lines N`, `column N`, and `col N` are removed;
//! - whitespace runs collapse to one space.
//!
//! Numeric literals and quoted identifiers are kept as they are: masking them would merge
//! findings that are different problems (`expected 2 arguments` and `expected 3 arguments`).

use std::fmt;

use super::Finding;

/// The identity of a finding, as a canonical single-line string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Fingerprint(String);

impl Fingerprint {
    /// The fingerprint of `finding`: `<kind>|<path>|<rule>|<normalized message>`.
    pub fn of(finding: &Finding) -> Self {
        Self(format!(
            "{}|{}|{}|{}",
            finding.kind.name(),
            finding.location.path.to_string_lossy(),
            finding.rule.as_deref().unwrap_or(""),
            normalize_message(&finding.message)
        ))
    }

    /// Rebuilds a fingerprint from its stored form.
    pub fn from_stored(stored: impl Into<String>) -> Self {
        Self(stored.into())
    }

    /// The canonical string, for storage.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Strips line and column references and collapses whitespace; see the module docs.
pub fn normalize_message(message: &str) -> String {
    let stripped = strip_position_suffixes(message);
    let mut words: Vec<&str> = Vec::new();
    let mut tokens = stripped.split_whitespace().peekable();
    while let Some(word) = tokens.next() {
        let is_label = matches!(
            word.to_ascii_lowercase().as_str(),
            "line" | "lines" | "column" | "col"
        );
        if is_label
            && tokens
                .peek()
                .is_some_and(|next| is_number(next.trim_end_matches([',', '.', ')', ';'])))
        {
            tokens.next();
            continue;
        }
        words.push(word);
    }
    words.join(" ")
}

fn is_number(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Removes every `:<digits>` sequence, so `:<line>` and `:<line>:<column>` both go.
fn strip_position_suffixes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ':' {
            let digits = chars[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .count();
            if digits > 0 {
                i += 1 + digits;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::finding::{Location, Severity};
    use crate::toolchain::CheckKind;

    fn finding(path: &str, line: u32, rule: Option<&str>, msg: &str) -> Finding {
        let f = Finding::new(
            CheckKind::Lint,
            Severity::Error,
            Location::line(path, line),
            msg,
        );
        match rule {
            Some(r) => f.with_rule(r),
            None => f,
        }
    }

    #[test]
    fn line_and_column_do_not_matter() {
        let a = finding("src/a.rs", 3, Some("r"), "bad");
        let mut b = finding("src/a.rs", 90, Some("r"), "bad");
        b.location.column = Some(4);
        assert_eq!(Fingerprint::of(&a), Fingerprint::of(&b));
    }

    #[test]
    fn kind_path_rule_and_message_matter() {
        let base = finding("src/a.rs", 1, Some("r"), "bad");
        let mut other_kind = base.clone();
        other_kind.kind = CheckKind::Typecheck;
        for other in [
            other_kind,
            finding("src/b.rs", 1, Some("r"), "bad"),
            finding("src/a.rs", 1, Some("s"), "bad"),
            finding("src/a.rs", 1, None, "bad"),
            finding("src/a.rs", 1, Some("r"), "worse"),
        ] {
            assert_ne!(Fingerprint::of(&base), Fingerprint::of(&other));
        }
    }

    #[test]
    fn canonical_form_is_stable_and_round_trips() {
        let fp = Fingerprint::of(&finding("src/a.rs", 1, Some("E1"), "bad  thing"));
        assert_eq!(fp.as_str(), "lint|src/a.rs|E1|bad thing");
        assert_eq!(fp.to_string(), "lint|src/a.rs|E1|bad thing");
        assert_eq!(Fingerprint::from_stored(fp.as_str()), fp);
        let none = Fingerprint::of(&finding("a", 1, None, "m"));
        assert_eq!(none.as_str(), "lint|a||m");
    }

    #[test]
    fn strips_positions_and_collapses_whitespace() {
        assert_eq!(normalize_message("  a \t b\n c  "), "a b c");
        assert_eq!(
            normalize_message("see a.rs:12:5 and b.rs:7"),
            "see a.rs and b.rs"
        );
        assert_eq!(normalize_message("at x.ts:10: bad"), "at x.ts: bad");
        assert_eq!(normalize_message("std::vec::Vec"), "std::vec::Vec");
        assert_eq!(normalize_message("E0599: no method"), "E0599: no method");
        assert_eq!(normalize_message("a:b"), "a:b");
        assert_eq!(normalize_message("a:1x"), "ax");
        assert_eq!(normalize_message("end:"), "end:");
        assert_eq!(normalize_message("a:1:b"), "a:b");
        assert_eq!(normalize_message("a:1:2:3"), "a");
    }

    #[test]
    fn strips_line_and_column_words_only_before_numbers() {
        assert_eq!(
            normalize_message("defined at line 12, here"),
            "defined at here"
        );
        assert_eq!(normalize_message("Line 3 column 9."), "");
        assert_eq!(normalize_message("lines 4) and col 2"), "and");
        assert_eq!(normalize_message("the line is long"), "the line is long");
        assert_eq!(normalize_message("a line"), "a line");
        assert_eq!(normalize_message("line 4x"), "line 4x");
        assert_eq!(normalize_message("line ."), "line .");
    }

    #[test]
    fn other_numbers_are_kept() {
        assert_eq!(
            normalize_message("expected 2 arguments, found 3"),
            "expected 2 arguments, found 3"
        );
    }
}
