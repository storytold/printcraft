//! A minimal text extractor, independent of the interpreter's font metrics and hit test.

use std::collections::HashMap;

use pdfcraft_content::{Matrix, parse};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use super::util::{Ctx, latin1, resolve_strict, squash};
use crate::annots_of;
use crate::limits::{MAX_CMAP, MAX_CMAP_ENTRIES, MAX_CMAP_WORK, MAX_DEPTH};

/// Is this one of the glyph-carrying text-showing operators (`Tj`, `'`, `"`, and `TJ` with at
/// least one non-empty string)? A `TJ` of numbers only (the gap redaction leaves for a removed
/// string) moves the cursor but shows nothing, so it does not count.
pub(super) fn carries_glyphs(op: &pdfcraft_content::Op) -> bool {
    let nonempty = |o: &Object| o.as_string().is_some_and(|s| !s.bytes.is_empty());
    match op.op.as_slice() {
        b"Tj" | b"'" | b"\"" => op.operands.last().is_some_and(nonempty),
        b"TJ" => op.operands.first().and_then(Object::as_array).is_some_and(|a| a.iter().any(nonempty)),
        _ => false,
    }
}

/// A page's text as two whitespace-free views: decoded text, and the character codes as Latin-1
/// (what fonts without a usable mapping show).
#[derive(Default)]
pub(super) struct Extracted {
    pub(super) text: String,
    pub(super) raw: String,
    pub(super) glyph_ops: usize,
    /// Glyph-carrying operators whose text could not be decoded.
    pub(super) undecodable_ops: usize,
    /// Per region asked about: glyphs whose centre this extractor places inside it.
    pub(super) shown_in: Vec<usize>,
    /// Glyph-carrying operators drawn where this extractor can't place them (tiling patterns,
    /// or an annotation appearance without a usable `/Rect`).
    pub(super) unplaced: usize,
}

/// How one font turns codes into text.
#[derive(Clone, Default)]
pub(super) struct Decoder {
    pub(super) map: HashMap<u32, String>,
    /// Bytes per code.
    pub(super) width: usize,
    /// No way to read these codes as text (composite font without a mapping).
    pub(super) opaque: bool,
}

impl Decoder {
    fn latin1() -> Self {
        Decoder { map: HashMap::new(), width: 1, opaque: false }
    }

    /// `Err` when the font's own data can't be read in full (a damaged or oversized CMap).
    fn new(doc: &Document, font: &Dict, ctx: &Ctx) -> Result<Self, ()> {
        let composite = font.name(b"Subtype") == Some(b"Type0");
        let mut d = Decoder { map: HashMap::new(), width: if composite { 2 } else { 1 }, opaque: false };
        let tu = font.get(b"ToUnicode").map(|t| doc.resolve(t));
        if let Some(Object::Stream(s)) = tu.as_deref() {
            let data = ctx.decode(s)?;
            if data.len() > MAX_CMAP {
                return Err(());
            }
            let (map, width) = parse_cmap(&data).ok_or(())?;
            if !map.is_empty() {
                d.map = map;
                if let Some(w) = width {
                    d.width = w;
                }
                return Ok(d);
            }
        }
        if composite {
            d.opaque = true;
            return Ok(d);
        }
        // Simple font: Latin-1, with the /Differences of the encoding.
        if let Some(enc) = font.get(b"Encoding").map(|e| doc.resolve(e))
            && let Some(diff) = enc.as_dict().and_then(|e| e.get(b"Differences")).map(|x| doc.resolve(x))
            && let Some(items) = diff.as_array()
        {
            let mut code = 0u32;
            for it in items {
                match &*doc.resolve(it) {
                    Object::Int(i) => code = u32::try_from(*i).unwrap_or(0),
                    Object::Name(n) => {
                        if let Some(c) = std::str::from_utf8(n).ok().and_then(pdfcraft_fonts::pdf::glyph_unicode) {
                            d.map.insert(code, c.to_string());
                        }
                        code = code.saturating_add(1);
                    }
                    _ => {}
                }
            }
        }
        Ok(d)
    }

