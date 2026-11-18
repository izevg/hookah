//! The module graph of a Maven reactor, read from its `pom.xml` files.
//!
//! `-amd` of Maven adds the modules that depend on the selected one, but `-am` only adds what the
//! selected modules need, not what the dependents need: a dependent's other reactor dependencies
//! then resolve from `~/.m2`, which `-o` forbids filling. So hookah computes the dependents
//! itself and names them in `-pl`, which makes `-am` cover their upstream too.
//!
//! The graph follows what Maven's own reactor sorting follows:
//! - `<modules>`, recursively, for the members (a module entry may name a `pom.xml` file);
//! - `<dependencies>` of any type and classifier, so a `test-jar` (`<type>test-jar</type>` or
//!   `<classifier>tests</classifier>`) is an edge like any other;
//! - `<parent>`, `<build><plugins>` with their `<dependencies>`, and `<build><extensions>`.
//!
//! Limits: edges match on `groupId:artifactId` and ignore the version (a dependency on another
//! version of a member still counts, which can only add a module); `${project.groupId}` and
//! `${project.parent.groupId}` are resolved and any other property in a `groupId` matches every
//! group; an `artifactId` that is a property never matches; `<dependencyManagement>`, `<profiles>`,
//! and imported BOMs are not read. A pom that cannot be read is a module without edges.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use hookah_domain::path::normalize;

use super::relative_slashed;
use crate::ecosystems::java::xml::{self, Node};

/// The most poms read, so a cyclic or huge tree cannot make detection slow.
const MAX_MODULES: usize = 500;

/// A `groupId:artifactId`; `None` as group matches any group.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Coord {
    group: Option<String>,
    artifact: String,
}

/// One member of the reactor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Module {
    /// The directory relative to the reactor root, `/` separated; empty for the root itself.
    dir: String,
    own: Coord,
    uses: Vec<Coord>,
}

/// Every module of the reactor at `root`, the root included, in discovery order.
pub(super) fn modules(root: &Path) -> Vec<Module> {
    let mut out = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut todo = vec![normalize(root)];
    while let Some(dir) = todo.pop() {
        if out.len() >= MAX_MODULES || !seen.insert(dir.clone()) {
            continue;
        }
        let Some(pom) = read(&dir) else { continue };
        for entry in pom.at(&["modules", "module"]).iter().rev() {
            let path = dir.join(entry.text.trim());
            let member = if path.extension().is_some_and(|e| e == "xml") {
                path.parent().map(Path::to_path_buf).unwrap_or(path)
            } else {
                path
            };
            todo.push(normalize(&member));
        }
        out.push(module(root, &dir, &pom));
    }
    out
}

fn read(dir: &Path) -> Option<Node> {
    xml::parse(&std::fs::read_to_string(dir.join("pom.xml")).ok()?).ok()
}

fn module(root: &Path, dir: &Path, pom: &Node) -> Module {
    let parent_group = pom
        .child("parent")
        .and_then(|p| p.child_text("groupId"))
        .map(str::to_owned);
    let own_group = pom
        .child_text("groupId")
        .map(str::to_owned)
        .or_else(|| parent_group.clone());
    let resolve = |text: Option<&str>| -> Option<String> {
        let text = text?.trim();
        match text {
            "${project.groupId}" | "${pom.groupId}" | "${groupId}" => own_group.clone(),
            "${project.parent.groupId}" => parent_group.clone(),
            t if t.contains("${") || t.is_empty() => None,
            t => Some(t.to_owned()),
        }
    };
    let coord = |n: &Node| -> Option<Coord> {
        let artifact = n.child_text("artifactId").filter(|a| !a.contains("${"))?;
        Some(Coord {
            group: resolve(n.child_text("groupId")),
            artifact: artifact.to_owned(),
        })
    };

    let mut uses: Vec<Coord> = pom
        .at(&["dependencies", "dependency"])
        .into_iter()
        .filter_map(coord)
        .collect();
    uses.extend(pom.child("parent").and_then(coord));
    let plugins = pom.at(&["build", "plugins", "plugin"]);
    uses.extend(plugins.iter().copied().filter_map(coord));
    uses.extend(
        plugins
            .iter()
            .flat_map(|p| p.at(&["dependencies", "dependency"]))
            .filter_map(coord),
    );
    uses.extend(
        pom.at(&["build", "extensions", "extension"])
            .into_iter()
            .filter_map(coord),
    );
    let artifact = pom.child_text("artifactId").unwrap_or_default().to_owned();
    Module {
        dir: relative_slashed(dir, root).unwrap_or_default(),
        own: Coord {
            group: own_group,
            artifact,
        },
        uses,
    }
}

