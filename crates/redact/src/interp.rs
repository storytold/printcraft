//! The content interpreter that applies (or verifies) redaction on one content scope: a page's
//! streams or a form XObject's.
//!
//! It tracks just enough graphics state to place every glyph, image and path in user space:
//! the CTM stack, the text state (font, size, spacing, scaling, rise, leading) and the text
//! matrices. In apply mode it rewrites the operators:
//! - text: glyphs whose boxes overlap a region are cut out of `Tj`/`TJ`/`'`/`"`, replaced by a
//!   `TJ` displacement of the same width so the remaining text stays exactly where it was;
//! - images: fully covered → the `Do` is removed; partly covered → the covered pixels are
//!   cleared in a copy of the image (or the image is removed when its codec can't be re-encoded);
//!   inline images under a region are removed;
//! - vectors: fully covered paths are removed; partly covered ones are clipped so nothing shows
//!   inside the regions; shadings are clipped the same way;
//! - form XObjects under a region are rewritten recursively into new objects (copy-on-write, so
//!   other pages using the original are untouched).
//!
//! In verify mode nothing changes; glyphs and inline images that still overlap a region are
//! counted.
//!
//! Where the geometry of what is painted can't be established (a font whose metrics can't be
//! resolved, vertical writing, text or images hidden in tiling patterns, Type 3 glyph procedures
//! or soft-mask groups, form matrices that can't be read, inline images whose extent is
//! ambiguous, content too large to scan) the scope records an
//! [`Unsupported`] reason and the caller fails the whole operation: nothing is guessed.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use pdfcraft_content::{Matrix, Op, Pieces, contains, num, overlaps, parse, serialize_ops, string};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::limits::{Budget, EDGE_EPS, MAX_CMAP, MAX_DEPTH, MAX_FORMS, MAX_PAGE_TOTAL, Refused};
use crate::{Report, image, tags};
use pdfcraft_fonts::pdf::Metrics;

/// Why content under a mark can't be redacted safely; the operation fails with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Unsupported {
    #[error("text uses a font whose metrics can't be resolved, so it can't be placed")]
    UnresolvedFont,
    #[error("text uses vertical writing, which isn't supported")]
    VerticalWriting,
    #[error("a tiling pattern carries text")]
    PatternText,
    #[error("a tiling pattern stream can't be read")]
    PatternUnreadable,
    #[error("a soft-mask group carries text or can't be read")]
    SoftMask,
    #[error("a Type 3 glyph procedure carries text or can't be read")]
    Type3Text,
    #[error("a tiling pattern, soft-mask group or Type 3 glyph procedure paints an image")]
    HiddenImage,
    #[error("an inline image's extent can't be established")]
    InlineImage,
    #[error("a form XObject's matrix can't be read, so its content can't be placed")]
    FormMatrix,
    #[error("the content has bytes that can't be read as operators")]
    UnparsedContent,
    #[error("the content is too large to be scanned in full")]
    TooLarge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Apply,
    Verify,
}

#[derive(Clone)]
struct Gs {
    ctm: Matrix,
    font: Rc<FontInfo>,
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    render_mode: i64,
    scale: f64,
    leading: f64,
    rise: f64,
}

/// A run of glyphs cut out under a region, kept for the redaction proof (`verify`).
pub(crate) struct Removed {
    /// Index of the region (in `Scope::rects`) the run lay under.
    pub rect: usize,
    /// The character codes as they stood in the string.
    pub raw: Vec<u8>,
    /// What the font says they showed (codes without a known meaning are left out).
    pub text: String,
}

/// What processing a scope produced.
#[derive(Default)]
pub(crate) struct Output {
    /// The rewritten streams (`None` = unchanged).
    pub streams: Vec<Option<Vec<u8>>>,
    /// New XObjects the rewritten content refers to: (resource name, object).
    pub xobjects: Vec<(Vec<u8>, ObjRef)>,
    /// Glyphs / inline images still overlapping a region (verify mode).
    pub residue: usize,
    /// Content whose geometry couldn't be established (the first reason found).
    pub failure: Option<Unsupported>,
    /// Property lists (resource name, list) that held alternate text of removed content, with
    /// that text taken out; the caller puts them into the scope's `/Properties` resources.
    pub properties: Vec<(Vec<u8>, Dict)>,
    /// Names this scope's rewritten forms retired (from the form's own resources or from the
    /// resources it inherited): they must leave every map above that still holds them — the
    /// page's own resources and the page-tree nodes — or the originals stay reachable (apply
    /// mode).
    pub inherited_gone: Vec<Vec<u8>>,
}

/// A font's metrics plus whether glyph positions computed from them can be trusted.
struct FontInfo {
    metrics: Metrics,
    /// `Some` when positions can't be trusted (no widths, unresolved encoding, vertical mode).
    doubt: Option<Unsupported>,
    /// Simple fonts: the codes that have a width.
    coded: Option<Range<u32>>,
    /// Type 3: the glyph box in text space (per unit of font size).
    type3_box: Option<[f64; 4]>,
}

impl FontInfo {
    /// The stand-in for text shown before any font is selected or with a font that doesn't
    /// exist.
    fn unresolved() -> Self {
        FontInfo { metrics: Metrics::fallback(), doubt: Some(Unsupported::UnresolvedFont), coded: None, type3_box: None }
    }

    /// Does this font give `code` a width we can rely on?
    fn certain(&self, code: u32) -> Option<Unsupported> {
        self.doubt.or_else(|| self.coded.as_ref().filter(|r| !r.contains(&code)).map(|_| Unsupported::UnresolvedFont))
    }
}

/// Numbers of an array, `None` unless every entry is one.
fn strict_nums(doc: &Document, o: Option<&Object>) -> Option<Vec<f64>> {
    doc.resolve(o?).as_array()?.iter().map(|x| doc.resolve(x).as_f64().filter(|v| v.is_finite())).collect()
}

