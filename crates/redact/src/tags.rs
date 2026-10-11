//! Redaction and the structure tree: tags must not keep what was removed from the page.
//!
//! Marked content (MCIDs) that applying redactions changed is found by comparing each MCID's
//! operators before and after. Elements owning such content lose their alternate text, actual
//! text and expansion (which may spell out the removed words); references to marked content that
//! is now empty are dropped from the element. The same goes for the ancestors of such an element
//! (their alternate text stands for everything below), for marked content kept in a form XObject
//! that was rewritten (`/Stm`), and for object references (`/OBJR`) to removed annotations or
//! forms. The parent tree forgets what no longer exists.
//!
//! Retirement owns names where they live. A name whose `Do` disappeared — from a page's content,
//! or from a rewritten form, whether that form drew with its own /Resources or with the resources
//! it inherited — leaves the page's own /Resources and every page-tree node above whose own
//! /Resources still maps it: `/Resources` is an inheritable entry (ISO 32000-2, 7.8.3, Table 30),
//! so an inherited entry is owned by the ancestor, and as long as the node maps the name the
//! original object stays reachable and a full save keeps it. An entry only goes when its owner's
//! render no longer draws the name: for a node, when no page still draws it (in its own content,
//! or inside a form the page draws, each followed with the resources that form draws with), so a
//! sibling that still draws it keeps the entry and the object with it; for the page's own
//! resources, when that page's render no longer draws it, so a name a rewritten form retired does
//! not strip an entry the page still draws by another route. When a page or such a form can't be
//! read in full, nothing is retired at that owner: it could be drawing any name. Reads use the
//! same model: a page-own /Resources shadows an inherited one whole (per entry, not per name),
//! which `pdfcraft_model::pages` resolves into every page's dictionary.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::RedactError;

/// Structure elements nested this deep or deeper are not searched for removed content: they and
/// everything below them lose their alternate text unconditionally (their children are still
/// visited, so no subtree keeps any).
const MAX_DEPTH: usize = 64;
/// A parent tree deeper than `MAX_DEPTH`, a structure tree with more than `MAX_ELEMS` elements
/// and more than `MAX_FORMS` forms below a rewritten one can't be cleaned completely: the
/// operation fails rather than clean part of it.
const MAX_ELEMS: usize = 2_000_000;
/// Form XObjects followed below a rewritten form (however deep they nest: the walk is iterative
/// and each form is visited once).
const MAX_FORMS: usize = 100_000;

/// Each MCID's operators (serialized), and whether anything is still drawn inside it.
fn by_mcid(data: &[u8]) -> HashMap<i64, (Vec<u8>, bool)> {
    let mut out: HashMap<i64, (Vec<u8>, bool)> = HashMap::new();
    let mut stack: Vec<Option<i64>> = Vec::new();
    for op in pdfcraft_content::parse(data).ops {
        match op.op.as_slice() {
            b"BDC" => {
                let id = match op.operands.get(1) {
                    Some(Object::Dict(d)) => d.get(b"MCID").and_then(Object::as_int),
                    _ => None,
                };
                if let Some(id) = id {
                    out.entry(id).or_default();
                }
                stack.push(id);
                continue;
            }
            b"BMC" => {
                stack.push(None);
                continue;
            }
            b"EMC" => {
                stack.pop();
                continue;
            }
            _ => {}
        }
        let draws = match op.op.as_slice() {
            b"Tj" | b"'" | b"\"" => op.operands.last().and_then(Object::as_string).is_some_and(|s| !s.bytes.is_empty()),
            b"TJ" => {
                op.operands.first().and_then(Object::as_array).is_some_and(|a| a.iter().any(|x| x.as_string().is_some_and(|s| !s.bytes.is_empty())))
            }
            b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" | b"sh" | b"Do" | b"BI" => true,
            _ => false,
        };
        let mut bytes = Vec::new();
        pdfcraft_content::write_op(&op, &mut bytes);
        for id in stack.iter().flatten() {
            let e = out.entry(*id).or_default();
            e.0.extend_from_slice(&bytes);
            e.1 |= draws;
        }
    }
    out
}

