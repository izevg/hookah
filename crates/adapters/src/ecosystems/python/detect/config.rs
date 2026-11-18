//! Tool configuration of a Python project.
//!
//! Reads `pyproject.toml`, `setup.cfg`, and `tox.ini` as text, and notes which dedicated config
//! files exist (`ruff.toml`, `.ruff.toml`, `mypy.ini`, `.mypy.ini`, `.importlinter`). No TOML or
//! INI parser is needed: only table headers and the names of declared dependencies matter, and
//! both are recognized line by line. A file that cannot be read counts as empty.

use std::fs;
use std::path::Path;

use super::markers;

/// How mypy finds its configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MypyConfig {
    /// mypy discovers it by itself (`mypy.ini`, `.mypy.ini`, `pyproject.toml`, `setup.cfg`).
    Auto,
    /// Only `tox.ini` has a `[mypy]` section, so it must be passed with `--config-file`.
    ToxIni,
}

/// The configuration facts of one project.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pyproject: String,
    setup_cfg: String,
    tox_ini: String,
    pytest_ini: Option<String>,
    mypy_ini: String,
    mypy_ini_exists: bool,
    ruff_toml: bool,
    importlinter: bool,
}

impl Config {
    /// Reads the configuration files in `root`.
    pub fn load(root: &Path) -> Self {
        let read = |name: &str| fs::read_to_string(root.join(name)).unwrap_or_default();
        let exists = |names: &[&str]| names.iter().any(|n| root.join(n).is_file());
        Self {
            pyproject: read("pyproject.toml"),
            setup_cfg: read("setup.cfg"),
            tox_ini: read("tox.ini"),
            pytest_ini: root
                .join("pytest.ini")
                .is_file()
                .then(|| read("pytest.ini")),
            mypy_ini: format!("{}{}", read("mypy.ini"), read(".mypy.ini")),
            mypy_ini_exists: exists(&["mypy.ini", ".mypy.ini"]),
            ruff_toml: exists(&["ruff.toml", ".ruff.toml"]),
            importlinter: exists(&[".importlinter"]),
        }
    }

    /// Ruff has a configuration (`[tool.ruff]` or a ruff config file).
    pub fn ruff_configured(&self) -> bool {
        self.ruff_toml || has_table(&self.pyproject, "tool.ruff")
    }

    /// `[tool.black]` exists.
    pub fn black_configured(&self) -> bool {
        has_table(&self.pyproject, "tool.black")
    }

    /// How mypy is configured, or `None` when it is not.
    pub fn mypy(&self) -> Option<MypyConfig> {
        if self.mypy_ini_exists
            || has_table(&self.pyproject, "tool.mypy")
            || has_section(&self.setup_cfg, "mypy")
        {
            Some(MypyConfig::Auto)
        } else if has_section(&self.tox_ini, "mypy") {
            Some(MypyConfig::ToxIni)
        } else {
            None
        }
    }

    /// Whether mypy's own configuration names what to check (`files`, `packages`, or `modules`
    /// in `[tool.mypy]`, or in a `[mypy]` section of `mypy.ini`, `.mypy.ini`, `setup.cfg`, or
    /// `tox.ini`). Without arguments mypy then checks exactly that.
    pub fn mypy_names_targets(&self) -> bool {
        const KEYS: &[&str] = &["files", "packages", "modules"];
        section_sets(&self.pyproject, "tool.mypy", KEYS)
            || [&self.mypy_ini, &self.setup_cfg, &self.tox_ini]
                .iter()
                .any(|text| section_sets(text, "mypy", KEYS))
    }

    /// The pytest `-m` expression that deselects the heavy markers (`integration`, `e2e`,
    /// `slow`) the project registers, or `None` when it registers none. pytest reads one config
    /// source: `pytest.ini` (even without a section), else `pyproject.toml` with
    /// `[tool.pytest.ini_options]`, else `tox.ini` with `[pytest]`, else `setup.cfg` with
    /// `[tool:pytest]`.
    pub fn heavy_marker_expression(&self) -> Option<String> {
        let registered = if let Some(ini) = &self.pytest_ini {
            markers::registered(ini, "pytest", false)
        } else if has_table(&self.pyproject, "tool.pytest.ini_options") {
            markers::registered(&self.pyproject, "tool.pytest.ini_options", true)
        } else if has_section(&self.tox_ini, "pytest") {
            markers::registered(&self.tox_ini, "pytest", false)
        } else {
            markers::registered(&self.setup_cfg, "tool:pytest", false)
        };
        markers::deselect_expression(&registered)
    }

    /// import-linter has a configuration.
    pub fn importlinter_configured(&self) -> bool {
        self.importlinter
            || has_table(&self.pyproject, "tool.importlinter")
            || has_section(&self.setup_cfg, "importlinter")
    }

