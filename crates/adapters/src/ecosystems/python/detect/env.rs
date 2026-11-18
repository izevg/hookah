//! The interpreter environment tools run in, and which tools it provides.
//!
//! The environment is the first of these that applies:
//!
//! 1. A virtual environment: `$UV_PROJECT_ENVIRONMENT` when set, else `.venv/` or `venv/` (with a
//!    `pyvenv.cfg`) in the project or in a parent directory up to the repository root (a
//!    directory with `.git`) or the uv workspace root (a `pyproject.toml` with
//!    `[tool.uv.workspace]`). Tools run as `<venv>/bin/<tool>`. This covers plain venvs, uv
//!    (`uv sync` creates `.venv`), and Poetry with `virtualenvs.in-project`.
//! 2. `poetry.lock` and `poetry` on `PATH`: tools run as `poetry run <tool>`, because Poetry's
//!    own environments live in its cache directory, where hookah cannot see them.
//! 3. The tools on `PATH`.
//!
//! `uv run --frozen --no-sync` is deliberately not an environment. With no `.venv`, uv creates an
//! empty one before it runs anything (`--no-sync` skips the installing, not the creating), and
//! hookah must not write environments into the project. uv's own environment is always a `.venv`
//! (or `$UV_PROJECT_ENVIRONMENT`), which step 1 already finds.
//!
//! A tool is *available* only if it resolves in the chosen environment: its executable exists in
//! the virtual environment, its package is in `poetry.lock`, or its executable is on `PATH`.
//! There is no fallback to another environment: a project whose `.venv` lacks `ruff` has no
//! ruff, even if one is installed globally. Nothing is ever installed.

use std::fs;
use std::path::{Path, PathBuf};

use super::config::normalize;

/// A tool: the executable to run and the package that provides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolRef {
    /// Executable name, such as `lint-imports`.
    pub exe: &'static str,
    /// Package name, such as `import-linter`.
    pub package: &'static str,
}

/// How one tool is started: a program and its leading arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// Program to run.
    pub program: String,
    /// Arguments that come before the tool's own.
    pub args: Vec<String>,
    /// Environment variables to set for the tool.
    pub env: Vec<(String, String)>,
}

impl Invocation {
    /// The same invocation with an environment variable set for the tool.
    pub fn with_env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

/// Which package manager the project uses, for install hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    /// `uv.lock` exists.
    Uv,
    /// `poetry.lock` exists.
    Poetry,
    /// Neither.
    Pip,
}