/// The MCIDs whose content changed between `before` and `after`, and those left empty.
pub(crate) fn touched(before: &[u8], after: &[u8]) -> (HashSet<i64>, HashSet<i64>) {
    let (b, a) = (by_mcid(before), by_mcid(after));
    let mut changed = HashSet::new();
    let mut empty = HashSet::new();
    for (id, (ops, _)) in &b {
        match a.get(id) {
            Some((new, draws)) => {
                if new != ops {
                    changed.insert(*id);
                }
                if !draws {
                    empty.insert(*id);
                }
            }
            None => {
                changed.insert(*id);
                empty.insert(*id);
            }
        }
    }
    (changed, empty)
}

/// A child of a structure element.
enum Kid {
    /// Marked content: in the page's content (no `/Stm`) or in a form XObject (`/Stm`).
    Content {
        page: Option<ObjRef>,
        id: i64,
        stm: Option<ObjRef>,
    },
    /// An object reference: an annotation or XObject.
    Object {
        obj: Option<ObjRef>,
    },
    Elem,
}

fn classify(doc: &Document, kid: &Object, page: Option<ObjRef>) -> Kid {
    let o = doc.resolve(kid);
    match &*o {
        Object::Int(id) => Kid::Content { page, id: *id, stm: None },
        Object::Dict(m) if m.name(b"Type") == Some(b"MCR") => Kid::Content {
            page: m.get(b"Pg").and_then(Object::as_ref).or(page),
            id: m.get(b"MCID").and_then(Object::as_int).unwrap_or(-1),
            stm: m.get(b"Stm").and_then(Object::as_ref),
        },
        Object::Dict(m) if m.name(b"Type") == Some(b"OBJR") => Kid::Object { obj: m.get(b"Obj").and_then(Object::as_ref) },
        _ => Kid::Elem,
    }
}

/// What to clean the structure tree of.
#[derive(Default)]
struct Spec<'a> {
    /// The redacted page, and its marked content that changed or is empty now.
    page: Option<ObjRef>,
    changed: HashSet<i64>,
    empty: HashSet<i64>,
    /// Form XObjects that were rewritten (marked content kept in them is gone or changed).
    forms: HashSet<ObjRef>,
    /// Removed annotations.
    objs: &'a [ObjRef],
    /// Parent tree entries to drop.
    tree_keys: HashSet<i64>,
}

impl Spec<'_> {
    /// Does this kid hold removed content, and is it gone from the page altogether?
    fn judge(&self, k: &Kid) -> (bool, bool) {
        match k {
            Kid::Content { page, stm: Some(s), .. } => {
                let hit = self.forms.contains(s) && page.is_none_or(|p| Some(p) == self.page);
                (hit, hit)
            }
            Kid::Content { page, id, stm: None } => {
                let on_page = page.is_some() && *page == self.page;
                (on_page && self.changed.contains(id), on_page && self.empty.contains(id))
            }
            Kid::Object { obj: Some(o) } => {
                let hit = self.forms.contains(o) || self.objs.contains(o);
                (hit, hit)
            }
            _ => (false, false),
        }
    }
}

fn kids_of(doc: &Document, d: &Dict) -> Vec<Object> {
    match d.get(b"K") {
        None => Vec::new(),
        Some(k) => match &*doc.resolve(k) {
            Object::Array(a) => a.clone(),
            Object::Null => Vec::new(),
            _ => vec![k.clone()],
        },
    }
}

/// What a replaced parent tree value becomes.
enum Fix {
    Keep,
    Drop,
    /// The array the value is (or points to) gets these entries.
    Array(Vec<Object>),
}