    /// Whether `package` is named as a dependency in `pyproject.toml`, `setup.cfg`, or
    /// `tox.ini`, in any group (project, optional, dependency group, Poetry, uv, extras).
    pub fn declares(&self, package: &str) -> bool {
        let wanted = normalize(package);
        [&self.pyproject, &self.setup_cfg, &self.tox_ini]
            .iter()
            .flat_map(|text| text.lines())
            .any(|line| names_in(line).iter().any(|n| normalize(n) == wanted))
    }
}

/// The canonical form of a package name: lowercase, with `_` and `.` as `-`.
pub fn normalize(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| match c {
            '_' | '.' => '-',
            c => c.to_ascii_lowercase(),
        })
        .collect()
}

fn headers(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter_map(|line| {
        let line = line.trim();
        let inner = line.strip_prefix('[')?;
        let inner = inner.strip_prefix('[').unwrap_or(inner);
        let end = inner.find(']')?;
        Some(inner[..end].trim())
    })
}

/// A TOML table `name` or a sub-table of it (`tool.mypy`, `tool.mypy.overrides`).
fn has_table(text: &str, name: &str) -> bool {
    headers(text).any(|h| h == name || h.strip_prefix(name).is_some_and(|r| r.starts_with('.')))
}

/// An INI section exactly named `name`.
fn has_section(text: &str, name: &str) -> bool {
    headers(text).any(|h| h == name)
}

/// Whether the table or section `name` of `text` sets one of `keys` (`key = ...`). Only the
/// section itself counts, not its sub-tables such as `tool.mypy.overrides`.
fn section_sets(text: &str, name: &str, keys: &[&str]) -> bool {
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if let Some(inner) = line.strip_prefix('[')
            && line.ends_with(']')
            && !inner.contains('"')
        {
            let inner = inner.strip_prefix('[').unwrap_or(inner);
            inside = inner.trim_end_matches(']').trim() == name;
        } else if inside
            && let Some((key, _)) = line.split_once('=')
            && keys.contains(&key.trim())
        {
            return true;
        }
    }
    false
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
}

pub(super) fn leading_name(text: &str) -> &str {
    let end = text.find(|c| !is_name_char(c)).unwrap_or(text.len());
    &text[..end]
}

