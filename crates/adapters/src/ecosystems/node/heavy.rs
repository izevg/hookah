//! Heavy Node tests stay out of the stop phase.
//!
//! The naming rule is the domain's ([`TestWeight`]): directories `e2e/`, `integration/`,
//! `playwright/`, `cypress/`; files `*.e2e.*`, `*.int.*`, `*.integration.*`, and stems with the
//! words `integration` or `e2e`. Two places apply it:
//!
//! - the changed files handed to the runner (see [`super::select::related_tests`]), and
//! - the runner itself, whose own import graph finds the related tests: a changed source can be
//!   imported by an e2e test, which `hookah` never names. So the related-tests command carries
//!   exclusion flags, verified against Vitest 5.1 and Jest 30.5:
//!   - Vitest: one `--exclude=<glob>` per pattern. They add to the project's own excludes.
//!   - Jest: one `--testPathIgnorePatterns=<regex>` per pattern, anchored at `<rootDir>` so a
//!     project that lives under a directory called `e2e` is not excluded whole. Jest replaces
//!     the project's own `testPathIgnorePatterns` when the flag is given, so `/node_modules/`
//!     is repeated here. This is the one place `hookah` overrides a project setting.
//!
//! The patterns approximate [`TestWeight::of_path`] (the runners match globs and regexes, not
//! words, and Jest's are case-sensitive); the file filter applies the exact rule to everything
//! `hookah` names itself.

use std::path::Path;

use hookah_domain::light::TestWeight;
use hookah_domain::project::Project;

/// The Vitest flags that exclude heavy tests.
pub fn vitest_flags() -> Vec<String> {
    let dirs = TestWeight::DIRS.join(",");
    let segments = TestWeight::SEGMENTS.join(",");
    let words = TestWeight::WORDS.join(",");
    [
        format!("**/{{{dirs}}}/**"),
        format!("**/*.{{{segments}}}.*"),
        format!("**/{{{words}}}[_-]*"),
        format!("**/*[_-]{{{words}}}.*"),
        format!("**/*[_-]{{{words}}}[_-]*"),
    ]
    .iter()
    .map(|g| format!("--exclude={g}"))
    .collect()
}

/// The Jest flags that ignore heavy tests.
pub fn jest_flags() -> Vec<String> {
    let dirs = TestWeight::DIRS.join("|");
    let segments = TestWeight::SEGMENTS.join("|");
    let words = TestWeight::WORDS.join("|");
    [
        "/node_modules/".to_string(),
        format!("<rootDir>/(.*/)?({dirs})/"),
        format!(r"<rootDir>/.*\.({segments})\.[^/]*$"),
        format!(r"<rootDir>/(.*/)?([^/]*[_-])?({words})([_.-][^/]*)?$"),
    ]
    .iter()
    .map(|r| format!("--testPathIgnorePatterns={r}"))
    .collect()
}

/// Whether the file at `file` (absolute, inside the project) is a heavy test or heavy test
/// support.
pub fn is_heavy(project: &Project, file: &Path) -> bool {
    project
        .relative(file)
        .is_ok_and(|rel| TestWeight::of_path(&rel).is_heavy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flags_are_the_patterns_verified_against_vitest_and_jest() {
        assert_eq!(
            vitest_flags(),
            [
                "--exclude=**/{integration,integrations,e2e,playwright,cypress}/**",
                "--exclude=**/*.{e2e,int,integration}.*",
                "--exclude=**/{integration,e2e,playwright,cypress}[_-]*",
                "--exclude=**/*[_-]{integration,e2e,playwright,cypress}.*",
                "--exclude=**/*[_-]{integration,e2e,playwright,cypress}[_-]*",
            ]
        );
        assert_eq!(
            jest_flags(),
            [
                "--testPathIgnorePatterns=/node_modules/",
                "--testPathIgnorePatterns=<rootDir>/(.*/)?(integration|integrations|e2e|playwright|cypress)/",
                r"--testPathIgnorePatterns=<rootDir>/.*\.(e2e|int|integration)\.[^/]*$",
                r"--testPathIgnorePatterns=<rootDir>/(.*/)?([^/]*[_-])?(integration|e2e|playwright|cypress)([_.-][^/]*)?$",
            ]
        );
    }
}
