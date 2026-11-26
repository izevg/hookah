//! Antigravity puts named hook groups at the top of hooks.json.

use serde_json::{Map, Value, json};

use super::super::Unmerged;
use crate::target::Target;

const GROUP: &str = "hookah";

pub(in crate::merge) fn merge(existing: Option<&str>, command: &str) -> Result<String, String> {
    let mut top = existing.map(parse).transpose()?.unwrap_or_default();
    let mut group = match top.get(GROUP) {
        None => Map::new(),
        Some(group) => {
            let conflict = "the `hookah` hook group is already used by another command";
            let mut group = group.as_object().cloned().ok_or(conflict)?;
            if !strip(&mut group) {
                return Err(conflict.into());
            }
            group
        }
    };
    for spec in Target::Antigravity.hooks() {
        let hook = json!({"type": "command", "command": command, "timeout": spec.timeout});
        let value = match spec.matcher {
            Some(matcher) => json!({"matcher": matcher, "hooks": [hook]}),
            None => hook,
        };
        let list = group
            .entry(spec.event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| format!("`hookah.{}` is not an array", spec.event))?;
        list.push(value);
    }
    top.insert(GROUP.into(), Value::Object(group));
    pretty(&top)
}

pub(in crate::merge) fn unmerge(text: &str) -> Result<Unmerged, String> {
    let mut top = parse(text)?;
    let Some(group) = top.get_mut(GROUP).and_then(Value::as_object_mut) else {
        return Ok(Unmerged::Unchanged);
    };
    if !strip(group) {
        return Ok(Unmerged::Unchanged);
    }
    if group.is_empty() {
        top.shift_remove(GROUP);
    }
    Ok(Unmerged::Removed {
        text: pretty(&top)?,
        empty: top.is_empty(),
    })
}

/// Whether the named group contains one of Hookah's commands.
pub(crate) fn configured(text: &str) -> bool {
    let Ok(mut top) = parse(text) else {
        return false;
    };
    top.get_mut(GROUP)
        .and_then(Value::as_object_mut)
        .is_some_and(strip)
}

fn is_ours(hook: &Value) -> bool {
    super::super::json::is_ours(hook, Target::Antigravity.name())
        || hook["command"]
            .as_str()
            .is_some_and(|command| command.ends_with(" hook antigravity"))
}

/// Retain any hook the user added to the same group, including shared matcher groups.
fn strip(group: &mut Map<String, Value>) -> bool {
    let mut changed = false;
    group.retain(|event, value| {
        let Some(entries) = value.as_array_mut() else {
            return true;
        };
        let before = entries.len();
        entries.retain_mut(|entry| {
            if matches!(event.as_str(), "PreToolUse" | "PostToolUse") {
                let Some(hooks) = entry.get_mut("hooks").and_then(Value::as_array_mut) else {
                    return true;
                };
                let before = hooks.len();
                hooks.retain(|hook| !is_ours(hook));
                changed |= before != hooks.len();
                before == hooks.len() || !hooks.is_empty()
            } else {
                !is_ours(entry)
            }
        });
        changed |= before != entries.len();
        before == entries.len() || !entries.is_empty()
    });
    changed
}

fn parse(text: &str) -> Result<Map<String, Value>, String> {
    serde_json::from_str(text).map_err(|e| format!("not a valid JSON object ({e})"))
}

fn pretty(top: &Map<String, Value>) -> Result<String, String> {
    serde_json::to_string_pretty(top)
        .map(|text| format!("{text}\n"))
        .map_err(|e| e.to_string())
}
