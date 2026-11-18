//! The `tsc` typecheck commands.
//!
//! [`tools`] builds the commands at detection time, from the package alone: `tsc --noEmit -p
//! <tsconfig>` for the package, and one per workspace dependent (or one for a workspace
//! solution config that references them). [`specialize`] refines them once the edited files are
//! known, which only matters for a tsconfig with project `references`.
//!
//! Every command is read-only for the project:
//!
//! - `--noEmit` writes no JavaScript or declarations;
//! - `--composite false` drops what composite projects force (declarations, `*.tsbuildinfo` next
//!   to the outputs, the rule that every file must be listed). Without it `tsc --noEmit` on a
//!   composite project still writes `tsconfig.tsbuildinfo` into the project, and `--incremental
//!   false` is rejected (`TS6379`);
//! - the incremental state goes to `--tsBuildInfoFile <cache>/...`, or `--incremental false` when
//!   there is no cache directory.
//!
//! `tsc -b` is never used: it writes the declared outputs of every referenced project, and it
//! rejects `--noEmit` (`TS5094` before TypeScript 5.6, `TS6310` since) while still writing
//! `*.tsbuildinfo`. A plain `-p` run of a project that references a composite project reports
//! `TS6305` for each import of it until that project has been built, and types the import as
//! `any`. So a project with references is checked through an *overlay* config in the cache
//! directory: it extends the project's config (whose `include` and other relative paths keep
//! resolving against the original), drops the `references` (which `extends` does not inherit),
//! and turns `composite` off. The imports of the referenced projects then resolve to their
//! sources and are checked in full. Two options that default to the config's location are set
//! explicitly, since the overlay lives elsewhere: `rootDir` (TypeScript 6 and 7 default it to the
//! config directory and fail with `TS6059` for files outside it) and `typeRoots` (the
//! `node_modules/@types` directories above the project, unless the project sets its own). When
//! the overlay cannot be written, the plain run stands and the parser drops `TS6305` (see
//! `parse::graph`).
//!
//! A solution config checks nothing itself, so with references the command runs for the projects
//! that own the edited files (see [`super::tsconfig::owner`]), not for every referenced project:
//! an edit under `src/` has no use for the errors of `tsconfig.node.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hookah_domain::path::normalize;
use hookah_domain::toolchain::{Arg, CheckKind, CommandSpec, Tool};

use super::package::PackageInfo;
use super::tsconfig::glob::{ancestors, below, resolve, slashed};
use super::tsconfig::owner::{self, Found};
use super::tsconfig::{self};
use super::{Installed, TSBUILDINFO, TSC, TSC_TIMEOUT, TSCONFIG, lit, tool};

/// Most projects one specialized run checks; the rest are left out.
const MAX_PROJECTS: usize = 8;

/// The typecheck commands of an edit in the package: the package itself plus the workspace
/// packages that depend on it, since an API change breaks them, not the package.
///
/// A workspace root `tsconfig.json` with `references` to the package or one of its dependents is
/// a solution config, checked once from the workspace root instead. Otherwise the package and
/// each dependent with a `tsconfig.json` run on their own, from their directory.
pub(super) fn tools(
    info: &PackageInfo,
    bin: &Installed,
    cache: &impl Fn() -> Option<PathBuf>,
) -> Vec<Tool> {
    if solution_references(info) {
        return vec![tool(
            TSC,
            CheckKind::Typecheck,
            bin,
            args(TSCONFIG, None),
            &info.workspace_root,
            TSC_TIMEOUT,
        )];
    }
    let own = has_tsconfig(&info.root).then(|| {
        let state = cache().map(|dir| dir.join(TSBUILDINFO));
        (info.root.as_path(), args(TSCONFIG, state.as_deref()))
    });
    let dependents = info
        .dependents
        .iter()
        .filter(|d| has_tsconfig(&d.root))
        .map(|d| (d.root.as_path(), args(TSCONFIG, None)));
    own.into_iter()
        .chain(dependents)
        .map(|(dir, args)| tool(TSC, CheckKind::Typecheck, bin, args, dir, TSC_TIMEOUT))
        .collect()
}

