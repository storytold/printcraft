//! Remove Hidden Information and Sanitize Document (architecture §11.1, execution plan M8.4).
//!
//! [`scan`] counts what each category would remove; [`remove_hidden`] removes the chosen
//! categories; [`sanitize`] removes all of them. Every removal makes the next save a full
//! rewrite, so the earlier revision (which still holds the data) leaves the file.
//!
//! A pass first *plans*: it walks the document and lists the edits, without changing anything,
//! then applies exactly that list. The counts come from the plan, so what is reported is what was
//! removed. [`crate::apply_with`] runs the same passes as the last step of applying redactions
//! ([`Sanitize`]).

use std::collections::HashSet;

use pdfcraft_content::Matrix;
use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::interp::{Mode, Scope, process};
use crate::{RedactError, Report, annots_of, page_streams};

/// The categories of Acrobat's Remove Hidden Information panel that PdfCraft handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hidden {
    /// Document information (`/Info`) and XMP metadata streams.
    Metadata,
    /// Embedded files and file attachment annotations.
    Attachments,
    /// Comments and markup annotations (with their pop-ups).
    Comments,
    /// Interactive form fields (flattened: their appearance stays as page content).
    FormFields,
    /// Text drawn invisibly (render modes 3 and 7) or wholly off the page.
    HiddenText,
    /// Optional content (layers) that is off: its content, then the layers themselves.
    HiddenLayers,
    Bookmarks,
    /// Links, actions (open action, additional actions) and document JavaScript.
    LinksActionsScripts,
    /// Private data of other applications (`/PieceInfo`).
    PrivateData,
}

pub const HIDDEN: [Hidden; 9] = [
    Hidden::Metadata,
    Hidden::Attachments,
    Hidden::Comments,
    Hidden::FormFields,
    Hidden::HiddenText,
    Hidden::HiddenLayers,
    Hidden::Bookmarks,
    Hidden::LinksActionsScripts,
    Hidden::PrivateData,
];

impl Hidden {
    pub fn label(self) -> &'static str {
        match self {
            Hidden::Metadata => "Metadata",
            Hidden::Attachments => "File attachments",
            Hidden::Comments => "Comments and markups",
            Hidden::FormFields => "Form fields",
            Hidden::HiddenText => "Hidden text",
            Hidden::HiddenLayers => "Hidden layers",
            Hidden::Bookmarks => "Bookmarks",
            Hidden::LinksActionsScripts => "Links, actions and JavaScripts",
            Hidden::PrivateData => "Private data of other applications",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Hidden::Metadata => "metadata",
            Hidden::Attachments => "attachments",
            Hidden::Comments => "comments",
            Hidden::FormFields => "form-fields",
            Hidden::HiddenText => "hidden-text",
            Hidden::HiddenLayers => "hidden-layers",
            Hidden::Bookmarks => "bookmarks",
            Hidden::LinksActionsScripts => "links-actions-scripts",
            Hidden::PrivateData => "private-data",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        HIDDEN.into_iter().find(|h| h.id() == id)
    }
}

fn catalog(doc: &Document) -> Option<(ObjRef, Dict)> {
    let r = doc.root()?;
    Some((r, doc.get(r).as_dict()?.clone()))
}

fn sub(doc: &Document, d: &Dict, key: &[u8]) -> Option<Dict> {
    d.get(key).and_then(|o| doc.resolve(o).as_dict().cloned())
}

fn outline_len(doc: &Document, first: Option<&Object>, depth: usize) -> usize {
    let mut n = 0;
    let mut cur = first.cloned();
    let mut guard = 0;
    while let Some(o) = cur {
        guard += 1;
        if guard > 100_000 || depth > 64 {
            break;
        }
        let Some(d) = doc.resolve(&o).as_dict().cloned() else { break };
        n += 1 + outline_len(doc, d.get(b"First"), depth + 1);
        cur = d.get(b"Next").cloned();
    }
    n
}

fn is_comment(subtype: &[u8]) -> bool {
    !matches!(subtype, b"Link" | b"Widget" | b"Popup" | b"FileAttachment" | b"Redact")
}

/// The layers that are off in the default configuration.
fn hidden_layers(doc: &Document) -> Vec<ObjRef> {
    let Some((_, cat)) = catalog(doc) else { return Vec::new() };
    let Some(oc) = sub(doc, &cat, b"OCProperties") else { return Vec::new() };
    let Some(d) = sub(doc, &oc, b"D") else { return Vec::new() };
    let all: Vec<ObjRef> =
        oc.get(b"OCGs").and_then(|g| doc.resolve(g).as_array().map(|a| a.iter().filter_map(Object::as_ref).collect())).unwrap_or_default();
    let list = |k: &[u8]| -> Vec<ObjRef> {
        d.get(k).and_then(|g| doc.resolve(g).as_array().map(|a| a.iter().filter_map(Object::as_ref).collect())).unwrap_or_default()
    };
    let (on, off) = (list(b"ON"), list(b"OFF"));
    if d.name(b"BaseState") == Some(b"OFF") {
        all.into_iter().filter(|g| !on.contains(g)).collect()
    } else {
        off.into_iter().filter(|g| all.contains(g)).collect()
    }
}

