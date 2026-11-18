//! The detection cache.
//!
//! Every hook call is a fresh process, and detection (reading manifests and configs, resolving
//! binaries, running `cargo metadata` and `cargo mutants --version`) is repeated work when nothing
//! changed. This module keeps the results between calls, in small JSON files under the project
//! cache directory (see [`project_cache_dir`](crate::store::cache_dir::project_cache_dir)):
//!
//! - `detect-<ecosystem>.json`: the detected [`Toolchain`], keyed by project root and ecosystem;
//! - `metadata.json`: the `cargo metadata` result.
//!
//! Only Rust uses it: `cargo metadata` and `cargo mutants --version` cost about 40 ms, while Node
//! and Python detection costs about as much as starting the process.
//!
//! An entry is valid while the fingerprint of its inputs is unchanged (see [`fingerprint`]). The
//! ecosystems list the inputs: configuration files, lockfiles, the directories whose contents
//! decide which tools exist. A few inputs can only be named by the computation itself, such as
//! the other members of a workspace; those are stored in the entry as *derived* inputs. The
//! fingerprint also covers hookah's version and the environment variables that steer tool
//! resolution (`PATH` first of all), so a new `cargo-mutants` on `PATH` is noticed.
//!
//! The cache never gets in the way: a missing, corrupt, or unwritable cache is a miss, and
//! `HOOKAH_NO_CACHE=1` switches it off.

pub mod codec;
pub mod file;
pub mod fingerprint;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};

use hookah_application::error::PortError;
use hookah_domain::project::Project;
use hookah_domain::toolchain::Toolchain;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::store::cache_dir::project_cache_dir;
use codec::ToolchainDto;
use file::{Entry, SCHEMA};
pub use fingerprint::Input;
use fingerprint::fingerprint;

/// The environment variable that disables the cache.
pub const NO_CACHE_VAR: &str = "HOOKAH_NO_CACHE";

/// The environment variables whose values steer which tools detection finds. Their values go
/// into every fingerprint.
const SALT_VARS: [&str; 3] = ["PATH", "RUSTUP_TOOLCHAIN", "CARGO_HOME"];

/// Where a result came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Read from a valid cache entry.
    Hit,
    /// Computed, and stored when the cache directory allowed it.
    Miss,
    /// Computed with the cache switched off or unavailable.
    Off,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Hit => "hit",
            Self::Miss => "miss",
            Self::Off => "off",
        })
    }
}

#[derive(Debug, Clone)]
enum DirSource {
    /// [`project_cache_dir`].
    Project,
    /// A given directory; for tests.
    Fixed(PathBuf),
}

/// How the cache behaves: on or off, the environment it fingerprints, and where it lives.
#[derive(Debug, Clone)]
pub struct CacheConfig {
    enabled: bool,
    vars: Vec<(&'static str, Option<OsString>)>,
    dir: DirSource,
}

impl CacheConfig {
    /// The configuration of the running process: the cache is on unless `HOOKAH_NO_CACHE` is set
    /// to something other than an empty string or `0`.
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var_os(key))
    }

    /// Like [`CacheConfig::from_env`], with the environment given by `lookup`.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<OsString>) -> Self {
        let disabled = lookup(NO_CACHE_VAR).is_some_and(|v| !v.is_empty() && v != "0");
        Self {
            enabled: !disabled,
            vars: SALT_VARS.iter().map(|k| (*k, lookup(k))).collect(),
            dir: DirSource::Project,
        }
    }

    /// A configuration with the cache off; the default of directly built ecosystems.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            vars: vec![],
            dir: DirSource::Project,
        }
    }

    /// Keeps every cache file in `dir` instead of the project cache directory.
    #[must_use]
    pub fn in_dir(mut self, dir: PathBuf) -> Self {
        self.dir = DirSource::Fixed(dir);
        self
    }

    /// Whether the cache is on.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The fingerprinted environment variable `name`, if it was set.
    pub fn var(&self, name: &str) -> Option<&OsStr> {
        self.vars
            .iter()
            .find(|(k, _)| *k == name)
            .and_then(|(_, v)| v.as_deref())
    }

    /// The directories of `PATH`.
    pub fn path_dirs(&self) -> Vec<PathBuf> {
        self.var("PATH")
            .map(|p| std::env::split_paths(p).collect())
            .unwrap_or_default()
    }

    /// What goes into every fingerprint besides the inputs: hookah's version and the variables.
    fn salt(&self) -> String {
        let mut salt = format!("hookah {}", env!("CARGO_PKG_VERSION"));
        for (key, value) in &self.vars {
            salt.push_str(&format!(
                "\0{key}={}",
                value.as_deref().unwrap_or_default().to_string_lossy()
            ));
        }
        salt
    }

    /// The directory for the cache files of the project at `root`, or `None` when the cache is
    /// off or has no directory.
    fn dir_for(&self, root: &Path) -> Option<PathBuf> {
        if !self.enabled {
            return None;
        }
        match &self.dir {
            DirSource::Project => project_cache_dir(root).ok(),
            DirSource::Fixed(dir) => Some(dir.clone()),
        }
    }
}

