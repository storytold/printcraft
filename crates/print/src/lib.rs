//! Printing (execution plan M10.5). Layer L4.
//!
//! [`impose`] turns the pages to print into a print-ready PDF of *sheets*, laid out the way
//! Acrobat's Print dialog describes them:
//! - **Size**: one page per sheet, Fit, Actual size, Shrink oversized pages or a custom scale,
//!   centred;
//! - **Multiple**: pages per sheet in a grid (2, 4, 6, 9, 16 or custom), four page orders,
//!   optional page borders and auto-rotation;
//! - **Booklet**: saddle-stitched spreads (both sides, front or back only; left or right
//!   binding);
//! - **Poster**: each page enlarged across several sheets with overlap and cut marks;
//! - orientation (auto, portrait, landscape) and what to print (the document, with markups,
//!   with stamps, or form fields only; annotations print only with their Print flag).
//!
//! The sheets are written as a new, unencrypted, garbage-collected PDF; [`spool`] hands it to
//! the system's print spooler (CUPS on macOS and Linux).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_content::Matrix;
use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};

pub mod range;
pub mod spool;
#[cfg(test)]
mod tests;

pub use range::{Subset, select_listed, select_pages};

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum PrintError {
    #[error("there are no pages to print")]
    NoPages,
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Spool(String),
    #[error(transparent)]
    Cos(#[from] pdfcraft_cos::CosError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Orientation {
    #[default]
    Auto,
    Portrait,
    Landscape,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum SizeMode {
    #[default]
    Fit,
    Actual,
    Shrink,
    /// Percent.
    Custom(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PageOrder {
    #[default]
    Horizontal,
    HorizontalReversed,
    Vertical,
    VerticalReversed,
    /// Single-sided sheets: cut into cell piles, then stack them in row-major order.
    CutStack,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BookletSubset {
    #[default]
    BothSides,
    FrontOnly,
    BackOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Binding {
    #[default]
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Layout {
    Size(SizeMode),
    Multiple {
        cols: usize,
        rows: usize,
        order: PageOrder,
        border: bool,
        auto_rotate: bool,
    },
    Booklet {
        subset: BookletSubset,
        binding: Binding,
    },
    /// Tile scale (percent), overlap (points), cut marks.
    Poster {
        scale: f64,
        overlap: f64,
        cut_marks: bool,
    },
}

impl Default for Layout {
    fn default() -> Self {
        Layout::Size(SizeMode::Fit)
    }
}

impl Layout {
    /// Acrobat's pages-per-sheet choices as a grid (columns × rows on a portrait sheet).
    pub fn multiple(per_sheet: usize) -> Layout {
        let (cols, rows) = match per_sheet {
            2 => (1, 2),
            4 => (2, 2),
            6 => (2, 3),
            9 => (3, 3),
            16 => (4, 4),
            n => {
                let c = (n as f64).sqrt().ceil().max(1.0) as usize;
                (c, n.div_ceil(c).max(1))
            }
        };
        Layout::Multiple { cols, rows, order: PageOrder::Horizontal, border: false, auto_rotate: true }
    }
}

/// Acrobat's Comments & Forms choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Content {
    /// Page content and form fields.
    Document,
    /// Page content, form fields and every printable comment.
    #[default]
    DocumentAndMarkups,
    /// Page content, form fields and stamps.
    DocumentAndStamps,
    /// Form fields only.
    FormFieldsOnly,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Source pages (0-based) in print order; see [`select_pages`].
    pub pages: Vec<usize>,
    /// Paper (width, height) in points, portrait.
    pub paper: (f64, f64),
    pub orientation: Orientation,
    pub layout: Layout,
    pub content: Content,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { pages: Vec::new(), paper: PAPERS[0].1, orientation: Orientation::Auto, layout: Layout::default(), content: Content::default() }
    }
}

/// Common paper sizes (points, portrait).
pub const PAPERS: [(&str, (f64, f64)); 6] = [
    ("US Letter", (612.0, 792.0)),
    ("US Legal", (612.0, 1008.0)),
    ("Tabloid", (792.0, 1224.0)),
    ("A3", (841.89, 1190.55)),
    ("A4", (595.28, 841.89)),
    ("A5", (419.53, 595.28)),
];

/// The unprintable margin assumed around the sheet for Fit, Multiple and Booklet.
pub const MARGIN: f64 = 18.0;

/// One page as placed on a sheet: the source page, the matrix from its *display space* to the
/// sheet, and the visible part of it in display space (clipping).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub page: usize,
    pub matrix: Matrix,
    pub clip: [f64; 4],
}

/// One output sheet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sheet {
    pub size: (f64, f64),
    pub placed: Vec<Placement>,
    /// Page borders (Multiple) and cut marks (Poster), in sheet space: rectangles and lines.
    pub borders: Vec<[f64; 4]>,
    pub lines: Vec<[f64; 4]>,
}

fn oriented(paper: (f64, f64), o: Orientation, landscape_wanted: bool) -> (f64, f64) {
    let (w, h) = (paper.0.min(paper.1), paper.0.max(paper.1));
    match o {
        Orientation::Portrait => (w, h),
        Orientation::Landscape => (h, w),
        Orientation::Auto => {
            if landscape_wanted {
                (h, w)
            } else {
                (w, h)
            }
        }
    }
}

/// Place a page of display size `(dw, dh)` inside `cell` (sheet space), scaled by `s`
/// (centred), optionally rotated 90° (counter-clockwise).
fn place_in(page: usize, (dw, dh): (f64, f64), cell: [f64; 4], s: f64, rotate: bool) -> Placement {
    let (cw, ch) = (cell[2] - cell[0], cell[3] - cell[1]);
    let (pw, ph) = if rotate { (dh * s, dw * s) } else { (dw * s, dh * s) };
    let (x, y) = (cell[0] + (cw - pw) / 2.0, cell[1] + (ch - ph) / 2.0);
    let matrix = if rotate { Matrix([0.0, s, -s, 0.0, x + pw, y]) } else { Matrix([s, 0.0, 0.0, s, x, y]) };
    Placement { page, matrix, clip: [0.0, 0.0, dw, dh] }
}

fn fit_scale((dw, dh): (f64, f64), (cw, ch): (f64, f64)) -> f64 {
    if dw <= 0.0 || dh <= 0.0 { 1.0 } else { (cw / dw).min(ch / dh) }
}

/// Lay out the sheets for `settings` given each source page's display size.
pub fn layout(sizes: &[(f64, f64)], settings: &Settings) -> Result<Vec<Sheet>, PrintError> {
    let pages: Vec<usize> = settings.pages.iter().copied().filter(|p| *p < sizes.len()).collect();
    if pages.is_empty() {
        return Err(PrintError::NoPages);
    }
    let paper = settings.paper;
    if !(paper.0 >= 72.0 && paper.1 >= 72.0 && paper.0.is_finite() && paper.1.is_finite()) {
        return Err(PrintError::Invalid("invalid paper size".into()));
    }
    let mut sheets = Vec::new();
    match settings.layout {
        Layout::Size(mode) => {
            for &p in &pages {
                let d = sizes[p];
                let size = oriented(paper, settings.orientation, d.0 > d.1);
                let fit = fit_scale(d, (size.0 - 2.0 * MARGIN, size.1 - 2.0 * MARGIN));
                let s = match mode {
                    SizeMode::Fit => fit,
                    SizeMode::Actual => 1.0,
                    SizeMode::Shrink => fit.min(1.0),
                    SizeMode::Custom(pct) if pct.is_finite() && pct > 0.0 => pct / 100.0,
                    SizeMode::Custom(_) => return Err(PrintError::Invalid("the scale must be more than 0%".into())),
                };
                sheets.push(Sheet { size, placed: vec![place_in(p, d, [0.0, 0.0, size.0, size.1], s, false)], ..Sheet::default() });
            }
        }
        Layout::Multiple { cols, rows, order, border, auto_rotate } => {
            let cells = cols.checked_mul(rows).filter(|&n| n > 0 && n <= 256);
            let Some(cells) = cells else {
                return Err(PrintError::Invalid("invalid number of pages per sheet".into()));
            };
            // A grid with more columns than rows wants a landscape sheet, and the reverse.
            let first = pages.first().and_then(|&p| sizes.get(p)).copied().ok_or(PrintError::NoPages)?;
            let page_landscape = first.0 > first.1;
            let (gc, gr, sheet_landscape) = if cols == rows {
                (cols, rows, page_landscape)
            } else if settings.orientation == Orientation::Landscape || (settings.orientation == Orientation::Auto && cols < rows) {
                // 2 per sheet (1 × 2 on portrait) prints side by side on landscape paper.
                (rows, cols, true)
            } else {
                (cols, rows, false)
            };
            let size = oriented(paper, settings.orientation, sheet_landscape);
            let gap = 6.0;
            let (aw, ah) = (size.0 - 2.0 * MARGIN, size.1 - 2.0 * MARGIN);
            let (cw, ch) = ((aw - gap * (gc - 1) as f64) / gc as f64, (ah - gap * (gr - 1) as f64) / gr as f64);
            if cw <= 0.0 || ch <= 0.0 {
                return Err(PrintError::Invalid("the page grid does not fit on the paper".into()));
            }
            let sheet_count = pages.len().div_ceil(cells);
            for sheet_index in 0..sheet_count {
                let mut sheet = Sheet { size, ..Sheet::default() };
                if order == PageOrder::CutStack {
                    // Marks align across every sheet, including unoccupied cells. Cut in the
                    // middle of each gutter; marks stay in the printable area, clear of cells.
                    for col in 1..gc {
                        let x = MARGIN + col as f64 * (cw + gap) - gap / 2.0;
                        sheet.lines.extend([[x, MARGIN, x, MARGIN + 8.0], [x, size.1 - MARGIN - 8.0, x, size.1 - MARGIN]]);
                    }
                    for row in 1..gr {
                        let y = size.1 - MARGIN - row as f64 * (ch + gap) + gap / 2.0;
                        sheet.lines.extend([[MARGIN, y, MARGIN + 8.0, y], [size.0 - MARGIN - 8.0, y, size.0 - MARGIN, y]]);
                    }
                }
                for k in 0..cells {
                    // A cell's pile gets consecutive pages. Missing pages leave blank cells,
                    // rather than shifting later piles left on the final sheets.
                    let index = if order == PageOrder::CutStack {
                        k.checked_mul(sheet_count).and_then(|n| n.checked_add(sheet_index))
                    } else {
                        sheet_index.checked_mul(cells).and_then(|n| n.checked_add(k))
                    };
                    let Some(p) = index.and_then(|i| pages.get(i)).copied() else { continue };

                    let (col, row) = match order {
                        PageOrder::Horizontal | PageOrder::CutStack => (k % gc, k / gc),
                        PageOrder::HorizontalReversed => (gc - 1 - k % gc, k / gc),
                        PageOrder::Vertical => (k / gr, k % gr),
                        PageOrder::VerticalReversed => (gc - 1 - k / gr, k % gr),
                    };
                    let x0 = MARGIN + col as f64 * (cw + gap);
                    let y1 = size.1 - MARGIN - row as f64 * (ch + gap);
                    let cell = [x0, y1 - ch, x0 + cw, y1];
                    let d = sizes.get(p).copied().ok_or_else(|| PrintError::Invalid("page size is missing".into()))?;
                    if !(d.0.is_finite() && d.1.is_finite() && d.0 > 0.0 && d.1 > 0.0) {
                        return Err(PrintError::Invalid("invalid page size".into()));
                    }
                    let rotate = auto_rotate && (d.0 > d.1) != (cw > ch) && (d.0 - d.1).abs() > 1.0;
                    let s = fit_scale(if rotate { (d.1, d.0) } else { d }, (cw, ch));
                    if !s.is_finite() || s <= 0.0 {
                        return Err(PrintError::Invalid("page size cannot be scaled to the grid".into()));
                    }
                    let pl = place_in(p, d, cell, s, rotate);
                    if border {
                        let b = pl.matrix.bbox(pl.clip);
                        sheet.borders.push(b);
                    }
                    sheet.placed.push(pl);
                }
                sheets.push(sheet);
            }
        }
        Layout::Booklet { subset, binding } => {
            // Pad to a multiple of 4 with blank pages (None).
            let n4 = pages.len().div_ceil(4) * 4;
            let at = |i: usize| pages.get(i).copied();
            let size = oriented(paper, Orientation::Landscape, true);
            let half = (size.0 - 2.0 * MARGIN) / 2.0;
            let cells = [[MARGIN, MARGIN, MARGIN + half, size.1 - MARGIN], [MARGIN + half, MARGIN, size.0 - MARGIN, size.1 - MARGIN]];
            for k in 0..n4 / 4 {
                let front = (at(n4 - 1 - 2 * k), at(2 * k));
                let back = (at(2 * k + 1), at(n4 - 2 - 2 * k));
                let mut sides = Vec::new();
                if subset != BookletSubset::BackOnly {
                    sides.push(front);
                }
                if subset != BookletSubset::FrontOnly {
                    sides.push(back);
                }
                for (l, r) in sides {
                    let (l, r) = if binding == Binding::Right { (r, l) } else { (l, r) };
                    let mut sheet = Sheet { size, ..Sheet::default() };
                    for (p, cell) in [(l, cells[0]), (r, cells[1])] {
                        if let Some(p) = p {
                            let d = sizes[p];
                            let s = fit_scale(d, (cell[2] - cell[0], cell[3] - cell[1]));
                            sheet.placed.push(place_in(p, d, cell, s, false));
                        }
                    }
                    sheets.push(sheet);
                }
            }
        }
        Layout::Poster { scale, overlap, cut_marks } => {
            if !(scale.is_finite() && scale > 0.0 && overlap.is_finite() && overlap >= 0.0) {
                return Err(PrintError::Invalid("invalid tile scale or overlap".into()));
            }
            let s = scale / 100.0;
            for &p in &pages {
                let d = sizes[p];
                let size = oriented(paper, settings.orientation, d.0 > d.1);
                // Each tile shows `step` of the scaled page plus `overlap` shared with the next.
                let (tw, th) = (size.0 - 2.0 * MARGIN, size.1 - 2.0 * MARGIN);
                if overlap * 2.0 >= tw.min(th) {
                    return Err(PrintError::Invalid("the overlap is larger than the tile".into()));
                }
                let (sw, sh) = (d.0 * s, d.1 * s);
                let (stepx, stepy) = (tw - overlap, th - overlap);
                let nx = (((sw - overlap) / stepx).ceil() as usize).max(1);
                let ny = (((sh - overlap) / stepy).ceil() as usize).max(1);
                if nx * ny > 1024 {
                    return Err(PrintError::Invalid("the poster would need more than 1024 sheets".into()));
                }
                // Tiles from the top-left, row by row.
                for ty in 0..ny {
                    for tx in 0..nx {
                        // The window into the scaled page (scaled display space, y up).
                        let wx0 = tx as f64 * stepx;
                        let wy1 = sh - ty as f64 * stepy;
                        let matrix = Matrix([s, 0.0, 0.0, s, MARGIN - wx0, size.1 - MARGIN - wy1]);
                        let clip = [wx0 / s, (wy1 - th) / s, (wx0 + tw) / s, wy1 / s];
                        let mut sheet = Sheet { size, placed: vec![Placement { page: p, matrix, clip }], ..Sheet::default() };
                        if cut_marks {
                            let (x0, y0, x1, y1) = (MARGIN, MARGIN, size.0 - MARGIN, size.1 - MARGIN);
                            for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
                                let dx = if x == x0 { -1.0 } else { 1.0 };
                                let dy = if y == y0 { -1.0 } else { 1.0 };
                                sheet.lines.push([x + dx * 2.0, y, x + dx * (MARGIN - 2.0), y]);
                                sheet.lines.push([x, y + dy * 2.0, x, y + dy * (MARGIN - 2.0)]);
                            }
                        }
                        sheets.push(sheet);
                    }
                }
            }
        }
    }
    Ok(sheets)
}

fn n(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn decoded_contents(doc: &Document, page: &Dict) -> Vec<u8> {
    let list: Vec<Object> = match page.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        },
    };
    let mut out = Vec::new();
    for o in list {
        if let Object::Stream(s) = &*doc.resolve(&o)
            && let Ok(d) = s.decoded()
        {
            out.extend_from_slice(&d);
            out.push(b'\n');
        }
    }
    out
}

/// Does annotation `d` print under `content`?
fn prints(doc: &Document, d: &Dict, content: Content) -> bool {
    const HIDDEN: i64 = 2;
    const PRINT: i64 = 4;
    let flags = d.get(b"F").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0);
    if flags & PRINT == 0 || flags & HIDDEN != 0 {
        return false;
    }
    let subtype = d.name(b"Subtype").unwrap_or(b"");
    match subtype {
        b"Widget" => true,
        b"Link" | b"Popup" => false,
        b"Stamp" => content != Content::Document && content != Content::FormFieldsOnly,
        _ => content == Content::DocumentAndMarkups,
    }
}

