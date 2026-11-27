//! Per-project tool environment.
//!
//! `hookah` hooks are installed once for all projects, but the tools of one project may need
//! their own environment: a JDK, a Maven `settings.xml` for a corporate registry, a `PATH`
//! entry. [`resolve`] works out the variables for a directory and [`ProjectEnvRunner`] applies
//! them to every tool process spawned for it, in every phase (the baseline job included).
//!
//! Sources, lowest to highest precedence:
//!
//! 1. `<root>/.hookah.toml`, `[env]`: committed, shared with the team;
//! 2. `<root>/.hookah.local.toml`, `[env]`: personal, not committed;
//! 3. the user config, every `[[project]]` whose `path` matches the root, in file order (later
//!    sections win): the place for repositories that cannot take a file.
//!
//! The root is the nearest directory, from the tool's working directory up to the git root,
//! that holds a `.hookah.toml` or `.hookah.local.toml`; failing that the git root; failing that
//! the working directory itself. A broken file is skipped with a warning and never blocks.

mod expand;
pub(crate) mod file;
mod runner;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hookah_domain::model::ModelFilter;

pub use runner::ProjectEnvRunner;

use expand::{Context, expand, matches, matches_subtree};

/// Project config file shared with the team.
pub const PROJECT_FILE: &str = ".hookah.toml";
/// Personal project config file, kept out of version control.
pub const LOCAL_FILE: &str = ".hookah.local.toml";
/// Environment variable that names the user config file.
pub const CONFIG_VAR: &str = "HOOKAH_CONFIG";
/// Environment variable that overrides the configured model filter (comma-separated patterns).
pub const MODELS_VAR: &str = "HOOKAH_MODELS";

/// An environment lookup.
pub type VarFn = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The machine facts config resolution depends on, injectable for tests.
pub struct Host {
    /// Environment lookup.
    pub var: VarFn,
    /// Whether the host is Windows (path matching ignores case there).
    pub windows: bool,
}

impl Host {
    /// The running process's environment.
    pub fn from_process() -> Self {
        Self {
            var: Box::new(|name| std::env::var(name).ok()),
            windows: cfg!(windows),
        }
    }

    fn home(&self) -> Option<String> {
        let name = if self.windows { "USERPROFILE" } else { "HOME" };
        (self.var)(name).filter(|h| !h.is_empty())
    }

    /// The user config path: `$HOOKAH_CONFIG` when absolute; on Windows
    /// `%APPDATA%\hookah\config.toml`; elsewhere `$XDG_CONFIG_HOME/hookah/config.toml` when that is
    /// absolute, else `~/.config/hookah/config.toml` (macOS included).
    pub fn user_config(&self) -> Option<PathBuf> {
        let absolute = |name: &str| {
            (self.var)(name)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        };
        if let Some(path) = absolute(CONFIG_VAR) {
            return Some(path);
        }
        let base = if self.windows {
            absolute("APPDATA").or_else(|| {
                self.home()
                    .map(|h| PathBuf::from(h).join("AppData").join("Roaming"))
            })
        } else {
            absolute("XDG_CONFIG_HOME")
                .or_else(|| self.home().map(|h| PathBuf::from(h).join(".config")))
        };
        base.map(|b| b.join("hookah").join("config.toml"))
    }
}

/// One variable of the effective environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvEntry {
    /// Variable name.
    pub key: String,
    /// Expanded value.
    pub value: String,
    /// The file (and for the user config, the section) that set it last.
    pub source: String,
}

/// The effective model filter and where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelSetting {
    /// Which models hooks run for.
    pub filter: ModelFilter,
    /// The config file, `[[project]]` section, or environment variable that set it.
    pub source: Option<String>,
}

/// What config says about one project.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectEnv {
    /// The config root.
    pub root: PathBuf,
    /// The git checkout root, or [`ProjectEnv::root`] when there is none.
    pub git_root: PathBuf,
    /// The user config path, if one can be resolved (it may not exist).
    pub user_config: Option<PathBuf>,
    /// Config files that exist and parsed, lowest precedence first.
    pub files: Vec<PathBuf>,
    /// The effective variables, sorted by name.
    pub entries: Vec<EnvEntry>,
    /// The effective model filter for hook invocations.
    pub models: ModelSetting,
    /// Problems that made a file or value be skipped.
    pub warnings: Vec<String>,
}

