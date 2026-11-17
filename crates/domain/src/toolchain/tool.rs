//! `Tool` and `CommandSpec` value objects.
//!
//! `MissingTool` names a tool the project lacks. A `Tool` is one configured program (`biome`, `ruff`, `clippy`, `pytest`, ...): its name, the
//! check it serves, and a `CommandSpec` (program, arguments, working directory, timeout). A
//! `CommandSpec` only describes a command; the `CheckRunner` port executes it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::check::CheckKind;

/// One command-line argument of a [`CommandSpec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    /// Passed as is.
    Literal(String),
    /// Placeholder replaced by every target, each as its own argument.
    Targets,
    /// Placeholder replaced by the targets that are neither one of these paths nor below one of
    /// them (whole path components), each as its own argument. For tools that are handed whole
    /// directories already and reject the same file given twice.
    TargetsOutside(Vec<String>),
}

/// A command to run: program, arguments with target placeholders, directory, and time limit.
///
/// Targets are strings: file paths relative to `cwd` for format and lint, package names or test
/// ids for tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    /// Program name or path.
    pub program: String,
    /// Arguments in order.
    pub args: Vec<Arg>,
    /// Working directory, normally the project root.
    pub cwd: PathBuf,
    /// Hard time limit.
    pub timeout: Duration,
}

impl CommandSpec {
    /// Builds a spec.
    pub fn new(
        program: impl Into<String>,
        args: Vec<Arg>,
        cwd: impl Into<PathBuf>,
        timeout: Duration,
    ) -> Self {
        Self {
            program: program.into(),
            args,
            cwd: cwd.into(),
            timeout,
        }
    }

    /// The final argument list with each [`Arg::Targets`] replaced by `targets`.
    pub fn expand_args(&self, targets: &[String]) -> Vec<String> {
        let mut out = Vec::with_capacity(self.args.len() + targets.len());
        for arg in &self.args {
            match arg {
                Arg::Literal(s) => out.push(s.clone()),
                Arg::Targets => out.extend(targets.iter().cloned()),
                Arg::TargetsOutside(covered) => out.extend(
                    targets
                        .iter()
                        .filter(|t| !covered.iter().any(|c| Path::new(t).starts_with(c)))
                        .cloned(),
                ),
            }
        }
        out
    }
}

/// A configured tool serving one kind of check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    /// Display name used in notices, such as `clippy`.
    pub name: String,
    /// The check this tool performs.
    pub kind: CheckKind,
    /// How to run it.
    pub command: CommandSpec,
}

/// A tool the project should have for a check but does not. `hookah` reports it and never
/// installs anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingTool {
    /// Tool name, such as `cargo-mutants`.
    pub name: String,
    /// The check it would serve.
    pub kind: CheckKind,
    /// How the user can get it, such as `cargo install cargo-mutants`.
    pub hint: String,
}

impl MissingTool {
    /// Builds a missing-tool entry.
    pub fn new(name: impl Into<String>, kind: CheckKind, hint: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind,
            hint: hint.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_targets_in_place() {
        let spec = CommandSpec::new(
            "cargo",
            vec![
                Arg::Literal("test".into()),
                Arg::Literal("-p".into()),
                Arg::Targets,
                Arg::Literal("--quiet".into()),
            ],
            "/r",
            Duration::from_secs(1),
        );
        assert_eq!(
            spec.expand_args(&["a".into(), "b".into()]),
            ["test", "-p", "a", "b", "--quiet"]
        );
        assert_eq!(spec.expand_args(&[]), ["test", "-p", "--quiet"]);
    }

    #[test]
    fn targets_outside_skip_covered_paths_by_component() {
        let spec = CommandSpec::new(
            "mypy",
            vec![
                Arg::Literal("src/shop".into()),
                Arg::TargetsOutside(vec!["src/shop".into(), "tests".into()]),
            ],
            "/r",
            Duration::from_secs(1),
        );
        let targets: Vec<String> = [
            "src/shop",
            "src/shop/a.py",
            "src/shop2/b.py",
            "tests/t.py",
            "tests",
            "setup.py",
            "src/other/c.py",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            spec.expand_args(&targets),
            ["src/shop", "src/shop2/b.py", "setup.py", "src/other/c.py"]
        );
        assert_eq!(spec.expand_args(&[]), ["src/shop"]);
    }
}
