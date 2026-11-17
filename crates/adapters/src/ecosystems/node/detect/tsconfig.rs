//! Reading `tsconfig.json`.
//!
//! `tsconfig.json` is JSON with comments and trailing commas, which `serde_json` rejects.
//! [`strip_jsonc`] removes both outside of strings, and [`has_references`] asks the one question
//! the detector needs: does the project use TypeScript project references? Those need `tsc -b`.

use serde_json::Value;

/// Removes `//` and `/* */` comments and trailing commas outside of string literals.
pub fn strip_jsonc(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => {
                out.push(c);
                i += 1;
                while i < chars.len() {
                    out.push(chars[i]);
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        out.push(chars[i + 1]);
                        i += 2;
                        continue;
                    }
                    i += 1;
                    if chars[i - 1] == '"' {
                        break;
                    }
                }
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
            }
            ',' if next_significant(&chars, i + 1).is_some_and(|n| n == '}' || n == ']') => {
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// The next character at or after `from` that is not whitespace and not inside a comment.
fn next_significant(chars: &[char], mut from: usize) -> Option<char> {
    while from < chars.len() {
        match chars[from] {
            c if c.is_whitespace() => from += 1,
            '/' if chars.get(from + 1) == Some(&'/') => {
                while from < chars.len() && chars[from] != '\n' {
                    from += 1;
                }
            }
            '/' if chars.get(from + 1) == Some(&'*') => {
                from += 2;
                while from < chars.len()
                    && !(chars[from] == '*' && chars.get(from + 1) == Some(&'/'))
                {
                    from += 1;
                }
                from += 2;
            }
            c => return Some(c),
        }
    }
    None
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
    fn detects_references() {
        assert!(has_references(
            r#"{ "files": [], "references": [ { "path": "./a" }, ] }"#
        ));
        assert!(!has_references(r#"{ "references": [] }"#));
        assert!(!has_references(r#"{ "compilerOptions": {} }"#));
        assert!(!has_references("not json at all"));
    }
}