/// Rewrite the `/Nums` of a number tree; returns the node when it is a direct dictionary that
/// changed (an indirect one is written back).
fn tree_fix(
    doc: &mut Document,
    node: &Object,
    depth: usize,
    f: &mut dyn FnMut(&Document, i64, &Object) -> Fix,
) -> Result<Option<Object>, RedactError> {
    if depth > MAX_DEPTH {
        return Err(RedactError::StructureTooLarge);
    }
    let (r, mut d) = match node {
        Object::Ref(r) => match doc.get(*r).as_dict() {
            Some(d) => (Some(*r), d.clone()),
            None => return Ok(None),
        },
        Object::Dict(d) => (None, d.clone()),
        _ => return Ok(None),
    };
    let mut changed = false;
    if let Some(nums) = d.get(b"Nums").map(|n| doc.resolve(n)).and_then(|n| n.as_array().cloned()) {
        let mut out: Vec<Object> = Vec::with_capacity(nums.len());
        for pair in nums.chunks(2) {
            let (Some(first), Some(key), Some(value)) = (pair.first(), pair.first().and_then(Object::as_int), pair.get(1)) else {
                out.extend(pair.iter().cloned());
                continue;
            };
            match f(doc, key, value) {
                Fix::Keep => out.extend(pair.iter().cloned()),
                Fix::Drop => changed = true,
                Fix::Array(a) => {
                    changed = true;
                    out.push(first.clone());
                    match value {
                        Object::Ref(vr) => {
                            doc.set(*vr, Object::Array(a));
                            out.push(value.clone());
                        }
                        _ => out.push(Object::Array(a)),
                    }
                }
            }
        }
        if changed {
            d.set(b"Nums".to_vec(), Object::Array(out));
        }
    }
    if let Some(kids) = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()) {
        let mut out = Vec::with_capacity(kids.len());
        let mut kids_changed = false;
        for k in kids {
            match tree_fix(doc, &k, depth + 1, f)? {
                Some(new) => {
                    kids_changed = true;
                    out.push(new);
                }
                None => out.push(k),
            }
        }
        if kids_changed {
            d.set(b"Kids".to_vec(), Object::Array(out));
            changed = true;
        }
    }
    match (changed, r) {
        (false, _) => Ok(None),
        (true, Some(r)) => {
            doc.set(r, Object::Dict(d));
            Ok(None)
        }
        (true, None) => Ok(Some(Object::Dict(d))),
    }
}

/// The parent tree: entries of removed objects and forms go, and the entry of the redacted page
/// forgets the elements of marked content that is gone.
fn fix_parent_tree(doc: &mut Document, spec: &Spec<'_>) -> Result<(), RedactError> {
    let Some(catalog) = doc.root() else { return Ok(()) };
    let Some(root_obj) = doc.get(catalog).as_dict().and_then(|c| c.get(b"StructTreeRoot").cloned()) else { return Ok(()) };
    // The root may be a direct dictionary of the catalog.
    let root_ref = root_obj.as_ref();
    let Some(root) = doc.resolve(&root_obj).as_dict().cloned() else { return Ok(()) };
    let Some(tree) = root.get(b"ParentTree").cloned() else { return Ok(()) };
    let page_key = spec.page.and_then(|p| doc.get(p).as_dict().and_then(|d| d.int(b"StructParents")));
    let mut f = |doc: &Document, key: i64, value: &Object| -> Fix {
        if spec.tree_keys.contains(&key) {
            return Fix::Drop;
        }
        if Some(key) == page_key && !spec.empty.is_empty() {
            let entries = doc.resolve(value).as_array().cloned().unwrap_or_default();
            // The element of marked content with number `i` sits at index `i`.
            return Fix::Array(
                entries.into_iter().enumerate().map(|(i, e)| if spec.empty.contains(&(i as i64)) { Object::Null } else { e }).collect(),
            );
        }
        Fix::Keep
    };
    if let Some(new) = tree_fix(doc, &tree, 0, &mut f)? {
        match root_ref {
            Some(r) => doc.update_dict(r, |d| d.set(b"ParentTree".to_vec(), new))?,
            None => {
                let mut root = root;
                root.set(b"ParentTree".to_vec(), new);
                doc.update_dict(catalog, |d| d.set(b"StructTreeRoot".to_vec(), Object::Dict(root)))?;
            }
        }
    }
    Ok(())
}