/// Glyphs of hidden text, or content of hidden layers, on every page (`write` = remove them).
fn content_pass(doc: &mut Document, text: bool, layers: bool, write: bool) -> Result<usize, RedactError> {
    let off = if layers { hidden_layers(doc) } else { Vec::new() };
    if !text && off.is_empty() {
        return Ok(0);
    }
    let mut total = 0;
    // The page tree is read once; the pages' own dictionaries only change below, one page at a time.
    for (pi, page) in pdfcraft_model::pages(doc).iter().enumerate() {
        let Ok((list, data)) = page_streams(doc, &page.dict, pi) else {
            if write {
                return Err(RedactError::Unreadable(pi + 1));
            }
            continue;
        };
        let resources = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
        let mut report = Report::default();
        let mode = if write { Mode::Apply } else { Mode::Verify };
        let mut scope = Scope::new(&[], mode, &mut report);
        if text {
            let c = page.crop(doc);
            scope.hidden_text = Some([c[0].min(c[2]), c[1].min(c[3]), c[0].max(c[2]), c[1].max(c[3])]);
        }
        scope.hidden_layers = off.clone();
        let out = process(doc, &mut scope, &data, &resources, Matrix::IDENTITY);
        let blocks = scope.layer_blocks;
        total += out.residue + blocks + report.glyphs;
        if !write {
            continue;
        }
        let mut new_list = list.clone();
        let mut changed = false;
        for (i, new) in out.streams.into_iter().enumerate() {
            let (Some(bytes), Some(old), Some(slot)) = (new, list.get(i), new_list.get_mut(i)) else { continue };
            let mut dict = match &*doc.resolve(old) {
                Object::Stream(s) => s.dict.clone(),
                _ => Dict::new(),
            };
            dict.remove(b"Length");
            *slot = Object::Ref(doc.add(Object::Stream(pdfcraft_cos::Stream::flate(dict, &bytes))));
            changed = true;
        }
        if changed || !out.xobjects.is_empty() {
            let mut res = resources;
            if !out.xobjects.is_empty() {
                let mut xo = res.get(b"XObject").and_then(|x| doc.resolve(x).as_dict().cloned()).unwrap_or_default();
                for (n, r) in &out.xobjects {
                    xo.set(n.clone(), Object::Ref(*r));
                }
                res.set(b"XObject".to_vec(), Object::Dict(xo));
            }
            doc.update_dict(page.obj, |d| {
                d.set(b"Contents".to_vec(), Object::Array(new_list));
                d.set(b"Resources".to_vec(), Object::Dict(res));
            })?;
            // The forms that were rewritten leave the resources with their old content — and the
            // page-tree nodes above with the names the forms retired against inherited resources.
            let after = doc.get(page.obj).as_dict().cloned().unwrap_or_default();
            let (_, now) = page_streams(doc, &after, pi)?;
            let mut gone = crate::tags::retired_names(&data.join(&b'\n'), &now.join(&b'\n'));
            gone.extend(out.inherited_gone.iter().cloned());
            gone.sort_unstable();
            gone.dedup();
            crate::tags::retire_forms(doc, page.obj, &gone)?;
        }
    }
    Ok(total)
}

// ── What a pass removes ─────────────────────────────────────────────────────────────────────

/// How far a pass goes in a category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    Off,
    /// Only what could carry content the caller removed (dangerous actions, text of annotations
    /// on redacted pages, form values of removed widgets).
    Scrub,
    /// The whole category.
    Remove,
}

/// What happens to optional content (layers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layers {
    Off,
    /// Layers stay (and stay as hidden as they are); no usage-based rule can switch them on.
    Hold,
    /// Every layer is switched off.
    ForceOff,
    /// Hidden layers and their content are removed.
    Remove,
}

#[derive(Clone, Copy, Debug)]
struct Policy {
    metadata: bool,
    attachments: bool,
    private: bool,
    bookmarks: bool,
    actions: Level,
    comments: Level,
    forms: Level,
    text: bool,
    layers: Layers,
}

impl Policy {
    /// The categories the user picked, each removed whole.
    fn only(which: &[Hidden]) -> Self {
        let has = |h: Hidden| which.contains(&h);
        let level = |h: Hidden| if has(h) { Level::Remove } else { Level::Off };
        Policy {
            metadata: has(Hidden::Metadata),
            attachments: has(Hidden::Attachments),
            private: has(Hidden::PrivateData),
            bookmarks: has(Hidden::Bookmarks),
            actions: level(Hidden::LinksActionsScripts),
            comments: level(Hidden::Comments),
            forms: level(Hidden::FormFields),
            text: has(Hidden::HiddenText),
            layers: if has(Hidden::HiddenLayers) { Layers::Remove } else { Layers::Off },
        }
    }

    /// What applying redactions should clean around the removed content, without taking away
    /// what the reader still sees: hidden data, scripts and the text of what was redacted.
    fn recommended(layers: LayerPolicy) -> Self {
        Policy {
            metadata: true,
            attachments: true,
            private: true,
            bookmarks: true,
            actions: Level::Scrub,
            comments: Level::Scrub,
            forms: Level::Scrub,
            text: false,
            layers: match layers {
                LayerPolicy::Hold => Layers::Hold,
                LayerPolicy::ForceOff => Layers::ForceOff,
            },
        }
    }
}

/// What a sanitize pass inside [`crate::apply_with`] does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Sanitize {
    /// Nothing; a document with XFA form data is refused (redaction can't reach XFA).
    None,
    /// Hidden data, scripts and the text of what was redacted: metadata (document information,
    /// XMP, thumbnails), attachments, private data, bookmarks and named destinations, dangerous
    /// actions, annotation text on redacted pages, values of removed form fields, XFA data,
    /// multimedia annotations; layers are held hidden. What the reader sees stays.
    #[default]
    Recommended,
    /// These categories, each removed whole (as [`remove_hidden`] does).
    Only(Vec<Hidden>),
}

impl Sanitize {
    /// Every category.
    pub fn all() -> Self {
        Sanitize::Only(HIDDEN.to_vec())
    }

    /// Does this pass remove XFA form data?
    pub(crate) fn removes_xfa(&self) -> bool {
        match self {
            Sanitize::None => false,
            Sanitize::Recommended => true,
            Sanitize::Only(which) => which.contains(&Hidden::FormFields),
        }
    }
}

