//! Node output parsing.
//!
//! [`parse`] sends the output of one tool to the parser that knows its format. Every parser
//! prefers the tool's machine-readable output and returns findings with paths relative to the
//! project root (or to the tool's own directory, see [`parse`]):
//!
//! - [`lint`]: Biome and ESLint JSON, Prettier syntax errors (format and lint tools);
//! - [`graph`]: `tsc` diagnostics and the dependency-cruiser JSON (whole-program checks);
//! - [`runs`]: Vitest and Jest reports and the Stryker mutation report (test runs).
//!
//! A tool that fails and leaves nothing readable becomes a [`PortError`] with a short tail of its
//! output, never a silent pass.

pub mod graph;
pub mod lint;
pub mod runs;

use std::path::{Component, Path, PathBuf};

use hookah_application::error::PortError;
use hookah_application::ports::ProcessOutput;
use hookah_domain::finding::Finding;
use hookah_domain::project::Project;
use hookah_domain::toolchain::{CheckKind, Tool};
use serde_json::Value;

use super::detect;
use crate::ecosystems::rust::parse::tail;

/// Turns the output of `tool` into findings with paths relative to the project root. A tool that
/// ran in another directory (a workspace package that depends on the project) reports paths
/// relative to that directory; they are re-based onto the project root, so they read `../app/x.ts`.
pub fn parse(
    project: &Project,
    tool: &Tool,
    output: &ProcessOutput,
) -> Result<Vec<Finding>, PortError> {
    let cwd = tool.command.cwd.as_path();
    let mut found = parse_in(cwd, tool, output)?;
    if cwd != project.root {
        for finding in &mut found {
            let abs = normalize(&cwd.join(&finding.location.path));
            finding.location.path = relative(&project.root, &abs.to_string_lossy());
        }
    }
    Ok(found)
}

/// Parses with paths relative to `root`, the directory the tool ran in.
fn parse_in(root: &Path, tool: &Tool, output: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    match (tool.name.as_str(), tool.kind) {
        (detect::BIOME, CheckKind::Format) => lint::biome_format(root, output),
        (detect::BIOME, _) => lint::biome_lint(root, output),
        (detect::PRETTIER, _) => lint::prettier(root, output),
        (detect::ESLINT, _) => lint::eslint(root, output),
        (detect::TSC, _) => graph::tsc(root, output),
        (detect::DEPCRUISE, _) => graph::depcruise(root, output),
        (detect::VITEST, _) => runs::vitest(root, output),
        (detect::JEST, _) => runs::jest(root, output),
        (detect::STRYKER, _) => runs::stryker(root, output),
        (other, _) => Err(PortError::new(format!("unknown Node tool `{other}`"))),
    }
}

/// The error for a tool that failed without readable output: its name and the tail of what it
/// printed.
pub(crate) fn unreadable(tool: &str, output: &ProcessOutput) -> PortError {
    let text = if output.stderr.trim().is_empty() {
        &output.stdout
    } else {
        &output.stderr
    };
    PortError::new(format!(
        "{tool} failed (exit {}): {}",
        output
            .exit_code
            .map_or_else(|| "none".to_owned(), |c| c.to_string()),
        tail(&strip_ansi(text))
    ))
}

/// Removes ANSI escape sequences (colors and cursor movement).
pub(crate) fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    out
}

/// The first JSON document in `text` that starts a line. Tools print notices, progress, and log
/// lines around their JSON, so the whole text is rarely valid JSON by itself.
pub(crate) fn json_in(text: &str) -> Option<Value> {
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        if matches!(line.trim_start().chars().next(), Some('{' | '['))
            && let Some(Ok(value)) = serde_json::Deserializer::from_str(&text[start..])
                .into_iter::<Value>()
                .next()
        {
            return Some(value);
        }
        start += line.len();
    }
    None
}

/// `file` as a path relative to the project root. Relative input is relative to the root already
/// (every tool runs with the root as working directory). Absolute input is stripped of the root,
/// also in its symlink-resolved form (Node reports real paths); a file elsewhere keeps `..`.
pub(crate) fn relative(root: &Path, file: &str) -> PathBuf {
    let path = Path::new(file);
    if !path.is_absolute() {
        return normalize(path);
    }
    if let Ok(rel) = path.strip_prefix(root) {
        return normalize(rel);
    }
    let real_root = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let real = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    match real.strip_prefix(&real_root) {
        Ok(rel) => normalize(rel),
        Err(_) => diff(&real_root, &normalize(&real)),
    }
}

