//! The form as scripts see it (XFA 3.3 part 6): the instance tree with values and presence, the
//! events a template declares, the presence overrides scripts make (kept in the PDF so they
//! survive undo and reopening), and laying the form out again after scripts changed it.

use std::collections::{BTreeMap, HashMap};

use pdfcraft_cos::{Dict, Document, Object, PdfString};

use crate::data::{DataNode, data_path, parse_datasets};
use crate::model::*;
use crate::{Report, XfaError};

/// Private key on the AcroForm: presence scripts set, by SOM path.
pub const OVERRIDES_KEY: &[u8] = b"PCXfaOverrides";

const MAX_DEPTH: usize = 64;
const MAX_EVENTS: usize = 20_000;
/// Most objects of the live form built (the scripting engine's snapshot holds as many):
/// nested repeating subforms multiply, so the walk stops here and says so.
pub const MAX_FORM_NODES: usize = 200_000;
const MAX_OVERRIDES: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Subform,
    Field,
    ExclGroup,
    Draw,
    Area,
}

/// One object of the live form.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormNode {
    pub name: String,
    pub som: String,
    pub kind: Option<NodeKind>,
    pub value: String,
    pub numeric: bool,
    pub presence: String,
    pub access: String,
    pub repeatable: bool,
    pub occur_min: usize,
    pub occur_max: Option<usize>,
    pub index: usize,
    pub children: Vec<FormNode>,
}

/// A script the template runs for an event of one node instance.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptEvent {
    pub som: String,
    /// `initialize`, `calculate`, `validate`, `click`, `change`, `exit`, …
    pub activity: String,
    pub script: String,
    pub formcalc: bool,
    /// Validate scripts: the message shown when they fail.
    pub message: Option<String>,
}

/// The live form as [`form_tree`] builds it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveForm {
    /// The root subform's instance tree.
    pub root: FormNode,
    /// Every scripted event, in document order.
    pub events: Vec<ScriptEvent>,
    /// The form was larger than [`MAX_FORM_NODES`] objects (or [`MAX_EVENTS`] events): what
    /// is past that was left out.
    pub truncated: bool,
}

/// What scripts changed about the template's objects, by SOM path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overrides {
    pub presence: BTreeMap<String, String>,
    pub access: BTreeMap<String, String>,
}

impl Overrides {
    pub fn is_empty(&self) -> bool {
        self.presence.is_empty() && self.access.is_empty()
    }
}

fn presence_name(p: Presence) -> &'static str {
    match p {
        Presence::Visible => "visible",
        Presence::Invisible => "invisible",
        Presence::Hidden => "hidden",
        Presence::Inactive => "inactive",
    }
}

fn access_name(a: Access) -> &'static str {
    match a {
        Access::Open => "open",
        Access::ReadOnly => "readOnly",
        Access::Protected => "protected",
        Access::NonInteractive => "nonInteractive",
    }
}

fn acroform(doc: &Document) -> Option<(Option<pdfcraft_cos::ObjRef>, Dict)> {
    let root = doc.root()?;
    let catalog = doc.get(root);
    let o = catalog.as_dict()?.get(b"AcroForm")?.clone();
    let d = doc.resolve(&o).as_dict().cloned()?;
    Some((o.as_ref(), d))
}

/// The overrides the document carries.
pub fn overrides(doc: &Document) -> Overrides {
    let mut out = Overrides::default();
    let Some((_, acro)) = acroform(doc) else { return out };
    let Some(o) = acro.get(OVERRIDES_KEY).map(|o| doc.resolve(o)) else { return out };
    let Some(d) = o.as_dict() else { return out };
    for (which, map) in [(&b"Presence"[..], &mut out.presence), (&b"Access"[..], &mut out.access)] {
        if let Some(m) = d.get(which).map(|m| doc.resolve(m))
            && let Some(m) = m.as_dict()
        {
            for (k, v) in m.iter().take(MAX_OVERRIDES) {
                if let Some(v) = v.as_string() {
                    map.insert(String::from_utf8_lossy(k).into_owned(), v.to_text());
                }
            }
        }
    }
    out
}

