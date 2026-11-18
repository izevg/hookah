//! `TestWeight`: whether a test file is light enough to run on every stop.
//!
//! The rule is about names, so every ecosystem classifies with the same words. A project-relative
//! path is [`TestWeight::Heavy`] when
//!
//! - a directory is called `integration`, `integrations`, `e2e`, `playwright`, or `cypress`, or
//! - a dot segment between the stem and the extension is `e2e`, `int`, or `integration`
//!   (`checkout.e2e.ts`, `db.int.ts`), or
//! - the stem, split at `_` and `-`, has a word `integration`, `e2e`, `playwright`, or `cypress`
//!   (`test_cart_integration.py`, `playwright.config.ts`).
//!
//! Matching ignores case. Everything else is [`TestWeight::Light`].

use std::path::Path;

const HEAVY_DIRS: &[&str] = &[
    "integration",
    "integrations",
    "e2e",
    "playwright",
    "cypress",
];
const HEAVY_SEGMENTS: &[&str] = &["e2e", "int", "integration"];
const HEAVY_WORDS: &[&str] = &["integration", "e2e", "playwright", "cypress"];

/// How expensive a test is to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TestWeight {
    /// Fast, in-process; runs at stop.
    Light,
    /// Integration, end-to-end, or browser test; never selected.
    Heavy,
}

impl TestWeight {
    /// Directory names that hold heavy tests.
    pub const DIRS: &'static [&'static str] = HEAVY_DIRS;
    /// Dot segments (`name.<segment>.ext`) that mark a heavy test.
    pub const SEGMENTS: &'static [&'static str] = HEAVY_SEGMENTS;
    /// Words of a file stem that mark a heavy test.
    pub const WORDS: &'static [&'static str] = HEAVY_WORDS;

    /// The weight of the test file at `rel`, a path relative to the project root.
    pub fn of_path(rel: &Path) -> Self {
        let in_heavy_dir = rel.parent().is_some_and(|dir| {
            dir.components().any(|c| {
                c.as_os_str()
                    .to_str()
                    .is_some_and(|d| HEAVY_DIRS.contains(&d.to_lowercase().as_str()))
            })
        });
        let name = rel
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();
        if in_heavy_dir || name_is_heavy(&name) {
            Self::Heavy
        } else {
            Self::Light
        }
    }

    /// Whether the weight is [`TestWeight::Heavy`].
    pub fn is_heavy(self) -> bool {
        self == Self::Heavy
    }
}

fn name_is_heavy(name: &str) -> bool {
    let parts: Vec<&str> = name.split('.').collect();
    let stem = parts.first().copied().unwrap_or("");
    let middle = parts.get(1..parts.len().saturating_sub(1)).unwrap_or(&[]);
    middle.iter().any(|p| HEAVY_SEGMENTS.contains(p))
        || stem.split(['_', '-']).any(|w| HEAVY_WORDS.contains(&w))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weight(p: &str) -> TestWeight {
        TestWeight::of_path(Path::new(p))
    }

    #[test]
    fn heavy_directories() {
        for p in [
            "e2e/a.test.ts",
            "tests/integration/test_a.py",
            "src/Integration/a.ts",
            "playwright/a.spec.ts",
            "cypress/e2e/a.cy.ts",
            "tests/e2e/main.rs",
            "a/integrations/b.py",
        ] {
            assert_eq!(weight(p), TestWeight::Heavy, "{p}");
        }
    }

    #[test]
    fn heavy_file_names() {
        for p in [
            "src/checkout.e2e.ts",
            "src/db.int.test.ts",
            "src/db.integration.spec.js",
            "tests/test_cart_integration.py",
            "tests/integration_flow.rs",
            "tests/api-e2e.rs",
            "playwright.config.ts",
            "cypress.config.js",
            "tests/E2E.rs",
        ] {
            assert_eq!(weight(p), TestWeight::Heavy, "{p}");
        }
    }

    #[test]
    fn everything_else_is_light() {
        for p in [
            "src/foo.test.ts",
            "tests/test_int_parse.py",
            "tests/test_cart.py",
            "tests/integrationish.rs",
            "tests/integrations.rs",
            "src/interpreter.spec.ts",
            "tests/unit/e2ee.rs",
            "src/int.ts",
            "a.rs",
            "",
        ] {
            assert_eq!(weight(p), TestWeight::Light, "{p}");
        }
        assert!(weight("e2e/a.ts").is_heavy());
        assert!(!weight("a.ts").is_heavy());
    }
}
