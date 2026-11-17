//! pytest output (`-rfE --tb=short -q`).
//!
//! pytest exits 0 when all tests pass, 5 when it collected none (both are clean), 1 when tests
//! failed, and 2 when collection failed or the run was interrupted. Exit 3 and 4 (internal and
//! usage errors) are failures of the tool itself.
//!
//! Failures are read from two places:
//!
//! - the `short test summary info` lines `FAILED path::Class::test - message` and `ERROR path -
//!   message`, which name the failing tests and their files;
//! - the detail sections (`___ Class.test ___`), whose short traceback frames
//!   `path:line: in function` give the line. The last frame inside the test's own file is the
//!   location; a failure raised deep inside the code under test is therefore reported at the
//!   test line that triggered it. A collection error with no frame in its file (a syntax error)
//!   takes the line from the `line N` of the exception text.
//!
//! Every failed or erroring test is one `Error` finding of kind `Test`. The message is the first
//! `E` line of a failure (the assertion or the exception) and the last one of a collection error.

use hookah_application::error::PortError;
use hookah_application::ports::ProcessOutput;
use hookah_domain::finding::{Finding, Location, Severity};
use hookah_domain::toolchain::CheckKind;

use crate::ecosystems::python::parse::{Root, clip, failure, parse_position};

/// Longest message kept per finding.
const MESSAGE_LIMIT: usize = 200;

/// Findings of one pytest run.
pub fn parse(root: &Root, out: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    if out.timed_out {
        return Err(failure("pytest", out));
    }
    match out.exit_code {
        Some(0 | 5) => Ok(vec![]),
        Some(1 | 2) => {
            let findings = findings(root, &out.stdout);
            if findings.is_empty() {
                Err(failure("pytest", out))
            } else {
                Ok(findings)
            }
        }
        _ => Err(failure("pytest", out)),
    }
}

/// A line of the summary: a failed test or an error.
#[derive(Debug)]
struct Entry {
    is_error: bool,
    nodeid: String,
    message: Option<String>,
}

impl Entry {
    fn path(&self) -> &str {
        self.nodeid.split("::").next().unwrap_or(&self.nodeid)
    }

    /// `Class.test[param]`, as pytest titles the detail section; `None` for a whole file.
    fn name(&self) -> Option<String> {
        let (_, rest) = self.nodeid.split_once("::")?;
        Some(rest.split("::").collect::<Vec<_>>().join("."))
    }
}

/// A detail section.
#[derive(Debug)]
struct Section {
    title: String,
    frames: Vec<(String, u32)>,
    errors: Vec<String>,
    used: bool,
}

fn findings(root: &Root, stdout: &str) -> Vec<Finding> {
    let (entries, mut sections) = read(stdout);
    entries
        .iter()
        .map(|entry| finding(root, entry, &mut sections))
        .collect()
}

fn read(stdout: &str) -> (Vec<Entry>, Vec<Section>) {
    let mut entries = Vec::new();
    let mut sections: Vec<Section> = Vec::new();
    let mut in_summary = false;
    let mut open = false;
    for line in stdout.lines() {
        if line.starts_with('=') {
            in_summary = line.contains("short test summary info");
            open = false;
            continue;
        }
        if in_summary {
            if let Some(entry) = summary_entry(line) {
                entries.push(entry);
            }
            continue;
        }
        if let Some(title) = heading(line) {
            sections.push(Section {
                title,
                frames: vec![],
                errors: vec![],
                used: false,
            });
            open = true;
            continue;
        }
        let Some(section) = sections.last_mut().filter(|_| open) else {
            continue;
        };
        if let Some(text) = line
            .strip_prefix('E')
            .filter(|t| t.starts_with(char::is_whitespace))
        {
            let text = text.trim();
            if !text.is_empty() {
                section.errors.push(text.to_string());
            }
        } else if let Some(frame) = frame(line) {
            section.frames.push(frame);
        }
    }
    (entries, sections)
}

fn summary_entry(line: &str) -> Option<Entry> {
    let (is_error, rest) = if let Some(r) = line.strip_prefix("FAILED ") {
        (false, r)
    } else {
        (true, line.strip_prefix("ERROR ")?)
    };
    let (nodeid, message) = match rest.split_once(" - ") {
        Some((n, m)) => (n, Some(m.trim().to_string())),
        None => (rest, None),
    };
    Some(Entry {
        is_error,
        nodeid: nodeid.trim().to_string(),
        message,
    })
}

/// `____ title ____`.
fn heading(line: &str) -> Option<String> {
    let inner = line.strip_prefix("__")?.trim_start_matches('_');
    let inner = inner.strip_suffix('_')?.trim_end_matches('_');
    let title = inner.trim();
    (inner.starts_with(' ') && !title.is_empty()).then(|| title.to_string())
}

/// `path.py:line: in function`.
fn frame(line: &str) -> Option<(String, u32)> {
    if line.starts_with(char::is_whitespace) {
        return None;
    }
    let (prefix, _) = line.split_once(": in ")?;
    let (path, Some(number), None) = parse_position(prefix)? else {
        return None;
    };
    path.ends_with(".py").then(|| (path.to_string(), number))
}

