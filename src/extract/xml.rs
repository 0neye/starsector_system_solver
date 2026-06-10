//! Arena-based XML tree parsing for Starsector save files.

use std::collections::HashMap;

use quick_xml::events::Event;
use quick_xml::reader::Reader;

/// One XML node stored in an arena.
#[derive(Debug, Clone)]
pub struct XmlNode {
    tag: String,
    attrs: HashMap<String, String>,
    text: String,
    children: Vec<usize>,
    parent: Option<usize>,
}

impl XmlNode {
    pub fn tag(&self) -> &str {
        &self.tag
    }

    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.get(key).map(|value| value.as_str())
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn children<'a>(&'a self, doc: &'a XmlDoc) -> XmlChildren<'a> {
        XmlChildren {
            doc,
            iter: self.children.iter(),
        }
    }

    pub fn child_by_tag<'a>(&'a self, doc: &'a XmlDoc, tag: &str) -> Option<&'a XmlNode> {
        self.children
            .iter()
            .copied()
            .map(|idx| &doc.nodes[idx])
            .find(|node| node.tag() == tag)
    }

    pub fn resolve<'a>(&'a self, doc: &'a XmlDoc) -> &'a XmlNode {
        let mut current = self;
        let mut guard = 0usize;

        while let Some(ref_attr) = current.attr("ref") {
            let Some(z) = ref_attr.parse::<i64>().ok() else {
                break;
            };
            let Some(&next_idx) = doc.z_index.get(&z) else {
                break;
            };
            let next = &doc.nodes[next_idx];
            if std::ptr::eq(current, next) {
                break;
            }
            current = next;
            guard += 1;
            if guard > doc.nodes.len() {
                break;
            }
        }

        current
    }

    pub(crate) fn parent(&self) -> Option<usize> {
        self.parent
    }
}

pub struct XmlChildren<'a> {
    doc: &'a XmlDoc,
    iter: std::slice::Iter<'a, usize>,
}

impl<'a> Iterator for XmlChildren<'a> {
    type Item = &'a XmlNode;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|idx| &self.doc.nodes[*idx])
    }
}

/// XML arena document.
#[derive(Debug, Clone)]
pub struct XmlDoc {
    pub(crate) nodes: Vec<XmlNode>,
    pub(crate) z_index: HashMap<i64, usize>,
    root: usize,
}

impl XmlDoc {
    pub fn root(&self) -> &XmlNode {
        &self.nodes[self.root]
    }

    pub fn node(&self, idx: usize) -> &XmlNode {
        &self.nodes[idx]
    }

    pub(crate) fn node_by_z(&self, z: i64) -> Option<&XmlNode> {
        self.z_index.get(&z).map(|idx| &self.nodes[*idx])
    }
}

/// Parse a document into an arena-backed tree.
pub fn parse(xml_text: &str) -> XmlDoc {
    let mut reader = Reader::from_str(xml_text);
    reader.config_mut().trim_text(true);

    let mut doc = XmlDoc {
        nodes: Vec::new(),
        z_index: HashMap::new(),
        root: 0,
    };

    doc.nodes.push(XmlNode {
        tag: "#document".to_string(),
        attrs: HashMap::new(),
        text: String::new(),
        children: Vec::new(),
        parent: None,
    });

    let mut stack = vec![0usize];
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let attrs = collect_attrs(&reader, event.attributes());
                let idx = push_node(&mut doc, &stack, event.name().as_ref(), attrs);
                stack.push(idx);
            }
            Ok(Event::Empty(event)) => {
                let attrs = collect_attrs(&reader, event.attributes());
                let _ = push_node(&mut doc, &stack, event.name().as_ref(), attrs);
            }
            Ok(Event::End(_)) => {
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            Ok(Event::Text(event)) => {
                let text = String::from_utf8_lossy(event.as_ref()).into_owned();
                append_text(&mut doc, *stack.last().unwrap(), text);
            }
            Ok(Event::CData(event)) => {
                let text = String::from_utf8_lossy(event.as_ref()).into_owned();
                append_text(&mut doc, *stack.last().unwrap(), text);
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    doc
}

fn collect_attrs(
    reader: &Reader<&[u8]>,
    mut attrs: quick_xml::events::attributes::Attributes<'_>,
) -> HashMap<String, String> {
    let mut map = HashMap::new();

    for attr in attrs.with_checks(false) {
        let Ok(attr) = attr else {
            continue;
        };
        let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
        let value = attr
            .decode_and_unescape_value(reader.decoder())
            .map(|cow| cow.into_owned())
            .unwrap_or_else(|_| String::from_utf8_lossy(attr.value.as_ref()).into_owned());
        map.insert(key, value);
    }

    map
}

fn push_node(
    doc: &mut XmlDoc,
    stack: &[usize],
    tag_bytes: &[u8],
    attrs: HashMap<String, String>,
) -> usize {
    let tag = String::from_utf8_lossy(tag_bytes).into_owned();
    let parent = Some(*stack.last().unwrap());
    let idx = doc.nodes.len();

    if let Some(z) = attrs.get("z").and_then(|value| value.parse::<i64>().ok()) {
        doc.z_index.insert(z, idx);
    }

    doc.nodes.push(XmlNode {
        tag,
        attrs,
        text: String::new(),
        children: Vec::new(),
        parent,
    });

    doc.nodes[parent.unwrap()].children.push(idx);
    idx
}

fn append_text(doc: &mut XmlDoc, idx: usize, text: String) {
    if !text.is_empty() {
        doc.nodes[idx].text.push_str(&text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tree_and_resolves_refs() {
        let xml = r#"
<root z="1">
  <thing z="2">
    <name>def</name>
    <child ref="3" />
  </thing>
  <thing z="3">
    <name>ref target</name>
  </thing>
</root>
"#;
        let doc = parse(xml);
        let root = doc.root();
        assert_eq!(root.tag(), "#document");

        let root_elem = root.child_by_tag(&doc, "root").unwrap();
        assert_eq!(root_elem.attr("z"), Some("1"));
        let thing = root_elem.child_by_tag(&doc, "thing").unwrap();
        assert_eq!(thing.child_by_tag(&doc, "name").unwrap().text(), "def");
        let child = thing.child_by_tag(&doc, "child").unwrap();
        assert_eq!(child.resolve(&doc).attr("z"), Some("3"));
        assert_eq!(child.resolve(&doc).child_by_tag(&doc, "name").unwrap().text(), "ref target");
    }
}