impl Module {
    /// Whether this module uses `other`.
    fn uses(&self, other: &Module) -> bool {
        self.dir != other.dir
            && self.uses.iter().any(|c| {
                c.artifact == other.own.artifact
                    && match (&c.group, &other.own.group) {
                        (Some(a), Some(b)) => a == b,
                        _ => true,
                    }
            })
    }
}

/// The directories of the modules that need the module in `of`, directly or through others,
/// sorted. `of` itself is never in the result.
pub(super) fn downstream(modules: &[Module], of: &str) -> Vec<String> {
    let mut found: BTreeSet<&str> = BTreeSet::new();
    let mut frontier: Vec<&Module> = modules.iter().filter(|m| m.dir == of).collect();
    // Each round adds the modules that use one found in the round before. A chain of dependents
    // is no longer than the list of modules, which also bounds the rounds.
    for _ in 0..modules.len() {
        let mut next = Vec::new();
        for m in modules {
            if m.dir != of && !found.contains(m.dir.as_str()) && frontier.iter().any(|c| m.uses(c))
            {
                found.insert(&m.dir);
                next.push(m);
            }
        }
        frontier = next;
    }
    found.into_iter().map(str::to_owned).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::tests::Tree;
    use super::*;

    fn pom(group: &str, artifact: &str, deps: &[(&str, &str, &str)]) -> String {
        let deps: String = deps
            .iter()
            .map(|(g, a, extra)| {
                format!(
                    "<dependency><groupId>{g}</groupId><artifactId>{a}</artifactId>{extra}</dependency>"
                )
            })
            .collect();
        format!(
            "<project><groupId>{group}</groupId><artifactId>{artifact}</artifactId><dependencies>{deps}</dependencies></project>"
        )
    }

    /// The topology of a real report: `login` needs `dicts` (changed) and also `sms`, `sms`'s
    /// test-jar, and `broker`, which `dicts` does not need.
    fn monorepo() -> Tree {
        let t = Tree::new("reactor");
        t.write(
            "pom.xml",
            "<project><groupId>g</groupId><artifactId>root</artifactId><modules>\
             <module>providers/dicts</module><module>login</module><module>libs/pom.xml</module>\
             <module>other</module></modules></project>",
        );
        t.write("providers/dicts/pom.xml", &pom("g", "dicts", &[]));
        t.write(
            "login/pom.xml",
            &pom(
                "g",
                "login",
                &[
                    ("g", "dicts", ""),
                    ("g", "sms", ""),
                    ("g", "sms", "<type>test-jar</type><scope>test</scope>"),
                    ("g", "broker", ""),
                ],
            ),
        );
        t.write(
            "libs/pom.xml",
            "<project><groupId>g</groupId><artifactId>libs</artifactId><packaging>pom</packaging>\
             <modules><module>sms</module><module>broker</module></modules></project>",
        );
        t.write("libs/sms/pom.xml", &pom("g", "sms", &[]));
        t.write("libs/broker/pom.xml", &pom("g", "broker", &[]));
        t.write(
            "other/pom.xml",
            &pom("g", "other", &[("org.x", "dicts", "")]),
        );
        t
    }

    #[test]
    fn members_are_found_through_nested_aggregators_and_pom_files() {
        let t = monorepo();
        let dirs: BTreeSet<String> = modules(&t.0).into_iter().map(|m| m.dir).collect();
        assert_eq!(
            dirs.into_iter().collect::<Vec<_>>(),
            [
                "",
                "libs",
                "libs/broker",
                "libs/sms",
                "login",
                "other",
                "providers/dicts"
            ]
        );
    }

    #[test]
    fn downstream_is_the_modules_that_need_the_changed_one() {
        let t = monorepo();
        let m = modules(&t.0);
        // `other` uses a `dicts` of another group, which is not the member.
        assert_eq!(downstream(&m, "providers/dicts"), ["login"]);
        assert_eq!(downstream(&m, "libs/sms"), ["login"]);
        assert!(downstream(&m, "login").is_empty());
        assert!(downstream(&m, "nowhere").is_empty());
    }

    #[test]
    fn downstream_is_transitive_and_survives_cycles() {
        let t = Tree::new("reactor-chain");
        t.write(
            "pom.xml",
            "<project><groupId>g</groupId><artifactId>r</artifactId><modules>\
             <module>a</module><module>b</module><module>c</module></modules></project>",
        );
        t.write("a/pom.xml", &pom("g", "a", &[("g", "c", "")]));
        t.write("b/pom.xml", &pom("g", "b", &[("g", "a", "")]));
        t.write("c/pom.xml", &pom("g", "c", &[("g", "b", "")]));
        let m = modules(&t.0);
        assert_eq!(downstream(&m, "a"), ["b", "c"]);
        assert_eq!(downstream(&m, "c"), ["a", "b"]);
    }

    #[test]
    fn parents_plugins_and_extensions_are_edges_and_groups_resolve() {
        let t = Tree::new("reactor-edges");
        t.write(
            "pom.xml",
            "<project><groupId>g</groupId><artifactId>r</artifactId><modules>\
             <module>base</module><module>child</module><module>tool</module><module>ext</module>\
             <module>prop</module><module>lone</module></modules></project>",
        );
        t.write("base/pom.xml", &pom("g", "base", &[]));
        t.write(
            "child/pom.xml",
            "<project><parent><groupId>g</groupId><artifactId>base</artifactId></parent>\
             <artifactId>child</artifactId></project>",
        );
        t.write(
            "tool/pom.xml",
            "<project><groupId>g</groupId><artifactId>tool</artifactId><build><plugins><plugin>\
             <groupId>g</groupId><artifactId>x-plugin</artifactId><dependencies><dependency>\
             <groupId>g</groupId><artifactId>base</artifactId></dependency></dependencies>\
             </plugin></plugins></build></project>",
        );
        t.write(
            "ext/pom.xml",
            "<project><groupId>g</groupId><artifactId>ext</artifactId><build><extensions><extension>\
             <groupId>g</groupId><artifactId>base</artifactId></extension></extensions></build></project>",
        );
        t.write(
            "prop/pom.xml",
            "<project><parent><groupId>g</groupId><artifactId>none</artifactId></parent>\
             <artifactId>prop</artifactId><dependencies>\
             <dependency><groupId>${project.parent.groupId}</groupId><artifactId>base</artifactId></dependency>\
             <dependency><groupId>${some.other}</groupId><artifactId>${dyn}</artifactId></dependency>\
             </dependencies></project>",
        );
        t.write(
            "lone/pom.xml",
            "<project><groupId>g</groupId><artifactId>lone</artifactId><dependencies>\
             <dependency><groupId>${project.groupId}</groupId><artifactId>lone</artifactId></dependency>\
             </dependencies></project>",
        );
        let m = modules(&t.0);
        assert_eq!(downstream(&m, "base"), ["child", "ext", "prop", "tool"]);
        assert!(downstream(&m, "lone").is_empty(), "never its own dependent");
    }

    #[test]
    fn a_property_group_matches_every_group() {
        let t = Tree::new("reactor-prop");
        t.write(
            "pom.xml",
            "<project><groupId>g</groupId><artifactId>r</artifactId><modules>\
             <module>a</module><module>b</module></modules></project>",
        );
        t.write("a/pom.xml", &pom("g", "a", &[]));
        t.write("b/pom.xml", &pom("g", "b", &[("${grp}", "a", "")]));
        assert_eq!(downstream(&modules(&t.0), "a"), ["b"]);
    }

    #[test]
    fn a_module_listed_twice_is_one_module() {
        let t = Tree::new("reactor-twice");
        t.write(
            "pom.xml",
            "<project><artifactId>r</artifactId><modules><module>a</module><module>./a</module>\
             <module>a/pom.xml</module></modules></project>",
        );
        t.write("a/pom.xml", &pom("g", "a", &[]));
        assert_eq!(modules(&t.0).len(), 2);
    }

    #[test]
    fn unreadable_poms_and_missing_members_are_skipped() {
        let t = Tree::new("reactor-broken");
        t.write(
            "pom.xml",
            "<project><modules><module>bad</module><module>gone</module><module>../r</module>\
             </modules></project>",
        );
        t.write("bad/pom.xml", "<project><unclosed>");
        let m = modules(&t.0);
        assert_eq!(m.len(), 1);
        assert!(downstream(&m, "bad").is_empty());
    }
}