fn finding(root: &Root, entry: &Entry, sections: &mut [Section]) -> Finding {
    let path = entry.path();
    let section = matching_section(entry, sections);
    let line = section.and_then(|s| line_in(s, path));
    let message = match section {
        Some(s) => {
            let text = if entry.is_error && entry.name().is_none() {
                s.errors.last()
            } else {
                s.errors.first()
            };
            text.cloned().or_else(|| entry.message.clone())
        }
        None => entry.message.clone(),
    };
    let message = clip(message.as_deref().unwrap_or("no details"), MESSAGE_LIMIT);
    let message = match (entry.is_error, entry.name()) {
        (false, Some(name)) => format!("{name} failed: {message}"),
        (true, Some(name)) => format!("error in {name}: {message}"),
        (_, None) => format!("collection error: {message}"),
    };
    let file = root.relative(path);
    let location = match line {
        Some(l) => Location::line(file, l),
        None => Location::file(file),
    };
    Finding::new(CheckKind::Test, Severity::Error, location, message)
}

/// The first unused section that belongs to `entry`, marked used.
fn matching_section<'a>(entry: &Entry, sections: &'a mut [Section]) -> Option<&'a Section> {
    let path = entry.path().to_string();
    let wanted = entry.name();
    let matches = |s: &Section| match (&wanted, entry.is_error) {
        (None, true) => s.title == format!("ERROR collecting {path}"),
        (Some(name), true) => {
            let last = name.rsplit('.').next().unwrap_or(name);
            (s.title.starts_with("ERROR at setup of ")
                || s.title.starts_with("ERROR at teardown of "))
                && s.title.ends_with(last)
        }
        (Some(name), false) => &s.title == name,
        (None, false) => false,
    };
    let candidates: Vec<usize> = sections
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.used && matches(s))
        .map(|(i, _)| i)
        .collect();
    let pick = candidates
        .iter()
        .copied()
        .find(|&i| sections[i].frames.iter().any(|(p, _)| same_file(p, &path)))
        .or_else(|| candidates.first().copied())?;
    sections[pick].used = true;
    Some(&sections[pick])
}

fn same_file(a: &str, b: &str) -> bool {
    a.trim_start_matches("./") == b.trim_start_matches("./")
}

