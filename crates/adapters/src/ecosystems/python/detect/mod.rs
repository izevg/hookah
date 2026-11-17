//! Python toolchain detection.
//!
//! The project root is the directory of the nearest `pyproject.toml`. Tools are found in the
//! project's environment (see [`env`]) and configured by `pyproject.toml`, `setup.cfg`,
//! `tox.ini`, and the tools' own files (see [`config`]). All commands run in the project root; the
//! commands below are shown without their environment prefix (`<venv>/bin/` or `poetry run`).
//!
//! | Check        | Tool          | Command                                                         | Timeout |
//! |--------------|---------------|-----------------------------------------------------------------|---------|
//! | format       | `ruff`        | `ruff format --force-exclude {files}`                           | 30 s    |
//! | format       | `black`       | `black {files}` (only without ruff; needs `[tool.black]` or a declared dependency) | 30 s |
//! | lint         | `ruff`        | `ruff check --output-format json --force-exclude --no-fix {files}` | 30 s |
//! | typecheck    | `mypy`        | `mypy [-O json] --show-column-numbers --show-error-codes --no-error-summary --no-color-output {files}` | 120 s |
//! | architecture | `lint-imports`| `lint-imports` (project-wide; `PYTHONPATH=src` in the src layout) | 60 s  |
//! | test         | `pytest`      | `pytest [--testmon] -rfE --tb=short -q --color=no --rootdir=. {tests}`       | 300 s   |
//! | mutation     | `mutmut`      | `mutmut run {globs}` (mutmut 3 only)                            | 480 s   |
//!
//! `-O json` is used only when the installed mypy is known to be 1.11 or newer (read from the
//! virtual environment); otherwise the text output is parsed. mypy runs when it is available and
//! configured (`[tool.mypy]`, `mypy.ini`, `.mypy.ini`, `setup.cfg [mypy]`, `tox.ini [mypy]`) or a
//! declared dependency. import-linter needs its own configuration. With `pytest-testmon`
//! installed, pytest runs with `--testmon` and no file arguments, so testmon picks the tests.
//!
//! Tools the project configures or implies but the environment lacks become [`MissingTool`]s:
//! mutmut when pytest is available, and ruff, mypy, or import-linter when configured. The hint
//! matches the package manager (`uv add --dev`, `poetry add --group dev`, `pip install`).

pub mod config;
pub mod env;

use std::path::Path;
use std::time::Duration;

use hookah_domain::toolchain::{Arg, CheckKind, CommandSpec, MissingTool, Tool, Toolchain};

use crate::ecosystems::python::layout;
use config::{Config, MypyConfig};
use env::{Env, Invocation, Manager, ToolRef};

/// Name of the `ruff format` tool.
pub const RUFF_FORMAT: &str = "ruff-format";
/// Name of the `black` tool.
pub const BLACK: &str = "black";
/// Name of the `ruff check` tool.
pub const RUFF: &str = "ruff";
/// Name of the mypy tool.
pub const MYPY: &str = "mypy";
/// Name of the import-linter tool.
pub const IMPORT_LINTER: &str = "import-linter";
/// Name of the pytest tool.
pub const PYTEST: &str = "pytest";
/// Name of the mutmut tool.
pub const MUTMUT: &str = "mutmut";

const RUFF_REF: ToolRef = ToolRef {
    exe: "ruff",
    package: "ruff",
};
const BLACK_REF: ToolRef = ToolRef {
    exe: "black",
    package: "black",
};
const MYPY_REF: ToolRef = ToolRef {
    exe: "mypy",
    package: "mypy",
};
const IMPORT_LINTER_REF: ToolRef = ToolRef {
    exe: "lint-imports",
    package: "import-linter",
};
const PYTEST_REF: ToolRef = ToolRef {
    exe: "pytest",
    package: "pytest",
};
const MUTMUT_REF: ToolRef = ToolRef {
    exe: "mutmut",
    package: "mutmut",
};

const FORMAT_TIMEOUT: Duration = Duration::from_secs(30);
const LINT_TIMEOUT: Duration = Duration::from_secs(30);
const MYPY_TIMEOUT: Duration = Duration::from_secs(120);
const IMPORT_LINTER_TIMEOUT: Duration = Duration::from_secs(60);
const PYTEST_TIMEOUT: Duration = Duration::from_secs(300);
const MUTMUT_TIMEOUT: Duration = Duration::from_secs(480);

/// The first mypy version with `-O json`.
const MYPY_JSON: (u32, u32) = (1, 11);
/// mutmut major versions below this are not supported.
const MUTMUT_MIN_MAJOR: u32 = 3;