/// The value kept in the cache file `file` for the project at `root`, or the result of `compute`.
///
/// `fixed` are the inputs known up front. `compute` returns the value and the *derived* inputs it
/// learned on the way. A value is stored only when the fixed inputs did not change while it was
/// computed, so a file edited mid-run cannot be stamped as seen. Errors of `compute` pass through
/// and are never cached.
pub fn cached<T: Serialize + DeserializeOwned>(
    config: &CacheConfig,
    root: &Path,
    file: &str,
    fixed: &[Input],
    compute: impl FnOnce() -> Result<(T, Vec<Input>), PortError>,
) -> Result<(T, Origin), PortError> {
    let Some(dir) = config.dir_for(root) else {
        return Ok((compute()?.0, Origin::Off));
    };
    let path = dir.join(file);
    let key = root.to_string_lossy().into_owned();
    let salt = config.salt();
    if let Some(entry) = file::read::<T>(&path)
        && entry.schema == SCHEMA
        && entry.key == key
        && fingerprint(&[fixed, &entry.derived].concat(), &salt) == entry.fingerprint
    {
        return Ok((entry.value, Origin::Hit));
    }
    let before = fingerprint(fixed, &salt);
    let (value, derived) = compute()?;
    if fingerprint(fixed, &salt) == before {
        let fingerprint = fingerprint(&[fixed, &derived].concat(), &salt);
        file::write(
            &path,
            &Entry {
                schema: SCHEMA,
                key,
                fingerprint,
                derived,
                value: &value,
            },
        );
    }
    Ok((value, Origin::Miss))
}

