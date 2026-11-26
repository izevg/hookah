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
pub(crate) mod zcode;

use hookah_harness::opencode::PLUGIN_SOURCE;
use hookah_harness::plugins::hermes::{PLUGIN_MANIFEST, PLUGIN_SOURCE as HERMES_PLUGIN_SOURCE};
use hookah_harness::plugins::openclaw::{
    MANIFEST as OPENCLAW_MANIFEST, PLUGIN_SOURCE as OPENCLAW_PLUGIN_SOURCE,
};
use hookah_harness::plugins::pi::PLUGIN_SOURCE as PI_PLUGIN_SOURCE;

use crate::target::{Program, Target};

/// Merges hookah's hooks for `target` into `existing` (`None` when the file does not exist).
///
/// `program` is the command prefix that starts hookah, for example `/home/u/.local/bin/hookah`;
/// each hook runs `<program> hook <target>`.
pub fn merge<'a>(
    target: Target,
    existing: Option<&str>,
    program: impl Into<Program<'a>>,
) -> Result<String, String> {
    let program = program.into();
    let existing = existing.filter(|text| !text.trim().is_empty());
    let command = || format!("{} hook {}", program.shell_command(), target.name());
    match target {
        Target::Claude | Target::Qoder => json::merge(target, existing, &command()),
        Target::Codex => toml::codex::merge(target, existing, &command()),
        Target::Kimi => toml::kimi::merge(target, existing, &command()),
        Target::Cursor => cursor::merge(target, existing, &command()),
        Target::Hermes => yaml::merge(existing),
        Target::Zcode => zcode::merge(existing, program),
        Target::Opencode | Target::Pi | Target::Openclaw => {
            Ok(plugin_text(target).unwrap_or_default().to_owned())
        }
    }
}

/// The text of the one file a plugin harness gets as its config file: the whole plugin. `None`
/// for the harnesses whose config file is merged (Hermes's plugin is a companion file).
pub fn plugin_text(target: Target) -> Option<&'static str> {
    match target {
        Target::Opencode => Some(PLUGIN_SOURCE),
        Target::Pi => Some(PI_PLUGIN_SOURCE),
        Target::Openclaw => Some(OPENCLAW_PLUGIN_SOURCE),
        _ => None,
    }
}

/// What [`unmerge`] found in a config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unmerged {
    /// hookah has no entry in the file; nothing to do.
    Unchanged,
    /// The file without hookah's entries.
    Removed {
        /// The new text.
        text: String,
        /// True when nothing is left in it but what setup alone would have written, so the file
        /// can go if setup created it.
        empty: bool,
    },
}

/// The inverse of [`merge`]: `existing` without hookah's entries. Entries are recognized the way
/// merge recognizes them (`hookah hook <name>` in the command), and a group, an event, or a
/// table is dropped only when this removal emptied it. Everything else, including comments and
/// order, stays. A harness whose config file is a whole plugin has no entries to remove here, so
/// it is always [`Unmerged::Unchanged`].
pub fn unmerge(target: Target, existing: &str) -> Result<Unmerged, String> {
    if existing.trim().is_empty() {
        return Ok(Unmerged::Unchanged);
    }
    match target {
        Target::Claude | Target::Qoder => json::unmerge(target, existing),
        Target::Codex => toml::codex::unmerge(target, existing),
        Target::Kimi => toml::kimi::unmerge(target, existing),
        Target::Cursor => cursor::unmerge(target, existing),
        Target::Hermes => yaml::unmerge(existing),
        Target::Zcode => zcode::unmerge(existing, None),
        Target::Opencode | Target::Pi | Target::Openclaw => Ok(Unmerged::Unchanged),
    }
}

/// The files `target` needs besides its config file: `(path relative to the harness's user
/// directory, content)`. Hermes has its plugin directory (the config file merely enables it),
/// and OpenClaw its plugin manifest next to the entry point.
pub fn companions(target: Target) -> &'static [(&'static str, &'static str)] {
    match target {
        Target::Hermes => &[
            ("plugins/hookah/plugin.yaml", PLUGIN_MANIFEST),
            ("plugins/hookah/__init__.py", HERMES_PLUGIN_SOURCE),
        ],
        Target::Openclaw => &[("extensions/hookah/openclaw.plugin.json", OPENCLAW_MANIFEST)],
        _ => &[],
    }
}