/// The normal appearance stream to draw for an annotation (honouring `/AS`).
fn appearance(doc: &Document, d: &Dict) -> Option<ObjRef> {
    let ap = doc.resolve(d.get(b"AP")?);
    let normal = ap.as_dict()?.get(b"N")?.clone();
    match &*doc.resolve(&normal) {
        Object::Stream(_) => normal.as_ref(),
        Object::Dict(states) => {
            let state = d.name(b"AS")?;
            states.get(state)?.as_ref()
        }
        _ => None,
    }
}

/// Algorithm 8.1: the matrix placing an appearance form (BBox × Matrix) onto the annotation rect.
fn appearance_matrix(doc: &Document, form: &Dict, rect: [f64; 4]) -> Option<Matrix> {
    let nums =
        |k: &[u8]| -> Option<Vec<f64>> { Some(doc.resolve(form.get(k)?).as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect()) };
    let bbox = nums(b"BBox").filter(|b| b.len() == 4)?;
    let m = nums(b"Matrix").filter(|m| m.len() == 6).map(|m| Matrix([m[0], m[1], m[2], m[3], m[4], m[5]])).unwrap_or_default();
    let b = m.bbox([bbox[0], bbox[1], bbox[2], bbox[3]]);
    let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
    if bw <= 0.0 || bh <= 0.0 {
        return None;
    }
    let a = Matrix([
        (rect[2] - rect[0]) / bw,
        0.0,
        0.0,
        (rect[3] - rect[1]) / bh,
        rect[0] - b[0] * (rect[2] - rect[0]) / bw,
        rect[1] - b[1] * (rect[3] - rect[1]) / bh,
    ]);
    // The Do that draws the form applies its /Matrix itself, so only the placement goes here.
    Some(a)
}

