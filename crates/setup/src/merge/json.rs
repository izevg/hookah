//! Claude Code and Qoder: `{"hooks": {"<Event>": [{"matcher"?, "hooks": [{"type","command","timeout"}]}]}}`.

use serde_json::{Map, Value, json};

use super::Unmerged;
use crate::target::{HookSpec, Target, mentions_hookah};

pub fn merge(target: Target, existing: Option<&str>, command: &str) -> Result<String, String> {
    let mut top = parse_object(existing)?;
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
        upsert(
            groups,
            spec,
            |hook| is_ours(hook, target.name()),
            |hook| {
                hook.insert("type".into(), json!("command"));
                hook.insert("command".into(), json!(command));
                hook.insert("timeout".into(), json!(spec.timeout));
            },
        );
    }
    encode(&top)
}

pub(super) fn parse_object(text: Option<&str>) -> Result<Map<String, Value>, String> {
    match text
        .map(|text| serde_json::from_str(text).map_err(|e| format!("not valid JSON ({e})")))
        .transpose()?
    {
        Some(Value::Object(top)) => Ok(top),
        Some(_) => Err("the top level is not a JSON object".into()),
        None => Ok(Map::new()),
    }
}

pub(super) fn encode(top: &Map<String, Value>) -> Result<String, String> {
    let mut text = serde_json::to_string_pretty(top).map_err(|e| e.to_string())?;
    text.push('\n');
    Ok(text)
}

pub fn unmerge(target: Target, text: &str) -> Result<Unmerged, String> {
    unmerge_with(
        text,
        |list| strip_groups(list, |hook| is_ours(hook, target.name())),
        |top| top.is_empty(),
    )
}

/// Removes hookah's entries from every event list of `hooks` with `strip` (which says whether it
/// removed any), and drops the event lists and the `hooks` object that this left empty.
/// `is_empty` says whether what remains of the top-level object is only setup's own addition.
pub(super) fn unmerge_with(
    text: &str,
    strip: impl Fn(&mut Vec<Value>) -> bool,
    is_empty: impl Fn(&Map<String, Value>) -> bool,
) -> Result<Unmerged, String> {
    let mut top = parse_object(Some(text))?;
    let Some(hooks) = top.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(Unmerged::Unchanged);
    };
    if !strip_events(hooks, strip) {
        return Ok(Unmerged::Unchanged);
    }
    if hooks.is_empty() {
        top.shift_remove("hooks");
    }
    Ok(Unmerged::Removed {
        text: encode(&top)?,
        empty: is_empty(&top),
    })
}

/// Drops event lists that are emptied by removing owned hooks.
pub(super) fn strip_events(
    events: &mut Map<String, Value>,
    strip: impl Fn(&mut Vec<Value>) -> bool,
) -> bool {
    let mut changed = false;
    events.retain(|_, value| {
        if let Some(list) = value.as_array_mut()
            && strip(list)
        {
            changed = true;
            return !list.is_empty();
        }
        true
    });
    changed
}

/// Drops hookah's hooks from each matcher group, and the groups that this left without any.
pub(super) fn strip_groups(groups: &mut Vec<Value>, is_ours: impl Fn(&Value) -> bool) -> bool {
    let mut removed = false;
    let mut emptied = Vec::new();
    for group in groups.iter_mut() {
        let hooks = group.get_mut("hooks").and_then(Value::as_array_mut);
        let Some(hooks) = hooks else {
            emptied.push(false);
            continue;
        };
        let before = hooks.len();
        hooks.retain(|hook| !is_ours(hook));
        let touched = hooks.len() != before;
        removed |= touched;
        emptied.push(touched && hooks.is_empty());
    }
    let mut emptied = emptied.into_iter();
    groups.retain(|_| emptied.next() != Some(true));
    removed
}

/// True if a hook object runs `hookah hook <name>`.
pub(super) fn is_ours(hook: &Value, name: &str) -> bool {
    hook.get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| mentions_hookah(c, name))
}