/// What a sanitize pass inside apply does with optional content (layers).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LayerPolicy {
    /// Keep the layers as they are, hidden ones hidden (the default configuration is written out
    /// explicitly and usage rules that could switch a layer on are dropped).
    #[default]
    Hold,
    /// Switch every layer off.
    ForceOff,
}

/// What the sanitize pass of one operation removed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    /// Items per category (categories with nothing are left out).
    pub counts: Vec<(Hidden, usize)>,
    /// The names of the layers that were held hidden or switched off.
    pub layers: Vec<String>,
}

/// The widgets and pages a redaction removed things from (for the scrubs that follow it).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Marked<'a> {
    /// 0-based pages that were redacted.
    pub pages: &'a [usize],
    /// Widgets removed because they lay under a mark.
    pub widgets: &'a [ObjRef],
}

// ── The plan: what to change, found without changing anything ───────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum Seg {
    Key(Vec<u8>),
    Idx(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Root {
    Trailer,
    Obj(ObjRef),
}

/// A dictionary: an object, or a dictionary held directly inside one (the path).
#[derive(Clone, Debug)]
struct Loc {
    root: Root,
    path: Vec<Seg>,
}

impl Loc {
    fn obj(r: ObjRef) -> Self {
        Loc { root: Root::Obj(r), path: Vec::new() }
    }

    /// The dictionary under `key` of the dictionary `d` that sits here (an indirect one is its
    /// own object).
    fn child(&self, d: &Dict, key: &[u8]) -> Option<Loc> {
        match d.get(key)? {
            Object::Ref(r) => Some(Loc::obj(*r)),
            Object::Dict(_) => {
                let mut path = self.path.clone();
                path.push(Seg::Key(key.to_vec()));
                Some(Loc { root: self.root, path })
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
enum Op {
    /// Set (or, with no value, remove) `key` of the dictionary at `at`.
    Edit { at: Loc, key: Vec<u8>, value: Option<Object> },
    /// Take these annotations out of a page's `/Annots`.
    Annots { page: ObjRef, drop: Vec<ObjRef> },
}

fn rm(at: &Loc, key: &[u8]) -> Op {
    Op::Edit { at: at.clone(), key: key.to_vec(), value: None }
}

#[derive(Debug)]
struct Item {
    cat: Hidden,
    n: usize,
    ops: Vec<Op>,
}

#[derive(Debug, Default)]
struct Plan {
    items: Vec<Item>,
    layers: Vec<String>,
    /// Containers nested too deeply to search were met: the plan is not complete.
    too_deep: bool,
}

impl Plan {
    fn add(&mut self, cat: Hidden, n: usize, ops: Vec<Op>) {
        if n > 0 || !ops.is_empty() {
            self.items.push(Item { cat, n, ops });
        }
    }

    fn count(&self, h: Hidden) -> usize {
        self.items.iter().filter(|i| i.cat == h).map(|i| i.n).sum()
    }
}

fn read_at(doc: &Document, at: &Loc) -> Option<Dict> {
    let mut cur: Object = match at.root {
        Root::Trailer => Object::Dict(doc.trailer().clone()),
        Root::Obj(r) => (*doc.get(r)).clone(),
    };
    for s in &at.path {
        cur = match (s, &cur) {
            (Seg::Key(k), Object::Dict(d)) => d.get(k)?.clone(),
            (Seg::Key(k), Object::Stream(st)) => st.dict.get(k)?.clone(),
            (Seg::Idx(i), Object::Array(a)) => a.get(*i)?.clone(),
            _ => return None,
        };
    }
    cur.as_dict().cloned()
}

fn nav<'a>(o: &'a mut Object, path: &[Seg]) -> Option<&'a mut Object> {
    let mut cur = o;
    for s in path {
        cur = match s {
            Seg::Key(k) => cur.as_dict_mut()?.get_mut(k)?,
            Seg::Idx(i) => match cur {
                Object::Array(a) => a.get_mut(*i)?,
                _ => return None,
            },
        };
    }
    Some(cur)
}

fn catalog_loc(doc: &Document) -> Option<(Loc, Dict)> {
    let (r, d) = catalog(doc)?;
    Some((Loc::obj(r), d))
}

/// Entries of a tree (`leaf` is `/Names` or `/Nums`; `/Kids` recursion), bounded against cycles
/// and against [`MAX_KEY_WORK`]: `None` when counting costs more than that (a tree whose kids are
/// shared references fans out under the depth cap), whereupon the caller fails the pass.
fn tree_len(doc: &Document, node: &Dict, leaf: &[u8], depth: usize, work: &mut usize) -> Option<usize> {
    if depth > 32 {
        return Some(0);
    }
    if !spend(work) {
        return None;
    }
    let leaves = node.get(leaf).and_then(|n| doc.resolve(n).as_array().map(|a| a.len() / 2)).unwrap_or(0);
    let kids = node.get(b"Kids").and_then(|k| doc.resolve(k).as_array().cloned()).unwrap_or_default();
    let mut n = leaves;
    for k in &kids {
        let Some(kid) = doc.resolve(k).as_dict().cloned() else { continue };
        n += tree_len(doc, &kid, leaf, depth + 1, work)?;
    }
    Some(n)
}

/// Is this action harmless to keep? Only internal navigation is (default-deny); a chained
/// action (`/Next`) must be harmless too.
fn benign_action(doc: &Document, action: &Object, depth: usize) -> bool {
    let a = doc.resolve(action);
    let Some(d) = a.as_dict() else { return false };
    if depth > 8 || d.name(b"S") != Some(b"GoTo") {
        return false;
    }
    match d.get(b"Next").map(|n| doc.resolve(n)) {
        None => true,
        Some(n) => match &*n {
            Object::Array(list) => list.iter().all(|x| benign_action(doc, x, depth + 1)),
            Object::Null => true,
            other => benign_action(doc, other, depth + 1),
        },
    }
}

/// The keys of `d` that hold an action not worth keeping: every `/AA`, and an `/A` that is not
/// plain navigation.
fn dangerous_actions(doc: &Document, d: &Dict) -> Vec<&'static [u8]> {
    let mut keys: Vec<&'static [u8]> = Vec::new();
    if d.contains(b"AA") {
        keys.push(b"AA");
    }
    if d.get(b"A").is_some_and(|a| !benign_action(doc, a, 0)) {
        keys.push(b"A");
    }
    keys
}

const MULTIMEDIA: [&[u8]; 5] = [b"RichMedia", b"Screen", b"Movie", b"Sound", b"3D"];

/// Keys that may sit in any dictionary, and the category that owns them.
const EVERYWHERE: [(&[u8], Hidden); 4] =
    [(b"Metadata", Hidden::Metadata), (b"Thumb", Hidden::PrivateData), (b"PieceInfo", Hidden::PrivateData), (b"AF", Hidden::Attachments)];

/// Directly nested containers are followed this deep; beyond it the pass fails.
const MAX_KEY_DEPTH: usize = 64;
/// Most containers one walk may look into over all its branches (like `limits::MAX_CMAP_WORK`, for
/// the document's structure): a fan-out of shared references would otherwise multiply the paths
/// under the depth cap into an unbounded run. `walk_keys` and `tree_len` both spend it; spent, the
/// pass fails like one past [`MAX_KEY_DEPTH`].
const MAX_KEY_WORK: usize = 1 << 20;

/// One step of a bounded walk: the budget runs out after [`MAX_KEY_WORK`] containers.
fn spend(work: &mut usize) -> bool {
    *work = work.saturating_sub(1);
    *work > 0
}

/// Collect the `wanted` keys below `o`; returns `false` when containers nest too deeply to look
/// into or the walk spends its budget (what may hang there can't be removed, so the caller must
/// not carry on).
fn walk_keys(o: &Object, at: &mut Loc, depth: usize, wanted: &[(&[u8], Hidden)], out: &mut Vec<(Hidden, Op)>, work: &mut usize) -> bool {
    if depth > MAX_KEY_DEPTH || !spend(work) {
        return false;
    }
    let mut complete = true;
    match o {
        Object::Dict(_) | Object::Stream(_) => {
            let Some(d) = o.as_dict() else { return true };
            for (k, v) in d.iter() {
                if let Some((_, cat)) = wanted.iter().find(|(w, _)| *w == k.as_slice()) {
                    out.push((*cat, rm(at, k)));
                    continue;
                }
                at.path.push(Seg::Key(k.clone()));
                complete &= walk_keys(v, at, depth + 1, wanted, out, work);
                at.path.pop();
            }
        }
        Object::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                // Elements of arrays are not dictionaries to edit by key; descend to the
                // dictionaries inside them.
                at.path.push(Seg::Idx(i));
                complete &= walk_keys(v, at, depth + 1, wanted, out, work);
                at.path.pop();
            }
        }
        _ => {}
    }
    complete
}

