//! Which tsconfig owns a file.
//!
//! A solution-style `tsconfig.json` (`"files": []` plus `references`) checks nothing itself; each
//! referenced config is a project with its own `files`, `include`, and `exclude`. An edit to a
//! file is checked by the project whose file set contains it, never by its siblings: the Vite
//! template's `tsconfig.node.json` (for `vite.config.ts`) has nothing to say about `src/`.
//!
//! [`projects`] walks the references recursively and answers with the configs that own the
//! files, plus the configs that reference them (an edit in a referenced project can break its
//! users). A config's file set follows `tsc`: `files` first (these ignore `exclude`), then
//! `include` (default `**/*` when `files` is absent) minus `exclude` (default `node_modules`,
//! `bower_components`, `jspm_packages`, and the `outDir`). Wildcard matches count only for the
//! extensions `tsc` reads: TypeScript, plus JavaScript with `allowJs`. `files`, `include`,
//! `exclude`, and `outDir` are inherited through `extends` (a relative file, a package in
//! `node_modules`, or a list of them; the last one wins) and resolve against the directory of the
//! config that sets them. Anything unreadable is treated as an empty config, so a broken
//! tsconfig owns nothing.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::glob::{ancestors, below, matches, pattern, resolve, slashed};
use super::strip_jsonc;

/// Most configs one reference tree may hold, and the deepest `extends` chain followed.
const MAX_CONFIGS: usize = 64;
const MAX_EXTENDS: usize = 16;

const TYPESCRIPT: &[&str] = &["ts", "tsx", "mts", "cts"];
const JAVASCRIPT: &[&str] = &["js", "jsx", "mjs", "cjs"];
const DEFAULT_EXCLUDE: &[&str] = &["node_modules", "bower_components", "jspm_packages"];

/// A list from a config, with the directory its entries are relative to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Located {
    dir: String,
    items: Vec<String>,
}

/// The inheritable file-set settings of a config.
#[derive(Debug, Clone, Default)]
struct Settings {
    files: Option<Located>,
    include: Option<Located>,
    exclude: Option<Located>,
    out_dir: Option<Located>,
    allow_js: Option<bool>,
    /// Whether `typeRoots` is set anywhere in the chain.
    type_roots: bool,
}

impl Settings {
    /// Takes every setting `other` has, replacing this one's.
    fn overlay(&mut self, other: Settings) {
        let Settings {
            files,
            include,
            exclude,
            out_dir,
            allow_js,
            type_roots,
        } = other;
        self.files = files.or(self.files.take());
        self.include = include.or(self.include.take());
        self.exclude = exclude.or(self.exclude.take());
        self.out_dir = out_dir.or(self.out_dir.take());
        self.allow_js = allow_js.or(self.allow_js);
        self.type_roots |= type_roots;
    }
}

/// One tsconfig: where it is, its settings with `extends` applied, and its references.
#[derive(Debug, Clone)]
struct Config {
    /// The config file, slashed and absolute.
    file: String,
    dir: String,
    settings: Settings,
    references: Vec<String>,
}

impl Config {
    fn read(path: &Path) -> Option<Self> {
        let file = slashed(&path.to_string_lossy());
        let value = read_json(&file)?;
        let dir = parent(&file);
        let mut settings = Settings::default();
        for base in extends_of(&value) {
            if let Some(found) = base_settings(&dir, &base, 1) {
                settings.overlay(found);
            }
        }
        settings.overlay(own_settings(&value, &dir));
        Some(Self {
            file,
            dir,
            settings,
            references: references_of(&value),
        })
    }

    /// Whether the config lists any file at all. A solution root with `"files": []` does not.
    fn has_file_set(&self) -> bool {
        let s = &self.settings;
        match (&s.files, &s.include) {
            (None, None) => true,
            (files, include) => [files, include]
                .iter()
                .any(|l| l.as_ref().is_some_and(|l| !l.items.is_empty())),
        }
    }

