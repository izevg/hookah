//! Parsing of unified-diff hunk headers.

use hookah_domain::session::LineRange;

/// Extracts the new-side ranges from `git diff -U0` output. Headers look like
/// `@@ -a[,b] +c[,d] @@`; an omitted count means 1, and `d == 0` (pure deletion) is skipped.
pub fn parse_hunks(diff: &str) -> Vec<LineRange> {
    diff.lines().filter_map(parse_header).collect()
}

fn parse_header(line: &str) -> Option<LineRange> {
    let rest = line.strip_prefix("@@ ")?;
    let mut parts = rest.split_whitespace();
    parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let (start, count) = match new.split_once(',') {
        Some((s, c)) => (s.parse::<u32>().ok()?, c.parse::<u32>().ok()?),
        None => (new.parse::<u32>().ok()?, 1),
    };
    if count == 0 {
        return None;
    }
    LineRange::new(start, start.checked_add(count - 1)?)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn r(a: u32, b: u32) -> LineRange {
        LineRange::new(a, b).unwrap()
    }

    #[test]
    fn single_line_without_count() {
        assert_eq!(parse_hunks("@@ -3 +3 @@"), vec![r(3, 3)]);
    }

    #[test]
    fn counted_hunks() {
        assert_eq!(parse_hunks("@@ -1,2 +5,3 @@"), vec![r(5, 7)]);
        assert_eq!(parse_hunks("@@ -0,0 +1,4 @@"), vec![r(1, 4)]);
    }

    #[test]
    fn pure_deletion_is_skipped() {
        assert!(parse_hunks("@@ -4,2 +3,0 @@").is_empty());
    }

    #[test]
    fn section_heading_after_header_is_ignored() {
        assert_eq!(parse_hunks("@@ -1 +2,2 @@ fn main() {"), vec![r(2, 3)]);
    }

    #[test]
    fn multiple_hunks_and_noise() {
        let diff = "diff --git a/x b/x\nindex 1..2 100644\n--- a/x\n+++ b/x\n\
                    @@ -1 +1 @@\n-a\n+b\n@@ -9,0 +10,2 @@\n+c\n+d\n@@ -20,3 +23,0 @@\n-e\n";
        assert_eq!(parse_hunks(diff), vec![r(1, 1), r(10, 11)]);
    }

    #[test]
    fn malformed_headers_are_ignored() {
        for bad in [
            "",
            "@@",
            "@@ -1 @@",
            "@@ -1 +x @@",
            "@@ -1 +1,y @@",
            "@@ -1 +0 @@",
            "+@@ -1 +1 @@",
            " @@ -1 +1 @@",
        ] {
            assert!(parse_hunks(bad).is_empty(), "{bad}");
        }
    }
}
