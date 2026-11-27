//! Value expansion and path matching. Pure string logic, no I/O.
//!
//! Expansion is a single pass, so substituted text is never expanded again:
//!
//! - a leading `~` (alone, or followed by `/` or `\`) is the home directory;
//! - `{root}` is the project root and `{git_root}` the root of its git checkout;
//! - `${NAME}` is the inherited environment variable `NAME` (empty when unset).
//!
//! Anything else, including an unknown `{name}` or a `$` without braces, stays as written.

/// What a value can refer to.
pub struct Context<'a> {
    /// The user's home directory.
    pub home: Option<&'a str>,
    /// The project root.
    pub root: &'a str,
    /// The root of the git checkout, or the project root when there is none.
    pub git_root: &'a str,
    /// Inherited environment lookup.
    pub var: &'a dyn Fn(&str) -> Option<String>,
}

/// Expands `value`.
pub fn expand(value: &str, ctx: &Context) -> String {
    let (home, rest) = split_home(value, ctx.home);
    let mut out = String::from(home);
    let mut rest = rest;
    while let Some(i) = rest.find(['{', '$']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some((text, after)) = substitute(tail, ctx) {
            out.push_str(&text);
            rest = after;
        } else {
            out.push_str(&tail[..1]);
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

fn split_home<'a>(value: &'a str, home: Option<&'a str>) -> (&'a str, &'a str) {
    match (home, value.strip_prefix('~')) {
        (Some(home), Some(rest))
            if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') =>
        {
            (home, rest)
        }
        _ => ("", value),
    }
}

/// The replacement for the placeholder at the start of `tail`, and the text after it.
fn substitute<'a>(tail: &'a str, ctx: &Context) -> Option<(String, &'a str)> {
    if let Some(after) = tail.strip_prefix("{root}") {
        return Some((ctx.root.to_string(), after));
    }
    if let Some(after) = tail.strip_prefix("{git_root}") {
        return Some((ctx.git_root.to_string(), after));
    }
    let name_and_rest = tail.strip_prefix("${")?;
    let (name, after) = name_and_rest.split_once('}')?;
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    Some(((ctx.var)(name).unwrap_or_default(), after))
}

/// Whether the project at `path` is selected by `pattern`.
///
/// A pattern without `*` or `?` is a prefix: it selects the directory and everything below it. A
/// glob selects a directory it matches and everything below that, where `*` and `?` stay inside
/// one path segment and `**` spans any number of segments. Separators are normalized, so `\` and
/// `/` are the same; `windows` also ignores case.
pub fn matches(pattern: &str, path: &str, windows: bool) -> bool {
    let norm = |s: &str| {
        let s = s.replace('\\', "/");
        let s = s.trim_end_matches('/').to_string();
        if windows { s.to_lowercase() } else { s }
    };
    let (pattern, path) = (norm(pattern), norm(path));
    let is_glob = pattern.contains(['*', '?']);
    let pat: Vec<&str> = pattern.split('/').collect();
    let mut current: Vec<&str> = path.split('/').collect();
    loop {
        let hit = if is_glob {
            segments(&pat, &current)
        } else {
            pat == current
        };
        if hit {
            return true;
        }
        if current.len() <= 1 {
            return false;
        }
        current.pop();
    }
}

/// Whether `pattern` matches `dir` itself or could match some descendant directory below `dir`.
pub fn matches_subtree(pattern: &str, dir: &str, windows: bool) -> bool {
    if matches(pattern, dir, windows) {
        return true;
    }
    let norm = |s: &str| {
        let s = s.replace('\\', "/");
        let s = s.trim_end_matches('/').to_string();
        if windows { s.to_lowercase() } else { s }
    };
    let (pattern, dir) = (norm(pattern), norm(dir));
    let pat: Vec<&str> = pattern.split('/').collect();
    let base: Vec<&str> = dir.split('/').collect();
    segments_prefix(&pat, &base)
}

/// True if `pat` can match `base` extended by zero or more trailing segments.
fn segments_prefix(pat: &[&str], base: &[&str]) -> bool {
    match (pat.split_first(), base.split_first()) {
        (None, _) | (Some(_), None) => true,
        (Some((&"**", rest)), Some(_)) => {
            (0..=base.len()).any(|skip| segments_prefix(rest, &base[skip..]))
        }
        (Some((first, rest)), Some((b, tail))) => wildcard(first, b) && segments_prefix(rest, tail),
    }
}

fn segments(pat: &[&str], path: &[&str]) -> bool {
    match pat.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments(rest, &path[skip..])),
        Some((first, rest)) => path
            .split_first()
            .is_some_and(|(p, tail)| wildcard(first, p) && segments(rest, tail)),
    }
}

/// `*` (any run) and `?` (any one character) inside a single segment.
fn wildcard(pat: &str, text: &str) -> bool {
    let (pat, text): (Vec<char>, Vec<char>) = (pat.chars().collect(), text.chars().collect());
    chars(&pat, &text)
}