    /// Whether the absolute, slashed `file` is part of the config's program.
    fn owns(&self, file: &str) -> bool {
        let s = &self.settings;
        if let Some(files) = &s.files
            && files
                .items
                .iter()
                .any(|item| resolve(&files.dir, item) == file)
        {
            return true;
        }
        if !self.extension_read(file) {
            return false;
        }
        let default_include;
        let include = match (&s.include, &s.files) {
            (Some(include), _) => include,
            (None, None) => {
                default_include = Located {
                    dir: self.dir.clone(),
                    items: vec!["**/*".into()],
                };
                &default_include
            }
            (None, Some(_)) => return false,
        };
        if !included(include, file) {
            return false;
        }
        !self.excluded(file)
    }

    fn excluded(&self, file: &str) -> bool {
        let s = &self.settings;
        match &s.exclude {
            Some(exclude) => included(exclude, file),
            None => {
                let defaults = DEFAULT_EXCLUDE.iter().map(|d| pattern(&self.dir, d));
                let out = s
                    .out_dir
                    .iter()
                    .flat_map(|o| o.items.iter().map(|item| pattern(&o.dir, item)));
                defaults.chain(out).any(|p| matches(&p, file))
            }
        }
    }

    fn extension_read(&self, file: &str) -> bool {
        let name = file.rsplit('/').next().unwrap_or(file);
        let Some((_, ext)) = name.rsplit_once('.') else {
            return false;
        };
        TYPESCRIPT.contains(&ext)
            || (self.settings.allow_js == Some(true) && JAVASCRIPT.contains(&ext))
    }
}

/// Whether one of `list`'s patterns matches `file`.
fn included(list: &Located, file: &str) -> bool {
    list.items
        .iter()
        .any(|item| matches(&pattern(&list.dir, item), file))
}

fn read_json(file: &str) -> Option<Value> {
    let text = fs::read_to_string(file).ok()?;
    serde_json::from_str(&strip_jsonc(&text)).ok()
}

fn parent(file: &str) -> String {
    file.rsplit_once('/')
        .map(|(dir, _)| if dir.is_empty() { "/" } else { dir })
        .unwrap_or(".")
        .to_string()
}

fn strings(value: Option<&Value>) -> Option<Vec<String>> {
    Some(
        value?
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
    )
}

fn own_settings(value: &Value, dir: &str) -> Settings {
    let located = |items: Option<Vec<String>>| {
        items.map(|items| Located {
            dir: dir.to_string(),
            items,
        })
    };
    let options = value.get("compilerOptions");
    Settings {
        files: located(strings(value.get("files"))),
        include: located(strings(value.get("include"))),
        exclude: located(strings(value.get("exclude"))),
        out_dir: located(
            options
                .and_then(|o| o.get("outDir")?.as_str())
                .map(|s| vec![s.to_string()]),
        ),
        allow_js: options.and_then(|o| o.get("allowJs")?.as_bool()),
        type_roots: options.is_some_and(|o| o.get("typeRoots").is_some()),
    }
}

/// The `extends` entries: one string or a list of them.
fn extends_of(value: &Value) -> Vec<String> {
    match value.get("extends") {
        Some(Value::String(s)) => vec![s.clone()],
        other => strings(other).unwrap_or_default(),
    }
}

