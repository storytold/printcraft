//! The data model (`datasets` packet, XFA 3.3 part 1, "Data Binding"): reading the values a
//! form was saved with, and writing the values of its fields back, so Adobe's viewers (which
//! lay the form out from the XFA packets) show what was filled in here.
//!
//! Binding is the default "normal" one: a field's data node is named after the field and nested
//! under nodes named after its named ancestor subforms, repeated instances as repeated sibling
//! elements. Areas are named in SOM expressions but are not data scopes, so their contents bind
//! under the area's parent ([`data_path`]). Explicit `bind ref` expressions are not followed.

use std::collections::HashMap;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};

use crate::XfaError;
use crate::model::{Node, Template};
use crate::packets::{Encoding, encode};

const XFA_DATA_NS: &str = "http://www.xfa.org/schema/xfa-data/1.0/";
/// Most data nodes read.
const MAX_NODES: u32 = 1_000_000;
const MAX_DEPTH: usize = 64;

/// One element of the data document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DataNode {
    pub name: String,
    pub text: String,
    pub children: Vec<DataNode>,
}

/// A path into the data: `(name, index among same-named siblings)` per level.
pub type DataPath = Vec<(String, usize)>;

impl DataNode {
    /// The node at `path` below this one.
    pub fn get(&self, path: &[(String, usize)]) -> Option<&DataNode> {
        let mut node = self;
        for (name, idx) in path {
            node = node.children.iter().filter(|c| &c.name == name).nth(*idx)?;
        }
        Some(node)
    }

    /// How many children of the node at `parent` are named `name`.
    pub fn count(&self, parent: &[(String, usize)], name: &str) -> usize {
        self.get(parent).map_or(0, |n| n.children.iter().filter(|c| c.name == name).count())
    }

    pub fn text_at(&self, path: &[(String, usize)]) -> Option<&str> {
        self.get(path).map(|n| n.text.as_str())
    }

    /// The value at `path`: its text, or, for a container whose children are all `value`
    /// elements (one way a multi-select list's picks are stored), those on separate lines.
    pub fn value_at(&self, path: &[(String, usize)]) -> Option<String> {
        let n = self.get(path)?;
        if n.children.is_empty() || !n.children.iter().all(|c| c.name == "value") {
            return Some(n.text.clone());
        }
        Some(n.children.iter().map(|c| c.text.trim()).filter(|t| !t.is_empty()).take(1000).collect::<Vec<_>>().join("\n"))
    }
}

fn to_node(n: roxmltree::Node, depth: usize) -> DataNode {
    let name = n.tag_name().name().to_string();
    let children: Vec<DataNode> =
        if depth < MAX_DEPTH { n.children().filter(|c| c.is_element()).map(|c| to_node(c, depth + 1)).collect() } else { Vec::new() };
    // The text of a leaf is its value; a container's own text is whitespace.
    let text = if children.is_empty() { n.children().filter_map(|c| c.text()).collect::<String>() } else { String::new() };
    DataNode { name, text, children }
}

/// The `xfa:data` element of the datasets packet in `xdp` (or a bare datasets document), as a
/// tree whose root is that element.
pub fn parse_datasets(xdp: &str) -> Option<DataNode> {
    let opts = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES };
    let doc = roxmltree::Document::parse_with_options(xdp, opts).ok()?;
    let datasets = doc.descendants().find(|n| n.is_element() && n.tag_name().name() == "datasets")?;
    let data = datasets.children().find(|n| n.is_element() && n.tag_name().name() == "data")?;
    Some(to_node(data, 0))
}

/// The data path a SOM expression names: `form[0].#subform[0].name[1]` → `[(form, 0), (name, 1)]`.
/// Unnamed containers (`#subform`) have no data node.
pub fn som_to_path(som: &str) -> DataPath {
    som.split('.')
        .filter_map(|seg| {
            let seg = seg.trim();
            if seg.is_empty() || seg.starts_with('#') || seg.starts_with('$') {
                return None;
            }
            let (name, idx) = match seg.split_once('[') {
                Some((n, rest)) => (n, rest.trim_end_matches(']').parse::<usize>().unwrap_or(0)),
                None => (seg, 0),
            };
            Some((name.to_string(), idx.min(100_000)))
        })
        .collect()
}

/// The data path the object at `som` binds to: [`som_to_path`] without the areas on the way.
/// An area is a named container in the form, so it is part of SOM expressions, but only
/// subforms, exclusion groups and fields are data scopes: what an area holds binds under the
/// area's parent, as Adobe's viewers write it. The template tells which segments are areas;
/// segments it doesn't have (page-set objects, or a SOM from another template) are kept.
pub fn data_path(tpl: &Template, som: &str) -> DataPath {
    let mut out = DataPath::new();
    // The template children the next segment is looked up in; `None` until the root matched,
    // and again once a segment is not a container the template has.
    let mut level: Option<&[Node]> = None;
    for (i, (name, index)) in som_to_path(som).into_iter().enumerate() {
        let found = match level {
            None if i == 0 && tpl.root.common.name.as_deref() == Some(name.as_str()) => Some(Container::Children(&tpl.root.children)),
            None => None,
            Some(children) => match named(children, &name, 0) {
                Some(Node::Subform(sf)) => Some(Container::Children(&sf.children)),
                Some(Node::Area(a)) => Some(Container::Area(&a.children)),
                _ => None,
            },
        };
        match found {
            Some(Container::Area(children)) => level = Some(children),
            Some(Container::Children(children)) => {
                level = Some(children);
                out.push((name, index));
            }
            None => {
                level = None;
                out.push((name, index));
            }
        }
    }
    out
}