/// Store `ov` in the document (none: the key is removed).
pub fn set_overrides(doc: &mut Document, ov: &Overrides) -> Result<(), XfaError> {
    let Some(root) = doc.root() else { return Ok(()) };
    let Some((acro_ref, mut acro)) = acroform(doc) else { return Ok(()) };
    if ov.is_empty() {
        acro.remove(OVERRIDES_KEY);
    } else {
        let mut d = Dict::new();
        for (which, map) in [("Presence", &ov.presence), ("Access", &ov.access)] {
            if map.is_empty() {
                continue;
            }
            let mut m = Dict::new();
            for (k, v) in map.iter().take(MAX_OVERRIDES) {
                m.set(k.as_bytes().to_vec(), Object::String(PdfString::text(v)));
            }
            d.set(which.as_bytes().to_vec(), Object::Dict(m));
        }
        acro.set(OVERRIDES_KEY.to_vec(), Object::Dict(d));
    }
    match acro_ref {
        Some(r) => doc.set(r, Object::Dict(acro)),
        None => doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(acro)))?,
    }
    Ok(())
}

/// Index of child `i` among the earlier siblings with the same name.
fn sib_index(children: &[Node], i: usize) -> usize {
    let name = children.get(i).and_then(|c| c.common().name.as_deref());
    children.iter().take(i).filter(|c| c.common().name.as_deref() == name && name.is_some()).count()
}

fn child_som(parent: &str, name: Option<&str>, index: usize) -> String {
    match name {
        Some(n) if !parent.is_empty() => format!("{parent}.{n}[{index}]"),
        Some(n) => format!("{n}[{index}]"),
        None => parent.to_string(),
    }
}

struct Walker<'a> {
    /// For data paths (areas are not data scopes).
    tpl: &'a Template,
    data: Option<&'a DataNode>,
    ov: &'a Overrides,
    events: Vec<ScriptEvent>,
    /// Objects built so far.
    nodes: usize,
    truncated: bool,
}