/// Clean the elements whose content `spec` removed; returns how many elements changed.
fn scrub(doc: &mut Document, spec: &Spec<'_>) -> Result<usize, RedactError> {
    let Some(root) = doc
        .root()
        .and_then(|r| doc.get(r).as_dict().cloned())
        .and_then(|c| c.get(b"StructTreeRoot").map(|s| doc.resolve(s)).and_then(|s| s.as_dict().cloned()))
    else {
        return Ok(0);
    };
    // The indirect elements with the page they inherit, their parent and their depth.
    struct Elem {
        r: ObjRef,
        d: Dict,
        parent: Option<usize>,
        hit: bool,
        gone: Vec<Object>,
    }
    let mut elems: Vec<Elem> = Vec::new();
    let mut todo: Vec<(Object, Option<ObjRef>, Option<usize>, usize)> = kids_of(doc, &root).into_iter().map(|k| (k, None, None, 0)).collect();
    let mut seen = HashSet::new();
    while let Some((k, pg, parent, depth)) = todo.pop() {
        match k {
            Object::Array(a) => todo.extend(a.into_iter().map(|x| (x, pg, parent, depth))),
            Object::Ref(r) => {
                if !seen.insert(r) {
                    continue;
                }
                if elems.len() >= MAX_ELEMS {
                    return Err(RedactError::StructureTooLarge);
                }
                let Some(d) = doc.get(r).as_dict().cloned() else {
                    // A reference to an array of kids.
                    if let Some(a) = doc.get(r).as_array() {
                        todo.extend(a.iter().map(|x| (x.clone(), pg, parent, depth)));
                    }
                    continue;
                };
                if !d.contains(b"S") {
                    continue;
                }
                let pg = d.get(b"Pg").and_then(Object::as_ref).or(pg);
                // Too deep to search: this element and everything below it is cleaned.
                let mut hit = depth >= MAX_DEPTH;
                let mut gone = Vec::new();
                let kids = kids_of(doc, &d);
                for kid in &kids {
                    let (h, g) = spec.judge(&classify(doc, kid, pg));
                    hit |= h;
                    if g {
                        gone.push(kid.clone());
                    }
                }
                let me = elems.len();
                elems.push(Elem { r, d, parent, hit, gone });
                todo.extend(kids.into_iter().map(|x| (x, pg, Some(me), depth + 1)));
            }
            _ => {}
        }
    }
    // An element that lost content loses its texts, and so do its ancestors: theirs stand for
    // everything below.
    let mut strip = vec![false; elems.len()];
    for i in 0..elems.len() {
        if !elems.get(i).is_some_and(|e| e.hit) {
            continue;
        }
        let mut cur = Some(i);
        // Parents come before their children in `elems`, so the walk ends; an element that is
        // already marked has had its ancestors marked too.
        while let Some(c) = cur {
            if strip.get(c).copied().unwrap_or(true) {
                break;
            }
            if let Some(s) = strip.get_mut(c) {
                *s = true;
            }
            cur = elems.get(c).and_then(|e| e.parent);
        }
    }
    let mut n = 0;
    for (e, strip) in elems.iter().zip(strip) {
        if !strip {
            continue;
        }
        let mut d = e.d.clone();
        for key in [&b"Alt"[..], b"ActualText", b"E"] {
            d.remove(key);
        }
        if !e.gone.is_empty() {
            // Marked content that is now empty no longer belongs to the element.
            match d.get(b"K").map(|k| doc.resolve(k)) {
                Some(k) if matches!(&*k, Object::Array(_)) => {
                    let keep: Vec<Object> = kids_of(doc, &d).into_iter().filter(|x| !e.gone.contains(x)).collect();
                    d.set(b"K".to_vec(), Object::Array(keep));
                }
                Some(_) => {
                    d.remove(b"K");
                }
                None => {}
            }
        }
        if d != e.d {
            doc.set(e.r, Object::Dict(d));
            n += 1;
        }
    }
    fix_parent_tree(doc, spec)?;
    Ok(n)
}

