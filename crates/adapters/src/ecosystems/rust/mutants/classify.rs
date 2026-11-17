//! File roles in a Cargo package.
//!
//! - Files under `tests/`, `benches/`, and `examples/` are [`FileRole::Test`]: they are never
//!   production code. Only `tests/` files lead to mutation targets (see `select`); an edited
//!   bench or example triggers nothing on its own.
//! - Any other file is scanned for the first line that starts, at column 0, with `#[cfg(test)]`
//!   (usually a trailing `mod tests`). If every changed line is at or after it, the edit is a
//!   test edit. Otherwise, and for a new file ([`ChangedLines::All`]), it is [`FileRole::Source`].
//!   The scan is line-based, so it never parses Rust and cannot fail on broken syntax.

use std::path::Path;

use hookah_domain::session::{ChangedLines, FileRole};

/// Directories whose files are tests rather than production code.
const TEST_DIRS: [&str; 3] = ["tests", "benches", "examples"];

/// The role of `file` (absolute) in the crate at `root`, given its `changed` lines.
pub fn classify(root: &Path, file: &Path, changed: &ChangedLines) -> FileRole {
    if let Ok(rel) = file.strip_prefix(root)
        && let Some(first) = rel.components().next()
        && TEST_DIRS.iter().any(|d| first.as_os_str() == *d)
    {
        return FileRole::Test;
    }
    let ChangedLines::Ranges(ranges) = changed else {
        return FileRole::Source;
    };
    if ranges.is_empty() || file.extension().is_none_or(|e| e != "rs") {
        return FileRole::Source;
    }
    let Ok(text) = std::fs::read_to_string(file) else {
        return FileRole::Source;
    };
    match first_test_line(&text) {
        Some(start) if ranges.iter().all(|r| r.start() >= start) => FileRole::Test,
        _ => FileRole::Source,
    }
}

/// The 1-based line of the first `#[cfg(test)]` at column 0.
fn first_test_line(text: &str) -> Option<u32> {
    let index = text.lines().position(|l| l.starts_with("#[cfg(test)]"))?;
    u32::try_from(index + 1).ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use hookah_domain::session::LineRange;

    use super::*;

    const SRC: &str = "pub fn f() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n";

    fn lines(a: u32, b: u32) -> ChangedLines {
        ChangedLines::Ranges(vec![LineRange::new(a, b).unwrap()])
    }

    #[test]
    fn finds_the_first_cfg_test_line() {
        assert_eq!(first_test_line(SRC), Some(3));
        assert_eq!(
            first_test_line("fn f() {\n    #[cfg(test)]\n    x();\n}\n"),
            None
        );
        assert_eq!(first_test_line(""), None);
    }

    #[test]
    fn test_directories_are_tests() {
        let root = Path::new("/p");
        for f in ["/p/tests/a.rs", "/p/benches/b.rs", "/p/examples/c.rs"] {
            assert_eq!(
                classify(root, Path::new(f), &ChangedLines::All),
                FileRole::Test
            );
        }
        assert_eq!(
            classify(root, Path::new("/p/src/tests/a.rs"), &ChangedLines::All),
            FileRole::Source
        );
    }

    #[test]
    fn inline_test_module_edits_are_tests() {
        let dir = std::env::temp_dir().join(format!("hookah-classify-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let file = dir.join("src/lib.rs");
        std::fs::write(&file, SRC).unwrap();
        let c = |l: &ChangedLines| classify(&dir, &file, l);
        assert_eq!(c(&lines(3, 7)), FileRole::Test);
        assert_eq!(c(&lines(5, 5)), FileRole::Test);
        assert_eq!(c(&lines(1, 1)), FileRole::Source);
        assert_eq!(c(&lines(2, 4)), FileRole::Source);
        let both = ChangedLines::Ranges(vec![
            LineRange::new(1, 1).unwrap(),
            LineRange::new(5, 5).unwrap(),
        ]);
        assert_eq!(c(&both), FileRole::Source);
        assert_eq!(c(&ChangedLines::All), FileRole::Source);
        assert_eq!(c(&ChangedLines::Ranges(vec![])), FileRole::Source);
        assert_eq!(
            classify(&dir, &dir.join("src/gone.rs"), &lines(5, 5)),
            FileRole::Source
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
