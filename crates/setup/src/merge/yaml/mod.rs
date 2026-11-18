//! Hermes Agent `config.yaml`: adds `hookah` to `plugins.enabled`, the allow-list that decides
//! which plugins load.
//!
//! No YAML crate: a YAML round trip would drop the comments of a file the user edits by hand.
//! This is a conservative line editor for the one shape Hermes writes itself (a block mapping
//! `plugins:` with a block list `enabled:`). It appends the missing pieces, never rewrites a
//! line, and refuses (so the file stays untouched) when `plugins` or `enabled` is written in a
//! form it cannot edit safely, such as a flow list `enabled: [a, b]`.

const PLUGIN: &str = "hookah";

/// `existing` with `hookah` enabled as a Hermes plugin.
pub fn merge(existing: Option<&str>) -> Result<String, String> {
    let text = existing.unwrap_or_default();
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let Some(plugins) = lines.iter().position(|l| key_of(l, 0) == Some("plugins")) else {
        let sep = if lines.is_empty() { "" } else { eol };
        let tail = ["plugins:", "  enabled:", "    - hookah"].join(eol);
        return Ok(format!("{}{sep}{tail}{eol}", lines.join(eol)));
    };
    if !value_of(&lines[plugins]).is_empty() {
        return Err(manual("`plugins` is not a block mapping"));
    }
    let end = block_end(&lines, plugins);
    let body = || (plugins..end).skip(1);
    let indent = body().find_map(|i| key_indent(&lines[i])).unwrap_or(2);
    let enabled = body().find(|&i| key_of(&lines[i], indent) == Some("enabled"));
    let disabled = body().find(|&i| key_of(&lines[i], indent) == Some("disabled"));
    if let Some(i) = disabled {
        let (items, _) = list_items(&lines, i, end)?;
        if items.iter().any(|item| item == PLUGIN) {
            return Err(
                "`hookah` is listed in plugins.disabled, which wins over plugins.enabled; \
                        remove it there first"
                    .to_owned(),
            );
        }
    }
    match enabled {
        None => {
            let pad = " ".repeat(indent);
            lines.insert(plugins + 1, format!("{pad}enabled:"));
            lines.insert(plugins + 2, format!("{pad}  - hookah"));
        }
        Some(i) => {
            let (items, last) = list_items(&lines, i, end)?;
            if items.iter().any(|item| item == PLUGIN) {
                return Ok(text.to_owned());
            }
            let pad = last.map_or(indent + 2, |l| dash_indent(&lines[l]));
            let after = last.unwrap_or(i);
            lines.insert(after + 1, format!("{}- hookah", " ".repeat(pad)));
        }
    }
    Ok(format!("{}{eol}", lines.join(eol)))
}

/// True if `existing` already enables `hookah`.
pub fn enabled(existing: &str) -> bool {
    merge(Some(existing)).is_ok_and(|merged| merged == existing)
}

fn manual(reason: &str) -> String {
    format!("{reason}; add `hookah` to plugins.enabled by hand, then run setup again")
}

/// The key of a mapping line indented by exactly `indent` spaces, `None` for anything else.
fn key_of(line: &str, indent: usize) -> Option<&str> {
    let rest = line.strip_prefix(&" ".repeat(indent))?;
    if rest.starts_with([' ', '\t', '#', '-']) {
        return None;
    }
    rest.split_once(':').map(|(key, _)| key.trim_end())
}

/// The indentation of a mapping line, `None` for blanks, comments, and list items.
fn key_indent(line: &str) -> Option<usize> {
    let rest = line.trim_start_matches(' ');
    (!rest.is_empty() && !rest.starts_with(['#', '-']) && rest.contains(':'))
        .then(|| line.len() - rest.len())
}

/// What follows the colon of a mapping line, without a trailing comment.
fn value_of(line: &str) -> &str {
    let after = line.split_once(':').map_or("", |(_, v)| v);
    after.split(" #").next().unwrap_or("").trim()
}