/// Drop the thumbnail and the private data (`/Thumb`, `/PieceInfo`) of the given pages (0-based
/// indices). A thumbnail is a picture of the page as it was, and private data may quote its
/// text, so redacting a page always removes both, whatever the sanitize options. Returns how many
/// entries were dropped; a page that doesn't exist is an error.
pub(crate) fn drop_page_leaks(doc: &mut Document, pages: &[usize]) -> Result<usize, RedactError> {
    let all = pdfcraft_model::pages(doc);
    let mut dropped = 0;
    for &pi in pages {
        let page = all.get(pi).ok_or(RedactError::Unreadable(pi.saturating_add(1)))?;
        let n = [&b"Thumb"[..], b"PieceInfo"].into_iter().filter(|k| page.dict.contains(k)).count();
        if n > 0 {
            doc.update_dict(page.obj, |d| {
                d.remove(b"Thumb");
                d.remove(b"PieceInfo");
            })?;
            dropped += n;
        }
    }
    Ok(dropped)
}

/// Make direct annotation dictionaries indirect, so every later step can name them.
pub(crate) fn promote_direct_annots(doc: &mut Document) -> Result<(), RedactError> {
    for p in pdfcraft_model::pages(doc) {
        let list = annots_of(doc, &p.dict);
        if !list.iter().any(|a| matches!(a, Object::Dict(_))) {
            continue;
        }
        let new: Vec<Object> = list
            .into_iter()
            .map(|a| match a {
                Object::Dict(d) => Object::Ref(doc.add(Object::Dict(d))),
                o => o,
            })
            .collect();
        doc.update_dict(p.obj, |d| d.set(b"Annots".to_vec(), Object::Array(new)))?;
    }
    Ok(())
}

