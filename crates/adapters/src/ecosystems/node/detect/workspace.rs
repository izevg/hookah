//! Workspace members and the packages that depend on one.
//!
//! An API change in a package breaks the workspace packages that use it, not the package itself,
//! so the checks of an edit also cover its dependents, as the Rust adapter does for path
//! dependents. [`dependents`] finds them from the workspace metadata alone:
//!
//! - the members are the directories (holding a `package.json`) matched by the `packages` list of
//!   `pnpm-workspace.yaml`, or by the `workspaces` field of the root `package.json` (an array, or
//!   an object with a `packages` array). Patterns are `/`-separated; a segment is a literal, a
//!   `*` wildcard (`pkg-*`), or `**`; a leading `!` excludes. `node_modules` and hidden
//!   directories are never entered;
//! - a member is a dependent when a dependency section (`dependencies`, `devDependencies`,
//!   `peerDependencies`, `optionalDependencies`) names the package with a spec that resolves to
//!   the workspace copy: any `workspace:` spec, `*`, `latest`, or an empty spec, and a `^`, `~`,
//!   or exact version that the package's own `version` satisfies. Other ranges are not matched.
//!   Only direct dependents count.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::package::{PackageInfo, read_manifest};

const SECTIONS: &[&str] = &[
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];

/// The directories of the workspace members that depend on `info`, sorted. Empty outside a
/// workspace or for an unnamed package.
pub fn dependents(info: &PackageInfo) -> Vec<PathBuf> {
    let Some(name) = info.name.as_deref() else {
        return vec![];
    };
    let patterns = patterns(&info.workspace_root);
    members(&info.workspace_root, &patterns)
        .into_iter()
        .filter(|dir| *dir != info.root)
        .filter(|dir| {
            read_manifest(&dir.join("package.json"))
                .is_ok_and(|m| depends_on(&m, name, info.version.as_deref()))
        })
        .collect()
}

/// Whether `manifest` declares `name` with a spec that the workspace copy at `version` satisfies.
fn depends_on(manifest: &Value, name: &str, version: Option<&str>) -> bool {
    SECTIONS.iter().any(|section| {
        manifest
            .get(section)
            .and_then(|deps| deps.get(name))
            .and_then(Value::as_str)
            .is_some_and(|spec| spec_matches(spec, version))
    })
}

fn spec_matches(spec: &str, version: Option<&str>) -> bool {
    let spec = spec.trim();
    if spec.starts_with("workspace:") || matches!(spec, "" | "*" | "latest" | "x") {
        return true;
    }
    let Some(version) = version else {
        return false;
    };
    let (op, wanted) = match spec.chars().next() {
        Some(c @ ('^' | '~')) => (c, &spec[1..]),
        Some('=') => ('=', &spec[1..]),
        _ => ('=', spec),
    };
    let wanted = numbers(wanted);
    let have = numbers(version);
    if wanted.is_empty() {
        return false;
    }
    let significant = match op {
        '^' => 1,
        '~' => 2,
        _ => 3,
    };
    let count = wanted.len().min(significant);
    have.len() >= count && wanted[..count] == have[..count]
}

/// The leading dotted numbers of a version, `1.2.3-beta` giving `[1, 2, 3]`.
fn numbers(version: &str) -> Vec<u64> {
    let mut out = Vec::new();
    for part in version.trim_start_matches('v').split('.') {
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        match digits.parse() {
            Ok(n) => out.push(n),
            Err(_) => break,
        }
        if digits.len() != part.len() {
            break;
        }
    }
    out
}

/// The workspace patterns declared at `workspace_root`.
fn patterns(workspace_root: &Path) -> Vec<String> {
    if let Ok(text) = fs::read_to_string(workspace_root.join("pnpm-workspace.yaml")) {
        return yaml_packages(&text);
    }
    let Ok(manifest) = read_manifest(&workspace_root.join("package.json")) else {
        return vec![];
    };
    let list = match manifest.get("workspaces") {
        Some(Value::Object(o)) => o.get("packages"),
        other => other,
    };
    list.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The `packages:` list of a `pnpm-workspace.yaml`, in block or flow style.
fn yaml_packages(text: &str) -> Vec<String> {
    let unquote = |s: &str| s.trim().trim_matches(['"', '\'']).to_owned();
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.split(" #").next().unwrap_or("");
        if let Some(rest) = line.strip_prefix("packages:") {
            let rest = rest.trim();
            if let Some(flow) = rest.strip_prefix('[') {
                let flow = flow.trim_end().trim_end_matches(']');
                out.extend(flow.split(',').map(unquote).filter(|p| !p.is_empty()));
                return out;
            }
            inside = true;
        } else if inside {
            if let Some(item) = line.trim_start().strip_prefix("- ") {
                out.push(unquote(item));
            } else if !line.trim().is_empty() && !line.starts_with([' ', '\t']) {
                inside = false;
            }
        }
    }
    out
}