/// The resources a page's content is read with: its own /Resources, or the inherited one of the
/// nearest page-tree node above it that has one (ISO 32000-2, 7.8.3: /Resources is inheritable,
/// and a page-own entry shadows the inherited one whole). Page reads go through
/// `pdfcraft_model::pages`, which resolves the same way; this local walk is for one node after
/// the content pass has run. Bounded: a cycle or absurd depth in `/Parent` ends it.
fn effective_resources(doc: &Document, page: ObjRef) -> Dict {
    let mut cur = Some(page);
    for _ in 0..MAX_DEPTH {
        let Some(node) = cur else { break };
        let Some(d) = doc.get(node).as_dict().cloned() else { break };
        if let Some(res) = d.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()) {
            return res;
        }
        cur = d.get(b"Parent").and_then(Object::as_ref);
    }
    Dict::new()
}

/// Form XObjects reachable from `roots` through their resources. A form without its own
/// /Resources draws with the resources of the scope that drew it, so each form is followed with
/// the resources of its parent scope (`inherited` is the page's for the roots).
fn forms_below(doc: &Document, roots: &[ObjRef], inherited: &Dict) -> Result<HashSet<ObjRef>, RedactError> {
    let mut seen: HashSet<ObjRef> = HashSet::new();
    let mut todo: Vec<(ObjRef, Dict)> = roots.iter().map(|r| (*r, inherited.clone())).collect();
    while let Some((r, parent)) = todo.pop() {
        if !seen.insert(r) {
            continue;
        }
        if seen.len() > MAX_FORMS {
            return Err(RedactError::StructureTooLarge);
        }
        let o = doc.get(r);
        let Some(d) = o.as_dict() else { continue };
        let own = d.get(b"Resources").and_then(|x| doc.dict(x));
        let res = own.unwrap_or_else(|| parent.clone());
        if let Some(xo) = res.get(b"XObject").and_then(|x| doc.dict(x)) {
            todo.extend(xo.iter().filter_map(|(_, v)| v.as_ref()).map(|f| (f, res.clone())));
        }
    }
    Ok(seen)
}

/// The XObject names `data` draws with `Do`.
fn names_in(data: &[u8]) -> HashSet<Vec<u8>> {
    pdfcraft_content::parse(data)
        .ops
        .iter()
        .filter(|op| op.op.as_slice() == b"Do")
        .filter_map(|op| op.operands.first().and_then(Object::as_name).map(<[u8]>::to_vec))
        .collect()
}

/// The XObject names `before` draws with `Do` that `after` no longer does.
pub(crate) fn retired_names(before: &[u8], after: &[u8]) -> Vec<Vec<u8>> {
    let is = names_in(after);
    names_in(before).into_iter().filter(|n| !is.contains(n)).collect()
}

/// The XObject names the pages still draw: in their own content, and inside the forms those
/// content streams still draw (each followed with the resources of the scope that drew it, so a
/// form without its own /Resources is counted with what it inherits). Forms no page draws any
/// more do not count: only the resources entries keep them reachable, and those are exactly what
/// retirement is cleaning. `None` when a page or a drawn form can't be read in full: it could be
/// drawing any name, so nothing may then be retired from the page tree.
fn names_still_drawn(doc: &Document) -> Option<HashSet<Vec<u8>>> {
    let mut out = HashSet::new();
    for (pi, p) in pdfcraft_model::pages(doc).iter().enumerate() {
        out.extend(names_drawn_on_page(doc, pi, p)?);
    }
    Some(out)
}