/// The line of the failure inside `path`.
fn line_in(section: &Section, path: &str) -> Option<u32> {
    let from_frame = section
        .frames
        .iter()
        .rev()
        .find(|(p, _)| same_file(p, path))
        .map(|(_, l)| *l);
    from_frame.or_else(|| {
        // `File ".../path", line N` in the exception text of a syntax error.
        section.errors.iter().find_map(|e| {
            let (_, after) = e.split_once(path)?;
            let digits = after.split_once(", line ")?.1;
            digits
                .split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse()
                .ok()
        })
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::Path;

    use crate::ecosystems::python::parse::testing::out;

    use super::*;

    fn root() -> Root {
        Root::new(Path::new("/w/pt"))
    }

    /// Captured from pytest 9.2.0 (class, parametrized, and multi-line assertion failures, with
    /// a frame in the code under test).
    const FAILURES: &str = "\
F.FF                                                                     [100%]
=================================== FAILURES ===================================
______________________________ TestDiv.test_zero _______________________________
tests/test_calc.py:6: in test_zero
    div(1, 0)
pkg/calc.py:2: in div
    return a / b
           ^^^^^
E   ZeroDivisionError: division by zero
___________________________ TestDiv.test_param[2-0] ____________________________
tests/test_calc.py:10: in test_param
    assert div(a, b) == 1
           ^^^^^^^^^
pkg/calc.py:2: in div
    return a / b
           ^^^^^
E   ZeroDivisionError: division by zero
__________________________________ test_multi __________________________________
tests/test_calc.py:13: in test_multi
    assert [1, 2, 3] == [1,
E   assert [1, 2, 3] == [1, 2, 4]
E
E     At index 2 diff: 3 != 4
E     Use -v to get more diff
=========================== short test summary info ============================
FAILED tests/test_calc.py::TestDiv::test_zero - ZeroDivisionError: division b...
FAILED tests/test_calc.py::TestDiv::test_param[2-0] - ZeroDivisionError: divi...
FAILED tests/test_calc.py::test_multi - assert [1, 2, 3] == [1, 2, 4]
3 failed, 1 passed in 0.04s
";

    /// Captured from pytest 9.2.0: an import error and a syntax error during collection.
    const COLLECTION: &str = "\
==================================== ERRORS ====================================
__________________ ERROR collecting tests/sub/test_broken.py ___________________
ImportError while importing test module '/w/pt/tests/sub/test_broken.py'.
Hint: make sure your test modules/packages have valid Python names.
Traceback:
../../.local/share/uv/python/importlib/__init__.py:88: in import_module
    return _bootstrap._gcd_import(name[level:], package, level)
tests/sub/test_broken.py:1: in <module>
    import does_not_exist
E   ModuleNotFoundError: No module named 'does_not_exist'
____________________ ERROR collecting tests/test_syntax.py _____________________
../.venv/lib/python3.13/site-packages/_pytest/python.py:528: in importtestmodule
    mod = import_path(
<frozen importlib._bootstrap>:1404: in _gcd_import
    ???
../../.local/share/uv/python/ast.py:50: in parse
    return compile(source, filename, mode, flags,
E     File \"/w/pt/tests/test_syntax.py\", line 1
E       def test_a(:
E                  ^
E   SyntaxError: invalid syntax
=========================== short test summary info ============================
ERROR tests/sub/test_broken.py
ERROR tests/test_syntax.py
!!!!!!!!!!!!!!!!!!! Interrupted: 2 errors during collection !!!!!!!!!!!!!!!!!!!!
2 errors in 0.07s
";

    #[test]
    fn failures_are_located_at_the_test_line() {
        let f = parse(&root(), &out(1, FAILURES, "")).unwrap();
        let lines: Vec<String> = f
            .iter()
            .map(|f| format!("{}: {}", f.location, f.message))
            .collect();
        assert_eq!(
            lines,
            [
                "tests/test_calc.py:6: TestDiv.test_zero failed: ZeroDivisionError: division by zero",
                "tests/test_calc.py:10: TestDiv.test_param[2-0] failed: ZeroDivisionError: division by zero",
                "tests/test_calc.py:13: test_multi failed: assert [1, 2, 3] == [1, 2, 4]",
            ]
        );
        assert!(
            f.iter()
                .all(|f| f.kind == CheckKind::Test && f.severity == Severity::Error)
        );
    }

    #[test]
    fn collection_errors_use_the_last_error_line_and_the_syntax_error_line() {
        let f = parse(&root(), &out(2, COLLECTION, "")).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].location, Location::line("tests/sub/test_broken.py", 1));
        assert_eq!(
            f[0].message,
            "collection error: ModuleNotFoundError: No module named 'does_not_exist'"
        );
        assert_eq!(f[1].location, Location::line("tests/test_syntax.py", 1));
        assert_eq!(
            f[1].message,
            "collection error: SyntaxError: invalid syntax"
        );
    }

    #[test]
    fn same_named_tests_in_different_files_get_their_own_sections() {
        let text = "\
=================================== FAILURES ===================================
_________________________________ test_same ____________________________________
tests/a/test_x.py:3: in test_same
    assert 0
E   assert 0
_________________________________ test_same ____________________________________
tests/b/test_x.py:8: in test_same
    assert 1 == 2
E   assert 1 == 2
=========================== short test summary info ============================
FAILED tests/b/test_x.py::test_same - assert 1 == 2
FAILED tests/a/test_x.py::test_same - assert 0
";
        let f = parse(&root(), &out(1, text, "")).unwrap();
        assert_eq!(f[0].location, Location::line("tests/b/test_x.py", 8));
        assert_eq!(f[1].location, Location::line("tests/a/test_x.py", 3));
    }

    #[test]
    fn setup_errors_and_summary_only_entries() {
        let text = "\
==================================== ERRORS ====================================
_______________________ ERROR at setup of test_err _____________________________
tests/test_skip.py:7: in test_err
E   fixture 'fixture_missing' not found
=========================== short test summary info ============================
ERROR tests/test_skip.py::test_err
FAILED tests/test_gone.py::test_x - AssertionError: nope
";
        let f = parse(&root(), &out(1, text, "")).unwrap();
        assert_eq!(f[0].location, Location::line("tests/test_skip.py", 7));
        assert_eq!(
            f[0].message,
            "error in test_err: fixture 'fixture_missing' not found"
        );
        // No detail section: the file and the summary message are all there is.
        assert_eq!(f[1].location, Location::file("tests/test_gone.py"));
        assert_eq!(f[1].message, "test_x failed: AssertionError: nope");
    }

    #[test]
    fn long_messages_are_clipped() {
        let long = "x".repeat(500);
        let text = format!(
            "=== FAILURES ===\n___ test_a ___\ntests/t.py:2: in test_a\nE   {long}\n=== short test summary info ===\nFAILED tests/t.py::test_a - y\n"
        );
        let f = parse(&root(), &out(1, &text, "")).unwrap();
        assert!(f[0].message.chars().count() < 230);
        assert!(f[0].message.ends_with("..."));
    }

    #[test]
    fn clean_runs_and_tool_failures() {
        assert!(
            parse(&root(), &out(0, ".. [100%]\n2 passed in 0.01s\n", ""))
                .unwrap()
                .is_empty()
        );
        assert!(
            parse(&root(), &out(5, "no tests ran in 0.00s\n", ""))
                .unwrap()
                .is_empty()
        );
        let usage =
            "ERROR: file or directory not found: tests/nothing.py\n\nno tests ran in 0.00s\n";
        let e = parse(&root(), &out(4, usage, "")).unwrap_err();
        assert!(e.message().contains("file or directory not found"));
        assert!(parse(&root(), &out(1, "garbled\n", "")).is_err());
        assert!(parse(&root(), &ProcessOutput::default()).is_err());
    }
}
