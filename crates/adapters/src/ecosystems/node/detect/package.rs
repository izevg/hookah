//! What the project's `package.json`, lockfile, and `node_modules` say.
//!
//! [`PackageInfo`] answers three questions for the detector:
//!
//! - which tools the project *declares*: the union of `dependencies` and `devDependencies` of the
//!   package and of its workspace root;
//! - where a tool's binary is: the first `node_modules/.bin/<name>` found walking up from the
//!   package to the workspace root, so hoisted installs (pnpm, yarn, npm workspaces) resolve;
//! - which package manager the project uses, from the nearest lockfile (`pnpm-lock.yaml`,
//!   `yarn.lock`, `bun.lock`/`bun.lockb`, `package-lock.json`), else the `packageManager` field.
//!
//! The workspace root is the nearest ancestor directory (the package itself included) that holds
//! a `pnpm-workspace.yaml` or a `package.json` with a `workspaces` field. The search stops at the
//! first directory that contains `.git`, so a stray manifest above the repository is never taken
//! for a workspace. Without a workspace root the package is its own root.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use hookah_application::error::PortError;
use serde_json::Value;

/// The package manager of a project. Used for install hints only; tools run from `.bin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    /// `npm`, also the default when nothing says otherwise.
    Npm,
    /// `pnpm`.
    Pnpm,
    /// `yarn`.
    Yarn,
    /// `bun`.
    Bun,
}

impl PackageManager {
    /// The command that adds development dependencies, such as `pnpm add -D`.
    pub fn add_dev(self) -> &'static str {
        match self {
            Self::Npm => "npm i -D",
            Self::Pnpm => "pnpm add -D",
            Self::Yarn => "yarn add -D",
            Self::Bun => "bun add -d",
        }
    }

    /// The command that installs the declared dependencies.
    pub fn install(self) -> &'static str {
        match self {
            Self::Npm => "npm install",
            Self::Pnpm => "pnpm install",
            Self::Yarn => "yarn install",
            Self::Bun => "bun install",
        }
    }

    fn from_lockfile(dir: &Path) -> Option<Self> {
        let has = |name: &str| dir.join(name).is_file();
        if has("pnpm-lock.yaml") {
            Some(Self::Pnpm)
        } else if has("yarn.lock") {
            Some(Self::Yarn)
        } else if has("bun.lock") || has("bun.lockb") {
            Some(Self::Bun)
        } else if has("package-lock.json") || has("npm-shrinkwrap.json") {
            Some(Self::Npm)
        } else {
            None
        }
    }

    /// Reads the `packageManager` field, such as `pnpm@9.1.0`.
    fn from_field(manifest: &Value) -> Option<Self> {
        let field = manifest.get("packageManager")?.as_str()?;
        match field.split('@').next()? {
            "pnpm" => Some(Self::Pnpm),
            "yarn" => Some(Self::Yarn),
            "bun" => Some(Self::Bun),
            "npm" => Some(Self::Npm),
            _ => None,
        }
    }
}

/// The facts about one package that tool detection needs.
#[derive(Debug, Clone)]
pub struct PackageInfo {
    /// The package directory: the project root.
    pub root: PathBuf,
    /// The workspace root; equals `root` outside a workspace.
    pub workspace_root: PathBuf,
    /// The package manager.
    pub manager: PackageManager,
    /// The `scripts.test` command of the package, if any.
    pub test_script: Option<String>,
    /// The `name` of the package, if any.
    pub name: Option<String>,
    /// The `version` of the package, if any.
    pub version: Option<String>,
    /// The other workspace packages that depend on this one (see [`super::workspace`]), each read
    /// without its own dependents.
    pub dependents: Vec<PackageInfo>,
    declared: BTreeSet<String>,
}

impl PackageInfo {
    /// Reads the package at `root` (the directory of its `package.json`) and finds the workspace
    /// packages that depend on it.
    pub fn read(root: &Path) -> Result<Self, PortError> {
        let mut info = Self::read_alone(root)?;
        info.dependents = super::workspace::dependents(&info)
            .iter()
            .filter_map(|dir| Self::read_alone(dir).ok())
            .collect();
        Ok(info)
    }

