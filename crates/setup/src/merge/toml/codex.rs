//! Codex CLI `config.toml`: nested arrays of tables, `[[hooks.<Event>]]` with
//! `[[hooks.<Event>.hooks]]` inside.

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

use crate::target::{HookSpec, Target, mentions_hookah};

pub fn merge(target: Target, existing: Option<&str>, command: &str) -> Result<String, String> {
    let mut doc: DocumentMut = existing
        .unwrap_or_default()
        .parse()
        .map_err(|e| format!("not valid TOML ({e})"))?;
    let hooks = doc.entry("hooks").or_insert_with(|| {
        let mut table = Table::new();
        table.set_implicit(true);
        Item::Table(table)
    });
    let hooks = hooks
        .as_table_mut()
        .ok_or("`hooks` is not a table (inline tables are not supported)")?;
    for spec in target.hooks() {
        let groups = hooks
            .entry(spec.event)
            .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()))
            .as_array_of_tables_mut()
            .ok_or_else(|| {
                format!(
                    "`hooks.{}` is not written as `[[hooks.{}]]` tables",
                    spec.event, spec.event
                )
            })?;
        upsert(groups, spec, target.name(), command);
    }
    Ok(doc.to_string())
}

/// Updates every hookah hook of the event in place, or appends a new group when there is none.
fn upsert(groups: &mut ArrayOfTables, spec: &HookSpec, name: &str, command: &str) {
    let mut found = false;
    for group in groups.iter_mut() {
        let mut touched = false;
        let mut count = 0;
        if let Some(hooks) = group
            .get_mut("hooks")
            .and_then(Item::as_array_of_tables_mut)
        {
            count = hooks.len();
            for hook in hooks.iter_mut() {
                let ours = hook
                    .get("command")
                    .and_then(Item::as_str)
                    .is_some_and(|c| mentions_hookah(c, name));
                if ours {
                    set_str(hook, "command", command);
                    set_int(hook, "timeout", spec.timeout);
                    touched = true;
                }
            }
        }
        if touched
            && count == 1
            && let Some(matcher) = spec.matcher
        {
            set_str(group, "matcher", matcher);
        }
        found |= touched;
    }
    if !found {
        let mut group = Table::new();
        if let Some(matcher) = spec.matcher {
            group["matcher"] = value(matcher);
        }
        let mut hook = Table::new();
        hook["type"] = value("command");
        hook["command"] = value(command);
        hook["timeout"] = value(spec.timeout);
        let mut hooks = ArrayOfTables::new();
        hooks.push(hook);
        group["hooks"] = Item::ArrayOfTables(hooks);
        groups.push(group);
    }
}

/// Sets a string key, keeping the existing formatting when the value is already right.
pub(super) fn set_str(table: &mut Table, key: &str, new: &str) {
    if table.get(key).and_then(Item::as_str) != Some(new) {
        table[key] = value(new);
    }
}

/// Sets an integer key, keeping the existing formatting when the value is already right.
pub(super) fn set_int(table: &mut Table, key: &str, new: i64) {
    if table.get(key).and_then(Item::as_integer) != Some(new) {
        table[key] = value(new);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const CMD: &str = "/bin/hookah hook codex";

    fn run(existing: Option<&str>) -> Result<String, String> {
        merge(Target::Codex, existing, CMD)
    }

    #[test]
    fn empty_input_yields_the_documented_codex_config() {
        let expected = "\
[[hooks.SessionStart]]
matcher = \"startup|resume\"

[[hooks.SessionStart.hooks]]
type = \"command\"
command = \"/bin/hookah hook codex\"
timeout = 30

[[hooks.PostToolUse]]
matcher = \"^apply_patch$\"

[[hooks.PostToolUse.hooks]]
type = \"command\"
command = \"/bin/hookah hook codex\"
timeout = 30

[[hooks.Stop]]

[[hooks.Stop.hooks]]
type = \"command\"
command = \"/bin/hookah hook codex\"
timeout = 120
";
        assert_eq!(run(None).unwrap(), expected);
    }

    #[test]
    fn comments_order_and_unrelated_tables_survive() {
        let existing = "# my codex config\nmodel = \"gpt\"   # inline\n\n[profiles.fast]\neffort = \"low\"\n\n[[hooks.Stop]]\n\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"notify.sh\"\n";
        let text = run(Some(existing)).unwrap();
        assert!(text.starts_with(existing));
        let doc: DocumentMut = text.parse().unwrap();
        assert_eq!(doc["hooks"]["Stop"].as_array_of_tables().unwrap().len(), 2);
        assert_eq!(doc["model"].as_str(), Some("gpt"));
    }

    #[test]
    fn an_existing_hookah_entry_is_updated_in_place() {
        let existing = "[[hooks.Stop]]\n# ours\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = '$HOME/.local/bin/hookah hook codex'\ntimeout = 5\n";
        let text = run(Some(existing)).unwrap();
        let doc: DocumentMut = text.parse().unwrap();
        let stop = doc["hooks"]["Stop"].as_array_of_tables().unwrap();
        assert_eq!(stop.len(), 1);
        let hook = &stop.get(0).unwrap()["hooks"].as_array_of_tables().unwrap();
        assert_eq!(hook.get(0).unwrap()["command"].as_str(), Some(CMD));
        assert_eq!(hook.get(0).unwrap()["timeout"].as_integer(), Some(120));
        assert!(text.contains("# ours"));
    }

    #[test]
    fn an_up_to_date_entry_keeps_its_formatting() {
        let existing = "[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = '/bin/hookah hook codex'\ntimeout = 120\n";
        let text = run(Some(existing)).unwrap();
        assert!(text.contains("command = '/bin/hookah hook codex'"));
    }

    #[test]
    fn a_lone_hookah_hook_gets_the_documented_matcher_back() {
        let existing = "[[hooks.PostToolUse]]\nmatcher = \"x\"\n[[hooks.PostToolUse.hooks]]\ncommand = \"hookah hook codex\"\n";
        let text = run(Some(existing)).unwrap();
        let doc: DocumentMut = text.parse().unwrap();
        let post = doc["hooks"]["PostToolUse"].as_array_of_tables().unwrap();
        assert_eq!(post.len(), 1);
        assert_eq!(
            post.get(0).unwrap()["matcher"].as_str(),
            Some("^apply_patch$")
        );
    }

    #[test]
    fn a_shared_group_keeps_its_matcher() {
        let existing = "[[hooks.PostToolUse]]\nmatcher = \"x\"\n[[hooks.PostToolUse.hooks]]\ncommand = \"a.sh\"\n[[hooks.PostToolUse.hooks]]\ncommand = \"hookah hook codex\"\n";
        let text = run(Some(existing)).unwrap();
        let doc: DocumentMut = text.parse().unwrap();
        let post = doc["hooks"]["PostToolUse"].as_array_of_tables().unwrap();
        assert_eq!(post.get(0).unwrap()["matcher"].as_str(), Some("x"));
    }

    #[test]
    fn merging_twice_is_byte_identical() {
        let once = run(Some("model = \"x\"\n")).unwrap();
        assert_eq!(run(Some(&once)).unwrap(), once);
    }

    #[test]
    fn malformed_or_oddly_shaped_files_are_refused() {
        for (bad, needle) in [
            ("[[hooks", "not valid TOML"),
            ("hooks = 1\n", "`hooks` is not a table"),
            (
                "[hooks]\nStop = []\n",
                "`hooks.Stop` is not written as `[[hooks.Stop]]` tables",
            ),
        ] {
            let err = run(Some(bad)).unwrap_err();
            assert!(err.contains(needle), "{bad}: {err}");
        }
    }
}