impl Manager {
    /// The command that adds `package` as a development dependency.
    pub fn install_hint(self, package: &str) -> String {
        match self {
            Self::Uv => format!("uv add --dev {package}"),
            Self::Poetry => format!("poetry add --group dev {package}"),
            Self::Pip => format!("pip install {package}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    /// A virtual environment at this directory.
    Venv(PathBuf),
    Poetry,
    Path,
}

/// The chosen environment of one project.
#[derive(Debug, Clone)]
pub struct Env {
    kind: Kind,
    uv_lock: Option<String>,
    poetry_lock: Option<String>,
    path_dirs: Vec<PathBuf>,
}

impl Env {
    /// Resolves the environment of the project at `root`. `path_dirs` are the `PATH` entries and
    /// `explicit_venv` is `$UV_PROJECT_ENVIRONMENT`, if set.
    pub fn resolve(root: &Path, path_dirs: Vec<PathBuf>, explicit_venv: Option<PathBuf>) -> Self {
        let dirs = search_dirs(root);
        let explicit = explicit_venv
            .map(|v| root.join(v))
            .filter(|v| v.join("pyvenv.cfg").is_file());
        let venv = explicit.or_else(|| {
            dirs.iter().find_map(|d| {
                [".venv", "venv"]
                    .iter()
                    .map(|n| d.join(n))
                    .find(|v| v.join("pyvenv.cfg").is_file())
            })
        });
        let lock = |name: &str| {
            dirs.iter()
                .find_map(|d| fs::read_to_string(d.join(name)).ok())
        };
        let (uv_lock, poetry_lock) = (lock("uv.lock"), lock("poetry.lock"));
        let mut env = Self {
            kind: Kind::Path,
            uv_lock,
            poetry_lock,
            path_dirs,
        };
        env.kind = if let Some(venv) = venv {
            Kind::Venv(venv)
        } else if env.poetry_lock.is_some() && env.on_path("poetry").is_some() {
            Kind::Poetry
        } else {
            Kind::Path
        };
        env
    }

    /// The package manager, judged by lock files.
    pub fn manager(&self) -> Manager {
        if self.uv_lock.is_some() {
            Manager::Uv
        } else if self.poetry_lock.is_some() {
            Manager::Poetry
        } else {
            Manager::Pip
        }
    }

    /// How to run `tool`, or `None` when the environment does not provide it.
    pub fn tool(&self, tool: ToolRef) -> Option<Invocation> {
        match &self.kind {
            Kind::Venv(venv) => ["bin", "Scripts"]
                .iter()
                .flat_map(|d| {
                    exe_names(tool.exe, cfg!(windows))
                        .into_iter()
                        .map(move |n| venv.join(d).join(n))
                })
                .find(|p| is_executable(p))
                .map(|p| Invocation {
                    program: p.to_string_lossy().into_owned(),
                    args: vec![],
                    env: vec![],
                }),
            Kind::Poetry => {
                lock_has(self.poetry_lock.as_deref(), tool.package).then(|| Invocation {
                    program: "poetry".into(),
                    args: vec!["run".into(), tool.exe.into()],
                    env: vec![],
                })
            }
            Kind::Path => self.on_path(tool.exe).map(|p| Invocation {
                program: p.to_string_lossy().into_owned(),
                args: vec![],
                env: vec![],
            }),
        }
    }

    /// Whether `package` is installed: known for a virtual environment (its `dist-info`) and for
    /// Poetry (the lock file); never assumed for tools on `PATH`.
    pub fn has_package(&self, package: &str) -> bool {
        match &self.kind {
            Kind::Venv(venv) => installed_version(venv, package).is_some(),
            Kind::Poetry => lock_has(self.poetry_lock.as_deref(), package),
            Kind::Path => false,
        }
    }

    /// The installed `(major, minor)` version of `package`, known for a virtual environment only.
    pub fn version(&self, package: &str) -> Option<(u32, u32)> {
        match &self.kind {
            Kind::Venv(venv) => installed_version(venv, package),
            _ => None,
        }
    }

    fn on_path(&self, exe: &str) -> Option<PathBuf> {
        self.path_dirs
            .iter()
            .flat_map(|d| {
                exe_names(exe, cfg!(windows))
                    .into_iter()
                    .map(move |n| d.join(n))
            })
            .find(|p| is_executable(p))
    }

    /// A short description for diagnostics.
    pub fn describe(&self) -> String {
        match &self.kind {
            Kind::Venv(v) => format!("venv {}", v.display()),
            Kind::Poetry => "poetry run".into(),
            Kind::Path => "PATH".into(),
        }
    }
}

/// The directories searched for environments and lock files, nearest first: the project, then
/// its parents up to and including the repository root or uv workspace root. Without such a
/// boundary only the project itself is searched.
fn search_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for dir in root.ancestors() {
        dirs.push(dir.to_path_buf());
        if dir.join(".git").exists() || is_uv_workspace_root(dir) {
            return dirs;
        }
    }
    vec![root.to_path_buf()]
}

fn is_uv_workspace_root(dir: &Path) -> bool {
    fs::read_to_string(dir.join("pyproject.toml")).is_ok_and(|t| {
        t.lines()
            .any(|l| l.trim().starts_with("[tool.uv.workspace"))
    })
}

/// The file names a tool can have on disk. Unix has the bare name; Windows tools carry an
/// executable extension (`ruff.exe` from a wheel, `.cmd` or `.bat` from some installers).
fn exe_names(exe: &str, windows: bool) -> Vec<String> {
    if windows {
        ["exe", "cmd", "bat"]
            .iter()
            .map(|ext| format!("{exe}.{ext}"))
            .collect()
    } else {
        vec![exe.to_string()]
    }
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && has_execute_permission(&m))
}

/// Unix: any execute bit. Windows has no execute bit, and [`exe_names`] already restricted the
/// name to an executable extension, so that branch accepts every file.
fn has_execute_permission(meta: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        true
    }
}