/// Read a font dictionary and decide whether its geometry can be trusted. Metrics that would
/// have to be guessed (standard fonts without `/Widths` other than Courier, predefined CMaps
/// other than Identity, composite fonts without descendant widths) make the font doubtful.
fn font_info(doc: &Document, d: &Dict) -> FontInfo {
    let mut info = FontInfo { metrics: Metrics::from_dict(doc, d), doubt: None, coded: None, type3_box: None };
    let unresolved = Some(Unsupported::UnresolvedFont);
    match d.name(b"Subtype") {
        Some(b"Type0") => {
            let desc = d.get(b"DescendantFonts").map(|x| doc.resolve(x)).and_then(|a| a.as_array().and_then(|a| a.first().cloned()));
            let desc = desc.and_then(|x| doc.resolve(&x).as_dict().cloned());
            info.doubt = match (desc, d.get(b"Encoding").map(|e| doc.resolve(e))) {
                (Some(desc), Some(enc)) if desc.contains(b"DW") || desc.contains(b"W") => match &*enc {
                    Object::Name(n) if n.starts_with(b"Identity-") => (n.as_slice() == b"Identity-V").then_some(Unsupported::VerticalWriting),
                    // Any other predefined CMap would need its CID mapping.
                    Object::Name(_) => unresolved,
                    Object::Stream(s) => match s.decoded_strict_within(MAX_CMAP) {
                        Ok(data) if s.dict.int(b"WMode").is_some_and(|w| w != 0) || cmap_vertical(&data) => Some(Unsupported::VerticalWriting),
                        Ok(_) => None,
                        Err(_) => unresolved,
                    },
                    _ => unresolved,
                },
                _ => unresolved,
            };
        }
        Some(b"Type3") => {
            let widths = strict_nums(doc, d.get(b"Widths")).filter(|w| !w.is_empty());
            let fm = strict_nums(doc, d.get(b"FontMatrix")).filter(|m| m.len() == 6 && m[0] != 0.0);
            let bbox = strict_nums(doc, d.get(b"FontBBox")).filter(|b| b.len() == 4);
            let first = d.get(b"FirstChar").and_then(|f| doc.resolve(f).as_int()).and_then(|f| u32::try_from(f).ok());
            match (widths, fm, bbox, first) {
                (Some(w), Some(fm), Some(bb), Some(first)) => {
                    info.coded = u32::try_from(w.len()).ok().and_then(|n| first.checked_add(n)).map(|end| first..end);
                    let m = Matrix([fm[0], fm[1], fm[2], fm[3], fm[4], fm[5]]);
                    info.type3_box = Some(m.bbox([bb[0].min(bb[2]), bb[1].min(bb[3]), bb[0].max(bb[2]), bb[1].max(bb[3])]));
                    info.doubt = info.coded.is_none().then_some(Unsupported::UnresolvedFont);
                }
                _ => info.doubt = unresolved,
            }
        }
        Some(b"Type1" | b"MMType1" | b"TrueType") => {
            let widths = strict_nums(doc, d.get(b"Widths")).filter(|w| !w.is_empty());
            let first = d.get(b"FirstChar").and_then(|f| doc.resolve(f).as_int()).and_then(|f| u32::try_from(f).ok());
            match (widths, first) {
                (Some(w), Some(first)) => {
                    info.coded = u32::try_from(w.len()).ok().and_then(|n| first.checked_add(n)).map(|end| first..end);
                    info.doubt = info.coded.is_none().then_some(Unsupported::UnresolvedFont);
                }
                // The only standard fonts whose widths are exact without a table.
                _ => {
                    let base = d.name(b"BaseFont").unwrap_or(b"");
                    let courier = matches!(base, b"Courier" | b"Courier-Bold" | b"Courier-Oblique" | b"Courier-BoldOblique");
                    info.doubt = if courier && !d.contains(b"Widths") { None } else { unresolved };
                }
            }
        }
        _ => info.doubt = unresolved,
    }
    info
}

/// Does an embedded CMap select vertical writing, i.e. does it hold `/WMode` followed by a
/// number other than zero? Read as tokens (comments and strings skipped), so that any spacing,
/// line break or spelling of the number (`1`, `01`, `1.0`) is seen.
pub(crate) fn cmap_vertical(data: &[u8]) -> bool {
    let is_ws = |b: u8| matches!(b, b'\0' | b'\t' | b'\n' | b'\x0C' | b'\r' | b' ');
    let is_delim = |b: u8| matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%');
    let mut tokens: Vec<&[u8]> = Vec::new();
    let mut i = 0;
    while let Some(&b) = data.get(i) {
        match b {
            _ if is_ws(b) => i += 1,
            b'%' => i = data.get(i..).and_then(|r| r.iter().position(|c| matches!(c, b'\n' | b'\r'))).map_or(data.len(), |n| i + n),
            b'(' => {
                // A literal string, nesting parentheses and honouring backslash escapes.
                let (mut depth, mut j) = (0usize, i);
                while let Some(&c) = data.get(j) {
                    match c {
                        b'\\' => j += 1,
                        b'(' => depth += 1,
                        b')' => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                i = j + 1;
            }
            _ if is_delim(b) && b != b'/' => i += 1,
            _ => {
                let start = i;
                i += 1;
                while data.get(i).is_some_and(|c| !is_ws(*c) && !is_delim(*c)) {
                    i += 1;
                }
                if let Some(t) = data.get(start..i) {
                    tokens.push(t);
                }
            }
        }
    }
    // A name token is `/` alone (the `/` is a delimiter), so look at its successor.
    tokens.windows(2).any(|w| {
        let [a, b] = w else { return false };
        *a == b"/WMode" && std::str::from_utf8(b).ok().and_then(|n| n.parse::<f64>().ok()).is_none_or(|n| n != 0.0)
    })
}

/// What a content stream that is painted without a `Do` was found to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Painted {
    Text,
    Image,
}

/// Does this content stream (a pattern cell, glyph procedure or mask group), or anything it
/// draws or paints with, show text or paint an image? Inline images count only when
/// `inline_images` (a Type 3 glyph procedure draws its bitmap that way, and the glyph goes whole
/// when it is under a region). `Err` (why) = something couldn't be read, the nesting is too deep
/// or there is too much of it: nothing is skipped quietly.
fn carries(
    doc: &Document,
    s: &Stream,
    outer: &Dict,
    depth: usize,
    seen: &mut Vec<ObjRef>,
    budget: &mut Budget,
    inline_images: bool,
) -> Result<Option<Painted>, Unsupported> {
    if depth > MAX_DEPTH {
        return Err(Unsupported::PatternUnreadable);
    }
    let data = budget.decode(s).map_err(|why| if why == Refused::OverBudget { Unsupported::TooLarge } else { Unsupported::PatternUnreadable })?;
    let res = s.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_else(|| outer.clone());
    let xobjects = res_dict(doc, &res, b"XObject");
    for op in &parse(&data).ops {
        match op.op.as_slice() {
            b"Tj" | b"TJ" | b"'" | b"\"" => return Ok(Some(Painted::Text)),
            b"BI" if inline_images => return Ok(Some(Painted::Image)),
            b"Do" => {
                let image = op
                    .name(0)
                    .and_then(|n| xobjects.get(n))
                    .is_some_and(|o| matches!(&*doc.resolve(o), Object::Stream(x) if x.dict.name(b"Subtype") == Some(b"Image")));
                if image {
                    return Ok(Some(Painted::Image));
                }
            }
            _ => {}
        }
    }
    for key in [&b"XObject"[..], b"Pattern"] {
        for (_, o) in res_dict(doc, &res, key).iter() {
            let target = o.as_ref();
            if let Some(r) = target {
                if seen.contains(&r) {
                    continue;
                }
                // Too many to follow: unexamined content is not "no content".
                if seen.len() >= MAX_FORMS {
                    return Err(Unsupported::PatternUnreadable);
                }
                seen.push(r);
            }
            let obj = doc.resolve(o);
            let Object::Stream(inner) = &*obj else { continue };
            let is_form = inner.dict.name(b"Subtype") == Some(b"Form");
            let is_tiling = inner.dict.int(b"PatternType") == Some(1);
            if (is_form || is_tiling)
                && let Some(found) = carries(doc, inner, &res, depth + 1, seen, budget, inline_images)?
            {
                return Ok(Some(found));
            }
        }
    }
    Ok(None)
}

/// Look through a resource tree (a page's or a form's, nested forms included) for content that
/// is painted without a `Do` and so can't be edited: tiling patterns, soft-mask groups and Type 3
/// glyph procedures that show text or paint an image. They aren't positioned like page content,
/// so the operation fails rather than leave their text or pixels in the file.
fn scan_resources(doc: &Document, res: &Dict, depth: usize, seen: &mut Vec<ObjRef>, budget: &mut Budget) -> Option<Unsupported> {
    // Forms nested deeper than the interpreter walks are removed whole when they are under a
    // mark, and can't paint anywhere else.
    if depth > MAX_DEPTH {
        return None;
    }
    // A scan that ran out of room says so; any other failure to read is the group's own.
    let unreadable = |why: Unsupported, own: Unsupported| if why == Unsupported::TooLarge { why } else { own };
    for (_, o) in res_dict(doc, res, b"Pattern").iter() {
        if let Object::Stream(p) = &*doc.resolve(o)
            && p.dict.int(b"PatternType") == Some(1)
        {
            match carries(doc, p, res, 0, &mut Vec::new(), budget, true) {
                Ok(None) => {}
                Ok(Some(Painted::Text)) => return Some(Unsupported::PatternText),
                Ok(Some(Painted::Image)) => return Some(Unsupported::HiddenImage),
                Err(why) => return Some(why),
            }
        }
    }
    for (_, o) in res_dict(doc, res, b"ExtGState").iter() {
        let gs = doc.resolve(o);
        let group = gs.as_dict().and_then(|g| g.get(b"SMask")).and_then(|m| doc.resolve(m).as_dict().cloned()).and_then(|m| m.get(b"G").cloned());
        if let Some(g) = group
            && let Object::Stream(s) = &*doc.resolve(&g)
        {
            match carries(doc, s, res, 0, &mut Vec::new(), budget, true) {
                Ok(None) => {}
                Ok(Some(_)) => return Some(Unsupported::SoftMask),
                Err(why) => return Some(unreadable(why, Unsupported::SoftMask)),
            }
        }
    }
    for (_, o) in res_dict(doc, res, b"Font").iter() {
        let f = doc.resolve(o);
        let Some(f) = f.as_dict().filter(|f| f.name(b"Subtype") == Some(b"Type3")) else { continue };
        let procs = f.get(b"CharProcs").and_then(|c| doc.resolve(c).as_dict().cloned()).unwrap_or_default();
        for (_, p) in procs.iter() {
            if let Object::Stream(s) = &*doc.resolve(p) {
                match carries(doc, s, res, 0, &mut Vec::new(), budget, false) {
                    Ok(None) => {}
                    Ok(Some(Painted::Text)) => return Some(Unsupported::Type3Text),
                    Ok(Some(Painted::Image)) => return Some(Unsupported::HiddenImage),
                    Err(why) => return Some(unreadable(why, Unsupported::Type3Text)),
                }
            }
        }
    }
    for (_, o) in res_dict(doc, res, b"XObject").iter() {
        if let Some(r) = o.as_ref() {
            if seen.contains(&r) {
                continue;
            }
            // Too many forms to follow: the rest would go unscanned.
            if seen.len() >= MAX_FORMS {
                return Some(Unsupported::PatternUnreadable);
            }
            seen.push(r);
        }
        if let Object::Stream(f) = &*doc.resolve(o)
            && f.dict.name(b"Subtype") == Some(b"Form")
        {
            let inner = f.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_else(|| res.clone());
            if let Some(why) = scan_resources(doc, &inner, depth + 1, seen, budget) {
                return Some(why);
            }
        }
    }
    None
}

pub(crate) struct Scope<'a> {
    pub rects: &'a [[f64; 4]],
    pub mode: Mode,
    pub report: &'a mut Report,
    /// Sanitize: remove hidden text (invisible render modes, or wholly outside this page box).
    pub hidden_text: Option<[f64; 4]>,
    /// Sanitize: optional content groups (and membership dictionaries) that are off; content
    /// marked with them is removed.
    pub hidden_layers: Vec<ObjRef>,
    /// Content removed from hidden layers (blocks and XObjects).
    pub layer_blocks: usize,
    /// Collects the glyph runs removed in apply mode (`None` = not collected).
    pub removed: Option<Vec<Removed>>,
    fonts: HashMap<Vec<u8>, Rc<FontInfo>>,
    used_names: Vec<Vec<u8>>,
    depth: usize,
    /// Form XObjects expanded so far, and the ones being expanded now (cycles).
    forms: usize,
    active: Vec<ObjRef>,
    /// The first reason content under a mark couldn't be handled.
    failure: Option<Unsupported>,
    /// What this scope (and the forms it expands) may still decode.
    budget: Budget,
}

