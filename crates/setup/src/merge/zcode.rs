//! ZCode native configuration nests event groups under `hooks.events`.

use serde_json::{Map, Value, json};

use super::{Unmerged, json as config};
use crate::target::{Program, Target, mentions_hookah};

const ARGUMENTS: [&str; 2] = ["hook", "zcode"];
const CUSTOM_RUNNER_STATUS: &str = "Running Hookah checks";

pub(super) fn merge(existing: Option<&str>, program: Program<'_>) -> Result<String, String> {
    let mut top = config::parse_object(existing)?;
    let hooks = object_entry(&mut top, "hooks", "hooks")?;
    let enabled = hooks.entry("enabled").or_insert_with(|| json!(true));
    if !enabled.is_boolean() {
        return Err("`hooks.enabled` is not a boolean".into());
    }
    let events = object_entry(hooks, "events", "hooks.events")?;
    for spec in Target::Zcode.hooks() {
        let groups = events
            .entry(spec.event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| format!("`hooks.events.{}` is not an array", spec.event))?;
        config::upsert(groups, spec, is_ours, |hook| {
            configure(hook, program, spec.timeout * 1000);
        });
    }
    config::encode(&top)
}

pub(crate) fn unmerge(text: &str, original: Option<&str>) -> Result<Unmerged, String> {
    let mut top = config::parse_object(Some(text))?;
    let Some(hooks) = top.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(Unmerged::Unchanged);
    };
    let Some(events) = hooks.get_mut("events").and_then(Value::as_object_mut) else {
        return Ok(Unmerged::Unchanged);
    };
    if !config::strip_events(events, |groups| config::strip_groups(groups, is_ours)) {
        return Ok(Unmerged::Unchanged);
    }
    if events.is_empty() {
        hooks.shift_remove("events");
    }
    let only_enable_flag = hooks.len() == 1 && hooks.get("enabled") == Some(&Value::Bool(true));
    if only_enable_flag && let Some(original) = original {
        // The backup can show setup added the flag, without restoring any of its old values.
        if config::parse_object(Some(original)).is_ok_and(|original| {
            original
                .get("hooks")
                .and_then(|hooks| hooks.get("enabled"))
                .is_none()
        }) {
            hooks.shift_remove("enabled");
        }
    }
    if hooks.is_empty() {
        top.shift_remove("hooks");
    }
    Ok(Unmerged::Removed {
        text: config::encode(&top)?,
        empty: top.is_empty() || (top.len() == 1 && only_enable_flag),
    })
}

/// True when a native event group contains a Hookah entry, even if disabled.
pub(crate) fn configured(text: &str) -> bool {
    config::parse_object(Some(text))
        .ok()
        .and_then(|top| {
            top.get("hooks")?.get("events")?.as_object().map(|events| {
                events
                    .values()
                    .filter_map(Value::as_array)
                    .flatten()
                    .filter_map(|group| group.get("hooks")?.as_array())
                    .flatten()
                    .any(is_ours)
            })
        })
        .unwrap_or(false)
}

fn object_entry<'a>(
    root: &'a mut Map<String, Value>,
    name: &str,
    path: &str,
) -> Result<&'a mut Map<String, Value>, String> {
    root.entry(name)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("`{path}` is not an object"))
}

fn configure(hook: &mut Map<String, Value>, program: Program<'_>, timeout_ms: i64) {
    let (runner, command) = match program {
        Program::Executable(executable) => ("process", executable.to_owned()),
        Program::Shell(prefix) => ("command", format!("{prefix} hook zcode")),
    };
    // The runner's visible status lets setup recognize a wrapper with a different name.
    let named_hookah = if matches!(program, Program::Executable(_)) {
        hookah_binary(&command)
    } else {
        mentions_hookah(&command, Target::Zcode.name())
    };
    if !named_hookah {
        hook.insert("statusMessage".into(), json!(CUSTOM_RUNNER_STATUS));
    } else if hook.get("statusMessage").and_then(Value::as_str) == Some(CUSTOM_RUNNER_STATUS) {
        hook.shift_remove("statusMessage");
    }
    hook.insert("type".into(), json!(runner));
    hook.insert("command".into(), json!(command));
    hook.insert("timeoutMs".into(), json!(timeout_ms));
    if matches!(program, Program::Executable(_)) {
        hook.insert("args".into(), json!(ARGUMENTS));
        for field in ["async", "shell", "timeout"] {
            hook.shift_remove(field);
        }
    } else {
        hook.shift_remove("args");
    }
}

fn is_ours(hook: &Value) -> bool {
    let Some(command) = hook.get("command").and_then(Value::as_str) else {
        return false;
    };
    let marked = hook.get("statusMessage").and_then(Value::as_str) == Some(CUSTOM_RUNNER_STATUS);
    match hook.get("type").and_then(Value::as_str) {
        Some("process") => {
            (marked || hookah_binary(command))
                && hook
                    .get("args")
                    .and_then(Value::as_array)
                    .is_some_and(|args| args.iter().map(Value::as_str).eq(ARGUMENTS.map(Some)))
        }
        Some("command") => {
            mentions_hookah(command, Target::Zcode.name())
                || (marked && command.ends_with(" hook zcode"))
        }
        _ => false,
    }
}