/// Everything detected about one project: its environment and configuration.
#[derive(Debug, Clone)]
pub struct Setup {
    /// Where tools run.
    pub env: Env,
    /// What the project configures.
    pub config: Config,
}

impl Setup {
    /// Detects the setup of the project at `root`, using the process's `PATH`.
    pub fn load(root: &Path) -> Self {
        let path_dirs = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        let uv_env = std::env::var_os("UV_PROJECT_ENVIRONMENT")
            .filter(|v| !v.is_empty())
            .map(std::path::PathBuf::from);
        Self::with_env(root, path_dirs, uv_env)
    }

    /// Detects the setup with an explicit `PATH` and `$UV_PROJECT_ENVIRONMENT`.
    pub fn with_env(
        root: &Path,
        path_dirs: Vec<std::path::PathBuf>,
        uv_env: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            env: Env::resolve(root, path_dirs, uv_env),
            config: Config::load(root),
        }
    }

    /// Detects the setup with an explicit `PATH`.
    pub fn with_path(root: &Path, path_dirs: Vec<std::path::PathBuf>) -> Self {
        Self::with_env(root, path_dirs, None)
    }

    /// Whether pytest runs through `pytest-testmon`.
    pub fn testmon(&self) -> bool {
        self.env.tool(PYTEST_REF).is_some() && self.env.has_package("pytest-testmon")
    }
}

fn spec(
    inv: Invocation,
    args: &[&str],
    tail: Vec<Arg>,
    root: &Path,
    timeout: Duration,
) -> CommandSpec {
    let mut all: Vec<Arg> = inv.args.into_iter().map(Arg::Literal).collect();
    all.extend(args.iter().map(|a| Arg::Literal((*a).to_string())));
    all.extend(tail);
    CommandSpec::new(inv.program, all, root, timeout)
}

fn tool(name: &str, kind: CheckKind, command: CommandSpec) -> Tool {
    Tool {
        name: name.into(),
        kind,
        command,
    }
}

/// Builds the toolchain of the project at `root`.
pub fn toolchain(setup: &Setup, root: &Path) -> Toolchain {
    let Setup { env, config } = setup;
    let manager = env.manager();
    let mut tools = Vec::new();
    let mut missing = Vec::new();
    let targets = || vec![Arg::Targets];

    // Format and lint.
    let ruff = env.tool(RUFF_REF);
    if let Some(inv) = ruff.clone() {
        tools.push(tool(
            RUFF_FORMAT,
            CheckKind::Format,
            spec(
                inv.clone(),
                &["format", "--force-exclude"],
                targets(),
                root,
                FORMAT_TIMEOUT,
            ),
        ));
        tools.push(tool(
            RUFF,
            CheckKind::Lint,
            spec(
                inv,
                &[
                    "check",
                    "--output-format",
                    "json",
                    "--force-exclude",
                    "--no-fix",
                ],
                targets(),
                root,
                LINT_TIMEOUT,
            ),
        ));
    } else {
        if config.ruff_configured() {
            missing.push(missing_tool("ruff", CheckKind::Lint, manager));
        }
        let black = env
            .tool(BLACK_REF)
            .filter(|_| config.black_configured() || config.declares("black"));
        if let Some(inv) = black {
            tools.push(tool(
                BLACK,
                CheckKind::Format,
                spec(inv, &[], targets(), root, FORMAT_TIMEOUT),
            ));
        }
    }

    // Type checking.
    match (env.tool(MYPY_REF), config.mypy()) {
        (Some(inv), configured) if configured.is_some() || config.declares("mypy") => {
            let json = env.version("mypy").is_some_and(|v| v >= MYPY_JSON);
            let mut args = Vec::new();
            if json {
                args.extend(["-O", "json"]);
            }
            args.extend([
                "--show-column-numbers",
                "--show-error-codes",
                "--no-error-summary",
                "--no-color-output",
            ]);
            if configured == Some(MypyConfig::ToxIni) {
                args.extend(["--config-file", "tox.ini"]);
            }
            tools.push(tool(
                MYPY,
                CheckKind::Typecheck,
                spec(inv, &args, targets(), root, MYPY_TIMEOUT),
            ));
        }
        (None, Some(_)) => missing.push(missing_tool("mypy", CheckKind::Typecheck, manager)),
        _ => {}
    }

    // Architecture.
    if config.importlinter_configured() {
        match env.tool(IMPORT_LINTER_REF) {
            Some(inv) => {
                let inv = if layout::has_src_layout(root) {
                    inv.with_env("PYTHONPATH", "src")
                } else {
                    inv
                };
                tools.push(tool(
                    IMPORT_LINTER,
                    CheckKind::Architecture,
                    spec(inv, &[], vec![], root, IMPORT_LINTER_TIMEOUT),
                ));
            }
            None => missing.push(missing_tool(
                "import-linter",
                CheckKind::Architecture,
                manager,
            )),
        }
    }

    // Tests and mutation testing.
    let pytest = env.tool(PYTEST_REF);
    if let Some(inv) = pytest.clone() {
        let mut args = Vec::new();
        let mut tail = Vec::new();
        if setup.testmon() {
            args.push("--testmon");
        } else {
            tail = targets();
        }
        args.extend(["-rfE", "--tb=short", "-q", "--color=no", "--rootdir=."]);
        tools.push(tool(
            PYTEST,
            CheckKind::Test,
            spec(inv, &args, tail, root, PYTEST_TIMEOUT),
        ));
    }
    let too_old = env
        .version("mutmut")
        .is_some_and(|(major, _)| major < MUTMUT_MIN_MAJOR);
    match env.tool(MUTMUT_REF) {
        Some(inv) if !too_old => tools.push(tool(
            MUTMUT,
            CheckKind::Mutation,
            spec(inv, &["run"], targets(), root, MUTMUT_TIMEOUT),
        )),
        Some(_) => missing.push(MissingTool::new(
            "mutmut",
            CheckKind::Mutation,
            format!(
                "mutmut {MUTMUT_MIN_MAJOR} or newer is required; upgrade with `{}`",
                manager.install_hint("'mutmut>=3'")
            ),
        )),
        None if pytest.is_some() => {
            missing.push(missing_tool("mutmut", CheckKind::Mutation, manager));
        }
        None => {}
    }

    Toolchain::new(tools).with_missing(missing)
}

