//! Python output parsing.
//!
//! Each tool's output becomes findings whose paths are relative to the project root:
//!
//! - [`style`]: `ruff check --output-format json`, `ruff format` and `black` errors.
//! - [`mypy`]: mypy's `-O json` lines or its `path:line:col: error: message  [code]` text.
//! - [`imports`]: import-linter's "Broken contracts" report.
//! - [`unused`]: Vulture unused-code diagnostics.
//! - [`dynamic`]: pytest failures and mutmut survivors.
//!
//! Every parser follows the fail-open rule: output it cannot understand, or a tool that fails
//! without reporting anything, is a [`PortError`] carrying a short tail of the output. It is never
//! read as a pass.

pub mod dynamic;
pub mod imports;
pub mod mypy;
pub mod style;
mod unused;

use std::path::{Path, PathBuf};

use hookah_application::error::PortError;
use hookah_application::ports::ProcessOutput;
use hookah_domain::finding::Finding;
use hookah_domain::path::is_driveless_root;
use hookah_domain::project::Project;
use hookah_domain::toolchain::Tool;

use super::detect;

/// Parses the output of `tool`, run in `project`.
pub fn parse(
    project: &Project,
    tool: &Tool,
    out: &ProcessOutput,
) -> Result<Vec<Finding>, PortError> {
    let root = Root::new(&project.root);
    match tool.name.as_str() {
        detect::RUFF_FORMAT => style::ruff_format(&root, out),
        detect::BLACK => style::black(&root, out),
        detect::RUFF => style::ruff_check(&root, out),
        detect::MYPY => mypy::parse(&root, out),
        detect::IMPORT_LINTER => imports::parse(&root, out),
        detect::VULTURE => unused::parse(&root, out),
        detect::PYTEST => dynamic::pytest(&root, out),
        detect::MUTMUT => dynamic::mutmut(&root, out),
        other => Err(PortError::new(format!("unknown Python tool `{other}`"))),
    }
}

/// The project root, in the two spellings tools may report paths in.
#[derive(Debug, Clone)]
pub struct Root {
    path: PathBuf,
    canonical: PathBuf,
}

impl Root {
    /// Wraps a project root.
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            canonical: dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
        }
    }

    /// The root as given.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Makes a tool-reported `file` project-relative. Relative input is already relative to the
    /// project root (tools run there). An absolute path outside the project stays absolute.
    pub fn relative(&self, file: &str) -> PathBuf {
        let path = Path::new(file);
        if is_driveless_root(path) {
            return path.to_path_buf();
        }
        if !path.is_absolute() {
            return path.strip_prefix(".").unwrap_or(path).to_path_buf();
        }
        let canon = dunce::canonicalize(path).ok();
        for candidate in [Some(path.to_path_buf()), canon].into_iter().flatten() {
            for root in [&self.path, &self.canonical] {
                if let Ok(rel) = candidate.strip_prefix(root) {
                    return rel.to_path_buf();
                }
            }
        }
        path.to_path_buf()
    }
}

/// The last few non-empty lines of `text`, joined for a one-line error message.
pub fn tail(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let start = lines.len().saturating_sub(3);
    let joined = lines[start..].join(" | ");
    if joined.is_empty() {
        "no output".into()
    } else {
        joined.chars().take(300).collect()
    }
}

/// The error for a tool that failed without a result we can use: the exit status plus the tail of
/// stderr, or of stdout when stderr is empty.
pub fn failure(tool: &str, out: &ProcessOutput) -> PortError {
    let text = if out.stderr.trim().is_empty() {
        &out.stdout
    } else {
        &out.stderr
    };
    let status = out
        .exit_code
        .map_or_else(|| "was killed".to_string(), |c| format!("exited with {c}"));
    PortError::new(format!("{tool} {status}: {}", tail(text)))
}

/// Splits `path`, `path:line`, or `path:line:col` into its parts. The path itself may contain
/// colons (a Windows drive).
pub fn parse_position(prefix: &str) -> Option<(&str, Option<u32>, Option<u32>)> {
    let Some((rest, last)) = prefix.rsplit_once(':') else {
        return Some((prefix, None, None));
    };
    let Ok(last) = last.parse::<u32>() else {
        return Some((prefix, None, None));
    };
    match rest.rsplit_once(':') {
        Some((path, mid)) => match mid.parse::<u32>() {
            Ok(line) => Some((path, Some(line), Some(last))),
            Err(_) => Some((rest, Some(last), None)),
        },
        None => Some((rest, Some(last), None)),
    }
}

/// Shortens `text` to at most `max` characters, marking the cut.
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let cut: String = text.chars().take(max.saturating_sub(3)).collect();
        format!("{cut}...")
    }
}

/// Test support shared by the parser tests.
#[cfg(test)]
pub mod testing {
    use hookah_application::ports::ProcessOutput;