enum Container<'a> {
    Children(&'a [Node]),
    /// Contents bind under the area's parent.
    Area(&'a [Node]),
}

/// The child of `nodes` named `name`, looking through unnamed subforms and areas (which have no
/// SOM segment of their own).
fn named<'a>(nodes: &'a [Node], name: &str, depth: usize) -> Option<&'a Node> {
    if depth > MAX_DEPTH {
        return None;
    }
    nodes.iter().find(|n| n.common().name.as_deref() == Some(name)).or_else(|| {
        nodes.iter().find_map(|n| match n {
            Node::Subform(sf) if sf.common.name.is_none() => named(&sf.children, name, depth + 1),
            Node::Area(a) if a.common.name.is_none() => named(&a.children, name, depth + 1),
            _ => None,
        })
    })
}

/// What a field holds, for writing the datasets.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldData {
    Text(String),
    /// Checked or not.
    Check(bool),
    /// The selected button's on value, if any.
    Radio(Option<String>),
    /// Nothing to write (buttons, signatures).
    None,
}

/// One field of the AcroForm, as the engine sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldDatum {
    /// The terminal field dictionary (where `/PCSom` and `/PCItems` live).
    pub obj: ObjRef,
    /// The fully qualified field name (a static form's SOM path).
    pub name: String,
    pub data: FieldData,
}

fn text_of(doc: &Document, d: &Dict, key: &[u8]) -> Option<String> {
    d.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_string().map(|s| s.to_text()))
}

/// The SOM path of a field: its `/PCSom` when PdfCraft generated it, else its name.
fn som_of(doc: &Document, f: &FieldDatum) -> String {
    doc.get(f.obj).as_dict().and_then(|d| text_of(doc, d, crate::pdf::SOM_KEY)).unwrap_or_else(|| f.name.clone())
}

/// The on and off item values of a check box (`/PCItems`), or the widget's on state.
fn items_of(doc: &Document, f: &FieldDatum) -> (String, String) {
    let d = doc.get(f.obj);
    let items: Vec<String> = d
        .as_dict()
        .and_then(|d| d.get(b"PCItems").map(|o| doc.resolve(o)))
        .and_then(|a| a.as_array().map(|a| a.iter().filter_map(|x| x.as_string().map(|s| s.to_text())).collect()))
        .unwrap_or_default();
    let on = items.first().cloned().unwrap_or_else(|| "1".into());
    let off = items.get(1).cloned().unwrap_or_default();
    (on, off)
}

/// The Acrobat date pattern a field's format action names (`AFDate_FormatEx("yyyy-mm-dd")`).
fn date_pattern_of(doc: &Document, f: &FieldDatum) -> Option<String> {
    let d = doc.get(f.obj);
    let aa = doc.resolve(d.as_dict()?.get(b"AA")?);
    let fa = doc.resolve(aa.as_dict()?.get(b"F")?);
    let js = text_of(doc, fa.as_dict()?, b"JS")?;
    let i = js.find("AFDate_FormatEx(")?;
    let rest = &js[i + "AFDate_FormatEx(".len()..];
    let q = rest.find('"')?;
    let body = &rest[q + 1..];
    Some(body[..body.find('"')?].to_string())
}

/// The tokens of a date pattern: `yyyy`, `yy`, `mm`, `m`, `dd`, `d`, or a literal character.
fn date_tokens(pattern: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i].to_ascii_lowercase();
        if "ymd".contains(c) {
            let mut j = i;
            while j < chars.len() && chars[j].to_ascii_lowercase() == c {
                j += 1;
            }
            out.push(std::iter::repeat_n(c, j - i).collect());
            i = j;
        } else {
            out.push(chars[i].to_string());
            i += 1;
        }
    }
    out
}

/// `2001-02-03` shown in `pattern` (`mm/dd/yyyy` → `02/03/2001`). Unknown patterns and
/// non-dates come back as they are.
pub fn iso_to_pattern(iso: &str, pattern: &str) -> String {
    let parts: Vec<&str> = iso.trim().splitn(3, '-').collect();
    let [y, m, d] = parts[..] else { return iso.to_string() };
    let (Ok(y), Ok(m), Ok(d)) = (y.parse::<u32>(), m.parse::<u32>(), d.parse::<u32>()) else { return iso.to_string() };
    let mut out = String::new();
    for t in date_tokens(pattern) {
        match t.as_str() {
            "yyyy" => out.push_str(&format!("{y:04}")),
            "yy" => out.push_str(&format!("{:02}", y % 100)),
            "mm" => out.push_str(&format!("{m:02}")),
            "m" => out.push_str(&m.to_string()),
            "dd" => out.push_str(&format!("{d:02}")),
            "d" => out.push_str(&d.to_string()),
            other => out.push_str(other),
        }
    }
    out
}