fn hookah_binary(command: &str) -> bool {
    matches!(
        command.rsplit(['/', '\\']).next(),
        Some("hookah" | "hookah.exe")
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const ENABLED: &str = r#"{"hooks":{"enabled":true}}"#;

    fn merged(existing: Option<&str>, program: Program<'_>) -> String {
        merge(existing, program).unwrap()
    }

    fn stop_hook(config: &str) -> Value {
        serde_json::from_str::<Value>(config).unwrap()["hooks"]["events"]["Stop"][0]["hooks"][0]
            .clone()
    }

    #[test]
    fn process_hooks_keep_the_executable_and_arguments_separate() {
        let exe = r"C:\Program Files\Hookah\hookah.exe";
        let config = merged(None, Program::Executable(exe));
        assert_eq!(
            stop_hook(&config),
            json!({"type":"process","command":exe,"timeoutMs":120000,"args":["hook","zcode"]})
        );
        assert!(configured(&config));
        assert_eq!(merged(Some(&config), Program::Executable(exe)), config);
        assert!(matches!(
            unmerge(&config, None).unwrap(),
            Unmerged::Removed { empty: true, .. }
        ));
    }

    #[test]
    fn setup_tracks_custom_wrappers_and_uninstall_keeps_the_explicit_enable_flag() {
        let program = Program::Shell("my-wrapper");
        let config = merged(Some(ENABLED), program);
        assert_eq!(stop_hook(&config)["command"], "my-wrapper hook zcode");
        assert!(configured(&config));
        assert_eq!(merged(Some(&config), program), config);
        let Unmerged::Removed { text, .. } = unmerge(&config, Some(ENABLED)).unwrap() else {
            panic!("expected the hook entries to be removed");
        };
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            serde_json::from_str::<Value>(ENABLED).unwrap()
        );
        assert!(!configured(&text));
    }

    #[test]
    fn uninstall_preserves_disabled_hooks_and_unrelated_runners() {
        let original = r#"{"hooks":{"enabled":false,"maxOutputBytes":2222,"events":{"Stop":[
            {"hooks":[{"type":"process","command":"other","args":["hook","zcode"]}]},
            {"hooks":[{"type":"process","command":"hookah","args":["check","foo.rs"]}]},
            {"hooks":[{"type":"command","command":"my-wrapper hook zcode"}]}
        ]}}}"#;
        assert!(!configured(original));
        assert_eq!(unmerge(original, None).unwrap(), Unmerged::Unchanged);

        let config = merged(Some(original), Program::Executable("hookah"));
        assert_eq!(
            serde_json::from_str::<Value>(&config).unwrap()["hooks"]["enabled"],
            false
        );
        let Unmerged::Removed { text, empty } = unmerge(&config, Some(original)).unwrap() else {
            panic!("expected the hook entries to be removed");
        };
        assert!(!empty);
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            serde_json::from_str::<Value>(original).unwrap()
        );
    }

    #[test]
    fn uninstall_drops_an_enable_flag_added_to_a_preexisting_empty_config() {
        let config = merged(Some("{}"), Program::Executable("hookah"));
        assert_eq!(
            unmerge(&config, Some("{}")).unwrap(),
            Unmerged::Removed {
                text: "{}\n".into(),
                empty: true
            }
        );
    }

    #[test]
    fn uninstall_keeps_hooks_disabled_after_setup() {
        let config = merged(None, Program::Executable("hookah"));
        let mut edited: Value = serde_json::from_str(&config).unwrap();
        edited["hooks"]["enabled"] = json!(false);
        let Unmerged::Removed { text, empty } = unmerge(&edited.to_string(), None).unwrap() else {
            panic!("expected the hook entries to be removed");
        };
        assert!(!empty);
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            json!({"hooks":{"enabled":false}})
        );
    }

    #[test]
    fn setup_replaces_its_runner_status_when_switching_to_the_standard_binary() {
        let config = merged(None, Program::Shell("my-wrapper"));
        let standard = merged(Some(&config), Program::Executable("hookah"));
        assert_eq!(
            stop_hook(&standard),
            stop_hook(&merged(None, Program::Executable("hookah")))
        );
    }

    #[test]
    fn setup_refuses_wrong_json_types_in_its_config() {
        for input in [
            r#"{"hooks":{"enabled":"maybe"}}"#,
            r#"{"hooks":{"events":[]}}"#,
            r#"{"hooks":{"events":{"Stop":{}}}}"#,
        ] {
            assert!(
                merge(Some(input), Program::Executable("hookah")).is_err(),
                "{input}"
            );
        }
    }
}
