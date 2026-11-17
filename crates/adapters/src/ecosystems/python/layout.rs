//! Python source layout: module names, the project's Python files, and where modules live.
//!
//! Both the flat layout (`pkg/mod.py` at the project root) and the src layout
//! (`src/pkg/mod.py`) are understood. All paths in this module are relative to the project root
//! unless stated otherwise.

use std::fs;
use std::path::{Path, PathBuf};

/// The most Python files one walk collects. A guard against pathological trees.
const MAX_FILES: usize = 50_000;
/// The deepest directory a walk enters.
const MAX_DEPTH: usize = 16;

/// Whether a directory is never searched: hidden, virtual environments, build output, caches,
/// and mutmut's `mutants/` copy.
pub fn is_skipped_dir(name: &str) -> bool {
    name.starts_with('.')
        || name.ends_with(".egg-info")
        || matches!(
            name,
            "venv"
                | "env"
                | "node_modules"
                | "__pycache__"
                | "site-packages"
                | "build"
                | "dist"
                | "mutants"
        )
}

/// Every `.py` file under `root`, sorted, as paths relative to `root`. Symlinks are not followed.
pub fn python_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, Path::new(""), 0, &mut out);
    out.sort();
    out
}

fn walk(root: &Path, rel: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    let Ok(entries) = fs::read_dir(root.join(rel)) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name();
        let Some(text) = name.to_str() else {
            continue;
        };
        let path = rel.join(text);
        if kind.is_dir() {
            if !is_skipped_dir(text) {
                walk(root, &path, depth + 1, out);
            }
        } else if kind.is_file() && text.ends_with(".py") {
            out.push(path);
        }
    }
}

/// The module name mutmut gives a source file: the path without `.py`, dots for separators, and
/// a leading `src.` removed. `src/shop/cart.py` is `shop.cart`.
pub fn mutmut_module(rel: &Path) -> String {
    let dotted = dotted(rel);
    dotted
        .strip_prefix("src.")
        .map_or(dotted.clone(), str::to_string)
}

/// The path without its extension, with `.` for separators.
pub fn dotted(rel: &Path) -> String {
    let stem = rel.with_extension("");
    stem.components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join(".")
}

/// The file defining `module` (`shop.domain.cart`): `<base>/shop/domain/cart.py` or
/// `<base>/shop/domain/cart/__init__.py`, where `<base>` is `src` or the project root itself.
/// Only existing files are returned.
pub fn module_file(root: &Path, module: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = module.split('.').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return None;
    }
    let tail: PathBuf = parts.iter().collect();
    for base in [Path::new("src"), Path::new("")] {
        let plain = base.join(&tail).with_extension("py");
        let package = base.join(&tail).join("__init__.py");
        for candidate in [plain, package] {
            if root.join(&candidate).is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Whether the project uses the src layout: a `src/` directory holding a Python package or module.
pub fn has_src_layout(root: &Path) -> bool {
    let Ok(entries) = fs::read_dir(root.join("src")) else {
        return false;
    };
    entries.flatten().any(|e| {
        let path = e.path();
        path.join("__init__.py").is_file()
            || path.extension().is_some_and(|x| x == "py")
            || (path.is_dir() && has_python_below(&path))
    })
}

fn has_python_below(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|e| e.path().extension().is_some_and(|x| x == "py"))
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use crate::ecosystems::python::fixture::Scratch;

    use super::*;

    #[test]
    fn module_names_follow_mutmut() {
        assert_eq!(mutmut_module(Path::new("src/shop/cart.py")), "shop.cart");
        assert_eq!(mutmut_module(Path::new("shop/cart.py")), "shop.cart");
        assert_eq!(mutmut_module(Path::new("lib/x.py")), "lib.x");
        assert_eq!(
            mutmut_module(Path::new("src/shop/__init__.py")),
            "shop.__init__"
        );
        assert_eq!(dotted(Path::new("a/b.py")), "a.b");
    }

    #[test]
    fn finds_python_files_and_skips_junk_dirs() {
        let s = Scratch::new("layout-walk");
        for f in [
            "src/shop/cart.py",
            "tests/test_cart.py",
            ".venv/lib/x.py",
            "venv/lib/x.py",
            "mutants/src/shop/cart.py",
            "node_modules/x.py",
            "build/lib/x.py",
            "src/shop.egg-info/x.py",
            "README.md",
        ] {
            s.write(f, "");
        }
        assert_eq!(
            python_files(s.root()),
            [
                PathBuf::from("src/shop/cart.py"),
                PathBuf::from("tests/test_cart.py")
            ]
        );
    }

    #[test]
    fn resolves_modules_in_src_and_flat_layouts() {
        let s = Scratch::new("layout-modules");
        s.write("src/shop/domain/cart.py", "");
        s.write("src/shop/domain/__init__.py", "");
        s.write("flat/mod.py", "");
        s.write("flat/pkg/__init__.py", "");
        let r = s.root();
        assert_eq!(
            module_file(r, "shop.domain.cart"),
            Some("src/shop/domain/cart.py".into())
        );
        assert_eq!(
            module_file(r, "shop.domain"),
            Some("src/shop/domain/__init__.py".into())
        );
        assert_eq!(module_file(r, "flat.mod"), Some("flat/mod.py".into()));
        assert_eq!(
            module_file(r, "flat.pkg"),
            Some("flat/pkg/__init__.py".into())
        );
        assert_eq!(module_file(r, "nope.mod"), None);
    }

    #[test]
    fn detects_src_layout() {
        let s = Scratch::new("layout-src");
        assert!(!has_src_layout(s.root()));
        s.write("src/shop/__init__.py", "");
        assert!(has_src_layout(s.root()));
        let flat = Scratch::new("layout-flat");
        flat.write("shop/__init__.py", "");
        assert!(!has_src_layout(flat.root()));
    }
}
