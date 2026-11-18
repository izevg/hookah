//! What the build files of a Java project say.
//!
//! A Java project root is the directory of the nearest `pom.xml`, `build.gradle(.kts)`, or
//! `settings.gradle(.kts)`: a module of a multi-module build, or the whole of a single-module one.
//! [`Build::load`] reads the build files (best effort: XML for Maven, plain text for the Groovy
//! and Kotlin DSLs of Gradle, nothing is ever executed) and answers:
//!
//! - which build tool drives the project, and which program runs it (the project's wrapper,
//!   `mvnw` or `gradlew`, when present, else `mvn` or `gradle` from `PATH`);
//! - where its commands run: the root of the Maven reactor or of the Gradle build, which is the
//!   module's own directory for a single-module project;
//! - how that root addresses the module (`-pl <path>` for Maven, `:<project>:<task>` for Gradle);
//! - which of the tools hookah knows are configured (see [`Features`]).
//!
//! Detection reads a handful of small files and costs well under a millisecond per file, so
//! nothing is cached.

pub mod gradle;
pub mod maven;
mod reactor;

use std::path::{Path, PathBuf};

use hookah_domain::path::slashed;

/// The build tool of a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildKind {
    /// Apache Maven.
    Maven,
    /// Gradle, with a Groovy or a Kotlin build script.
    Gradle,
}

/// The tools the build files configure.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Features {
    /// The Spotless plugin.
    pub spotless: bool,
    /// The Checkstyle plugin.
    pub checkstyle: bool,
    /// The PMD plugin.
    pub pmd: bool,
    /// The SpotBugs plugin.
    pub spotbugs: bool,
    /// SpotBugs writes an XML report (always true for Maven, which hookah asks for).
    pub spotbugs_xml: bool,
    /// The PIT mutation testing plugin.
    pub pitest: bool,
    /// PIT writes `mutations.xml` (always true for Maven, which hookah asks for).
    pub pitest_xml: bool,
    /// The project uses JUnit 5, so tags (`@Tag("integration")`) mean something.
    pub junit5: bool,
}

/// A Java project's build, seen from one module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    /// The build tool.
    pub kind: BuildKind,
    /// Where commands run: the reactor root or the Gradle settings directory.
    pub root: PathBuf,
    /// The module's own directory (the hookah project root).
    pub module_dir: PathBuf,
    /// Maven: the module path relative to the root, `None` when the module is the root. Gradle:
    /// the project path (`:app:core`, empty for the root project).
    pub selector: Option<String>,
    /// The program that runs the build: an absolute wrapper path, or `mvn`/`gradle` on `PATH`.
    pub program: String,
    /// The projects that depend on this one, directly or not. Gradle: project paths (`:app`).
    /// Maven: module directories relative to the root, within the budget (see
    /// [`maven::DOWNSTREAM_BUDGET`]); empty when there are none or when they are over it.
    pub dependents: Vec<String>,
    /// Maven: how many dependents were left out because they are over the budget.
    pub skipped_dependents: usize,
    /// The configured tools.
    pub features: Features,
}

impl Build {
    /// Reads the build files of the project at `module_dir`. `None` when there is none.
    pub fn load(module_dir: &Path) -> Option<Self> {
        if module_dir.join("pom.xml").is_file() {
            return maven::load(module_dir);
        }
        gradle::load(module_dir)
    }

    /// The Gradle task `name` of this module, such as `:app:compileJava`.
    pub fn task(&self, name: &str) -> String {
        format!("{}:{name}", self.selector.as_deref().unwrap_or_default())
    }

    /// The `-pl` selection of a Maven reactor build; empty for a single module. With `also`,
    /// the modules this one needs (`-am`) and the modules that need it are included.
    ///
    /// The dependents are named in `-pl` instead of asking for `-amd`: `-am` then adds what they
    /// need as well, so none of their reactor dependencies is looked up in `~/.m2`. For the same
    /// reason a run with dependents always has `-am`.
    pub fn reactor_args(&self, also: Also) -> Vec<String> {
        let Some(path) = &self.selector else {
            return vec![];
        };
        let mut modules = vec![path.as_str()];
        if also.downstream {
            modules.extend(self.dependents.iter().map(String::as_str));
        }
        let mut args = vec!["-pl".to_string(), modules.join(",")];
        if also.upstream || also.downstream {
            args.push("-am".into());
        }
        args
    }
}

/// Which neighbors of a module a Maven reactor run includes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Also {
    /// The modules this one depends on (`-am`), which resolve from the reactor, not `~/.m2`.
    pub upstream: bool,
    /// The modules that depend on it, and, with them, what those depend on.
    pub downstream: bool,
}

/// Looks in `dirs` for the wrapper script of `kind` and returns its absolute path, else the
/// program name on `PATH`. On Windows the script is the `.cmd` or `.bat` file.
pub(super) fn program(kind: BuildKind, dirs: &[&Path]) -> String {
    let (script, on_path) = match (kind, cfg!(windows)) {
        (BuildKind::Maven, false) => ("mvnw", "mvn"),
        (BuildKind::Maven, true) => ("mvnw.cmd", "mvn.cmd"),
        (BuildKind::Gradle, false) => ("gradlew", "gradle"),
        (BuildKind::Gradle, true) => ("gradlew.bat", "gradle.bat"),
    };
    dirs.iter()
        .map(|d| d.join(script))
        .find(|p| runnable(p))
        .map_or_else(|| on_path.to_string(), |p| p.to_string_lossy().into_owned())
}