/// A date typed as `pattern` back to `yyyy-mm-dd`. `None` when it doesn't match.
pub fn pattern_to_iso(text: &str, pattern: &str) -> Option<String> {
    let text = text.trim();
    let (mut y, mut m, mut d) = (None, None, None);
    let mut rest = text;
    let tokens = date_tokens(pattern);
    for (i, t) in tokens.iter().enumerate() {
        match t.as_str() {
            "yyyy" | "yy" | "mm" | "m" | "dd" | "d" => {
                // Digits up to the next literal (or a fixed width).
                let fixed = matches!(t.as_str(), "yyyy" | "yy" | "mm" | "dd");
                let width = if fixed {
                    t.len()
                } else {
                    rest.chars().take_while(|c| c.is_ascii_digit()).count().min(if t == "d" || t == "m" { 2 } else { 4 })
                };
                let digits: String = rest.chars().take(width).collect();
                if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
                    return None;
                }
                let v: u32 = digits.parse().ok()?;
                rest = &rest[digits.len()..];
                match t.as_str() {
                    "yyyy" => y = Some(v),
                    "yy" => y = Some(if v < 50 { 2000 + v } else { 1900 + v }),
                    "mm" | "m" => m = Some(v),
                    _ => d = Some(v),
                }
            }
            lit => {
                rest = rest.strip_prefix(lit)?;
            }
        }
        let _ = i;
    }
    if !rest.is_empty() {
        return None;
    }
    let (y, m, d) = (y?, m?, d?);
    (1..=12).contains(&m).then(|| format!("{y:04}-{m:02}-{d:02}")).filter(|_| (1..=31).contains(&d))
}

fn xml_name(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' { c } else { '_' }).collect();
    if out.is_empty() || !(out.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')) {
        out.insert(0, '_');
    }
    out
}

/// `s` as XML character data. Characters XML 1.0 cannot hold are left out; the flag says
/// whether any were.
fn escape(s: &str) -> (String, bool) {
    let mut out = String::with_capacity(s.len());
    let mut dropped = false;
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // Parsers turn a raw carriage return into a line feed; a reference keeps it.
            '\r' => out.push_str("&#xD;"),
            '\u{FFFE}' | '\u{FFFF}' => dropped = true,
            c if c.is_control() && c != '\n' && c != '\t' => dropped = true,
            c => out.push(c),
        }
    }
    (out, dropped)
}

/// Most data nodes one write may create (instances skipped by a SOM index count too).
const MAX_NEW_NODES: usize = 20_000;
/// Highest same-name sibling index a write follows.
const MAX_INDEX: usize = 10_000;

/// A data tree being built for writing.
#[derive(Default)]
struct Build {
    name: String,
    text: String,
    children: Vec<Build>,
    /// Positions in `children` by name, in order.
    by_name: HashMap<String, Vec<usize>>,
}

impl Build {
    /// The node at `path` below this one, created as needed. `None` when the path is deeper
    /// than [`MAX_DEPTH`], an index is above [`MAX_INDEX`], or creating it would spend more
    /// than `budget` new nodes.
    fn at(&mut self, path: &[(String, usize)], budget: &mut usize) -> Option<&mut Build> {
        if path.len() > MAX_DEPTH {
            return None;
        }
        let mut node = self;
        for (name, idx) in path {
            if *idx > MAX_INDEX {
                return None;
            }
            let name = xml_name(name);
            let have = node.by_name.get(&name).map_or(0, Vec::len);
            if *idx >= have {
                let need = idx - have + 1;
                if need > *budget {
                    return None;
                }
                *budget -= need;
                for _ in 0..need {
                    let pos = node.children.len();
                    node.by_name.entry(name.clone()).or_default().push(pos);
                    node.children.push(Build { name: name.clone(), ..Default::default() });
                }
            }
            let pos = *node.by_name.get(&name)?.get(*idx)?;
            node = node.children.get_mut(pos)?;
        }
        Some(node)
    }

    /// The tree as XML. Depth is bounded by [`Build::at`].
    fn write(&self, out: &mut String) {
        if self.children.is_empty() {
            out.push_str(&format!("<{}>{}</{}>", self.name, escape(&self.text).0, self.name));
            return;
        }
        out.push_str(&format!("<{}>", self.name));
        self.write_children(out);
        out.push_str(&format!("</{}>", self.name));
    }

    fn write_children(&self, out: &mut String) {
        for c in &self.children {
            c.write(out);
        }
    }
}

/// A value to write: where (the field's SOM path, and the data path it binds to) and what.
struct Want {
    som: String,
    path: DataPath,
    text: String,
    /// Create the node even for an empty value (an instance of a repeating subform).
    ensure: bool,
}

/// `tpl`, or the document's own template when none is given (parsed once).
fn with_template<T>(doc: &Document, tpl: Option<&Template>, f: impl FnOnce(Option<&Template>) -> T) -> T {
    match tpl {
        Some(t) => f(Some(t)),
        None => {
            let parsed = crate::script::template_of(doc).ok();
            f(parsed.as_ref())
        }
    }
}

/// [`data_path`] when the template is known, else [`som_to_path`].
fn path_of(tpl: Option<&Template>, som: &str) -> DataPath {
    match tpl {
        Some(t) => data_path(t, som),
        None => som_to_path(som),
    }
}

fn wanted(doc: &Document, fields: &[FieldDatum], tpl: Option<&Template>) -> Vec<Want> {
    let mut out = Vec::new();
    for f in fields {
        let som = som_of(doc, f);
        let path = path_of(tpl, &som);
        if path.is_empty() {
            continue;
        }
        let text = match &f.data {
            FieldData::Text(t) => match date_pattern_of(doc, f) {
                Some(p) => pattern_to_iso(t, &p).unwrap_or_else(|| t.clone()),
                None => t.clone(),
            },
            FieldData::Check(on) => {
                let (on_item, off_item) = items_of(doc, f);
                if *on { on_item } else { off_item }
            }
            FieldData::Radio(sel) => sel.clone().unwrap_or_default(),
            FieldData::None => continue,
        };
        out.push(Want { som, path, text, ensure: false });
    }
    out
}