fn missing_tool(package: &str, kind: CheckKind, manager: Manager) -> MissingTool {
    MissingTool::new(package, kind, manager.install_hint(package))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {

    use crate::ecosystems::python::fixture::Scratch;

    use super::*;

    /// A project with a venv holding `tools` (and `site` dist-infos), plus `files`.
    fn project(
        name: &str,
        tools: &[&str],
        dists: &[&str],
        files: &[(&str, &str)],
    ) -> (Scratch, Toolchain) {
        let s = Scratch::new(name);
        s.write(".venv/pyvenv.cfg", "");
        for t in tools {
            s.executable(&format!(".venv/bin/{t}"));
        }
        for d in dists {
            s.write(
                &format!(".venv/lib/python3.12/site-packages/{d}.dist-info/METADATA"),
                "",
            );
        }
        for (f, text) in files {
            s.write(f, text);
        }
        let tc = toolchain(&Setup::with_path(s.root(), vec![]), s.root());
        (s, tc)
    }

    fn names(tc: &Toolchain) -> Vec<&str> {
        tc.tools().iter().map(|t| t.name.as_str()).collect()
    }

    fn argv(tc: &Toolchain, name: &str, targets: &[&str]) -> (String, Vec<String>) {
        let t = tc.tools().iter().find(|t| t.name == name).unwrap();
        let targets: Vec<String> = targets.iter().map(|s| (*s).to_string()).collect();
        (t.command.program.clone(), t.command.expand_args(&targets))
    }

    #[test]
    fn full_toolchain_in_a_src_layout_venv() {
        let (s, tc) = project(
            "tc-full",
            &["ruff", "mypy", "lint-imports", "pytest", "mutmut"],
            &["mypy-1.11.2", "mutmut-3.8.1"],
            &[
                (
                    "pyproject.toml",
                    "[tool.mypy]\nstrict = true\n[tool.importlinter]\nroot_package = \"shop\"\n",
                ),
                ("src/shop/__init__.py", ""),
            ],
        );
        assert_eq!(
            names(&tc),
            [
                "ruff-format",
                "ruff",
                "mypy",
                "import-linter",
                "pytest",
                "mutmut"
            ]
        );
        assert!(tc.missing().is_empty());
        let bin = |t: &str| {
            s.root()
                .join(".venv/bin")
                .join(t)
                .to_string_lossy()
                .into_owned()
        };

        assert_eq!(
            argv(&tc, "ruff-format", &["a.py"]),
            (
                bin("ruff"),
                ["format", "--force-exclude", "a.py"]
                    .map(String::from)
                    .to_vec()
            )
        );
        assert_eq!(
            argv(&tc, "ruff", &["a.py"]).1,
            [
                "check",
                "--output-format",
                "json",
                "--force-exclude",
                "--no-fix",
                "a.py"
            ]
        );
        assert_eq!(
            argv(&tc, "mypy", &["a.py"]).1,
            [
                "-O",
                "json",
                "--show-column-numbers",
                "--show-error-codes",
                "--no-error-summary",
                "--no-color-output",
                "a.py"
            ]
        );
        // The src layout needs src on the import path for import-linter.
        let (program, args) = argv(&tc, "import-linter", &["a.py"]);
        assert_eq!(program, "env");
        assert_eq!(args, ["PYTHONPATH=src".to_string(), bin("lint-imports")]);
        assert_eq!(
            argv(&tc, "pytest", &["tests/test_a.py"]).1,
            [
                "-rfE",
                "--tb=short",
                "-q",
                "--color=no",
                "--rootdir=.",
                "tests/test_a.py"
            ]
        );
        assert_eq!(
            argv(&tc, "mutmut", &["shop.a.x[_ǁ]*"]).1,
            ["run", "shop.a.x[_ǁ]*"]
        );
        let timeouts: Vec<u64> = tc
            .tools()
            .iter()
            .map(|t| t.command.timeout.as_secs())
            .collect();
        assert_eq!(timeouts, [30, 30, 120, 60, 300, 480]);
        assert!(tc.tools().iter().all(|t| t.command.cwd == s.root()));
    }

    #[test]
    fn flat_layout_runs_import_linter_directly() {
        let (_s, tc) = project(
            "tc-flat",
            &["lint-imports"],
            &[],
            &[
                (
                    "pyproject.toml",
                    "[tool.importlinter]\nroot_package = \"shop\"\n",
                ),
                ("shop/__init__.py", ""),
            ],
        );
        assert!(argv(&tc, "import-linter", &[]).0.ends_with("/lint-imports"));
    }

    #[test]
    fn black_only_when_there_is_no_ruff_and_it_is_configured_or_declared() {
        let (_s, tc) = project(
            "tc-black",
            &["black"],
            &[],
            &[("pyproject.toml", "[tool.black]\n")],
        );
        assert_eq!(names(&tc), ["black"]);
        assert_eq!(argv(&tc, "black", &["a.py"]).1, ["a.py"]);
        let (_s, tc) = project(
            "tc-black-dep",
            &["black"],
            &[],
            &[("pyproject.toml", "[dependency-groups]\ndev = [\"black\"]\n")],
        );
        assert_eq!(names(&tc), ["black"]);
        // Installed but neither configured nor declared: not the project's tool.
        let (_s, tc) = project("tc-black-stray", &["black"], &[], &[("pyproject.toml", "")]);
        assert!(tc.tools().is_empty());
        // Ruff wins over black.
        let (_s, tc) = project(
            "tc-ruff-wins",
            &["black", "ruff"],
            &[],
            &[("pyproject.toml", "[tool.black]\n")],
        );
        assert_eq!(names(&tc), ["ruff-format", "ruff"]);
    }

    #[test]
    fn mypy_needs_configuration_or_a_declaration() {
        let (_s, tc) = project("tc-mypy-none", &["mypy"], &[], &[("pyproject.toml", "")]);
        assert!(tc.tools().is_empty());
        let (_s, tc) = project(
            "tc-mypy-dep",
            &["mypy"],
            &[],
            &[(
                "pyproject.toml",
                "[project.optional-dependencies]\ndev = [\"mypy>=1\"]\n",
            )],
        );
        assert_eq!(names(&tc), ["mypy"]);
    }

    #[test]
    fn mypy_json_needs_a_known_new_version() {
        let cfg = [("pyproject.toml", "[tool.mypy]\n")];
        let (_s, tc) = project("tc-mypy-old", &["mypy"], &["mypy-1.10.1"], &cfg);
        assert_eq!(argv(&tc, "mypy", &[]).1[0], "--show-column-numbers");
        let (_s, tc) = project("tc-mypy-unknown", &["mypy"], &[], &cfg);
        assert_eq!(argv(&tc, "mypy", &[]).1[0], "--show-column-numbers");
        let (_s, tc) = project("tc-mypy-new", &["mypy"], &["mypy-2.4.1"], &cfg);
        assert_eq!(&argv(&tc, "mypy", &[]).1[..2], ["-O", "json"]);
    }

    #[test]
    fn mypy_configured_only_in_tox_ini_gets_the_config_file() {
        let (_s, tc) = project(
            "tc-mypy-tox",
            &["mypy"],
            &[],
            &[("tox.ini", "[mypy]\nstrict = True\n")],
        );
        let args = argv(&tc, "mypy", &["a.py"]).1;
        assert!(args.windows(2).any(|w| w == ["--config-file", "tox.ini"]));
    }

    #[test]
    fn testmon_replaces_file_arguments() {
        let (_s, tc) = project("tc-testmon", &["pytest"], &["pytest_testmon-2.3.0"], &[]);
        assert_eq!(
            argv(&tc, "pytest", &["tests/test_a.py"]).1,
            [
                "--testmon",
                "-rfE",
                "--tb=short",
                "-q",
                "--color=no",
                "--rootdir=."
            ]
        );
    }

    #[test]
    fn missing_mutmut_is_reported_with_a_manager_matched_hint() {
        let (_s, tc) = project("tc-nomut", &["pytest"], &[], &[("uv.lock", "")]);
        assert_eq!(
            tc.missing(),
            [MissingTool::new(
                "mutmut",
                CheckKind::Mutation,
                "uv add --dev mutmut"
            )]
        );
        let (_s, tc) = project("tc-nomut-poetry", &["pytest"], &[], &[("poetry.lock", "")]);
        assert_eq!(tc.missing()[0].hint, "poetry add --group dev mutmut");
        let (_s, tc) = project("tc-nomut-pip", &["pytest"], &[], &[]);
        assert_eq!(tc.missing()[0].hint, "pip install mutmut");
        // Without pytest there is nothing to mutate against.
        let (_s, tc) = project("tc-nopytest", &[], &[], &[]);
        assert!(tc.is_empty());
    }

    #[test]
    fn mutmut_2_is_unsupported() {
        let (_s, tc) = project("tc-mut2", &["pytest", "mutmut"], &["mutmut-2.5.1"], &[]);
        assert!(!names(&tc).contains(&"mutmut"));
        assert!(tc.missing()[0].hint.contains("mutmut 3 or newer"));
    }

    #[test]
    fn configured_but_absent_tools_are_missing() {
        let (_s, tc) = project(
            "tc-cfg-missing",
            &[],
            &[],
            &[(
                "pyproject.toml",
                "[tool.ruff]\n[tool.mypy]\n[tool.importlinter]\n",
            )],
        );
        let kinds: Vec<(&str, CheckKind)> = tc
            .missing()
            .iter()
            .map(|m| (m.name.as_str(), m.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                ("ruff", CheckKind::Lint),
                ("mypy", CheckKind::Typecheck),
                ("import-linter", CheckKind::Architecture)
            ]
        );
    }

    #[test]
    fn uv_project_without_a_venv_uses_path_tools_and_uv_hints() {
        let s = Scratch::new("tc-uv");
        s.write("uv.lock", "[[package]]\nname = \"ruff\"\n");
        s.executable("bin/ruff");
        s.executable("bin/pytest");
        let setup = Setup::with_path(s.root(), vec![s.root().join("bin")]);
        let tc = toolchain(&setup, s.root());
        assert_eq!(names(&tc), ["ruff-format", "ruff", "pytest"]);
        assert_eq!(
            argv(&tc, "ruff", &[]).0,
            s.root().join("bin/ruff").to_string_lossy()
        );
        assert_eq!(tc.missing()[0].hint, "uv add --dev mutmut");
    }

    #[test]
    fn poetry_environment_runs_tools_through_poetry() {
        let s = Scratch::new("tc-poetry");
        s.write(
            "poetry.lock",
            "[[package]]\nname = \"ruff\"\n\n[[package]]\nname = \"pytest\"\n",
        );
        s.executable("bin/poetry");
        let setup = Setup::with_path(s.root(), vec![s.root().join("bin")]);
        let tc = toolchain(&setup, s.root());
        assert_eq!(names(&tc), ["ruff-format", "ruff", "pytest"]);
        let (program, args) = argv(&tc, "ruff", &["a.py"]);
        assert_eq!(program, "poetry");
        assert_eq!(&args[..2], ["run", "ruff"]);
        assert_eq!(tc.missing()[0].hint, "poetry add --group dev mutmut");
    }

    #[test]
    fn path_environment_resolves_tools_on_path() {
        let s = Scratch::new("tc-path");
        s.executable("bin/ruff");
        let setup = Setup::with_path(s.root(), vec![s.root().join("bin")]);
        let tc = toolchain(&setup, s.root());
        assert_eq!(names(&tc), ["ruff-format", "ruff"]);
    }
}