impl<'a> Scope<'a> {
    pub fn new(rects: &'a [[f64; 4]], mode: Mode, report: &'a mut Report) -> Self {
        Scope {
            rects,
            mode,
            report,
            hidden_text: None,
            hidden_layers: Vec::new(),
            layer_blocks: 0,
            removed: None,
            fonts: HashMap::new(),
            used_names: Vec::new(),
            depth: 0,
            forms: 0,
            active: Vec::new(),
            failure: None,
            budget: Budget::new(MAX_PAGE_TOTAL),
        }
    }

    /// Sanitizing visits every form XObject (not only those under a region).
    fn everywhere(&self) -> bool {
        self.hidden_text.is_some() || !self.hidden_layers.is_empty()
    }

    /// Is this optional-content reference (an OCG or OCMD) hidden?
    fn layer_hidden(&self, doc: &Document, o: &Object) -> bool {
        if self.hidden_layers.is_empty() {
            return false;
        }
        if let Some(r) = o.as_ref()
            && self.hidden_layers.contains(&r)
        {
            return true;
        }
        // A membership dictionary (default policy AnyOn): hidden when all its groups are.
        let d = doc.resolve(o);
        let Some(d) = d.as_dict() else { return false };
        if d.name(b"Type") != Some(b"OCMD") {
            return false;
        }
        let groups: Vec<ObjRef> = match d.get(b"OCGs").map(|g| (*doc.resolve(g)).clone()) {
            Some(Object::Array(a)) => a.iter().filter_map(Object::as_ref).collect(),
            Some(_) => d.get(b"OCGs").and_then(Object::as_ref).into_iter().collect(),
            None => Vec::new(),
        };
        !groups.is_empty() && groups.iter().all(|g| self.hidden_layers.contains(g))
    }

    fn hits(&self, b: [f64; 4]) -> bool {
        self.rects.iter().any(|r| overlaps(*r, b, 0.0))
    }

    fn covered(&self, b: [f64; 4]) -> bool {
        self.rects.iter().any(|r| contains(*r, b, 0.01))
    }

    /// The first region a glyph box counts as under, if any. Any overlap counts (the safe
    /// direction: a glyph that is partly covered goes whole), so only neighbours that merely
    /// touch an edge survive. Degenerate boxes (zero-size text) count when their origin is
    /// inside, and boxes that overflowed to a non-finite number always count (under the first
    /// region).
    fn glyph_rect(&self, b: [f64; 4], origin: (f64, f64)) -> Option<usize> {
        if !b.iter().all(|v| v.is_finite()) || !origin.0.is_finite() || !origin.1.is_finite() {
            return (!self.rects.is_empty()).then_some(0);
        }
        if b[2] - b[0] < 0.01 || b[3] - b[1] < 0.01 {
            return self.rects.iter().position(|r| origin.0 >= r[0] && origin.0 <= r[2] && origin.1 >= r[1] && origin.1 <= r[3]);
        }
        self.rects.iter().position(|r| overlaps(*r, b, EDGE_EPS))
    }