/// What a rewrite of the datasets packet does.
#[derive(Clone, Copy)]
enum Op<'a> {
    /// Put these values in (adding nodes as needed).
    Values(&'a [Want]),
    /// Take the node at this path out.
    Remove(&'a DataPath),
}

/// Note that a value could not be written (once per kind of reason, naming a few fields).
fn skipped(warnings: &mut Vec<String>, reason: &str, som: &str) {
    const MAX_WARNINGS: usize = 20;
    if let Some(w) = warnings.iter_mut().find(|w| w.starts_with(reason)) {
        if w.matches(", ").count() < 4 {
            w.push_str(&format!(", {som}"));
        } else if !w.ends_with('…') {
            w.push_str(", …");
        }
    } else if warnings.len() < MAX_WARNINGS {
        warnings.push(format!("{reason}: {som}"));
    }
}

const TOO_MANY: &str = "XFA data not written for fields whose data path is too deep or repeats too often";
const STRUCTURED: &str = "XFA data not written for fields whose data node holds structured content";
const CONFLICT: &str = "XFA data not written for fields bound to the same data node as another field";
const CONTROL: &str = "Characters XML can't hold were left out of the XFA data of";

/// Fresh data for `wants`: the children of an `xfa:data` element.
fn build_children(wants: &[Want], warnings: &mut Vec<String>) -> String {
    let mut root = Build::default();
    let mut budget = MAX_NEW_NODES;
    for w in wants {
        match root.at(&w.path, &mut budget) {
            Some(node) => node.text = w.text.clone(),
            None => skipped(warnings, TOO_MANY, &w.som),
        }
        if escape(&w.text).1 {
            skipped(warnings, CONTROL, &w.som);
        }
    }
    let mut out = String::new();
    root.write_children(&mut out);
    out
}

/// The `xfa:data` element for `fields`, built afresh.
pub fn build_data(doc: &Document, fields: &[FieldDatum]) -> String {
    let wants = with_template(doc, None, |t| wanted(doc, fields, t));
    format!("<xfa:data>{}</xfa:data>", build_children(&wants, &mut Vec::new()))
}

fn fresh_datasets(data_children: &str) -> String {
    format!("<xfa:datasets xmlns:xfa=\"{XFA_DATA_NS}\"><xfa:data>{data_children}</xfa:data></xfa:datasets>")
}

/// Where an element's start tag ends (the index of its `>`), skipping quoted attribute values.
fn start_tag_end(src: &str, range: &std::ops::Range<usize>) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut quote = None;
    for i in range.clone() {
        let b = *bytes.get(i)?;
        match (quote, b) {
            (Some(q), _) if b == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(b),
            (None, b'>') => return Some(i),
            _ => {}
        }
    }
    None
}

/// An element's qualified name as written.
fn qname<'s>(src: &'s str, range: &std::ops::Range<usize>) -> Option<&'s str> {
    let rest = src.get(range.start.checked_add(1)?..range.end)?;
    let end = rest.find(|c: char| c.is_whitespace() || c == '/' || c == '>')?;
    rest.get(..end).filter(|n| !n.is_empty())
}

/// One change to the packet text: replace `start..end` with `text`.
struct Edit {
    start: usize,
    end: usize,
    text: String,
}

/// The edit that puts `inner` (markup) at the end of `node`'s content.
fn append_edit(src: &str, node: roxmltree::Node, inner: &str) -> Option<Edit> {
    let range = node.range();
    let tag_end = start_tag_end(src, &range)?;
    if tag_end.checked_add(1)? == range.end && src.get(..tag_end)?.ends_with('/') {
        // `<a/>` becomes `<a>inner</a>`.
        let name = qname(src, &range)?;
        return Some(Edit { start: tag_end.checked_sub(1)?, end: range.end, text: format!(">{inner}</{name}>") });
    }
    let close = range.start + src.get(range.clone())?.rfind("</")?;
    Some(Edit { start: close, end: close, text: inner.to_string() })
}

/// The edit that makes `text` (escaped) the whole content of the leaf `node`.
fn replace_edit(src: &str, node: roxmltree::Node, text: &str) -> Option<Edit> {
    let range = node.range();
    let tag_end = start_tag_end(src, &range)?;
    if tag_end.checked_add(1)? == range.end && src.get(..tag_end)?.ends_with('/') {
        let name = qname(src, &range)?;
        return Some(Edit { start: tag_end.checked_sub(1)?, end: range.end, text: format!(">{text}</{name}>") });
    }
    let close = range.start + src.get(range.clone())?.rfind("</")?;
    (close > tag_end).then(|| Edit { start: tag_end + 1, end: close, text: text.to_string() })
}

/// What merging the values into a packet's text came to.
enum Merged {
    /// The data already holds every value.
    Unchanged,
    Changed(String),
    /// The text could not be merged into (the reason is in the warnings).
    Failed,
}

/// Where a datasets packet sits: one packet of an array (`Packet`), or inside the whole XDP.
#[derive(Clone, Copy, PartialEq)]
enum Within {
    Packet,
    Xdp,
}

const WRAP_OPEN: &str = "<pdfcraft-wrap xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\" xmlns:xdp=\"http://ns.adobe.com/xdp/\">";
const WRAP_CLOSE: &str = "</pdfcraft-wrap>";

