//! Maven: reactor discovery and plugin detection from `pom.xml` files.
//!
//! The reactor root is the topmost `pom.xml` that lists the module (through its `<modules>`,
//! directly or through intermediate aggregators). Commands run there with `-pl <module>`, so
//! sibling modules resolve from the reactor and not from `~/.m2`. A module whose parent does not
//! aggregate it is its own root and runs from its own directory.
//!
//! A tool counts as configured when its plugin is in `<build><plugins>` or `<reporting><plugins>`
//! of the module's pom, of an aggregator above it, or of a parent reachable through
//! `<relativePath>`. A plugin that only a parent outside the checkout configures is not seen.

use std::path::{Path, PathBuf};

use hookah_domain::path::normalize;

use super::{BuildKind, Features, ancestors_to_repo, program, relative_slashed};
use crate::ecosystems::java::xml::{self, Node};

/// How far up `<relativePath>` parents are followed.
const MAX_PARENTS: usize = 6;

pub(super) fn load(module_dir: &Path) -> Option<super::Build> {
    let (root, mut dirs) = reactor(module_dir);
    for parent in parents(module_dir) {
        if !dirs.contains(&parent) {
            dirs.push(parent);
        }
    }
    let poms: Vec<Node> = dirs.iter().filter_map(|d| read_pom(d)).collect();
    let mut features = features(&poms);
    features.spotbugs_xml = features.spotbugs;
    features.pitest_xml = features.pitest;
    Some(super::Build {
        kind: BuildKind::Maven,
        program: program(BuildKind::Maven, &[&root, module_dir]),
        selector: relative_slashed(module_dir, &root),
        root,
        module_dir: module_dir.to_path_buf(),
        dependents: vec![],
        features,
    })
}

fn read_pom(dir: &Path) -> Option<Node> {
    xml::parse(&std::fs::read_to_string(dir.join("pom.xml")).ok()?).ok()
}

/// The reactor root of the module at `module_dir`, and the directories of the poms between.
fn reactor(module_dir: &Path) -> (PathBuf, Vec<PathBuf>) {
    let mut chain = vec![module_dir.to_path_buf()];
    for ancestor in ancestors_to_repo(module_dir) {
        let current = &chain[chain.len() - 1];
        if lists(&ancestor, current) {
            chain.push(ancestor);
        }
    }
    let root = chain[chain.len() - 1].clone();
    (root, chain)
}

/// Whether the pom in `parent` lists `child` as a module.
fn lists(parent: &Path, child: &Path) -> bool {
    let Some(pom) = read_pom(parent) else {
        return false;
    };
    let child = normalize(child);
    pom.at(&["modules", "module"]).iter().any(|m| {
        let path = parent.join(m.text.trim());
        let dir = if path.extension().is_some_and(|e| e == "xml") {
            path.parent().map(Path::to_path_buf).unwrap_or(path)
        } else {
            path
        };
        normalize(&dir) == child
    })
}

/// The directories of the parent poms of `module_dir` found through `<relativePath>`, nearest
/// first. An empty `<relativePath/>` says there is none in the checkout.
fn parents(module_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dir = module_dir.to_path_buf();
    for _ in 0..MAX_PARENTS {
        let Some(pom) = read_pom(&dir) else { break };
        let Some(parent) = pom.child("parent") else {
            break;
        };
        let rel = parent.child_text("relativePath").unwrap_or("../pom.xml");
        if rel.is_empty() {
            break;
        }
        let path = normalize(&dir.join(rel));
        let next = if path.is_dir() {
            path
        } else {
            path.parent().map(Path::to_path_buf).unwrap_or(path)
        };
        if next == dir || !next.join("pom.xml").is_file() {
            break;
        }
        out.push(next.clone());
        dir = next;
    }
    out
}