/// The areas an annotation may draw on, in user space: its `/Rect`, and the extent of each
/// appearance stream (`/BBox` mapped by `/Matrix`) as written, whatever the rectangle says.
pub(crate) fn coverage(doc: &Document, annot: &Dict) -> Vec<[f64; 4]> {
    let mut out: Vec<[f64; 4]> = Vec::new();
    out.extend(crate::rect_of(doc, annot.get(b"Rect")));
    let Some(ap) = annot.get(b"AP").and_then(|a| doc.dict(a)) else { return out };
    let Some(n) = ap.get(b"N").map(|n| doc.resolve(n)) else { return out };
    let mut streams: Vec<Dict> = Vec::new();
    match &*n {
        Object::Stream(s) => streams.push(s.dict.clone()),
        // States (check boxes, radio buttons): the current one, or all when unknown.
        Object::Dict(states) => {
            let current = annot.name(b"AS");
            for (name, v) in states.iter() {
                if current.is_none_or(|c| c == name.as_slice())
                    && let Some(d) = doc.dict(v)
                {
                    streams.push(d);
                }
            }
        }
        _ => {}
    }
    for d in streams {
        let Some(b) = crate::rect_of(doc, d.get(b"BBox")) else { continue };
        let m = d
            .get(b"Matrix")
            .and_then(|m| doc.resolve(m).as_array().cloned())
            .and_then(|a| Matrix::from_operands(&a.iter().filter_map(|x| doc.resolve(x).as_f64().map(Object::Real)).collect::<Vec<_>>()))
            .unwrap_or(Matrix::IDENTITY);
        let [a, b2, c, dd, e, f] = m.0;
        let pts = [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])].map(|(x, y)| (a * x + c * y + e, b2 * x + dd * y + f));
        let xs = pts.map(|p| p.0);
        let ys = pts.map(|p| p.1);
        let r = [
            xs.iter().copied().fold(f64::MAX, f64::min),
            ys.iter().copied().fold(f64::MAX, f64::min),
            xs.iter().copied().fold(f64::MIN, f64::max),
            ys.iter().copied().fold(f64::MIN, f64::max),
        ];
        if r.iter().all(|v| v.is_finite()) {
            out.push(r);
        }
    }
    out
}

/// Add the replies (`/IRT`, also replies to replies) of the `removed` annotations to it;
/// returns how many annotations other than pop-ups that took in.
pub(crate) fn remove_replies(doc: &Document, annots: &[Object], removed: &mut Vec<ObjRef>) -> usize {
    let mut n = 0;
    loop {
        let mut grew = false;
        for a in annots {
            let Some(r) = a.as_ref() else { continue };
            if removed.contains(&r) {
                continue;
            }
            let obj = doc.get(r);
            let Some(d) = obj.as_dict() else { continue };
            if d.get(b"IRT").and_then(Object::as_ref).is_some_and(|t| removed.contains(&t)) {
                removed.push(r);
                grew = true;
                if d.name(b"Subtype") != Some(b"Popup") {
                    n += 1;
                }
            }
        }
        if !grew {
            return n;
        }
    }
}

const FIELD_VALUES: [&[u8]; 5] = [b"V", b"DV", b"TU", b"TM", b"RV"];

fn plan_annots(doc: &Document, pol: &Policy, ctx: &Marked, plan: &mut Plan) {
    let mut seen: HashSet<ObjRef> = HashSet::new();
    for (pi, p) in pdfcraft_model::pages(doc).iter().enumerate() {
        let annots = annots_of(doc, &p.dict);
        let mut drop: Vec<ObjRef> = Vec::new();
        for a in &annots {
            let Some(r) = a.as_ref() else { continue };
            let obj = doc.get(r);
            let Some(d) = obj.as_dict() else { continue };
            seen.insert(r);
            let s = d.name(b"Subtype").unwrap_or(b"");
            let (cat, doomed) = if s == b"FileAttachment" && pol.attachments {
                (Hidden::Attachments, true)
            } else if (pol.comments == Level::Remove && is_comment(s)) || (pol.comments != Level::Off && MULTIMEDIA.contains(&s)) {
                (Hidden::Comments, true)
            } else if pol.actions == Level::Remove && s == b"Link" {
                (Hidden::LinksActionsScripts, true)
            } else {
                (Hidden::Comments, false)
            };
            if doomed {
                drop.push(r);
                plan.add(cat, 1, Vec::new());
                continue;
            }
            let at = Loc::obj(r);
            if pol.actions != Level::Off {
                let keys = dangerous_actions(doc, d);
                if !keys.is_empty() {
                    plan.add(Hidden::LinksActionsScripts, 1, keys.into_iter().map(|k| rm(&at, k)).collect());
                }
            }
            // What the annotations on a redacted page say may repeat what was redacted.
            if pol.comments == Level::Scrub && ctx.pages.contains(&pi) && !matches!(s, b"Widget" | b"Redact") {
                let keys: Vec<&[u8]> = [&b"Contents"[..], b"RC"].into_iter().filter(|k| d.contains(k)).collect();
                if !keys.is_empty() {
                    plan.add(Hidden::Comments, 1, keys.into_iter().map(|k| rm(&at, k)).collect());
                }
            }
        }
        if drop.is_empty() {
            continue;
        }
        // Pop-ups and replies go with what they belong to.
        let replies = remove_replies(doc, &annots, &mut drop);
        plan.add(Hidden::Comments, replies, Vec::new());
        for a in &annots {
            let Some(r) = a.as_ref() else { continue };
            let popup_of_dropped = doc.get(r).as_dict().is_some_and(|d| {
                d.name(b"Subtype") == Some(b"Popup") && d.get(b"Parent").and_then(Object::as_ref).is_some_and(|x| drop.contains(&x))
            });
            if popup_of_dropped && !drop.contains(&r) {
                drop.push(r);
            }
        }
        plan.items.push(Item { cat: Hidden::Comments, n: 0, ops: vec![Op::Annots { page: p.obj, drop }] });
    }
    // Form fields that are not annotations of their own (the upper levels of a hierarchy).
    if pol.actions != Level::Off {
        for f in pdfcraft_forms::fields(doc) {
            if !seen.insert(f.obj) {
                continue;
            }
            let obj = doc.get(f.obj);
            let Some(d) = obj.as_dict() else { continue };
            let keys = dangerous_actions(doc, d);
            if !keys.is_empty() {
                plan.add(Hidden::LinksActionsScripts, 1, keys.into_iter().map(|k| rm(&Loc::obj(f.obj), k)).collect());
            }
        }
    }
}