/// The member directories matched by `patterns`, sorted and without duplicates.
fn members(root: &Path, patterns: &[String]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for pattern in patterns.iter().filter(|p| !p.starts_with('!')) {
        expand(root, &segments(pattern), &mut found);
    }
    for pattern in patterns.iter().filter_map(|p| p.strip_prefix('!')) {
        let mut excluded = Vec::new();
        expand(root, &segments(pattern), &mut excluded);
        found.retain(|dir| !excluded.contains(dir));
    }
    found.sort();
    found.dedup();
    found
}

fn segments(pattern: &str) -> Vec<&str> {
    pattern
        .split('/')
        .filter(|s| !matches!(*s, "" | "."))
        .collect()
}

fn expand(base: &Path, segments: &[&str], found: &mut Vec<PathBuf>) {
    let Some((first, rest)) = segments.split_first() else {
        if base.join("package.json").is_file() {
            found.push(base.to_path_buf());
        }
        return;
    };
    if *first == "**" {
        expand(base, rest, found);
        for dir in subdirs(base) {
            expand(&dir, segments, found);
        }
    } else if first.contains('*') {
        for dir in subdirs(base) {
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if wildcard(first, name) {
                expand(&dir, rest, found);
            }
        }
    } else {
        expand(&base.join(first), rest, found);
    }
}

/// The subdirectories of `dir`, except `node_modules` and hidden ones.
fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return vec![];
    };
    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            e.path().is_dir() && name != "node_modules" && !name.starts_with('.')
        })
        .map(|e| e.path())
        .collect()
}