    /// Each code of a shown string, with the text it gives (`None`: no way to read it as text).
    fn split(&self, s: &[u8]) -> Vec<(u32, Option<String>)> {
        let w = self.width.max(1);
        s.chunks(w)
            .map(|c| {
                let code = code_of(c);
                let text = if self.opaque {
                    None
                } else {
                    match self.map.get(&code) {
                        Some(t) => Some(t.clone()),
                        None if self.map.is_empty() || w == 1 && code < 256 => Some(char::from(u8::try_from(code).unwrap_or(b'?')).to_string()),
                        None => None,
                    }
                };
                (code, text)
            })
            .collect()
    }

    /// The text of a shown string.
    fn decode(&self, s: &[u8]) -> String {
        self.split(s).into_iter().filter_map(|(_, t)| t).collect()
    }
}

/// A font as the geometric check sees it: its decoder and its own simple width model.
#[derive(Clone)]
pub(super) struct Font {
    pub(super) dec: Decoder,
    /// Advance per code, in text space units per 1 em of font size (0.5 = half an em).
    pub(super) widths: HashMap<u32, f64>,
    /// Advance of a code `widths` doesn't list.
    pub(super) default_width: f64,
    pub(super) vertical: bool,
}

/// The width a font without usable `/Widths` is assumed to have (half an em).
pub(super) const FLAT_WIDTH: f64 = 0.5;

impl Font {
    fn flat() -> Self {
        Font { dec: Decoder::latin1(), widths: HashMap::new(), default_width: FLAT_WIDTH, vertical: false }
    }

    fn new(doc: &Document, font: &Dict, ctx: &Ctx) -> Result<Self, ()> {
        let dec = Decoder::new(doc, font, ctx)?;
        let mut f = Font { dec, widths: HashMap::new(), default_width: FLAT_WIDTH, vertical: false };
        let num = |o: &Object| doc.resolve(o).as_f64();
        if font.name(b"Subtype") == Some(b"Type0") {
            f.vertical = font.get(b"Encoding").map(|e| doc.resolve(e)).and_then(|e| e.as_name().map(|n| n.ends_with(b"-V"))).unwrap_or(false);
            let desc = font.get(b"DescendantFonts").map(|d| doc.resolve(d)).and_then(|d| d.as_array().and_then(|a| a.first().cloned()));
            let desc = desc.and_then(|d| doc.resolve(&d).as_dict().cloned());
            if let Some(d) = desc {
                f.default_width = d.get(b"DW").and_then(num).map_or(1.0, |w| w / 1000.0);
                let items: Vec<Object> = d.get(b"W").map(|w| doc.resolve(w)).and_then(|w| w.as_array().cloned()).unwrap_or_default();
                let mut i = 0;
                while let Some(first) = items.get(i).and_then(num) {
                    let first = first.max(0.0) as u32;
                    match items.get(i + 1).map(|o| doc.resolve(o)) {
                        Some(a) if a.as_array().is_some() => {
                            for (k, w) in a.as_array().into_iter().flatten().enumerate() {
                                if f.widths.len() >= MAX_CMAP_ENTRIES {
                                    return Err(());
                                }
                                if let (Some(w), Ok(k)) = (num(w), u32::try_from(k)) {
                                    f.widths.insert(first.saturating_add(k), w / 1000.0);
                                }
                            }
                            i += 2;
                        }
                        Some(last) => {
                            let (Some(last), Some(w)) = (last.as_f64(), items.get(i + 2).and_then(num)) else { break };
                            let last = (last.max(0.0) as u32).min(first.saturating_add(0xFFFF));
                            for c in first..=last {
                                if f.widths.len() >= MAX_CMAP_ENTRIES {
                                    return Err(());
                                }
                                f.widths.insert(c, w / 1000.0);
                            }
                            i += 3;
                        }
                        None => break,
                    }
                }
            }
        } else {
            // Glyph space units per em: 1000, or what a Type3 font's matrix says.
            let scale = if font.name(b"Subtype") == Some(b"Type3") {
                font.get(b"FontMatrix").map(|m| doc.resolve(m)).and_then(|m| m.as_array().and_then(|a| a.first().and_then(num))).unwrap_or(0.001)
            } else {
                0.001
            };
            let first = font.get(b"FirstChar").and_then(num).map_or(0, |v| v.max(0.0) as u32);
            let missing = font
                .get(b"FontDescriptor")
                .map(|d| doc.resolve(d))
                .and_then(|d| d.as_dict().and_then(|d| d.get(b"MissingWidth").and_then(num)))
                .map(|w| w * scale);
            if let Some(ws) = font.get(b"Widths").map(|w| doc.resolve(w)).and_then(|w| w.as_array().cloned()) {
                if ws.len() > MAX_CMAP_ENTRIES {
                    return Err(());
                }
                for (k, w) in ws.iter().enumerate() {
                    if let (Some(w), Ok(k)) = (num(w), u32::try_from(k)) {
                        f.widths.insert(first.saturating_add(k), w * scale);
                    }
                }
                f.default_width = missing.unwrap_or(0.0);
            }
        }
        Ok(f)
    }