fn has_tsconfig(dir: &Path) -> bool {
    dir.join(TSCONFIG).is_file()
}

/// Whether the workspace root `tsconfig.json` references the package or one of its dependents.
fn solution_references(info: &PackageInfo) -> bool {
    if info.dependents.is_empty() || info.workspace_root == info.root {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(info.workspace_root.join(TSCONFIG)) else {
        return false;
    };
    let referenced: Vec<PathBuf> = tsconfig::reference_paths(&text)
        .iter()
        .map(|r| {
            let path = normalize(&info.workspace_root.join(r));
            if path.extension().is_some_and(|e| e == "json") {
                path.parent().map(Path::to_path_buf).unwrap_or(path)
            } else {
                path
            }
        })
        .collect();
    std::iter::once(&info.root)
        .chain(info.dependents.iter().map(|d| &d.root))
        .any(|dir| referenced.contains(dir))
}

/// The arguments that check `config` without writing anything into the project.
fn args(config: &str, state: Option<&Path>) -> Vec<Arg> {
    let mut args: Vec<Arg> = ["--noEmit", "--pretty", "false", "-p"]
        .iter()
        .map(|a| lit(a))
        .chain([lit(config), lit("--composite"), lit("false")])
        .collect();
    match state {
        Some(file) => args.extend([
            lit("--incremental"),
            lit("--tsBuildInfoFile"),
            lit(&file.to_string_lossy()),
        ]),
        None => args.extend([lit("--incremental"), lit("false")]),
    }
    args
}

/// The commands that check `targets` (paths relative to `project`, or `.` for everything) in
/// place of the detection-time `tool`.
///
/// A tool of another kind, and a tsconfig without references, stay as they are: the
/// detection-time command already checks the whole program. With references, the result is one
/// command per project that owns a target (empty when none does), plus, for a dependent or a
/// workspace solution config, the projects of the dependents, which an edit in the package can
/// break. `cache` yields the directory for the incremental state of each project.
pub fn specialize(
    project: &Path,
    tool: &Tool,
    targets: &[String],
    cache: impl Fn() -> Option<PathBuf>,
) -> Vec<Tool> {
    if tool.name != TSC || tool.kind != CheckKind::Typecheck {
        return vec![tool.clone()];
    }
    let dir = tool.command.cwd.as_path();
    let root = dir.join(TSCONFIG);
    let referenced = std::fs::read_to_string(&root)
        .is_ok_and(|text| !tsconfig::reference_paths(&text).is_empty());
    if !referenced {
        return vec![tool.clone()];
    }
    let files = (!targets.is_empty() && !targets.iter().any(|t| t == ".")).then(|| {
        targets
            .iter()
            .map(|t| slashed(&normalize(&project.join(t)).to_string_lossy()))
            .collect::<Vec<_>>()
    });
    let mut found: BTreeMap<PathBuf, Found> = BTreeMap::new();
    let mut add = |projects: Vec<Found>, keep: &dyn Fn(&Found) -> bool| {
        for p in projects.into_iter().filter(|p| keep(p)) {
            found.insert(p.file.clone(), p);
        }
    };
    if dir == project {
        add(owner::projects(&root, files.as_deref()), &|_| true);
    } else {
        let info = PackageInfo::read(project).ok();
        let solution = info.as_ref().is_some_and(|i| i.workspace_root == dir);
        if solution {
            add(owner::projects(&root, files.as_deref()), &|_| true);
        }
        let dependents: Vec<&Path> = info
            .iter()
            .flat_map(|i| i.dependents.iter().map(|d| d.root.as_path()))
            .collect();
        add(owner::projects(&root, None), &|c| {
            !solution || dependents.iter().any(|d| owner::is_within(d, &c.file))
        });
    }
    let cache_dir = cache();
    found
        .into_values()
        .take(MAX_PROJECTS)
        .map(|config| {
            let full = slashed(&config.file.to_string_lossy());
            let name = state_name(project, &full);
            let state = cache_dir.as_deref().map(|d| d.join(&name));
            let overlay = state
                .as_deref()
                .filter(|_| config.referencing)
                .map(|file| file.with_extension("overlay.json"))
                .filter(|file| write_overlay(&full, config.type_roots, file));
            let relative = below(&slashed(&dir.to_string_lossy()), &full);
            let target = match &overlay {
                Some(file) => slashed(&file.to_string_lossy()),
                None => relative.unwrap_or(&full).to_string(),
            };
            let command = CommandSpec::new(
                tool.command.program.clone(),
                args(&target, state.as_deref()),
                dir,
                tool.command.timeout,
            )
            .with_env(tool.command.env.clone());
            Tool {
                name: tool.name.clone(),
                kind: tool.kind,
                command,
            }
        })
        .collect()
}

/// Writes the overlay of the project config `config` (slashed, absolute) to `file`, in the cache
/// directory. Returns whether the file is in place; the plain config is checked otherwise.
fn write_overlay(config: &str, type_roots: bool, file: &Path) -> bool {
    let mut options = serde_json::json!({ "composite": false, "rootDir": filesystem_root(config) });
    if !type_roots {
        options["typeRoots"] = type_roots_above(config).into();
    }
    let text = serde_json::json!({
        "extends": config,
        "references": [],
        "compilerOptions": options,
    })
    .to_string();
    if std::fs::read_to_string(file).is_ok_and(|old| old == text) {
        return true;
    }
    // Written under another name first, so that a concurrent run never reads half of it.
    let partial = file.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&partial, text).is_ok() && std::fs::rename(&partial, file).is_ok()
}