/// A Form XObject holding page `index` as it prints (user space; BBox = crop box).
fn page_form(doc: &mut Document, index: usize, content: Content) -> Result<ObjRef, PrintError> {
    let page = pdfcraft_model::pages(doc).swap_remove(index);
    let crop = page.crop(doc);
    let mut res = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    let mut body = if content == Content::FormFieldsOnly { Vec::new() } else { decoded_contents(doc, &page.dict) };
    // Wrap the page content so a state it leaves behind can't affect the annotations.
    if !body.is_empty() {
        let mut wrapped = b"q\n".to_vec();
        wrapped.extend_from_slice(&body);
        wrapped.extend_from_slice(b"\nQ\n");
        body = wrapped;
    }
    let annots = page.dict.get(b"Annots").and_then(|a| doc.resolve(a).as_array().cloned()).unwrap_or_default();
    let mut xobjects = res.get(b"XObject").and_then(|x| doc.resolve(x).as_dict().cloned()).unwrap_or_default();
    let mut k = 0;
    for a in annots {
        let Some(d) = doc.resolve(&a).as_dict().cloned() else { continue };
        if !prints(doc, &d, content) {
            continue;
        }
        let rect: Vec<f64> =
            d.get(b"Rect").and_then(|r| doc.resolve(r).as_array().map(|a| a.iter().filter_map(Object::as_f64).collect())).unwrap_or_default();
        if rect.len() != 4 {
            continue;
        }
        let rect = [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])];
        let Some(ap) = appearance(doc, &d) else { continue };
        let form = doc.get(ap).as_dict().cloned().unwrap_or_default();
        let Some(m) = appearance_matrix(doc, &form, rect) else { continue };
        let name = format!("PCPrA{k}");
        k += 1;
        let [a0, b0, c0, d0, e0, f0] = m.0;
        body.extend(format!("q {} {} {} {} {} {} cm /{name} Do Q\n", n(a0), n(b0), n(c0), n(d0), n(e0), n(f0)).bytes());
        xobjects.set(name.into_bytes(), Object::Ref(ap));
    }
    if !xobjects.is_empty() {
        res.set(b"XObject".to_vec(), Object::Dict(xobjects));
    }
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), Object::Array(crop.iter().map(|v| Object::Real(*v)).collect()));
    d.set(b"Resources".to_vec(), Object::Dict(res));
    if let Some(g) = page.dict.get(b"Group") {
        d.set(b"Group".to_vec(), g.clone());
    }
    Ok(doc.add(Object::Stream(Stream::flate(d, &body))))
}

