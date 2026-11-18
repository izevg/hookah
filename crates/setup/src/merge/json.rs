//! Claude Code and Qoder: `{"hooks": {"<Event>": [{"matcher"?, "hooks": [{"type","command","timeout"}]}]}}`.

use serde_json::{Map, Value, json};

use crate::target::{HookSpec, Target, mentions_hookah};

pub fn merge(target: Target, existing: Option<&str>, command: &str) -> Result<String, String> {
    let mut root: Value = match existing {
        None => Value::Object(Map::new()),
        Some(text) => serde_json::from_str(text).map_err(|e| format!("not valid JSON ({e})"))?,
    };
    let top = root
        .as_object_mut()
        .ok_or("the top level is not a JSON object")?;
    let hooks = top
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or("`hooks` is not an object")?;
    for spec in target.hooks() {
        let groups = hooks
            .entry(spec.event)
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| format!("`hooks.{}` is not an array", spec.event))?;
        upsert(groups, spec, target.name(), command);
    }
    let mut text = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    text.push('\n');
    Ok(text)
}

/// Updates every hookah hook of the event in place, or appends a new group when there is none.
fn upsert(groups: &mut Vec<Value>, spec: &HookSpec, name: &str, command: &str) {
    let mut found = false;
    for group in groups.iter_mut().filter_map(Value::as_object_mut) {
        let alone = group
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|h| h.len() == 1);
        let mut touched = false;
        if let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) {
            for hook in hooks.iter_mut().filter_map(Value::as_object_mut) {
                let ours = hook
                    .get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|c| mentions_hookah(c, name));
                if ours {
                    hook.insert("type".into(), json!("command"));
                    hook.insert("command".into(), json!(command));
                    hook.insert("timeout".into(), json!(spec.timeout));
                    touched = true;
                }
            }
        }
        if touched
            && alone
            && let Some(matcher) = spec.matcher
        {
            group.insert("matcher".into(), json!(matcher));
        }
        found |= touched;
    }
    if !found {
        let mut group = Map::new();
        if let Some(matcher) = spec.matcher {
            group.insert("matcher".into(), json!(matcher));
        }
        group.insert(
            "hooks".into(),
            json!([{ "type": "command", "command": command, "timeout": spec.timeout }]),
        );
        groups.push(Value::Object(group));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const CMD: &str = "/bin/hookah hook claude";

    fn run(existing: Option<&str>) -> Result<Value, String> {
        merge(Target::Claude, existing, CMD).map(|t| serde_json::from_str(&t).unwrap())
    }

    #[test]
    fn empty_input_yields_the_documented_claude_settings() {
        let got = run(None).unwrap();
        let hook = |t: i64| json!([{"type": "command", "command": CMD, "timeout": t}]);
        assert_eq!(
            got,
            json!({"hooks": {
                "SessionStart": [{"matcher": "startup|resume", "hooks": hook(30)}],
                "PostToolUse": [{"matcher": "Edit|Write|MultiEdit|NotebookEdit", "hooks": hook(30)}],
                "Stop": [{"hooks": hook(120)}],
                "SubagentStop": [{"hooks": hook(120)}],
            }})
        );
    }

    #[test]
    fn qoder_session_start_has_no_matcher() {
        let text = merge(Target::Qoder, None, "hookah hook qoder").unwrap();
        let got: Value = serde_json::from_str(&text).unwrap();
        assert!(got["hooks"]["SessionStart"][0].get("matcher").is_none());
        assert_eq!(got["hooks"]["PostToolUse"][0]["matcher"], "Write|Edit");
    }

    #[test]
    fn blank_input_is_treated_as_empty() {
        use crate::merge::merge as dispatch;
        assert_eq!(
            dispatch(Target::Claude, Some("  \n"), "/bin/hookah").unwrap(),
            dispatch(Target::Claude, None, "/bin/hookah").unwrap()
        );
    }

    #[test]
    fn unrelated_settings_and_their_order_survive() {
        let existing = r#"{"zeta": 1, "model": "x", "hooks": {"PostToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "lint.sh"}]}], "Notification": []}, "alpha": [1, 2]}"#;
        let text = merge(Target::Claude, Some(existing), CMD).unwrap();
        let got: Value = serde_json::from_str(&text).unwrap();
        let keys: Vec<_> = got.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["zeta", "model", "hooks", "alpha"]);
        let hook_keys: Vec<_> = got["hooks"].as_object().unwrap().keys().collect();
        assert_eq!(hook_keys[..2], ["PostToolUse", "Notification"]);
        let post = got["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post.len(), 2);
        assert_eq!(post[0]["hooks"][0]["command"], "lint.sh");
        assert_eq!(post[1]["hooks"][0]["command"], CMD);
        assert_eq!(got["alpha"], json!([1, 2]));
    }

    #[test]
    fn an_existing_hookah_entry_is_updated_in_place() {
        let existing = r#"{"hooks": {"Stop": [
            {"hooks": [{"type": "command", "command": "other.sh"}]},
            {"hooks": [{"type": "command", "command": "mise exec -- hookah hook claude", "timeout": 5, "extra": true}]}
        ]}}"#;
        let got = run(Some(existing)).unwrap();
        let stop = got["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[1]["hooks"][0]["command"], CMD);
        assert_eq!(stop[1]["hooks"][0]["timeout"], 120);
        assert_eq!(stop[1]["hooks"][0]["extra"], true);
    }

    #[test]
    fn a_lone_hookah_hook_gets_the_documented_matcher_back() {
        let existing = r#"{"hooks": {"PostToolUse": [{"matcher": "Edit", "hooks": [{"type": "command", "command": "hookah hook claude"}]}]}}"#;
        let got = run(Some(existing)).unwrap();
        let post = got["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post.len(), 1);
        assert_eq!(post[0]["matcher"], "Edit|Write|MultiEdit|NotebookEdit");
    }

    #[test]
    fn a_shared_group_keeps_its_matcher() {
        let existing = r#"{"hooks": {"PostToolUse": [{"matcher": "Edit", "hooks": [{"type": "command", "command": "a.sh"}, {"type": "command", "command": "hookah hook claude"}]}]}}"#;
        let got = run(Some(existing)).unwrap();
        assert_eq!(got["hooks"]["PostToolUse"][0]["matcher"], "Edit");
    }

    #[test]
    fn another_harness_entry_is_not_ours() {
        let existing = r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "hookah hook codex"}]}]}}"#;
        let got = run(Some(existing)).unwrap();
        assert_eq!(got["hooks"]["Stop"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn merging_twice_is_byte_identical() {
        let once = merge(Target::Claude, Some(r#"{"a": 1}"#), CMD).unwrap();
        let twice = merge(Target::Claude, Some(&once), CMD).unwrap();
        assert_eq!(once, twice);
        assert!(once.ends_with("}\n"));
    }

    #[test]
    fn malformed_or_oddly_shaped_files_are_refused() {
        for (bad, needle) in [
            ("{not json", "not valid JSON"),
            ("[1]", "top level"),
            (r#"{"hooks": []}"#, "`hooks` is not an object"),
            (r#"{"hooks": {"Stop": {}}}"#, "`hooks.Stop` is not an array"),
        ] {
            let err = run(Some(bad)).unwrap_err();
            assert!(err.contains(needle), "{bad}: {err}");
        }
    }
}