impl Walker<'_> {
    /// Count one more object; false once the budget is spent.
    fn take(&mut self) -> bool {
        if self.nodes >= MAX_FORM_NODES {
            self.truncated = true;
            return false;
        }
        self.nodes += 1;
        true
    }

    fn push_events(&mut self, som: &str, scripts: &[Script], calculate: Option<&Script>, validate: Option<&Script>, message: Option<&str>) {
        if self.events.len() >= MAX_EVENTS {
            self.truncated = true;
            return;
        }
        for s in scripts {
            if s.text.trim().is_empty() {
                continue;
            }
            self.events.push(ScriptEvent {
                som: som.to_string(),
                activity: s.activity.clone(),
                script: s.text.clone(),
                formcalc: s.formcalc,
                message: None,
            });
        }
        if let Some(c) = calculate
            && !c.text.trim().is_empty()
        {
            self.events.push(ScriptEvent {
                som: som.to_string(),
                activity: "calculate".into(),
                script: c.text.clone(),
                formcalc: c.formcalc,
                message: None,
            });
        }
        if let Some(v) = validate
            && !v.text.trim().is_empty()
        {
            self.events.push(ScriptEvent {
                som: som.to_string(),
                activity: "validate".into(),
                script: v.text.clone(),
                formcalc: v.formcalc,
                message: message.map(str::to_string),
            });
        }
    }

    fn presence(&self, som: &str, p: Presence) -> String {
        self.ov.presence.get(som).cloned().unwrap_or_else(|| presence_name(p).to_string())
    }

    fn access(&self, som: &str, a: Access) -> String {
        self.ov.access.get(som).cloned().unwrap_or_else(|| access_name(a).to_string())
    }

    fn field(&mut self, f: &Field, parent_som: &str, index: usize, radio: Option<&str>) -> FormNode {
        let som = child_som(parent_som, f.common.name.as_deref().or(Some("field")), index);
        let data_value = self.data.and_then(|d| d.value_at(&data_path(self.tpl, &som))).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        let value = match (radio, data_value) {
            // A radio button's value is its on value when the group picked it.
            (Some(selected), _) => {
                let on = f.items.first().cloned().unwrap_or_default();
                if selected == on { on } else { String::new() }
            }
            (None, Some(v)) => f.saved_value(&v),
            (None, None) => f.value.plain().map(|v| f.saved_value(v.trim())).unwrap_or_default(),
        };
        self.push_events(&som, &f.scripts, f.calculate.as_ref(), f.validate.as_ref(), f.validate_message.as_deref());
        FormNode {
            name: f.common.name.clone().unwrap_or_else(|| "field".into()),
            som: som.clone(),
            kind: Some(NodeKind::Field),
            value,
            numeric: f.numeric,
            presence: self.presence(&som, f.common.presence),
            access: self.access(&som, f.access),
            repeatable: false,
            occur_min: 1,
            occur_max: Some(1),
            index,
            children: Vec::new(),
        }
    }

    fn children(&mut self, nodes: &[Node], parent_som: &str, depth: usize) -> Vec<FormNode> {
        let mut out = Vec::new();
        if depth > MAX_DEPTH {
            return out;
        }
        for (i, n) in nodes.iter().enumerate() {
            if !self.take() {
                break;
            }
            let sib = sib_index(nodes, i);
            match n {
                Node::Subform(sf) => {
                    let from_data = match (&sf.common.name, self.data) {
                        (Some(name), Some(d)) if sf.occur.max != Some(1) => d.count(&data_path(self.tpl, parent_som), name),
                        _ => 0,
                    };
                    let instances = crate::layout::instance_count(&sf.occur, from_data);
                    for k in 0..instances {
                        // The first instance was counted above.
                        if k > 0 && !self.take() {
                            break;
                        }
                        let index = sib + k;
                        let som = child_som(parent_som, sf.common.name.as_deref(), index);
                        self.push_events(&som, &sf.scripts, None, None, None);
                        let children = self.children(&sf.children, &som, depth + 1);
                        out.push(FormNode {
                            name: sf.common.name.clone().unwrap_or_else(|| "#subform".into()),
                            som: som.clone(),
                            kind: Some(NodeKind::Subform),
                            value: String::new(),
                            numeric: false,
                            presence: self.presence(&som, sf.common.presence),
                            access: "open".into(),
                            repeatable: sf.occur.max != Some(1),
                            occur_min: sf.occur.min,
                            occur_max: sf.occur.max,
                            index,
                            children,
                        });
                    }
                }
                Node::Area(a) => {
                    let som = child_som(parent_som, a.common.name.as_deref(), sib);
                    let children = self.children(&a.children, &som, depth + 1);
                    out.push(FormNode {
                        name: a.common.name.clone().unwrap_or_else(|| "#area".into()),
                        som: som.clone(),
                        kind: Some(NodeKind::Area),
                        presence: self.presence(&som, a.common.presence),
                        access: "open".into(),
                        occur_min: 1,
                        occur_max: Some(1),
                        index: sib,
                        children,
                        ..Default::default()
                    });
                }
                Node::Field(f) => out.push(self.field(f, parent_som, sib, None)),
                Node::Draw(d) => {
                    let som = child_som(parent_som, d.common.name.as_deref(), sib);
                    out.push(FormNode {
                        name: d.common.name.clone().unwrap_or_else(|| "draw".into()),
                        som: som.clone(),
                        kind: Some(NodeKind::Draw),
                        value: d.value.plain().unwrap_or_default(),
                        presence: self.presence(&som, d.common.presence),
                        access: "open".into(),
                        occur_min: 1,
                        occur_max: Some(1),
                        index: sib,
                        ..Default::default()
                    });
                }
                Node::ExclGroup(g) => {
                    let som = child_som(parent_som, g.common.name.as_deref().or(Some("group")), sib);
                    let selected =
                        self.data.and_then(|d| d.text_at(&data_path(self.tpl, &som))).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
                    self.push_events(&som, &g.scripts, None, None, None);
                    let fields: Vec<Node> = g.fields.iter().map(|f| Node::Field(Box::new(f.clone()))).collect();
                    let mut children = Vec::new();
                    for (fi, f) in g.fields.iter().enumerate() {
                        if !self.take() {
                            break;
                        }
                        children.push(self.field(f, &som, sib_index(&fields, fi), Some(selected.as_deref().unwrap_or(""))));
                    }
                    out.push(FormNode {
                        name: g.common.name.clone().unwrap_or_else(|| "group".into()),
                        som: som.clone(),
                        kind: Some(NodeKind::ExclGroup),
                        value: selected.unwrap_or_default(),
                        presence: self.presence(&som, g.common.presence),
                        access: "open".into(),
                        occur_min: 1,
                        occur_max: Some(1),
                        index: sib,
                        children,
                        ..Default::default()
                    });
                }
            }
        }
        out
    }
}