fn references_of(value: &Value) -> Vec<String> {
    value
        .get("references")
        .and_then(Value::as_array)
        .map(|refs| {
            refs.iter()
                .filter_map(|r| r.get("path")?.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The settings of the base config `spec`, named from a config in `dir`.
fn base_settings(dir: &str, spec: &str, depth: usize) -> Option<Settings> {
    if depth > MAX_EXTENDS {
        return None;
    }
    let file = resolve_extends(dir, spec)?;
    let value = read_json(&file)?;
    let base_dir = parent(&file);
    let mut settings = Settings::default();
    for next in extends_of(&value) {
        if let Some(found) = base_settings(&base_dir, &next, depth + 1) {
            settings.overlay(found);
        }
    }
    settings.overlay(own_settings(&value, &base_dir));
    Some(settings)
}

/// The file an `extends` entry names: a relative path (`.json` is optional), or a package
/// looked up in the `node_modules` directories above `dir`.
fn resolve_extends(dir: &str, spec: &str) -> Option<String> {
    let spec = slashed(spec);
    let is_file = |p: &str| Path::new(p).is_file();
    if spec.starts_with('.') || spec.starts_with('/') || spec.as_bytes().get(1) == Some(&b':') {
        let base = resolve(dir, &spec);
        return [base.clone(), format!("{base}.json")]
            .into_iter()
            .find(|p| is_file(p));
    }
    ancestors(dir).into_iter().find_map(|at| {
        let base = format!("{}/node_modules/{spec}", at.trim_end_matches('/'));
        [
            base.clone(),
            format!("{base}.json"),
            format!("{base}/tsconfig.json"),
        ]
        .into_iter()
        .find(|p| is_file(p))
    })
}

/// The config file a reference `path` names, from a config in `dir`: the file itself, or
/// `tsconfig.json` of a directory.
fn reference_file(dir: &str, path: &str) -> String {
    let target = resolve(dir, path);
    if Path::new(&target).is_dir() {
        format!("{target}/tsconfig.json")
    } else {
        target
    }
}

/// The root config and every config it references, directly or not, each once, root first.
fn tree(root: &Path) -> Vec<Config> {
    let mut out: Vec<Config> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut pending = vec![slashed(&root.to_string_lossy())];
    while let Some(file) = pending.pop() {
        if out.len() >= MAX_CONFIGS || !seen.insert(file.clone()) {
            continue;
        }
        let Some(config) = Config::read(Path::new(&file)) else {
            continue;
        };
        // Reversed, so that the first reference is read first.
        pending.extend(
            config
                .references
                .iter()
                .rev()
                .map(|r| reference_file(&config.dir, r)),
        );
        out.push(config);
    }
    out
}

/// A project found by [`projects`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The config file, slashed and absolute.
    pub file: PathBuf,
    /// Whether it declares project `references`.
    pub referencing: bool,
    /// Whether its `typeRoots` is set, here or in a config it extends.
    pub type_roots: bool,
}

/// The configs of the tree under `root` that hold a file set, for the whole-project run (`files`
/// is `None`), or that own one of the absolute `files`. For files, the root itself is left out
/// when a referenced config answers instead, and the configs that reference an answering
/// config (directly or not) are added: an API change breaks its users. Empty when nothing matches. Slashed absolute paths.
pub fn projects(root: &Path, files: Option<&[String]>) -> Vec<Found> {
    let configs = tree(root);
    let first = configs.first().map(|c| c.file.as_str());
    let mut wanted: Vec<bool> = configs
        .iter()
        .map(|c| match files {
            None => c.has_file_set(),
            Some(files) => files.iter().any(|f| c.owns(&slashed(f))),
        })
        .collect();
    if files.is_some() {
        // The root answers only when no referenced config does.
        let referenced = configs
            .iter()
            .zip(&wanted)
            .any(|(c, &w)| w && Some(c.file.as_str()) != first);
        if referenced && let Some(first) = wanted.first_mut() {
            *first = false;
        }
        // An edit can break the projects that reference the one that owns the file, and those that
        // reference them in turn: one round per config is enough to follow any chain.
        for _ in &configs {
            let before = wanted.clone();
            for (config, flag) in configs.iter().zip(wanted.iter_mut()) {
                let reaches = config.references.iter().any(|r| {
                    let target = reference_file(&config.dir, r);
                    configs
                        .iter()
                        .zip(&before)
                        .any(|(c, &w)| w && c.file == target)
                });
                *flag |= reaches && config.has_file_set();
            }
        }
    }
    configs
        .iter()
        .zip(wanted)
        .filter(|&(_, w)| w)
        .map(|(c, _)| Found {
            file: PathBuf::from(&c.file),
            referencing: !c.references.is_empty(),
            type_roots: c.settings.type_roots,
        })
        .collect()
}

/// Whether `file` (a config path) lies in `dir`.
pub fn is_within(dir: &Path, file: &Path) -> bool {
    below(
        &slashed(&dir.to_string_lossy()),
        &slashed(&file.to_string_lossy()),
    )
    .is_some()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::ecosystems::node::detect::package::scratch::Scratch;

    fn owners(s: &Scratch, root: &str, files: &[&str]) -> Vec<String> {
        let files: Vec<String> = files
            .iter()
            .map(|f| slashed(&s.path(f).to_string_lossy()))
            .collect();
        let base = slashed(&s.0.to_string_lossy());
        projects(&s.path(root), Some(&files))
            .iter()
            .map(|p| slashed(&p.file.to_string_lossy()).replace(&format!("{base}/"), ""))
            .collect()
    }

    /// The Vite template: a solution root, an app project, and a config project.
    fn vite() -> Scratch {
        let s = Scratch::new();
        s.write(
            "tsconfig.json",
            r#"{ "files": [], "references": [{ "path": "./tsconfig.app.json" }, { "path": "./tsconfig.node.json" }] }"#,
        );
        s.write(
            "tsconfig.app.json",
            r#"{ // app
              "compilerOptions": { "composite": true },
              "include": ["src"], }"#,
        );
        s.write(
            "tsconfig.node.json",
            r#"{ "compilerOptions": { "composite": true }, "include": ["vite.config.ts"] }"#,
        );
        s
    }

    #[test]
    fn the_solution_root_owns_nothing_and_each_leaf_owns_its_files() {
        let s = vite();
        assert_eq!(
            owners(&s, "tsconfig.json", &["src/a.ts"]),
            ["tsconfig.app.json"]
        );
        assert_eq!(
            owners(&s, "tsconfig.json", &["src/x/y.tsx"]),
            ["tsconfig.app.json"]
        );
        assert_eq!(
            owners(&s, "tsconfig.json", &["vite.config.ts"]),
            ["tsconfig.node.json"]
        );
        assert!(owners(&s, "tsconfig.json", &["scripts/a.ts"]).is_empty());
        assert!(owners(&s, "tsconfig.json", &["src/readme.md"]).is_empty());
        assert!(owners(&s, "tsconfig.json", &["src/a.js"]).is_empty());
        assert_eq!(
            owners(&s, "tsconfig.json", &["vite.config.ts", "src/a.ts"]),
            ["tsconfig.app.json", "tsconfig.node.json"]
        );
    }

    #[test]
    fn the_whole_project_run_takes_every_leaf_with_a_file_set() {
        let s = vite();
        let all = projects(&s.path("tsconfig.json"), None);
        assert_eq!(all.len(), 2);
        // A plain config is its own project.
        let s = Scratch::new();
        s.write("tsconfig.json", "{}");
        assert_eq!(projects(&s.path("tsconfig.json"), None).len(), 1);
        // A root that lists files of its own stays next to its references.
        s.write(
            "tsconfig.json",
            r#"{"include":["a"],"references":[{"path":"b"}]}"#,
        );
        s.write("b/tsconfig.json", r#"{"files":["x.ts"]}"#);
        assert_eq!(projects(&s.path("tsconfig.json"), None).len(), 2);
    }

    #[test]
    fn files_win_over_exclude_and_exclude_wins_over_include() {
        let s = Scratch::new();
        s.write(
            "tsconfig.json",
            r#"{ "files": ["src/main.ts"], "include": ["src"], "exclude": ["src/main.ts", "src/gen", "**/*.spec.ts"] }"#,
        );
        assert_eq!(
            owners(&s, "tsconfig.json", &["src/main.ts"]),
            ["tsconfig.json"]
        );
        assert_eq!(
            owners(&s, "tsconfig.json", &["src/a.ts"]),
            ["tsconfig.json"]
        );
        assert!(owners(&s, "tsconfig.json", &["src/gen/a.ts"]).is_empty());
        assert!(owners(&s, "tsconfig.json", &["src/deep/a.spec.ts"]).is_empty());
    }

    #[test]
    fn defaults_cover_the_directory_but_not_packages_or_the_out_dir() {
        let s = Scratch::new();
        s.write(
            "tsconfig.json",
            r#"{ "compilerOptions": { "outDir": "build" } }"#,
        );
        assert_eq!(
            owners(&s, "tsconfig.json", &["a.ts", "x/y.mts", "x/y.d.ts"]),
            ["tsconfig.json"]
        );
        assert!(owners(&s, "tsconfig.json", &["node_modules/p/a.ts"]).is_empty());
        assert!(owners(&s, "tsconfig.json", &["build/a.ts"]).is_empty());
        // An explicit exclude replaces the defaults.
        s.write("tsconfig.json", r#"{ "exclude": ["dist"] }"#);
        assert_eq!(
            owners(&s, "tsconfig.json", &["build/a.ts"]),
            ["tsconfig.json"]
        );
        // `files` alone has no include.
        s.write("tsconfig.json", r#"{ "files": ["a.ts"] }"#);
        assert!(owners(&s, "tsconfig.json", &["b.ts"]).is_empty());
        assert_eq!(owners(&s, "tsconfig.json", &["a.ts"]), ["tsconfig.json"]);
    }

    #[test]
    fn javascript_counts_with_allow_js() {
        let s = Scratch::new();
        s.write(
            "tsconfig.json",
            r#"{ "compilerOptions": { "allowJs": true } }"#,
        );
        assert_eq!(
            owners(&s, "tsconfig.json", &["a.js", "b.jsx", "c.cjs"]),
            ["tsconfig.json"]
        );
        assert!(owners(&s, "tsconfig.json", &["data.json", "Makefile"]).is_empty());
    }

    #[test]
    fn extends_brings_the_file_set_relative_to_the_base() {
        let s = Scratch::new();
        s.write(
            "base/tsconfig.base.json",
            r#"{ "include": ["src"], "compilerOptions": { "allowJs": true } }"#,
        );
        s.write(
            "app/tsconfig.json",
            r#"{ "extends": "../base/tsconfig.base" }"#,
        );
        // `src` resolves against the base's directory.
        assert_eq!(
            owners(&s, "app/tsconfig.json", &["base/src/a.js"]),
            ["app/tsconfig.json"]
        );
        assert!(owners(&s, "app/tsconfig.json", &["app/src/a.ts"]).is_empty());
        // The child overrides what it sets.
        s.write(
            "app/tsconfig.json",
            r#"{ "extends": "../base/tsconfig.base.json", "include": ["lib"] }"#,
        );
        assert_eq!(
            owners(&s, "app/tsconfig.json", &["app/lib/a.js"]),
            ["app/tsconfig.json"]
        );
        assert!(owners(&s, "app/tsconfig.json", &["base/src/a.js"]).is_empty());
    }

    #[test]
    fn extends_lists_and_packages_and_chains() {
        let s = Scratch::new();
        s.write(
            "node_modules/@cfg/node/tsconfig.json",
            r#"{ "include": ["src"] }"#,
        );
        s.write("one.json", r#"{ "include": ["one"] }"#);
        s.write("two.json", r#"{ "include": ["two"] }"#);
        s.write(
            "a/tsconfig.json",
            r#"{ "extends": ["../one.json", "../two"] }"#,
        );
        // The last one wins.
        assert_eq!(
            owners(&s, "a/tsconfig.json", &["two/a.ts"]),
            ["a/tsconfig.json"]
        );
        assert!(owners(&s, "a/tsconfig.json", &["one/a.ts"]).is_empty());
        // A package found in a `node_modules` above, through a chain.
        s.write("mid.json", r#"{ "extends": "@cfg/node" }"#);
        s.write("a/tsconfig.json", r#"{ "extends": "../mid.json" }"#);
        // The package's `include` resolves against the package's own directory.
        assert_eq!(
            owners(&s, "a/tsconfig.json", &["node_modules/@cfg/node/src/x.ts"]),
            ["a/tsconfig.json"]
        );
        assert!(owners(&s, "a/tsconfig.json", &["a/x.ts"]).is_empty());
        // An absolute path names the file as well.
        let absolute = slashed(&s.path("one.json").to_string_lossy());
        s.write(
            "a/tsconfig.json",
            &format!(r#"{{ "extends": "{absolute}" }}"#),
        );
        assert_eq!(
            owners(&s, "a/tsconfig.json", &["one/x.ts"]),
            ["a/tsconfig.json"]
        );
        // An unresolvable base is ignored; a cycle ends.
        s.write("a/tsconfig.json", r#"{ "extends": "@nope/missing" }"#);
        assert_eq!(
            owners(&s, "a/tsconfig.json", &["a/x.ts"]),
            ["a/tsconfig.json"]
        );
        s.write("c1.json", r#"{ "extends": "./c2.json" }"#);
        s.write("c2.json", r#"{ "extends": "./c1.json", "include": ["a"] }"#);
        s.write("a/tsconfig.json", r#"{ "extends": "../c1.json" }"#);
        // The chain is cut where it loops; `include` of c2 is relative to its own directory.
        assert_eq!(
            owners(&s, "a/tsconfig.json", &["a/x.ts"]),
            ["a/tsconfig.json"]
        );
        assert!(owners(&s, "a/tsconfig.json", &["b/x.ts"]).is_empty());
    }

    #[test]
    fn extends_chains_stop_after_sixteen_bases() {
        let chain = |length: usize| {
            let s = Scratch::new();
            for i in 1..=length {
                let next = if i == length {
                    r#""include": ["deep"]"#.to_string()
                } else {
                    format!(r#""extends": "./b{}.json""#, i + 1)
                };
                s.write(&format!("b{i}.json"), &format!("{{ {next} }}"));
            }
            s.write("tsconfig.json", r#"{ "extends": "./b1.json" }"#);
            owners(&s, "tsconfig.json", &["other/x.ts"])
        };
        // The last base limits the include to `deep`.
        assert!(chain(16).is_empty());
        // The seventeenth base is not read, so the default include (the whole directory) stays.
        assert_eq!(chain(17), ["tsconfig.json"]);
    }

    #[test]
    fn references_nest_and_may_point_at_directories_or_files() {
        let s = Scratch::new();
        s.write(
            "tsconfig.json",
            r#"{ "files": [], "references": [{ "path": "packages" }] }"#,
        );
        s.write("packages/tsconfig.json", r#"{ "files": [], "references": [{ "path": "./lib" }, { "path": "app/tsconfig.build.json" }] }"#);
        s.write(
            "packages/lib/tsconfig.json",
            r#"{ "include": ["src"], "references": [{ "path": "../app/tsconfig.build.json" }] }"#,
        );
        s.write(
            "packages/app/tsconfig.build.json",
            r#"{ "include": ["src"], "references": [{ "path": "../lib" }] }"#,
        );
        // The two reference each other.
        let both = [
            "packages/lib/tsconfig.json",
            "packages/app/tsconfig.build.json",
        ];
        assert_eq!(
            owners(&s, "tsconfig.json", &["packages/lib/src/a.ts"]),
            both
        );
        assert_eq!(
            owners(&s, "tsconfig.json", &["packages/app/src/a.ts"]),
            both
        );
        assert!(owners(&s, "tsconfig.json", &["packages/other/a.ts"]).is_empty());
    }

    #[test]
    fn a_root_with_its_own_files_answers_when_no_reference_does() {
        let s = Scratch::new();
        s.write(
            "tsconfig.json",
            r#"{ "include": ["src"], "references": [{ "path": "./tests" }] }"#,
        );
        s.write(
            "tests/tsconfig.json",
            r#"{ "include": ["."] , "compilerOptions": {} }"#,
        );
        assert_eq!(
            owners(&s, "tsconfig.json", &["src/a.ts"]),
            ["tsconfig.json"]
        );
        // The root lists files and references `tests`, so it is one of its users.
        assert_eq!(
            owners(&s, "tsconfig.json", &["tests/a.ts"]),
            ["tsconfig.json", "tests/tsconfig.json"]
        );
    }

    #[test]
    fn the_projects_that_reference_the_owner_are_checked_too() {
        let s = Scratch::new();
        s.write(
            "tsconfig.json",
            r#"{ "files": [], "references": [{ "path": "core" }, { "path": "web" }, { "path": "ui" }, { "path": "docs" }] }"#,
        );
        s.write("core/tsconfig.json", r#"{ "include": ["src"] }"#);
        s.write(
            "ui/tsconfig.json",
            r#"{ "include": ["src"], "references": [{ "path": "../core" }] }"#,
        );
        s.write(
            "web/tsconfig.json",
            r#"{ "include": ["src"], "references": [{ "path": "../ui/tsconfig.json" }] }"#,
        );
        s.write("docs/tsconfig.json", r#"{ "include": ["src"] }"#);
        // `web` reaches `core` through `ui`; `docs` reaches nothing.
        assert_eq!(
            owners(&s, "tsconfig.json", &["core/src/a.ts"]),
            [
                "core/tsconfig.json",
                "web/tsconfig.json",
                "ui/tsconfig.json"
            ]
        );
        assert_eq!(
            owners(&s, "tsconfig.json", &["ui/src/a.ts"]),
            ["web/tsconfig.json", "ui/tsconfig.json"]
        );
        assert_eq!(
            owners(&s, "tsconfig.json", &["web/src/a.ts"]),
            ["web/tsconfig.json"]
        );
        assert_eq!(
            owners(&s, "tsconfig.json", &["docs/src/a.ts"]),
            ["docs/tsconfig.json"]
        );
    }

    #[test]
    fn unreadable_configs_own_nothing() {
        let s = Scratch::new();
        s.write(
            "tsconfig.json",
            r#"{ "files": [], "references": [{ "path": "bad" }, { "path": "missing" }] }"#,
        );
        s.write("bad/tsconfig.json", "not json");
        assert!(owners(&s, "tsconfig.json", &["bad/a.ts"]).is_empty());
        assert!(projects(&s.path("nothing.json"), None).is_empty());
    }

    #[test]
    fn a_reference_cycle_ends() {
        let s = Scratch::new();
        s.write(
            "a/tsconfig.json",
            r#"{ "include": ["."], "references": [{ "path": "../b" }] }"#,
        );
        s.write(
            "b/tsconfig.json",
            r#"{ "include": ["."], "references": [{ "path": "../a" }] }"#,
        );
        // Each references the other, so an edit in one is checked in both.
        assert_eq!(
            owners(&s, "a/tsconfig.json", &["a/x.ts"]),
            ["a/tsconfig.json", "b/tsconfig.json"]
        );
        assert_eq!(projects(&s.path("a/tsconfig.json"), None).len(), 2);
    }

    #[test]
    fn a_found_project_says_whether_it_references_and_sets_type_roots() {
        let s = Scratch::new();
        s.write(
            "tsconfig.json",
            r#"{ "files": [], "references": [{ "path": "app" }, { "path": "lib" }] }"#,
        );
        s.write("app/tsconfig.json", r#"{ "include": ["."], "compilerOptions": { "typeRoots": [] }, "references": [{ "path": "../lib" }] }"#);
        s.write("lib/tsconfig.json", r#"{ "include": ["."] }"#);
        let all = projects(&s.path("tsconfig.json"), None);
        let by = |name: &str| {
            all.iter()
                .find(|f| slashed(&f.file.to_string_lossy()).ends_with(name))
                .unwrap()
        };
        assert!(by("app/tsconfig.json").referencing);
        assert!(by("app/tsconfig.json").type_roots);
        assert!(!by("lib/tsconfig.json").referencing);
        assert!(!by("lib/tsconfig.json").type_roots);
    }

    #[test]
    fn a_config_lists_files_only_when_it_has_a_file_set() {
        let c = |text: &str| {
            let s = Scratch::new();
            s.write("tsconfig.json", text);
            Config::read(&s.path("tsconfig.json"))
                .unwrap()
                .has_file_set()
        };
        assert!(c("{}"));
        assert!(c(r#"{"include":["src"]}"#));
        assert!(c(r#"{"files":["a.ts"]}"#));
        assert!(c(r#"{"files":[],"include":["src"]}"#));
        assert!(!c(r#"{"files":[]}"#));
        assert!(!c(r#"{"files":[],"include":[]}"#));
        assert!(!c(r#"{"include":[]}"#));
    }

    #[test]
    fn within_a_directory() {
        assert!(is_within(
            Path::new("/w/app"),
            Path::new("/w/app/tsconfig.json")
        ));
        assert!(is_within(Path::new("/w/app"), Path::new("/w/app")));
        assert!(!is_within(
            Path::new("/w/app"),
            Path::new("/w/app2/tsconfig.json")
        ));
        assert!(is_within(
            Path::new("C:\\w\\app"),
            Path::new("C:\\w\\app\\tsconfig.json")
        ));
    }

    #[test]
    fn windows_spellings_match_the_same_way() {
        // Entries written with backslashes, in a config whose directory is a drive path.
        let config = Config {
            file: "C:/p/tsconfig.json".into(),
            dir: "C:/p".into(),
            settings: Settings {
                include: Some(Located {
                    dir: "C:/p".into(),
                    items: vec!["src\\**\\*".into()],
                }),
                ..Settings::default()
            },
            references: vec![],
        };
        assert!(config.owns("C:/p/src/a/b.ts"));
        assert!(!config.owns("C:/p/lib/b.ts"));
    }
}
