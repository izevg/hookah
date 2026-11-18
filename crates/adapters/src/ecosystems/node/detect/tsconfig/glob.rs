//! Path strings and the glob patterns of `include` and `exclude`.
//!
//! tsconfig patterns are `/`-separated but written by people on every platform, so everything
//! here works on strings with `\` turned into `/`, never on [`std::path::Path`] components. That
//! keeps the logic identical for `C:\proj\src\a.ts` and `/proj/src/a.ts`, and testable on any host.
//!
//! A pattern segment is a literal, `*` (any characters in one segment), `?` (one character), or
//! `**` (any number of directories). As in `tsc`, wildcards never match a name that starts with
//! `.`, and `**` never enters `node_modules`, `bower_components`, or `jspm_packages`.

/// Directories `**` never enters.
const PACKAGE_DIRS: &[&str] = &["node_modules", "bower_components", "jspm_packages"];

/// `path` with every `\` turned into `/`.
pub fn slashed(path: &str) -> String {
    path.replace('\\', "/")
}

/// Whether `path` (slashed) is absolute: a leading `/` or a drive such as `C:`.
fn is_absolute(path: &str) -> bool {
    path.starts_with('/') || path.as_bytes().get(1) == Some(&b':')
}

/// `rel` resolved against the absolute directory `dir`, with `.` and `..` collapsed. An absolute
/// `rel` replaces `dir`. Both may use either separator.
pub fn resolve(dir: &str, rel: &str) -> String {
    let rel = slashed(rel);
    let joined = if is_absolute(&rel) {
        rel
    } else {
        format!("{}/{rel}", slashed(dir))
    };
    let rooted = joined.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                // The drive of `C:/...` is never popped.
                if parts.last().is_some_and(|p| !p.ends_with(':')) {
                    parts.pop();
                }
            }
            other => parts.push(other),
        }
    }
    let body = parts.join("/");
    if rooted { format!("/{body}") } else { body }
}

/// The part of `path` below `base`, without a leading `/`, or `None` when `path` is not inside
/// `base`. Drive paths compare without regard to case.
pub fn below<'a>(base: &str, path: &'a str) -> Option<&'a str> {
    let base = base.trim_end_matches('/');
    let head = path.get(..base.len())?;
    let same = if is_absolute(base) && !base.starts_with('/') {
        head.eq_ignore_ascii_case(base)
    } else {
        head == base
    };
    let rest = path.get(base.len()..)?;
    (same && (rest.is_empty() || rest.starts_with('/'))).then(|| rest.trim_start_matches('/'))
}

/// The absolute pattern for the entry `item` of a config in `dir`. An entry whose last segment
/// is a bare name (no `.`, `*`, or `?`), or is `.` or `..`, names a directory and stands for
/// everything below it, as in `tsc`; a trailing `**` means `**/*`.
pub fn pattern(dir: &str, item: &str) -> String {
    let item = slashed(item);
    let last = item.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    let directory = matches!(last, "" | "." | "..") || !last.contains(['.', '*', '?']);
    if last == "**" {
        resolve(dir, &format!("{}/*", item.trim_end_matches('/')))
    } else if directory {
        resolve(dir, &format!("{}/**/*", item.trim_end_matches('/')))
    } else {
        resolve(dir, &item)
    }
}

/// Whether the absolute `pattern` (see [`pattern()`]) matches the absolute `path`.
pub fn matches(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    segments(&pattern, &path)
}

fn segments(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => {
            // Zero directories, or one more directory (not the file itself) swallowed.
            segments(rest, path)
                || path.split_first().is_some_and(|(dir, below)| {
                    !below.is_empty() && enterable(dir) && segments(pattern, below)
                })
        }
        Some((seg, rest)) => path
            .split_first()
            .is_some_and(|(name, below)| segment(seg, name) && segments(rest, below)),
    }
}

/// Whether `**` may walk through the directory `name`.
fn enterable(name: &str) -> bool {
    !name.starts_with('.') && !PACKAGE_DIRS.contains(&name)
}

/// One segment against one name. `*` and `?` never match a leading `.`.
fn segment(pattern: &str, name: &str) -> bool {
    if pattern.starts_with(['*', '?']) && name.starts_with('.') {
        return false;
    }
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    wildcard(&p, &n)
}

fn wildcard(pattern: &[char], name: &[char]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some(('*', rest)) => (0..=name.len()).any(|skipped| wildcard(rest, &name[skipped..])),
        Some((c, rest)) => name
            .split_first()
            .is_some_and(|(n, tail)| (*c == '?' || c == n) && wildcard(rest, tail)),
    }
}