/// Whether `path` is a file the program can run: on Unix one with an execute bit.
fn runnable(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    #[cfg(unix)]
    let executable = {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    };
    #[cfg(not(unix))]
    let executable = true;
    meta.is_file() && executable
}

/// The directories from `dir` (excluded) upward that the search for a build root may visit:
/// it ends at the first one that holds a `.git`, which that one is part of.
pub(super) fn ancestors_to_repo(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for ancestor in dir.ancestors().skip(1) {
        out.push(ancestor.to_path_buf());
        if ancestor.join(".git").exists() {
            break;
        }
    }
    out
}

/// `path` relative to `root` with `/` separators; `None` when `path` is `root` itself.
pub(super) fn relative_slashed(path: &Path, root: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    (!rel.as_os_str().is_empty()).then(|| slashed(rel))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(super) mod tests {
    use std::fs;

    use super::*;

    /// A scratch directory tree that removes itself.
    pub struct Tree(pub PathBuf);

    impl Tree {
        pub fn new(tag: &str) -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("hookah-java-{tag}-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        pub fn write(&self, rel: &str, text: &str) -> PathBuf {
            let path = self.0.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
            path
        }

        pub fn path(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn load_picks_the_build_tool_by_the_files() {
        let t = Tree::new("load");
        assert!(Build::load(&t.path("none")).is_none());
        t.write("m/pom.xml", "<project/>");
        t.write("g/build.gradle", "");
        t.write("b/pom.xml", "<project/>");
        t.write("b/build.gradle", "");
        assert_eq!(Build::load(&t.path("m")).unwrap().kind, BuildKind::Maven);
        assert_eq!(Build::load(&t.path("g")).unwrap().kind, BuildKind::Gradle);
        assert_eq!(Build::load(&t.path("b")).unwrap().kind, BuildKind::Maven);
    }

    #[test]
    fn task_and_reactor_args_address_the_module() {
        let mut b = Build {
            kind: BuildKind::Gradle,
            root: "/r".into(),
            module_dir: "/r/app".into(),
            selector: Some(":app".into()),
            program: "gradle".into(),
            dependents: vec![],
            skipped_dependents: 0,
            features: Features::default(),
        };
        assert_eq!(b.task("test"), ":app:test");
        b.selector = Some(String::new());
        assert_eq!(b.task("test"), ":test");
        b.selector = None;
        assert_eq!(b.task("test"), ":test");

        let both = Also {
            upstream: true,
            downstream: true,
        };
        let none = Also {
            upstream: false,
            downstream: false,
        };
        assert!(b.reactor_args(both).is_empty());
        b.selector = Some("a/b".into());
        assert_eq!(b.reactor_args(none), ["-pl", "a/b"]);
        assert_eq!(
            b.reactor_args(Also {
                upstream: true,
                downstream: false
            }),
            ["-pl", "a/b", "-am"]
        );
        // Dependents go into -pl, and -am then covers their upstream too.
        b.dependents = vec!["c".into(), "d/e".into()];
        assert_eq!(b.reactor_args(both), ["-pl", "a/b,c,d/e", "-am"]);
        assert_eq!(
            b.reactor_args(Also {
                upstream: false,
                downstream: true
            }),
            ["-pl", "a/b,c,d/e", "-am"]
        );
        assert_eq!(
            b.reactor_args(Also {
                upstream: true,
                downstream: false
            }),
            ["-pl", "a/b", "-am"]
        );
        assert_eq!(b.reactor_args(none), ["-pl", "a/b"]);
    }

    #[cfg(unix)]
    #[test]
    fn the_wrapper_wins_when_it_is_executable() {
        use std::os::unix::fs::PermissionsExt;

        let t = Tree::new("wrapper");
        let script = t.write("mvnw", "#!/bin/sh\n");
        let dirs = [t.0.as_path()];
        // Present but not executable: the program on PATH.
        assert_eq!(program(BuildKind::Maven, &dirs), "mvn");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            program(BuildKind::Maven, &dirs),
            script.to_string_lossy().as_ref()
        );
        assert_eq!(program(BuildKind::Gradle, &dirs), "gradle");
        // A directory with that name is not a wrapper, whatever its mode.
        fs::create_dir_all(t.path("gradlew")).unwrap();
        assert_eq!(program(BuildKind::Gradle, &dirs), "gradle");
        assert_eq!(program(BuildKind::Maven, &[]), "mvn");
    }

    #[test]
    fn ancestors_stop_at_the_repository() {
        let t = Tree::new("anc");
        fs::create_dir_all(t.path("r/.git")).unwrap();
        fs::create_dir_all(t.path("r/a/b")).unwrap();
        let found = ancestors_to_repo(&t.path("r/a/b"));
        assert_eq!(found, [t.path("r/a"), t.path("r")]);
    }

    #[test]
    fn relative_paths_are_slashed_and_never_empty() {
        let root = Path::new("/r");
        assert_eq!(relative_slashed(Path::new("/r/a/b"), root).unwrap(), "a/b");
        assert_eq!(relative_slashed(Path::new("/r"), root), None);
        assert_eq!(relative_slashed(Path::new("/x"), root), None);
    }
}