/// Write `wants` into the data of the datasets packet in `text`, changing only what has to
/// change: values go into the data nodes they bind to, missing nodes are added, and everything
/// else (data no field binds to, other namespaces, attributes, comments, layout) stays as
/// written.
fn merge(text: &str, within: Within, op: Op, warnings: &mut Vec<String>) -> Merged {
    let wants: &[Want] = match op {
        Op::Values(w) => w,
        Op::Remove(_) => &[],
    };
    let opts = || roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES };
    // A packet of an array may use the prefixes the XDP envelope declares: read it inside
    // an element declaring the usual ones.
    let wrapped;
    let (src, shift, parsed) = match roxmltree::Document::parse_with_options(text, opts()) {
        Ok(d) => (text, 0, d),
        Err(first) => {
            wrapped = format!("{WRAP_OPEN}{text}{WRAP_CLOSE}");
            match (within, roxmltree::Document::parse_with_options(&wrapped, opts())) {
                (Within::Packet, Ok(d)) => (wrapped.as_str(), WRAP_OPEN.len(), d),
                _ => {
                    warnings.push(format!("The XFA data could not be read ({first}); the field values were not written to it"));
                    return Merged::Failed;
                }
            }
        }
    };
    let is = |n: &roxmltree::Node, name: &str| n.is_element() && n.tag_name().name() == name;
    let datasets = parsed
        .descendants()
        .find(|n| is(n, "datasets") && n.tag_name().namespace() == Some(XFA_DATA_NS))
        .or_else(|| parsed.descendants().find(|n| is(n, "datasets")));
    let mut edits = Vec::new();
    let mut data = None;
    match datasets {
        None if matches!(op, Op::Remove(_)) => return Merged::Unchanged,
        None if within == Within::Xdp => {
            let root = parsed.root_element();
            match append_edit(src, root, &fresh_datasets(&build_children(wants, warnings))) {
                Some(e) => edits.push(e),
                None => {
                    warnings.push("The XFA packets could not be extended with data; the field values were not written".into());
                    return Merged::Failed;
                }
            }
        }
        None => {
            warnings.push("The XFA datasets packet holds no datasets element; the field values were not written to it".into());
            return Merged::Failed;
        }
        Some(ds) => match ds.children().find(|n| is(n, "data")) {
            Some(d) => data = Some(d),
            None if matches!(op, Op::Remove(_)) => return Merged::Unchanged,
            None => {
                let prefix = qname(src, &ds.range()).and_then(|q| q.split_once(':')).map(|(p, _)| format!("{p}:")).unwrap_or_default();
                let element = format!("<{prefix}data>{}</{prefix}data>", build_children(wants, warnings));
                match append_edit(src, ds, &element) {
                    Some(e) => edits.push(e),
                    None => {
                        warnings.push("The XFA datasets could not be extended with data; the field values were not written".into());
                        return Merged::Failed;
                    }
                }
            }
        },
    }
    if let Some(data) = data {
        match op {
            Op::Values(w) => merge_values(src, data, w, &mut edits, warnings),
            Op::Remove(path) => {
                // The element at `path`, with the whitespace before it.
                let mut node = Some(data);
                for (name, idx) in path {
                    node = node.and_then(|n| n.children().filter(|c| c.is_element() && c.tag_name().name() == name).nth(*idx));
                }
                if let Some(n) = node {
                    let r = n.range();
                    let start = src.get(..r.start).map_or(r.start, |before| r.start - before.len() + before.trim_end().len());
                    edits.push(Edit { start, end: r.end, text: String::new() });
                }
            }
        }
    }
    if edits.is_empty() {
        return Merged::Unchanged;
    }
    // Apply from the end, so earlier positions stay valid; overlapping edits (two fields bound
    // to one node) keep the first.
    edits.sort_by_key(|e| (e.start, e.end));
    let mut kept: Vec<Edit> = Vec::with_capacity(edits.len());
    for e in edits {
        if kept.last().is_some_and(|k| e.start < k.end) {
            if kept.last().is_some_and(|k| k.text != e.text) && !warnings.iter().any(|w| w == CONFLICT) {
                warnings.push(CONFLICT.into());
            }
            continue;
        }
        kept.push(e);
    }
    let mut out = src.to_string();
    for e in kept.iter().rev() {
        if out.get(e.start..e.end).is_none() {
            warnings.push("The XFA data could not be updated; the field values were not written to it".into());
            return Merged::Failed;
        }
        out.replace_range(e.start..e.end, &e.text);
    }
    if shift > 0 {
        match out.get(shift..out.len().saturating_sub(WRAP_CLOSE.len())) {
            Some(inner) => out = inner.to_string(),
            None => return Merged::Failed,
        }
    }
    Merged::Changed(out)
}