/// Package names that may be dependency declarations on one line: the start of each quoted
/// string (`"ruff>=0.4"`), and the start of the line itself (a Poetry key, or a bare INI
/// requirement).
fn names_in(line: &str) -> Vec<&str> {
    let line = line.trim();
    if line.starts_with('#') || line.starts_with('[') {
        return vec![];
    }
    let mut names = vec![leading_name(line)];
    names.extend(line.split(['"', '\'']).skip(1).map(leading_name));
    names.retain(|n| !n.is_empty());
    names
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use crate::ecosystems::python::fixture::Scratch;

    use super::*;

    fn load(files: &[(&str, &str)]) -> Config {
        let s = Scratch::new("config");
        for (name, text) in files {
            s.write(name, text);
        }
        Config::load(s.root())
    }

    #[test]
    fn heavy_markers_come_from_the_one_config_pytest_reads() {
        let expr = |files: &[(&str, &str)]| load(files).heavy_marker_expression();
        let toml = "[tool.pytest.ini_options]\nmarkers = [\"slow: s\", \"unit\"]\n";
        assert_eq!(
            expr(&[("pyproject.toml", toml)]).as_deref(),
            Some("not (slow)")
        );
        // pytest.ini wins over pyproject.toml, even without markers.
        assert_eq!(expr(&[("pyproject.toml", toml), ("pytest.ini", "")]), None);
        assert_eq!(
            expr(&[("pytest.ini", "[pytest]\nmarkers =\n  e2e: x\n")]).as_deref(),
            Some("not (e2e)")
        );
        assert_eq!(
            expr(&[("tox.ini", "[pytest]\nmarkers =\n  integration\n")]).as_deref(),
            Some("not (integration)")
        );
        assert_eq!(
            expr(&[("setup.cfg", "[tool:pytest]\nmarkers =\n  slow\n")]).as_deref(),
            Some("not (slow)")
        );
        // A heavy marker in a config pytest does not read counts for nothing.
        assert_eq!(
            expr(&[
                ("pyproject.toml", "[tool.pytest.ini_options]\n"),
                ("setup.cfg", "[tool:pytest]\nmarkers =\n  slow\n")
            ]),
            None
        );
        assert_eq!(expr(&[]), None);
    }

    #[test]
    fn mypy_names_targets_only_in_its_own_section() {
        let names = |files: &[(&str, &str)]| load(files).mypy_names_targets();
        assert!(names(&[(
            "pyproject.toml",
            "[tool.mypy]\nfiles = [\"src\"]\n"
        )]));
        assert!(names(&[(
            "pyproject.toml",
            "[tool.mypy]\nstrict = true\npackages=[\"a\"]\n"
        )]));
        assert!(names(&[("mypy.ini", "[mypy]\nmodules = a\n")]));
        assert!(names(&[(".mypy.ini", "[mypy]\nfiles = src\n")]));
        assert!(names(&[("setup.cfg", "[mypy]\nfiles = src\n")]));
        assert!(names(&[("tox.ini", "[mypy]\nfiles = src\n")]));
        assert!(!names(&[(
            "pyproject.toml",
            "[tool.mypy]\nstrict = true\n"
        )]));
        assert!(!names(&[(
            "pyproject.toml",
            "[tool.mypy]\nstrict = true\n[tool.other]\nfiles = 1\n"
        )]));
        assert!(!names(&[(
            "pyproject.toml",
            "[[tool.mypy.overrides]]\nmodule = \"a\"\nfiles = 1\n"
        )]));
        assert!(!names(&[(
            "pyproject.toml",
            "[tool.mypy]\nfilesystem = 1\n"
        )]));
        assert!(!names(&[("setup.cfg", "[flake8]\nfiles = 1\n")]));
        assert!(!names(&[]));
    }

    #[test]
    fn empty_project_configures_nothing() {
        let c = load(&[]);
        assert!(!c.ruff_configured() && !c.black_configured());
        assert!(c.mypy().is_none() && !c.importlinter_configured());
        assert!(!c.declares("ruff"));
    }

    #[test]
    fn pyproject_tables() {
        let c = load(&[(
            "pyproject.toml",
            "[tool.ruff.lint]\nselect=[\"E\"]\n[tool.black]\n[tool.mypy]\nstrict=true\n[tool.importlinter]\nroot_package=\"a\"\n",
        )]);
        assert!(c.ruff_configured() && c.black_configured());
        assert_eq!(c.mypy(), Some(MypyConfig::Auto));
        assert!(c.importlinter_configured());
    }

    #[test]
    fn similarly_named_tables_do_not_match() {
        let c = load(&[(
            "pyproject.toml",
            "[tool.mypy-plugin]\n[tool.ruffian]\n[tool.blackish]\n",
        )]);
        assert!(!c.ruff_configured() && !c.black_configured() && c.mypy().is_none());
    }

    #[test]
    fn dedicated_config_files() {
        assert!(load(&[(".ruff.toml", "")]).ruff_configured());
        assert!(load(&[("ruff.toml", "")]).ruff_configured());
        assert_eq!(load(&[(".mypy.ini", "")]).mypy(), Some(MypyConfig::Auto));
        assert!(load(&[(".importlinter", "")]).importlinter_configured());
    }

    #[test]
    fn setup_cfg_and_tox_ini() {
        let c = load(&[("setup.cfg", "[mypy]\nstrict = True\n[importlinter]\n")]);
        assert_eq!(c.mypy(), Some(MypyConfig::Auto));
        assert!(c.importlinter_configured());
        let c = load(&[("tox.ini", "[tox]\n[mypy]\nstrict = True\n")]);
        assert_eq!(c.mypy(), Some(MypyConfig::ToxIni));
        // A per-module section alone does not configure mypy.
        assert!(load(&[("setup.cfg", "[mypy-foo.*]\n")]).mypy().is_none());
    }

    #[test]
    fn declared_dependencies_in_every_style() {
        let c = load(&[(
            "pyproject.toml",
            r#"[project]
dependencies = ["requests>=2"]
[project.optional-dependencies]
dev = ["Ruff >= 0.4", "pytest-testmon"]
[dependency-groups]
lint = ['mypy[faster-cache]==1.10']
[tool.poetry.group.dev.dependencies]
import_linter = "^2.0"
black = { version = "24" }
# mutmut = "3"
"#,
        )]);
        for dep in [
            "requests",
            "ruff",
            "pytest-testmon",
            "mypy",
            "import-linter",
            "black",
        ] {
            assert!(c.declares(dep), "{dep}");
        }
        assert!(!c.declares("mutmut"), "comments do not count");
        assert!(!c.declares("pytest"), "a prefix is not the package");
    }

    #[test]
    fn declared_in_setup_cfg_and_tox() {
        let c = load(&[
            (
                "setup.cfg",
                "[options.extras_require]\ndev =\n    mypy>=1\n",
            ),
            ("tox.ini", "[testenv]\ndeps =\n    pytest\n    mutmut\n"),
        ]);
        assert!(c.declares("mypy") && c.declares("pytest") && c.declares("mutmut"));
    }

    #[test]
    fn comments_and_table_headers_declare_nothing() {
        assert!(names_in("# \"ruff>=0.4\"").is_empty());
        assert!(names_in("[tool.poetry.dependencies]").is_empty());
        assert!(names_in("[\"ruff\"]").is_empty());
        assert_eq!(names_in("  ruff = \"^0.4\""), ["ruff"]);
    }

    #[test]
    fn normalizes_names() {
        assert_eq!(normalize("Import_Linter"), "import-linter");
        assert_eq!(normalize("zope.interface"), "zope-interface");
    }
}
