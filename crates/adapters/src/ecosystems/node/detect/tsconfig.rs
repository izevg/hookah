//! Reading `tsconfig.json`.
//!
//! `tsconfig.json` is JSON with comments and trailing commas, which `serde_json` rejects.
//! [`strip_jsonc`] removes both outside of strings, and [`has_references`] asks the one question
//! the detector needs: does the project use TypeScript project references? Those need `tsc -b`.

use serde_json::Value;

/// Removes `//` and `/* */` comments and trailing commas outside of string literals.
pub fn strip_jsonc(text: &str) -> String {
    remove_trailing_commas(&remove_comments(text))
}

type Chars<'a> = std::iter::Peekable<std::str::Chars<'a>>;

/// Copies the rest of a string literal (its opening quote is already written) to `out`.
fn copy_string(chars: &mut Chars<'_>, out: &mut String) {
    while let Some(c) = chars.next() {
        out.push(c);
        match c {
            '\\' => out.extend(chars.next()),
            '"' => break,
            _ => {}
        }
    }
}

/// Drops `//` and `/* */` comments. A line comment keeps its newline.
fn remove_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                out.push(c);
                copy_string(&mut chars, &mut out);
            }
            '/' if chars.peek() == Some(&'/') => while chars.next_if(|&n| n != '\n').is_some() {},
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = ' ';
                for n in chars.by_ref() {
                    if previous == '*' && n == '/' {
                        break;
                    }
                    previous = n;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// Drops a comma that is followed, past whitespace, by a closing `}` or `]`.
fn remove_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    // Whitespace seen after a comma that is not yet written.
    let mut pending: Option<String> = None;
    while let Some(c) = chars.next() {
        if c.is_whitespace()
            && let Some(space) = pending.as_mut()
        {
            space.push(c);
            continue;
        }
        if let Some(space) = pending.take() {
            if !matches!(c, '}' | ']') {
                out.push(',');
            }
            out.push_str(&space);
        }
        match c {
            '"' => {
                out.push(c);
                copy_string(&mut chars, &mut out);
            }
            ',' => pending = Some(String::new()),
            _ => out.push(c),
        }
    }
    if let Some(space) = pending {
        out.push(',');
        out.push_str(&space);
    }
    out
}

/// The `path` of every project reference, as written.
pub fn reference_paths(text: &str) -> Vec<String> {
    serde_json::from_str::<Value>(&strip_jsonc(text))
        .ok()
        .and_then(|v| v.get("references")?.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|r| r.get("path")?.as_str().map(str::to_owned))
        .collect()
}

/// Whether the tsconfig text declares at least one project reference. Unreadable text counts as
/// "no references": the plain `-p` command then reports whatever is wrong with it.
pub fn has_references(text: &str) -> bool {
    serde_json::from_str::<Value>(&strip_jsonc(text))
        .ok()
        .and_then(|v| {
            v.get("references")
                .and_then(Value::as_array)
                .map(|r| !r.is_empty())
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_reference_paths() {
        let text = r#"{ // c
          "references": [{ "path": "./a" }, { "path": "b/tsconfig.json", }, { "nopath": 1 }] }"#;
        assert_eq!(reference_paths(text), ["./a", "b/tsconfig.json"]);
        assert!(reference_paths("{}").is_empty());
        assert!(reference_paths("nope").is_empty());
    }

    #[test]
    fn strips_comments_and_trailing_commas() {
        let text = r#"{
          // line comment
          "compilerOptions": { "strict": true, /* block */ "paths": { "@/*": ["src/*",], }, },
          "url": "http://x/y // not a comment",
          "esc": "a\"//b",
        }"#;
        let v: Value = serde_json::from_str(&strip_jsonc(text)).unwrap_or(Value::Null);
        assert_eq!(v["compilerOptions"]["strict"], true);
        assert_eq!(v["compilerOptions"]["paths"]["@/*"][0], "src/*");
        assert_eq!(v["url"], "http://x/y // not a comment");
        assert_eq!(v["esc"], "a\"//b");
    }

    #[test]
    fn comments_end_where_they_should() {
        assert_eq!(strip_jsonc("a // x\nb"), "a \nb");
        assert_eq!(strip_jsonc("a // x"), "a ");
        assert_eq!(strip_jsonc("a /* x */b"), "a b");
        assert_eq!(strip_jsonc("a /** x **/b"), "a b");
        assert_eq!(strip_jsonc("a /*/ b"), "a ");
        assert_eq!(strip_jsonc("a /* x"), "a ");
        assert_eq!(
            strip_jsonc(r#"["//", "/*", "*/"]"#),
            r#"["//", "/*", "*/"]"#
        );
        assert_eq!(strip_jsonc(r#""a\\" // c"#), r#""a\\" "#);
    }

    #[test]
    fn trailing_commas_go_but_separators_stay() {
        assert_eq!(strip_jsonc("[1,]"), "[1]");
        assert_eq!(strip_jsonc("[1, \n ]"), "[1 \n ]");
        assert_eq!(strip_jsonc(r#"{"a":1,}"#), r#"{"a":1}"#);
        assert_eq!(strip_jsonc("[1, // c\n]"), "[1 \n]");
        assert_eq!(strip_jsonc("[1, /* c */ ]"), "[1  ]");
        assert_eq!(strip_jsonc("[1,2]"), "[1,2]");
        assert_eq!(strip_jsonc("[1, 2,\n3]"), "[1, 2,\n3]");
        assert_eq!(strip_jsonc("[1,"), "[1,");
        assert_eq!(strip_jsonc("[1, "), "[1, ");
        assert_eq!(strip_jsonc(r#"["a,]", "b,}"]"#), r#"["a,]", "b,}"]"#);
        assert_eq!(strip_jsonc(r#"[",", ]"#), r#"["," ]"#);
    }

    #[test]
    fn a_lone_slash_is_not_a_comment() {
        assert_eq!(strip_jsonc("a / b"), "a / b");
        assert_eq!(strip_jsonc("a /* c */ b"), "a  b");
    }

    #[test]
    fn detects_references() {
        assert!(has_references(
            r#"{ "files": [], "references": [ { "path": "./a" }, ] }"#
        ));
        assert!(!has_references(r#"{ "references": [] }"#));
        assert!(!has_references(r#"{ "compilerOptions": {} }"#));
        assert!(!has_references("not json at all"));
    }
}