/// `dir` and every directory above it, nearest first, ending at the root (`/`, or a drive such as
/// `C:/`).
pub fn ancestors(dir: &str) -> Vec<String> {
    let parts: Vec<&str> = dir.trim_end_matches('/').split('/').collect();
    (1..=parts.len())
        .rev()
        .map(|keep| {
            let joined = parts[..keep].join("/");
            if joined.is_empty() || joined.ends_with(':') {
                format!("{joined}/")
            } else {
                joined
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_against_a_directory() {
        assert_eq!(resolve("/p/app", "src/a.ts"), "/p/app/src/a.ts");
        assert_eq!(resolve("/p/app", "./src//a.ts"), "/p/app/src/a.ts");
        assert_eq!(resolve("/p/app", "../lib/x.ts"), "/p/lib/x.ts");
        assert_eq!(resolve("/p/app", "/abs/y.ts"), "/abs/y.ts");
        assert_eq!(resolve("/", "../../a"), "/a");
        assert_eq!(resolve("C:\\p\\app", "..\\lib\\x.ts"), "C:/p/lib/x.ts");
        assert_eq!(resolve("C:\\p", "..\\..\\..\\a"), "C:/a");
        assert_eq!(resolve("C:/p", "D:\\q\\r.ts"), "D:/q/r.ts");
    }

    #[test]
    fn below_needs_a_whole_segment() {
        assert_eq!(below("/p/app", "/p/app/src/a.ts"), Some("src/a.ts"));
        assert_eq!(below("/p/app/", "/p/app/a.ts"), Some("a.ts"));
        assert_eq!(below("/p/app", "/p/app"), Some(""));
        assert_eq!(below("/p/app", "/p/app2/a.ts"), None);
        assert_eq!(below("/p/app", "/p/a"), None);
        assert_eq!(below("/p/app", "/q/app/a.ts"), None);
        assert_eq!(below("C:/p", "c:/p/a.ts"), Some("a.ts"));
        assert_eq!(below("/p", "/P/a.ts"), None);
        assert_eq!(below("C:/p", "C:/pp/a.ts"), None);
    }

    #[test]
    fn bare_names_are_directories() {
        let m = |item: &str, path: &str| matches(&pattern("/p", item), path);
        assert!(m("src", "/p/src/a.ts"));
        assert!(m("src/", "/p/src/a.ts"));
        assert!(m("src", "/p/src/deep/er/a.ts"));
        assert!(!m("src", "/p/srcs/a.ts"));
        assert!(!m("src", "/p/src"));
        assert!(m("a.ts", "/p/a.ts"));
        assert!(!m("a.ts", "/p/b.ts"));
        assert!(!m("a.ts", "/p/a.ts/b"));
        assert!(m(".", "/p/a.ts"));
        assert!(m("./", "/p/a.ts"));
        assert!(matches(&pattern("/p/q", ".."), "/p/a.ts"));
        assert!(m("**/*", "/p/a/b.ts"));
        assert!(m("src\\lib", "/p/src/lib/a.ts"));
        // A dot in the directory the config lives in changes nothing.
        assert!(matches(&pattern("/p/my.app", "."), "/p/my.app/a.ts"));
        assert!(!matches(&pattern("/p/my.app", "."), "/p/other/a.ts"));
        assert_eq!(pattern("/p/app", "../lib"), "/p/lib/**/*");
        assert_eq!(pattern("/p", "v.config.ts"), "/p/v.config.ts");
        assert_eq!(pattern("C:\\p", "src\\*.ts"), "C:/p/src/*.ts");
    }

    #[test]
    fn wildcards() {
        assert!(matches("/p/**/*", "/p/a.ts"));
        assert!(matches("/p/**/*", "/p/a/b/c.ts"));
        assert!(matches("/p/src/*.ts", "/p/src/a.ts"));
        assert!(!matches("/p/src/*.ts", "/p/src/a/b.ts"));
        assert!(matches("/p/**/*.test.ts", "/p/src/a.test.ts"));
        assert!(matches("/p/**/*.test.ts", "/p/a.test.ts"));
        assert!(!matches("/p/**/*.test.ts", "/p/src/a.ts"));
        assert!(matches("/p/s?c/a.ts", "/p/src/a.ts"));
        assert!(!matches("/p/s?c/a.ts", "/p/sc/a.ts"));
        assert!(matches("/p/src/a*b*c.ts", "/p/src/aXXbYYc.ts"));
        assert!(!matches("/p/src/a*b*c.ts", "/p/src/aXXbYY.ts"));
        assert!(matches(&pattern("/p", "src/**"), "/p/src/a/b.ts"));
        assert!(matches(&pattern("/p", "src/**"), "/p/src/b.ts"));
        assert!(matches("/p/*/a.ts", "/p/src/a.ts"));
        assert!(matches("C:/p/**/*", "C:/p/src/a.ts"));
    }

    #[test]
    fn wildcards_skip_hidden_names_and_package_directories() {
        assert!(!matches("/p/**/*", "/p/.cache/a.ts"));
        assert!(!matches("/p/*", "/p/.a.ts"));
        assert!(!matches("/p/**/*", "/p/node_modules/x/a.ts"));
        assert!(!matches("/p/**/*", "/p/a/bower_components/a.ts"));
        assert!(!matches("/p/**/*", "/p/jspm_packages/a.ts"));
        // A literal segment may name them.
        assert!(matches(
            &pattern("/p", "node_modules"),
            "/p/node_modules/x/a.ts"
        ));
        assert!(matches("/p/.cache/*", "/p/.cache/a.ts"));
        assert!(matches("/p/.a.ts", "/p/.a.ts"));
    }

    #[test]
    fn ancestors_end_at_the_root() {
        assert_eq!(ancestors("/p/a"), ["/p/a", "/p", "/"]);
        assert_eq!(ancestors("/p/a/"), ["/p/a", "/p", "/"]);
        assert_eq!(ancestors("/"), ["/"]);
        assert_eq!(ancestors("C:/p"), ["C:/p", "C:/"]);
    }

    #[test]
    fn a_star_backtracks() {
        assert!(segment("*a", "aaa"));
        assert!(segment("a*a", "aa"));
        assert!(!segment("a*a", "a"));
        assert!(segment("*", ""));
        assert!(segment("a?c", "abc"));
        assert!(!segment("abc", "abd"));
        assert!(!segment("abc", "ab"));
        assert!(!segment("ab", "abc"));
        assert!(segment("*.min.*", "a.min.js"));
        assert!(segment("a**b", "ab"));
        assert!(segment("?*", "x"));
        assert!(!segment("?*", ""));
        assert!(segment("a?c*", "abcd"));
        assert!(!segment("a?c*", "acd"));
    }
}
