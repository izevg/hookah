//! Vulture's text report of unused and unreachable code.

use hookah_application::error::PortError;
use hookah_application::ports::ProcessOutput;
use hookah_domain::finding::{Finding, Location, Severity};
use hookah_domain::toolchain::CheckKind;

use super::{Root, failure};

pub(super) fn parse(root: &Root, out: &ProcessOutput) -> Result<Vec<Finding>, PortError> {
    // Vulture can exit with 3 when one file failed to parse but it found dead code in another.
    if out.timed_out || !out.stderr.trim().is_empty() {
        return Err(failure("vulture", out));
    }
    match out.exit_code {
        Some(0) => return Ok(vec![]),
        Some(3) => {}
        _ => return Err(failure("vulture", out)),
    }
    let findings: Vec<_> = out
        .stdout
        .lines()
        .filter_map(|line| finding(root, line))
        .collect();
    if findings.is_empty() {
        return Err(PortError::new(
            "unreadable Vulture report (use make_whitelist = false for diagnostics)",
        ));
    }
    Ok(findings)
}

fn finding(root: &Root, line: &str) -> Option<Finding> {
    let (diagnostic, confidence) = line.strip_suffix(')')?.rsplit_once(" (")?;
    // In size order, Vulture adds ", N lines" inside the confidence suffix.
    let confidence = confidence
        .split(", ")
        .next()?
        .strip_suffix("% confidence")?;
    let confidence = confidence.parse::<u8>().ok()?;
    // A path can contain a drive letter or a colon followed by a space.
    let (file, row, message) = diagnostic.match_indices(": ").find_map(|(i, _)| {
        let (file, row) = diagnostic[..i].rsplit_once(':')?;
        Some((file, row.parse().ok()?, &diagnostic[i + 2..]))
    })?;
    Some(Finding::new(
        CheckKind::Unused,
        Severity::Warning,
        Location::line(root.relative(file), row),
        format!("{message} ({confidence}% confidence)"),
    ))
}
