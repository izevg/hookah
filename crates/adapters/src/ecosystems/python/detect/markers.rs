//! Registered pytest markers.
//!
//! The stop phase deselects slow tests with `-m "not (integration or e2e or slow)"`, but only
//! with the markers the project registers: under `--strict-markers` (or `strict_markers`) an
//! unknown marker is an error, and an unregistered one cannot be on any test anyway. The markers
//! live in `[tool.pytest.ini_options]` of `pyproject.toml`, `[pytest]` of `pytest.ini` and
//! `tox.ini`, and `[tool:pytest]` of `setup.cfg`, as a `markers` key. Like the rest of the
//! configuration reading, this is line by line, without a TOML or INI parser.

use super::config::leading_name;

/// The markers that mean a test is not light.
pub const HEAVY_MARKERS: [&str; 3] = ["integration", "e2e", "slow"];

/// The names registered under `markers` in the section `name` of `text`. `toml` selects the array
/// syntax (`markers = ["slow: why", ...]`) over the INI one (indented `name: why` lines).
pub fn registered(text: &str, name: &str, toml: bool) -> Vec<String> {
    let mut inside = false;
    let mut collecting = false;
    let mut names = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(inner) = line.strip_prefix('[')
            && line.ends_with(']')
            && !inner.contains('"')
        {
            let inner = inner.strip_prefix('[').unwrap_or(inner);
            inside = inner.trim_end_matches(']').trim() == name;
            collecting = false;
        } else if !inside {
            continue;
        } else if collecting {
            if toml {
                names.extend(quoted_names(line));
                collecting = !line.contains(']');
            } else if raw.starts_with([' ', '\t']) && !line.is_empty() {
                names.push(leading_name(line).to_string());
            } else if !line.is_empty() {
                collecting = false;
            }
        }
        if inside
            && !collecting
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "markers"
        {
            let value = value.trim();
            if toml {
                names.extend(quoted_names(value));
                collecting = !value.contains(']');
            } else {
                if !value.is_empty() {
                    names.push(leading_name(value).to_string());
                }
                collecting = true;
            }
        }
    }
    names.retain(|n| !n.is_empty());
    names
}

/// The name at the start of each quoted string on `line`.
fn quoted_names(line: &str) -> Vec<String> {
    line.split(['"', '\''])
        .skip(1)
        .step_by(2)
        .map(|s| leading_name(s.trim()).to_string())
        .collect()
}

/// The pytest `-m` expression that deselects the heavy markers in `registered`, or `None` when
/// the project registers none of them.
pub fn deselect_expression(registered: &[String]) -> Option<String> {
    let heavy: Vec<&str> = HEAVY_MARKERS
        .into_iter()
        .filter(|m| registered.iter().any(|r| r == m))
        .collect();
    (!heavy.is_empty()).then(|| format!("not ({})", heavy.join(" or ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_toml_array_in_every_layout() {
        let multi = "[tool.pytest.ini_options]\nminversion = \"7\"\nmarkers = [\n  \"slow: marks slow\",\n  'integration: db',\n]\naddopts = \"-q\"\n";
        assert_eq!(
            registered(multi, "tool.pytest.ini_options", true),
            ["slow", "integration"]
        );
        let one = "[tool.pytest.ini_options]\nmarkers = [\"e2e: x\", \"unit\"]\n";
        assert_eq!(
            registered(one, "tool.pytest.ini_options", true),
            ["e2e", "unit"]
        );
        let first = "[tool.pytest.ini_options]\nmarkers = [\"a: x\",\n \"b: y\"]\n";
        assert_eq!(
            registered(first, "tool.pytest.ini_options", true),
            ["a", "b"]
        );
    }

    #[test]
    fn reads_the_ini_lines() {
        let text = "[pytest]\naddopts = -q\nmarkers =\n    slow: marks slow\n    integration(x): why\n\n    e2e\ntestpaths = tests\n";
        assert_eq!(
            registered(text, "pytest", false),
            ["slow", "integration", "e2e"]
        );
        let inline = "[tool:pytest]\nmarkers = slow: s\n  e2e: e\n";
        assert_eq!(registered(inline, "tool:pytest", false), ["slow", "e2e"]);
    }

    #[test]
    fn markers_elsewhere_do_not_count() {
        let text = "[tool.other]\nmarkers = [\"slow\"]\n[tool.pytest.ini_options]\naddopts = \"\"\n[tool.after]\nmarkers = [\"e2e\"]\n";
        assert!(registered(text, "tool.pytest.ini_options", true).is_empty());
        assert!(registered("", "pytest", false).is_empty());
        // An INI key after the list ends it.
        let text = "[pytest]\nmarkers =\n    slow: s\nfoo = integration\n";
        assert_eq!(registered(text, "pytest", false), ["slow"]);
    }

    #[test]
    fn the_expression_names_only_registered_heavy_markers() {
        let names = |n: &[&str]| n.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        assert_eq!(deselect_expression(&names(&["unit", "django_db"])), None);
        assert_eq!(deselect_expression(&[]), None);
        assert_eq!(
            deselect_expression(&names(&["slow", "unit"])).as_deref(),
            Some("not (slow)")
        );
        assert_eq!(
            deselect_expression(&names(&["slow", "e2e", "integration"])).as_deref(),
            Some("not (integration or e2e or slow)")
        );
    }
}