    /// Could text of unknown extent starting here (shown with `trm`'s origin) reach a region?
    /// Without trustworthy advances the run can extend any distance along its writing
    /// direction, so this tests the whole band that direction sweeps: a generous glyph height
    /// for horizontal text, a column two ems wide for vertical.
    fn band_hit(&self, gs: &Gs, tm: &Matrix, why: Unsupported) -> bool {
        let trm = Matrix([gs.size * gs.scale, 0.0, 0.0, gs.size, 0.0, gs.rise]).then(tm).then(&gs.ctm);
        let Some(inv) = trm.invert() else { return !self.rects.is_empty() };
        self.rects.iter().any(|r| {
            let c = inv.bbox(*r);
            if !c.iter().all(|v| v.is_finite()) {
                return true;
            }
            if why == Unsupported::VerticalWriting { c[2] > -1.0 && c[0] < 1.0 } else { c[3] > -0.5 && c[1] < 1.2 }
        })
    }
}

fn res_dict(doc: &Document, res: &Dict, key: &[u8]) -> Dict {
    res.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default()
}

fn fresh_name(scope: &mut Scope<'_>, xobjects: &Dict) -> Vec<u8> {
    let mut i = scope.used_names.len() + 1;
    loop {
        let n = format!("PCRedacted{i}").into_bytes();
        if !xobjects.contains(&n) && !scope.used_names.contains(&n) {
            scope.used_names.push(n.clone());
            return n;
        }
        i += 1;
    }
}

/// The clip that hides everything inside each region: one even-odd clip per region (a big
/// rectangle with the region cut out), in the current user space. Returns `None` when the CTM
/// can't be inverted (nothing is painted then anyway).
fn clip_out(rects: &[[f64; 4]], ctm: &Matrix, around: [f64; 4]) -> Option<Vec<Op>> {
    let inv = ctm.invert()?;
    let mut ops = Vec::new();
    let big = [around[0] - 10.0, around[1] - 10.0, around[2] + 10.0, around[3] + 10.0];
    for r in rects.iter().filter(|r| overlaps(**r, around, 0.0)) {
        let outer = [(big[0], big[1]), (big[2], big[1]), (big[2], big[3]), (big[0], big[3])];
        let inner = [(r[0], r[1]), (r[0], r[3]), (r[2], r[3]), (r[2], r[1])];
        for poly in [outer, inner] {
            for (k, (x, y)) in poly.iter().enumerate() {
                let (u, v) = inv.apply(*x, *y);
                ops.push(Op::new(if k == 0 { "m" } else { "l" }, vec![num(u), num(v)]));
            }
            ops.push(Op::new("h", vec![]));
        }
        ops.push(Op::new("W*", vec![]));
        ops.push(Op::new("n", vec![]));
    }
    Some(ops)
}

/// One glyph of a shown string.
struct Glyph {
    bytes: std::ops::Range<usize>,
    /// Advance in unscaled text space (before `Th`).
    advance: f64,
    hit: bool,
    /// The region it lies under (none for hidden text).
    rect: Option<usize>,
}