/// The live form: the root subform's instance tree (values from `data`, presence and access
/// from the template and `ov`) and every scripted event, in document order, within
/// [`MAX_FORM_NODES`] objects.
pub fn form_tree(tpl: &Template, data: Option<&DataNode>, ov: &Overrides) -> LiveForm {
    let mut w = Walker { tpl, data, ov, events: Vec::new(), nodes: 0, truncated: false };
    let root = Node::Subform(Box::new(tpl.root.clone()));
    let mut nodes = w.children(std::slice::from_ref(&root), "", 0);
    let mut root_node = nodes.pop().unwrap_or_default();
    // Page masters' fields (Print and Save buttons, page numbers) belong to the layout model
    // in XFA; they hang off the root here, under their page area's name, as the layouter
    // names them, so their click scripts can run.
    if let Some(ps) = &tpl.root.page_set {
        for (i, area) in ps.areas.iter().enumerate() {
            let som = area.name.clone().map(|n| format!("{n}[0]")).unwrap_or_else(|| format!("#pageArea[{i}]"));
            let children = w.children(&area.items, &som, 1);
            if children.is_empty() {
                continue;
            }
            root_node.children.push(FormNode {
                name: area.name.clone().unwrap_or_else(|| "#pageArea".into()),
                som,
                kind: Some(NodeKind::Area),
                presence: "visible".into(),
                access: "open".into(),
                occur_min: 1,
                occur_max: Some(1),
                index: 0,
                children,
                ..Default::default()
            });
        }
    }
    LiveForm { root: root_node, events: w.events, truncated: w.truncated }
}

/// Does the template declare any script (an event, calculation or validation)? Forms without
/// one need no live form and run nothing.
pub fn has_scripts(tpl: &Template) -> bool {
    fn any(s: &[Script]) -> bool {
        s.iter().any(|s| !s.text.trim().is_empty())
    }
    fn field(f: &Field) -> bool {
        any(&f.scripts) || [&f.calculate, &f.validate].into_iter().flatten().any(|s| !s.text.trim().is_empty())
    }
    fn walk(nodes: &[Node], depth: usize) -> bool {
        depth <= MAX_DEPTH
            && nodes.iter().any(|n| match n {
                Node::Subform(sf) => any(&sf.scripts) || walk(&sf.children, depth + 1),
                Node::Area(a) => walk(&a.children, depth + 1),
                Node::Field(f) => field(f),
                Node::ExclGroup(g) => any(&g.scripts) || g.fields.iter().any(field),
                Node::Draw(_) => false,
            })
    }
    any(&tpl.root.scripts) || walk(&tpl.root.children, 1) || tpl.root.page_set.as_ref().is_some_and(|ps| ps.areas.iter().any(|a| walk(&a.items, 1)))
}

/// The template with `ov` applied to its objects (instance 0 of each; scripts that hide one
/// instance of a repeating subform hide them all).
pub fn apply_overrides(tpl: &Template, ov: &Overrides) -> Template {
    fn walk(nodes: &mut [Node], parent_som: &str, ov: &Overrides, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let sibs: Vec<usize> = (0..nodes.len()).map(|i| sib_index(nodes, i)).collect();
        for (i, n) in nodes.iter_mut().enumerate() {
            let sib = sibs.get(i).copied().unwrap_or(0);
            let name_default = match n {
                Node::Field(_) => Some("field"),
                Node::ExclGroup(_) => Some("group"),
                _ => None,
            };
            let som = child_som(parent_som, n.common().name.as_deref().or(name_default), sib);
            let presence = ov.presence.get(&som).map(|p| Presence::parse(Some(p)));
            let access = ov.access.get(&som).cloned();
            match n {
                Node::Subform(s) => {
                    if let Some(p) = presence {
                        s.common.presence = p;
                    }
                    walk(&mut s.children, &som, ov, depth + 1);
                }
                Node::Area(a) => {
                    if let Some(p) = presence {
                        a.common.presence = p;
                    }
                    walk(&mut a.children, &som, ov, depth + 1);
                }
                Node::Field(f) => {
                    if let Some(p) = presence {
                        f.common.presence = p;
                    }
                    if let Some(a) = access {
                        f.access = match a.as_str() {
                            "readOnly" => Access::ReadOnly,
                            "protected" => Access::Protected,
                            "nonInteractive" => Access::NonInteractive,
                            _ => Access::Open,
                        };
                    }
                }
                Node::Draw(d) => {
                    if let Some(p) = presence {
                        d.common.presence = p;
                    }
                }
                Node::ExclGroup(g) => {
                    if let Some(p) = presence {
                        g.common.presence = p;
                    }
                    let fields: Vec<Node> = g.fields.iter().map(|f| Node::Field(Box::new(f.clone()))).collect();
                    for (fi, f) in g.fields.iter_mut().enumerate() {
                        let fsom = child_som(&som, f.common.name.as_deref().or(Some("field")), sib_index(&fields, fi));
                        if let Some(p) = ov.presence.get(&fsom) {
                            f.common.presence = Presence::parse(Some(p));
                        }
                    }
                }
            }
        }
    }
    let mut t = tpl.clone();
    if !ov.is_empty() {
        let mut root = vec![Node::Subform(Box::new(t.root.clone()))];
        walk(&mut root, "", ov, 0);
        if let Some(Node::Subform(r)) = root.pop() {
            t.root = *r;
        }
    }
    t
}

