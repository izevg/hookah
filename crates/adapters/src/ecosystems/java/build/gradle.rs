//! Gradle: build root, project path, plugins, and dependents from the build scripts.
//!
//! The scripts are Groovy or Kotlin code, which hookah never runs. It reads them as text, which
//! is enough for the few facts it needs and fails open: what it cannot find is simply not
//! configured.
//!
//! - The build root is the nearest directory, the module's own included, with a `settings.gradle`
//!   or `settings.gradle.kts`. Without one the module is a single-project build and its own root.
//! - The project path is the module's directory relative to the root, as `:a:b` (the default
//!   layout; `projectDir` overrides are not followed).
//! - A plugin counts when its id is quoted in the module's script or in the root script (where
//!   `subprojects {}` and `allprojects {}` configure all projects), or, for the plugins Gradle
//!   ships (`checkstyle`, `pmd`), when it is named bare in a `plugins {}` block or configured by
//!   a `checkstyle {` block.
//! - Dependents are the included projects whose scripts mention `project(":<path>")` of this
//!   module, or of a project that is a dependent already.

use std::collections::BTreeSet;
use std::path::Path;

use super::{BuildKind, Features, ancestors_to_repo, program, relative_slashed};

const SETTINGS: [&str; 2] = ["settings.gradle.kts", "settings.gradle"];
const SCRIPTS: [&str; 2] = ["build.gradle.kts", "build.gradle"];

pub(super) fn load(module_dir: &Path) -> Option<super::Build> {
    let has_file = |dir: &Path, names: &[&str]| names.iter().any(|n| dir.join(n).is_file());
    if !has_file(module_dir, &SETTINGS) && !has_file(module_dir, &SCRIPTS) {
        return None;
    }
    let root = std::iter::once(module_dir.to_path_buf())
        .chain(ancestors_to_repo(module_dir))
        .find(|d| has_file(d, &SETTINGS))
        .unwrap_or_else(|| module_dir.to_path_buf());
    let selector =
        relative_slashed(module_dir, &root).map(|rel| format!(":{}", rel.replace('/', ":")));

    let mut text = script(module_dir);
    if root != module_dir {
        text.push('\n');
        text.push_str(&script(&root));
    }
    let projects = read(&root, &SETTINGS)
        .map(|s| includes(&s))
        .unwrap_or_default();
    let dependents = match &selector {
        Some(path) => dependents(&root, path, &projects),
        None => dependents(&root, ":", &projects),
    };
    Some(super::Build {
        kind: BuildKind::Gradle,
        program: program(BuildKind::Gradle, &[&root, module_dir]),
        features: features(&text),
        selector,
        root,
        module_dir: module_dir.to_path_buf(),
        dependents,
    })
}

fn read(dir: &Path, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|n| std::fs::read_to_string(dir.join(n)).ok())
}

fn script(dir: &Path) -> String {
    read(dir, &SCRIPTS).unwrap_or_default()
}