/// Build the print-ready PDF for `settings`. Encryption is not carried over (the file goes to a
/// printer or is the user's own copy); callers must check the print permission first.
pub fn impose(src: &Document, settings: &Settings) -> Result<Vec<u8>, PrintError> {
    let pages = pdfcraft_model::pages(src);
    let sizes: Vec<(f64, f64)> = pages.iter().map(|p| p.display_size(src)).collect();
    let sheets = layout(&sizes, settings)?;
    let mut doc = src.clone();
    // One form per source page used (a page may appear on several sheets: posters).
    let mut forms: std::collections::HashMap<usize, ObjRef> = std::collections::HashMap::new();
    let views: Vec<[f64; 6]> = pages.iter().map(|p| p.view_matrix(src)).collect();
    let pages_root = doc.add(Object::Null);
    let mut kids = Vec::new();
    for sheet in &sheets {
        let mut c = String::new();
        let mut xo = Dict::new();
        for (i, pl) in sheet.placed.iter().enumerate() {
            let form = match forms.get(&pl.page) {
                Some(f) => *f,
                None => {
                    let f = page_form(&mut doc, pl.page, settings.content)?;
                    forms.insert(pl.page, f);
                    f
                }
            };
            let name = format!("P{i}");
            xo.set(name.clone().into_bytes(), Object::Ref(form));
            // Display space → sheet, then clip to the visible part, then user → display.
            let [a, b, cc, d, e, f] = pl.matrix.0;
            let to_display = Matrix(views[pl.page]).invert().unwrap_or_default();
            let [ua, ub, uc, ud, ue, uf] = to_display.0;
            let [x0, y0, x1, y1] = pl.clip;
            c.push_str(&format!(
                "q {} {} {} {} {} {} cm {} {} {} {} re W n {} {} {} {} {} {} cm /{name} Do Q\n",
                n(a),
                n(b),
                n(cc),
                n(d),
                n(e),
                n(f),
                n(x0),
                n(y0),
                n(x1 - x0),
                n(y1 - y0),
                n(ua),
                n(ub),
                n(uc),
                n(ud),
                n(ue),
                n(uf)
            ));
        }
        if !sheet.borders.is_empty() || !sheet.lines.is_empty() {
            c.push_str("q 0 G 0.5 w\n");
            for r in &sheet.borders {
                c.push_str(&format!("{} {} {} {} re S\n", n(r[0]), n(r[1]), n(r[2] - r[0]), n(r[3] - r[1])));
            }
            for l in &sheet.lines {
                c.push_str(&format!("{} {} m {} {} l S\n", n(l[0]), n(l[1]), n(l[2]), n(l[3])));
            }
            c.push_str("Q\n");
        }
        let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), c.as_bytes())));
        let mut res = Dict::new();
        res.set(b"XObject".to_vec(), Object::Dict(xo));
        let mut p = Dict::new();
        p.set(b"Type".to_vec(), Object::name("Page"));
        p.set(b"Parent".to_vec(), Object::Ref(pages_root));
        p.set(b"MediaBox".to_vec(), Object::Array([0.0, 0.0, sheet.size.0, sheet.size.1].iter().map(|v| Object::Real(*v)).collect()));
        p.set(b"Resources".to_vec(), Object::Dict(res));
        p.set(b"Contents".to_vec(), Object::Ref(contents));
        kids.push(Object::Ref(doc.add(Object::Dict(p))));
    }
    let mut tree = Dict::new();
    tree.set(b"Type".to_vec(), Object::name("Pages"));
    tree.set(b"Count".to_vec(), Object::Int(kids.len() as i64));
    tree.set(b"Kids".to_vec(), Object::Array(kids));
    doc.set(pages_root, Object::Dict(tree));
    let mut cat = Dict::new();
    cat.set(b"Type".to_vec(), Object::name("Catalog"));
    cat.set(b"Pages".to_vec(), Object::Ref(pages_root));
    let root = doc.add(Object::Dict(cat));
    doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(root));
    doc.trailer_mut().remove(b"Info");
    if doc.trailer().contains(b"Encrypt") {
        doc.remove_encryption();
    }
    Ok(write_full(&doc, &SaveOptions::default())?)
}

/// Which source pages land on each sheet (for previews and the sheet count).
pub fn sheet_pages(sheets: &[Sheet]) -> Vec<Vec<usize>> {
    sheets.iter().map(|s| s.placed.iter().map(|p| p.page).collect()).collect()
}