fn chars(pat: &[char], text: &[char]) -> bool {
    match pat.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len()).any(|skip| chars(rest, &text[skip..])),
        Some((c, rest)) => text
            .split_first()
            .is_some_and(|(t, tail)| (*c == '?' || c == t) && chars(rest, tail)),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn run(value: &str) -> String {
        let var = |n: &str| match n {
            "PATH" => Some("/bin:/usr/bin".to_string()),
            "EMPTY" => Some(String::new()),
            _ => None,
        };
        let ctx = Context {
            home: Some("/home/u"),
            root: "/p/app",
            git_root: "/p",
            var: &var,
        };
        expand(value, &ctx)
    }

    #[test]
    fn expands_home_only_at_the_start() {
        assert_eq!(run("~"), "/home/u");
        assert_eq!(run("~/x"), "/home/u/x");
        assert_eq!(run("~\\x"), "/home/u\\x");
        assert_eq!(run("-s ~/x"), "-s ~/x");
        assert_eq!(run("~other/x"), "~other/x");
    }

    #[test]
    fn expands_without_home_leaves_the_tilde() {
        let var = |_: &str| None;
        let ctx = Context {
            home: None,
            root: "/r",
            git_root: "/r",
            var: &var,
        };
        assert_eq!(expand("~/x", &ctx), "~/x");
    }

    #[test]
    fn expands_placeholders() {
        assert_eq!(run("{root}/bin:${PATH}"), "/p/app/bin:/bin:/usr/bin");
        assert_eq!(run("-s {git_root}/s.xml"), "-s /p/s.xml");
        assert_eq!(run("a${EMPTY}b${NOPE}c"), "abc");
    }

    #[test]
    fn leaves_what_it_does_not_know() {
        assert_eq!(
            run("{other} $HOME ${ ${} ${PATH"),
            "{other} $HOME ${ ${} ${PATH"
        );
        assert_eq!(run("{root"), "{root");
        assert_eq!(run(""), "");
    }

    #[test]
    fn substituted_text_is_not_expanded_again() {
        let var = |n: &str| (n == "A").then(|| "${B}".to_string());
        let ctx = Context {
            home: None,
            root: "{git_root}",
            git_root: "g",
            var: &var,
        };
        assert_eq!(expand("${A}{root}", &ctx), "${B}{git_root}");
    }

    #[test]
    fn a_plain_pattern_is_a_prefix() {
        assert!(matches("/w/corp", "/w/corp", false));
        assert!(matches("/w/corp", "/w/corp/app/sub", false));
        assert!(matches("/w/corp/", "/w/corp/app", false));
        assert!(!matches("/w/corp", "/w/corporate", false));
        assert!(!matches("/w/corp", "/w", false));
    }

    #[test]
    fn globs_span_segments_with_double_star() {
        assert!(matches("/w/corp/**", "/w/corp/a/b", false));
        assert!(matches("/w/corp/**", "/w/corp", false));
        assert!(!matches("/w/corp/**", "/w/other/a", false));
        assert!(matches("/w/**/app", "/w/a/b/app", false));
        assert!(matches("/w/**/app", "/w/app/sub", false));
        assert!(!matches("/w/**/app", "/w/a/b/api", false));
    }

    #[test]
    fn single_star_stays_in_a_segment_but_covers_what_is_below() {
        assert!(matches("/w/*", "/w/a", false));
        assert!(matches("/w/*", "/w/a/b", false));
        assert!(!matches("/w/*", "/w", false));
        assert!(matches("/w/c*p/x", "/w/corp/x", false));
        assert!(matches("/w/c?rp", "/w/corp", false));
        assert!(!matches("/w/c?rp", "/w/coorp", false));
        assert!(matches("/w/*r*", "/w/corp", false));
        assert!(!matches("/w/a*b", "/w/axc", false));
        assert!(matches("/w/a*b*c", "/w/aXbYbZc", false));
    }

    #[test]
    fn windows_paths_and_case() {
        assert!(matches("C:\\Work\\corp\\**", "C:\\Work\\corp\\app", true));
        assert!(matches("c:/work/corp", "C:\\Work\\Corp\\app", true));
        assert!(!matches("c:/work/corp", "C:\\Work\\Corp\\app", false));
        assert!(!matches("C:/Work/corp", "D:/Work/corp", true));
    }

    #[test]
    fn matches_subtree_distinguishes_unrelated_paths_from_descendants() {
        assert!(matches_subtree("/work/repo/sub", "/work/repo", false));
        assert!(matches_subtree("/work/repo/**/child", "/work/repo", false));
        assert!(matches_subtree("/work/r*/child", "/work/repo", false));
        assert!(matches_subtree("/work", "/work/repo", false));
        assert!(!matches_subtree(
            "/somewhere/unrelated",
            "/work/repo",
            false
        ));
        assert!(!matches_subtree("/somewhere/**/child", "/work/repo", false));
        assert!(!matches_subtree("/work/other/sub", "/work/repo", false));
        assert!(!matches_subtree("/work/repo2/sub", "/work/repo", false));
    }
}