/// Process a scope's content streams (state flows from one stream to the next).
pub(crate) fn process(doc: &mut Document, scope: &mut Scope<'_>, streams: &[Vec<u8>], resources: &Dict, ctm: Matrix) -> Output {
    let mut out = Output::default();
    let fallback = Rc::new(FontInfo::unresolved());
    let mut gs =
        Gs { ctm, font: fallback.clone(), size: 0.0, char_spacing: 0.0, word_spacing: 0.0, render_mode: 0, scale: 1.0, leading: 0.0, rise: 0.0 };
    // Text positions that can't be trusted after a doubtful font was shown, until the next
    // operator that sets the line position afresh.
    let mut taint: Option<Unsupported> = None;
    // Open marked-content blocks: what to scrub when something under them is removed.
    let mut blocks: Vec<Block> = Vec::new();
    let mut tally = removals(scope.report);
    if scope.depth == 0 && !scope.rects.is_empty() {
        let found = scan_resources(doc, resources, 0, &mut Vec::new(), &mut scope.budget);
        scope.failure = scope.failure.or(found);
    }
    let mut stack: Vec<Gs> = Vec::new();
    let (mut tm, mut tlm) = (Matrix::IDENTITY, Matrix::IDENTITY);
    let fonts_res = res_dict(doc, resources, b"Font");
    let properties = res_dict(doc, resources, b"Properties");
    // Names of property lists whose alternate text must go (content under them was removed).
    let mut scrubbed: Vec<Vec<u8>> = Vec::new();
    let mut xobjects = res_dict(doc, resources, b"XObject");
    // The current path: its operators, its bounding box in user space, and whether it clips.
    let mut path: Vec<Op> = Vec::new();
    let mut path_box: Option<[f64; 4]> = None;
    let mut clip = false;
    let mut forced_change = false;

    // The streams are one content stream in pieces, which may be split between any two tokens
    // (ISO 32000-2 §7.8.2): an operator's operands can end one piece and the operator start the
    // next. Parse them joined, then give each operator back to the piece its keyword is in.
    let joined = Pieces::join(streams);
    let mut parsed = joined.parse();
    // The parser is tolerant: bytes it can't tokenize are skipped (an unterminated string swallows
    // the whole rest, hiding real operators from this model while another reader may still show
    // them). What was skipped can be neither rewritten nor reasoned about, so refuse the scope.
    if parsed.skipped > 0 {
        scope.failure = scope.failure.or(Some(Unsupported::UnparsedContent));
    }
    if !scope.hidden_layers.is_empty() {
        let (kept, removed) = strip_hidden_layers(doc, scope, parsed.ops, &properties);
        parsed.ops = kept;
        if removed > 0 {
            scope.layer_blocks += removed;
            forced_change = true;
        }
    }
    if !scope.rects.is_empty() && inline_image_unclear(&parsed, joined.data()) {
        scope.failure = scope.failure.or(Some(Unsupported::InlineImage));
    }
    let mut pieces: Vec<Vec<Op>> = streams.iter().map(|_| Vec::new()).collect();
    let mut rewrite: Vec<bool> = vec![forced_change; streams.len()];
    for op in parsed.ops {
        let (first, last) = joined.pieces_of(&op);
        // An operator split across pieces is written whole into its keyword's piece, so every
        // piece it spans is rewritten.
        if first < last {
            for r in rewrite.iter_mut().take(last + 1).skip(first) {
                *r = true;
            }
        }
        if let Some(p) = pieces.get_mut(last) {
            p.push(op);
        }
    }

    // Each piece's rewritten operators and its changed flag. Serialization waits until every
    // piece has been walked: a marked-content block can open in one piece and be tainted by a
    // removal in a later one, which reaches back into the earlier piece's operators.
    let mut rewritten: Vec<Vec<Op>> = vec![Vec::new(); streams.len()];
    let mut changed: Vec<bool> = rewrite;
    // Marked-content blocks already closed by their `EMC`, whose scrubs may still be pending.
    let mut closed: Vec<Block> = Vec::new();
    for (no, piece) in pieces.into_iter().enumerate() {
        let Some(ops) = rewritten.get_mut(no) else { continue };
        let Some(changed) = changed.get_mut(no) else { continue };
        for op in piece {
            // Whatever the previous operator removed taints the blocks it sat in.
            let now = removals(scope.report);
            if now != tally {
                tally = now;
                blocks.iter_mut().for_each(|b| b.removed = true);
            }
            let o = op.op.as_slice();
            // Path construction.
            if matches!(o, b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re" | b"W" | b"W*") {
                let pts: Vec<(f64, f64)> = match o {
                    b"re" => op.nums::<4>().map(|[x, y, w, h]| vec![(x, y), (x + w, y), (x, y + h), (x + w, y + h)]).unwrap_or_default(),
                    b"h" | b"W" | b"W*" => Vec::new(),
                    _ => op.operands.iter().filter_map(Object::as_f64).collect::<Vec<_>>().as_chunks::<2>().0.iter().map(|p| (p[0], p[1])).collect(),
                };
                for (x, y) in pts {
                    let (u, v) = gs.ctm.apply(x, y);
                    path_box = Some(match path_box {
                        None => [u, v, u, v],
                        Some(b) => [b[0].min(u), b[1].min(v), b[2].max(u), b[3].max(v)],
                    });
                }
                clip |= matches!(o, b"W" | b"W*");
                path.push(op);
                continue;
            }
            if matches!(o, b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n") {
                let body = std::mem::take(&mut path);
                let bbox = path_box.take();
                let clips = std::mem::replace(&mut clip, false);
                let hit = scope.mode == Mode::Apply && o != b"n" && bbox.is_some_and(|b| scope.hits(b));
                if !hit {
                    ops.extend(body);
                    ops.push(op);
                    continue;
                }
                let b = bbox.unwrap_or_default();
                *changed = true;
                if scope.covered(b) {
                    scope.report.paths_removed += 1;
                    if clips {
                        // Keep the clipping, drop the painting.
                        ops.extend(body);
                        ops.push(Op::new("n", vec![]));
                    }
                } else if clips {
                    // A clip-and-paint path can't be wrapped in q/Q without losing its clip:
                    // paint it clipped, then set its clip with a no-op path.
                    scope.report.paths_clipped += 1;
                    if let Some(c) = clip_out(scope.rects, &gs.ctm, b) {
                        let plain: Vec<Op> = body.iter().filter(|p| !p.is("W") && !p.is("W*")).cloned().collect();
                        ops.push(Op::new("q", vec![]));
                        ops.extend(c);
                        ops.extend(plain);
                        ops.push(op);
                        ops.push(Op::new("Q", vec![]));
                    }
                    ops.extend(body);
                    ops.push(Op::new("n", vec![]));
                } else {
                    scope.report.paths_clipped += 1;
                    if let Some(c) = clip_out(scope.rects, &gs.ctm, b) {
                        ops.push(Op::new("q", vec![]));
                        ops.extend(c);
                        ops.extend(body);
                        ops.push(op);
                        ops.push(Op::new("Q", vec![]));
                    }
                }
                continue;
            }
            if !path.is_empty() {
                // A path not ended by a painting operator: pass it through.
                ops.append(&mut path);
                path_box = None;
                clip = false;
            }
            match o {
                b"q" => stack.push(gs.clone()),
                b"Q" => {
                    if let Some(g) = stack.pop() {
                        gs = g;
                    }
                }
                b"cm" => {
                    if let Some(m) = op.nums::<6>() {
                        gs.ctm = Matrix(m).then(&gs.ctm);
                    }
                }
                b"BT" => {
                    tm = Matrix::IDENTITY;
                    tlm = Matrix::IDENTITY;
                    taint = None;
                }
                b"Tf" => {
                    gs.size = op.num(1).filter(|v| v.is_finite()).unwrap_or(gs.size);
                    if let Some(name) = op.name(0) {
                        gs.font = match scope.fonts.get(name) {
                            Some(f) => f.clone(),
                            None => {
                                let m = fonts_res
                                    .get(name)
                                    .and_then(|f| doc.resolve(f).as_dict().cloned())
                                    .map(|d| Rc::new(font_info(doc, &d)))
                                    .unwrap_or_else(|| fallback.clone());
                                scope.fonts.insert(name.to_vec(), m.clone());
                                m
                            }
                        };
                    }
                }
                b"Tc" => gs.char_spacing = op.num(0).unwrap_or(0.0),
                b"Tw" => gs.word_spacing = op.num(0).unwrap_or(0.0),
                b"Tz" => gs.scale = op.num(0).filter(|v| v.is_finite()).unwrap_or(100.0) / 100.0,
                b"TL" => gs.leading = op.num(0).unwrap_or(0.0),
                b"Ts" => gs.rise = op.num(0).unwrap_or(0.0),
                b"Tr" => gs.render_mode = op.num(0).unwrap_or(0.0) as i64,
                b"Td" | b"TD" => {
                    if let Some([x, y]) = op.nums::<2>() {
                        if o == b"TD" {
                            gs.leading = -y;
                        }
                        tlm = Matrix::translate(x, y).then(&tlm);
                        tm = tlm;
                        taint = None;
                    }
                }
                b"Tm" => {
                    if let Some(m) = op.nums::<6>() {
                        tlm = Matrix(m);
                        tm = tlm;
                        taint = None;
                    }
                }
                b"T*" => {
                    tlm = Matrix::translate(0.0, -gs.leading).then(&tlm);
                    tm = tlm;
                    taint = None;
                }
                b"Tj" | b"TJ" | b"'" | b"\"" => {
                    if o == b"\"" {
                        gs.word_spacing = op.num(0).unwrap_or(gs.word_spacing);
                        gs.char_spacing = op.num(1).unwrap_or(gs.char_spacing);
                    }
                    if matches!(o, b"'" | b"\"") {
                        tlm = Matrix::translate(0.0, -gs.leading).then(&tlm);
                        tm = tlm;
                        taint = None;
                    }
                    let items: Vec<Object> = match o {
                        b"TJ" => op.operands.first().and_then(Object::as_array).cloned().unwrap_or_default(),
                        _ => op.operands.last().cloned().into_iter().collect(),
                    };
                    let start = tm;
                    let shown = show(scope, &gs, &mut tm, &items);
                    for why in shown.doubt.into_iter().chain(taint) {
                        if scope.band_hit(&gs, &start, why) {
                            scope.failure = scope.failure.or(Some(why));
                        }
                    }
                    taint = shown.doubt.or(taint);
                    let (new_items, removed) = (shown.items, shown.removed);
                    if removed == 0 {
                        ops.push(op);
                        continue;
                    }
                    if scope.mode == Mode::Verify {
                        out.residue += removed;
                        ops.push(op);
                        continue;
                    }
                    *changed = true;
                    scope.report.glyphs += removed;
                    match o {
                        b"\"" => {
                            ops.push(Op::new("Tw", vec![num(gs.word_spacing)]));
                            ops.push(Op::new("Tc", vec![num(gs.char_spacing)]));
                            ops.push(Op::new("T*", vec![]));
                        }
                        b"'" => ops.push(Op::new("T*", vec![])),
                        _ => {}
                    }
                    ops.push(Op::new("TJ", vec![Object::Array(new_items)]));
                    continue;
                }
                b"BI" => {
                    let b = gs.ctm.bbox([0.0, 0.0, 1.0, 1.0]);
                    if scope.hits(b) {
                        if scope.mode == Mode::Verify {
                            out.residue += 1;
                        } else {
                            scope.report.images_removed += 1;
                            *changed = true;
                            continue;
                        }
                    }
                }
                b"sh" if scope.mode == Mode::Apply => {
                    // A shading fills the current clip: clip the regions out.
                    if let Some(c) = clip_out(scope.rects, &gs.ctm, [-1e6, -1e6, 1e6, 1e6]) {
                        *changed = true;
                        ops.push(Op::new("q", vec![]));
                        ops.extend(c);
                        ops.push(op);
                        ops.push(Op::new("Q", vec![]));
                        continue;
                    }
                }
                b"BMC" | b"BDC" => {
                    let mut block = Block { scrub: Vec::new(), names: Vec::new(), removed: false };
                    if o == b"BDC" {
                        block.note(doc, &properties, &op, no, ops.len());
                    }
                    blocks.push(block);
                }
                b"DP" => {
                    if let Some(b) = blocks.last_mut() {
                        b.note(doc, &properties, &op, no, ops.len());
                    }
                }
                b"EMC" => {
                    // The block keeps its recorded entries: a dictionary of it may sit in an
                    // earlier piece, and what was removed under the block is only known once the
                    // whole content has been walked.
                    if let Some(b) = blocks.pop() {
                        closed.push(b);
                    }
                }
                b"Do" => {
                    if let Some(new) = xobject(doc, scope, &op, &gs, resources, &mut xobjects, &mut out) {
                        *changed = true;
                        if let Some(n) = new {
                            ops.push(n);
                        }
                        continue;
                    }
                }
                _ => {}
            }
            ops.push(op);
        }
        if !path.is_empty() {
            ops.append(&mut path);
        }
    }
    // Whatever was removed up to the end of the content taints the blocks still open.
    let now = removals(scope.report);
    if now != tally {
        blocks.iter_mut().for_each(|b| b.removed = true);
    }
    // Scrub every tainted block now, closed or still open: its dictionary may sit in a piece
    // before the one whose removal tainted it, and must not keep the removed text.
    for b in closed.iter_mut().chain(blocks.iter_mut()) {
        if !b.removed {
            continue;
        }
        for (piece, at) in &b.scrub {
            if let Some(ops) = rewritten.get_mut(*piece) {
                scrub(ops, &[*at]);
            }
            if let Some(flag) = changed.get_mut(*piece) {
                *flag = true;
            }
        }
        scrubbed.append(&mut b.names);
    }
    for (changed, ops) in changed.into_iter().zip(rewritten) {
        out.streams.push(changed.then(|| serialize_ops(&ops)));
    }
    scrubbed.sort();
    scrubbed.dedup();
    for name in scrubbed {
        if let Some(Object::Dict(list)) = properties.get(&name).map(|o| (*doc.resolve(o)).clone()) {
            let mut list = list;
            ALT_KEYS.iter().for_each(|k| {
                list.remove(k);
            });
            out.properties.push((name, list));
        }
    }
    out.failure = scope.failure;
    out
}