    /// A finished process.
    pub fn out(code: i32, stdout: &str, stderr: &str) -> ProcessOutput {
        ProcessOutput {
            exit_code: Some(code),
            stdout: stdout.into(),
            stderr: stderr.into(),
            timed_out: false,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::ecosystems::testing::abs;

    #[test]
    fn relative_paths() {
        let root = Root::new(&abs("/proj"));
        assert_eq!(root.relative("src/a.py"), Path::new("src/a.py"));
        assert_eq!(root.relative("./src/a.py"), Path::new("src/a.py"));
        assert_eq!(
            root.relative(&abs("/proj/src/a.py").to_string_lossy()),
            Path::new("src/a.py")
        );
        assert_eq!(
            root.relative(&abs("/elsewhere/a.py").to_string_lossy()),
            abs("/elsewhere/a.py")
        );
    }

    #[test]
    fn positions() {
        assert_eq!(
            parse_position("a/b.py:3:7"),
            Some(("a/b.py", Some(3), Some(7)))
        );
        assert_eq!(parse_position("a/b.py:3"), Some(("a/b.py", Some(3), None)));
        assert_eq!(parse_position("a/b.py"), Some(("a/b.py", None, None)));
        assert_eq!(
            parse_position(r"C:\p\b.py:3:7"),
            Some((r"C:\p\b.py", Some(3), Some(7)))
        );
    }

    #[test]
    fn failure_mentions_status_and_stderr_tail() {
        let e = failure("ruff", &testing::out(2, "", "error: bad config\n"));
        assert_eq!(e.message(), "ruff exited with 2: error: bad config");
        let e = failure("ruff", &ProcessOutput::default());
        assert_eq!(e.message(), "ruff was killed: no output");
    }

    fn tool(name: &str) -> Tool {
        use hookah_domain::toolchain::{CheckKind, CommandSpec};
        Tool {
            name: name.into(),
            kind: CheckKind::Lint,
            command: CommandSpec::new(name, vec![], "/proj", std::time::Duration::from_secs(1)),
        }
    }

    #[test]
    fn parse_dispatches_on_the_tool_name() {
        use hookah_domain::project::Ecosystem;
        let project = Project {
            root: "/proj".into(),
            ecosystem: Ecosystem::Python,
            manifests: vec![],
        };
        let mypy = testing::out(1, "a.py:3: error: bad type  [assignment]\n", "");
        let f = parse(&project, &tool(detect::MYPY), &mypy).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule.as_deref(), Some("assignment"));
        let ruff = testing::out(1, "[]", "");
        assert!(
            parse(&project, &tool(detect::RUFF), &ruff)
                .unwrap()
                .is_empty()
        );
        let e = parse(&project, &tool("flake8"), &ruff).unwrap_err();
        assert_eq!(e.message(), "unknown Python tool `flake8`");
    }

    #[test]
    fn vulture_parses_findings_and_skips_verbose_output() {
        use hookah_domain::finding::Severity;
        use hookah_domain::project::Ecosystem;
        use hookah_domain::toolchain::CheckKind;

        let root = abs("/proj");
        let absolute = root.join("src/cart.py");
        let project = Project {
            root,
            ecosystem: Ecosystem::Python,
            manifests: vec![],
        };
        let stdout = format!(
            "Scanning: src/cart.py\n{}:7: unused function 'lost' (60% confidence, 3 lines)\n./folder: note.py:19: unsatisfiable 'if' condition (100% confidence)\n",
            absolute.display()
        );
        let got = parse(&project, &tool("vulture"), &testing::out(3, &stdout, "")).unwrap();

        assert_eq!(got.len(), 2);
        assert_eq!(got[0].kind, CheckKind::Unused);
        assert_eq!(got[0].severity, Severity::Warning);
        assert_eq!(got[0].location.path, Path::new("src/cart.py"));
        assert_eq!(got[0].location.line, Some(7));
        assert_eq!(got[0].message, "unused function 'lost' (60% confidence)");
        assert_eq!(got[1].location.path, Path::new("folder: note.py"));
        assert_eq!(got[1].location.line, Some(19));
        assert_eq!(
            got[1].message,
            "unsatisfiable 'if' condition (100% confidence)"
        );
    }

    #[test]
    fn vulture_only_accepts_complete_reports() {
        use hookah_domain::project::Ecosystem;

        let project = Project {
            root: abs("/proj"),
            ecosystem: Ecosystem::Python,
            manifests: vec![],
        };
        let parse_report = |output| parse(&project, &tool("vulture"), &output);
        let report = "src/cart.py:7: unused function 'lost' (60% confidence)\n";

        assert!(
            parse_report(testing::out(0, "Scanning: src/cart.py\n", ""))
                .unwrap()
                .is_empty()
        );
        for output in [
            testing::out(1, "", "src/cart.py:1: invalid syntax"),
            testing::out(2, "", "Please pass at least one file or directory"),
            testing::out(3, report, "src/broken.py:1: invalid syntax"),
            testing::out(3, "lost  # unused function (src/cart.py:7)\n", ""),
            testing::out(
                3,
                "src/cart.py:7: unused function 'lost' (bad% confidence)\n",
                "",
            ),
            testing::out(
                3,
                "bad location: unused function 'lost' (60% confidence)\n",
                "",
            ),
            ProcessOutput {
                timed_out: true,
                ..testing::out(3, report, "")
            },
        ] {
            assert!(parse_report(output).is_err());
        }
    }

    #[test]
    fn clips_long_text() {
        assert_eq!(clip("abc", 5), "abc");
        assert_eq!(clip("abcdefgh", 5), "ab...");
    }
}