/// Updates every hookah hook of the event in place, or appends a new group when there is none.
pub(super) fn upsert(
    groups: &mut Vec<Value>,
    spec: &HookSpec,
    is_ours: impl Fn(&Value) -> bool,
    configure: impl Fn(&mut Map<String, Value>),
) {
    let mut found = false;
    for group in groups.iter_mut().filter_map(Value::as_object_mut) {
        let alone = group
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|h| h.len() == 1);
        let mut touched = false;
        if let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) {
            for hook in hooks.iter_mut().filter(|hook| is_ours(hook)) {
                if let Some(hook) = hook.as_object_mut() {
                    configure(hook);
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
        let mut hook = Map::new();
        configure(&mut hook);
        group.insert("hooks".into(), json!([hook]));
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

    fn removed(target: Target, text: &str) -> (String, bool) {
        match unmerge(target, text).unwrap() {
            Unmerged::Removed { text, empty } => (text, empty),
            Unmerged::Unchanged => panic!("nothing removed from {text}"),
        }
    }

    #[test]
    fn unmerge_is_the_exact_inverse_of_merge_on_a_pretty_file() {
        let original = "{\n  \"zeta\": 1,\n  \"hooks\": {\n    \"PostToolUse\": [\n      {\n        \"matcher\": \"Bash\",\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"lint.sh\"\n          }\n        ]\n      }\n    ],\n    \"Notification\": []\n  },\n  \"alpha\": [\n    1,\n    2\n  ]\n}\n";
        for target in [Target::Claude, Target::Qoder] {
            let merged = merge(
                target,
                Some(original),
                &format!("/b/hookah hook {}", target.name()),
            )
            .unwrap();
            assert_ne!(merged, original);
            assert_eq!(
                removed(target, &merged),
                (original.to_owned(), false),
                "{target:?}"
            );
        }
    }

    #[test]
    fn unmerge_of_a_file_setup_created_is_empty() {
        let merged = merge(Target::Claude, None, CMD).unwrap();
        assert_eq!(removed(Target::Claude, &merged), ("{}\n".to_owned(), true));
    }

    #[test]
    fn unmerge_removes_only_hookahs_hook_from_a_shared_group() {
        let text = r#"{"hooks": {"Stop": [{"hooks": [
            {"type": "command", "command": "/x/hookah hook claude"},
            {"type": "command", "command": "notify.sh"}
        ]}, {"hooks": []}]}}"#;
        let (out, empty) = removed(Target::Claude, text);
        let got: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            got,
            json!({"hooks": {"Stop": [
                {"hooks": [{"type": "command", "command": "notify.sh"}]},
                {"hooks": []}
            ]}})
        );
        assert!(!empty);
    }

    #[test]
    fn unmerge_matches_every_spelling_and_only_its_own_harness() {
        let text = r#"{"hooks": {"Stop": [
            {"hooks": [{"command": "mise exec -- hookah hook claude"}]},
            {"hooks": [{"command": "\"C:/a b/hookah.exe\" hook claude"}]},
            {"hooks": [{"command": "hookah hook codex"}]},
            {"hooks": [{"timeout": 3}]},
            "stray"
        ]}}"#;
        let (out, _) = removed(Target::Claude, text);
        let got: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            got["hooks"]["Stop"],
            json!([{"hooks": [{"command": "hookah hook codex"}]}, {"hooks": [{"timeout": 3}]}, "stray"])
        );
    }

    #[test]
    fn unmerge_reports_nothing_to_remove() {
        for text in [
            "{}",
            "[]",
            r#"{"hooks": []}"#,
            r#"{"hooks": {"Stop": 3, "X": [{"hooks": [{"command": "a.sh"}]}]}}"#,
            r#"{"hooks": {"Stop": [{"matcher": "x"}]}}"#,
        ] {
            let got = unmerge(Target::Claude, text);
            if text == "[]" {
                assert!(got.unwrap_err().contains("not a JSON object"));
            } else {
                assert_eq!(got.unwrap(), Unmerged::Unchanged, "{text}");
            }
        }
        assert!(
            unmerge(Target::Claude, "{ nope")
                .unwrap_err()
                .contains("not valid JSON")
        );
    }

    #[test]
    fn unmerge_keeps_an_event_list_it_did_not_empty_and_a_hooks_object_with_other_events() {
        let text =
            r#"{"hooks": {"Empty": [], "Stop": [{"hooks": [{"command": "hookah hook claude"}]}]}}"#;
        let (out, empty) = removed(Target::Claude, text);
        let got: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(got, json!({"hooks": {"Empty": []}}));
        assert!(!empty);
    }
}
