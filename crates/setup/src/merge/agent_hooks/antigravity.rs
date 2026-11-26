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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const MIXED: &str = r#"{
        "other": {"Stop": [{"command": "hookah hook antigravity"}]},
        "hookah": {
            "PreToolUse": [],
            "PostToolUse": [
                {"matcher": "valid", "hooks": [
                    {"command": "/opt/bin/hookah-dev hook antigravity"},
                    {"command": "user-format"}
                ]},
                {"matcher": "owned", "hooks": [{"command": "hookah hook antigravity"}]},
                {"matcher": "empty", "hooks": []},
                {"matcher": "custom"}
            ],
            "PreInvocation": [{"command": "hookah hook antigravity"}],
            "Stop": [
                {"command": "hookah hook antigravity --report-limit 3"},
                {"command": "user-stop"}
            ],
            "future": {"user": true}
        }
    }"#;

    #[test]
    fn setup_creates_the_expected_events_and_is_idempotent() {
        let installed = crate::merge::merge(Target::Antigravity, None, "hookah").unwrap();
        let value: Value = serde_json::from_str(&installed).unwrap();

        assert_eq!(
            value,
            json!({"hookah": {
                "SessionStart": [{"type": "command", "command": "hookah hook antigravity", "timeout": 30}],
                "PostToolUse": [{
                    "matcher": hookah_harness::antigravity::EDIT_TOOLS,
                    "hooks": [{"type": "command", "command": "hookah hook antigravity", "timeout": 30}]
                }],
                "PreInvocation": [{"type": "command", "command": "hookah hook antigravity", "timeout": 5}],
                "Stop": [{"type": "command", "command": "hookah hook antigravity", "timeout": 120}]
            }})
        );
        assert_eq!(
            crate::merge::merge(Target::Antigravity, Some(&installed), "hookah").unwrap(),
            installed
        );
        assert_eq!(
            crate::merge::unmerge(Target::Antigravity, &installed).unwrap(),
            Unmerged::Removed {
                text: "{}\n".into(),
                empty: true
            }
        );
    }

    #[test]
    fn uninstall_preserves_user_hooks_and_only_drops_newly_emptied_groups() {
        let Unmerged::Removed { text, empty } =
            crate::merge::unmerge(Target::Antigravity, MIXED).unwrap()
        else {
            panic!("expected to remove managed hooks");
        };
        assert!(!empty);
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            json!({
                "other": {"Stop": [{"command": "hookah hook antigravity"}]},
                "hookah": {
                    "PreToolUse": [],
                    "PostToolUse": [
                        {"matcher": "valid", "hooks": [{"command": "user-format"}]},
                        {"matcher": "empty", "hooks": []},
                        {"matcher": "custom"}
                    ],
                    "Stop": [{"command": "user-stop"}],
                    "future": {"user": true}
                }
            })
        );
        assert_eq!(unmerge(&text).unwrap(), Unmerged::Unchanged);
    }

    #[test]
    fn a_shared_matcher_alone_counts_as_configured_and_can_be_removed() {
        let original = r#"{"hookah":{"PostToolUse":[{"matcher":"edit","hooks":[
            {"command":"/opt/bin/hookah-dev hook antigravity"},
            {"command":"user-format"}
        ]}]}}"#;
        assert!(configured(original));
        let Unmerged::Removed { text, .. } = unmerge(original).unwrap() else {
            panic!("expected to remove a managed command from the shared matcher");
        };
        assert!(!configured(&text));
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            json!({"hookah":{"PostToolUse":[{"matcher":"edit","hooks":[{"command":"user-format"}]}]}})
        );
    }

    #[test]
    fn repair_updates_only_managed_commands_in_mixed_groups() {
        let updated =
            crate::merge::merge(Target::Antigravity, Some(MIXED), "/opt/bin/hookah-next").unwrap();
        let value: Value = serde_json::from_str(&updated).unwrap();

        assert_eq!(
            value["hookah"]["Stop"],
            json!([
                {"command": "user-stop"},
                {"type": "command", "command": "/opt/bin/hookah-next hook antigravity", "timeout": 120}
            ])
        );
        assert_eq!(unmerge(&updated).unwrap(), unmerge(MIXED).unwrap());
    }

    #[test]
    fn malformed_configs_are_refused() {
        for original in [
            r#"[]"#,
            r#"{"hookah":"reserved"}"#,
            r#"{"hookah":{"PostToolUse":{},"Stop":[{"command":"hookah hook antigravity"}]}}"#,
        ] {
            assert!(
                merge(Some(original), "hookah hook antigravity").is_err(),
                "{original}"
            );
        }
        assert!(unmerge("invalid JSON").is_err());
    }

    #[test]
    fn detection_looks_inside_the_managed_group_only() {
        assert!(!configured("not valid JSON"));
        assert!(!configured(
            r#"{"hookah":{"Stop":[{"command":"another-program"}]}}"#
        ));
        assert!(!configured(
            r#"{"other":{"Stop":[{"command":"hookah hook antigravity"}]}}"#
        ));
        assert!(configured(
            r#"{"hookah":{"Stop":[{"command":"hookah hook antigravity"}]}}"#
        ));
    }

    #[test]
    fn unrelated_group_is_neither_detected_nor_removed() {
        let original = r#"{"hookah":{"Stop":[{"command":"another-program"}]}}"#;
        assert_eq!(unmerge(original).unwrap(), Unmerged::Unchanged);
        assert!(merge(Some(original), "hookah hook antigravity").is_err());
    }
}
