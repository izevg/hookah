//! mutmut 3 output (`mutmut run <globs>`).
//!
//! When given mutant names or globs, `mutmut run` ends with a `Mutant results` list that covers
//! exactly the selected mutants, including those whose result came from mutmut's own incremental
//! cache:
//!
//! ```text
//! Mutant results
//! --------------
//! 🙁 shop.domain.cart.x_total__mutmut_9
//! 🎉 shop.domain.cart.x_total__mutmut_6
//! ```
//!
//! `🙁` marks a survivor. Each survivor is one `Error` finding of kind `Mutation`:
//! `mutant survived: <name> (`<original line>` -> `<mutated line>`)`. The location is the changed
//! line of the original source when it can be derived, else the file.
//!
//! The line comes from mutmut's work copy. `mutants/<file>` holds, for each mutated function, the
//! original as `x_<name>__mutmut_orig` and every mutant as `x_<name>__mutmut_<n>` (methods:
//! `xǁClass.ǁ<method>`). The first line where a mutant differs from the original, counted from
//! the `def`, is added to the `def` line of the function in the real source file.
//!
//! mutmut exits non-zero when the baseline test run fails (`failed to collect stats`), which is a
//! [`PortError`]. A glob that matches no mutant (for example, a file outside the configured
//! `source_paths`) makes mutmut fail with an assertion; that means there is nothing to mutate and
//! is a clean result.

use std::fs;
use std::path::{Path, PathBuf};

use hookah_application::error::PortError;
use hookah_application::ports::ProcessOutput;
use hookah_domain::finding::{Finding, Location, Severity};
use hookah_domain::toolchain::CheckKind;

use crate::ecosystems::python::parse::{Root, clip, failure};

const SURVIVED: &str = "🙁";
const NOTHING_MATCHES: &str = "nothing matches";
/// Separates class and method in the mangled name of a method.
const CLASS_SEPARATOR: char = 'ǁ';
/// Longest source line quoted in a message.
const QUOTE_LIMIT: usize = 60;

/// Findings of one `mutmut run`.
pub fn parse(root: &Root, out: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    if out.timed_out || out.exit_code.is_none() {
        return Err(failure("mutmut", out));
    }
    if out.exit_code != Some(0) {
        let both = format!("{}\n{}", out.stdout, out.stderr);
        return if both.contains(NOTHING_MATCHES) {
            Ok(vec![])
        } else {
            Err(failure("mutmut", out))
        };
    }
    let mut lines = out
        .stdout
        .lines()
        .map(|l| l.trim_end())
        .skip_while(|l| l.trim() != "Mutant results");
    if lines.next().is_none() {
        return Err(PortError::new(format!(
            "mutmut printed no result list: {}",
            super::super::tail(&out.stdout)
        )));
    }
    Ok(lines
        .filter_map(|l| l.strip_prefix(SURVIVED))
        .map(|name| survivor(root, name.trim()))
        .collect())
}

/// A mutant name split into its parts.
#[derive(Debug, PartialEq, Eq)]
struct MutantName<'a> {
    /// `shop.domain.cart`
    module: &'a str,
    /// `x_total` or `xǁCartǁadd`
    key: &'a str,
    /// `x_total__mutmut_9`, the mutant function's name.
    function: &'a str,
}

fn split_name(name: &str) -> Option<MutantName<'_>> {
    let (mangled, _) = name.split_once("__mutmut_")?;
    let (module, key) = mangled.rsplit_once('.')?;
    let function = &name[module.len() + 1..];
    Some(MutantName {
        module,
        key,
        function,
    })
}

/// The Python function name and optional class of a key.
fn function_of(key: &str) -> Option<(Option<&str>, &str)> {
    if let Some(rest) = key.strip_prefix(&format!("x{CLASS_SEPARATOR}")) {
        let (class, method) = rest.split_once(CLASS_SEPARATOR)?;
        Some((Some(class), method))
    } else {
        Some((None, key.strip_prefix("x_")?))
    }
}

fn survivor(root: &Root, name: &str) -> Finding {
    let message = format!("mutant survived: {name}");
    let parts = split_name(name);
    let Some(parts) = parts else {
        return finding(Location::file(PathBuf::from("pyproject.toml")), message);
    };
    let file = source_file(root.path(), parts.module);
    match changed_line(root.path(), &file, &parts) {
        Some(change) => {
            let message = format!(
                "{message} (`{}` -> `{}`)",
                clip(&change.original, QUOTE_LIMIT),
                clip(&change.mutated, QUOTE_LIMIT)
            );
            finding(Location::line(file, change.line), message)
        }
        None => finding(Location::file(file), message),
    }
}

fn finding(location: Location, message: String) -> Finding {
    Finding::new(CheckKind::Mutation, Severity::Error, location, message)
}

