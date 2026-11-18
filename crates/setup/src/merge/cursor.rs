//! Cursor `hooks.json`: `{"version": 1, "hooks": {"<event>": [{"command", "timeout"}]}}`.
//!
//! Unlike Claude Code, Cursor entries are flat: there are no matcher groups around them.

use serde_json::{Map, Value, json};

use crate::target::{HookSpec, Target, mentions_hookah};

/// The `version` Cursor requires at the top of the file.
const VERSION: i64 = 1;

pub fn merge(target: Target, existing: Option<&str>, command: &str) -> Result<String, String> {
    let mut root: Value = match existing {
        None => Value::Object(Map::new()),
        Some(text) => serde_json::from_str(text).map_err(|e| format!("not valid JSON ({e})"))?,
    };
    let top = root
        .as_object_mut()
        .ok_or("the top level is not a JSON object")?;
    top.entry("version").or_insert_with(|| json!(VERSION));
    let hooks = top
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or("`hooks` is not an object")?;
    for spec in target.hooks() {
        let entries = hooks
            .entry(spec.event)
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| format!("`hooks.{}` is not an array", spec.event))?;
        upsert(entries, spec, target.name(), command);
    }
    let mut text = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    text.push('\n');
    Ok(text)
}

/// Updates every hookah entry of the event in place, or appends one when there is none.
fn upsert(entries: &mut Vec<Value>, spec: &HookSpec, name: &str, command: &str) {
    let mut found = false;
    for entry in entries.iter_mut().filter_map(Value::as_object_mut) {
        let ours = entry
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|c| mentions_hookah(c, name));
        if ours {
            entry.insert("command".into(), json!(command));
            entry.insert("timeout".into(), json!(spec.timeout));
            found = true;
        }
    }
    if !found {
        entries.push(json!({ "command": command, "timeout": spec.timeout }));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const CMD: &str = "/bin/hookah hook cursor";

    fn run(existing: Option<&str>) -> Value {
        let text = merge(Target::Cursor, existing, CMD).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn empty_input_yields_the_documented_cursor_hooks() {
        let entry = |t: i64| json!([{"command": CMD, "timeout": t}]);
        assert_eq!(
            run(None),
            json!({"version": 1, "hooks": {
                "sessionStart": entry(30),
                "afterFileEdit": entry(30),
                "stop": entry(120),
            }})
        );
    }

    #[test]
    fn unrelated_settings_and_hooks_survive_in_order() {
        let existing = r#"{"version": 1, "extra": true, "hooks": {
            "beforeShellExecution": [{"command": "./audit.sh"}],
            "stop": [{"command": "./notify.sh", "loop_limit": 3}]}}"#;
        let got = run(Some(existing));
        assert_eq!(got["extra"], true);
        assert_eq!(
            got["hooks"]["beforeShellExecution"][0]["command"],
            "./audit.sh"
        );
        assert_eq!(got["hooks"]["stop"][0]["loop_limit"], 3);
        assert_eq!(got["hooks"]["stop"][1]["command"], CMD);
        let keys: Vec<_> = got["hooks"].as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys[..2], ["beforeShellExecution", "stop"]);
    }

    #[test]
    fn an_existing_hookah_entry_is_updated_in_place_and_keeps_its_extra_keys() {
        let existing = r#"{"version": 1, "hooks": {"stop": [
            {"command": "\"/old dir/hookah.exe\" hook cursor", "timeout": 5, "loop_limit": 2}]}}"#;
        let got = run(Some(existing));
        let stop = got["hooks"]["stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1);
        assert_eq!(
            stop[0],
            json!({"command": CMD, "timeout": 120, "loop_limit": 2})
        );
    }

    #[test]
    fn a_second_run_changes_nothing() {
        let once = merge(Target::Cursor, None, CMD).unwrap();
        assert_eq!(merge(Target::Cursor, Some(&once), CMD).unwrap(), once);
    }

    #[test]
    fn another_version_number_is_kept() {
        assert_eq!(run(Some(r#"{"version": 2}"#))["version"], 2);
    }

    #[test]
    fn broken_files_are_refused() {
        assert!(merge(Target::Cursor, Some("{nope"), CMD).is_err());
        assert!(merge(Target::Cursor, Some("[]"), CMD).is_err());
        assert!(merge(Target::Cursor, Some(r#"{"hooks": []}"#), CMD).is_err());
        assert!(merge(Target::Cursor, Some(r#"{"hooks": {"stop": {}}}"#), CMD).is_err());
    }
}