/// The toolchain of `project`, from the cache when its inputs are unchanged, else from `run`.
pub fn detect_cached(
    config: &CacheConfig,
    project: &Project,
    fixed: &[Input],
    run: impl FnOnce() -> Result<(Toolchain, Vec<Input>), PortError>,
) -> Result<(Toolchain, Origin), PortError> {
    let file = format!("detect-{}.json", project.ecosystem.name());
    let (dto, origin) = cached::<ToolchainDto>(config, &project.root, &file, fixed, || {
        let (toolchain, derived) = run()?;
        Ok((ToolchainDto::from(&toolchain), derived))
    })?;
    Ok((dto.into(), origin))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::cell::Cell;
    use std::fs;

    use hookah_domain::project::Ecosystem;
    use hookah_domain::toolchain::{Arg, CheckKind, CommandSpec, MissingTool, Tool};

    use super::fingerprint::tests::{Dir, age};
    use super::*;

    fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| OsString::from(v))
        }
    }

    fn config(d: &Dir) -> CacheConfig {
        CacheConfig::from_lookup(lookup(&[("PATH", "/bin")])).in_dir(d.0.join("cache"))
    }

    fn toolchain(name: &str) -> Toolchain {
        let tool = Tool {
            name: name.into(),
            kind: CheckKind::Lint,
            command: CommandSpec::new(
                name,
                vec![Arg::Targets],
                "/root",
                std::time::Duration::from_secs(3),
            ),
        };
        Toolchain::new(vec![tool]).with_missing(vec![MissingTool::new("m", CheckKind::Test, "h")])
    }

    fn project(d: &Dir) -> Project {
        Project {
            root: d.0.join("project"),
            ecosystem: Ecosystem::Node,
            manifests: vec![],
        }
    }

    /// Runs `detect_cached` counting the computations; the computed toolchain is named `name`.
    fn run(
        cfg: &CacheConfig,
        p: &Project,
        fixed: &[Input],
        name: &str,
        runs: &Cell<u32>,
    ) -> (Toolchain, Origin) {
        detect_cached(cfg, p, fixed, || {
            runs.set(runs.get() + 1);
            Ok((toolchain(name), vec![]))
        })
        .unwrap()
    }

    fn setup(tag: &str) -> (Dir, CacheConfig, Project, PathBuf) {
        let d = Dir::new(tag);
        fs::create_dir_all(d.0.join("cache")).unwrap();
        let cfg = config(&d);
        let p = project(&d);
        let input = d.write("project/package.json", "{}");
        (d, cfg, p, input)
    }

    #[test]
    fn the_second_call_is_a_hit_with_the_same_toolchain() {
        let (_d, cfg, p, input) = setup("hit");
        let fixed = [Input::File(input)];
        let runs = Cell::new(0);
        let (first, origin) = run(&cfg, &p, &fixed, "one", &runs);
        assert_eq!(origin, Origin::Miss);
        let (second, origin) = run(&cfg, &p, &fixed, "two", &runs);
        assert_eq!(origin, Origin::Hit);
        assert_eq!(second, first);
        assert_eq!(runs.get(), 1);
    }

    #[test]
    fn a_changed_file_input_is_a_miss_and_the_result_is_replaced() {
        let (d, cfg, p, input) = setup("invalidate-file");
        let fixed = [Input::File(input.clone())];
        let runs = Cell::new(0);
        run(&cfg, &p, &fixed, "one", &runs);
        age(&input, 30);
        let (tc, origin) = run(&cfg, &p, &fixed, "two", &runs);
        assert_eq!((origin, tc), (Origin::Miss, toolchain("two")));
        let (tc, origin) = run(&cfg, &p, &fixed, "three", &runs);
        assert_eq!((origin, tc), (Origin::Hit, toolchain("two")));
        d.write("project/package.json", "{ \"a\": 1 }");
        assert_eq!(run(&cfg, &p, &fixed, "four", &runs).1, Origin::Miss);
    }

    #[test]
    fn directory_and_existence_inputs_invalidate() {
        let (d, cfg, p, _) = setup("invalidate-dirs");
        let fixed = [
            Input::Exists(d.0.join("project/marker")),
            Input::Dirs(d.0.join("project/pkgs")),
        ];
        let runs = Cell::new(0);
        run(&cfg, &p, &fixed, "x", &runs);
        assert_eq!(run(&cfg, &p, &fixed, "x", &runs).1, Origin::Hit);
        d.write("project/marker", "");
        assert_eq!(run(&cfg, &p, &fixed, "x", &runs).1, Origin::Miss);
        assert_eq!(run(&cfg, &p, &fixed, "x", &runs).1, Origin::Hit);
        fs::create_dir_all(d.0.join("project/pkgs/a")).unwrap();
        assert_eq!(run(&cfg, &p, &fixed, "x", &runs).1, Origin::Miss);
    }

    #[test]
    fn derived_inputs_are_checked_on_the_next_call() {
        let (d, cfg, p, input) = setup("derived");
        let member = d.write("project/member/Cargo.toml", "x");
        let fixed = [Input::File(input)];
        let runs = Cell::new(0);
        let go = |runs: &Cell<u32>| {
            detect_cached(&cfg, &p, &fixed, || {
                runs.set(runs.get() + 1);
                Ok((toolchain("t"), vec![Input::File(member.clone())]))
            })
            .unwrap()
            .1
        };
        assert_eq!(go(&runs), Origin::Miss);
        assert_eq!(go(&runs), Origin::Hit);
        age(&member, 30);
        assert_eq!(go(&runs), Origin::Miss);
        assert_eq!(go(&runs), Origin::Hit);
        assert_eq!(runs.get(), 2);
    }

    #[test]
    fn the_environment_and_the_version_are_part_of_the_fingerprint() {
        let (d, cfg, p, input) = setup("salt");
        let fixed = [Input::File(input)];
        let runs = Cell::new(0);
        run(&cfg, &p, &fixed, "x", &runs);
        let other_path =
            CacheConfig::from_lookup(lookup(&[("PATH", "/usr/bin")])).in_dir(d.0.join("cache"));
        assert_eq!(run(&other_path, &p, &fixed, "x", &runs).1, Origin::Miss);
        for var in ["RUSTUP_TOOLCHAIN", "CARGO_HOME"] {
            let pairs = [("PATH", "/usr/bin"), (var, "v")];
            let c = CacheConfig::from_lookup(lookup(&pairs)).in_dir(d.0.join("cache"));
            assert_eq!(run(&c, &p, &fixed, "x", &runs).1, Origin::Miss, "{var}");
        }
        assert!(cfg.salt().contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn a_stored_entry_of_another_version_schema_root_or_ecosystem_is_a_miss() {
        let (d, cfg, p, input) = setup("identity");
        let fixed = [Input::File(input)];
        let runs = Cell::new(0);
        run(&cfg, &p, &fixed, "x", &runs);
        let path = d.0.join("cache/detect-node.json");
        let text = fs::read_to_string(&path).unwrap();
        // Another schema.
        fs::write(&path, text.replacen("\"schema\":1", "\"schema\":99", 1)).unwrap();
        assert_eq!(run(&cfg, &p, &fixed, "x", &runs).1, Origin::Miss);
        // Another root under the same file name.
        let other = Project {
            root: d.0.join("elsewhere"),
            ..p.clone()
        };
        assert_eq!(run(&cfg, &other, &fixed, "x", &runs).1, Origin::Miss);
        // Another ecosystem has its own file.
        let python = Project {
            ecosystem: Ecosystem::Python,
            ..p.clone()
        };
        assert_eq!(run(&cfg, &python, &fixed, "x", &runs).1, Origin::Miss);
        assert!(d.0.join("cache/detect-python.json").is_file());
    }

    #[test]
    fn a_corrupt_file_is_recomputed_and_rewritten() {
        let (d, cfg, p, input) = setup("corrupt");
        let fixed = [Input::File(input)];
        let runs = Cell::new(0);
        d.write("cache/detect-node.json", "{ truncated");
        assert_eq!(run(&cfg, &p, &fixed, "x", &runs).1, Origin::Miss);
        assert_eq!(run(&cfg, &p, &fixed, "x", &runs).1, Origin::Hit);
    }

    #[test]
    fn an_unwritable_cache_directory_still_gives_the_result() {
        let (d, _, p, input) = setup("unwritable");
        let fixed = [Input::File(input)];
        let runs = Cell::new(0);
        // The cache directory is a file, so nothing can be stored in it.
        let file = d.write("blocker", "x");
        let cfg = CacheConfig::from_lookup(lookup(&[])).in_dir(file);
        for _ in 0..2 {
            let (tc, origin) = run(&cfg, &p, &fixed, "x", &runs);
            assert_eq!((tc, origin), (toolchain("x"), Origin::Miss));
        }
        assert_eq!(runs.get(), 2);
        // A directory that does not exist behaves the same.
        let cfg = CacheConfig::from_lookup(lookup(&[])).in_dir(d.0.join("no/such/dir"));
        assert_eq!(run(&cfg, &p, &fixed, "x", &runs).1, Origin::Miss);
    }

    #[test]
    fn the_switch_turns_the_cache_off() {
        let (d, _, p, input) = setup("switch");
        let fixed = [Input::File(input)];
        let runs = Cell::new(0);
        for (value, off) in [("1", true), ("true", true), ("0", false), ("", false)] {
            let cfg = CacheConfig::from_lookup(lookup(&[(NO_CACHE_VAR, value)]))
                .in_dir(d.0.join(format!("c{}", value.len())));
            assert_eq!(cfg.enabled(), !off, "{value:?}");
        }
        let cfg =
            CacheConfig::from_lookup(lookup(&[(NO_CACHE_VAR, "1")])).in_dir(d.0.join("cache"));
        for _ in 0..2 {
            assert_eq!(run(&cfg, &p, &fixed, "x", &runs).1, Origin::Off);
        }
        assert_eq!(runs.get(), 2);
        assert!(!d.0.join("cache/detect-node.json").exists());
        assert!(CacheConfig::from_lookup(lookup(&[])).enabled());
        assert!(!CacheConfig::disabled().enabled());
    }

    #[test]
    fn errors_are_passed_on_and_not_cached() {
        let (d, cfg, p, input) = setup("errors");
        let fixed = [Input::File(input)];
        let err = detect_cached(&cfg, &p, &fixed, || Err(PortError::new("boom")));
        assert_eq!(err.unwrap_err().to_string(), "boom");
        assert!(!d.0.join("cache/detect-node.json").exists());
        let off = CacheConfig::disabled();
        assert!(detect_cached(&off, &p, &fixed, || Err(PortError::new("x"))).is_err());
    }

    #[test]
    fn a_file_changed_during_the_computation_is_not_stored() {
        let (d, cfg, p, input) = setup("race");
        let fixed = [Input::File(input.clone())];
        let runs = Cell::new(0);
        let (_, origin) = detect_cached(&cfg, &p, &fixed, || {
            runs.set(runs.get() + 1);
            d.write("project/package.json", "{\"edited\":true}");
            Ok((toolchain("x"), vec![]))
        })
        .unwrap();
        assert_eq!(origin, Origin::Miss);
        assert!(!d.0.join("cache/detect-node.json").exists());
        assert_eq!(run(&cfg, &p, &fixed, "x", &runs).1, Origin::Miss);
        assert_eq!(run(&cfg, &p, &fixed, "x", &runs).1, Origin::Hit);
    }

    #[test]
    fn path_dirs_and_vars_read_the_given_environment() {
        let joined = std::env::join_paths(["/a", "/b"]).unwrap();
        let joined = joined.to_str().unwrap().to_owned();
        let pairs = [("PATH", joined.as_str()), ("CARGO_HOME", "/ch")];
        let cfg = CacheConfig::from_lookup(lookup(&pairs));
        assert_eq!(cfg.path_dirs(), [PathBuf::from("/a"), PathBuf::from("/b")]);
        assert_eq!(cfg.var("CARGO_HOME"), Some(OsStr::new("/ch")));
        assert_eq!(cfg.var("RUSTUP_TOOLCHAIN"), None);
        assert_eq!(cfg.var("HOME"), None);
        assert!(CacheConfig::from_lookup(lookup(&[])).path_dirs().is_empty());
    }

    #[test]
    fn origin_names() {
        assert_eq!(Origin::Hit.to_string(), "hit");
        assert_eq!(Origin::Miss.to_string(), "miss");
        assert_eq!(Origin::Off.to_string(), "off");
    }
}