fn plan_forms(doc: &Document, pol: &Policy, ctx: &Marked, plan: &mut Plan) {
    if pol.forms == Level::Off {
        return;
    }
    if let Some((cat_loc, cat)) = catalog_loc(doc) {
        if cat.contains(b"XFA") {
            plan.add(Hidden::FormFields, 1, vec![rm(&cat_loc, b"XFA")]);
        }
        if let Some(af) = cat_loc.child(&cat, b"AcroForm")
            && read_at(doc, &af).is_some_and(|d| d.contains(b"XFA"))
        {
            plan.add(Hidden::FormFields, 1, vec![rm(&af, b"XFA")]);
        }
    }
    if pol.forms != Level::Scrub {
        return;
    }
    // The values of a removed widget's field, and of the fields above it, which its siblings
    // would otherwise still carry.
    let mut done: HashSet<ObjRef> = HashSet::new();
    for &w in ctx.widgets {
        let mut cur = Some(w);
        for _ in 0..16 {
            let Some(r) = cur else { break };
            if !done.insert(r) {
                break;
            }
            let obj = doc.get(r);
            let Some(d) = obj.as_dict() else { break };
            let keys: Vec<&[u8]> = FIELD_VALUES.into_iter().filter(|k| d.contains(k)).collect();
            if !keys.is_empty() {
                let at = Loc::obj(r);
                plan.add(Hidden::FormFields, 1, keys.into_iter().map(|k| rm(&at, k)).collect());
            }
            cur = d.get(b"Parent").and_then(Object::as_ref);
        }
    }
}

fn plan_layers(doc: &Document, pol: &Policy, plan: &mut Plan) {
    if pol.layers == Layers::Off {
        return;
    }
    let Some((cat_loc, cat)) = catalog_loc(doc) else { return };
    let Some(oc) = sub(doc, &cat, b"OCProperties") else { return };
    let off = hidden_layers(doc);
    let all: Vec<ObjRef> =
        oc.get(b"OCGs").and_then(|g| doc.resolve(g).as_array().map(|a| a.iter().filter_map(Object::as_ref).collect())).unwrap_or_default();
    let list = |d: &Dict, k: &[u8]| -> Vec<Object> { d.get(k).and_then(|g| doc.resolve(g).as_array().cloned()).unwrap_or_default() };
    let name_of = |r: ObjRef| -> String {
        doc.get(r).as_dict().and_then(|d| d.get(b"Name").and_then(|n| doc.resolve(n).as_string().map(|s| s.to_text()))).unwrap_or_default()
    };
    let mut oc = oc;
    let mut d = sub(doc, &oc, b"D").unwrap_or_default();
    match pol.layers {
        Layers::Remove => {
            if off.is_empty() {
                return;
            }
            // The off layers leave the configuration (their content is gone already).
            let keep = |o: &Object| !o.as_ref().is_some_and(|r| off.contains(&r));
            oc.set(b"OCGs".to_vec(), Object::Array(list(&oc, b"OCGs").into_iter().filter(keep).collect()));
            for k in [&b"ON"[..], b"OFF", b"Order", b"Locked"] {
                if d.contains(k) {
                    d.set(k.to_vec(), Object::Array(list(&d, k).into_iter().filter(keep).collect()));
                }
            }
        }
        Layers::Hold | Layers::ForceOff => {
            let held: Vec<ObjRef> = if pol.layers == Layers::Hold { off } else { all.clone() };
            if held.is_empty() {
                return;
            }
            plan.layers.extend(held.iter().map(|r| name_of(*r)));
            let on: Vec<Object> = if pol.layers == Layers::Hold {
                list(&d, b"ON").into_iter().filter(|o| !o.as_ref().is_some_and(|r| held.contains(&r))).collect()
            } else {
                Vec::new()
            };
            d.set(b"ON".to_vec(), Object::Array(on));
            d.set(b"OFF".to_vec(), Object::Array(held.iter().map(|r| Object::Ref(*r)).collect()));
            if pol.layers == Layers::ForceOff {
                d.set(b"BaseState".to_vec(), Object::name("OFF"));
            }
            // Usage rules (view, print, export) could switch a layer on again.
            d.remove(b"AS");
        }
        Layers::Off => return,
    }
    oc.set(b"D".to_vec(), Object::Dict(d));
    let n = if pol.layers == Layers::Remove { 0 } else { plan.layers.len() };
    plan.add(Hidden::HiddenLayers, n, vec![Op::Edit { at: cat_loc, key: b"OCProperties".to_vec(), value: Some(Object::Dict(oc)) }]);
}