    fn width(&self, code: u32) -> f64 {
        self.widths.get(&code).copied().unwrap_or(self.default_width)
    }
}

pub(super) fn hex_token(tok: &[u8]) -> Option<Vec<u8>> {
    let digits: Vec<u8> = tok.iter().filter_map(|c| (*c as char).to_digit(16).map(|d| d as u8)).collect();
    if digits.is_empty() || !digits.len().is_multiple_of(2) {
        return None;
    }
    Some(digits.chunks(2).map(|p| p.iter().fold(0u8, |a, d| (a << 4) | d)).collect())
}

pub(super) fn utf16_text(b: &[u8]) -> String {
    if let [c] = b {
        return char::from(*c).to_string();
    }
    let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
    String::from_utf16_lossy(&units)
}

pub(super) fn code_of(b: &[u8]) -> u32 {
    b.iter().fold(0u32, |a, x| a.wrapping_shl(8) | u32::from(*x))
}

pub(super) enum Tok {
    Hex(Vec<u8>),
    Word(String),
    Open,
    Close,
}

pub(super) fn cmap_tokens(data: &[u8]) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&c) = data.get(i) {
        match c {
            b'<' if data.get(i + 1) != Some(&b'<') => {
                let end = data.get(i..).and_then(|d| d.iter().position(|x| *x == b'>')).map_or(data.len(), |e| i + e);
                if let Some(h) = data.get(i + 1..end).and_then(hex_token) {
                    out.push(Tok::Hex(h));
                }
                i = end + 1;
            }
            b'[' => {
                out.push(Tok::Open);
                i += 1;
            }
            b']' => {
                out.push(Tok::Close);
                i += 1;
            }
            b'%' => {
                while data.get(i).is_some_and(|x| !matches!(x, b'\n' | b'\r')) {
                    i += 1;
                }
            }
            c if c.is_ascii_alphabetic() => {
                let start = i;
                while data.get(i).is_some_and(|x| x.is_ascii_alphanumeric()) {
                    i += 1;
                }
                out.push(Tok::Word(String::from_utf8_lossy(data.get(start..i).unwrap_or_default()).into_owned()));
            }
            _ => i += 1,
        }
    }
    out
}

