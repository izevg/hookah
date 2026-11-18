//! Kimi Code `config.toml`: flat `[[hooks]]` entries with `event`, `matcher`, `command`,
//! `timeout`. Kimi rejects any other field, so none is added.

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

use super::codex::{set_int, set_str};
use crate::target::{Target, mentions_hookah};

pub fn merge(target: Target, existing: Option<&str>, command: &str) -> Result<String, String> {
    let mut doc: DocumentMut = existing
        .unwrap_or_default()
        .parse()
        .map_err(|e| format!("not valid TOML ({e})"))?;
    let rules = doc
        .entry("hooks")
        .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .ok_or("`hooks` is not written as `[[hooks]]` tables")?;
    for spec in target.hooks() {
        let mut found = false;
        for rule in rules.iter_mut() {
            let ours = rule.get("event").and_then(Item::as_str) == Some(spec.event)
                && rule
                    .get("command")
                    .and_then(Item::as_str)
                    .is_some_and(|c| mentions_hookah(c, target.name()));
            if ours {
                set_str(rule, "command", command);
                set_int(rule, "timeout", spec.timeout);
                if let Some(matcher) = spec.matcher {
                    set_str(rule, "matcher", matcher);
                }
                found = true;
            }
        }
        if !found {
            let mut rule = Table::new();
            rule["event"] = value(spec.event);
            if let Some(matcher) = spec.matcher {
                rule["matcher"] = value(matcher);
            }
            rule["command"] = value(command);
            rule["timeout"] = value(spec.timeout);
            rules.push(rule);
        }
    }
    Ok(doc.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const CMD: &str = "/bin/hookah hook kimi";

    fn run(existing: Option<&str>) -> Result<String, String> {
        merge(Target::Kimi, existing, CMD)
    }

    #[test]
    fn empty_input_yields_the_documented_kimi_config() {
        let expected = "\
[[hooks]]
event = \"SessionStart\"
matcher = \"startup|resume\"
command = \"/bin/hookah hook kimi\"
timeout = 30

[[hooks]]
event = \"PostToolUse\"
matcher = \"Write|Edit\"
command = \"/bin/hookah hook kimi\"
timeout = 30
";
        assert_eq!(run(None).unwrap(), expected);
    }

    #[test]
    fn existing_settings_and_comments_survive() {
        let existing = "# kimi\ndefault_model = \"k2\"\n\n[[hooks]]\nevent = \"Stop\"\ncommand = \"notify.sh\"\n";
        let text = run(Some(existing)).unwrap();
        assert!(text.starts_with(existing));
        let doc: DocumentMut = text.parse().unwrap();
        assert_eq!(doc["hooks"].as_array_of_tables().unwrap().len(), 3);
    }

    #[test]
    fn an_existing_hookah_rule_is_updated_in_place() {
        let existing = "[[hooks]]\nevent = \"PostToolUse\"\nmatcher = \"x\"\ncommand = \"hookah hook kimi\"\ntimeout = 1\n";
        let text = run(Some(existing)).unwrap();
        let doc: DocumentMut = text.parse().unwrap();
        let rules = doc["hooks"].as_array_of_tables().unwrap();
        assert_eq!(rules.len(), 2);
        let first = rules.get(0).unwrap();
        assert_eq!(first["event"].as_str(), Some("PostToolUse"));
        assert_eq!(first["matcher"].as_str(), Some("Write|Edit"));
        assert_eq!(first["command"].as_str(), Some(CMD));
        assert_eq!(first["timeout"].as_integer(), Some(30));
        assert_eq!(
            rules.get(1).unwrap()["event"].as_str(),
            Some("SessionStart")
        );
    }

    #[test]
    fn a_hookah_rule_for_another_event_is_not_reused() {
        let existing = "[[hooks]]\nevent = \"Stop\"\ncommand = \"hookah hook kimi\"\n";
        let text = run(Some(existing)).unwrap();
        let doc: DocumentMut = text.parse().unwrap();
        assert_eq!(doc["hooks"].as_array_of_tables().unwrap().len(), 3);
    }

    #[test]
    fn merging_twice_is_byte_identical() {
        let once = run(Some("a = 1\n")).unwrap();
        assert_eq!(run(Some(&once)).unwrap(), once);
    }

    #[test]
    fn malformed_or_oddly_shaped_files_are_refused() {
        for (bad, needle) in [
            ("[[hooks", "not valid TOML"),
            ("hooks = 3\n", "`hooks` is not written as `[[hooks]]`"),
        ] {
            let err = run(Some(bad)).unwrap_err();
            assert!(err.contains(needle), "{bad}: {err}");
        }
    }
}
