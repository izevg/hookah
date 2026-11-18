//! A small XML tree for build files and tool reports.
//!
//! `pom.xml`, Checkstyle, PMD, SpotBugs, JUnit, and PIT reports are all plain element trees, so one
//! tree type reads them all. Namespaces are dropped (names are local names), entities and CDATA are
//! resolved into text, and comments, processing instructions, and the DOCTYPE are skipped.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

/// An element with its attributes, the text directly inside it, and its child elements.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Node {
    /// Local name, without a namespace prefix.
    pub name: String,
    attrs: Vec<(String, String)>,
    /// The text directly inside the element, children's text excluded, untrimmed.
    pub text: String,
    /// Child elements in document order.
    pub children: Vec<Node>,
}

impl Node {
    /// The value of attribute `name`.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The first child element called `name`.
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    /// The child elements called `name`.
    pub fn children_named(&self, name: &str) -> impl Iterator<Item = &Node> {
        let name = name.to_owned();
        self.children.iter().filter(move |c| c.name == name)
    }

    /// The elements reached by following `names` one level at a time.
    pub fn at(&self, names: &[&str]) -> Vec<&Node> {
        let mut level = vec![self];
        for name in names {
            level = level
                .into_iter()
                .flat_map(|n| n.children_named(name))
                .collect();
        }
        level
    }

    /// The trimmed text of the first child element called `name`.
    pub fn child_text(&self, name: &str) -> Option<&str> {
        self.child(name).map(|c| c.text.trim())
    }

    /// Every element below this one (not itself), depth first.
    pub fn descendants(&self) -> Vec<&Node> {
        let mut out = Vec::new();
        for child in &self.children {
            out.push(child);
            out.extend(child.descendants());
        }
        out
    }
}

/// Parses `text` into its root element. `Err` carries a short reason.
pub fn parse(text: &str) -> Result<Node, String> {
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;
    loop {
        let event = reader.read_event().map_err(|e| e.to_string())?;
        if matches!(event, Event::Eof) {
            break;
        }
        match event {
            Event::Start(e) => stack.push(element(&e)?),
            Event::Empty(e) => close(element(&e)?, &mut stack, &mut root),
            Event::End(_) => {
                let node = stack.pop().ok_or("unbalanced end tag")?;
                close(node, &mut stack, &mut root);
            }
            Event::Text(t) => {
                let text = t.decode().map_err(|e| e.to_string())?;
                push_text(&mut stack, &text);
            }
            Event::CData(c) => {
                let text = c.decode().map_err(|e| e.to_string())?;
                push_text(&mut stack, &text);
            }
            Event::GeneralRef(r) => {
                let name = r.decode().map_err(|e| e.to_string())?;
                let resolved = match r.resolve_char_ref().map_err(|e| e.to_string())? {
                    Some(c) => c.to_string(),
                    None => predefined(&name).unwrap_or_default().to_string(),
                };
                push_text(&mut stack, &resolved);
            }
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err("unclosed element".into());
    }
    root.ok_or_else(|| "no root element".into())
}

fn element(start: &BytesStart<'_>) -> Result<Node, String> {
    let name = String::from_utf8_lossy(start.local_name().as_ref()).into_owned();
    let mut attrs = Vec::new();
    for attr in start.attributes() {
        let attr = attr.map_err(|e| e.to_string())?;
        let key = String::from_utf8_lossy(attr.key.local_name().as_ref()).into_owned();
        let value = attr.unescape_value().map_err(|e| e.to_string())?;
        attrs.push((key, value.into_owned()));
    }
    Ok(Node {
        name,
        attrs,
        ..Node::default()
    })
}

fn close(node: Node, stack: &mut [Node], root: &mut Option<Node>) {
    match stack.last_mut() {
        Some(parent) => parent.children.push(node),
        None => *root = Some(node),
    }
}

fn push_text(stack: &mut [Node], text: &str) {
    if let Some(top) = stack.last_mut() {
        top.text.push_str(text);
    }
}

fn predefined(name: &str) -> Option<&'static str> {
    Some(match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        _ => return None,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn reads_names_attributes_text_and_children() {
        let n = parse(
            "<?xml version=\"1.0\"?><!DOCTYPE x><!-- c --><a:r xmlns:a=\"u\" k=\"v\"><b x='1'/><b x='2'>t</b></a:r>",
        )
        .unwrap();
        assert_eq!(n.name, "r");
        assert_eq!(n.attr("k"), Some("v"));
        assert_eq!(n.attr("zz"), None);
        let bs: Vec<_> = n.children_named("b").collect();
        assert_eq!(bs.len(), 2);
        assert_eq!(bs[0].attr("x"), Some("1"));
        assert_eq!(bs[1].text, "t");
        assert!(n.child("b").is_some());
        assert!(n.child("c").is_none());
    }

    #[test]
    fn entities_and_cdata_become_text() {
        let n =
            parse("<a v=\"&apos;x&apos; &lt;\">1 &amp; 2 &#65;&#x42;&bogus;<![CDATA[<z>]]></a>")
                .unwrap();
        assert_eq!(n.attr("v"), Some("'x' <"));
        assert_eq!(n.text, "1 & 2 AB<z>");
        assert_eq!(parse("<a>&lt;&gt;&quot;&apos;</a>").unwrap().text, "<>\"'");
    }

    #[test]
    fn text_is_kept_per_element() {
        let n = parse("<a>x<b>y</b>z</a>").unwrap();
        assert_eq!(n.text, "xz");
        assert_eq!(n.child_text("b"), Some("y"));
        assert_eq!(n.child_text("q"), None);
        assert_eq!(
            parse("<a><b> y </b></a>").unwrap().child_text("b"),
            Some("y")
        );
    }

    #[test]
    fn walks_paths_and_descendants() {
        let n = parse("<p><x><y>1</y><y>2</y></x><x><y>3</y></x><z><y>4</y></z></p>").unwrap();
        let ys = n.at(&["x", "y"]);
        assert_eq!(
            ys.iter().map(|y| y.text.as_str()).collect::<Vec<_>>(),
            ["1", "2", "3"]
        );
        assert!(n.at(&["x", "q"]).is_empty());
        assert_eq!(n.at(&[]).len(), 1);
        let all: Vec<_> = n.descendants().iter().map(|d| d.name.as_str()).collect();
        assert_eq!(all, ["x", "y", "y", "x", "y", "z", "y"]);
    }

    #[test]
    fn broken_input_is_an_error() {
        assert!(parse("").is_err());
        assert!(parse("<a><b></a>").is_err());
        assert!(parse("<a>").is_err());
        assert!(parse("</a>").is_err());
        assert!(parse("just text").is_err());
    }
}