/// The generated fields of the AcroForm by SOM path: `som → (field name, is a radio group)`.
pub fn fields_by_som(doc: &Document) -> HashMap<String, String> {
    fn walk(doc: &Document, r: pdfcraft_cos::ObjRef, prefix: &str, out: &mut HashMap<String, String>, depth: usize) {
        if depth > MAX_DEPTH || out.len() > 100_000 {
            return;
        }
        let o = doc.get(r);
        let Some(d) = o.as_dict() else { return };
        let t = d.get(b"T").map(|t| doc.resolve(t)).and_then(|t| t.as_string().map(|s| s.to_text()));
        let name = match (&t, prefix.is_empty()) {
            (Some(t), true) => t.clone(),
            (Some(t), false) => format!("{prefix}.{t}"),
            (None, _) => prefix.to_string(),
        };
        if let Some(som) = d.get(crate::pdf::SOM_KEY).map(|s| doc.resolve(s)).and_then(|s| s.as_string().map(|s| s.to_text()))
            && t.is_some()
        {
            out.insert(som, name.clone());
        }
        if let Some(kids) = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()) {
            for k in kids.iter().filter_map(Object::as_ref) {
                walk(doc, k, &name, out, depth + 1);
            }
        }
    }
    let mut out = HashMap::new();
    let Some((_, acro)) = acroform(doc) else { return out };
    let fields = acro.get(b"Fields").map(|f| doc.resolve(f)).and_then(|f| f.as_array().cloned()).unwrap_or_default();
    for f in fields.iter().filter_map(Object::as_ref) {
        walk(doc, f, "", &mut out, 0);
    }
    out
}

/// Lay the form out again from its template, data and overrides, replacing the pages and the
/// generated fields (a script added a row or hid a subform).
pub fn rerender(doc: &mut Document, tpl: &Template) -> Result<Report, XfaError> {
    let packets = crate::read_packets(doc)?.ok_or(XfaError::NotXfa)?;
    let data = parse_datasets(&packets.xdp);
    let ov = overrides(doc);
    let laid = apply_overrides(tpl, &ov);
    let form = crate::layout(&laid, data.as_ref())?;
    if form.pages.is_empty() {
        return Err(XfaError::Malformed("the template laid out to no pages".into()));
    }
    // The AcroForm keeps only the fields PdfCraft did not generate.
    let Some(root) = doc.root() else { return Err(XfaError::NotXfa) };
    if let Some((acro_ref, mut acro)) = acroform(doc) {
        let fields = acro.get(b"Fields").map(|f| doc.resolve(f)).and_then(|f| f.as_array().cloned()).unwrap_or_default();
        let kept: Vec<Object> =
            fields.into_iter().filter(|f| f.as_ref().is_none_or(|r| doc.get(r).as_dict().is_none_or(|d| !d.contains(crate::pdf::SOM_KEY)))).collect();
        acro.set(b"Fields".to_vec(), Object::Array(kept));
        match acro_ref {
            Some(r) => doc.set(r, Object::Dict(acro)),
            None => doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(acro)))?,
        }
    }
    let written = crate::pdf::write_form(doc, &form)?;
    let mut warnings = form.warnings;
    warnings.extend(written.warnings);
    warnings.dedup();
    Ok(Report { pages: written.pages, fields: written.fields, template_bytes: packets.xdp.len(), warnings })
}

/// The parsed template of a document's XFA packets.
pub fn template_of(doc: &Document) -> Result<Template, XfaError> {
    let packets = crate::read_packets(doc)?.ok_or(XfaError::NotXfa)?;
    if packets.xdp.trim().is_empty() {
        return Err(XfaError::Malformed("the XFA packets hold no template".into()));
    }
    crate::parse(&packets.xdp).map(|(t, _)| t)
}

/// The data tree of a document's datasets packet.
pub fn data_of(doc: &Document) -> Option<DataNode> {
    crate::read_packets(doc).ok().flatten().and_then(|p| parse_datasets(&p.xdp))
}
