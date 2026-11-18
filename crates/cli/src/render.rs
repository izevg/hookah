//! Human-readable output for `hookah check` and `hookah detect`.
//!
//! Colors only on a TTY. Harness hook responses are not rendered here; harness adapters own that
//! format.

use std::io::IsTerminal;
use std::path::Path;

use hookah_adapters::ecosystems::cache::Origin;
use hookah_application::Outcome;
use hookah_domain::path::slashed;
use hookah_domain::project::Project;
use hookah_domain::toolchain::Toolchain;
use hookah_domain::verdict::Verdict;

/// Output styling.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    color: bool,
}

impl Style {
    /// Colors on when standard output is a terminal.
    pub fn for_stdout() -> Self {
        Self {
            color: std::io::stdout().is_terminal(),
        }
    }

    /// No colors.
    #[cfg(test)]
    pub fn plain() -> Self {
        Self { color: false }
    }

    fn paint(self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }
}

/// Renders the outcome of checking `path`.
pub fn outcome(style: Style, path: &Path, outcome: &Outcome) -> String {
    let mut out = String::new();
    match &outcome.verdict {
        Verdict::Pass => out.push_str(&format!("{} {}\n", style.paint("32", "ok"), path.display())),
        Verdict::Fail(report) => {
            out.push_str(&format!(
                "{} {}\n",
                style.paint("31;1", "FAIL"),
                path.display()
            ));
            for line in report.lines() {
                out.push_str(&format!("  {line}\n"));
            }
        }
        Verdict::Degraded(reason) => out.push_str(&format!(
            "{} {}: {reason}\n",
            style.paint("33", "skipped"),
            path.display()
        )),
    }
    for notice in &outcome.notices {
        out.push_str(&format!("  {}\n", style.paint("33", &notice.to_string())));
    }
    out
}

/// Renders the located project and its toolchain.
///
/// The `cache:` line says whether the toolchain came from the detection cache (`hit`), was
/// computed and stored (`miss`), or was computed with the cache switched off (`off`).
pub fn detection(style: Style, project: Option<(&Project, &Toolchain, Origin)>) -> String {
    let Some((project, toolchain, origin)) = project else {
        return "no project found\n".into();
    };
    let mut out = format!(
        "project: {}\necosystem: {}\ncache: {origin}\n",
        slashed(&project.root),
        project.ecosystem.name()
    );
    for m in toolchain.missing() {
        out.push_str(&format!(
            "missing: {} {}: {}\n",
            m.kind.name(),
            m.name,
            m.hint
        ));
    }
    if toolchain.tools().is_empty() {
        out.push_str("tools: none detected\n");
        return out;
    }
    out.push_str("tools:\n");
    for tool in toolchain.tools() {
        let args = tool.command.expand_args(&["<targets>".to_owned()]);
        out.push_str(&format!(
            "  {} {}: {} {}\n",
            style.paint("1", tool.kind.name()),
            tool.name,
            slashed(Path::new(&tool.command.program)),
            args.join(" ")
        ));
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn pass_and_degraded_render_without_color() {
        let p = PathBuf::from("a.rs");
        assert_eq!(outcome(Style::plain(), &p, &Outcome::pass()), "ok a.rs\n");
        assert_eq!(
            outcome(Style::plain(), &p, &Outcome::degraded("boom")),
            "skipped a.rs: boom\n"
        );
    }

    #[test]
    fn color_wraps_in_ansi() {
        let s = Style { color: true };
        assert_eq!(s.paint("32", "ok"), "\x1b[32mok\x1b[0m");
    }

    #[test]
    fn detection_names_the_origin_of_the_toolchain() {
        use hookah_domain::project::Ecosystem;

        let project = Project {
            root: "/r".into(),
            ecosystem: Ecosystem::Rust,
            manifests: vec![],
        };
        let toolchain = Toolchain::default();
        for (origin, line) in [
            (Origin::Hit, "cache: hit\n"),
            (Origin::Miss, "cache: miss\n"),
            (Origin::Off, "cache: off\n"),
        ] {
            let text = detection(Style::plain(), Some((&project, &toolchain, origin)));
            assert!(
                text.starts_with(&format!("project: /r\necosystem: rust\n{line}")),
                "{text}"
            );
        }
    }

    #[test]
    fn no_project_message() {
        assert_eq!(detection(Style::plain(), None), "no project found\n");
    }
}