/// The edits that put `wants` into the `data` element.
fn merge_values(src: &str, data: roxmltree::Node, wants: &[Want], edits: &mut Vec<Edit>, warnings: &mut Vec<String>) {
    // Element children by local name, per element visited (built once each).
    let mut index: HashMap<roxmltree::NodeId, HashMap<String, Vec<roxmltree::NodeId>>> = HashMap::new();
    let tree = data.document();
    // Nodes to add, per existing element they go into.
    let mut added: Vec<(roxmltree::NodeId, Build)> = Vec::new();
    let mut budget = MAX_NEW_NODES;
    for w in wants {
        let (escaped, dropped) = escape(&w.text);
        let mut node = data;
        let mut depth = 0;
        let mut have = 0;
        for (name, idx) in &w.path {
            let kids = index.entry(node.id()).or_insert_with(|| {
                let mut by: HashMap<String, Vec<roxmltree::NodeId>> = HashMap::new();
                for c in node.children().filter(|c| c.is_element()) {
                    by.entry(c.tag_name().name().to_string()).or_default().push(c.id());
                }
                by
            });
            let same = kids.get(name.as_str());
            match same.and_then(|v| v.get(*idx)).and_then(|id| tree.get_node(*id)) {
                Some(next) => {
                    node = next;
                    depth += 1;
                }
                None => {
                    have = same.map_or(0, Vec::len);
                    break;
                }
            }
        }
        if depth == w.path.len() {
            // The data node exists: give it the value, unless it already holds it. A node
            // only wanted to exist (a row instance) is left as it is.
            if w.ensure && w.text.is_empty() {
                continue;
            }
            let current: String = node.children().filter(|c| c.is_text()).filter_map(|c| c.text()).collect();
            if node.children().any(|c| c.is_element()) {
                let all: String = node.descendants().filter(|c| c.is_text()).filter_map(|c| c.text()).collect();
                if all.trim() != w.text.trim() {
                    skipped(warnings, STRUCTURED, &w.som);
                }
                continue;
            }
            if current == w.text {
                continue;
            }
            match replace_edit(src, node, &escaped) {
                Some(e) => edits.push(e),
                None => skipped(warnings, STRUCTURED, &w.som),
            }
        } else {
            // Missing: add the rest of the path below the deepest node that exists. An empty
            // value needs no node, unless the node itself is wanted (a new instance).
            if w.text.is_empty() && !w.ensure {
                continue;
            }
            let mut rest: DataPath = w.path.get(depth..).unwrap_or_default().to_vec();
            if let Some(first) = rest.first_mut() {
                first.1 = first.1.saturating_sub(have);
            }
            let pos = match added.iter().position(|(id, _)| *id == node.id()) {
                Some(p) => p,
                None => {
                    added.push((node.id(), Build::default()));
                    added.len() - 1
                }
            };
            let built = added.get_mut(pos).and_then(|(_, b)| b.at(&rest, &mut budget));
            match built {
                Some(b) => b.text = w.text.clone(),
                None => {
                    skipped(warnings, TOO_MANY, &w.som);
                    continue;
                }
            }
        }
        if dropped {
            skipped(warnings, CONTROL, &w.som);
        }
    }
    for (id, build) in added {
        let Some(node) = tree.get_node(id) else { continue };
        let mut inner = String::new();
        build.write_children(&mut inner);
        match append_edit(src, node, &inner) {
            Some(e) => edits.push(e),
            None => warnings.push("Some XFA data nodes could not be added; their field values were not written".into()),
        }
    }
}

/// What [`write_datasets`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DatasetsWrite {
    /// The datasets packet changed (a new revision of it was added to the document).
    pub written: bool,
    /// Values that could not be written, and why.
    pub warnings: Vec<String>,
    /// The datasets stream written, when one was.
    pub stream: Option<ObjRef>,
}

/// A packet stream's text and encoding; `None` (with a warning) when it can't be read exactly.
fn packet_text(doc: &Document, o: &Object, warnings: &mut Vec<String>) -> Option<(String, Encoding)> {
    let s = doc.resolve(o);
    let Object::Stream(s) = &*s else {
        warnings.push("The XFA data packet is not a stream; the field values were not written to it".into());
        return None;
    };
    let bytes = match s.decoded_within(64 << 20) {
        Ok(b) => b,
        Err(e) => {
            warnings.push(format!("The XFA data could not be decoded ({e}); the field values were not written to it"));
            return None;
        }
    };
    let decoded = crate::packets::decode(&bytes);
    if decoded.is_none() {
        warnings.push("The XFA data is neither UTF-8 nor UTF-16; the field values were not written to it, so it stays as it was".into());
    }
    decoded
}

/// Write the fields' values into the datasets packet, keeping all the data no field binds to.
/// Nothing is written when the document has no XFA entry or the data already holds the values;
/// values that can't be written are reported, never silently dropped.
pub fn write_datasets(doc: &mut Document, fields: &[FieldDatum]) -> Result<DatasetsWrite, XfaError> {
    write_datasets_reusing(doc, fields, None)
}

/// [`write_datasets`], replacing the stream `reuse` in place when the packet is that stream
/// (written earlier in the same edit) rather than adding another.
pub fn write_datasets_reusing(doc: &mut Document, fields: &[FieldDatum], reuse: Option<ObjRef>) -> Result<DatasetsWrite, XfaError> {
    write_datasets_with(doc, None, fields, reuse)
}

/// [`write_datasets_reusing`] with the form's parsed template when the caller has it (it says
/// which SOM segments are areas, see [`data_path`]); `None` parses the document's.
pub fn write_datasets_with(
    doc: &mut Document,
    tpl: Option<&Template>,
    fields: &[FieldDatum],
    reuse: Option<ObjRef>,
) -> Result<DatasetsWrite, XfaError> {
    let wants = with_template(doc, tpl, |t| wanted(doc, fields, t));
    rewrite(doc, &[Op::Values(&wants)], reuse)
}

/// One change to the data, for [`write_data_ops`].
#[derive(Clone, Debug, PartialEq)]
pub enum DataOp {
    /// Put `text` at the node `path`, adding the node (and the ones above it) as needed.
    Value { path: DataPath, text: String },
    /// Make sure the node at `parent` has at least `count` children named `name` (a script
    /// added rows: the layout repeats the subform once per data instance).
    Instances { parent: DataPath, name: String, count: usize },
    /// Take the node at `path` out (a script removed a row).
    Remove(DataPath),
}

impl DataNode {
    /// The node at `path`, made (with empty siblings before it) where missing; `None` past
    /// the index cap.
    fn ensure(&mut self, path: &[(String, usize)]) -> Option<&mut DataNode> {
        let mut node = self;
        for (name, idx) in path {
            if *idx >= MAX_INDEX {
                return None;
            }
            let have = node.children.iter().filter(|c| &c.name == name).count();
            for _ in have..=*idx {
                node.children.push(DataNode { name: name.clone(), ..Default::default() });
            }
            node = node.children.iter_mut().filter(|c| &c.name == name).nth(*idx)?;
        }
        Some(node)
    }