/// The source file of a mutmut module name: `src/<path>.py` or `<path>.py`, whichever exists.
/// Without either, the flat path is a best guess.
fn source_file(root: &Path, module: &str) -> PathBuf {
    let flat = PathBuf::from(module.replace('.', "/")).with_extension("py");
    [Path::new("src").join(&flat), flat.clone()]
        .into_iter()
        .find(|p| root.join(p).is_file())
        .unwrap_or(flat)
}

struct Change {
    line: u32,
    original: String,
    mutated: String,
}

fn changed_line(root: &Path, file: &Path, name: &MutantName<'_>) -> Option<Change> {
    let mutated = fs::read_to_string(root.join("mutants").join(file)).ok()?;
    let source = fs::read_to_string(root.join(file)).ok()?;
    locate(&source, &mutated, name)
}

/// Pure core of [`changed_line`]: compares the mutant to the original inside the mutmut copy and
/// maps the difference onto the real source.
fn locate(source: &str, mutated: &str, name: &MutantName<'_>) -> Option<Change> {
    let original_fn = format!("{}__mutmut_orig", name.key);
    let original = block(mutated, &original_fn)?;
    let mutant = block(mutated, name.function)?;
    // The `def` lines differ in the function's name only; compare them without it.
    let neutral = |lines: &[&str]| -> Vec<String> {
        let mut out: Vec<String> = lines.iter().map(|l| (*l).to_string()).collect();
        if let Some(first) = out.first_mut() {
            *first = first
                .replacen(&original_fn, "_", 1)
                .replacen(name.function, "_", 1);
        }
        out
    };
    let (original_cmp, mutant_cmp) = (neutral(&original), neutral(&mutant));
    let offset = (0..original.len().max(mutant.len()))
        .find(|&i| original_cmp.get(i) != mutant_cmp.get(i))?;
    let (class, function) = function_of(name.key)?;
    let def_line = def_line(source, class, function)?;
    Some(Change {
        line: u32::try_from(def_line + offset + 1).ok()?,
        original: original.get(offset).map_or("", |l| l.trim()).to_string(),
        mutated: mutant.get(offset).map_or("", |l| l.trim()).to_string(),
    })
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The lines of the function `name` in mutmut's copy: from its `def` to the last line indented
/// deeper than the `def`.
fn block<'a>(text: &'a str, name: &str) -> Option<Vec<&'a str>> {
    let lines: Vec<&str> = text.lines().collect();
    let header = format!("def {name}(");
    let start = lines.iter().position(|l| {
        let t = l.trim_start();
        t.starts_with(&header)
            || t.strip_prefix("async ")
                .is_some_and(|r| r.starts_with(&header))
    })?;
    let base = indent(lines[start]);
    let mut end = start + 1;
    while end < lines.len() && (lines[end].trim().is_empty() || indent(lines[end]) > base) {
        end += 1;
    }
    let mut body = lines[start..end].to_vec();
    while body.last().is_some_and(|l| l.trim().is_empty()) {
        body.pop();
    }
    Some(body)
}

