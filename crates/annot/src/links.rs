//! Links (ISO 32000-2 §12.5.6.5): create, edit and remove link annotations with Acrobat's Link
//! Properties (appearance and a go-to-page or open-a-web-page action).

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};

use crate::{AnnotError, annots, page_ref, page_refs, set_annots};

/// Where a link goes.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkAction {
    /// A page of this document (0-based), shown fitting the window (`/Fit`).
    Page(usize),
    Uri(String),
    /// Something PdfCraft doesn't edit yet (named destination, JavaScript…): kept as is.
    Other(String),
}

/// Acrobat's Link Properties ▸ Appearance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinkStyle {
    /// A visible rectangle (`/Border` width > 0) or invisible.
    pub visible: bool,
    pub color: [f64; 3],
    /// 1 thin, 2 medium, 3 thick.
    pub width: f64,
    pub dashed: bool,
    pub underline: bool,
    /// Highlight style when clicked: None, Invert, Outline, Inset (`/H` N, I, O, P).
    pub highlight: Highlight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Highlight {
    None,
    Invert,
    Outline,
    Inset,
}

impl Highlight {
    pub const ALL: [Highlight; 4] = [Highlight::None, Highlight::Invert, Highlight::Outline, Highlight::Inset];

    fn code(self) -> &'static str {
        match self {
            Highlight::None => "N",
            Highlight::Invert => "I",
            Highlight::Outline => "O",
            Highlight::Inset => "P",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Highlight::None => "None",
            Highlight::Invert => "Invert",
            Highlight::Outline => "Outline",
            Highlight::Inset => "Inset",
        }
    }
}

impl Default for LinkStyle {
    fn default() -> Self {
        // Acrobat's defaults for a new link: invisible rectangle, invert highlight.
        LinkStyle { visible: false, color: [0.0, 0.0, 1.0], width: 1.0, dashed: false, underline: false, highlight: Highlight::Invert }
    }
}

/// A link on a page.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkItem {
    pub page: usize,
    /// Index in the page's `/Annots`.
    pub index: usize,
    pub obj: Option<ObjRef>,
    pub rect: [f64; 4],
    pub action: LinkAction,
    pub style: LinkStyle,
}

fn nums(doc: &Document, o: Option<&Object>) -> Vec<f64> {
    o.map(|o| doc.resolve(o)).and_then(|a| a.as_array().map(|a| a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect())).unwrap_or_default()
}

fn action_of(doc: &Document, d: &Dict, pages: &[ObjRef]) -> LinkAction {
    let dest_page = |dest: &Object| -> Option<usize> {
        let a = doc.resolve(dest);
        let first = a.as_array()?.first()?.clone();
        match first {
            Object::Ref(r) => pages.iter().position(|p| *p == r),
            Object::Int(i) => usize::try_from(i).ok(),
            _ => None,
        }
    };
    if let Some(dest) = d.get(b"Dest") {
        return dest_page(dest).map_or_else(|| LinkAction::Other("named destination".into()), LinkAction::Page);
    }
    let Some(a) = d.get(b"A").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned()) else { return LinkAction::Other("no action".into()) };
    match a.name(b"S") {
        Some(b"URI") => LinkAction::Uri(
            a.get(b"URI").and_then(|u| doc.resolve(u).as_string().map(|s| String::from_utf8_lossy(&s.bytes).into_owned())).unwrap_or_default(),
        ),
        Some(b"GoTo") => a.get(b"D").and_then(dest_page).map_or_else(|| LinkAction::Other("named destination".into()), LinkAction::Page),
        Some(s) => LinkAction::Other(String::from_utf8_lossy(s).into_owned()),
        None => LinkAction::Other("no action".into()),
    }
}

fn style_of(doc: &Document, d: &Dict) -> LinkStyle {
    let bs = d.get(b"BS").and_then(|b| doc.resolve(b).as_dict().cloned());
    let width = match &bs {
        Some(b) => b.get(b"W").and_then(Object::as_f64).unwrap_or(1.0),
        None => nums(doc, d.get(b"Border")).get(2).copied().unwrap_or(1.0),
    };
    let s = bs.as_ref().and_then(|b| b.name(b"S").map(<[u8]>::to_vec)).unwrap_or_default();
    let c = nums(doc, d.get(b"C"));
    LinkStyle {
        visible: width > 0.0,
        color: if c.len() == 3 { [c[0], c[1], c[2]] } else { [0.0, 0.0, 1.0] },
        width: if width > 0.0 { width } else { 1.0 },
        dashed: s == b"D",
        underline: s == b"U",
        highlight: match d.name(b"H") {
            Some(b"N") => Highlight::None,
            Some(b"O") => Highlight::Outline,
            Some(b"P") => Highlight::Inset,
            _ => Highlight::Invert,
        },
    }
}