impl ProjectEnv {
    /// The variables as `(name, value)` pairs.
    pub fn pairs(&self) -> Vec<(String, String)> {
        self.entries
            .iter()
            .map(|e| (e.key.clone(), e.value.clone()))
            .collect()
    }
}

/// Resolves the environment for tools that run in `dir`.
pub fn resolve(dir: &Path, host: &Host) -> ProjectEnv {
    let dir = dunce::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let (root, git_root) = roots(&dir);
    let home = host.home();
    let (root_text, git_text) = (text(&root), text(&git_root));
    let ctx = Context {
        home: home.as_deref(),
        root: &root_text,
        git_root: &git_text,
        var: &*host.var,
    };
    let mut out = ProjectEnv {
        root: root.clone(),
        git_root: git_root.clone(),
        user_config: host.user_config(),
        ..ProjectEnv::default()
    };
    let mut layers: Vec<(String, BTreeMap<String, String>)> = Vec::new();
    let mut user_models: Option<ModelSetting> = None;
    let mut models: Option<ModelSetting> = None;
    for name in [PROJECT_FILE, LOCAL_FILE] {
        let path = root.join(name);
        if let Some(parsed) = load(&path, &mut out) {
            let source = text(&path);
            if let Some(list) = parsed.models {
                models = Some(ModelSetting {
                    filter: ModelFilter::from_patterns(list),
                    source: Some(source.clone()),
                });
            }
            layers.push((source, parsed.env));
        }
    }
    if let Some(user) = out.user_config.clone()
        && let Some(parsed) = load(&user, &mut out)
    {
        if let Some(list) = parsed.models {
            user_models = Some(ModelSetting {
                filter: ModelFilter::from_patterns(list),
                source: Some(text(&user)),
            });
        }
        for section in parsed.project {
            let pattern = expand(&section.path, &ctx);
            if matches(&pattern, &root_text, host.windows) {
                let source = format!("{} [[project]] path = {:?}", text(&user), section.path);
                if let Some(list) = section.models {
                    models = Some(ModelSetting {
                        filter: ModelFilter::from_patterns(list),
                        source: Some(source.clone()),
                    });
                }
                layers.push((source, section.env));
            }
        }
    }
    out.models = models.or(user_models).unwrap_or_default();
    if let Some(raw) = (host.var)(MODELS_VAR) {
        let parts: Vec<String> = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if !parts.is_empty() {
            out.models = ModelSetting {
                filter: ModelFilter::from_patterns(parts),
                source: Some(MODELS_VAR.to_owned()),
            };
        }
    }
    let mut merged: BTreeMap<String, EnvEntry> = BTreeMap::new();
    for (source, env) in layers {
        for (key, value) in env {
            let value = expand(&value, &ctx);
            let entry = EnvEntry {
                key: key.clone(),
                value,
                source: source.clone(),
            };
            merged.insert(key, entry);
        }
    }
    out.entries = merged.into_values().collect();
    out
}

/// True when `HOOKAH_MODELS` is explicitly set in the environment to allow all models (e.g. `"*"`),
/// overriding every config file across the entire workspace.
pub fn models_env_allows_all(host: &Host) -> bool {
    let Some(raw) = (host.var)(MODELS_VAR) else {
        return false;
    };
    let parts: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    !parts.is_empty() && ModelFilter::from_patterns(parts).is_all()
}

