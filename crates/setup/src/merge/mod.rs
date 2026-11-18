//! Pure merges: existing config text in, new config text out.
//!
//! Each format keeps the user's content. JSON goes through `serde_json` with `preserve_order`:
//! key order and values survive, but whitespace is normalized to two-space indentation (a
//! format-preserving JSON editor is not worth a dependency for files an agent or a tool already
//! rewrites). TOML goes through `toml_edit`, which keeps comments, spacing, and order.
//!
//! An error is a plain reason string; the caller adds the path.

mod codex;
mod json;
mod kimi;

use hookah_harness::opencode::PLUGIN_SOURCE;

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
        Target::Codex => codex::merge(target, existing, &command),
        Target::Kimi => kimi::merge(target, existing, &command),
        Target::Opencode => Ok(PLUGIN_SOURCE.to_owned()),
    }
}