/// A marked-content block that is open: the operators whose inline property dictionaries hold
/// alternate text, as (piece, index into that piece's rewritten operators) — a block can span
/// pieces, so its dictionaries may sit in a different piece than the removal that taints it —
/// the resource names of property lists that do, and whether content under it has been removed.
struct Block {
    scrub: Vec<(usize, usize)>,
    names: Vec<Vec<u8>>,
    removed: bool,
}

impl Block {
    /// Remember the property list of a `BDC`/`DP` (at `index` in piece `no`'s rewritten
    /// operators) when it holds alternate text, whether written inline or named in the
    /// `/Properties` resources.
    fn note(&mut self, doc: &Document, properties: &Dict, op: &Op, no: usize, index: usize) {
        match op.operands.get(1) {
            Some(Object::Dict(d)) if ALT_KEYS.iter().any(|k| d.contains(k)) => self.scrub.push((no, index)),
            Some(Object::Name(n)) => {
                let list = properties.get(n).map(|o| doc.resolve(o));
                if list.as_deref().and_then(Object::as_dict).is_some_and(|d| ALT_KEYS.iter().any(|k| d.contains(k))) {
                    self.names.push(n.clone());
                }
            }
            _ => {}
        }
    }
}

/// Everything redaction has removed or changed so far (a change shows an operator did something).
fn removals(r: &Report) -> usize {
    r.glyphs + r.images_removed + r.images_cleared + r.paths_removed + r.paths_clipped + r.forms_rewritten + r.forms_removed
}

/// Property keys that repeat the text of the content they mark.
const ALT_KEYS: [&[u8]; 3] = [b"ActualText", b"Alt", b"E"];

/// Drop the alternate text from the inline property lists of the operators at `at`.
fn scrub(ops: &mut [Op], at: &[usize]) {
    for i in at {
        if let Some(Object::Dict(d)) = ops.get_mut(*i).and_then(|op| op.operands.get_mut(1)) {
            ALT_KEYS.iter().for_each(|k| {
                d.remove(k);
            });
        }
    }
}

/// Is there an inline image in this stream whose end can't be established? Its data is binary and
/// the parser finds the end by looking for `EI`, which can occur inside the data. Without a
/// filter the exact length is known; with one, junk after the supposed end gives it away.
fn inline_image_unclear(parsed: &pdfcraft_content::Parsed, data: &[u8]) -> bool {
    let key = |d: &Dict, short: &[u8], long: &[u8]| d.get(short).or_else(|| d.get(long)).cloned();
    let mut any = false;
    for op in &parsed.ops {
        let Some((dict, img)) = &op.inline else { continue };
        any = true;
        // The stream ended inside the image.
        let tail = data.get(op.span.clone()).unwrap_or_default();
        if !tail.trim_ascii_end().ends_with(b"EI") {
            return true;
        }
        if key(dict, b"F", b"Filter").is_some() {
            continue;
        }
        let w = key(dict, b"W", b"Width").and_then(|v| v.as_int()).and_then(|v| u64::try_from(v).ok());
        let h = key(dict, b"H", b"Height").and_then(|v| v.as_int()).and_then(|v| u64::try_from(v).ok());
        let stencil = matches!(key(dict, b"IM", b"ImageMask"), Some(Object::Bool(true)));
        let bpc = if stencil { Some(1) } else { key(dict, b"BPC", b"BitsPerComponent").and_then(|v| v.as_int()).and_then(|v| u64::try_from(v).ok()) };
        let comps = if stencil {
            Some(1)
        } else {
            match key(dict, b"CS", b"ColorSpace") {
                Some(Object::Name(n)) => match n.as_slice() {
                    b"G" | b"DeviceGray" | b"I" | b"Indexed" => Some(1),
                    b"RGB" | b"DeviceRGB" => Some(3),
                    b"CMYK" | b"DeviceCMYK" => Some(4),
                    _ => None,
                },
                Some(Object::Array(a)) if matches!(a.first().and_then(Object::as_name), Some(b"I" | b"Indexed")) => Some(1),
                _ => None,
            }
        };
        let expected = match (w, h, bpc, comps) {
            (Some(w), Some(h), Some(b), Some(c)) => {
                w.checked_mul(c).and_then(|v| v.checked_mul(b)).map(|bits| bits.div_ceil(8)).and_then(|row| row.checked_mul(h))
            }
            _ => None,
        };
        // A producer may leave out the whitespace before `EI` (or add a byte); more than that
        // means the data was cut short or run on.
        match expected {
            Some(e) if e.abs_diff(img.len() as u64) <= 2 => {}
            _ => return true,
        }
    }
    any && parsed.skipped > 0
}