/// True if `dir` itself, a matching `[[project]]` section of the user config, `HOOKAH_MODELS`, or
/// any nested `.hookah.toml` / `.hookah.local.toml` below `dir` currently restricts models (i.e.
/// resolves to something other than [`ModelFilter::All`]).
///
/// When `HOOKAH_MODELS` is set in the environment it overrides every config file, so
/// `HOOKAH_MODELS="*"` immediately returns `false` without walking the tree. When the directory
/// walk reaches its depth or entry cap without finishing, it fails safe and returns `true` so a
/// deeply nested filtered project is never silently missed.
pub fn has_model_filter_in_tree(dir: &Path, host: &Host) -> bool {
    if let Some(raw) = (host.var)(MODELS_VAR) {
        let parts: Vec<String> = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if !parts.is_empty() {
            return !ModelFilter::from_patterns(parts).is_all();
        }
    }
    let dir = dunce::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if !resolve(&dir, host).models.filter.is_all() {
        return true;
    }
    if let Some(user) = host.user_config()
        && let Ok(text_of) = std::fs::read_to_string(&user)
        && let Ok(parsed) = file::parse(&text_of)
    {
        let (root, git_root) = roots(&dir);
        let home = host.home();
        let (dir_text, root_text, git_text) = (text(&dir), text(&root), text(&git_root));
        let ctx = Context {
            home: home.as_deref(),
            root: &root_text,
            git_root: &git_text,
            var: &*host.var,
        };
        if parsed.project.into_iter().any(|section| {
            let restricted = section
                .models
                .is_some_and(|list| !ModelFilter::from_patterns(list).is_all());
            if !restricted {
                return false;
            }
            let pattern = expand(&section.path, &ctx);
            matches_subtree(&pattern, &dir_text, host.windows)
        }) {
            return true;
        }
    }
    scan_nested_model_filter(&dir, host, 0, &mut 0)
}

const MAX_CONFIG_SCAN_DEPTH: usize = 16;
const MAX_CONFIG_SCAN_DIRS: usize = 2_048;
const SKIP_SCAN_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".tox",
    ".next",
    ".nuxt",
    "vendor",
    ".idea",
    ".vscode",
    ".gradle",
    ".mvn",
    "out",
    "bin",
    "obj",
];

fn scan_nested_model_filter(dir: &Path, host: &Host, depth: usize, visited: &mut usize) -> bool {
    if depth >= MAX_CONFIG_SCAN_DEPTH || *visited >= MAX_CONFIG_SCAN_DIRS {
        // Incomplete scan: fail safe rather than falsely claiming no nested filter exists.
        return true;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if depth > 0 && (name_str == PROJECT_FILE || name_str == LOCAL_FILE || name_str == ".git") {
            // A nested config file or nested git checkout (including symlinked markers) changes
            // config-root inheritance, so evaluate its effective filter.
            if !resolve(dir, host).models.filter.is_all() {
                return true;
            }
            continue;
        }
        if !kind.is_symlink() && kind.is_dir() && !SKIP_SCAN_DIRS.contains(&name_str) {
            subdirs.push(entry.path());
        }
    }
    for sub in subdirs {
        *visited += 1;
        if *visited > MAX_CONFIG_SCAN_DIRS {
            return true;
        }
        if scan_nested_model_filter(&sub, host, depth + 1, visited) {
            return true;
        }
    }
    false
}

/// Reads one config file; records it in `out.files`, or a warning when it is unusable. A file
/// that does not exist is silent.
fn load(path: &Path, out: &mut ProjectEnv) -> Option<file::ConfigFile> {
    let text_of = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            out.warnings
                .push(format!("{}: cannot read: {e}; ignoring it", text(path)));
            return None;
        }
    };
    match file::parse(&text_of) {
        Ok(parsed) => {
            out.files.push(path.to_path_buf());
            Some(parsed)
        }
        Err(e) => {
            out.warnings
                .push(format!("{}: {e}; ignoring it", text(path)));
            None
        }
    }
}

/// `(config root, git root)` for a directory; see the module docs.
fn roots(dir: &Path) -> (PathBuf, PathBuf) {
    let mut config_root: Option<&Path> = None;
    let mut git_root: Option<&Path> = None;
    for ancestor in dir.ancestors() {
        if config_root.is_none()
            && (ancestor.join(PROJECT_FILE).is_file() || ancestor.join(LOCAL_FILE).is_file())
        {
            config_root = Some(ancestor);
        }
        if ancestor.join(".git").exists() {
            git_root = Some(ancestor);
            break;
        }
    }
    let root = config_root.or(git_root).unwrap_or(dir);
    (root.to_path_buf(), git_root.unwrap_or(root).to_path_buf())
}