/// Everything `pol` removes from the document's objects, found without changing anything.
fn plan(doc: &Document, pol: &Policy, ctx: &Marked) -> Plan {
    let mut plan = Plan::default();
    let pages = pdfcraft_model::pages(doc);

    if pol.metadata
        && let Some(info) = doc.trailer().get(b"Info")
    {
        let trailer = Loc { root: Root::Trailer, path: Vec::new() };
        let mut ops = Vec::new();
        let mut n = 0;
        // Emptied in place, so nothing that still points at it keeps the entries.
        if let Object::Ref(r) = info {
            let obj = doc.get(*r);
            if let Some(d) = obj.as_dict() {
                n = d.len();
                ops.extend(d.iter().map(|(k, _)| rm(&Loc::obj(*r), k)));
            }
        } else if let Object::Dict(d) = info {
            n = d.len();
        }
        ops.push(rm(&trailer, b"Info"));
        plan.add(Hidden::Metadata, n, ops);
    }

    // Metadata streams, thumbnails, private data and associated files, wherever they hang.
    let wanted: Vec<(&[u8], Hidden)> = EVERYWHERE
        .into_iter()
        .filter(|(_, c)| match c {
            Hidden::Metadata => pol.metadata,
            Hidden::PrivateData => pol.private,
            _ => pol.attachments,
        })
        .collect();
    if !wanted.is_empty() {
        for num in doc.object_numbers() {
            let r = ObjRef::new(num, doc.generation(num));
            let mut found = Vec::new();
            let mut work = MAX_KEY_WORK;
            plan.too_deep |= !walk_keys(&doc.get(r), &mut Loc::obj(r), 0, &wanted, &mut found, &mut work);
            for (cat, op) in found {
                plan.add(cat, 1, vec![op]);
            }
        }
    }

    if let Some((cat_loc, cat)) = catalog_loc(doc) {
        let names_loc = cat_loc.child(&cat, b"Names");
        let names = names_loc.as_ref().and_then(|l| read_at(doc, l)).unwrap_or_default();
        // Entries of a name tree, or `too_deep` when the tree costs more than a walk may spend.
        let name_tree = |k: &[u8], plan: &mut Plan| -> usize {
            let n = sub(doc, &names, k).and_then(|t| {
                let mut work = MAX_KEY_WORK;
                tree_len(doc, &t, b"Names", 0, &mut work)
            });
            plan.too_deep |= n.is_none();
            n.unwrap_or(0)
        };
        if pol.attachments
            && names.contains(b"EmbeddedFiles")
            && let Some(l) = &names_loc
        {
            let n = name_tree(b"EmbeddedFiles", &mut plan);
            plan.add(Hidden::Attachments, n, vec![rm(l, b"EmbeddedFiles")]);
        }
        if pol.private && cat.contains(b"Collection") {
            plan.add(Hidden::PrivateData, 1, vec![rm(&cat_loc, b"Collection")]);
        }
        if pol.bookmarks {
            if let Some(o) = sub(doc, &cat, b"Outlines") {
                let mut ops = vec![rm(&cat_loc, b"Outlines")];
                if cat.name(b"PageMode") == Some(b"UseOutlines") {
                    ops.push(rm(&cat_loc, b"PageMode"));
                }
                plan.add(Hidden::Bookmarks, outline_len(doc, o.get(b"First"), 0), ops);
            }
            // Named destinations and page labels carry names and text of their own, and threads
            // carry titles.
            if let Some(d) = sub(doc, &cat, b"Dests") {
                plan.add(Hidden::Bookmarks, d.len(), vec![rm(&cat_loc, b"Dests")]);
            }
            if names.contains(b"Dests")
                && let Some(l) = &names_loc
            {
                let n = name_tree(b"Dests", &mut plan);
                plan.add(Hidden::Bookmarks, n, vec![rm(l, b"Dests")]);
            }
            if let Some(t) = sub(doc, &cat, b"PageLabels") {
                let mut work = MAX_KEY_WORK;
                let n = tree_len(doc, &t, b"Nums", 0, &mut work);
                plan.too_deep |= n.is_none();
                plan.add(Hidden::Bookmarks, n.unwrap_or(0), vec![rm(&cat_loc, b"PageLabels")]);
            }
            if let Some(t) = cat.get(b"Threads").map(|t| doc.resolve(t)).and_then(|t| t.as_array().map(Vec::len)) {
                plan.add(Hidden::Bookmarks, t, vec![rm(&cat_loc, b"Threads")]);
            }
        }
        if pol.actions != Level::Off {
            if let Some(a) = cat.get(b"OpenAction").filter(|a| doc.resolve(a).as_dict().is_some())
                && (pol.actions == Level::Remove || !benign_action(doc, a, 0))
            {
                plan.add(Hidden::LinksActionsScripts, 1, vec![rm(&cat_loc, b"OpenAction")]);
            }
            if cat.contains(b"AA") {
                plan.add(Hidden::LinksActionsScripts, 1, vec![rm(&cat_loc, b"AA")]);
            }
            if names.contains(b"JavaScript")
                && let Some(l) = &names_loc
            {
                let n = name_tree(b"JavaScript", &mut plan);
                plan.add(Hidden::LinksActionsScripts, n, vec![rm(l, b"JavaScript")]);
            }
            for p in &pages {
                let ops: Vec<Op> = [&b"AA"[..], b"A"].into_iter().filter(|k| p.dict.contains(k)).map(|k| rm(&Loc::obj(p.obj), k)).collect();
                plan.add(Hidden::LinksActionsScripts, usize::from(!ops.is_empty()), ops);
            }
        }
    }
    plan_annots(doc, pol, ctx, &mut plan);
    plan_forms(doc, pol, ctx, &mut plan);
    plan_layers(doc, pol, &mut plan);
    plan
}

fn apply_ops(doc: &mut Document, plan: &Plan) -> Result<(), RedactError> {
    for op in plan.items.iter().flat_map(|i| i.ops.iter()) {
        match op {
            Op::Edit { at, key, value } => match at.root {
                Root::Trailer => {
                    let t = doc.trailer_mut();
                    match value {
                        Some(v) => t.set(key.clone(), v.clone()),
                        None => {
                            t.remove(key);
                        }
                    }
                }
                Root::Obj(r) => {
                    let mut obj = (*doc.get(r)).clone();
                    // Objects an earlier step removed are not there to edit.
                    let Some(d) = nav(&mut obj, &at.path).and_then(Object::as_dict_mut) else { continue };
                    match value {
                        Some(v) => d.set(key.clone(), v.clone()),
                        None => {
                            d.remove(key);
                        }
                    }
                    doc.set(r, obj);
                }
            },
            Op::Annots { page, drop } => {
                let Some(d) = doc.get(*page).as_dict().cloned() else { continue };
                let list = annots_of(doc, &d);
                let kept: Vec<Object> = list.iter().filter(|a| !a.as_ref().is_some_and(|r| drop.contains(&r))).cloned().collect();
                if kept.len() != list.len() {
                    doc.update_dict(*page, |d| {
                        if kept.is_empty() {
                            d.remove(b"Annots");
                        } else {
                            d.set(b"Annots".to_vec(), Object::Array(kept));
                        }
                    })?;
                }
            }
        }
    }
    Ok(())
}

