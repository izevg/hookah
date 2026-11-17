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
    python_files_within(root, MAX_DEPTH, MAX_FILES)
}

/// [`python_files`] with explicit limits: directories deeper than `max_depth` are not entered and
/// the walk stops once `max_files` files are collected.
fn python_files_within(root: &Path, max_depth: usize, max_files: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(root, Path::new(""), (0, max_depth, max_files), &mut out);
    out.sort();
    out
}

/// One step of the walk; `limits` is `(depth, max_depth, max_files)`.
fn walk(root: &Path, rel: &Path, limits: (usize, usize, usize), out: &mut Vec<PathBuf>) {
    let (depth, max_depth, max_files) = limits;
    let Ok(entries) = fs::read_dir(root.join(rel)) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= max_files {
            return;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name();
        let Some(text) = name.to_str() else {
            continue;
        };
        let path = rel.join(text);
        if kind.is_dir() {
            if depth < max_depth && !is_skipped_dir(text) {
                walk(root, &path, (depth + 1, max_depth, max_files), out);
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

/// The top-level packages (and modules, below `src/`) of the project, as paths relative to the
/// root, sorted: the entries of `src/` in the src layout, else the directories with an
/// `__init__.py` at the root. Namespace packages (directories without `__init__.py`) count only
/// in the src layout, where a directory holding Python files is one.
pub fn top_level_packages(root: &Path) -> Vec<PathBuf> {
    let src = has_src_layout(root);
    let base = if src {
        root.join("src")
    } else {
        root.to_path_buf()
    };
    let Ok(entries) = fs::read_dir(&base) else {
        return vec![];
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| {
            let path = e.path();
            let name = e.file_name();
            let Some(name) = name.to_str() else {
                return false;
            };
            if path.is_dir() {
                !is_skipped_dir(name)
                    && (path.join("__init__.py").is_file() || (src && has_python_below(&path)))
            } else {
                src && name.ends_with(".py")
            }
        })
        .map(|e| {
            let name = e.file_name();
            if src {
                Path::new("src").join(name)
            } else {
                PathBuf::from(name)
            }
        })
        .collect();
    out.sort();
    out
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
    fn top_level_packages_in_the_src_layout() {
        let s = Scratch::new("layout-top-src");
        for f in [
            "src/shop/__init__.py",
            "src/ns/x.py",
            "src/solo.py",
            "src/notes.txt",
            "src/empty/readme.md",
            "src/.hidden/__init__.py",
            "src/shop.egg-info/__init__.py",
            "shop/__init__.py",
            "tests/__init__.py",
        ] {
            s.write(f, "");
        }
        std::fs::create_dir_all(s.root().join("src/nopy")).unwrap();
        assert_eq!(
            top_level_packages(s.root()),
            [
                PathBuf::from("src/ns"),
                PathBuf::from("src/shop"),
                PathBuf::from("src/solo.py")
            ]
        );
    }

    #[test]
    fn top_level_packages_in_the_flat_layout() {
        let s = Scratch::new("layout-top-flat");
        for f in [
            "shop/__init__.py",
            "tests/__init__.py",
            "scripts/run.py",
            "venv/__init__.py",
            "main.py",
        ] {
            s.write(f, "");
        }
        assert_eq!(
            top_level_packages(s.root()),
            [PathBuf::from("shop"), PathBuf::from("tests")]
        );
        assert!(top_level_packages(&s.root().join("missing")).is_empty());
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

    #[test]
    fn src_layout_needs_python_in_src() {
        let layout = |name: &str, files: &[&str], dirs: &[&str]| {
            let s = Scratch::new(name);
            for f in files {
                s.write(f, "");
            }
            for d in dirs {
                std::fs::create_dir_all(s.root().join(d)).unwrap();
            }
            has_src_layout(s.root())
        };
        assert!(layout("src-pkg", &["src/pkg/__init__.py"], &[]));
        assert!(layout("src-mod", &["src/mod.py"], &[]));
        assert!(layout("src-ns", &["src/ns/mod.py"], &[]));
        assert!(!layout("src-readme", &["src/README.md"], &[]));
        assert!(!layout("src-empty-dir", &[], &["src/ns"]));
        assert!(!layout("src-ns-text", &["src/ns/notes.txt"], &[]));
        assert!(!layout("src-empty", &[], &["src"]));
    }

    #[test]
    fn walk_respects_depth_and_file_limits() {
        let s = Scratch::new("layout-limits");
        for f in ["a.py", "d1/b.py", "d1/d2/c.py", "d1/d2/d3/d.py"] {
            s.write(f, "");
        }
        let names = |depth, files| {
            python_files_within(s.root(), depth, files)
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(0, 100), ["a.py"]);
        assert_eq!(names(1, 100), ["a.py", "d1/b.py"]);
        assert_eq!(names(2, 100), ["a.py", "d1/b.py", "d1/d2/c.py"]);
        assert_eq!(names(16, 100).len(), 4);
        assert_eq!(names(16, 2).len(), 2);
        assert_eq!(names(16, 4).len(), 4);
    }
}