fn features(poms: &[Node]) -> Features {
    let plugin = |artifact: &str| {
        poms.iter().any(|pom| {
            ["build", "reporting"].iter().any(|section| {
                pom.at(&[section, "plugins", "plugin"])
                    .iter()
                    .any(|p| p.child_text("artifactId") == Some(artifact))
            })
        })
    };
    let junit5 = poms.iter().any(|pom| {
        pom.at(&["dependencies", "dependency"])
            .iter()
            .chain(
                pom.at(&["dependencyManagement", "dependencies", "dependency"])
                    .iter(),
            )
            .filter_map(|d| d.child_text("artifactId"))
            .any(|a| {
                a.starts_with("junit-jupiter")
                    || a == "junit-bom"
                    || a == "spring-boot-starter-test"
            })
    });
    Features {
        spotless: plugin("spotless-maven-plugin"),
        checkstyle: plugin("maven-checkstyle-plugin"),
        pmd: plugin("maven-pmd-plugin"),
        spotbugs: plugin("spotbugs-maven-plugin"),
        pitest: plugin("pitest-maven"),
        junit5,
        ..Features::default()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::tests::Tree;
    use super::*;

    const AGGREGATOR: &str = r#"<project><modelVersion>4.0.0</modelVersion>
<packaging>pom</packaging>
<modules><module>core</module><module>services/app</module><module>tools/pom.xml</module></modules>
<build><plugins>
  <plugin><groupId>com.diffplug.spotless</groupId><artifactId>spotless-maven-plugin</artifactId></plugin>
</plugins></build></project>"#;

    #[test]
    fn a_single_module_is_its_own_root() {
        let t = Tree::new("mvn-single");
        t.write(
            "pom.xml",
            r#"<project xmlns="http://maven.apache.org/POM/4.0.0">
<dependencies><dependency><artifactId>junit-jupiter</artifactId></dependency></dependencies>
<build><plugins>
  <plugin><artifactId>maven-checkstyle-plugin</artifactId></plugin>
  <plugin><artifactId>maven-pmd-plugin</artifactId></plugin>
  <plugin><artifactId>spotbugs-maven-plugin</artifactId></plugin>
  <plugin><artifactId>pitest-maven</artifactId></plugin>
</plugins></build></project>"#,
        );
        let b = load(&t.0).unwrap();
        assert_eq!(b.kind, BuildKind::Maven);
        assert_eq!(b.root, t.0);
        assert_eq!(b.selector, None);
        assert_eq!(b.program, "mvn");
        assert_eq!(
            b.features,
            Features {
                spotless: false,
                checkstyle: true,
                pmd: true,
                spotbugs: true,
                spotbugs_xml: true,
                pitest: true,
                pitest_xml: true,
                junit5: true,
            }
        );
    }

    #[test]
    fn a_module_runs_from_the_reactor_root_with_inherited_plugins() {
        let t = Tree::new("mvn-reactor");
        t.write("pom.xml", AGGREGATOR);
        t.write("core/pom.xml", "<project/>");
        t.write("services/app/pom.xml", "<project/>");
        t.write("tools/pom.xml", "<project/>");
        for (module, path) in [
            ("core", Some("core")),
            ("services/app", Some("services/app")),
            ("tools", Some("tools")),
        ] {
            let b = load(&t.path(module)).unwrap();
            assert_eq!(b.root, t.0, "{module}");
            assert_eq!(b.selector.as_deref(), path, "{module}");
            assert!(
                b.features.spotless,
                "{module} inherits the aggregator's plugin"
            );
            assert!(!b.features.checkstyle);
        }
        let root = load(&t.0).unwrap();
        assert_eq!(root.selector, None);
    }

    #[test]
    fn nested_aggregators_lead_to_the_topmost_root() {
        let t = Tree::new("mvn-nested");
        t.write(
            "pom.xml",
            "<project><modules><module>mid</module></modules></project>",
        );
        t.write(
            "mid/pom.xml",
            "<project><modules><module>leaf</module></modules></project>",
        );
        t.write("mid/leaf/pom.xml", "<project/>");
        let b = load(&t.path("mid/leaf")).unwrap();
        assert_eq!(b.root, t.0);
        assert_eq!(b.selector.as_deref(), Some("mid/leaf"));
        let mid = load(&t.path("mid")).unwrap();
        assert_eq!(mid.selector.as_deref(), Some("mid"));
    }

    #[test]
    fn a_parent_that_does_not_aggregate_gives_plugins_but_not_the_root() {
        let t = Tree::new("mvn-parent");
        t.write(
            "pom.xml",
            "<project><build><plugins><plugin><artifactId>maven-pmd-plugin</artifactId></plugin></plugins></build></project>",
        );
        t.write(
            "app/pom.xml",
            "<project><parent><artifactId>p</artifactId></parent></project>",
        );
        let b = load(&t.path("app")).unwrap();
        assert_eq!(b.root, t.path("app"));
        assert_eq!(b.selector, None);
        assert!(b.features.pmd);
    }

    #[test]
    fn parents_stop_at_the_module_itself_and_at_directories_without_a_pom() {
        let t = Tree::new("mvn-parents");
        t.write(
            "a/pom.xml",
            "<project><parent><relativePath>.</relativePath></parent></project>",
        );
        assert!(parents(&t.path("a")).is_empty());
        t.write(
            "b/pom.xml",
            "<project><parent><relativePath>pom.xml</relativePath></parent></project>",
        );
        assert!(parents(&t.path("b")).is_empty());
        t.write(
            "c/pom.xml",
            "<project><parent><relativePath>../nopom</relativePath></parent></project>",
        );
        t.write("nopom/readme.txt", "");
        assert!(parents(&t.path("c")).is_empty());
        t.write("d/pom.xml", "<project><parent/></project>");
        t.write("pom.xml", "<project/>");
        assert_eq!(parents(&t.path("d")), std::slice::from_ref(&t.0));
        assert!(parents(&t.path("missing")).is_empty());
    }

    #[test]
    fn an_empty_relative_path_means_no_parent_in_the_checkout() {
        let t = Tree::new("mvn-norel");
        t.write(
            "pom.xml",
            "<project><build><plugins><plugin><artifactId>maven-pmd-plugin</artifactId></plugin></plugins></build></project>",
        );
        t.write(
            "app/pom.xml",
            "<project><parent><relativePath/></parent></project>",
        );
        assert!(!load(&t.path("app")).unwrap().features.pmd);
    }

    #[test]
    fn a_custom_relative_path_finds_the_parent() {
        let t = Tree::new("mvn-relpath");
        t.write(
            "build/parent/pom.xml",
            "<project><build><plugins><plugin><artifactId>maven-pmd-plugin</artifactId></plugin></plugins></build></project>",
        );
        t.write(
            "app/pom.xml",
            "<project><parent><relativePath>../build/parent</relativePath></parent></project>",
        );
        assert!(load(&t.path("app")).unwrap().features.pmd);
    }

    #[test]
    fn plugins_outside_build_and_reporting_do_not_count() {
        let poms = [xml::parse(
            "<project><build><pluginManagement><plugins><plugin><artifactId>maven-pmd-plugin</artifactId></plugin></plugins></pluginManagement></build>\
             <profiles><profile><build><plugins><plugin><artifactId>maven-checkstyle-plugin</artifactId></plugin></plugins></build></profile></profiles>\
             <reporting><plugins><plugin><artifactId>spotbugs-maven-plugin</artifactId></plugin></plugins></reporting></project>",
        )
        .unwrap()];
        let f = features(&poms);
        assert!(!f.pmd);
        assert!(!f.checkstyle);
        assert!(f.spotbugs);
    }

    #[test]
    fn junit5_is_read_from_dependencies_and_the_bom() {
        let junit5 = |text: &str| features(&[xml::parse(text).unwrap()]).junit5;
        assert!(junit5(
            "<project><dependencies><dependency><artifactId>junit-jupiter-api</artifactId></dependency></dependencies></project>"
        ));
        assert!(junit5(
            "<project><dependencyManagement><dependencies><dependency><artifactId>junit-bom</artifactId></dependency></dependencies></dependencyManagement></project>"
        ));
        assert!(junit5(
            "<project><dependencies><dependency><artifactId>spring-boot-starter-test</artifactId></dependency></dependencies></project>"
        ));
        assert!(!junit5(
            "<project><dependencies><dependency><artifactId>junit</artifactId></dependency></dependencies></project>"
        ));
    }

    #[test]
    fn an_unreadable_pom_gives_no_features() {
        let t = Tree::new("mvn-broken");
        t.write("pom.xml", "<project><unclosed>");
        let b = load(&t.0).unwrap();
        assert_eq!(b.features, Features::default());
    }
}