/// `bfchar` / `bfrange` entries of a ToUnicode CMap, and the code length of its codespace.
/// `None` when the CMap is bigger than [`MAX_CMAP_ENTRIES`] entries or costs more than
/// [`MAX_CMAP_WORK`] steps: a map that is cut short would read as "no such code".
pub(super) fn parse_cmap(data: &[u8]) -> Option<(HashMap<u32, String>, Option<usize>)> {
    let toks = cmap_tokens(data);
    let mut map: HashMap<u32, String> = HashMap::new();
    let mut width = None;
    // Every step is paid for, whether or not it grows the map (a range may rewrite the same
    // codes again and again).
    let mut work = 0usize;
    let mut step = |n: usize| -> Option<()> {
        work = work.saturating_add(n);
        (work <= MAX_CMAP_WORK).then_some(())
    };
    let mut i = 0;
    while let Some(t) = toks.get(i) {
        i += 1;
        let Tok::Word(w) = t else { continue };
        match w.as_str() {
            "begincodespacerange" => {
                if let Some(Tok::Hex(h)) = toks.get(i) {
                    width = Some(h.len());
                }
            }
            "beginbfchar" => {
                while let (Some(Tok::Hex(src)), Some(Tok::Hex(dst))) = (toks.get(i), toks.get(i + 1)) {
                    step(1)?;
                    map.insert(code_of(src), utf16_text(dst));
                    if map.len() > MAX_CMAP_ENTRIES {
                        return None;
                    }
                    i += 2;
                }
            }
            "beginbfrange" => {
                while let (Some(Tok::Hex(lo)), Some(Tok::Hex(hi))) = (toks.get(i), toks.get(i + 1)) {
                    let (lo, hi) = (code_of(lo), code_of(hi));
                    let span = hi.saturating_sub(lo).min(0xFFFF);
                    match toks.get(i + 2) {
                        Some(Tok::Hex(dst)) => {
                            let base = utf16_text(dst);
                            step(usize::try_from(span).ok()?.saturating_add(1))?;
                            for k in 0..=span {
                                let mut chars: Vec<char> = base.chars().collect();
                                if let Some(last) = chars.last_mut() {
                                    *last = char::from_u32(u32::from(*last).saturating_add(k)).unwrap_or('\u{fffd}');
                                }
                                map.insert(lo.saturating_add(k), chars.into_iter().collect());
                            }
                            if map.len() > MAX_CMAP_ENTRIES {
                                return None;
                            }
                            i += 3;
                        }
                        Some(Tok::Open) => {
                            i += 3;
                            let mut k = 0u32;
                            while let Some(Tok::Hex(d)) = toks.get(i) {
                                step(1)?;
                                map.insert(lo.saturating_add(k), utf16_text(d));
                                if map.len() > MAX_CMAP_ENTRIES {
                                    return None;
                                }
                                k = k.saturating_add(1);
                                i += 1;
                            }
                            i += 1; // the closing bracket
                        }
                        _ => break,
                    }
                }
            }
            _ => {}
        }
    }
    Some((map, width))
}

pub(super) fn res_dict(doc: &Document, res: &Dict, key: &[u8]) -> Dict {
    res.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default()
}

pub(super) fn push_squashed(into: &mut String, s: &str) {
    into.extend(s.chars().filter(|c| !c.is_whitespace() && *c != '\u{feff}'));
}

/// A form's or pattern's `/Matrix`: absent is the identity, present but malformed is an error
/// (guessing the identity would put its content in the wrong place).
pub(super) fn matrix_of(doc: &Document, dict: &Dict) -> Result<Matrix, ()> {
    match dict.get(b"Matrix") {
        None => Ok(Matrix::IDENTITY),
        Some(m) => {
            let m = doc.resolve(m);
            let items: Vec<Object> = m.as_array().ok_or(())?.iter().map(|x| (*doc.resolve(x)).clone()).collect();
            Matrix::from_operands(&items).filter(|m| m.0.iter().all(|v| v.is_finite())).ok_or(())
        }
    }
}

/// A form's `/BBox` as `[x0 y0 x1 y1]`: absent is `None`, present but malformed is an error.
pub(super) fn bbox_of(doc: &Document, dict: &Dict) -> Result<Option<[f64; 4]>, ()> {
    let Some(b) = dict.get(b"BBox") else { return Ok(None) };
    let b = doc.resolve(b);
    let v: Vec<f64> =
        b.as_array().ok_or(())?.iter().map(|x| doc.resolve(x).as_f64().filter(|v| v.is_finite()).ok_or(())).collect::<Result<_, _>>()?;
    match v[..] {
        [a, b, c, d] => Ok(Some([a.min(c), b.min(d), a.max(c), b.max(d)])),
        _ => Err(()),
    }
}

