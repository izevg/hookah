//! Pure merges: existing config text in, new config text out.
//!
//! Each format keeps the user's content. JSON goes through `serde_json` with `preserve_order`:
//! key order and values survive, but whitespace is normalized to two-space indentation (a
//! format-preserving JSON editor is not worth a dependency for files an agent or a tool already
//! rewrites). TOML goes through `toml_edit`, which keeps comments, spacing, and order. YAML (Hermes)
//! is a small line editor that only appends, so comments and order survive without a YAML crate.
//!
//! An error is a plain reason string; the caller adds the path.

mod cursor;
mod json;
mod toml;
pub(crate) mod yaml;

use hookah_harness::opencode::PLUGIN_SOURCE;
use hookah_harness::plugins::hermes::{PLUGIN_MANIFEST, PLUGIN_SOURCE as HERMES_PLUGIN_SOURCE};
use hookah_harness::plugins::pi::PLUGIN_SOURCE as PI_PLUGIN_SOURCE;

use crate::target::Target;

/// Merges hookah's hooks for `target` into `existing` (`None` when the file does not exist).
///
/// `program` is the command prefix that starts hookah, for example `/home/u/.local/bin/hookah`;
/// each hook runs `<program> hook <target>`.
pub fn merge(target: Target, existing: Option<&str>, program: &str) -> Result<String, String> {
    let command = format!("{program} hook {}", target.name());
    let existing = existing.filter(|text| !text.trim().is_empty());
    match target {
        Target::Claude | Target::Qoder => json::merge(target, existing, &command),
        Target::Codex => toml::codex::merge(target, existing, &command),
        Target::Kimi => toml::kimi::merge(target, existing, &command),
        Target::Cursor => cursor::merge(target, existing, &command),
        Target::Opencode => Ok(PLUGIN_SOURCE.to_owned()),
        Target::Hermes => yaml::merge(existing),
        Target::Pi => Ok(PI_PLUGIN_SOURCE.to_owned()),
    }
}

/// The files `target` needs besides its config file: `(path relative to the harness's user
/// directory, content)`. Only Hermes has any: its plugin is a directory with a manifest, and
/// the config file merely enables it.
pub fn companions(target: Target) -> &'static [(&'static str, &'static str)] {
    match target {
        Target::Hermes => &[
            ("plugins/hookah/plugin.yaml", PLUGIN_MANIFEST),
            ("plugins/hookah/__init__.py", HERMES_PLUGIN_SOURCE),
        ],
        _ => &[],
    }
}