    /// Apply `op` to this tree (the root is the `xfa:data` element), as [`write_data_ops`]
    /// will to the packet, so scripts that run before it is written see the change.
    pub fn apply(&mut self, op: &DataOp) {
        match op {
            DataOp::Value { path, text } => {
                if let Some(n) = self.ensure(path)
                    && n.children.is_empty()
                {
                    n.text = text.clone();
                }
            }
            DataOp::Instances { parent, name, count } => {
                if let Some(&last) = count.checked_sub(1).as_ref() {
                    let mut path = parent.clone();
                    path.push((name.clone(), last.min(MAX_INDEX.saturating_sub(1))));
                    let _ = self.ensure(&path);
                }
            }
            DataOp::Remove(path) => {
                let Some(((name, idx), parent)) = path.split_last() else { return };
                let mut node = self;
                for (n, i) in parent {
                    let Some(next) = node.children.iter_mut().filter(|c| &c.name == n).nth(*i) else { return };
                    node = next;
                }
                if let Some(at) = node.children.iter().enumerate().filter(|(_, c)| &c.name == name).nth(*idx).map(|(at, _)| at) {
                    node.children.remove(at);
                }
            }
        }
    }
}

fn path_som(path: &DataPath) -> String {
    path.iter().map(|(n, i)| format!("{n}[{i}]")).collect::<Vec<_>>().join(".")
}

/// Make sure the data has at least `count` instances of `name` under `parent` (a script added
/// rows: the layout repeats the subform once per data instance).
pub fn add_data_instances(doc: &mut Document, parent: &DataPath, name: &str, count: usize) -> Result<DatasetsWrite, XfaError> {
    write_data_ops(doc, &[DataOp::Instances { parent: parent.clone(), name: name.to_string(), count }], None)
}

/// Put `text` at the data node `path` (a field with no widget, a hidden one or a draw, that a
/// script gave a value), adding the node as needed.
pub fn write_data_value(doc: &mut Document, path: &DataPath, text: &str) -> Result<DatasetsWrite, XfaError> {
    write_data_ops(doc, &[DataOp::Value { path: path.clone(), text: text.to_string() }], None)
}

/// Remove the data node at `path` (a script removed a row).
pub fn remove_data_instance(doc: &mut Document, path: &DataPath) -> Result<DatasetsWrite, XfaError> {
    write_data_ops(doc, &[DataOp::Remove(path.clone())], None)
}

/// Apply `ops` to the datasets packet, in order, as one rewrite of it: everything one script
/// event changed becomes a single new datasets stream. When the packet is already the stream
/// `reuse` (written earlier for the same event), that stream is replaced in place instead of
/// a new one being added. The stream written is in the report.
pub fn write_data_ops(doc: &mut Document, ops: &[DataOp], reuse: Option<ObjRef>) -> Result<DatasetsWrite, XfaError> {
    // Runs of values (and instances) merge in one pass; a value set twice keeps the last.
    let mut steps: Vec<Vec<Want>> = Vec::new();
    let mut removes: Vec<Option<DataPath>> = Vec::new();
    let mut run: Vec<Want> = Vec::new();
    let flush = |run: &mut Vec<Want>, steps: &mut Vec<Vec<Want>>, removes: &mut Vec<Option<DataPath>>| {
        if !run.is_empty() {
            let mut seen = std::collections::HashSet::new();
            let mut kept: Vec<Want> = std::mem::take(run).into_iter().rev().filter(|w| seen.insert((w.ensure, w.path.clone()))).collect();
            kept.reverse();
            steps.push(kept);
            removes.push(None);
        }
    };
    for op in ops {
        match op {
            DataOp::Value { path, text } if !path.is_empty() => {
                // An empty value clears an existing node and needs no new one.
                run.push(Want { som: path_som(path), path: path.clone(), text: text.clone(), ensure: false });
            }
            DataOp::Instances { parent, name, count } => {
                for k in 0..(*count).min(MAX_INDEX) {
                    let mut path = parent.clone();
                    path.push((name.clone(), k));
                    run.push(Want { som: format!("{name}[{k}]"), path, text: String::new(), ensure: true });
                }
            }
            DataOp::Remove(path) if !path.is_empty() => {
                flush(&mut run, &mut steps, &mut removes);
                steps.push(Vec::new());
                removes.push(Some(path.clone()));
            }
            _ => {}
        }
    }
    flush(&mut run, &mut steps, &mut removes);
    let plan: Vec<Op> = steps
        .iter()
        .zip(&removes)
        .map(|(w, r)| match r {
            Some(p) => Op::Remove(p),
            None => Op::Values(w),
        })
        .collect();
    rewrite(doc, &plan, reuse)
}

/// Put `bytes` in the document as a datasets stream: in place of `old` when that is the stream
/// `reuse`, else as a new object.
fn put_stream(doc: &mut Document, old: Option<&Object>, reuse: Option<ObjRef>, bytes: &[u8]) -> ObjRef {
    let stream = Object::Stream(Stream::flate(Dict::new(), bytes));
    match (old.and_then(Object::as_ref), reuse) {
        (Some(r), Some(keep)) if r == keep => {
            doc.set(r, stream);
            r
        }
        _ => doc.add(stream),
    }
}