    /// Reads the package at `root` without looking for its dependents.
    fn read_alone(root: &Path) -> Result<Self, PortError> {
        let manifest = read_manifest(&root.join("package.json"))?;
        let workspace_root = find_workspace_root(root);
        let root_manifest = if workspace_root == root {
            None
        } else {
            read_manifest(&workspace_root.join("package.json")).ok()
        };

        let mut declared = BTreeSet::new();
        for m in std::iter::once(&manifest).chain(root_manifest.as_ref()) {
            for section in ["dependencies", "devDependencies"] {
                if let Some(deps) = m.get(section).and_then(Value::as_object) {
                    declared.extend(deps.keys().cloned());
                }
            }
        }

        let mut info = Self {
            root: root.to_path_buf(),
            workspace_root,
            manager: PackageManager::Npm,
            test_script: manifest
                .get("scripts")
                .and_then(|s| s.get("test"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            name: string_field(&manifest, "name"),
            version: string_field(&manifest, "version"),
            dependents: Vec::new(),
            declared,
        };
        let manager = info
            .dirs()
            .find_map(PackageManager::from_lockfile)
            .or_else(|| PackageManager::from_field(&manifest))
            .or_else(|| root_manifest.as_ref().and_then(PackageManager::from_field))
            .unwrap_or(PackageManager::Npm);
        info.manager = manager;
        Ok(info)
    }

    /// The directories from the package up to the workspace root, nearest first.
    pub fn dirs(&self) -> impl Iterator<Item = &Path> {
        self.root
            .ancestors()
            .take_while(|dir| dir.starts_with(&self.workspace_root))
    }

    /// Whether `package` is in the dependencies of the package or of the workspace root.
    pub fn declares(&self, package: &str) -> bool {
        self.declared.contains(package)
    }

    /// The first `node_modules/.bin/<name>` found from the package up to the workspace root. On
    /// Windows that is the `.cmd` shim the package manager writes.
    pub fn bin(&self, name: &str) -> Option<PathBuf> {
        self.dirs()
            .flat_map(|dir| {
                bin_names(name, cfg!(windows))
                    .into_iter()
                    .map(move |n| dir.join("node_modules").join(".bin").join(n))
            })
            .find(|path| path.is_file())
    }

    /// The installed version of `package`, from its `package.json` in `node_modules`.
    pub fn installed_version(&self, package: &str) -> Option<String> {
        self.dirs().find_map(|dir| {
            let manifest = dir.join("node_modules").join(package).join("package.json");
            let value = read_manifest(&manifest).ok()?;
            Some(value.get("version")?.as_str()?.to_owned())
        })
    }

    /// The first existing file called one of `names` in the package or any directory up to the
    /// workspace root.
    pub fn config_file(&self, names: &[&str]) -> Option<PathBuf> {
        self.dirs()
            .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
            .find(|path| path.is_file())
    }
}

fn string_field(manifest: &Value, key: &str) -> Option<String> {
    manifest.get(key)?.as_str().map(str::to_owned)
}

/// The file names a `.bin` entry can have. Unix links the script under its bare name. Windows
/// cannot run that name (it is a `sh` script), so package managers add `<name>.cmd` shims, and
/// std runs a `.cmd` through `cmd.exe` with its own argument escaping.
fn bin_names(name: &str, windows: bool) -> Vec<String> {
    if windows {
        vec![format!("{name}.cmd"), format!("{name}.exe")]
    } else {
        vec![name.to_string()]
    }
}

/// Reads and parses a `package.json`.
pub(super) fn read_manifest(path: &Path) -> Result<Value, PortError> {
    let text = fs::read_to_string(path)
        .map_err(|e| PortError::new(format!("cannot read {}: {e}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|e| PortError::new(format!("unreadable {}: {e}", path.display())))
}

fn find_workspace_root(package: &Path) -> PathBuf {
    for dir in package.ancestors() {
        if is_workspace_root(dir) {
            return dir.to_path_buf();
        }
        if dir.join(".git").exists() {
            break;
        }
    }
    package.to_path_buf()
}

fn is_workspace_root(dir: &Path) -> bool {
    dir.join("pnpm-workspace.yaml").is_file()
        || read_manifest(&dir.join("package.json"))
            .is_ok_and(|m| m.get("workspaces").is_some_and(|w| !w.is_null()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod scratch {
    //! A throwaway project directory for tests.

    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// A temporary directory removed on drop.
    pub struct Scratch(pub PathBuf);

    impl Scratch {
        pub fn new() -> Self {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("hookah-node-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dunce::canonicalize(dir).unwrap())
        }

        /// `rel` (written with `/`) under the root, joined one component at a time so the
        /// separators are the platform's.
        pub fn path(&self, rel: &str) -> PathBuf {
            rel.split('/')
                .filter(|part| !part.is_empty())
                .fold(self.0.clone(), |path, part| path.join(part))
        }

        /// Where `install` puts the `.bin` entry `name` of the package in `dir`: a `.cmd` shim on
        /// Windows, the bare name elsewhere.
        pub fn bin_path(&self, dir: &str, name: &str) -> PathBuf {
            let file = if cfg!(windows) {
                format!("{name}.cmd")
            } else {
                name.to_owned()
            };
            self.path(dir).join("node_modules").join(".bin").join(file)
        }

        /// Writes `rel` (creating directories) and returns its absolute path.
        pub fn write(&self, rel: &str, text: &str) -> PathBuf {
            let path = if Path::new(rel).is_absolute() {
                PathBuf::from(rel)
            } else {
                self.path(rel)
            };
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
            path
        }

        /// Installs a fake package: `node_modules/<name>/package.json` and, when `bin` is given,
        /// an executable-looking `node_modules/.bin/<bin>`, under `dir` (relative).
        pub fn install(&self, dir: &str, name: &str, version: &str, bin: Option<&str>) {
            self.write(
                &self
                    .path(dir)
                    .join("node_modules")
                    .join(name)
                    .join("package.json")
                    .to_string_lossy(),
                &format!(r#"{{"name":"{name}","version":"{version}"}}"#),
            );
            if let Some(bin) = bin {
                self.write(&self.bin_path(dir, bin).to_string_lossy(), "#!/bin/sh\n");
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::scratch::Scratch;
    use super::*;

    #[test]
    fn bin_names_per_platform() {
        assert_eq!(bin_names("biome", false), ["biome"]);
        assert_eq!(bin_names("biome", true), ["biome.cmd", "biome.exe"]);
    }

    #[test]
    fn standalone_package_declares_its_own_dependencies() {
        let s = Scratch::new();
        s.write(
            "package.json",
            r#"{"dependencies":{"a":"1"},"devDependencies":{"b":"2"},"scripts":{"test":"vitest"}}"#,
        );
        let info = PackageInfo::read(&s.0).unwrap();
        assert_eq!(info.workspace_root, s.0);
        assert!(info.declares("a") && info.declares("b") && !info.declares("c"));
        assert_eq!(info.test_script.as_deref(), Some("vitest"));
        assert_eq!(info.manager, PackageManager::Npm);
    }

    #[test]
    fn lockfile_picks_the_manager() {
        for (lock, manager) in [
            ("pnpm-lock.yaml", PackageManager::Pnpm),
            ("yarn.lock", PackageManager::Yarn),
            ("bun.lockb", PackageManager::Bun),
            ("bun.lock", PackageManager::Bun),
            ("package-lock.json", PackageManager::Npm),
            ("npm-shrinkwrap.json", PackageManager::Npm),
        ] {
            let s = Scratch::new();
            s.write("package.json", "{}");
            s.write(lock, "");
            assert_eq!(PackageInfo::read(&s.0).unwrap().manager, manager, "{lock}");
        }
    }

    #[test]
    fn either_npm_lockfile_beats_the_field() {
        for lock in ["package-lock.json", "npm-shrinkwrap.json"] {
            let s = Scratch::new();
            s.write("package.json", r#"{"packageManager":"pnpm@9.0.0"}"#);
            s.write(lock, "");
            assert_eq!(
                PackageInfo::read(&s.0).unwrap().manager,
                PackageManager::Npm,
                "{lock}"
            );
        }
    }

    #[test]
    fn package_manager_field_is_the_fallback() {
        let s = Scratch::new();
        s.write("package.json", r#"{"packageManager":"yarn@4.1.0"}"#);
        assert_eq!(
            PackageInfo::read(&s.0).unwrap().manager,
            PackageManager::Yarn
        );
    }

    #[test]
    fn package_manager_field_names_each_manager() {
        for (field, manager) in [
            ("pnpm@9.1.0", PackageManager::Pnpm),
            ("yarn@4.1.0", PackageManager::Yarn),
            ("bun@1.1.0", PackageManager::Bun),
            ("npm@10.2.0", PackageManager::Npm),
        ] {
            let manifest = serde_json::json!({ "packageManager": field });
            assert_eq!(
                PackageManager::from_field(&manifest),
                Some(manager),
                "{field}"
            );
        }
        let unknown = serde_json::json!({ "packageManager": "deno@2.0.0" });
        assert_eq!(PackageManager::from_field(&unknown), None);
        assert_eq!(PackageManager::from_field(&serde_json::json!({})), None);
    }

    #[test]
    fn pnpm_workspace_members_inherit_root_declarations_and_binaries() {
        let s = Scratch::new();
        s.write("pnpm-workspace.yaml", "packages:\n  - 'packages/*'\n");
        s.write("pnpm-lock.yaml", "");
        s.write("package.json", r#"{"devDependencies":{"eslint":"9"}}"#);
        s.write(
            "packages/app/package.json",
            r#"{"devDependencies":{"vitest":"2"}}"#,
        );
        s.install("", "eslint", "9.1.0", Some("eslint"));
        s.install("packages/app", "vitest", "2.0.0", Some("vitest"));
        let info = PackageInfo::read(&s.path("packages/app")).unwrap();
        assert_eq!(info.workspace_root, s.0);
        assert_eq!(info.manager, PackageManager::Pnpm);
        assert!(info.declares("eslint") && info.declares("vitest"));
        // The root binary is found by walking up; the package's own wins when both exist.
        assert_eq!(info.bin("eslint"), Some(s.bin_path("", "eslint")));
        assert_eq!(
            info.bin("vitest"),
            Some(s.bin_path("packages/app", "vitest"))
        );
        assert_eq!(info.bin("tsc"), None);
        assert_eq!(info.installed_version("eslint").as_deref(), Some("9.1.0"));
        assert_eq!(info.installed_version("nope"), None);
    }

    #[test]
    fn npm_and_yarn_workspaces_field_marks_the_root() {
        for field in [r#"["packages/*"]"#, r#"{"packages":["packages/*"]}"#] {
            let s = Scratch::new();
            s.write("package.json", &format!(r#"{{"workspaces":{field}}}"#));
            s.write("yarn.lock", "");
            s.write("packages/lib/package.json", "{}");
            let info = PackageInfo::read(&s.path("packages/lib")).unwrap();
            assert_eq!(info.workspace_root, s.0);
            // The lockfile lives at the workspace root.
            assert_eq!(info.manager, PackageManager::Yarn);
        }
    }

    #[test]
    fn the_workspace_search_stops_at_the_repository_boundary() {
        let s = Scratch::new();
        s.write(
            "package.json",
            r#"{"workspaces":["*"],"devDependencies":{"x":"1"}}"#,
        );
        s.write("repo/.git/HEAD", "");
        s.write("repo/package.json", "{}");
        let info = PackageInfo::read(&s.path("repo")).unwrap();
        assert_eq!(info.workspace_root, s.path("repo"));
        assert!(!info.declares("x"));
    }

    #[test]
    fn binaries_above_the_workspace_root_do_not_count() {
        let s = Scratch::new();
        s.install("", "eslint", "9.0.0", Some("eslint"));
        s.write("proj/package.json", "{}");
        let info = PackageInfo::read(&s.path("proj")).unwrap();
        assert_eq!(info.bin("eslint"), None);
    }

    #[test]
    fn config_files_are_found_up_to_the_workspace_root() {
        let s = Scratch::new();
        s.write("pnpm-workspace.yaml", "");
        s.write("package.json", "{}");
        s.write("biome.json", "{}");
        s.write("packages/app/package.json", "{}");
        let info = PackageInfo::read(&s.path("packages/app")).unwrap();
        assert_eq!(
            info.config_file(&["biome.jsonc", "biome.json"]),
            Some(s.path("biome.json"))
        );
        assert_eq!(info.config_file(&["eslint.config.js"]), None);
    }

    #[test]
    fn broken_manifests_are_port_errors() {
        let s = Scratch::new();
        s.write("package.json", "{ nope");
        assert!(PackageInfo::read(&s.0).is_err());
        let s = Scratch::new();
        assert!(PackageInfo::read(&s.0).is_err());
    }
}