/// The XObject names one page still draws: in its own content, and inside the forms it draws
/// (each followed with the resources that form draws with — its own, or the enclosing scope's).
/// `None` when the page or a drawn form can't be read in full: it could be drawing any name.
fn names_drawn_on_page(doc: &Document, pi: usize, p: &pdfcraft_model::Page) -> Option<HashSet<Vec<u8>>> {
    let mut out = HashSet::new();
    let (_, data) = crate::page_streams(doc, &p.dict, pi).ok()?;
    let joined = data.join(&b'\n');
    let top = names_in(&joined);
    out.extend(top.iter().cloned());
    let res = p.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    let xobjects = res.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
    let mut seen = HashSet::new();
    for name in &top {
        let Some(entry) = xobjects.get(name) else { continue };
        let Object::Stream(s) = &*doc.resolve(entry) else { continue };
        if s.dict.name(b"Subtype") != Some(b"Form") {
            continue;
        }
        if !entry.as_ref().is_some_and(|r| seen.insert(r)) {
            continue;
        }
        if seen.len() > MAX_FORMS {
            return None;
        }
        names_below(doc, s, &res, &mut out, &mut seen, 1)?;
    }
    Some(out)
}

/// The `Do` names drawn by the form `s` and by the forms it draws (`parent`: the resources of the
/// scope that drew `s`, which the forms below without their own /Resources draw with); `None`
/// when they nest too deep or one can't be read in full (it could draw anything).
fn names_below(doc: &Document, s: &Stream, parent: &Dict, out: &mut HashSet<Vec<u8>>, seen: &mut HashSet<ObjRef>, depth: usize) -> Option<()> {
    if depth > MAX_DEPTH {
        return None;
    }
    let data = s.decoded_strict_within(crate::limits::MAX_STREAM).ok()?;
    let top = names_in(&data);
    out.extend(top.iter().cloned());
    let own = s.dict.get(b"Resources").and_then(|x| doc.resolve(x).as_dict().cloned());
    let res = own.unwrap_or_else(|| parent.clone());
    let xobjects = res.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
    for name in &top {
        let Some(entry) = xobjects.get(name) else { continue };
        let Object::Stream(inner) = &*doc.resolve(entry) else { continue };
        if inner.dict.name(b"Subtype") != Some(b"Form") {
            continue;
        }
        if !entry.as_ref().is_some_and(|r| seen.insert(r)) {
            continue;
        }
        if seen.len() > MAX_FORMS {
            return None;
        }
        names_below(doc, inner, &res, out, seen, depth + 1)?;
    }
    Some(())
}