/// Whether a lock file lists `package` (`name = "<package>"`).
fn lock_has(lock: Option<&str>, package: &str) -> bool {
    let wanted = normalize(package);
    lock.is_some_and(|text| {
        text.lines().any(|l| {
            l.trim()
                .strip_prefix("name = \"")
                .and_then(|r| r.strip_suffix('"'))
                .is_some_and(|n| normalize(n) == wanted)
        })
    })
}

/// The `(major, minor)` of `package` from `<name>-<version>.dist-info` in the environment's
/// `site-packages`.
fn installed_version(venv: &Path, package: &str) -> Option<(u32, u32)> {
    let wanted = normalize(package);
    let mut site_dirs = vec![venv.join("Lib").join("site-packages")];
    for lib in ["lib", "lib64"] {
        let Ok(entries) = fs::read_dir(venv.join(lib)) else {
            continue;
        };
        site_dirs.extend(entries.flatten().map(|e| e.path().join("site-packages")));
    }
    site_dirs.iter().find_map(|dir| {
        fs::read_dir(dir).ok()?.flatten().find_map(|e| {
            let name = e.file_name();
            let stem = name.to_str()?.strip_suffix(".dist-info")?;
            let (dist, version) = stem.split_once('-')?;
            (normalize(dist) == wanted).then(|| parse_version(version))?
        })
    })
}

fn parse_version(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.split('.').map(|p| {
        let digits: String = p.chars().take_while(char::is_ascii_digit).collect();
        digits.parse::<u32>().ok()
    });
    let major = parts.next()??;
    Some((major, parts.next().flatten().unwrap_or(0)))
}

#[cfg(test)]
mod name_tests {
    use super::exe_names;

    #[test]
    fn executable_names_per_platform() {
        assert_eq!(exe_names("ruff", false), ["ruff"]);
        assert_eq!(
            exe_names("ruff", true),
            ["ruff.exe", "ruff.cmd", "ruff.bat"]
        );
    }
}

