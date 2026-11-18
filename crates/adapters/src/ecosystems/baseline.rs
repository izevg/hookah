//! Whole-project targets for the session baseline.
//!
//! The baseline runs each Lint, Typecheck, and Architecture tool once over the whole project.
//! A tool that takes file targets (eslint, biome, ruff) gets the project root `.`. A tool whose
//! command already names the whole project gets no targets, because extra ones would add files
//! twice or sweep in generated directories:
//!
//! - Rust: `cargo clippy -p <package> ...` covers the package and its dependents; there is no
//!   target placeholder, so the targets are ignored.
//! - TypeScript: `tsc` runs by `tsconfig.json`, not by target, so the targets are ignored. The
//!   architecture tool (dependency-cruiser) gets `src` when the package has one, else `.`: it
//!   follows imports, and a bare `.` would walk `node_modules` and build output in projects whose
//!   config does not exclude them.
//! - Java: the lint and typecheck commands name the module and take no file targets, so the
//!   targets are ignored.
//! - Python: mypy already lists the top-level packages in its command; import-linter reads its
//!   contracts from the config file. Both get no targets.

use hookah_domain::project::{Ecosystem, Project};
use hookah_domain::toolchain::Tool;

use super::node::detect::DEPCRUISE;
use super::python::detect::{IMPORT_LINTER, MYPY};

/// The targets that make `tool` cover all of `project`.
pub fn targets(project: &Project, tool: &Tool) -> Vec<String> {
    let whole = vec![".".to_string()];
    match project.ecosystem {
        Ecosystem::Python if tool.name == MYPY || tool.name == IMPORT_LINTER => vec![],
        Ecosystem::Node if tool.name == DEPCRUISE && project.root.join("src").is_dir() => {
            vec!["src".to_string()]
        }
        Ecosystem::Java | Ecosystem::Rust | Ecosystem::Node | Ecosystem::Python => whole,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use hookah_domain::toolchain::{CheckKind, CommandSpec};

    use super::*;

    fn tool(name: &str, kind: CheckKind) -> Tool {
        Tool {
            name: name.into(),
            kind,
            command: CommandSpec::new(name, vec![], "/r", Duration::from_secs(1)),
        }
    }

    fn project(root: &std::path::Path, ecosystem: Ecosystem) -> Project {
        Project {
            root: root.into(),
            ecosystem,
            manifests: vec![],
        }
    }

    #[test]
    fn file_tools_get_the_project_root() {
        let p = project("/r".as_ref(), Ecosystem::Python);
        assert_eq!(targets(&p, &tool("ruff", CheckKind::Lint)), ["."]);
        let p = project("/r".as_ref(), Ecosystem::Node);
        assert_eq!(targets(&p, &tool("eslint", CheckKind::Lint)), ["."]);
        assert_eq!(targets(&p, &tool("tsc", CheckKind::Typecheck)), ["."]);
        let p = project("/r".as_ref(), Ecosystem::Rust);
        assert_eq!(targets(&p, &tool("clippy", CheckKind::Lint)), ["."]);
    }

    #[test]
    fn python_project_wide_tools_get_no_targets() {
        let p = project("/r".as_ref(), Ecosystem::Python);
        assert!(targets(&p, &tool("mypy", CheckKind::Typecheck)).is_empty());
        assert!(targets(&p, &tool("import-linter", CheckKind::Architecture)).is_empty());
        // Another ecosystem's project with a tool of the same name is not special.
        let p = project("/r".as_ref(), Ecosystem::Node);
        assert_eq!(targets(&p, &tool("mypy", CheckKind::Typecheck)), ["."]);
    }

    #[test]
    fn dependency_cruiser_gets_src_when_it_exists() {
        let dir = std::env::temp_dir().join(format!("hookah-bt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let depcruise = tool("depcruise", CheckKind::Architecture);
        let p = project(&dir, Ecosystem::Node);
        assert_eq!(targets(&p, &depcruise), ["."]);
        fs::create_dir(dir.join("src")).unwrap();
        assert_eq!(targets(&p, &depcruise), ["src"]);
        assert_eq!(targets(&p, &tool("eslint", CheckKind::Lint)), ["."]);
        let p = project(&dir, Ecosystem::Python);
        assert_eq!(targets(&p, &depcruise), ["."]);
        let _ = fs::remove_dir_all(&dir);
    }
}