/// `/`, or the drive (`C:/`) of the absolute `path`: a root that contains every file of it.
fn filesystem_root(path: &str) -> String {
    match path.strip_prefix('/') {
        Some(_) => "/".to_string(),
        None => format!("{}/", path.get(..2).unwrap_or("")),
    }
}

/// The `node_modules/@types` directories that exist in the directory of `config` and above, which
/// is where `tsc` looks by default.
fn type_roots_above(config: &str) -> Vec<String> {
    ancestors(&resolve(config, ".."))
        .into_iter()
        .map(|dir| format!("{}/node_modules/@types", dir.trim_end_matches('/')))
        .filter(|candidate| Path::new(candidate).is_dir())
        .collect()
}

/// The incremental state file of a tsconfig: `tsc-<path>.tsbuildinfo`, the path relative to the
/// project when it is inside, with every character outside `[A-Za-z0-9.-]` as `_`.
fn state_name(project: &Path, config: &str) -> String {
    let project = slashed(&project.to_string_lossy());
    let path = below(&project, config).unwrap_or(config);
    let flat: String = path
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("tsc-{flat}.tsbuildinfo")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::time::Duration;

    use super::super::package::scratch::Scratch;
    use super::*;

    fn template(dir: &Path) -> Tool {
        Tool {
            name: TSC.into(),
            kind: CheckKind::Typecheck,
            command: CommandSpec::new("tsc", args(TSCONFIG, None), dir, Duration::from_secs(7))
                .with_env(vec![("K".into(), "V".into())]),
        }
    }

    fn cache() -> Option<PathBuf> {
        Some("/cache".into())
    }

    fn files(targets: &[&str], s: &Scratch, tool: &Tool) -> Vec<Vec<String>> {
        let targets: Vec<String> = targets.iter().map(|t| t.to_string()).collect();
        specialize(&s.0, tool, &targets, cache)
            .iter()
            .map(|t| t.command.expand_args(&[]))
            .collect()
    }

    fn vite() -> Scratch {
        let s = Scratch::new();
        s.write(
            "tsconfig.json",
            r#"{ "files": [], "references": [{ "path": "./tsconfig.app.json" }, { "path": "./tsconfig.node.json" }] }"#,
        );
        s.write(
            "tsconfig.app.json",
            r#"{ "compilerOptions": { "composite": true }, "include": ["src"] }"#,
        );
        s.write(
            "tsconfig.node.json",
            r#"{ "compilerOptions": { "composite": true }, "include": ["vite.config.ts"] }"#,
        );
        s
    }

    fn command(config: &str, state: &str) -> Vec<String> {
        [
            "--noEmit",
            "--pretty",
            "false",
            "-p",
            config,
            "--composite",
            "false",
            "--incremental",
            "--tsBuildInfoFile",
            state,
        ]
        .iter()
        .map(|a| a.to_string())
        .collect()
    }

    fn cached(name: &str) -> String {
        Path::new("/cache")
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn commands_never_write_into_the_project() {
        let flat = |state: Option<&Path>| {
            let spec = CommandSpec::new("tsc", args("a.json", state), "/r", Duration::from_secs(1));
            spec.expand_args(&[])
        };
        assert_eq!(
            flat(None),
            [
                "--noEmit",
                "--pretty",
                "false",
                "-p",
                "a.json",
                "--composite",
                "false",
                "--incremental",
                "false"
            ]
        );
        assert_eq!(flat(Some(Path::new("/c/x"))), command("a.json", "/c/x"));
    }

    #[test]
    fn the_app_edit_checks_only_the_app_project() {
        let s = vite();
        let t = template(&s.0);
        assert_eq!(
            files(&["src/a.ts"], &s, &t),
            [command(
                "tsconfig.app.json",
                &cached("tsc-tsconfig.app.json.tsbuildinfo")
            )]
        );
        assert_eq!(
            files(&["vite.config.ts"], &s, &t),
            [command(
                "tsconfig.node.json",
                &cached("tsc-tsconfig.node.json.tsbuildinfo")
            )]
        );
        assert_eq!(files(&["src/a.ts", "vite.config.ts"], &s, &t).len(), 2);
        // Nothing owns it: nothing runs.
        assert!(files(&["scripts/a.ts"], &s, &t).is_empty());
        assert!(files(&["README.md"], &s, &t).is_empty());
        // The whole project: every project.
        assert_eq!(files(&["."], &s, &t).len(), 2);
        assert_eq!(files(&[], &s, &t).len(), 2);
    }

    #[test]
    fn the_command_keeps_the_program_directory_timeout_and_environment() {
        let s = vite();
        let t = template(&s.0);
        let out = specialize(&s.0, &t, &["src/a.ts".into()], cache);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, TSC);
        assert_eq!(out[0].kind, CheckKind::Typecheck);
        assert_eq!(out[0].command.program, "tsc");
        assert_eq!(out[0].command.cwd, s.0);
        assert_eq!(out[0].command.timeout, Duration::from_secs(7));
        assert_eq!(out[0].command.env, [("K".to_string(), "V".to_string())]);
        // Without a cache directory there is no state file at all.
        let out = specialize(&s.0, &t, &["src/a.ts".into()], || None);
        assert_eq!(
            out[0].command.expand_args(&[]).last().map(String::as_str),
            Some("false")
        );
    }

    #[test]
    fn other_tools_and_plain_configs_stay_as_they_are() {
        let s = Scratch::new();
        s.write("tsconfig.json", r#"{ "include": ["src"] }"#);
        let t = template(&s.0);
        assert_eq!(
            specialize(&s.0, &t, &["lib/a.ts".into()], cache),
            std::slice::from_ref(&t)
        );
        let mut other = t.clone();
        other.name = "eslint".into();
        s.write("tsconfig.json", r#"{ "references": [{ "path": "a" }] }"#);
        assert_eq!(
            specialize(&s.0, &other, &["a.ts".into()], cache),
            [other.clone()]
        );
        let mut other = t.clone();
        other.kind = CheckKind::Lint;
        assert_eq!(
            specialize(&s.0, &other, &["a.ts".into()], cache),
            [other.clone()]
        );
        // No tsconfig, no references.
        std::fs::remove_file(s.path("tsconfig.json")).unwrap();
        assert_eq!(
            specialize(&s.0, &t, &["a.ts".into()], cache),
            std::slice::from_ref(&t)
        );
    }

    #[test]
    fn a_project_outside_the_directory_is_named_by_its_absolute_path() {
        let s = Scratch::new();
        s.write(
            "app/tsconfig.json",
            r#"{ "references": [{ "path": "../shared" }] , "files": [] }"#,
        );
        s.write("shared/tsconfig.json", r#"{ "include": ["."] }"#);
        let t = template(&s.path("app"));
        let out = specialize(&s.0, &t, &["shared/a.ts".into()], cache);
        assert_eq!(out.len(), 1);
        let argv = out[0].command.expand_args(&[]);
        let config = argv[argv.iter().position(|a| a == "-p").unwrap() + 1].clone();
        assert!(Path::new(&config).is_absolute(), "{config}");
        assert!(config.ends_with("tsconfig.json"));
    }

    fn p_arg(argv: &[String]) -> String {
        argv[argv.iter().position(|a| a == "-p").unwrap() + 1].clone()
    }

    /// A project whose app config references a library config, and a cache directory.
    fn referencing() -> (Scratch, Scratch) {
        let s = Scratch::new();
        s.write(
            "tsconfig.json",
            r#"{ "files": [], "references": [{ "path": "app" }, { "path": "lib" }] }"#,
        );
        s.write(
            "app/tsconfig.json",
            r#"{ "include": ["src"], "references": [{ "path": "../lib" }] }"#,
        );
        s.write("lib/tsconfig.json", r#"{ "include": ["src"] }"#);
        s.write("node_modules/@types/node/index.d.ts", "");
        s.write("app/node_modules/@types/dom/index.d.ts", "");
        (s, Scratch::new())
    }

    #[test]
    fn a_project_with_references_is_checked_through_an_overlay_in_the_cache() {
        let (s, c) = referencing();
        let t = template(&s.0);
        let out = specialize(&s.0, &t, &["app/src/a.ts".into()], || Some(c.0.clone()));
        assert_eq!(out.len(), 1);
        let argv = out[0].command.expand_args(&[]);
        let overlay = c.path("tsc-app_tsconfig.json.overlay.json");
        assert_eq!(p_arg(&argv), slashed(&overlay.to_string_lossy()));
        assert_eq!(
            argv.last().unwrap(),
            &c.path("tsc-app_tsconfig.json.tsbuildinfo")
                .to_string_lossy()
        );
        let text: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&overlay).unwrap()).unwrap();
        let original = slashed(&s.path("app/tsconfig.json").to_string_lossy());
        assert_eq!(text["extends"], original);
        assert_eq!(text["references"], serde_json::json!([]));
        assert_eq!(text["compilerOptions"]["composite"], false);
        assert_eq!(
            text["compilerOptions"]["rootDir"],
            filesystem_root(&original)
        );
        // The @types directories above the project, nearest first.
        let roots: Vec<String> = text["compilerOptions"]["typeRoots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r.as_str().unwrap().to_string())
            .collect();
        let base = slashed(&s.0.to_string_lossy());
        assert_eq!(
            roots,
            [
                format!("{base}/app/node_modules/@types"),
                format!("{base}/node_modules/@types")
            ]
        );
        // An edit in the library is checked there as it is, and in the app that references it.
        let out = specialize(&s.0, &t, &["lib/src/a.ts".into()], || Some(c.0.clone()));
        assert_eq!(out.len(), 2);
        assert_eq!(
            p_arg(&out[0].command.expand_args(&[])),
            slashed(&overlay.to_string_lossy())
        );
        assert_eq!(p_arg(&out[1].command.expand_args(&[])), "lib/tsconfig.json");
        assert!(!c.path("tsc-lib_tsconfig.json.overlay.json").exists());
    }

    #[test]
    fn the_overlay_keeps_the_typeroots_of_the_project() {
        let (s, c) = referencing();
        s.write(
            "app/tsconfig.json",
            r#"{ "include": ["src"], "compilerOptions": { "typeRoots": ["./types"] }, "references": [{ "path": "../lib" }] }"#,
        );
        specialize(&s.0, &template(&s.0), &["app/src/a.ts".into()], || {
            Some(c.0.clone())
        });
        let text = std::fs::read_to_string(c.path("tsc-app_tsconfig.json.overlay.json")).unwrap();
        assert!(!text.contains("typeRoots"), "{text}");
        // Inherited from a base counts too.
        s.write("base.json", r#"{ "compilerOptions": { "typeRoots": [] } }"#);
        s.write(
            "app/tsconfig.json",
            r#"{ "extends": "../base.json", "include": ["src"], "references": [{ "path": "../lib" }] }"#,
        );
        specialize(&s.0, &template(&s.0), &["app/src/a.ts".into()], || {
            Some(c.0.clone())
        });
        let text = std::fs::read_to_string(c.path("tsc-app_tsconfig.json.overlay.json")).unwrap();
        assert!(!text.contains("typeRoots"), "{text}");
    }

    #[test]
    fn the_overlay_is_rewritten_only_when_it_changes_and_leaves_no_partial_file() {
        let (s, c) = referencing();
        let file = c.path("o.json");
        let config = slashed(&s.path("app/tsconfig.json").to_string_lossy());
        assert!(write_overlay(&config, false, &file));
        let first = std::fs::read_to_string(&file).unwrap();
        assert!(write_overlay(&config, false, &file));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), first);
        assert!(write_overlay(&config, true, &file));
        assert_ne!(std::fs::read_to_string(&file).unwrap(), first);
        let names: Vec<_> = std::fs::read_dir(&c.0).unwrap().collect();
        assert_eq!(names.len(), 1, "{names:?}");
        // An unwritable place falls back to the plain config, which is still checked.
        assert!(!write_overlay(
            &config,
            false,
            &c.path("missing/dir/o.json")
        ));
        let t = template(&s.0);
        let out = specialize(&s.0, &t, &["app/src/a.ts".into()], || Some(c.path("gone")));
        assert_eq!(p_arg(&out[0].command.expand_args(&[])), "app/tsconfig.json");
        let out = specialize(&s.0, &t, &["app/src/a.ts".into()], || None);
        assert_eq!(p_arg(&out[0].command.expand_args(&[])), "app/tsconfig.json");
    }

    #[test]
    fn filesystem_roots_and_type_root_walks() {
        assert_eq!(filesystem_root("/w/app/tsconfig.json"), "/");
        assert_eq!(filesystem_root("C:/w/app/tsconfig.json"), "C:/");
        assert!(type_roots_above("/definitely/not/here/tsconfig.json").is_empty());
    }

    #[test]
    fn state_files_are_named_by_project_path() {
        let base = "/w/app";
        assert_eq!(
            state_name(Path::new(base), "/w/app/tsconfig.app.json"),
            "tsc-tsconfig.app.json.tsbuildinfo"
        );
        assert_eq!(
            state_name(Path::new(base), "/w/app/a/b/tsconfig.json"),
            "tsc-a_b_tsconfig.json.tsbuildinfo"
        );
        assert_eq!(
            state_name(Path::new(base), "/w/lib/tsconfig.json"),
            "tsc-_w_lib_tsconfig.json.tsbuildinfo"
        );
        assert_eq!(
            state_name(Path::new("C:\\w"), "C:/w/a b/t.json"),
            "tsc-a_b_t.json.tsbuildinfo"
        );
    }

    #[test]
    fn at_most_eight_projects_run() {
        let s = Scratch::new();
        let refs: Vec<String> = (0..12).map(|i| format!(r#"{{"path":"p{i}"}}"#)).collect();
        s.write(
            "tsconfig.json",
            &format!(r#"{{"files":[],"references":[{}]}}"#, refs.join(",")),
        );
        for i in 0..12 {
            s.write(&format!("p{i}/tsconfig.json"), r#"{"include":["."]}"#);
        }
        assert_eq!(files(&["."], &s, &template(&s.0)).len(), MAX_PROJECTS);
    }

    /// A pnpm workspace: `packages/lib`, `packages/app` (depends on lib), and a solution config.
    fn workspace() -> Scratch {
        let s = Scratch::new();
        s.write("pnpm-workspace.yaml", "packages:\n  - packages/*\n");
        s.write("package.json", "{}");
        s.write("packages/lib/package.json", r#"{"name":"lib"}"#);
        s.write(
            "packages/app/package.json",
            r#"{"name":"app","dependencies":{"lib":"workspace:*"}}"#,
        );
        s.write("packages/cli/package.json", r#"{"name":"cli"}"#);
        s.write("packages/lib/tsconfig.json", r#"{"include":["src"]}"#);
        s.write("packages/app/tsconfig.json", r#"{"include":["src"]}"#);
        s.write("packages/cli/tsconfig.json", r#"{"include":["src"]}"#);
        s.write(
            "tsconfig.json",
            r#"{"files":[],"references":[{"path":"packages/lib"},{"path":"packages/app"},{"path":"packages/cli"}]}"#,
        );
        s
    }

    #[test]
    fn a_workspace_solution_checks_the_owner_and_the_dependents() {
        let s = workspace();
        let lib = s.path("packages/lib");
        let t = template(&s.0);
        let out = specialize(&lib, &t, &["src/a.ts".into()], cache);
        let configs: Vec<String> = out
            .iter()
            .map(|t| {
                let argv = t.command.expand_args(&[]);
                argv[argv.iter().position(|a| a == "-p").unwrap() + 1].clone()
            })
            .collect();
        // The lib project owns the file, the app project depends on it, cli is unrelated.
        assert_eq!(
            configs,
            ["packages/app/tsconfig.json", "packages/lib/tsconfig.json"]
        );
        assert!(out.iter().all(|t| t.command.cwd == s.0));
        // An edit outside every project still checks the dependents.
        let out = specialize(&lib, &t, &["notes.ts".into()], cache);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn a_dependent_with_references_checks_all_its_projects() {
        let s = workspace();
        std::fs::remove_file(s.path("tsconfig.json")).unwrap();
        s.write(
            "packages/app/tsconfig.json",
            r#"{"files":[],"references":[{"path":"./tsconfig.src.json"},{"path":"./tsconfig.node.json"}]}"#,
        );
        s.write("packages/app/tsconfig.src.json", r#"{"include":["src"]}"#);
        s.write(
            "packages/app/tsconfig.node.json",
            r#"{"include":["vite.config.ts"]}"#,
        );
        let t = template(&s.path("packages/app"));
        let out = specialize(&s.path("packages/lib"), &t, &["src/a.ts".into()], cache);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|t| t.command.cwd == s.path("packages/app")));
    }

    fn info_tools(s: &Scratch, dir: &str, cache: impl Fn() -> Option<PathBuf>) -> Vec<Tool> {
        let info = PackageInfo::read(&s.path(dir)).unwrap();
        let bin = Installed {
            bin: "tsc".into(),
            version: None,
        };
        tools(&info, &bin, &cache)
    }

    #[test]
    fn detection_time_commands() {
        let s = workspace();
        std::fs::remove_file(s.path("tsconfig.json")).unwrap();
        let out = info_tools(&s, "packages/lib", cache);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].command.cwd, s.path("packages/lib"));
        assert_eq!(
            out[0].command.expand_args(&[]),
            command("tsconfig.json", &cached("tsc.tsbuildinfo"))
        );
        assert_eq!(out[1].command.cwd, s.path("packages/app"));
        assert_eq!(
            out[1].command.expand_args(&[]).last().map(String::as_str),
            Some("false"),
            "dependents keep no state"
        );
        // A package without a tsconfig has none of its own.
        std::fs::remove_file(s.path("packages/lib/tsconfig.json")).unwrap();
        assert_eq!(info_tools(&s, "packages/lib", cache).len(), 1);
    }
}