fn text(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    /// A scratch directory, canonical, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "hookah-env-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            Self(dunce::canonicalize(dir).unwrap())
        }

        fn write(&self, rel: &str, text: &str) -> PathBuf {
            let path = self.0.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn host(vars: &[(&str, &str)], windows: bool) -> Host {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        Host {
            var: Box::new(move |name| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())),
            windows,
        }
    }

    fn value<'a>(env: &'a ProjectEnv, key: &str) -> Option<&'a str> {
        env.entries
            .iter()
            .find(|e| e.key == key)
            .map(|e| e.value.as_str())
    }

    #[test]
    fn user_config_location_follows_the_platform() {
        let unix = |vars: &[(&str, &str)]| host(vars, false).user_config();
        // `Path::is_absolute` follows the OS running the tests, so the absolute paths here are
        // spelled for it: `/c` on Unix, `C:/c` on Windows.
        let a = |p: &str| {
            if cfg!(windows) {
                format!("C:{p}")
            } else {
                p.to_string()
            }
        };
        assert_eq!(
            unix(&[("HOOKAH_CONFIG", a("/c/h.toml").as_str()), ("HOME", "/h")]),
            Some(PathBuf::from(a("/c/h.toml")))
        );
        assert_eq!(
            unix(&[("XDG_CONFIG_HOME", a("/x").as_str()), ("HOME", "/h")]),
            Some(PathBuf::from(a("/x")).join("hookah").join("config.toml"))
        );
        assert_eq!(
            unix(&[("XDG_CONFIG_HOME", "rel"), ("HOME", "/h")]),
            Some(
                PathBuf::from("/h")
                    .join(".config")
                    .join("hookah")
                    .join("config.toml")
            )
        );
        assert_eq!(
            unix(&[("HOOKAH_CONFIG", "rel"), ("HOME", "/h")]),
            Some(
                PathBuf::from("/h")
                    .join(".config")
                    .join("hookah")
                    .join("config.toml")
            )
        );
        assert_eq!(unix(&[("HOME", "")]), None);
        assert_eq!(unix(&[]), None);
    }

    #[test]
    fn user_config_location_on_windows() {
        let win = |vars: &[(&str, &str)]| host(vars, true).user_config();
        // `is_absolute` is OS specific, so only assert what holds on the running OS.
        if cfg!(windows) {
            assert_eq!(
                win(&[("APPDATA", "C:\\A")]),
                Some(PathBuf::from("C:\\A\\hookah\\config.toml"))
            );
        }
        assert_eq!(
            win(&[("USERPROFILE", "U")]),
            Some(
                PathBuf::from("U")
                    .join("AppData")
                    .join("Roaming")
                    .join("hookah")
                    .join("config.toml")
            )
        );
        assert_eq!(win(&[("HOME", "/h")]), None, "HOME is not a Windows home");
    }

    #[test]
    fn no_config_means_no_env() {
        let t = Scratch::new();
        let env = resolve(&t.0, &host(&[], false));
        assert!(env.entries.is_empty() && env.files.is_empty() && env.warnings.is_empty());
        assert_eq!(env.root, t.0);
        assert_eq!(env.git_root, t.0);
    }

    #[test]
    fn layers_override_in_order() {
        let t = Scratch::new();
        t.write(
            PROJECT_FILE,
            "[env]\nA = \"shared\"\nB = \"shared\"\nC = \"shared\"\n",
        );
        t.write(LOCAL_FILE, "[env]\nB = \"local\"\nC = \"local\"\n");
        let user = t.write(
            "user.toml",
            &format!(
                "[[project]]\npath = {:?}\n[project.env]\nC = \"user\"\n[[project]]\npath = \"/elsewhere\"\n[project.env]\nA = \"nope\"\n",
                t.0.display().to_string()
            ),
        );
        let cfg = user.display().to_string();
        let env = resolve(&t.0, &host(&[("HOOKAH_CONFIG", &cfg)], false));
        assert_eq!(value(&env, "A"), Some("shared"));
        assert_eq!(value(&env, "B"), Some("local"));
        assert_eq!(value(&env, "C"), Some("user"));
        assert_eq!(env.files.len(), 3);
        let source = |k: &str| {
            env.entries
                .iter()
                .find(|e| e.key == k)
                .unwrap()
                .source
                .clone()
        };
        assert!(source("A").ends_with(PROJECT_FILE));
        assert!(source("B").ends_with(LOCAL_FILE));
        assert!(source("C").contains("[[project]]"));
        let keys: Vec<_> = env.entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["A", "B", "C"]);
        assert_eq!(env.pairs()[0], ("A".to_string(), "shared".to_string()));
    }

    #[test]
    fn later_project_sections_win_and_globs_match() {
        let t = Scratch::new();
        let base = t.0.display().to_string().replace('\\', "/");
        let user = t.write(
            "user.toml",
            &format!(
                "[[project]]\npath = \"{base}/**\"\n[project.env]\nK = \"first\"\nONLY = \"x\"\n[[project]]\npath = \"{base}\"\n[project.env]\nK = \"second\"\n"
            ),
        );
        let cfg = user.display().to_string();
        let env = resolve(&t.0, &host(&[("HOOKAH_CONFIG", &cfg)], cfg!(windows)));
        assert_eq!(value(&env, "K"), Some("second"));
        assert_eq!(value(&env, "ONLY"), Some("x"));
    }

    #[test]
    fn values_are_expanded_with_the_root_home_and_environment() {
        let t = Scratch::new();
        t.write(
            LOCAL_FILE,
            "[env]\nS = \"-s {root}/ci/settings.xml\"\nH = \"~/jdk\"\nP = \"{root}/bin:${PATH}\"\nG = \"{git_root}\"\n",
        );
        let env = resolve(&t.0, &host(&[("HOME", "/home/u"), ("PATH", "/bin")], false));
        let root = t.0.display().to_string();
        assert_eq!(
            value(&env, "S"),
            Some(format!("-s {root}/ci/settings.xml").as_str())
        );
        assert_eq!(value(&env, "H"), Some("/home/u/jdk"));
        assert_eq!(value(&env, "P"), Some(format!("{root}/bin:/bin").as_str()));
        assert_eq!(value(&env, "G"), Some(root.as_str()));
    }

    #[test]
    fn the_root_is_found_upwards_and_stops_at_the_git_root() {
        let t = Scratch::new();
        fs::create_dir_all(t.0.join("repo/.git")).unwrap();
        fs::create_dir_all(t.0.join("repo/mod/src")).unwrap();
        t.write(PROJECT_FILE, "[env]\nOUTSIDE = \"1\"\n");
        t.write("repo/.hookah.toml", "[env]\nREPO = \"1\"\n");
        let deep = t.0.join("repo/mod/src");
        let env = resolve(&deep, &host(&[], false));
        assert_eq!(env.root, t.0.join("repo"));
        assert_eq!(env.git_root, t.0.join("repo"));
        assert_eq!(value(&env, "REPO"), Some("1"));
        assert_eq!(value(&env, "OUTSIDE"), None);

        t.write("repo/mod/.hookah.local.toml", "[env]\nMOD = \"1\"\n");
        let env = resolve(&deep, &host(&[], false));
        assert_eq!(env.root, t.0.join("repo/mod"));
        assert_eq!(env.git_root, t.0.join("repo"));
        assert_eq!(value(&env, "MOD"), Some("1"));
        assert_eq!(value(&env, "REPO"), None, "the nearest root wins");

        fs::remove_file(t.0.join("repo/mod/.hookah.local.toml")).unwrap();
        fs::remove_file(t.0.join("repo/.hookah.toml")).unwrap();
        let env = resolve(&deep, &host(&[], false));
        assert_eq!(env.root, t.0.join("repo"), "no file: the git root");
    }

    #[test]
    fn a_broken_file_is_skipped_with_a_warning() {
        let t = Scratch::new();
        t.write(PROJECT_FILE, "[env\n");
        t.write(LOCAL_FILE, "[env]\nOK = \"1\"\n");
        let env = resolve(&t.0, &host(&[], false));
        assert_eq!(value(&env, "OK"), Some("1"));
        assert_eq!(env.warnings.len(), 1);
        assert!(env.warnings[0].contains(PROJECT_FILE));
        assert!(env.warnings[0].ends_with("ignoring it"));
        assert_eq!(env.files.len(), 1);
    }

    #[test]
    fn models_follow_precedence_and_env_override() {
        let t = Scratch::new();
        let user = t.write(
            "user.toml",
            &format!(
                "models = [\"global-*\"]\n[[project]]\npath = {:?}\nmodels = [\"section-*\"]\n",
                t.0.display().to_string()
            ),
        );
        let cfg = user.display().to_string();

        // Only user global default (when no [[project]] matches).
        let other = Scratch::new();
        let resolved = resolve(&other.0, &host(&[("HOOKAH_CONFIG", &cfg)], false));
        assert_eq!(
            resolved.models.filter,
            ModelFilter::Only(vec!["global-*".into()])
        );
        assert_eq!(resolved.models.source.as_deref(), Some(cfg.as_str()));

        // .hookah.toml beats user global default.
        other.write(PROJECT_FILE, "models = [\"project-*\"]\n");
        let resolved = resolve(&other.0, &host(&[("HOOKAH_CONFIG", &cfg)], false));
        assert_eq!(
            resolved.models.filter,
            ModelFilter::Only(vec!["project-*".into()])
        );
        assert!(resolved.models.source.unwrap().ends_with(PROJECT_FILE));

        // .hookah.local.toml beats .hookah.toml (and "*" resets to All).
        other.write(LOCAL_FILE, "models = [\"*\"]\n");
        let resolved = resolve(&other.0, &host(&[("HOOKAH_CONFIG", &cfg)], false));
        assert_eq!(resolved.models.filter, ModelFilter::All);
        assert!(resolved.models.source.unwrap().ends_with(LOCAL_FILE));

        // Matching [[project]] section beats .hookah.local.toml.
        t.write(LOCAL_FILE, "models = [\"local-*\"]\n");
        let resolved = resolve(&t.0, &host(&[("HOOKAH_CONFIG", &cfg)], false));
        assert_eq!(
            resolved.models.filter,
            ModelFilter::Only(vec!["section-*".into()])
        );
        assert!(resolved.models.source.unwrap().contains("[[project]]"));

        // HOOKAH_MODELS beats every config file; blank HOOKAH_MODELS is ignored.
        let resolved = resolve(
            &t.0,
            &host(
                &[
                    ("HOOKAH_CONFIG", &cfg),
                    (MODELS_VAR, " claude-sonnet-* , gpt-5 "),
                ],
                false,
            ),
        );
        assert_eq!(
            resolved.models.filter,
            ModelFilter::Only(vec!["claude-sonnet-*".into(), "gpt-5".into()])
        );
        assert_eq!(resolved.models.source.as_deref(), Some(MODELS_VAR));

        let blank = resolve(
            &t.0,
            &host(&[("HOOKAH_CONFIG", &cfg), (MODELS_VAR, " , ")], false),
        );
        assert_eq!(
            blank.models.filter,
            ModelFilter::Only(vec!["section-*".into()])
        );
    }

    #[test]
    fn an_unreadable_file_is_a_warning_not_a_failure() {
        let t = Scratch::new();
        fs::create_dir_all(t.0.join(PROJECT_FILE)).unwrap();
        fs::create_dir_all(t.0.join(".git")).unwrap();
        let env = resolve(&t.0, &host(&[], false));
        assert_eq!(env.warnings.len(), 1, "{:?}", env.warnings);
        assert!(env.warnings[0].contains("cannot read"));
    }

    #[test]
    fn tree_filter_detection_handles_nested_git_roots_and_git_root_placeholders() {
        let t = Scratch::new();
        fs::create_dir_all(t.0.join(".git")).unwrap();
        fs::create_dir_all(t.0.join("frontend")).unwrap();
        fs::create_dir_all(t.0.join("backend")).unwrap();

        // A [[project]] with `{git_root}/backend` does NOT match sibling `frontend`.
        let cfg_placeholder = t.write(
            "user-placeholder.toml",
            "[[project]]\npath = \"{git_root}/backend\"\nmodels = [\"claude-sonnet-*\"]\n",
        );
        let h = host(
            &[("HOOKAH_CONFIG", &cfg_placeholder.to_string_lossy())],
            false,
        );
        assert!(!has_model_filter_in_tree(&t.0.join("frontend"), &h));
        assert!(has_model_filter_in_tree(&t.0.join("backend"), &h));

        // Unrestricted root (`models = ["*"]`) with a nested git checkout 4 levels down that
        // inherits the user's top-level `models` filter MUST be detected.
        t.write(PROJECT_FILE, "models = [\"*\"]\n");
        let nested_git = t.0.join("a/b/c/d");
        fs::create_dir_all(nested_git.join(".git")).unwrap();
        let cfg_global = t.write("user-global.toml", "models = [\"gemini-*\"]\n");
        let h_global = host(&[("HOOKAH_CONFIG", &cfg_global.to_string_lossy())], false);
        assert!(has_model_filter_in_tree(&t.0, &h_global));
    }
}