/// Every link in the document.
pub fn list(doc: &Document) -> Vec<LinkItem> {
    let Ok(pages) = page_refs(doc) else { return Vec::new() };
    let mut out = Vec::new();
    for (pi, p) in pages.iter().enumerate() {
        for (k, a) in annots(doc, *p).iter().enumerate() {
            let Some(d) = doc.resolve(a).as_dict().cloned() else { continue };
            if d.name(b"Subtype") != Some(b"Link") {
                continue;
            }
            let r = nums(doc, d.get(b"Rect"));
            if r.len() != 4 {
                continue;
            }
            out.push(LinkItem {
                page: pi,
                index: k,
                obj: a.as_ref(),
                rect: [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])],
                action: action_of(doc, &d, &pages),
                style: style_of(doc, &d),
            });
        }
    }
    out
}

fn write_style(d: &mut Dict, s: &LinkStyle) {
    let mut bs = Dict::new();
    bs.set(b"W".to_vec(), Object::Real(if s.visible { s.width.clamp(0.5, 12.0) } else { 0.0 }));
    bs.set(
        b"S".to_vec(),
        Object::name(if s.dashed {
            "D"
        } else if s.underline {
            "U"
        } else {
            "S"
        }),
    );
    if s.dashed {
        bs.set(b"D".to_vec(), Object::Array(vec![Object::Int(3)]));
    }
    d.set(b"BS".to_vec(), Object::Dict(bs));
    d.remove(b"Border");
    d.set(b"C".to_vec(), Object::Array(s.color.iter().map(|v| Object::Real(v.clamp(0.0, 1.0))).collect()));
    d.set(b"H".to_vec(), Object::name(s.highlight.code()));
}

fn write_action(doc: &Document, d: &mut Dict, a: &LinkAction) -> Result<(), AnnotError> {
    match a {
        LinkAction::Page(p) => {
            let target = page_ref(doc, *p)?;
            d.remove(b"A");
            d.set(b"Dest".to_vec(), Object::Array(vec![Object::Ref(target), Object::name("Fit")]));
        }
        LinkAction::Uri(u) => {
            let u = u.trim();
            if u.is_empty() {
                return Err(AnnotError::Invalid("type the web address first".into()));
            }
            d.remove(b"Dest");
            let mut act = Dict::new();
            act.set(b"S".to_vec(), Object::name("URI"));
            // URIs are 7-bit ASCII strings (§12.6.4.8).
            act.set(b"URI".to_vec(), PdfString::literal(u.as_bytes().to_vec()));
            d.set(b"A".to_vec(), Object::Dict(act));
        }
        LinkAction::Other(_) => {}
    }
    Ok(())
}

/// Check link geometry before writing PDF objects: a user or automation call can supply
/// non-finite coordinates or a finite pair whose difference overflows to infinity.
fn checked_rect(rect: [f64; 4]) -> Result<[f64; 4], AnnotError> {
    if !rect.iter().all(|v| v.is_finite()) {
        return Err(AnnotError::Invalid("the link area must have finite coordinates".into()));
    }
    let r = [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])];
    let (width, height) = (r[2] - r[0], r[3] - r[1]);
    if !width.is_finite() || !height.is_finite() || width < 2.0 || height < 2.0 {
        return Err(AnnotError::Invalid("the link area is too small or too large".into()));
    }
    Ok(r)
}

/// Add a link over `rect` (user space). Returns its index in the page's `/Annots`.
pub fn add(doc: &mut Document, page: usize, rect: [f64; 4], action: &LinkAction, style: &LinkStyle) -> Result<usize, AnnotError> {
    let p = page_ref(doc, page)?;
    let r = checked_rect(rect)?;
    if matches!(action, LinkAction::Other(_)) {
        return Err(AnnotError::Invalid("choose a page or a web address".into()));
    }
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Annot"));
    d.set(b"Subtype".to_vec(), Object::name("Link"));
    d.set(b"Rect".to_vec(), Object::Array(r.iter().map(|v| Object::Real(*v)).collect()));
    d.set(b"P".to_vec(), Object::Ref(p));
    d.set(b"F".to_vec(), Object::Int(4));
    write_style(&mut d, style);
    write_action(doc, &mut d, action)?;
    let obj = doc.add(Object::Dict(d));
    let mut list = annots(doc, p);
    list.push(Object::Ref(obj));
    let index = list.len() - 1;
    set_annots(doc, p, list)?;
    Ok(index)
}