// Unix only: the fake tools are files with an execute bit named without an extension.
#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use crate::ecosystems::python::fixture::Scratch;

    use super::*;

    const RUFF: ToolRef = ToolRef {
        exe: "ruff",
        package: "ruff",
    };
    const LINT_IMPORTS: ToolRef = ToolRef {
        exe: "lint-imports",
        package: "import-linter",
    };

    fn venv(s: &Scratch, at: &str, tools: &[&str]) {
        s.write(&format!("{at}/pyvenv.cfg"), "home = /usr/bin\n");
        for t in tools {
            s.executable(&format!("{at}/bin/{t}"));
        }
    }

    fn path_of(s: &Scratch, dir: &str) -> Vec<PathBuf> {
        vec![s.root().join(dir)]
    }

    #[test]
    fn project_venv_wins_and_runs_tools_by_path() {
        let s = Scratch::new("env-venv");
        venv(&s, ".venv", &["ruff"]);
        s.write("uv.lock", "[[package]]\nname = \"ruff\"\n");
        s.executable("bin/uv");
        let env = Env::resolve(s.root(), path_of(&s, "bin"), None);
        assert_eq!(
            env.describe(),
            format!("venv {}", s.root().join(".venv").display())
        );
        let inv = env.tool(RUFF).unwrap();
        assert_eq!(
            inv.program,
            s.root().join(".venv/bin/ruff").to_string_lossy()
        );
        assert!(inv.args.is_empty());
        assert!(env.tool(LINT_IMPORTS).is_none(), "the venv lacks the tool");
    }

    #[test]
    fn venv_without_the_tool_does_not_fall_back_to_path() {
        let s = Scratch::new("env-nofallback");
        venv(&s, "venv", &[]);
        s.executable("bin/ruff");
        let env = Env::resolve(s.root(), path_of(&s, "bin"), None);
        assert!(env.tool(RUFF).is_none());
    }

    #[test]
    fn directory_without_pyvenv_cfg_is_not_a_venv() {
        let s = Scratch::new("env-notvenv");
        s.executable("venv/bin/ruff");
        let env = Env::resolve(s.root(), vec![], None);
        assert!(env.tool(RUFF).is_none());
    }

    #[test]
    fn monorepo_member_uses_the_repository_root_venv() {
        let s = Scratch::new("env-monorepo");
        s.write(".git/HEAD", "");
        venv(&s, ".venv", &["ruff"]);
        s.write("services/api/pyproject.toml", "");
        let env = Env::resolve(&s.root().join("services/api"), vec![], None);
        assert!(env.tool(RUFF).is_some());
    }

    #[test]
    fn member_venv_shadows_the_root_venv() {
        let s = Scratch::new("env-shadow");
        s.write(".git/HEAD", "");
        venv(&s, ".venv", &["ruff"]);
        venv(&s, "svc/.venv", &[]);
        let env = Env::resolve(&s.root().join("svc"), vec![], None);
        assert!(env.tool(RUFF).is_none());
    }

    #[test]
    fn uv_workspace_root_bounds_the_search() {
        let s = Scratch::new("env-uvws");
        s.write("pyproject.toml", "[tool.uv.workspace]\nmembers = [\"a\"]\n");
        venv(&s, ".venv", &["ruff"]);
        let env = Env::resolve(&s.root().join("a"), vec![], None);
        assert!(env.tool(RUFF).is_some());
    }

    #[test]
    fn search_is_confined_to_the_project_without_a_boundary() {
        let s = Scratch::new("env-confined");
        venv(&s, ".venv", &["ruff"]);
        s.write("app/pyproject.toml", "");
        let env = Env::resolve(&s.root().join("app"), vec![], None);
        assert!(env.tool(RUFF).is_none());
    }

    #[test]
    fn uv_project_without_a_venv_never_runs_uv() {
        // `uv run --no-sync` would create an empty .venv, so uv.lock only sets the hint.
        let s = Scratch::new("env-uv");
        s.write("uv.lock", "version = 1\n\n[[package]]\nname = \"ruff\"\n");
        s.executable("bin/uv");
        s.executable("bin/ruff");
        let env = Env::resolve(s.root(), path_of(&s, "bin"), None);
        let inv = env.tool(RUFF).unwrap();
        assert_eq!(inv.program, s.root().join("bin/ruff").to_string_lossy());
        assert_eq!(env.manager(), Manager::Uv);
        assert!(!s.root().join(".venv").exists());
    }

    #[test]
    fn uv_project_environment_is_a_venv() {
        let s = Scratch::new("env-uvenv");
        venv(&s, "elsewhere/env", &["ruff"]);
        let explicit = Some(PathBuf::from("elsewhere/env"));
        let env = Env::resolve(s.root(), vec![], explicit);
        assert!(env.tool(RUFF).is_some());
        // A path that is no venv is ignored.
        let env = Env::resolve(s.root(), vec![], Some(PathBuf::from("nope")));
        assert!(env.tool(RUFF).is_none());
        // An explicit environment beats .venv.
        venv(&s, ".venv", &[]);
        let env = Env::resolve(s.root(), vec![], Some(s.root().join("elsewhere/env")));
        assert!(env.tool(RUFF).is_some());
    }

    #[test]
    fn poetry_lock_and_poetry_on_path() {
        let s = Scratch::new("env-poetry");
        s.write(
            "poetry.lock",
            "[[package]]\nname = \"ruff\"\nversion = \"1\"\n",
        );
        s.executable("bin/poetry");
        let env = Env::resolve(s.root(), path_of(&s, "bin"), None);
        let inv = env.tool(RUFF).unwrap();
        assert_eq!(
            (inv.program.as_str(), inv.args),
            ("poetry", vec!["run".into(), "ruff".into()])
        );
        assert_eq!(env.manager(), Manager::Poetry);
    }

    #[test]
    fn lock_without_its_manager_on_path_falls_through_to_path() {
        let s = Scratch::new("env-nomanager");
        s.write("uv.lock", "[[package]]\nname = \"ruff\"\n");
        s.executable("bin/ruff");
        let env = Env::resolve(s.root(), path_of(&s, "bin"), None);
        let inv = env.tool(RUFF).unwrap();
        assert_eq!(inv.program, s.root().join("bin/ruff").to_string_lossy());
    }

    #[test]
    fn poetry_needs_both_its_lock_and_its_executable() {
        let lock = "[[package]]\nname = \"ruff\"\nversion = \"1\"\n";
        let ruff_path = |s: &Scratch| s.root().join("bin/ruff").to_string_lossy().into_owned();
        // The lock without `poetry` on PATH.
        let s = Scratch::new("env-poetry-lock-only");
        s.write("poetry.lock", lock);
        s.executable("bin/ruff");
        let env = Env::resolve(s.root(), path_of(&s, "bin"), None);
        assert_eq!(env.tool(RUFF).unwrap().program, ruff_path(&s));
        assert_eq!(env.manager(), Manager::Poetry);
        // `poetry` on PATH without a lock.
        let s = Scratch::new("env-poetry-exe-only");
        s.executable("bin/poetry");
        s.executable("bin/ruff");
        let env = Env::resolve(s.root(), path_of(&s, "bin"), None);
        assert_eq!(env.tool(RUFF).unwrap().program, ruff_path(&s));
        assert_eq!(env.manager(), Manager::Pip);
    }

    #[test]
    fn plain_path_environment() {
        let s = Scratch::new("env-path");
        s.executable("bin/ruff");
        let env = Env::resolve(s.root(), path_of(&s, "bin"), None);
        assert!(env.tool(RUFF).is_some());
        assert!(env.tool(LINT_IMPORTS).is_none());
        assert_eq!(env.manager(), Manager::Pip);
        assert!(!env.has_package("pytest-testmon"));
    }

    #[test]
    fn non_executable_files_do_not_resolve() {
        let s = Scratch::new("env-noexec");
        s.write("bin/ruff", "x");
        assert!(
            Env::resolve(s.root(), path_of(&s, "bin"), None)
                .tool(RUFF)
                .is_none()
        );
    }

    #[test]
    fn installed_versions_come_from_dist_info() {
        let s = Scratch::new("env-version");
        venv(&s, ".venv", &[]);
        let site = ".venv/lib/python3.13/site-packages";
        s.write(&format!("{site}/mutmut-3.8.1.dist-info/METADATA"), "");
        s.write(&format!("{site}/import_linter-2.16.dist-info/METADATA"), "");
        s.write(
            &format!("{site}/pytest_testmon-2.3.0.dist-info/METADATA"),
            "",
        );
        let env = Env::resolve(s.root(), vec![], None);
        assert_eq!(env.version("mutmut"), Some((3, 8)));
        assert_eq!(env.version("import-linter"), Some((2, 16)));
        assert!(env.has_package("pytest-testmon"));
        assert!(!env.has_package("black"));
        assert_eq!(env.version("black"), None);
    }

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("1.11.0"), Some((1, 11)));
        assert_eq!(parse_version("2"), Some((2, 0)));
        assert_eq!(parse_version("2.0rc1"), Some((2, 0)));
        assert_eq!(parse_version("x"), None);
    }

    #[test]
    fn executable_names_per_platform() {
        assert_eq!(exe_names("ruff", false), ["ruff"]);
        assert_eq!(
            exe_names("ruff", true),
            ["ruff.exe", "ruff.cmd", "ruff.bat"]
        );
    }

    #[test]
    fn env_and_hints() {
        let inv = Invocation {
            program: "uv".into(),
            args: vec!["run".into(), "x".into()],
            env: vec![],
        }
        .with_env("PYTHONPATH", "src");
        assert_eq!(inv.program, "uv");
        assert_eq!(inv.args, ["run", "x"]);
        assert_eq!(inv.env, [("PYTHONPATH".to_string(), "src".to_string())]);
        assert_eq!(Manager::Uv.install_hint("mutmut"), "uv add --dev mutmut");
        assert_eq!(
            Manager::Poetry.install_hint("mutmut"),
            "poetry add --group dev mutmut"
        );
        assert_eq!(Manager::Pip.install_hint("mutmut"), "pip install mutmut");
    }
}