/// The first line after `start` that is not part of its block: a non-blank, non-comment line
/// with no indentation.
fn block_end(lines: &[String], start: usize) -> usize {
    (start + 1..lines.len())
        .find(|&i| {
            let l = &lines[i];
            !l.trim().is_empty() && !l.starts_with([' ', '\t', '#'])
        })
        .unwrap_or(lines.len())
}

fn dash_indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// The scalar items of the block list under the key at `key`, and the index of its last item.
/// A flow list (`[a, b]`) is refused.
fn list_items(
    lines: &[String],
    key: usize,
    end: usize,
) -> Result<(Vec<String>, Option<usize>), String> {
    let inline = value_of(&lines[key]);
    if !inline.is_empty() {
        return Err(manual("a plugins list is written inline (`[...]`)"));
    }
    let key_pad = dash_indent(&lines[key]);
    let mut items = Vec::new();
    let mut last = None;
    for (i, line) in lines.iter().enumerate().take(end).skip(key + 1) {
        let rest = line.trim_start_matches(' ');
        if rest.is_empty() || rest.starts_with('#') {
            continue;
        }
        let Some(item) = rest.strip_prefix('-') else {
            break;
        };
        if dash_indent(line) < key_pad {
            break;
        }
        let item = item.split(" #").next().unwrap_or("").trim();
        items.push(item.trim_matches(['"', '\'']).to_owned());
        last = Some(i);
    }
    Ok((items, last))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn merged(text: &str) -> String {
        merge(Some(text)).unwrap()
    }

    #[test]
    fn a_missing_file_gets_the_whole_block() {
        assert_eq!(merge(None).unwrap(), "plugins:\n  enabled:\n    - hookah\n");
        assert_eq!(merged(""), merge(None).unwrap());
    }

    #[test]
    fn a_file_without_plugins_gets_the_block_appended_and_keeps_everything_else() {
        let text = "# my config\nmodel: x\nterminal:\n  cwd: .\n";
        assert_eq!(
            merged(text),
            format!("{text}plugins:\n  enabled:\n    - hookah\n")
        );
        // No trailing newline in the original.
        assert_eq!(
            merged("model: x"),
            "model: x\nplugins:\n  enabled:\n    - hookah\n"
        );
    }

    #[test]
    fn an_existing_list_gets_hookah_after_its_last_item_with_the_same_indent() {
        let text = "plugins:\n  enabled:\n    - kanban   # keep\n    - \"mem\"\n  disabled:\n    - x\nmodel: y\n";
        assert_eq!(
            merged(text),
            "plugins:\n  enabled:\n    - kanban   # keep\n    - \"mem\"\n    - hookah\n  disabled:\n    - x\nmodel: y\n"
        );
    }

    #[test]
    fn list_items_at_the_key_indent_are_supported() {
        let text = "plugins:\n  enabled:\n  - a\n  - b\nmodel: y\n";
        assert_eq!(
            merged(text),
            "plugins:\n  enabled:\n  - a\n  - b\n  - hookah\nmodel: y\n"
        );
    }

    #[test]
    fn an_empty_enabled_key_gets_its_first_item() {
        assert_eq!(
            merged("plugins:\n  enabled:\nmodel: y\n"),
            "plugins:\n  enabled:\n    - hookah\nmodel: y\n"
        );
    }

    #[test]
    fn a_plugins_block_without_enabled_gets_the_key_at_the_block_indent() {
        assert_eq!(
            merged("plugins:\n    hook_callback_timeout: 60\nmodel: y\n"),
            "plugins:\n    enabled:\n      - hookah\n    hook_callback_timeout: 60\nmodel: y\n"
        );
        assert_eq!(merged("plugins:\n"), "plugins:\n  enabled:\n    - hookah\n");
    }

    #[test]
    fn comments_and_blank_lines_do_not_set_the_block_indent() {
        assert_eq!(
            merged("plugins:\n      # deep: note\n\n  enabled:\n    - a\n"),
            "plugins:\n      # deep: note\n\n  enabled:\n    - a\n    - hookah\n"
        );
        assert_eq!(
            merged("plugins:\n      # deep note\n  x: 1\n"),
            "plugins:\n  enabled:\n    - hookah\n      # deep note\n  x: 1\n"
        );
    }

    #[test]
    fn an_empty_list_key_indents_its_first_item_two_columns_past_the_key() {
        assert_eq!(
            merged("plugins:\n    enabled:\n    other: 1\n"),
            "plugins:\n    enabled:\n      - hookah\n    other: 1\n"
        );
    }

    #[test]
    fn an_already_enabled_plugin_changes_nothing_in_any_spelling() {
        for text in [
            "plugins:\n  enabled:\n    - hookah\n",
            "plugins:\n  enabled:\n    - 'hookah'  # ours\n    - b\n",
            "plugins:\n  enabled:\n  - \"hookah\"\nx: 1",
        ] {
            assert_eq!(merged(text), text);
            assert!(enabled(text), "{text}");
        }
        assert!(!enabled("plugins:\n  enabled:\n    - b\n"));
        assert!(!enabled("model: x\n"));
        assert!(!enabled("plugins: {}\n"));
    }

    #[test]
    fn a_second_run_changes_nothing() {
        for text in [
            "",
            "model: x\n",
            "plugins:\n  enabled:\n    - a\n",
            "plugins:\n  x: 1\n",
        ] {
            let once = merged(text);
            assert_eq!(merged(&once), once, "{text:?}");
        }
    }

    #[test]
    fn windows_line_endings_are_kept() {
        assert_eq!(
            merged("model: x\r\nplugins:\r\n  enabled:\r\n    - a\r\n"),
            "model: x\r\nplugins:\r\n  enabled:\r\n    - a\r\n    - hookah\r\n"
        );
        assert_eq!(
            merged("model: x\r\n"),
            "model: x\r\nplugins:\r\n  enabled:\r\n    - hookah\r\n"
        );
    }

    #[test]
    fn shapes_it_cannot_edit_safely_are_refused() {
        for text in [
            "plugins: {}\n",
            "plugins: [a]\n",
            "plugins:\n  enabled: [a, b]\n",
            "plugins:\n  enabled: []\n",
        ] {
            let err = merge(Some(text)).unwrap_err();
            assert!(err.contains("to plugins.enabled by hand"), "{text}: {err}");
        }
    }

    #[test]
    fn a_disabled_hookah_is_refused_because_disable_wins() {
        let err = merge(Some("plugins:\n  disabled:\n    - hookah\n")).unwrap_err();
        assert!(err.contains("plugins.disabled"), "{err}");
        // Another disabled plugin is fine.
        assert!(merge(Some("plugins:\n  disabled:\n    - x\n")).is_ok());
    }

    #[test]
    fn similar_keys_and_nested_lists_do_not_confuse_it() {
        // `enabled` under another top-level key is not plugins.enabled.
        let text = "tools:\n  enabled:\n    - a\nplugins:\n  other: 1\n";
        assert_eq!(
            merged(text),
            "tools:\n  enabled:\n    - a\nplugins:\n  enabled:\n    - hookah\n  other: 1\n"
        );
        // A key that merely starts with `plugins` is not the block.
        assert_eq!(
            merged("plugins_dir: x\n"),
            "plugins_dir: x\nplugins:\n  enabled:\n    - hookah\n"
        );
        // Blank lines and comments inside the block do not end it.
        let text = "plugins:\n\n  # note\n  enabled:\n    - a\n\n    - b\nmodel: y\n";
        assert_eq!(
            merged(text),
            "plugins:\n\n  # note\n  enabled:\n    - a\n\n    - b\n    - hookah\nmodel: y\n"
        );
    }
}
