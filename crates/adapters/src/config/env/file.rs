//! One config file: the TOML shape and its parser.
//!
//! The same shape serves all three sources. A project file (`.hookah.toml`,
//! `.hookah.local.toml`) uses `[env]`; the user config uses `[[project]]` sections, each with a
//! `path` and an `[project.env]`. A source ignores the part it does not use. Unknown keys are
//! ignored, so a file written for a newer `hookah` still gives an older one its `[env]`.

use std::collections::BTreeMap;

use serde::Deserialize;

/// A parsed config file.
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
pub struct ConfigFile {
    /// Variables for the project the file belongs to.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Per-project sections of the user config, in file order.
    #[serde(default)]
    pub project: Vec<ProjectSection>,
    /// Self-update switches of the user config.
    #[serde(default)]
    pub update: UpdateSection,
}

/// The `[update]` table of the user config.
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
pub struct UpdateSection {
    /// `false` switches every update check and install off.
    pub check: Option<bool>,
    /// `false` keeps the check and only notifies, never installs by itself.
    pub auto: Option<bool>,
}

/// A `[[project]]` section: variables for every project whose path matches.
#[derive(Debug, PartialEq, Eq, Deserialize)]
pub struct ProjectSection {
    /// Path prefix or glob, matched over the canonical project root.
    pub path: String,
    /// Variables to set.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Parses `text`; the error is a one-line reason.
pub fn parse(text: &str) -> Result<ConfigFile, String> {
    toml_edit::de::from_str(text).map_err(|e| e.to_string().replace('\n', " ").trim().to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn reads_env_and_project_sections() {
        let file = parse(
            "[env]\nA = \"1\"\n[[project]]\npath = \"/x/**\"\n[project.env]\nB = \"2\"\n[[project]]\npath = \"/y\"\n",
        )
        .unwrap();
        assert_eq!(file.env["A"], "1");
        assert_eq!(file.project.len(), 2);
        assert_eq!(file.project[0].path, "/x/**");
        assert_eq!(file.project[0].env["B"], "2");
        assert!(file.project[1].env.is_empty());
    }

    #[test]
    fn an_empty_file_and_unknown_keys_are_fine() {
        assert_eq!(parse("").unwrap(), ConfigFile::default());
        assert_eq!(
            parse("future = 1\n[tools]\nx = 2\n").unwrap(),
            ConfigFile::default()
        );
    }

    #[test]
    fn reads_the_update_table() {
        let file = parse("[update]\ncheck = false\nauto = true\nfuture = 1\n").unwrap();
        assert_eq!(file.update.check, Some(false));
        assert_eq!(file.update.auto, Some(true));
        assert_eq!(
            parse("[update]\n").unwrap().update,
            UpdateSection::default()
        );
        assert_eq!(parse("").unwrap().update.check, None);
        assert!(parse("[update]\ncheck = \"no\"\n").is_err());
    }

    #[test]
    fn malformed_input_is_an_error() {
        assert!(parse("[env\nA=1").is_err());
        assert!(parse("[env]\nA = 1\n").is_err(), "values must be strings");
        assert!(parse("[[project]]\n").is_err(), "a section needs a path");
        assert!(!parse("[env]\nA = 1\n").unwrap_err().contains('\n'));
    }
}