/// Collapses `.` and `..` components lexically.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// `to` relative to `from`, using `..` where needed. Both are absolute and normalized.
fn diff(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for c in &to[common..] {
        out.push(c);
    }
    out
}

/// The first non-empty line of `text`, trimmed and capped so one finding stays one short line.
pub(crate) fn first_line(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty());
    cap(line.unwrap_or(""), 200)
}

/// `text` with whitespace runs collapsed to single spaces, cut to `max` characters.
pub(crate) fn cap(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        return one_line;
    }
    let mut cut: String = one_line.chars().take(max.saturating_sub(3)).collect();
    cut.push_str("...");
    cut
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn tsc_tool(cwd: &str) -> Tool {
        use hookah_domain::toolchain::CommandSpec;
        Tool {
            name: detect::TSC.into(),
            kind: CheckKind::Typecheck,
            command: CommandSpec::new("tsc", vec![], cwd, std::time::Duration::from_secs(1)),
        }
    }

    #[test]
    fn findings_of_a_tool_in_another_directory_are_rebased_onto_the_project() {
        use hookah_domain::project::Ecosystem;
        let project = Project {
            root: "/ws/packages/lib".into(),
            ecosystem: Ecosystem::Node,
            manifests: vec![],
        };
        let output = ProcessOutput {
            exit_code: Some(2),
            stdout: "src/main.ts(4,24): error TS2353: bad.\n".into(),
            ..ProcessOutput::default()
        };
        let in_dependent = parse(&project, &tsc_tool("/ws/packages/app"), &output).unwrap();
        assert_eq!(
            in_dependent[0].location.path,
            Path::new("../app/src/main.ts")
        );
        let in_workspace = parse(&project, &tsc_tool("/ws"), &output).unwrap();
        assert_eq!(
            in_workspace[0].location.path,
            Path::new("../../src/main.ts")
        );
        let in_project = parse(&project, &tsc_tool("/ws/packages/lib"), &output).unwrap();
        assert_eq!(in_project[0].location.path, Path::new("src/main.ts"));
    }

    #[test]
    fn strips_ansi_sequences() {
        assert_eq!(
            strip_ansi("\u{1b}[32m15:00 INFO\u{1b}[39m x"),
            "15:00 INFO x"
        );
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn finds_json_among_noise() {
        let text = "warming up\n[error] not json\n{\"a\": [1, 2]}\ntrailing text\n";
        assert_eq!(json_in(text).unwrap()["a"][1], 2);
        assert_eq!(json_in("[{\"x\":1}]\n").unwrap()[0]["x"], 1);
        assert!(json_in("no json here\n[still not\n").is_none());
        assert!(json_in("").is_none());
    }

    #[test]
    fn relative_paths() {
        let root = Path::new("/ws/app");
        assert_eq!(relative(root, "src/a.ts"), Path::new("src/a.ts"));
        assert_eq!(relative(root, "./src/../src/a.ts"), Path::new("src/a.ts"));
        assert_eq!(relative(root, "/ws/app/src/a.ts"), Path::new("src/a.ts"));
        assert_eq!(relative(root, "../lib/x.ts"), Path::new("../lib/x.ts"));
        assert_eq!(relative(root, "/ws/lib/x.ts"), Path::new("../lib/x.ts"));
    }

    #[cfg(unix)]
    #[test]
    fn relative_follows_symlinked_roots() {
        let dir = std::env::temp_dir().join(format!("hookah-rel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("real/src")).unwrap();
        std::fs::write(dir.join("real/src/a.ts"), "").unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).unwrap();
        let root = dir.join("link");
        let real_file = dunce::canonicalize(dir.join("real/src/a.ts")).unwrap();
        assert_eq!(
            relative(&root, &real_file.to_string_lossy()),
            Path::new("src/a.ts")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn caps_and_collapses_text() {
        assert_eq!(first_line("\n  hello   \nworld"), "hello");
        assert_eq!(cap("a\n  b", 10), "a b");
        assert_eq!(cap("abcdefghij", 8), "abcde...");
    }

    #[test]
    fn unreadable_output_names_the_tool_and_the_tail() {
        let out = ProcessOutput {
            exit_code: Some(2),
            stderr: "\u{1b}[31mboom\u{1b}[39m\n".into(),
            ..ProcessOutput::default()
        };
        assert_eq!(
            unreadable("eslint", &out).message(),
            "eslint failed (exit 2): boom"
        );
        let killed = ProcessOutput::default();
        assert_eq!(
            unreadable("tsc", &killed).message(),
            "tsc failed (exit none): no output"
        );
    }
}