/// Matches `name` against a pattern whose only special character is `*` (any run of characters).
fn wildcard(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let [first, middle @ .., last] = parts.as_slice() else {
        return pattern == name;
    };
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    for part in middle {
        let Some(at) = rest.find(part) else {
            return false;
        };
        rest = &rest[at + part.len()..];
    }
    rest.ends_with(last)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::package::scratch::Scratch;
    use super::*;

    fn names(s: &Scratch, package: &str) -> Vec<String> {
        let info = PackageInfo::read(&s.path(package)).unwrap();
        info.dependents
            .iter()
            .map(|d| d.name.clone().unwrap())
            .collect()
    }

    fn package(s: &Scratch, dir: &str, name: &str, deps: &str) {
        s.write(
            &format!("{dir}/package.json"),
            &format!(r#"{{"name":"{name}","version":"1.2.3","dependencies":{{{deps}}}}}"#),
        );
    }

    #[test]
    fn pnpm_workspace_dependents_by_workspace_protocol() {
        let s = Scratch::new();
        s.write("pnpm-workspace.yaml", "packages:\n  - 'packages/*'\n");
        package(&s, "packages/lib", "@a/lib", "");
        package(&s, "packages/app", "@a/app", r#""@a/lib":"workspace:*""#);
        package(&s, "packages/cli", "@a/cli", r#""@a/app":"workspace:^""#);
        package(
            &s,
            "packages/other",
            "@a/other",
            r#""@a/lib-x":"workspace:*""#,
        );
        assert_eq!(names(&s, "packages/lib"), ["@a/app"]);
        assert_eq!(names(&s, "packages/app"), ["@a/cli"]);
        assert!(names(&s, "packages/cli").is_empty());
    }

    #[test]
    fn npm_workspaces_match_by_version() {
        let s = Scratch::new();
        s.write(
            "package.json",
            r#"{"name":"root","workspaces":["libs/*","apps/**"]}"#,
        );
        package(&s, "libs/core", "core", "");
        package(&s, "apps/web/site", "site", r#""core":"^1.0.0""#);
        package(&s, "libs/tilde", "tilde", r#""core":"~1.2.0""#);
        package(&s, "libs/exact", "exact", r#""core":"1.2.3""#);
        package(&s, "libs/old", "old", r#""core":"^0.9.0""#);
        package(&s, "libs/range", "range", r#""core":">=1""#);
        let mut got = names(&s, "libs/core");
        got.sort();
        assert_eq!(got, ["exact", "site", "tilde"]);
    }

    #[test]
    fn every_dependency_section_counts_and_the_package_itself_does_not() {
        let s = Scratch::new();
        s.write("package.json", r#"{"workspaces":{"packages":["p/*"]}}"#);
        package(&s, "p/a", "a", r#""a":"*""#);
        for (dir, section) in [
            ("dev", "devDependencies"),
            ("peer", "peerDependencies"),
            ("opt", "optionalDependencies"),
        ] {
            s.write(
                &format!("p/{dir}/package.json"),
                &format!(r#"{{"name":"{dir}","{section}":{{"a":"latest"}}}}"#),
            );
        }
        assert_eq!(names(&s, "p/a"), ["dev", "opt", "peer"]);
    }

    #[test]
    fn exclusions_hidden_directories_and_node_modules_are_skipped() {
        let s = Scratch::new();
        s.write(
            "pnpm-workspace.yaml",
            "packages:\n  - \"p/**\"\n  - '!p/skip'\nother: 1\n",
        );
        package(&s, "p/a", "a", "");
        package(&s, "p/skip", "skip", r#""a":"*""#);
        package(&s, "p/node_modules/x", "x", r#""a":"*""#);
        package(&s, "p/.hidden", "hidden", r#""a":"*""#);
        package(&s, "p/deep/er", "er", r#""a":"*""#);
        assert_eq!(names(&s, "p/a"), ["er"]);
    }

    #[test]
    fn no_workspace_or_no_name_means_no_dependents() {
        let s = Scratch::new();
        package(&s, ".", "solo", "");
        assert!(names(&s, ".").is_empty());
        let s = Scratch::new();
        s.write("pnpm-workspace.yaml", "packages: ['p/*']\n");
        s.write("p/a/package.json", "{}");
        package(&s, "p/b", "b", r#""a":"*""#);
        assert!(names(&s, "p/a").is_empty());
    }

    #[test]
    fn patterns_with_dots_and_empty_segments_find_the_same_directories() {
        let s = Scratch::new();
        s.write("pnpm-workspace.yaml", "packages:\n  - ./packages//*\n");
        package(&s, "packages/lib", "lib", "");
        package(&s, "packages/app", "app", r#""lib":"*""#);
        let info = PackageInfo::read(&s.path("packages/lib")).unwrap();
        assert_eq!(info.dependents[0].root, s.path("packages/app"));
    }

    #[test]
    fn spec_rules() {
        assert!(spec_matches("workspace:~", None));
        assert!(spec_matches(" * ", None));
        assert!(!spec_matches("^1.0.0", None));
        assert!(spec_matches("^1", Some("1.9.0")));
        assert!(!spec_matches("^2", Some("1.9.0")));
        assert!(spec_matches("~1.2", Some("1.2.9")));
        assert!(!spec_matches("~1.2.0", Some("1.3.0")));
        assert!(spec_matches("=1.2.3", Some("1.2.3-beta.1")));
        assert!(!spec_matches("1.2.4", Some("1.2.3")));
        assert!(!spec_matches("npm:other@1", Some("1.0.0")));
        assert!(!spec_matches("^1.2.3", Some("beta")));
        assert!(spec_matches("v1.2.3", Some("v1.2.3")));
    }

    #[test]
    fn wildcards() {
        assert!(wildcard("*", "anything"));
        assert!(wildcard("pkg-*", "pkg-a"));
        assert!(!wildcard("pkg-*", "xpkg-a"));
        assert!(wildcard("*-lib", "my-lib"));
        assert!(!wildcard("*-lib", "my-lib2"));
        assert!(wildcard("a*b*c", "a--b--c"));
        assert!(!wildcard("a*b*c", "a--c"));
        assert!(wildcard("exact", "exact"));
        assert!(!wildcard("exact", "exact2"));
        assert!(!wildcard("a*a", "a"));
        assert!(wildcard("a*b*b", "abb"));
        assert!(!wildcard("a*b*b", "ab"));
    }

    #[test]
    fn yaml_lists() {
        assert_eq!(
            yaml_packages("packages:\n  - a/*  # c\n  - \"b\"\ncatalog:\n  - x\n"),
            ["a/*", "b"]
        );
        assert_eq!(yaml_packages("packages: [ 'a/*', b ]\n"), ["a/*", "b"]);
        assert_eq!(
            yaml_packages("packages:\n  - a\n\n    # note\n  - b\nx: 1\n  - c\n"),
            ["a", "b"]
        );
        assert!(yaml_packages("other: 1\n").is_empty());
    }
}