/// The content streams of a page as `(raw object, decoded)`, a missing or unreadable entry an
/// error. `skip_overlay` leaves out the streams this run drew the boxes and labels with.
pub(super) fn page_content(doc: &Document, page: &Dict, ctx: &Ctx, skip_overlay: bool) -> Result<Vec<u8>, ()> {
    let list: Vec<Object> = match page.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*resolve_strict(doc, c)? {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        },
    };
    let mut data: Vec<u8> = Vec::new();
    for o in &list {
        match &*resolve_strict(doc, o)? {
            Object::Stream(s) => {
                let d = ctx.decode(s)?;
                if skip_overlay && ctx.is_overlay(&d) {
                    continue;
                }
                data.extend(d);
                data.push(b'\n');
            }
            // A null entry is an empty piece, as for the redaction itself (ISO 32000-2 7.3.9).
            Object::Null => {}
            _ => return Err(()),
        }
    }
    Ok(data)
}

/// The text of a page: its content, the forms and patterns it reaches (visible or not), and the
/// appearance streams of its annotations; and which of `regions` still hold glyphs.
pub(super) fn extract_page(doc: &Document, page: &Dict, ctx: &Ctx, regions: &[[f64; 4]]) -> Result<Extracted, ()> {
    let data = page_content(doc, page, ctx, true)?;
    let res = page.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    let mut ex = Extractor { doc, ctx, regions, out: Extracted { shown_in: vec![0; regions.len()], ..Extracted::default() }, stack: Vec::new() };
    ex.scope(&data, &res, Matrix::IDENTITY, 0, true)?;
    for a in annots_of(doc, page) {
        let Some(d) = doc.resolve(&a).as_dict().cloned() else { continue };
        let Some(ap) = d.get(b"AP").and_then(|x| doc.resolve(x).as_dict().cloned()) else { continue };
        let rect =
            d.get(b"Rect").and_then(|r| doc.resolve(r).as_array().map(|a| a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect::<Vec<_>>()));
        for (_, state) in ap.iter() {
            let s = doc.resolve(state);
            let streams: Vec<(Option<ObjRef>, Object)> = match &*s {
                Object::Stream(_) => vec![(state.as_ref(), (*s).clone())],
                Object::Dict(sub) => sub.iter().map(|(_, v)| (v.as_ref(), (*doc.resolve(v)).clone())).collect(),
                _ => Vec::new(),
            };
            for (r, o) in streams {
                if let Object::Stream(st) = o {
                    // Where the appearance lands on the page (ISO 32000-2 §12.5.5), if it can be told.
                    let ctm = rect.as_deref().and_then(|r| appearance_matrix(doc, &st, r));
                    ex.stream(&st, r, &res, ctm.unwrap_or_default(), 1, ctm.is_some())?;
                }
            }
        }
    }
    Ok(ex.out)
}

/// The matrix that maps an appearance stream's form space onto an annotation's `/Rect`.
pub(super) fn appearance_matrix(doc: &Document, ap: &Stream, rect: &[f64]) -> Option<Matrix> {
    let [rx0, ry0, rx1, ry1] = rect[..] else { return None };
    let bbox = bbox_of(doc, &ap.dict).ok()??;
    let m = matrix_of(doc, &ap.dict).ok()?;
    let t = m.bbox(bbox);
    let (tw, th) = (t[2] - t[0], t[3] - t[1]);
    if tw <= 0.0 || th <= 0.0 {
        return None;
    }
    let (rx0, rx1, ry0, ry1) = (rx0.min(rx1), rx0.max(rx1), ry0.min(ry1), ry0.max(ry1));
    let (sx, sy) = ((rx1 - rx0) / tw, (ry1 - ry0) / th);
    let a = Matrix([sx, 0.0, 0.0, sy, rx0 - t[0] * sx, ry0 - t[1] * sy]);
    Some(m.then(&a))
}

/// Text-state parameters that `q`/`Q` save and restore.
#[derive(Clone)]
pub(super) struct TextGs {
    pub(super) ctm: Matrix,
    pub(super) tc: f64,
    pub(super) tw: f64,
    pub(super) th: f64,
    pub(super) tl: f64,
    pub(super) rise: f64,
    pub(super) size: f64,
    pub(super) font: Vec<u8>,
}

