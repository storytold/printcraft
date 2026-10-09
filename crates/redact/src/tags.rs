//! Redaction and the structure tree: tags must not keep what was removed from the page.
//!
//! Marked content (MCIDs) that applying redactions changed is found by comparing each MCID's
//! operators before and after. Elements owning such content lose their alternate text, actual
//! text and expansion (which may spell out the removed words); references to marked content that
//! is now empty are dropped from the element. The same goes for the ancestors of such an element
//! (their alternate text stands for everything below), for marked content kept in a form XObject
//! that was rewritten (`/Stm`), and for object references (`/OBJR`) to removed annotations or
//! forms. The parent tree forgets what no longer exists.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

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

/// Form XObjects reachable from `roots` through their resources.
fn forms_below(doc: &Document, roots: &[ObjRef]) -> Result<HashSet<ObjRef>, RedactError> {
    let mut seen: HashSet<ObjRef> = HashSet::new();
    let mut todo: Vec<ObjRef> = roots.to_vec();
    while let Some(r) = todo.pop() {
        if !seen.insert(r) {
            continue;
        }
        if seen.len() > MAX_FORMS {
            return Err(RedactError::StructureTooLarge);
        }
        let o = doc.get(r);
        let Some(d) = o.as_dict() else { continue };
        let xo = d.get(b"Resources").and_then(|x| doc.dict(x)).and_then(|res| res.get(b"XObject").and_then(|x| doc.dict(x)));
        if let Some(xo) = xo {
            todo.extend(xo.iter().filter_map(|(_, v)| v.as_ref()));
        }
    }
    Ok(seen)
}

/// The XObject names `before` draws with `Do` that `after` no longer does.
pub(crate) fn retired_names(before: &[u8], after: &[u8]) -> Vec<Vec<u8>> {
    let names = |data: &[u8]| -> HashSet<Vec<u8>> {
        pdfcraft_content::parse(data)
            .ops
            .into_iter()
            .filter(|op| op.op.as_slice() == b"Do")
            .filter_map(|op| op.operands.first().and_then(Object::as_name).map(<[u8]>::to_vec))
            .collect()
    };
    let is = names(after);
    names(before).into_iter().filter(|n| !is.contains(n)).collect()
}

/// The form XObjects the page drew before but no longer draws by that name: applying the marks
/// replaced them with rewritten copies (or removed them). They leave the page's resources, so the
/// original content is not kept there; the forms are returned.
pub(crate) fn retire_forms(doc: &mut Document, page: ObjRef, before: &[u8], after: &[u8]) -> Result<Vec<ObjRef>, RedactError> {
    let gone = retired_names(before, after);
    let Some(mut res) = doc.get(page).as_dict().and_then(|d| d.get(b"Resources").and_then(|r| doc.dict(r))) else { return Ok(Vec::new()) };
    let Some(mut xo) = res.get(b"XObject").and_then(|x| doc.dict(x)) else { return Ok(Vec::new()) };
    let mut forms = Vec::new();
    for n in &gone {
        if let Some(r) = xo.remove(n).and_then(|o| o.as_ref()) {
            forms.push(r);
        }
    }
    if !forms.is_empty() {
        res.set(b"XObject".to_vec(), Object::Dict(xo));
        doc.update_dict(page, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?;
    }
    Ok(forms)
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
    let forms = forms_below(doc, rewritten)?;
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
