//! Human-readable output for `hookah check` and `hookah detect`.
//!
//! Colors only on a TTY. Harness hook responses are not rendered here; harness adapters own that
//! format.

use std::io::IsTerminal;
use std::path::Path;

use hookah_adapters::ecosystems::cache::Origin;
use hookah_application::{Outcome, ProjectStatus};
use hookah_domain::path::slashed;
use hookah_domain::project::Project;
use hookah_domain::status::{Standing, overall};
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

/// Renders `hookah status` as one JSON document:
///
/// ```json
/// {"v":1,"root":"/w","overall":"fail","projects":[{"root":"/w","status":{"phase":"stop",
///  "standing":"fail","problems":3,"detail":null,"at_ms":1760000000000,"lines":["a.rs:1: msg"]}}]}
/// ```
///
/// `overall` is the worst standing of the projects that have a record, `null` when none has;
/// a project without a record has `"status": null`.
pub fn status_json(root: &Path, found: &[ProjectStatus]) -> String {
    use serde_json::{Value, json};
    let projects: Vec<Value> = found
        .iter()
        .map(|p| {
            let status = p.recorded.as_ref().map(|r| {
                json!({
                    "phase": r.status.phase.name(),
                    "standing": r.status.standing.name(),
                    "problems": r.status.problems,
                    "detail": r.status.detail,
                    "at_ms": r.at_ms,
                    "lines": r.status.lines,
                })
            });
            json!({"root": p.root.display().to_string(), "status": status})
        })
        .collect();
    let worst = overall(
        found
            .iter()
            .filter_map(|p| p.recorded.as_ref().map(|r| r.status.standing)),
    );
    let doc = json!({
        "v": 1,
        "root": root.display().to_string(),
        "overall": worst.map(Standing::name),
        "projects": projects,
    });
    format!("{doc}\n")
}

/// Renders `hookah status` for people: one line per project, then its first report lines.
pub fn status_text(found: &[ProjectStatus]) -> String {
    if found.is_empty() {
        return "no project found\n".into();
    }
    let mut out = String::new();
    for p in found {
        let root = p.root.display();
        let Some(recorded) = &p.recorded else {
            out.push_str(&format!("{root}: no run recorded yet\n"));
            continue;
        };
        let s = &recorded.status;
        let phase = s.phase.name();
        match s.standing {
            Standing::Pass => out.push_str(&format!("{root}: ok after the last {phase} check\n")),
            Standing::Degraded => out.push_str(&format!(
                "{root}: skipped after the last {phase} check: {}\n",
                s.detail.as_deref().unwrap_or("unknown reason")
            )),
            Standing::Fail => out.push_str(&format!(
                "{root}: {} after the last {phase} check\n",
                match s.problems {
                    1 => "1 problem".to_owned(),
                    n => format!("{n} problems"),
                }
            )),
        }
        for line in &s.lines {
            out.push_str(&format!("  {line}\n"));
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::PathBuf;

    use hookah_domain::session::Phase;
    use hookah_domain::status::{RecordedStatus, Status};

    use super::*;

    fn recorded(standing: Standing, problems: usize, detail: Option<&str>) -> ProjectStatus {
        ProjectStatus {
            root: "/w/a".into(),
            recorded: Some(RecordedStatus {
                at_ms: 42,
                status: Status {
                    phase: Phase::Stop,
                    standing,
                    problems,
                    detail: detail.map(str::to_owned),
                    lines: if problems > 0 {
                        vec!["a.rs:1: msg".into()]
                    } else {
                        vec![]
                    },
                },
            }),
        }
    }

    fn unrecorded(root: &str) -> ProjectStatus {
        ProjectStatus {
            root: root.into(),
            recorded: None,
        }
    }

    #[test]
    fn status_json_is_one_stable_document() {
        let found = [recorded(Standing::Fail, 3, None), unrecorded("/w/b")];
        let text = status_json(Path::new("/w"), &found);
        assert!(text.ends_with('\n'));
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["v"], 1);
        assert_eq!(doc["root"], "/w");
        assert_eq!(doc["overall"], "fail");
        assert_eq!(doc["projects"][0]["root"], "/w/a");
        assert_eq!(doc["projects"][0]["status"]["phase"], "stop");
        assert_eq!(doc["projects"][0]["status"]["standing"], "fail");
        assert_eq!(doc["projects"][0]["status"]["problems"], 3);
        assert_eq!(doc["projects"][0]["status"]["at_ms"], 42);
        assert_eq!(
            doc["projects"][0]["status"]["detail"],
            serde_json::Value::Null
        );
        assert_eq!(doc["projects"][0]["status"]["lines"][0], "a.rs:1: msg");
        assert_eq!(doc["projects"][1]["status"], serde_json::Value::Null);
    }

    #[test]
    fn status_json_without_records_has_no_overall() {
        let text = status_json(Path::new("/w"), &[unrecorded("/w")]);
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["overall"], serde_json::Value::Null);
        let text = status_json(Path::new("/w"), &[]);
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["projects"], serde_json::json!([]));
    }

    #[test]
    fn overall_is_the_worst_standing() {
        let found = [
            recorded(Standing::Pass, 0, None),
            recorded(Standing::Degraded, 0, Some("slow")),
        ];
        let doc: serde_json::Value =
            serde_json::from_str(&status_json(Path::new("/w"), &found)).unwrap();
        assert_eq!(doc["overall"], "degraded");
        assert_eq!(doc["projects"][1]["status"]["detail"], "slow");
    }

    #[test]
    fn status_text_reads_one_line_per_project() {
        assert_eq!(status_text(&[]), "no project found\n");
        assert_eq!(
            status_text(&[unrecorded("/w")]),
            "/w: no run recorded yet\n"
        );
        assert_eq!(
            status_text(&[recorded(Standing::Pass, 0, None)]),
            "/w/a: ok after the last stop check\n"
        );
        assert_eq!(
            status_text(&[recorded(Standing::Degraded, 0, Some("slow"))]),
            "/w/a: skipped after the last stop check: slow\n"
        );
        assert_eq!(
            status_text(&[recorded(Standing::Fail, 1, None)]),
            "/w/a: 1 problem after the last stop check\n  a.rs:1: msg\n"
        );
        assert!(
            status_text(&[recorded(Standing::Fail, 2, None)]).starts_with("/w/a: 2 problems after")
        );
    }

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