/// A laid-out string: the `TJ` items with the glyphs under a region replaced by displacements,
/// how many glyphs were removed, and whether the font gave the layout no firm footing.
struct Shown {
    items: Vec<Object>,
    removed: usize,
    doubt: Option<Unsupported>,
}

/// Lay out a shown string (or `TJ` array) glyph by glyph, advancing `tm`. In apply mode the
/// runs removed under each region are also handed to `scope.removed`.
fn show(scope: &mut Scope<'_>, gs: &Gs, tm: &mut Matrix, items: &[Object]) -> Shown {
    let f = &gs.font;
    let m = &f.metrics;
    let size = gs.size;
    let mut out: Vec<Object> = Vec::new();
    let mut removed = 0;
    let mut doubt = f.doubt;
    let push_num = |out: &mut Vec<Object>, v: f64| {
        if v == 0.0 {
            return;
        }
        if let Some(last) = out.last_mut()
            && let Some(prev) = last.as_f64()
        {
            *last = num(prev + v);
            return;
        }
        out.push(num(v));
    };
    for item in items {
        match item {
            Object::String(s) => {
                let bytes = &s.bytes;
                let mut glyphs = Vec::new();
                let mut pos = 0;
                for (code, len) in m.codes(bytes) {
                    let w0 = m.width(code);
                    doubt = doubt.or(f.certain(code));
                    let spacing = gs.char_spacing + if m.is_space(code, len) { gs.word_spacing } else { 0.0 };
                    let trm = Matrix([size * gs.scale, 0.0, 0.0, size, 0.0, gs.rise]).then(tm).then(&gs.ctm);
                    let glyph = match f.type3_box {
                        Some(bb) => [bb[0].min(0.0), bb[1], bb[2].max(w0), bb[3]],
                        None => [0.0, m.descent, w0, m.ascent],
                    };
                    let b = trm.bbox(glyph);
                    let origin = trm.apply(0.0, 0.0);
                    let advance = w0 * size + spacing;
                    let mut rect = None;
                    let hit = match scope.hidden_text {
                        // Invisible (Tr 3) and clip-only (Tr 7) text, or text wholly off the page.
                        Some(page) => {
                            let degenerate = b[2] - b[0] < 0.01 && b[3] - b[1] < 0.01;
                            matches!(gs.render_mode, 3 | 7) || !(overlaps(page, b, 0.0) || degenerate)
                        }
                        None => {
                            rect = scope.glyph_rect(b, origin);
                            rect.is_some()
                        }
                    };
                    glyphs.push(Glyph { bytes: pos..pos + len, advance, hit, rect });
                    *tm = Matrix::translate(advance * gs.scale, 0.0).then(tm);
                    pos += len;
                }
                if scope.mode == Mode::Apply
                    && let Some(sink) = scope.removed.as_mut()
                {
                    // Consecutive removed glyphs under one region are one run.
                    let mut k = 0;
                    while let Some(g) = glyphs.get(k) {
                        let Some(rect) = g.rect.filter(|_| g.hit) else {
                            k += 1;
                            continue;
                        };
                        let mut end = k;
                        while glyphs.get(end + 1).is_some_and(|n| n.hit && n.rect == Some(rect)) {
                            end += 1;
                        }
                        let span = g.bytes.start..glyphs.get(end).map_or(g.bytes.end, |l| l.bytes.end);
                        let raw = bytes.get(span).unwrap_or_default().to_vec();
                        sink.push(Removed { rect, text: m.decode(&raw), raw });
                        k = end + 1;
                    }
                }
                let mut run: Vec<u8> = Vec::new();
                for g in &glyphs {
                    if g.hit {
                        removed += 1;
                        if !run.is_empty() {
                            out.push(string(std::mem::take(&mut run)));
                        }
                        // A TJ number moves by -n/1000 × size (× Th): the removed advance.
                        if size != 0.0 {
                            push_num(&mut out, -g.advance * 1000.0 / size);
                        }
                    } else {
                        run.extend_from_slice(bytes.get(g.bytes.clone()).unwrap_or_default());
                    }
                }
                if !run.is_empty() {
                    out.push(string(run));
                }
            }
            other => {
                if let Some(v) = other.as_f64() {
                    *tm = Matrix::translate(-v / 1000.0 * size * gs.scale, 0.0).then(tm);
                    push_num(&mut out, v);
                }
            }
        }
    }
    Shown { items: out, removed, doubt }
}

/// `Do`: `None` = leave the operator alone; `Some(None)` = drop it; `Some(Some(op))` = replace it.
fn xobject(
    doc: &mut Document,
    scope: &mut Scope<'_>,
    op: &Op,
    gs: &Gs,
    resources: &Dict,
    xobjects: &mut Dict,
    out: &mut Output,
) -> Option<Option<Op>> {
    let name = op.name(0)?.to_vec();
    let r = xobjects.get(&name)?.as_ref()?;
    let obj = doc.get(r);
    let Object::Stream(s) = &*obj else { return None };
    match s.dict.name(b"Subtype") {
        _ if s.dict.get(b"OC").is_some_and(|oc| scope.layer_hidden(doc, oc)) => {
            if scope.mode == Mode::Verify {
                return None;
            }
            scope.layer_blocks += 1;
            Some(None)
        }
        Some(b"Image") => image_xobject(doc, scope, s, gs, xobjects, out),
        Some(b"Form") => form_xobject(doc, scope, (s, r), gs, resources, xobjects, out),
        _ => None,
    }
}

/// An image drawn by `Do`: removed when a region covers it, replaced by a copy with the covered
/// pixels cleared when it only overlaps one. `None` leaves the operator alone.
fn image_xobject(doc: &mut Document, scope: &mut Scope<'_>, s: &Stream, gs: &Gs, xobjects: &mut Dict, out: &mut Output) -> Option<Option<Op>> {
    let b = gs.ctm.bbox([0.0, 0.0, 1.0, 1.0]);
    if !scope.hits(b) || scope.mode == Mode::Verify {
        return None;
    }
    if scope.covered(b) {
        scope.report.images_removed += 1;
        return Some(None);
    }
    match image::clear(doc, s, &gs.ctm, scope.rects) {
        Ok(None) => None,
        Ok(Some(new)) => {
            let nr = doc.add(Object::Stream(new));
            let n = fresh_name(scope, xobjects);
            xobjects.set(n.clone(), Object::Ref(nr));
            out.xobjects.push((n.clone(), nr));
            scope.report.images_cleared += 1;
            Some(Some(Op::new("Do", vec![Object::Name(n)])))
        }
        Err(()) => {
            scope.report.images_removed += 1;
            Some(None)
        }
    }
}