/// The 0-based line of `def function(` in the real source, inside `class` when given.
fn def_line(source: &str, class: Option<&str>, function: &str) -> Option<usize> {
    let lines: Vec<&str> = source.lines().collect();
    let from = match class {
        Some(class) => lines.iter().position(|l| {
            let t = l.trim_start();
            t.strip_prefix("class ").is_some_and(|r| {
                r.strip_prefix(class)
                    .is_some_and(|r| r.starts_with(['(', ':', ' ']))
            })
        })?,
        None => 0,
    };
    let header = format!("def {function}(");
    lines.iter().enumerate().skip(from).find_map(|(i, l)| {
        let t = l.trim_start();
        (t.starts_with(&header)
            || t.strip_prefix("async ")
                .is_some_and(|r| r.starts_with(&header)))
        .then_some(i)
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use crate::ecosystems::python::fixture::Scratch;
    use crate::ecosystems::python::parse::testing::out;

    use super::*;

    /// Captured from mutmut 3.8.1 (`mutmut run "shop.domain.cart.*"`), progress lines omitted.
    const RUN: &str = "\
    done in 39ms (2 files mutated, 0 ignored, 0 unmodified)
9/11  🎉 4 🫥 0  ⏰ 0  🤔 0  🙁 5  🔇 0  🧙 0
222.09 mutations/second

Mutant results
--------------
🙁 shop.domain.cart.x_total__mutmut_1
🙁 shop.domain.cart.x_total__mutmut_2
🎉 shop.domain.cart.x_total__mutmut_4
⏰ shop.domain.cart.x_total__mutmut_6
🙁 shop.domain.cart.x_total__mutmut_9

";

    const SOURCE: &str = "import os\nfrom shop.infra.db import connect\n\n\ndef total(items: list[int]) -> int:\n    x: int = \"a\"\n    return sum(items) + 1 if connect() else 0\n";

    /// mutmut's copy of `SOURCE`, as generated by mutmut 3.8.1 (two mutants shown).
    const MUTATED: &str = "import os
from shop.infra.db import connect


from mutmut.mutation.trampoline import wrap_in_trampoline as _mutmut_mutated, MutantDict
mutants_x_total__mutmut: MutantDict = {}  # type: ignore


@_mutmut_mutated(mutants_x_total__mutmut)
def total(items: list[int]) -> int:
    x: int = \"a\"
    return sum(items) + 1 if connect() else 0


def x_total__mutmut_orig(items: list[int]) -> int:
    x: int = \"a\"
    return sum(items) + 1 if connect() else 0


def x_total__mutmut_1(items: list[int]) -> int:
    x: int = None
    return sum(items) + 1 if connect() else 0


def x_total__mutmut_9(items: list[int]) -> int:
    x: int = \"a\"
    return sum(items) + 1 if connect() else 1

mutants_x_total__mutmut['_mutmut_orig'] = x_total__mutmut_orig # type: ignore # mutmut generated
";

    fn project() -> Scratch {
        let s = Scratch::new("mutmut");
        s.write("src/shop/domain/cart.py", SOURCE);
        s.write("mutants/src/shop/domain/cart.py", MUTATED);
        s
    }

    #[test]
    fn survivors_are_located_at_the_changed_source_line() {
        let s = project();
        let f = parse(&Root::new(s.root()), &out(0, RUN, "")).unwrap();
        assert_eq!(f.len(), 3, "only 🙁 lines");
        assert_eq!(f[0].location, Location::line("src/shop/domain/cart.py", 6));
        assert_eq!(
            f[0].message,
            "mutant survived: shop.domain.cart.x_total__mutmut_1 (`x: int = \"a\"` -> `x: int = None`)"
        );
        // mutmut_2 is not in the copy: the file is known, the line is not.
        assert_eq!(f[1].location, Location::file("src/shop/domain/cart.py"));
        assert_eq!(
            f[1].message,
            "mutant survived: shop.domain.cart.x_total__mutmut_2"
        );
        assert_eq!(f[2].location, Location::line("src/shop/domain/cart.py", 7));
        assert!(f[2].message.contains("`return sum(items) + 1 if connect() else 0` -> `return sum(items) + 1 if connect() else 1`"));
        assert!(
            f.iter()
                .all(|f| f.kind == CheckKind::Mutation && f.severity == Severity::Error)
        );
    }

    #[test]
    fn flat_layout_and_missing_work_copy() {
        let s = Scratch::new("mutmut-flat");
        s.write("shop/domain/cart.py", SOURCE);
        let f = parse(&Root::new(s.root()), &out(0, RUN, "")).unwrap();
        assert_eq!(f[0].location, Location::file("shop/domain/cart.py"));
    }

    #[test]
    fn methods_are_found_inside_their_class() {
        let source = "class A:\n    def go(self):\n        pass\n\nclass Cart:\n    def add(self, x):\n        return x + 1\n";
        let mutated = "class xǁCartǁ:\n    pass\n\ndef xǁCartǁadd__mutmut_orig(self, x):\n    return x + 1\n\ndef xǁCartǁadd__mutmut_1(self, x):\n    return x - 1\n";
        let name = split_name("shop.cart.xǁCartǁadd__mutmut_1").unwrap();
        assert_eq!(name.key, "xǁCartǁadd");
        let c = locate(source, mutated, &name).unwrap();
        assert_eq!(
            (c.line, c.original.as_str(), c.mutated.as_str()),
            (7, "return x + 1", "return x - 1")
        );
    }

    #[test]
    fn name_splitting() {
        let n = split_name("shop.domain.cart.x_total__mutmut_9").unwrap();
        assert_eq!(
            n,
            MutantName {
                module: "shop.domain.cart",
                key: "x_total",
                function: "x_total__mutmut_9"
            }
        );
        assert_eq!(function_of("x_total"), Some((None, "total")));
        assert_eq!(function_of("xǁCartǁadd"), Some((Some("Cart"), "add")));
        assert!(split_name("garbage").is_none());
    }

    #[test]
    fn no_survivors_and_nothing_to_mutate() {
        let root = Root::new(Path::new("/p"));
        let killed = "Mutant results\n--------------\n🎉 a.x_f__mutmut_1\n";
        assert!(parse(&root, &out(0, killed, "")).unwrap().is_empty());
        let trace = "AssertionError: Filtered for specific mutants, but nothing matches\n\nFilter: ('x.*',)\n";
        assert!(parse(&root, &out(1, "", trace)).unwrap().is_empty());
    }

    #[test]
    fn baseline_failures_and_missing_results_are_port_errors() {
        let root = Root::new(Path::new("/p"));
        let text = "FAILED tests/test_zz.py::test_bad - assert False\n1 failed in 0.00s\nfailed to collect stats. runner returned 1\n";
        let e = parse(&root, &out(1, text, "")).unwrap_err();
        assert!(e.message().contains("failed to collect stats"));
        assert!(parse(&root, &out(0, "9/11\n", "")).is_err());
        assert!(parse(&root, &ProcessOutput::default()).is_err());
    }
}