/// The XObjects the page no longer draws by one of the `gone` names (from the content before and
/// after, plus what rewritten forms retired — against the form's own resources or the resources
/// it inherited): they leave the page's /Resources, and they leave every page-tree node above
/// that owns an entry of the same name — that is where an inherited entry lives, and while the
/// node maps the name the original object stays reachable and a full save keeps it. An entry only
/// goes when its owner's render no longer draws the name — for a node, no page; for the page's
/// own resources, the page itself — so a sibling that still draws the name keeps the entry, and
/// the original with it. The retired XObjects are returned.
pub(crate) fn retire_forms(doc: &mut Document, page: ObjRef, pi: usize, gone: &[Vec<u8>]) -> Result<Vec<ObjRef>, RedactError> {
    let mut forms = Vec::new();
    if gone.is_empty() {
        return Ok(forms);
    }
    // The page's own resources first. An entry only goes when this page's render — its own
    // content, and the forms it still draws, each followed with the resources it draws with —
    // no longer draws the name: a rewritten form may retire a name the page still draws by
    // another route, and that entry keeps the original the page still shows. A sibling's
    // same-named entry elsewhere does not keep this page's entry: page-own shadows an inherited
    // one per entry. When the page or a drawn form can't be read in full, nothing goes: it
    // could be drawing any name.
    let mut drawn: Option<Option<HashSet<Vec<u8>>>> = None;
    if let Some(mut res) = doc.get(page).as_dict().and_then(|d| d.get(b"Resources").and_then(|r| doc.dict(r)))
        && let Some(mut xo) = res.get(b"XObject").and_then(|x| doc.dict(x))
    {
        for n in gone {
            if !xo.contains(n) {
                continue;
            }
            let drawn = drawn.get_or_insert_with(|| pdfcraft_model::pages(doc).get(pi).and_then(|p| names_drawn_on_page(doc, pi, p)));
            let Some(drawn) = drawn else { break };
            if drawn.contains(n) {
                continue;
            }
            if let Some(r) = xo.remove(n).and_then(|o| o.as_ref()) {
                forms.push(r);
            }
        }
        if !forms.is_empty() {
            res.set(b"XObject".to_vec(), Object::Dict(xo));
            doc.update_dict(page, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?;
        }
    }
    // Then the nodes the page inherited entries from.
    retire_inherited(doc, page, gone)?;
    Ok(forms)
}

/// Strip the `gone` names from the /XObject of every page-tree node above `page` that owns one,
/// so far as no page still draws them. A shared (indirect) resources dictionary is written back
/// as the object it is: everyone it reaches loses exactly the names no page draws any more.
fn retire_inherited(doc: &mut Document, page: ObjRef, gone: &[Vec<u8>]) -> Result<(), RedactError> {
    let mut seen = HashSet::new();
    let mut drawn: Option<Option<HashSet<Vec<u8>>>> = None;
    let mut cur = page;
    for _ in 0..MAX_DEPTH {
        let Some(parent) = doc.get(cur).as_dict().and_then(|d| d.get(b"Parent")).and_then(Object::as_ref) else { break };
        if !seen.insert(parent) {
            break;
        }
        cur = parent;
        let Some(res_v) = doc.get(parent).as_dict().and_then(|d| d.get(b"Resources").cloned()) else { continue };
        let indirect = res_v.as_ref();
        let Some(mut res) = doc.dict(&res_v) else { continue };
        let Some(mut xo) = res.get(b"XObject").and_then(|x| doc.dict(x)) else { continue };
        if !gone.iter().any(|n| xo.contains(n)) {
            continue;
        }
        let Some(drawn) = drawn.get_or_insert_with(|| names_still_drawn(doc)) else { break };
        let mut removed = false;
        for n in gone {
            if drawn.contains(n) {
                continue;
            }
            removed |= xo.remove(n).is_some();
        }
        if !removed {
            continue;
        }
        res.set(b"XObject".to_vec(), Object::Dict(xo));
        match indirect {
            Some(r) => doc.set(r, Object::Dict(res)),
            None => doc.update_dict(parent, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?,
        }
    }
    Ok(())
}

/// Clean the elements owning `changed` marked content on `page`, and marked content kept in the
/// `rewritten` form XObjects (and the forms inside them); returns how many elements changed.
pub(crate) fn clean(
    doc: &mut Document,
    page: ObjRef,
    changed: &HashSet<i64>,
    empty: &HashSet<i64>,
    rewritten: &[ObjRef],
) -> Result<usize, RedactError> {
    if changed.is_empty() && rewritten.is_empty() {
        return Ok(0);
    }
    // A rewritten form without its own /Resources drew with the page's effective resources: the
    // forms below it are followed with those.
    let inherited = effective_resources(doc, page);
    let forms = forms_below(doc, rewritten, &inherited)?;
    let tree_keys = forms.iter().filter_map(|f| doc.get(*f).as_dict().and_then(|d| d.int(b"StructParents"))).collect();
    scrub(doc, &Spec { page: Some(page), changed: changed.clone(), empty: empty.clone(), forms, objs: &[], tree_keys })
}

/// Drop the references to removed annotations from the structure tree (and the alternate text
/// of the elements that held them); returns how many elements changed.
pub(crate) fn drop_objects(doc: &mut Document, removed: &[ObjRef]) -> Result<usize, RedactError> {
    if removed.is_empty() {
        return Ok(0);
    }
    let tree_keys = removed.iter().filter_map(|a| doc.get(*a).as_dict().and_then(|d| d.int(b"StructParent"))).collect();
    scrub(doc, &Spec { objs: removed, tree_keys, ..Spec::default() })
}