pub(super) struct Extractor<'a> {
    pub(super) doc: &'a Document,
    pub(super) ctx: &'a Ctx,
    pub(super) regions: &'a [[f64; 4]],
    pub(super) out: Extracted,
    pub(super) stack: Vec<ObjRef>,
}

impl Extractor<'_> {
    /// A form or pattern stream. `ctm` maps its space onto the page's; `placed` is false where
    /// that mapping is not known.
    fn stream(&mut self, s: &Stream, id: Option<ObjRef>, parent: &Dict, ctm: Matrix, depth: usize, placed: bool) -> Result<(), ()> {
        if let Some(r) = id {
            if self.stack.contains(&r) {
                return Ok(());
            }
            self.stack.push(r);
        }
        let result = (|| {
            let data = self.ctx.decode(s)?;
            let own = s.dict.get(b"Resources").and_then(|r| self.doc.resolve(r).as_dict().cloned());
            self.scope(&data, own.as_ref().unwrap_or(parent), ctm, depth, placed)
        })();
        if id.is_some() {
            self.stack.pop();
        }
        result
    }

    /// Show the strings of one text-showing operator: extract their text, and place each glyph
    /// to count the ones inside a region. `tm` moves along with the pen.
    fn show_text(&mut self, font: &Font, op: &pdfcraft_content::Op, gs: &TextGs, tm: &mut Matrix, placed: bool) {
        let o = op.op.as_slice();
        // Strings and TJ adjustments, in order.
        let items: Vec<&Object> = match o {
            b"TJ" => op.operands.first().and_then(Object::as_array).map(|a| a.iter().collect()).unwrap_or_default(),
            _ => op.operands.last().into_iter().collect(),
        };
        let mut any_text = false;
        for it in items {
            if let Some(n) = it.as_f64() {
                let d = -n / 1000.0 * gs.size;
                let m = if font.vertical { Matrix::translate(0.0, d) } else { Matrix::translate(d * gs.th, 0.0) };
                *tm = m.then(tm);
                continue;
            }
            let Some(s) = it.as_string() else { continue };
            let t = font.dec.decode(&s.bytes);
            any_text |= !squash(&t).is_empty();
            push_squashed(&mut self.out.text, &t);
            push_squashed(&mut self.out.raw, &latin1(&s.bytes));
            for (code, text) in font.dec.split(&s.bytes) {
                let w = font.width(code);
                let space = text.as_deref().is_some_and(|t| t == " ");
                // The glyph's centre, in text space (the box runs from 0.2 em below the
                // baseline to 0.8 em above), and where the pen moves to.
                let (centre, advance) = if font.vertical {
                    let adv = -gs.size + gs.tc + if space { gs.tw } else { 0.0 };
                    ((0.0, gs.rise - 0.5 * gs.size), Matrix::translate(0.0, adv))
                } else {
                    let adv = (w * gs.size + gs.tc + if space { gs.tw } else { 0.0 }) * gs.th;
                    ((0.5 * w * gs.size * gs.th, gs.rise + 0.3 * gs.size), Matrix::translate(adv, 0.0))
                };
                let visible = text.as_deref().is_none_or(|t| !squash(t).is_empty());
                if placed && visible && !self.regions.is_empty() {
                    let (x, y) = tm.then(&gs.ctm).apply(centre.0, centre.1);
                    for (r, n) in self.regions.iter().zip(self.out.shown_in.iter_mut()) {
                        if x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3] {
                            *n += 1;
                        }
                    }
                }
                *tm = advance.then(tm);
            }
        }
        if carries_glyphs(op) {
            self.out.glyph_ops += 1;
            if !any_text {
                self.out.undecodable_ops += 1;
            }
            if !placed {
                self.out.unplaced += 1;
            }
        }
    }

    fn scope(&mut self, data: &[u8], res: &Dict, ctm: Matrix, depth: usize, placed: bool) -> Result<(), ()> {
        if depth > MAX_DEPTH {
            return Err(());
        }
        let doc = self.doc;
        let fonts = res_dict(doc, res, b"Font");
        let xobjects = res_dict(doc, res, b"XObject");
        let mut cache: HashMap<Vec<u8>, Font> = HashMap::new();
        let mut gs = TextGs { ctm, tc: 0.0, tw: 0.0, th: 1.0, tl: 0.0, rise: 0.0, size: 0.0, font: Vec::new() };
        let mut saved: Vec<TextGs> = Vec::new();
        let (mut tm, mut tlm) = (Matrix::IDENTITY, Matrix::IDENTITY);
        for op in parse(data).ops {
            let o = op.op.as_slice();
            match o {
                b"q" => {
                    if saved.len() >= 4096 {
                        return Err(());
                    }
                    saved.push(gs.clone());
                }
                b"Q" => {
                    if let Some(g) = saved.pop() {
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
                }
                b"Tc" => gs.tc = op.num(0).unwrap_or(0.0),
                b"Tw" => gs.tw = op.num(0).unwrap_or(0.0),
                b"Tz" => gs.th = op.num(0).unwrap_or(100.0) / 100.0,
                b"TL" => gs.tl = op.num(0).unwrap_or(0.0),
                b"Ts" => gs.rise = op.num(0).unwrap_or(0.0),
                b"Tf" => {
                    gs.font = op.name(0).map(<[u8]>::to_vec).unwrap_or_default();
                    gs.size = op.num(1).unwrap_or(0.0);
                }
                b"Td" | b"TD" => {
                    if let Some([x, y]) = op.nums::<2>() {
                        if o == b"TD" {
                            gs.tl = -y;
                        }
                        tlm = Matrix::translate(x, y).then(&tlm);
                        tm = tlm;
                    }
                }
                b"Tm" => {
                    if let Some(m) = op.nums::<6>() {
                        tm = Matrix(m);
                        tlm = tm;
                    }
                }
                b"T*" => {
                    tlm = Matrix::translate(0.0, -gs.tl).then(&tlm);
                    tm = tlm;
                }
                b"Tj" | b"TJ" | b"'" | b"\"" => {
                    if matches!(o, b"'" | b"\"") {
                        tlm = Matrix::translate(0.0, -gs.tl).then(&tlm);
                        tm = tlm;
                    }
                    if o == b"\""
                        && let (Some(aw), Some(ac)) = (op.num(0), op.num(1))
                    {
                        (gs.tw, gs.tc) = (aw, ac);
                    }
                    if !cache.contains_key(&gs.font) {
                        let f = match fonts.get(&gs.font).map(|f| doc.resolve(f)) {
                            Some(f) => f.as_dict().map_or_else(|| Ok(Font::flat()), |d| Font::new(doc, d, self.ctx))?,
                            None => Font::flat(),
                        };
                        cache.insert(gs.font.clone(), f);
                    }
                    let Some(font) = cache.get(&gs.font) else { continue };
                    self.show_text(font, &op, &gs, &mut tm, placed);
                }
                b"Do" => {
                    let Some(name) = op.name(0) else { continue };
                    let Some(entry) = xobjects.get(name) else { continue };
                    if let Object::Stream(s) = &*doc.resolve(entry)
                        && s.dict.name(b"Subtype") == Some(b"Form")
                    {
                        let m = matrix_of(doc, &s.dict)?;
                        bbox_of(doc, &s.dict)?;
                        self.stream(s, entry.as_ref(), res, m.then(&gs.ctm), depth + 1, placed)?;
                    }
                }
                _ => {}
            }
        }
        // Tiling patterns draw content of their own, repeated where the pattern is used: its
        // glyphs can't be placed.
        for (_, p) in res_dict(doc, res, b"Pattern").iter() {
            if let Object::Stream(s) = &*doc.resolve(p) {
                self.stream(s, p.as_ref(), res, Matrix::IDENTITY, depth + 1, false)?;
            }
        }
        Ok(())
    }
}