/// Items counted by walking page content or the form rather than by the plan.
fn content_counts(doc: &Document, pol: &Policy) -> Result<Vec<(Hidden, usize)>, RedactError> {
    let mut doc2 = doc.clone();
    let mut out = Vec::new();
    if pol.text {
        out.push((Hidden::HiddenText, content_pass(&mut doc2, true, false, false)?));
    }
    if pol.layers == Layers::Remove {
        let off = hidden_layers(doc).len();
        out.push((Hidden::HiddenLayers, if off == 0 { 0 } else { off + content_pass(&mut doc2, false, true, false)? }));
    }
    if pol.forms == Level::Remove {
        out.push((Hidden::FormFields, pdfcraft_forms::fields(doc).len()));
    }
    Ok(out)
}

fn tally(plan: &Plan, content: &[(Hidden, usize)]) -> Vec<(Hidden, usize)> {
    HIDDEN.into_iter().map(|h| (h, plan.count(h) + content.iter().filter(|c| c.0 == h).map(|c| c.1).sum::<usize>())).filter(|(_, n)| *n > 0).collect()
}

/// Run a pass: plan it, do the content steps, apply the plan. The counts are the plan's, so what
/// is reported is what was removed.
fn execute(doc: &mut Document, pol: &Policy, ctx: &Marked) -> Result<Outcome, RedactError> {
    promote_direct_annots(doc)?;
    let plan = plan(doc, pol, ctx);
    if plan.too_deep {
        return Err(RedactError::StructureTooLarge);
    }
    let content = content_counts(doc, pol)?;
    let count = |h: Hidden| content.iter().find(|c| c.0 == h).map_or(0, |c| c.1);
    // Content first (it needs the layers and the form still in place).
    if (pol.text && count(Hidden::HiddenText) > 0) || (pol.layers == Layers::Remove && count(Hidden::HiddenLayers) > 0) {
        content_pass(doc, pol.text, pol.layers == Layers::Remove, true)?;
    }
    if pol.forms == Level::Remove && count(Hidden::FormFields) > 0 {
        // Fields without appearances get one first, so their values stay visible.
        for f in pdfcraft_forms::fields(doc) {
            if f.widgets.iter().any(|w| !doc.get(w.obj).as_dict().is_some_and(|d| d.contains(b"AP"))) {
                pdfcraft_forms::redraw_field(doc, &f.name)?;
            }
        }
        let n = pdfcraft_model::pages(doc).len();
        pdfcraft_edit::flatten(doc, &(0..n).collect::<Vec<_>>(), false, true)?;
        if let Some(root) = doc.root() {
            doc.update_dict(root, |d| {
                d.remove(b"AcroForm");
                d.remove(b"XFA");
            })?;
        }
    }
    apply_ops(doc, &plan)?;
    let counts = tally(&plan, &content);
    if !counts.is_empty() || !plan.layers.is_empty() {
        doc.require_full_save_with_new_id();
    }
    Ok(Outcome { counts, layers: plan.layers })
}

/// Sanitize inside an operation that rewrites the file anyway.
pub(crate) fn run(doc: &mut Document, mode: &Sanitize, layers: LayerPolicy, ctx: &Marked) -> Result<Outcome, RedactError> {
    let pol = match mode {
        Sanitize::None => return Ok(Outcome::default()),
        Sanitize::Recommended => Policy::recommended(layers),
        Sanitize::Only(which) => Policy::only(which),
    };
    execute(doc, &pol, ctx)
}

/// How many items each category would remove (categories with nothing are included as 0).
pub fn scan(doc: &Document) -> Vec<(Hidden, usize)> {
    let mut work = doc.clone();
    // Direct annotations are looked at the way a removal would see them.
    if promote_direct_annots(&mut work).is_err() {
        return HIDDEN.into_iter().map(|h| (h, 0)).collect();
    }
    let pol = Policy::only(&HIDDEN);
    let Ok(content) = content_counts(&work, &pol) else { return HIDDEN.into_iter().map(|h| (h, 0)).collect() };
    let found = tally(&plan(&work, &pol, &Marked::default()), &content);
    HIDDEN.into_iter().map(|h| (h, found.iter().find(|f| f.0 == h).map_or(0, |f| f.1))).collect()
}

/// Remove the chosen categories. Returns what was removed per category.
pub fn remove_hidden(doc: &mut Document, which: &[Hidden]) -> Result<Vec<(Hidden, usize)>, RedactError> {
    catalog(doc).ok_or(RedactError::NothingToApply)?;
    let out = execute(doc, &Policy::only(which), &Marked::default())?;
    if out.counts.is_empty() {
        return Err(RedactError::NothingToApply);
    }
    Ok(out.counts)
}

/// Sanitize Document: remove every category of hidden information. A document with nothing to
/// remove is left as it is.
pub fn sanitize(doc: &mut Document) -> Result<Vec<(Hidden, usize)>, RedactError> {
    match remove_hidden(doc, &HIDDEN) {
        Err(RedactError::NothingToApply) => Ok(Vec::new()),
        r => r,
    }
}
