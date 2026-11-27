//! Unref's concise project-wide diagnostics.

use hookah_application::error::PortError;
use hookah_application::ports::ProcessOutput;
use hookah_domain::finding::{Finding, Location, Severity};
use hookah_domain::toolchain::CheckKind;

use super::super::{Root, clip, failure, parse_position};

pub(in crate::ecosystems::python::parse) fn parse(
    root: &Root,
    out: &ProcessOutput,
) -> Result<Vec<Finding>, PortError> {
    if out.timed_out || !out.stderr.trim().is_empty() {
        return Err(failure("unref", out));
    }
    match out.exit_code {
        Some(0) if out.stdout.trim().is_empty() => return Ok(Vec::new()),
        Some(1) if !out.stdout.trim().is_empty() => {}
        _ => return Err(failure("unref", out)),
    }
    out.stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (location, code, message) = diagnostic(root, line).ok_or_else(|| {
                PortError::new(format!("unreadable Unref diagnostic: {}", clip(line, 200)))
            })?;
            // A skipped file could still reference definitions reported as unused elsewhere.
            if matches!(code, "invalid-syntax" | "io-error") {
                return Err(PortError::new(format!("unref: {}", clip(line, 200))));
            }
            Ok(Finding::new(
                CheckKind::Unused,
                Severity::Warning,
                location,
                message,
            ))
        })
        .collect()
}

fn diagnostic<'a>(root: &Root, line: &'a str) -> Option<(Location, &'a str, &'a str)> {
    // A file path can contain colons, including the Windows drive and a literal ": " on Unix.
    line.match_indices(": ").find_map(|(index, _)| {
        let (file, row, column) = parse_position(&line[..index])?;
        let location = Location::at(root.relative(file), row?, column?);
        let message = &line[index + 2..];
        let (code, _) = message.split_once(' ')?;
        Some((location, code, message))
    })
}