fn link_at(doc: &Document, page: usize, index: usize) -> Result<(ObjRef, ObjRef), AnnotError> {
    let p = page_ref(doc, page)?;
    let a = annots(doc, p).get(index).cloned().ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    let r = a.as_ref().ok_or_else(|| AnnotError::Invalid("an inline link can't be edited".into()))?;
    if doc.get(r).as_dict().and_then(|d| d.name(b"Subtype").map(<[u8]>::to_vec)).as_deref() != Some(b"Link") {
        return Err(AnnotError::Invalid("that annotation isn't a link".into()));
    }
    Ok((p, r))
}

/// Link Properties: change a link's area, action and/or appearance.
pub fn set(
    doc: &mut Document,
    page: usize,
    index: usize,
    rect: Option<[f64; 4]>,
    action: Option<&LinkAction>,
    style: Option<&LinkStyle>,
) -> Result<(), AnnotError> {
    let (_, r) = link_at(doc, page, index)?;
    let mut d = doc.get(r).as_dict().cloned().unwrap_or_default();
    if let Some(rect) = rect {
        let rr = checked_rect(rect)?;
        d.set(b"Rect".to_vec(), Object::Array(rr.iter().map(|v| Object::Real(*v)).collect()));
    }
    if let Some(s) = style {
        write_style(&mut d, s);
    }
    if let Some(a) = action {
        write_action(doc, &mut d, a)?;
    }
    doc.set(r, Object::Dict(d));
    Ok(())
}

/// Delete one link.
pub fn delete(doc: &mut Document, page: usize, index: usize) -> Result<(), AnnotError> {
    let (p, _) = link_at(doc, page, index)?;
    let mut list = annots(doc, p);
    list.remove(index);
    set_annots(doc, p, list)
}

/// Remove every link on `pages` (all when `None`). Returns how many went.
pub fn remove_all(doc: &mut Document, pages: Option<&[usize]>) -> Result<usize, AnnotError> {
    let refs = page_refs(doc)?;
    let mut n = 0;
    for (pi, p) in refs.iter().enumerate() {
        if pages.is_some_and(|ps| !ps.contains(&pi)) {
            continue;
        }
        let list = annots(doc, *p);
        let kept: Vec<Object> = list
            .iter()
            .filter(|a| doc.resolve(a).as_dict().and_then(|d| d.name(b"Subtype").map(<[u8]>::to_vec)).as_deref() != Some(b"Link"))
            .cloned()
            .collect();
        if kept.len() != list.len() {
            n += list.len() - kept.len();
            set_annots(doc, *p, kept)?;
        }
    }
    Ok(n)
}

/// Find web addresses in text: `http(s)://…`, `www.…` and bare e-mail addresses become
/// `mailto:`. Returns character ranges and the URI for each.
pub fn find_urls(chars: &[char]) -> Vec<(std::ops::Range<usize>, String)> {
    let url_char = |c: char| !c.is_whitespace() && !"<>\"'()[]{}".contains(c);
    // Case-insensitive prefix test that never steps past the end. Comparing against the original
    // characters keeps every offset in `chars`'s own index space: case-folding a copy can change
    // its length (U+0130 becomes "i" plus a combining dot), so a folded copy must never supply
    // offsets into the source (that mismatch panicked; #816).
    let starts_with =
        |start: usize, pat: &str| pat.chars().enumerate().all(|(k, p)| chars.get(start + k).is_some_and(|c| c.eq_ignore_ascii_case(&p)));
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let boundary = i == 0 || chars.get(i - 1).is_some_and(|c| !c.is_alphanumeric());
        if boundary && (starts_with(i, "http://") || starts_with(i, "https://") || starts_with(i, "www.")) {
            let mut j = i;
            while chars.get(j).is_some_and(|c| url_char(*c)) {
                j += 1;
            }
            // Trailing punctuation belongs to the sentence.
            while j > i && chars.get(j - 1).is_some_and(|c| ".,;:!?".contains(*c)) {
                j -= 1;
            }
            let s: String = chars.get(i..j).map(|c| c.iter().collect()).unwrap_or_default();
            if s.len() > 6 && s.contains('.') {
                let uri = if s.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("www.")) { format!("http://{s}") } else { s };
                out.push((i..j, uri));
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Create links for found URLs: `(page, quads, uri)` with quads in user space (one link per
/// line rectangle). Returns how many links were added.
pub fn add_many(doc: &mut Document, links: &[(usize, Vec<[f64; 4]>, String)], style: &LinkStyle) -> Result<usize, AnnotError> {
    let mut n = 0;
    for (page, rects, uri) in links {
        for r in rects {
            add(doc, *page, *r, &LinkAction::Uri(uri.clone()), style)?;
            n += 1;
        }
    }
    Ok(n)
}