/// Merge `ops` into `text` in order. `None`: nothing changed (or nothing could be merged).
fn merge_all(text: &str, within: Within, ops: &[Op], warnings: &mut Vec<String>) -> Option<String> {
    let mut current: Option<String> = None;
    for op in ops {
        match merge(current.as_deref().unwrap_or(text), within, *op, warnings) {
            Merged::Changed(t) => current = Some(t),
            Merged::Unchanged => {}
            // Later steps still apply to what merged so far; the reason is in the warnings.
            Merged::Failed => {}
        }
    }
    current
}

fn rewrite(doc: &mut Document, ops: &[Op], reuse: Option<ObjRef>) -> Result<DatasetsWrite, XfaError> {
    let mut report = DatasetsWrite::default();
    if ops.is_empty() {
        return Ok(report);
    }
    let Some(root) = doc.root() else { return Ok(report) };
    let catalog = doc.get(root);
    let Some(acro_obj) = catalog.as_dict().and_then(|c| c.get(b"AcroForm")).cloned() else { return Ok(report) };
    let acro_ref = acro_obj.as_ref();
    let Some(mut acro) = doc.resolve(&acro_obj).as_dict().cloned() else { return Ok(report) };
    let Some(xfa) = acro.get(b"XFA").cloned() else { return Ok(report) };
    let warnings = &mut report.warnings;
    match &*doc.resolve(&xfa) {
        Object::Array(items) => {
            let mut items = items.clone();
            let at = (0..items.len())
                .step_by(2)
                .find(|&i| items.get(i).and_then(|n| n.as_string()).is_some_and(|s| s.to_text() == "datasets") && i + 1 < items.len());
            match at {
                Some(i) => {
                    let Some(existing) = items.get(i + 1).cloned() else { return Ok(report) };
                    let Some((text, encoding)) = packet_text(doc, &existing, warnings) else { return Ok(report) };
                    let Some(t) = merge_all(&text, Within::Packet, ops, warnings) else { return Ok(report) };
                    let r = put_stream(doc, Some(&existing), reuse, &encode(&t, encoding));
                    report.stream = Some(r);
                    if let Some(slot) = items.get_mut(i + 1) {
                        *slot = Object::Ref(r);
                    }
                }
                None => {
                    // No datasets packet yet: made from the first values, then the rest merged
                    // in; before the postamble, or last.
                    let Some(first) = ops.iter().position(|op| matches!(op, Op::Values(_))) else { return Ok(report) };
                    let Some(Op::Values(wants)) = ops.get(first) else { return Ok(report) };
                    let fresh = fresh_datasets(&build_children(wants, warnings));
                    let text = merge_all(&fresh, Within::Packet, ops.get(first + 1..).unwrap_or_default(), warnings).unwrap_or(fresh);
                    let r = doc.add(Object::Stream(Stream::flate(Dict::new(), text.as_bytes())));
                    report.stream = Some(r);
                    let pos = items.iter().position(|n| n.as_string().is_some_and(|s| s.to_text() == "postamble")).unwrap_or(items.len());
                    items.insert(pos, Object::Ref(r));
                    items.insert(pos, Object::String(PdfString::text("datasets")));
                }
            }
            acro.set(b"XFA".to_vec(), Object::Array(items));
        }
        Object::Stream(_) => {
            let Some((text, encoding)) = packet_text(doc, &xfa, warnings) else { return Ok(report) };
            let Some(new) = merge_all(&text, Within::Xdp, ops, warnings) else { return Ok(report) };
            let r = put_stream(doc, Some(&xfa), reuse, &encode(&new, encoding));
            report.stream = Some(r);
            acro.set(b"XFA".to_vec(), Object::Ref(r));
        }
        _ => return Ok(report),
    }
    match acro_ref {
        Some(ar) => doc.set(ar, Object::Dict(acro)),
        None => doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(acro)))?,
    }
    report.written = true;
    Ok(report)
}

/// The values the datasets hold for `fields`: `(field name, what it should hold)`, only for
/// fields the data mentions.
pub fn read_values(doc: &Document, fields: &[FieldDatum]) -> Vec<(String, FieldData)> {
    read_values_with(doc, None, fields)
}

/// [`read_values`] with the form's parsed template when the caller has it; `None` parses the
/// document's.
pub fn read_values_with(doc: &Document, tpl: Option<&Template>, fields: &[FieldDatum]) -> Vec<(String, FieldData)> {
    with_template(doc, tpl, |t| values_at(doc, t, fields))
}

fn values_at(doc: &Document, tpl: Option<&Template>, fields: &[FieldDatum]) -> Vec<(String, FieldData)> {
    let Ok(Some(p)) = crate::read_packets(doc) else { return Vec::new() };
    let Some(data) = parse_datasets(&p.xdp) else { return Vec::new() };
    let mut out = Vec::new();
    for f in fields {
        let path = path_of(tpl, &som_of(doc, f));
        let Some(text) = data.text_at(&path) else { continue };
        let value = match &f.data {
            FieldData::Text(_) => FieldData::Text(match date_pattern_of(doc, f) {
                Some(pat) => iso_to_pattern(text, &pat),
                None => text.to_string(),
            }),
            FieldData::Check(_) => {
                let (on, _) = items_of(doc, f);
                FieldData::Check(is_on(text, &on))
            }
            FieldData::Radio(_) => FieldData::Radio((!text.trim().is_empty()).then(|| text.trim().to_string())),
            FieldData::None => continue,
        };
        out.push((f.name.clone(), value));
    }
    out
}

/// Does a data value mean "checked" for a button whose on value is `on`?
pub fn is_on(text: &str, on: &str) -> bool {
    let t = text.trim();
    if t == on {
        return true;
    }
    !matches!(t.to_ascii_lowercase().as_str(), "" | "0" | "off" | "false" | "no") && on.trim().is_empty()
}