/// The project paths (`:a:b`) that `settings.gradle(.kts)` includes.
fn includes(settings: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // Where the last call read ended: an `include` inside it belongs to that call.
    let mut consumed = 0;
    for (at, _) in settings.match_indices("include") {
        let start = at + "include".len();
        let after = &settings[start..];
        // `include(...)` and `include 'a'`, not `includeBuild(...)` or `includeFlat`.
        if at < consumed || !after.starts_with(['(', ' ', '\t']) {
            continue;
        }
        let (quoted, used) = quoted_arguments(after);
        consumed = start + used;
        for q in quoted {
            let path = if q.starts_with(':') {
                q
            } else {
                format!(":{q}")
            };
            if !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

/// The quoted strings of one `include` call and how many bytes of `text` it spans. The call
/// continues over a line break after a comma or inside parentheses.
fn quoted_arguments(text: &str) -> (Vec<String>, usize) {
    let mut found = Vec::new();
    let mut depth = 0_i32;
    let mut quote: Option<(char, usize)> = None;
    let mut last_significant = ' ';
    for (i, c) in text.char_indices() {
        if let Some((q, start)) = quote {
            if c == q {
                found.push(text[start..i].to_string());
                quote = None;
                last_significant = c;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some((c, i + 1)),
            '(' => {
                depth += 1;
                last_significant = c;
            }
            ')' => {
                depth -= 1;
                last_significant = c;
                if depth <= 0 {
                    return (found, i + 1);
                }
            }
            '\n' if depth <= 0 && last_significant != ',' => return (found, i),
            c if !c.is_whitespace() => last_significant = c,
            _ => {}
        }
    }
    (found, text.len())
}

/// The included projects that depend on `path`, directly or through another dependent.
fn dependents(root: &Path, path: &str, projects: &[String]) -> Vec<String> {
    let texts: Vec<(&String, String)> = projects
        .iter()
        .map(|p| (p, script(&project_dir(root, p))))
        .collect();
    let mut found: BTreeSet<&String> = BTreeSet::new();
    // Each round adds the projects that mention the module or a dependent found so far. A chain
    // of dependents is no longer than the list of projects, which also bounds the rounds.
    for _ in 0..projects.len() {
        let before = found.len();
        for (project, text) in &texts {
            let depends = mentions_project(text, path)
                || found
                    .iter()
                    .any(|dependent| mentions_project(text, dependent));
            if depends && **project != path {
                found.insert(project);
            }
        }
        if found.len() == before {
            break;
        }
    }
    found.into_iter().cloned().collect()
}

/// Whether `text` declares a dependency `project(":path")`, in either quote style.
fn mentions_project(text: &str, path: &str) -> bool {
    let squeezed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    squeezed.contains(&format!("project(\"{path}\")"))
        || squeezed.contains(&format!("project('{path}')"))
}

/// Whether `text` applies the plugin `id`, quoted or (for the ones in `bare`) named bare.
fn applies(text: &str, id: &str, bare: Option<&str>) -> bool {
    if text.contains(&format!("\"{id}\"")) || text.contains(&format!("'{id}'")) {
        return true;
    }
    let Some(bare) = bare else { return false };
    text.lines().map(|l| l.trim().trim_matches('`')).any(|l| {
        l == bare || l.starts_with(&format!("{bare} {{")) || l.starts_with(&format!("{bare}{{"))
    })
}

fn features(text: &str) -> Features {
    let lower = text.to_lowercase();
    let spotbugs = applies(text, "com.github.spotbugs", None);
    let pitest = applies(text, "info.solidsoft.pitest", Some("pitest"));
    Features {
        spotless: applies(text, "com.diffplug.spotless", Some("spotless")),
        checkstyle: applies(text, "checkstyle", Some("checkstyle")),
        pmd: applies(text, "pmd", Some("pmd")),
        spotbugs,
        spotbugs_xml: spotbugs
            && ["xml.required", "xml.enabled", "\"xml\"", "'xml'", "xml {"]
                .iter()
                .any(|m| lower.contains(m)),
        pitest,
        pitest_xml: pitest && text.contains("outputFormats") && text.contains("XML"),
        junit5: lower.contains("jupiter")
            || text.contains("useJUnitPlatform")
            || text.contains("junit-bom")
            || text.contains("spring-boot-starter-test"),
    }
}

/// The directory of the project `path` below `root`.
fn project_dir(root: &Path, path: &str) -> std::path::PathBuf {
    root.join(path.trim_start_matches(':').replace(':', "/"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::tests::Tree;
    use super::*;

    const KOTLIN: &str = r#"
plugins {
    java
    checkstyle
    id("com.diffplug.spotless") version "7.0.4"
    id("info.solidsoft.pitest") version "1.19.0"
}
dependencies { testImplementation("org.junit.jupiter:junit-jupiter") }
pitest { outputFormats = setOf("XML") }
"#;

    const GROOVY: &str = r#"
plugins {
    id 'java'
    id 'pmd'
    id 'com.github.spotbugs' version '6.0.0'
}
spotbugsMain { reports { xml.required = true } }
test { useJUnitPlatform() }
"#;

    #[test]
    fn single_project_builds_are_their_own_root() {
        let t = Tree::new("gr-single");
        t.write("build.gradle.kts", KOTLIN);
        t.write("settings.gradle.kts", "rootProject.name = \"shop\"\n");
        let b = load(&t.0).unwrap();
        assert_eq!(b.kind, BuildKind::Gradle);
        assert_eq!(b.root, t.0);
        assert_eq!(b.selector, None);
        assert_eq!(b.task("test"), ":test");
        assert_eq!(
            b.program,
            if cfg!(windows) {
                "gradle.bat"
            } else {
                "gradle"
            }
        );
        assert!(b.dependents.is_empty());
        assert_eq!(
            b.features,
            Features {
                spotless: true,
                checkstyle: true,
                pmd: false,
                spotbugs: false,
                spotbugs_xml: false,
                pitest: true,
                pitest_xml: true,
                junit5: true,
            }
        );
    }

    #[test]
    fn each_marker_alone_means_junit_5() {
        for text in [
            "implementation 'org.junit.jupiter:junit-jupiter'",
            "test { useJUnitPlatform() }",
            "testImplementation(platform(\"org.junit:junit-bom:5.12.2\"))",
            "testImplementation 'org.springframework.boot:spring-boot-starter-test'",
        ] {
            assert!(features(text).junit5, "{text}");
        }
        assert!(!features("testImplementation 'junit:junit:4.13'").junit5);
    }

    #[test]
    fn groovy_scripts_are_read_too() {
        let f = features(GROOVY);
        assert!(f.pmd && f.spotbugs && f.spotbugs_xml && f.junit5);
        assert!(!f.checkstyle && !f.spotless && !f.pitest && !f.pitest_xml);
        // SpotBugs without an XML report is configured but cannot be read.
        let f = features("plugins { id 'com.github.spotbugs' }");
        assert!(f.spotbugs && !f.spotbugs_xml);
        // PIT without the XML output format likewise.
        let f =
            features("plugins { id 'info.solidsoft.pitest' }\npitest { outputFormats = ['HTML'] }");
        assert!(f.pitest && !f.pitest_xml);
        let f = features("apply plugin: 'checkstyle'");
        assert!(f.checkstyle);
        assert!(features("checkstyle{\n}").checkstyle);
        assert!(features("`pmd`").pmd);
        assert!(!features("// checkstyle is great\nrepositories {}").checkstyle);
        assert!(!features("").junit5);
    }

    #[test]
    fn plugin_ids_must_match_exactly() {
        assert!(!applies("id(\"checkstyle-extras\")", "checkstyle", None));
        assert!(applies("id(\"x.y\")", "x.y", None));
        assert!(applies("id('x.y')", "x.y", None));
        assert!(!applies("x.y", "x.y", None));
        assert!(applies("  pitest {  ", "info", Some("pitest")));
        assert!(!applies("pitestExtras {", "info", Some("pitest")));
    }

    #[test]
    fn subprojects_get_a_path_the_root_and_inherited_plugins() {
        let t = Tree::new("gr-multi");
        t.write(
            "settings.gradle.kts",
            "include(\"core\", \":services:app\")\n",
        );
        t.write(
            "build.gradle.kts",
            "subprojects { apply(plugin = \"checkstyle\") }\n",
        );
        t.write(
            "core/build.gradle.kts",
            "plugins { id(\"com.diffplug.spotless\") }\n",
        );
        t.write(
            "services/app/build.gradle.kts",
            "dependencies { implementation(project(\":core\")) }\n",
        );
        let core = load(&t.path("core")).unwrap();
        assert_eq!(core.root, t.0);
        assert_eq!(core.selector.as_deref(), Some(":core"));
        assert_eq!(core.task("compileJava"), ":core:compileJava");
        assert!(core.features.spotless);
        assert!(core.features.checkstyle);
        assert_eq!(core.dependents, [":services:app"]);
        let app = load(&t.path("services/app")).unwrap();
        assert_eq!(app.selector.as_deref(), Some(":services:app"));
        assert!(app.dependents.is_empty());
        assert!(!app.features.spotless);
    }

    #[test]
    fn dependents_are_transitive_and_exclude_the_project_itself() {
        let t = Tree::new("gr-trans");
        t.write("settings.gradle", "include 'a', 'b'\ninclude 'c'\n");
        t.write("a/build.gradle", "dependencies { api project(':b') }\n");
        t.write("b/build.gradle", "dependencies { api project( ':c' ) }\n");
        t.write("c/build.gradle", "dependencies { api project(':a') }\n");
        let c = load(&t.path("c")).unwrap();
        assert_eq!(c.dependents, [":a", ":b"]);
    }

    #[test]
    fn the_root_project_can_have_dependents() {
        let t = Tree::new("gr-rootdep");
        t.write("settings.gradle", "include 'a'\n");
        t.write("build.gradle", "");
        t.write("a/build.gradle", "dependencies { api project(':') }\n");
        let root = load(&t.0).unwrap();
        assert_eq!(root.selector, None);
        assert_eq!(root.dependents, [":a"]);
    }

    #[test]
    fn a_settings_file_alone_makes_a_project_and_none_makes_nothing() {
        let t = Tree::new("gr-settings");
        t.write("s/settings.gradle", "rootProject.name = 'x'\n");
        assert!(load(&t.path("s")).is_some());
        assert!(load(&t.path("empty")).is_none());
        t.write("e/readme.txt", "");
        assert!(load(&t.path("e")).is_none());
    }

    #[test]
    fn a_subproject_without_settings_nearby_is_its_own_root() {
        let t = Tree::new("gr-nosettings");
        t.write("a/build.gradle", "");
        let b = load(&t.path("a")).unwrap();
        assert_eq!(b.root, t.path("a"));
        assert_eq!(b.selector, None);
    }

    #[test]
    fn include_statements_in_all_their_spellings() {
        let all = includes(
            "rootProject.name = 'x'\n\
             include(\"a\")\n\
             include 'b', ':c:d'\n\
             include(\n  \"e\",\n  \"f\"\n)\n\
             include 'g',\n        'h'\n\
             include 'a'\n\
             // nothing else\n",
        );
        assert_eq!(all, [":a", ":b", ":c:d", ":e", ":f", ":g", ":h"]);
        assert!(includes("").is_empty());
        assert!(includes("include").is_empty());
        assert!(includes("includeBuild(\"../x\")\nincludeFlat 'y'\n").is_empty());
        assert_eq!(includes("includeBuild('x')\ninclude('a')"), [":a"]);
    }

    #[test]
    fn an_include_call_ends_where_its_statement_ends() {
        // Quoted strings of the next statement, on the same line or the next, are not projects.
        assert_eq!(includes("include(\"a\"); println(\"x\")\n"), [":a"]);
        assert_eq!(includes("include(\"a\")\nprintln(\"x\")\n"), [":a"]);
        assert_eq!(includes("include 'a'\nrootProject.name = 'x'\n"), [":a"]);
        // A comma, even followed by blanks, continues the call on the next line.
        assert_eq!(includes("include 'a', \n    'b'\n"), [":a", ":b"]);
        // Parentheses nest.
        assert_eq!(
            includes("include(listOf(\"a\"), \"b\")\nfoo(\"c\")\n"),
            [":a", ":b"]
        );
        // The span is what was consumed, so a second call right after is still found.
        assert_eq!(includes("include(\"a\")include(\"b\")"), [":a", ":b"]);
    }

    #[test]
    fn quoted_arguments_report_how_much_text_they_consumed() {
        let (found, used) = quoted_arguments("(\"a\", \"b\") tail");
        assert_eq!(found, ["a", "b"]);
        assert_eq!(&"(\"a\", \"b\") tail"[used..], " tail");
        let (found, used) = quoted_arguments(" 'a'\nnext 'b'");
        assert_eq!(found, ["a"]);
        assert_eq!(used, " 'a'".len());
    }

    #[test]
    fn project_references_need_the_exact_path() {
        assert!(mentions_project("project(\":core\")", ":core"));
        assert!(mentions_project("project ( ':core' )", ":core"));
        assert!(!mentions_project("project(\":core:sub\")", ":core"));
        assert!(!mentions_project("project(\":core\")", ":other"));
        assert_eq!(
            project_dir(Path::new("/r"), ":a:b"),
            std::path::PathBuf::from("/r/a/b")
        );
    }
}