/// A form drawn by `Do` (`form`: the stream and its reference): walked, and replaced by a
/// rewritten copy when its content changed, or removed whole when it can't be walked. `None`
/// leaves the operator alone.
fn form_xobject(
    doc: &mut Document,
    scope: &mut Scope<'_>,
    form: (&Stream, ObjRef),
    gs: &Gs,
    resources: &Dict,
    xobjects: &mut Dict,
    out: &mut Output,
) -> Option<Option<Op>> {
    let (s, r) = form;
    // A matrix that is there but unreadable would place the content somewhere unknown.
    let m = match s.dict.get(b"Matrix") {
        None | Some(Object::Null) => Matrix::default(),
        Some(m) => match strict_nums(doc, Some(m)).filter(|v| v.len() == 6) {
            Some(v) => Matrix([v[0], v[1], v[2], v[3], v[4], v[5]]),
            // (Sanitizing without regions only looks for hidden content and goes on.)
            None if scope.rects.is_empty() => Matrix::default(),
            None => {
                scope.failure = scope.failure.or(Some(Unsupported::FormMatrix));
                return None;
            }
        },
    };
    let fctm = m.then(&gs.ctm);
    // A missing or malformed /BBox gives no bounds to test, so the form is walked whole.
    let bbox =
        strict_nums(doc, s.dict.get(b"BBox")).filter(|v| v.len() == 4).map(|v| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])]);
    if let Some(bb) = bbox
        && !scope.everywhere()
        && !scope.hits(fctm.bbox(bb))
    {
        return None;
    }
    // Too deep, too many, or referring back to itself: can't be walked, so it goes whole.
    scope.forms += 1;
    if scope.depth >= MAX_DEPTH || scope.forms > MAX_FORMS || scope.active.contains(&r) {
        if scope.mode == Mode::Verify {
            out.residue += 1;
            return None;
        }
        scope.report.forms_removed += 1;
        return Some(None);
    }
    let data = match scope.budget.decode(s) {
        Ok(d) => d,
        // Out of budget: the form can't be walked, and removing every later one quietly
        // would be wrong too, so the operation stops.
        Err(Refused::OverBudget) => {
            scope.failure = scope.failure.or(Some(Unsupported::TooLarge));
            return None;
        }
        // Damaged or too long to examine in full: it goes whole (apply), and left alone
        // it would count as something still there (verify).
        Err(Refused::Unreadable) if scope.mode == Mode::Apply => {
            scope.report.forms_removed += 1;
            return Some(None);
        }
        Err(Refused::Unreadable) => {
            out.residue += 1;
            return None;
        }
    };
    let own = s.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned());
    let res = own.clone().unwrap_or_else(|| resources.clone());
    let saved_fonts = std::mem::take(&mut scope.fonts);
    scope.depth += 1;
    scope.active.push(r);
    let before = [data];
    let inner = process(doc, scope, &before, &res, fctm);
    scope.active.pop();
    scope.depth -= 1;
    scope.fonts = saved_fonts;
    out.residue += inner.residue;
    if scope.mode == Mode::Verify {
        return None;
    }
    let new_data = inner.streams.into_iter().next().flatten();
    if new_data.is_none() && inner.xobjects.is_empty() && inner.properties.is_empty() {
        return None;
    }
    let mut dict = s.dict.clone();
    dict.remove(b"Length");
    // Names the form's old content drew and the new one doesn't: they leave the form's resources
    // (its own, or the copy this rewrite freezes out of the resources it inherited), so the
    // original content is not kept there. Names a nested form retired against resources it
    // inherited from this form leave the frozen copy too.
    let mut gone = match (&new_data, before.first()) {
        (Some(after), Some(was)) => tags::retired_names(was, after),
        _ => Vec::new(),
    };
    gone.extend(inner.inherited_gone.iter().cloned());
    if !gone.is_empty() {
        gone.sort_unstable();
        gone.dedup();
    }
    // Every retired name is reported up, whatever resources the form drew with: with its own
    // /Resources the rewrite froze a clean copy, but the page's own /XObject — or a page-tree
    // node — may still map the same name to the very same object, and the original would stay
    // reachable in every full save. The callers retire only what no page draws any more, so a
    // sibling that still draws the name keeps it, and the object with it.
    out.inherited_gone.extend(gone.iter().cloned());
    if !inner.xobjects.is_empty() || !gone.is_empty() || !inner.properties.is_empty() {
        let mut res = res;
        let mut xo = res_dict(doc, &res, b"XObject");
        for n in &gone {
            xo.remove(n);
        }
        if own.is_none() {
            // The frozen copy must not keep drawing this very form: the scope that drew it did
            // so by a name that sits in these resources.
            let self_drawn: Vec<Vec<u8>> = xo.iter().filter(|(_, v)| v.as_ref() == Some(r)).map(|(k, _)| k.clone()).collect();
            for n in &self_drawn {
                xo.remove(n);
            }
        }
        for (n, r) in &inner.xobjects {
            xo.set(n.clone(), Object::Ref(*r));
        }
        res.set(b"XObject".to_vec(), Object::Dict(xo));
        if !inner.properties.is_empty() {
            let mut props = res_dict(doc, &res, b"Properties");
            for (n, list) in &inner.properties {
                props.set(n.clone(), Object::Dict(list.clone()));
            }
            res.set(b"Properties".to_vec(), Object::Dict(props));
        }
        dict.set(b"Resources".to_vec(), Object::Dict(res));
    }
    let stream = match new_data {
        Some(d) => Stream::flate(dict, &d),
        None => Stream { dict, raw: s.raw.clone() },
    };
    let nr = doc.add(Object::Stream(stream));
    let n = fresh_name(scope, xobjects);
    xobjects.set(n.clone(), Object::Ref(nr));
    out.xobjects.push((n.clone(), nr));
    scope.report.forms_rewritten += 1;
    Some(Some(Op::new("Do", vec![Object::Name(n)])))
}

/// Remove marked-content blocks of hidden layers (`/OC /name BDC … EMC`, nested blocks
/// included). Returns the remaining operators and how many blocks went.
fn strip_hidden_layers(doc: &Document, scope: &Scope<'_>, ops: Vec<Op>, props: &Dict) -> (Vec<Op>, usize) {
    let mut out = Vec::with_capacity(ops.len());
    // For each open marked-content block: does it hide its contents?
    let mut stack: Vec<bool> = Vec::new();
    let mut removed = 0;
    for op in ops {
        let hidden_now = stack.last().copied().unwrap_or(false);
        match op.op.as_slice() {
            b"BDC" | b"BMC" => {
                let this = op.is("BDC")
                    && op.name(0) == Some(b"OC")
                    && match op.operands.get(1) {
                        Some(Object::Name(n)) => props.get(n).is_some_and(|o| scope.layer_hidden(doc, o)),
                        Some(o) => scope.layer_hidden(doc, o),
                        None => false,
                    };
                if this && !hidden_now {
                    removed += 1;
                }
                stack.push(hidden_now || this);
                if !(hidden_now || this) {
                    out.push(op);
                }
            }
            b"EMC" => {
                let was = stack.pop().unwrap_or(false);
                if !was {
                    out.push(op);
                }
            }
            _ => {
                if !hidden_now {
                    out.push(op);
                }
            }
        }
    }
    (out, removed)
}
