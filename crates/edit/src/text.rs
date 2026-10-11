//! Edit a PDF ▸ Edit text: the text lines already on a page, and replacing a line's text in place.
//!
//! A *line* is a run of text-showing operators inside one `BT … ET` on the same baseline, read
//! from one content stream. Replacing it rewrites only that stream (as a new object, since content
//! streams may be shared): the first operator of the line shows the new text, the others are
//! dropped, and every positioning operator stays, so the lines after it keep their places
//! (`Td`, `T*` and friends move from the line matrix, which showing text doesn't change).
//!
//! The new text uses the line's own font when every character has a code and a glyph in it;
//! otherwise it is set in Helvetica (WinAnsi), and the result says the font was substituted.
//! Reflowing a paragraph to a new width is not done here (see `edit.text-reflow`).

use std::collections::HashMap;
use std::rc::Rc;

use pdfcraft_content::{Matrix, Op, Pieces, parse, serialize_ops};
use pdfcraft_cos::{Dict, Document, Object, PdfString, Stream};
use pdfcraft_fonts::pdf::Metrics;
use pdfcraft_fonts::{CraftFont, GlyphError, japanese_glyph_from};

use crate::EditError;

/// One line of existing text.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    /// The line's text as shown.
    pub text: String,
    /// Its box in user space.
    pub rect: [f64; 4],
    /// The font's resource name and `/BaseFont` (or Type 3 descriptor's `/FontName`), and its size in text space.
    pub font: String,
    pub base_font: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    /// The current fill colour as RGB.
    pub color: [f64; 3],
    /// Whether new text in this font can only be shown by substituting another font (no
    /// Unicode mapping for the line, so nothing could be reused).
    pub decodable: bool,
    /// The Japanese fallback face the font subsets, when the line is in one (see [`same_font`]).
    face: Option<String>,
    /// The content stream piece the line starts in, and its operators (indices into the page's
    /// streams parsed joined).
    stream: usize,
    ops: Vec<usize>,
    /// Where the line starts: text matrix (text space), the state there, and its `BT`.
    origin: Origin,
}

/// Text and graphics state captured for rewriting.
#[derive(Clone, Debug, PartialEq)]
struct TextState {
    font: Option<(String, f64)>,
    char_spacing: f64,
    word_spacing: f64,
    scale: f64,
    leading: f64,
    rise: f64,
    fill: Vec<Op>,
}

impl TextState {
    fn of(ts: &Ts) -> Self {
        TextState {
            font: ts.font.as_ref().map(|(n, _)| (String::from_utf8_lossy(n).into_owned(), ts.size)),
            char_spacing: ts.char_spacing,
            word_spacing: ts.word_spacing,
            scale: ts.scale,
            leading: ts.leading,
            rise: ts.rise,
            fill: ts.fill.clone(),
        }
    }

    /// Operators that set this state (inside a text object).
    fn ops(&self) -> Vec<Op> {
        let n = pdfcraft_content::num;
        let mut v = Vec::new();
        if let Some((f, size)) = &self.font {
            v.push(Op::new("Tf", vec![Object::name(f), n(*size)]));
        }
        v.push(Op::new("Tc", vec![n(self.char_spacing)]));
        v.push(Op::new("Tw", vec![n(self.word_spacing)]));
        v.push(Op::new("Tz", vec![n(self.scale * 100.0)]));
        v.push(Op::new("TL", vec![n(self.leading)]));
        v.push(Op::new("Ts", vec![n(self.rise)]));
        v.extend(self.fill.iter().cloned());
        v
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Origin {
    tm: [f64; 6],
    /// The text line matrix there (where the next line's Td/T* starts from).
    tlm: [f64; 6],
    /// Text-space units → user space (the scale of the text rendering matrix's y axis).
    k: f64,
    /// The CTM there (current space → page space), for moving the paragraph in page space.
    ctm: [f64; 6],
    baseline: f64,
    x: f64,
    state: TextState,
    bt_op: usize,
    bt_state: TextState,
}

/// A paragraph: consecutive lines in one stream with the same font and size, aligned on the left
/// and regularly spaced.
#[derive(Clone, Debug, PartialEq)]
pub struct TextBlock {
    /// The lines' text joined with spaces (lines that ended in a hyphen are joined directly).
    pub text: String,
    pub rect: [f64; 4],
    pub base_font: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    /// The text fill colour as RGB, used by the visual editor and preserved when no colour
    /// override is requested.
    pub color: [f64; 3],
    /// Indexes into [`text_lines`].
    pub lines: Vec<usize>,
}

impl TextLine {
    /// The baseline's height in user space.
    pub fn origin_baseline(&self) -> f64 {
        self.origin.baseline
    }
}

/// What replacing a line did.
#[derive(Clone, Debug, PartialEq)]
pub struct LineEdit {
    /// `Some(font)` when the line's font couldn't show the new text and Helvetica was used.
    pub substituted: Option<String>,
}

#[derive(Clone)]
struct Ts {
    ctm: Matrix,
    /// The operators that set the current fill colour (`g`, `rg`, `k`, or `cs` + `sc`/`scn`).
    fill: Vec<Op>,
    font: Option<(Vec<u8>, Rc<Metrics>)>,
    /// The Japanese fallback face the font subsets, when the font is one (see [`is_japanese_fallback`]).
    face: Option<String>,
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    scale: f64,
    leading: f64,
    rise: f64,
    render_mode: i64,
}

fn content_streams(doc: &Document, page: &Dict) -> Vec<(Object, Vec<u8>)> {
    let list: Vec<Object> = match page.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        },
    };
    list.into_iter()
        .map(|o| {
            let data = match &*doc.resolve(&o) {
                Object::Stream(s) => s.decoded().unwrap_or_default(),
                _ => Vec::new(),
            };
            (o, data)
        })
        .collect()
}

/// Rewrite a page's content (its streams joined, see [`Pieces`]): `edit(i)` gives the operators
/// to insert before operator `i` of `ops` (parsed from the joined pieces) and whether to keep it.
/// Everything else is copied byte for byte. A page's streams are one stream in pieces, split
/// between any two tokens (ISO 32000-2 §7.8.2), so an operator's operands can end one piece and
/// its keyword start the next; removing or replacing it rewrites every piece it spans. Returns
/// each piece's new bytes.
pub(crate) fn splice(pieces: &Pieces, ops: &[Op], mut edit: impl FnMut(usize) -> (Vec<Op>, bool)) -> Vec<Vec<u8>> {
    let data = pieces.data();
    let mut edits = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        let (insert, keep) = edit(i);
        if insert.is_empty() && keep {
            continue;
        }
        let mut bytes = Vec::new();
        if !insert.is_empty() {
            if op.span.start.checked_sub(1).and_then(|p| data.get(p)).is_some_and(|b| !b.is_ascii_whitespace()) {
                bytes.push(b'\n');
            }
            bytes.extend_from_slice(&serialize_ops(&insert));
        }
        let end = if keep { op.span.start } else { op.span.end };
        edits.push((op.span.start..end, bytes));
    }
    pieces.splice(edits)
}

/// The page's `/Contents` after a [`splice`]: pieces whose bytes changed become new streams
/// (keeping their dictionaries), the others stay as they were.
pub(crate) fn rewritten_contents(doc: &mut Document, streams: &[(Object, Vec<u8>)], new: Vec<Vec<u8>>) -> Object {
    let mut contents: Vec<Object> = Vec::with_capacity(streams.len());
    for ((obj, old), data) in streams.iter().zip(new) {
        if *old == data {
            contents.push(obj.clone());
            continue;
        }
        let mut dict = match &*doc.resolve(obj) {
            Object::Stream(s) => s.dict.clone(),
            _ => Dict::new(),
        };
        dict.remove(b"Length");
        contents.push(Object::Ref(doc.add(Object::Stream(Stream::flate(dict, &data)))));
    }
    match <[Object; 1]>::try_from(contents) {
        Ok([one]) => one,
        Err(many) => Object::Array(many),
    }
}

fn page_dict(doc: &Document, page: usize) -> Result<pdfcraft_model::Page, EditError> {
    pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))
}

fn fill_color(fill: &[Op]) -> [f64; 3] {
    let Some(op) = fill.iter().rev().find(|op| matches!(op.op.as_slice(), b"g" | b"rg" | b"k")) else {
        return [0.0, 0.0, 0.0];
    };
    match op.op.as_slice() {
        b"g" => {
            let v = op.operands.first().and_then(Object::as_f64).unwrap_or(0.0).clamp(0.0, 1.0);
            [v, v, v]
        }
        b"rg" => [
            op.operands.first().and_then(Object::as_f64).unwrap_or(0.0).clamp(0.0, 1.0),
            op.operands.get(1).and_then(Object::as_f64).unwrap_or(0.0).clamp(0.0, 1.0),
            op.operands.get(2).and_then(Object::as_f64).unwrap_or(0.0).clamp(0.0, 1.0),
        ],
        b"k" => {
            let c = op.operands.first().and_then(Object::as_f64).unwrap_or(0.0);
            let m = op.operands.get(1).and_then(Object::as_f64).unwrap_or(0.0);
            let y = op.operands.get(2).and_then(Object::as_f64).unwrap_or(0.0);
            let k = op.operands.get(3).and_then(Object::as_f64).unwrap_or(0.0);
            [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)]
        }
        _ => [0.0, 0.0, 0.0],
    }
}

/// The text-showing operators of one stream with their text, font, box and baseline.
struct Shown {
    op: usize,
    /// The text matrix where it starts (text space), and the state there and at its `BT`.
    tm: Matrix,
    tlm: Matrix,
    state: Ts,
    bt_op: usize,
    bt_state: Ts,
    text: String,
    rect: [f64; 4],
    baseline: f64,
    start_x: f64,
    end_x: f64,
    bt: usize,
    font: Vec<u8>,
    base_font: String,
    face: Option<String>,
    size: f64,
    bold: bool,
    italic: bool,
    decodable: bool,
}

/// The graphics state carried from one of a page's content streams to the next: the streams
/// are one stream in pieces (§7.8.2), so a `cm` (AutoCAD scales the whole page in the first
/// stream), an unbalanced `q`, the font and the colour still apply in the streams after it.
struct Carry {
    ts: Ts,
    stack: Vec<Ts>,
}

impl Carry {
    fn new() -> Self {
        let ts = Ts {
            ctm: Matrix::IDENTITY,
            fill: Vec::new(),
            font: None,
            face: None,
            size: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            scale: 1.0,
            leading: 0.0,
            rise: 0.0,
            render_mode: 0,
        };
        Carry { ts, stack: Vec::new() }
    }
}

/// A `Do` met while interpreting: its operator index, XObject name and the state there.
type Invocation = (usize, Vec<u8>, Ts);

fn interpret(
    doc: &Document,
    ops: &[Op],
    fonts_res: &Dict,
    cache: &mut HashMap<Vec<u8>, (Rc<Metrics>, Option<String>)>,
    carry: &mut Carry,
    mut invoked: Option<&mut Vec<Invocation>>,
) -> Vec<Shown> {
    let mut out = Vec::new();
    let mut ts = carry.ts.clone();
    let mut at_bt = (0usize, ts.clone());
    let mut stack: Vec<Ts> = std::mem::take(&mut carry.stack);
    let (mut tm, mut tlm) = (Matrix::IDENTITY, Matrix::IDENTITY);
    let mut bt = 0usize;
    for (i, op) in ops.iter().enumerate() {
        match op.op.as_slice() {
            b"q" => stack.push(ts.clone()),
            b"Q" => {
                if let Some(s) = stack.pop() {
                    ts = s;
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    ts.ctm = Matrix(m).then(&ts.ctm);
                }
            }
            b"BT" => {
                tm = Matrix::IDENTITY;
                tlm = Matrix::IDENTITY;
                bt += 1;
                at_bt = (i, ts.clone());
            }
            b"g" | b"rg" | b"k" | b"cs" => ts.fill = vec![op.clone()],
            b"sc" | b"scn" => {
                ts.fill.retain(|o| o.is("cs"));
                ts.fill.push(op.clone());
            }
            b"Tf" => {
                ts.size = op.num(1).unwrap_or(ts.size);
                if let Some(name) = op.name(0) {
                    let (m, face) = cache.entry(name.to_vec()).or_insert_with(|| {
                        let dict = fonts_res.get(name).and_then(|f| doc.resolve(f).as_dict().cloned());
                        let metrics = dict.as_ref().map_or_else(Metrics::fallback, |d| Metrics::from_dict(doc, d));
                        let face = dict.filter(is_japanese_fallback).map(|_| metrics.base_font.clone());
                        (Rc::new(metrics), face)
                    });
                    ts.font = Some((name.to_vec(), m.clone()));
                    ts.face = face.clone();
                }
            }
            b"Tc" => ts.char_spacing = op.num(0).unwrap_or(0.0),
            b"Tw" => ts.word_spacing = op.num(0).unwrap_or(0.0),
            b"Tz" => ts.scale = op.num(0).unwrap_or(100.0) / 100.0,
            b"TL" => ts.leading = op.num(0).unwrap_or(0.0),
            b"Ts" => ts.rise = op.num(0).unwrap_or(0.0),
            b"Tr" => ts.render_mode = op.operands.first().and_then(Object::as_int).unwrap_or(-1),
            b"Do" => {
                if let (Some(list), Some(name)) = (invoked.as_deref_mut(), op.name(0)) {
                    list.push((i, name.to_vec(), ts.clone()));
                }
            }
            b"Td" | b"TD" => {
                if let Some([x, y]) = op.nums::<2>() {
                    if op.is("TD") {
                        ts.leading = -y;
                    }
                    tlm = Matrix([1.0, 0.0, 0.0, 1.0, x, y]).then(&tlm);
                    tm = tlm;
                }
            }
            b"Tm" => {
                if let Some(m) = op.nums::<6>() {
                    tlm = Matrix(m);
                    tm = tlm;
                }
            }
            b"T*" => {
                tlm = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, -ts.leading]).then(&tlm);
                tm = tlm;
            }
            b"Tj" | b"TJ" | b"'" | b"\"" => {
                if matches!(op.op.as_slice(), b"'" | b"\"") {
                    if op.is("\"") {
                        ts.word_spacing = op.num(0).unwrap_or(ts.word_spacing);
                        ts.char_spacing = op.num(1).unwrap_or(ts.char_spacing);
                    }
                    tlm = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, -ts.leading]).then(&tlm);
                    tm = tlm;
                }
                let Some((name, m)) = ts.font.clone() else { continue };
                // Pieces: strings, and TJ number adjustments.
                let pieces: Vec<Object> = match op.op.as_slice() {
                    b"TJ" => op.operands.first().and_then(Object::as_array).cloned().unwrap_or_default(),
                    _ => op.operands.last().cloned().into_iter().collect(),
                };
                let trm0 = tm.then(&ts.ctm);
                let start = trm0.apply(0.0, ts.rise);
                let mut text = String::new();
                let mut decodable = true;
                let mut x_text = 0.0;
                for p in &pieces {
                    match p {
                        Object::String(s) => {
                            for (code, len) in m.codes(&s.bytes) {
                                match m.text_of(code) {
                                    Some(t) => text.push_str(&t),
                                    None => decodable = false,
                                }
                                let w = m.width(code) * ts.size + ts.char_spacing + if m.is_space(code, len) { ts.word_spacing } else { 0.0 };
                                x_text += w * ts.scale;
                            }
                        }
                        other => {
                            if let Some(n) = other.as_f64() {
                                let dx = -n / 1000.0 * ts.size * ts.scale;
                                // A large gap inside TJ reads as a space.
                                if dx > ts.size * 0.2 && !text.ends_with(' ') {
                                    text.push(' ');
                                }
                                x_text += dx;
                            }
                        }
                    }
                }
                tm = Matrix([1.0, 0.0, 0.0, 1.0, x_text, 0.0]).then(&tm);
                let end = tm.then(&ts.ctm).apply(0.0, ts.rise);
                // The box: from descent to ascent along the run.
                let corners = [
                    trm0.apply(0.0, ts.rise + m.descent * ts.size),
                    trm0.apply(x_text, ts.rise + m.descent * ts.size),
                    trm0.apply(0.0, ts.rise + m.ascent * ts.size),
                    trm0.apply(x_text, ts.rise + m.ascent * ts.size),
                ];
                let rect = corners
                    .iter()
                    .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)]);
                let size_user = (trm0.0[2].powi(2) + trm0.0[3].powi(2)).sqrt() * ts.size;
                // A numeric-only TJ adjusts the current text matrix but paints no glyphs.
                // It must not create a phantom editable line or enlarge its neighbours' boxes.
                if !pieces.iter().any(|p| matches!(p, Object::String(s) if !s.bytes.is_empty())) {
                    continue;
                }
                out.push(Shown {
                    op: i,
                    tm: Matrix([tm.0[0], tm.0[1], tm.0[2], tm.0[3], tm.0[4] - x_text * tm.0[0], tm.0[5] - x_text * tm.0[1]]),
                    state: ts.clone(),
                    tlm,
                    bt_op: at_bt.0,
                    bt_state: at_bt.1.clone(),
                    text,
                    rect,
                    baseline: start.1,
                    start_x: start.0,
                    end_x: end.0,
                    bt,
                    font: name,
                    base_font: m.base_font.clone(),
                    face: ts.face.clone(),
                    size: if size_user > 0.0 { size_user } else { ts.size },
                    bold: m.bold,
                    italic: m.italic,
                    decodable,
                });
            }
            _ => {}
        }
    }
    carry.ts = ts;
    carry.stack = stack;
    out
}

/// The lines of text on a page (0-based), in content order.
pub fn text_lines(doc: &Document, page: usize) -> Result<Vec<TextLine>, EditError> {
    let p = page_dict(doc, page)?;
    let res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let fonts_res = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
    let mut cache = HashMap::new();
    let mut lines: Vec<TextLine> = Vec::new();
    let mut carry = Carry::new();
    // The streams are one stream in pieces (§7.8.2), split between any two tokens: parse them
    // joined, so an operator whose operands end one piece still has them.
    let pieces = Pieces::join(&content_streams(doc, &p.dict).into_iter().map(|(_, d)| d).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let shown = interpret(doc, &ops, &fonts_res, &mut cache, &mut carry, None);
    let mut last: Option<(usize, usize, f64, f64, f64)> = None; // (piece, bt, baseline, end_x, size)
    for s in shown {
        // A line belongs to the piece its first operator starts in (lines in different pieces
        // aren't joined or grouped, as before; editing rewrites every piece an edit touches).
        let si = ops.get(s.op).map_or(0, |op| pieces.pieces_of(op).0);
        let joins = last.is_some_and(|(piece, bt, base, end, size)| {
            piece == si
                && bt == s.bt
                && lines.last().is_some_and(|l| same_font(l, &s.font, s.face.as_deref()))
                && lines.last().is_some_and(|l| {
                    (s.size - l.size).abs() < 0.01 && s.bold == l.bold && s.italic == l.italic && fill_color(&s.state.fill) == l.color
                })
                && (s.baseline - base).abs() < size * 0.3
                && s.start_x > end - size
                && s.start_x - end < size * 3.0
        });
        if joins && let Some(l) = lines.last_mut() {
            let gap = s.start_x - last.map_or(s.start_x, |x| x.3);
            if gap > l.size * 0.2 && !l.text.ends_with(' ') && !s.text.starts_with(' ') {
                l.text.push(' ');
            }
            l.text.push_str(&s.text);
            l.rect = [l.rect[0].min(s.rect[0]), l.rect[1].min(s.rect[1]), l.rect[2].max(s.rect[2]), l.rect[3].max(s.rect[3])];
            l.ops.push(s.op);
            l.decodable &= s.decodable;
            l.color = fill_color(&s.state.fill);
        } else {
            lines.push(TextLine {
                text: s.text.clone(),
                rect: s.rect,
                font: String::from_utf8_lossy(&s.font).into_owned(),
                base_font: s.base_font.clone(),
                size: s.size,
                bold: s.bold,
                italic: s.italic,
                color: fill_color(&s.state.fill),
                decodable: s.decodable,
                face: s.face.clone(),
                stream: si,
                ops: vec![s.op],
                origin: Origin {
                    tm: s.tm.0,
                    tlm: s.tlm.0,
                    k: (s.tm.then(&s.state.ctm).0[2].powi(2) + s.tm.then(&s.state.ctm).0[3].powi(2)).sqrt(),
                    ctm: s.state.ctm.0,
                    baseline: s.baseline,
                    x: s.start_x,
                    state: TextState::of(&s.state),
                    bt_op: s.bt_op,
                    bt_state: TextState::of(&s.bt_state),
                },
            });
        }
        last = Some((si, s.bt, s.baseline, s.end_x, s.size.max(1.0)));
    }
    // Lines of only spaces aren't editable text.
    lines.retain(|l| !l.text.trim().is_empty());
    Ok(lines)
}

/// The line `s` continues, or a new one: `last` is the previous run of the same stream
/// (`bt`, baseline, end x, size).
fn add_shown(lines: &mut Vec<TextLine>, last: &mut Option<(usize, f64, f64, f64)>, s: Shown, stream: usize) {
    let joins = last.is_some_and(|(bt, base, end, size)| {
        bt == s.bt
            && lines.last().is_some_and(|l| same_font(l, &s.font, s.face.as_deref()))
            && lines
                .last()
                .is_some_and(|l| (s.size - l.size).abs() < 0.01 && s.bold == l.bold && s.italic == l.italic && fill_color(&s.state.fill) == l.color)
            && (s.baseline - base).abs() < size * 0.3
            && s.start_x > end - size
            && s.start_x - end < size * 3.0
    });
    if joins && let Some(l) = lines.last_mut() {
        let gap = s.start_x - last.map_or(s.start_x, |x| x.2);
        if gap > l.size * 0.2 && !l.text.ends_with(' ') && !s.text.starts_with(' ') {
            l.text.push(' ');
        }
        l.text.push_str(&s.text);
        l.rect = [l.rect[0].min(s.rect[0]), l.rect[1].min(s.rect[1]), l.rect[2].max(s.rect[2]), l.rect[3].max(s.rect[3])];
        l.ops.push(s.op);
        l.decodable &= s.decodable;
        l.color = fill_color(&s.state.fill);
    } else {
        lines.push(TextLine {
            text: s.text.clone(),
            rect: s.rect,
            font: String::from_utf8_lossy(&s.font).into_owned(),
            base_font: s.base_font.clone(),
            size: s.size,
            bold: s.bold,
            italic: s.italic,
            color: fill_color(&s.state.fill),
            decodable: s.decodable,
            face: s.face.clone(),
            stream,
            ops: vec![s.op],
            origin: Origin {
                tm: s.tm.0,
                tlm: s.tlm.0,
                k: (s.tm.then(&s.state.ctm).0[2].powi(2) + s.tm.then(&s.state.ctm).0[3].powi(2)).sqrt(),
                ctm: s.state.ctm.0,
                baseline: s.baseline,
                x: s.start_x,
                state: TextState::of(&s.state),
                bt_op: s.bt_op,
                bt_state: TextState::of(&s.bt_state),
            },
        });
    }
    *last = Some((s.bt, s.baseline, s.end_x, s.size.max(1.0)));
}

/// How deep form XObjects nest before reading stops (they may also refer to themselves).
pub(crate) const MAX_FORM_DEPTH: usize = 12;

/// The most form XObjects one page's walk enters, at any depth. The depth cap alone still lets
/// a form that draws another form many times fan out exponentially (100 `Do`s on each of 12
/// levels), which would hang export on a hostile PDF.
pub(crate) const MAX_FORM_VISITS: usize = 4096;

/// A form XObject a page draws: its object, content, resources and its matrix composed with the
/// CTM at the `Do`.
pub(crate) struct FormCall {
    pub(crate) obj: Option<pdfcraft_cos::ObjRef>,
    pub(crate) data: Vec<u8>,
    pub(crate) resources: Dict,
    pub(crate) ctm: Matrix,
}

/// The form XObject `name` in `resources`, drawn with `ctm`. `None` for images, missing names
/// and forms already being read (`path`), nested too deep, or past the page's `visits` budget
/// ([`MAX_FORM_VISITS`]); each form entered counts one visit.
pub(crate) fn form_call(
    doc: &Document,
    resources: &Dict,
    name: &[u8],
    ctm: Matrix,
    path: &[pdfcraft_cos::ObjRef],
    visits: &mut usize,
) -> Option<FormCall> {
    if path.len() >= MAX_FORM_DEPTH || *visits >= MAX_FORM_VISITS {
        return None;
    }
    let xobjects = resources.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned())?;
    let o = xobjects.get(name)?;
    let obj = o.as_ref();
    if obj.is_some_and(|r| path.contains(&r)) {
        return None;
    }
    let Object::Stream(s) = &*doc.resolve(o) else { return None };
    if s.dict.name(b"Subtype") != Some(b"Form") {
        return None;
    }
    let m = s.dict.get(b"Matrix").map(|m| doc.resolve(m)).and_then(|m| {
        let a = m.as_array()?;
        let v: Vec<f64> = a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
        <[f64; 6]>::try_from(v).ok().filter(|v| v.iter().all(|x| x.is_finite()))
    });
    // A form without its own resources uses the ones it was drawn with (PDF 1.1 files).
    let own = s.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned());
    *visits = visits.saturating_add(1);
    Some(FormCall {
        obj,
        data: s.decoded().unwrap_or_default(),
        resources: own.unwrap_or_else(|| resources.clone()),
        ctm: m.map_or(ctm, |m| Matrix(m).then(&ctm)),
    })
}

/// The lines a page shows as it reads, including text drawn by form XObjects (which
/// [`text_lines`] leaves out because it can only rewrite the page's own streams). Lines from a
/// form carry a stream number past the page's streams, one per form drawn.
fn reading_lines(doc: &Document, page: usize) -> Result<Vec<TextLine>, EditError> {
    struct Walk<'a> {
        doc: &'a Document,
        lines: Vec<TextLine>,
        next_stream: usize,
        path: Vec<pdfcraft_cos::ObjRef>,
        visits: usize,
    }
    fn walk(w: &mut Walk, ops: &[Op], resources: &Dict, carry: &mut Carry, stream: usize) {
        let fonts_res = resources.get(b"Font").map(|f| w.doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
        let mut cache = HashMap::new();
        let mut invoked = Vec::new();
        let shown = interpret(w.doc, ops, &fonts_res, &mut cache, carry, Some(&mut invoked));
        let mut last = None;
        let mut forms = invoked.into_iter().peekable();
        for s in shown {
            while let Some((_, name, ts)) = forms.next_if(|f| f.0 < s.op) {
                enter(w, resources, &name, ts);
                // Text after a form starts a new line.
                last = None;
            }
            add_shown(&mut w.lines, &mut last, s, stream);
        }
        for (_, name, ts) in forms {
            enter(w, resources, &name, ts);
        }
    }
    fn enter(w: &mut Walk, resources: &Dict, name: &[u8], ts: Ts) {
        let Some(form) = form_call(w.doc, resources, name, ts.ctm, &w.path, &mut w.visits) else { return };
        let ops = parse(&form.data).ops;
        // The form starts with the graphics state at its `Do`, its own matrix applied.
        let mut carry = Carry { ts: Ts { ctm: form.ctm, ..ts }, stack: Vec::new() };
        let stream = w.next_stream;
        w.next_stream = w.next_stream.saturating_add(1);
        if let Some(r) = form.obj {
            w.path.push(r);
        }
        walk(w, &ops, &form.resources, &mut carry, stream);
        if form.obj.is_some() {
            w.path.pop();
        }
    }
    let p = page_dict(doc, page)?;
    let res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let streams = content_streams(doc, &p.dict);
    let mut w = Walk { doc, lines: Vec::new(), next_stream: streams.len(), path: Vec::new(), visits: 0 };
    let mut carry = Carry::new();
    for (si, (_, data)) in streams.into_iter().enumerate() {
        walk(&mut w, &parse(&data).ops, &res, &mut carry, si);
    }
    let mut lines = w.lines;
    lines.retain(|l| !l.text.trim().is_empty());
    Ok(lines)
}

/// Text → the runs that show it: each run's font resource name and the bytes shown in that font.
type Encoder = Box<dyn Fn(&str) -> Option<Vec<(String, Vec<u8>)>>>;

fn is_win_ansi_char(c: char) -> bool {
    c == '\t' || pdfcraft_fonts::win_ansi_byte(c).is_some()
}

fn needs_type3(text: &str) -> bool {
    text.chars().any(|c| !is_win_ansi_char(c))
}

/// The `/Name` of the Type 3 fonts generated for Japanese text.
const JAPANESE_FONT_NAME: &str = "PCJapanese";

/// Whether `font` is one of those generated fonts: a replacement longer than one of them is split
/// across several, which read back as one line (see [`same_font`]).
fn is_japanese_fallback(font: &Dict) -> bool {
    font.name(b"Subtype") == Some(b"Type3") && font.name(b"Name") == Some(JAPANESE_FONT_NAME.as_bytes())
}

/// Whether a run shown in `font` (a resource name, and the Japanese fallback face it subsets, if
/// any) continues `line`: the same font resource, or another subset of the same face.
fn same_font(line: &TextLine, font: &[u8], face: Option<&str>) -> bool {
    line.font.as_bytes() == font || (line.face.is_some() && line.face.as_deref() == face)
}

fn source_family(base_font: &str) -> crate::added::Family {
    let name = base_font.to_ascii_lowercase();
    if ["courier", "mono", "consolas", "menlo", "monaco", "lucida console"].iter().any(|s| name.contains(s)) {
        crate::added::Family::Courier
    } else if !name.contains("sans")
        && ["times", "serif", "roman", "mincho", "cambria", "georgia", "palatino", "garamond"].iter().any(|s| name.contains(s))
    {
        crate::added::Family::Times
    } else {
        crate::added::Family::Helvetica
    }
}

const SUBSTITUTE_NAME: &str = "PCEdHelv";
const SUBSTITUTE: &[u8] = SUBSTITUTE_NAME.as_bytes();
const MAX_TYPE3_GLYPHS: usize = 240;

/// The generated fonts for one replacement: one per run of at most `max_glyphs` unique characters,
/// in order of first appearance.
#[derive(Clone)]
struct Type3Fallback {
    /// The family its glyphs come from ("Shippori Mincho").
    family: &'static str,
    /// The fonts' resource names in order; the first is `PCJp`.
    names: Vec<String>,
    /// Every unique character with its advance (em), in order of first appearance.
    widths: Vec<(char, f64)>,
    /// Where each character is shown: the index of its font in `names`, and its code there.
    codes: HashMap<char, (usize, u8)>,
}

/// One glyph of a generated font: its code there, its character, its glyph procedure and advance.
struct Type3Glyph {
    code: u8,
    ch: char,
    path: Vec<u8>,
    width: f64,
}

fn pdf_num(v: f64) -> String {
    if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v:.4}").trim_end_matches('0').trim_end_matches('.').to_string() }
}

fn type3_path(face: &CraftFont, ch: char) -> Result<(Vec<u8>, f64), EditError> {
    let glyph = japanese_glyph_from(face, ch).map_err(|e| match e {
        GlyphError::NoFont => no_japanese_font(),
        GlyphError::Missing => EditError::Invalid(format!("no fallback font has a glyph for U+{:04X}", ch as u32)),
        GlyphError::TooComplex => EditError::Invalid(format!("Japanese fallback glyph U+{:04X} is too complex", ch as u32)),
    })?;
    let (dx, width) = proportional(ch, &glyph);
    let scale = 1000.0;
    // d1 is `wx wy llx lly urx ury` (ISO 32000-2 §9.6.4) with a box enclosing the glyph;
    // Acrobat draws a bullet in place of a glyph whose d1 is malformed.
    let b = if glyph.contours.is_empty() { [0.0; 4] } else { glyph.bbox };
    let mut out = format!(
        "{} 0 {} {} {} {} d1\n",
        pdf_num(width * scale),
        pdf_num(((b[0] + dx) * scale).floor()),
        pdf_num((b[1] * scale).floor()),
        pdf_num(((b[2] + dx) * scale).ceil()),
        pdf_num((b[3] * scale).ceil())
    )
    .into_bytes();
    for contour in glyph.contours {
        let Some(first) = contour.first() else { continue };
        out.extend_from_slice(format!("{} {} m\n", pdf_num((first[0] + dx) * scale), pdf_num(first[1] * scale)).as_bytes());
        for p in contour.iter().skip(1) {
            out.extend_from_slice(format!("{} {} l\n", pdf_num((p[0] + dx) * scale), pdf_num(p[1] * scale)).as_bytes());
        }
        out.extend_from_slice(b"h\n");
    }
    out.extend_from_slice(b"f\n");
    Ok((out, width))
}

/// Side bearing (em) given to a respaced glyph on each side.
const SIDE_BEARING: f64 = 0.05;

/// Shift and advance for a fallback glyph. Japanese faces draw Cyrillic and Greek full-width
/// (one em each, as in JIS X 0208) and have no proportional (`palt`) metrics for them, so a
/// word set that way reads as letter-spaced. Those letters get their ink width plus a side
/// bearing instead; everything else keeps the face's own advance.
fn proportional(ch: char, glyph: &pdfcraft_fonts::GlyphOutline) -> (f64, f64) {
    let respace = matches!(ch, '\u{0370}'..='\u{03FF}' | '\u{0400}'..='\u{052F}' | '\u{1F00}'..='\u{1FFF}');
    let ink = glyph.bbox[2] - glyph.bbox[0];
    if !respace || glyph.contours.is_empty() || !(ink > 0.0 && ink < glyph.width) {
        return (0.0, glyph.width);
    }
    (SIDE_BEARING - glyph.bbox[0], ink + 2.0 * SIDE_BEARING)
}

fn unicode_hex(ch: char) -> String {
    let mut units = [0u16; 2];
    let encoded = ch.encode_utf16(&mut units);
    encoded.iter().map(|u| format!("{u:04X}")).collect()
}

/// This build has no Japanese face to draw replacement text with.
fn no_japanese_font() -> EditError {
    EditError::Invalid(
        "this text needs PdfCraft's Japanese fallback font, which this build doesn't include \
         (official releases do; to build it in, set CRAFT_FONTS_DIR to a craft-fonts checkout)"
            .into(),
    )
}

/// The first of `faces` (best match first) that has a glyph for every one of `chars`, else the first.
fn fallback_face<'a>(faces: &[&'a CraftFont], chars: &[char], has_glyph: impl Fn(&CraftFont, char) -> bool) -> Option<&'a CraftFont> {
    faces.iter().copied().find(|face| chars.iter().all(|ch| has_glyph(face, *ch))).or_else(|| faces.first().copied())
}

fn type3_font(
    doc: &mut Document,
    fonts_res: &mut Dict,
    text: &str,
    family: crate::added::Family,
    bold: bool,
    max_glyphs: usize,
) -> Result<Type3Fallback, EditError> {
    let faces = pdfcraft_fonts::document_japanese_fonts_for_style(family == crate::added::Family::Times, bold);
    // Each unique character once, in order of first appearance.
    let mut chars = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for ch in text.chars() {
        if seen.insert(ch) {
            chars.push(ch);
        }
    }
    if chars.is_empty() {
        return Err(EditError::Invalid("replacement text is empty".into()));
    }
    // The faces differ in coverage (e.g. of Cyrillic), so use the best face that has every
    // character. When none has them all, the best face reports the character it lacks.
    let face = fallback_face(&faces, &chars, |face, ch| japanese_glyph_from(face, ch).is_ok()).ok_or_else(no_japanese_font)?;
    // Every outline is read before the document is touched, so a character that fails leaves it
    // as it was. Each font takes the next `per_font` characters; codes restart at 1 in each.
    let per_font = max_glyphs.clamp(1, MAX_TYPE3_GLYPHS);
    let mut glyphs = Vec::with_capacity(chars.len());
    for (i, ch) in chars.into_iter().enumerate() {
        let (path, width) = type3_path(face, ch)?;
        let code = u8::try_from((i % per_font).saturating_add(1))
            .map_err(|_| EditError::Invalid("Japanese replacement has too many unique characters".into()))?;
        glyphs.push(Type3Glyph { code, ch, path, width });
    }
    let mut codes = HashMap::with_capacity(glyphs.len());
    let mut widths = Vec::with_capacity(glyphs.len());
    let mut names = Vec::new();
    let mut glyphs = glyphs.into_iter();
    loop {
        let group: Vec<Type3Glyph> = glyphs.by_ref().take(per_font).collect();
        if group.is_empty() {
            break;
        }
        for glyph in &group {
            codes.insert(glyph.ch, (names.len(), glyph.code));
            widths.push((glyph.ch, glyph.width));
        }
        names.push(add_type3_font(doc, fonts_res, face, group));
    }
    Ok(Type3Fallback { family: face.family, names, widths, codes })
}

/// Adds one generated font whose codes 1 to `glyphs.len()` show `glyphs`, and returns its resource
/// name (`PCJp`, then `PCJp1`, ...). Nothing here can fail: the outlines were read already.
fn add_type3_font(doc: &mut Document, fonts_res: &mut Dict, face: &CraftFont, glyphs: Vec<Type3Glyph>) -> String {
    let last_char = glyphs.len();
    let mut charprocs = Dict::new();
    let mut widths = Vec::with_capacity(last_char);
    let mut differences = vec![Object::Int(1)];
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapType 2 def\n1 begincodespacerange\n<01> <FF>\nendcodespacerange\n",
    );
    cmap.push_str(&format!("{last_char} beginbfchar\n"));
    for glyph in glyphs {
        let glyph_name = format!("g{:02X}", glyph.code);
        let mut pd = Dict::new();
        pd.set(b"Length".to_vec(), glyph.path.len() as i64);
        let proc_ref = doc.add(Object::Stream(Stream::from_raw(pd, glyph.path)));
        charprocs.set(glyph_name.as_bytes().to_vec(), Object::Ref(proc_ref));
        differences.push(Object::name(&glyph_name));
        widths.push(Object::Real((glyph.width * 1000.0).round()));
        cmap.push_str(&format!("<{:02X}> <{}>\n", glyph.code, unicode_hex(glyph.ch)));
    }
    cmap.push_str("endbfchar\nendcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let mut cmap_dict = Dict::new();
    cmap_dict.set(b"Length".to_vec(), cmap.len() as i64);
    let cmap_ref = doc.add(Object::Stream(Stream::from_raw(cmap_dict, cmap.into_bytes())));
    let mut encoding = Dict::new();
    encoding.set(b"Type".to_vec(), Object::name("Encoding"));
    encoding.set(b"Differences".to_vec(), Object::Array(differences));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type3"));
    font.set(b"Name".to_vec(), Object::name(JAPANESE_FONT_NAME));
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name(&format!("{}-{}", face.family, face.style).replace(' ', "")));
    descriptor.set(b"FontFamily".to_vec(), Object::String(PdfString::literal(face.family.as_bytes().to_vec())));
    descriptor.set(b"Flags".to_vec(), Object::Int(if face.family.contains("Mincho") { 6 } else { 4 }));
    descriptor.set(b"ItalicAngle".to_vec(), Object::Int(0));
    // PDF 1.7 tables 5.9 and 5.19: a Type 3 descriptor is indirect; Ascent/Descent may be omitted.
    font.set(b"FontDescriptor".to_vec(), Object::Ref(doc.add(Object::Dict(descriptor))));
    font.set(b"FontBBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(-300), Object::Int(1000), Object::Int(1000)]));
    font.set(
        b"FontMatrix".to_vec(),
        Object::Array(vec![Object::Real(0.001), Object::Int(0), Object::Int(0), Object::Real(0.001), Object::Int(0), Object::Int(0)]),
    );
    font.set(b"FirstChar".to_vec(), Object::Int(1));
    font.set(b"LastChar".to_vec(), Object::Int(last_char as i64));
    font.set(b"Widths".to_vec(), Object::Array(widths));
    font.set(b"Encoding".to_vec(), Object::Dict(encoding));
    font.set(b"CharProcs".to_vec(), Object::Dict(charprocs));
    font.set(b"ToUnicode".to_vec(), Object::Ref(cmap_ref));
    let mut name = String::from("PCJp");
    let mut suffix = 0usize;
    while fonts_res.contains(name.as_bytes()) {
        suffix = suffix.saturating_add(1);
        name = format!("PCJp{suffix}");
    }
    fonts_res.set(name.as_bytes().to_vec(), Object::Dict(font));
    name
}

/// The runs that show `text` in the fallback: consecutive characters in one font form one run, so
/// a font is switched only where the text moves to another font. `None` if a character has no code.
fn type3_encode(fallback: &Type3Fallback, text: &str) -> Option<Vec<(String, Vec<u8>)>> {
    let mut runs: Vec<(usize, Vec<u8>)> = Vec::new();
    for ch in text.chars() {
        let (font, code) = *fallback.codes.get(&ch)?;
        match runs.last_mut() {
            Some((last, bytes)) if *last == font => bytes.push(code),
            _ => runs.push((font, vec![code])),
        }
    }
    runs.into_iter().map(|(font, bytes)| Some((fallback.names.get(font)?.clone(), bytes))).collect()
}

/// Replace the text of line `line` (an index into [`text_lines`]) on `page` with `text`.
pub fn replace_line(doc: &mut Document, page: usize, line: usize, text: &str) -> Result<LineEdit, EditError> {
    replace_line_with_limit(doc, page, line, text, MAX_TYPE3_GLYPHS)
}

/// [`replace_line`] with the most unique characters one generated font may hold (tests lower it).
pub(crate) fn replace_line_with_limit(doc: &mut Document, page: usize, line: usize, text: &str, max_glyphs: usize) -> Result<LineEdit, EditError> {
    let text = text.replace(['\n', '\r'], " ");
    let lines = text_lines(doc, page)?;
    let target = lines.get(line).cloned().ok_or_else(|| EditError::Invalid(format!("page {} has no line {}", page + 1, line + 1)))?;
    let p = page_dict(doc, page)?;
    let mut res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut fonts_res = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
    let streams = content_streams(doc, &p.dict);
    let pieces = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let first = target.ops.first().copied().filter(|i| *i < ops.len()).ok_or_else(|| EditError::Invalid("the page's content changed".into()))?;
    // The line's own font, when it can show every character.
    let font = fonts_res.get(target.font.as_bytes()).and_then(|f| doc.resolve(f).as_dict().cloned()).map(|d| Metrics::from_dict(doc, &d));
    let reused = font.as_ref().and_then(|m| m.encode(&text));
    let mut substituted = None;
    let mut replacement: Vec<Op> = Vec::new();
    // ' and " also move to the next line; keep that.
    match ops[first].op.as_slice() {
        b"'" => replacement.push(Op::new("T*", vec![])),
        b"\"" => {
            replacement.push(Op::new("Tw", vec![ops[first].operands.first().cloned().unwrap_or(Object::Int(0))]));
            replacement.push(Op::new("Tc", vec![ops[first].operands.get(1).cloned().unwrap_or(Object::Int(0))]));
            replacement.push(Op::new("T*", vec![]));
        }
        _ => {}
    }
    match reused {
        Some(bytes) => replacement.push(Op::new("Tj", vec![Object::String(PdfString::literal(bytes))])),
        None => {
            if needs_type3(&text) {
                let fallback = type3_font(doc, &mut fonts_res, &text, source_family(&target.base_font), target.bold, max_glyphs)?;
                let runs = type3_encode(&fallback, &text)
                    .ok_or_else(|| EditError::Invalid(format!("\"{text}\" can't be shown by the Japanese fallback")))?;
                let size = font_size_before(&ops, first).unwrap_or(target.size);
                for (name, bytes) in runs {
                    replacement.push(Op::new("Tf", vec![Object::name(&name), pdfcraft_content::num(size)]));
                    replacement.push(Op::new("Tj", vec![Object::String(PdfString::literal(bytes))]));
                }
                replacement.push(Op::new("Tf", vec![Object::name(&target.font), pdfcraft_content::num(size)]));
                substituted = Some(format!("{} Type3", fallback.family));
            } else {
                let win = pdfcraft_fonts::win_ansi(&text);
                // WinAnsi turns what it can't show into '?'; refuse rather than print the wrong thing.
                let back: String = win.iter().map(|b| char::from_u32(u32::from(*b)).unwrap_or('?')).collect();
                if text.chars().zip(back.chars()).any(|(a, b)| b == '?' && a != '?') {
                    return Err(EditError::Invalid(format!("\"{text}\" has characters neither {} nor Helvetica can show", target.base_font)));
                }
                // The size in text space: the current Tf's size.
                let size = font_size_before(&ops, first).unwrap_or(target.size);
                let family = source_family(&target.base_font);
                let base = family.base_font(target.bold, target.italic);
                let substitute_name = format!("PCEd{}", base.replace('-', ""));
                replacement.push(Op::new("Tf", vec![Object::name(&substitute_name), pdfcraft_content::num(size)]));
                replacement.push(Op::new("Tj", vec![Object::String(PdfString::literal(win))]));
                replacement.push(Op::new("Tf", vec![Object::name(&target.font), pdfcraft_content::num(size)]));
                substituted = Some(base.to_string());
                let mut f = Dict::new();
                f.set(b"Type".to_vec(), Object::name("Font"));
                f.set(b"Subtype".to_vec(), Object::name("Type1"));
                f.set(b"BaseFont".to_vec(), Object::name(base));
                f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
                fonts_res.set(substitute_name.into_bytes(), Object::Dict(f));
            }
        }
    }
    // Rebuild: the line's first operator becomes the replacement, its others go. Copies of the
    // line drawn in the same area go too, so the replacement is all that shows.
    let drops = coincident_ops(&lines, std::slice::from_ref(&target.rect));
    let new_data = splice(&pieces, &ops, |i| if i == first { (std::mem::take(&mut replacement), false) } else { (Vec::new(), !drops.contains(&i)) });
    let contents = rewritten_contents(doc, &streams, new_data);
    let page_ref = p.obj;
    if substituted.is_some() {
        res.set(b"Font".to_vec(), Object::Dict(fonts_res));
    }
    doc.update_dict(page_ref, |d| {
        d.set(b"Contents".to_vec(), contents);
        if substituted.is_some() {
            d.set(b"Resources".to_vec(), Object::Dict(res));
        }
    })?;
    Ok(LineEdit { substituted })
}

/// The size of the last `Tf` before operator `at`.
fn font_size_before(ops: &[Op], at: usize) -> Option<f64> {
    ops[..at].iter().rev().find(|o| o.is("Tf")).and_then(|o| o.num(1))
}

/// The operators of every line drawn in the same area as one of `rects`. Documents sometimes
/// draw a line more than once (fake bold, an invisible text layer), and a surviving copy would
/// show the old text under the replaced line. Copies overlap a member's rect by more than half of
/// the smaller rect; neighbouring lines share no area.
fn coincident_ops(lines: &[TextLine], rects: &[[f64; 4]]) -> std::collections::HashSet<usize> {
    let area = |r: [f64; 4]| ((r[2] - r[0]) * (r[3] - r[1])).max(0.0);
    let mut drop = std::collections::HashSet::new();
    for l in lines {
        let covered = rects.iter().any(|m| {
            let ix = (m[2].min(l.rect[2]) - m[0].max(l.rect[0])).max(0.0);
            let iy = (m[3].min(l.rect[3]) - m[1].max(l.rect[1])).max(0.0);
            let small = area(*m).min(area(l.rect));
            small > 0.0 && ix * iy / small > 0.5
        });
        if covered {
            drop.extend(l.ops.iter().copied());
        }
    }
    drop
}

/// The paragraphs on a page (0-based).
pub fn text_blocks(doc: &Document, page: usize) -> Result<Vec<TextBlock>, EditError> {
    let lines = text_lines(doc, page)?;
    Ok(group_blocks(&lines))
}

/// The paragraphs on a page as a reader sees them, including text drawn by form XObjects
/// (Export to Word, HTML and RTF). Read-only: indexes into this list are not
/// [`text_blocks`] indexes, so they can't be passed to [`replace_block`] or [`rewrite_block`].
pub fn reading_blocks(doc: &Document, page: usize) -> Result<Vec<TextBlock>, EditError> {
    let lines = reading_lines(doc, page)?;
    Ok(group_blocks(&lines))
}

/// Lines that share a left edge, a centre or a right edge (left, centred or right-aligned text).
fn aligned(a: &TextLine, b: &TextLine) -> bool {
    let tol = b.size.max(1.0);
    let centre = |r: [f64; 4]| (r[0] + r[2]) / 2.0;
    (a.origin.x - b.origin.x).abs() < tol || (centre(a.rect) - centre(b.rect)).abs() < tol || (a.rect[2] - b.rect[2]).abs() < tol
}

fn group_blocks(lines: &[TextLine]) -> Vec<TextBlock> {
    let mut blocks: Vec<TextBlock> = Vec::new();
    let mut gap: Option<f64> = None;
    for (i, l) in lines.iter().enumerate() {
        let joins = i > 0
            && blocks.last().is_some_and(|b| {
                let Some(&p) = b.lines.last() else { return false };
                let prev = &lines[p];
                let g = prev.origin.baseline - l.origin.baseline;
                prev.stream == l.stream
                    && same_font(prev, l.font.as_bytes(), l.face.as_deref())
                    && prev.base_font == l.base_font
                    && prev.bold == l.bold
                    && prev.italic == l.italic
                    && prev.color == l.color
                    && (prev.size - l.size).abs() < 0.01
                    && aligned(prev, l)
                    && g > l.size * 0.8
                    && g < l.size * 2.5
                    && gap.is_none_or(|first| (g - first).abs() < first * 0.2)
            });
        if joins
            && let Some(b) = blocks.last_mut()
            && let Some(&p) = b.lines.last()
        {
            let prev = &lines[p];
            gap.get_or_insert(prev.origin.baseline - l.origin.baseline);
            if b.text.ends_with('-') {
                b.text.pop();
            } else {
                b.text.push(' ');
            }
            b.text.push_str(l.text.trim_matches(paragraph_separator));
            b.rect = [b.rect[0].min(l.rect[0]), b.rect[1].min(l.rect[1]), b.rect[2].max(l.rect[2]), b.rect[3].max(l.rect[3])];
            b.lines.push(i);
        } else {
            gap = None;
            blocks.push(TextBlock {
                text: l.text.trim_matches(paragraph_separator).to_string(),
                rect: l.rect,
                base_font: l.base_font.clone(),
                size: l.size,
                bold: l.bold,
                italic: l.italic,
                color: l.color,
                lines: vec![i],
            });
        }
    }
    blocks
}

// An ideographic space carries intentional Japanese spacing, including paragraph indentation.
// Keep other whitespace normalization unchanged.
fn paragraph_separator(c: char) -> bool {
    c.is_whitespace() && c != '\u{3000}'
}

/// Greedy word wrapping to `width` with `advance` giving a string's width.
fn wrap(text: &str, width: f64, advance: impl Fn(&str) -> f64) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split(paragraph_separator).filter(|word| !word.is_empty()) {
        let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if !line.is_empty() && advance(&candidate) > width {
            out.push(std::mem::take(&mut line));
            line = word.to_string();
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() || out.is_empty() {
        out.push(line);
    }
    out
}

/// Replace paragraph `block` (an index into [`text_blocks`]) with `text`, rewrapped to the
/// paragraph's width with its line spacing. The paragraph keeps its first line's position, font
/// (or Helvetica when the font can't show the text), size and colour.
pub fn replace_block(doc: &mut Document, page: usize, block: usize, text: &str) -> Result<LineEdit, EditError> {
    rewrite_block(doc, page, block, Some(text), &BlockStyle::default())
}

/// Formatting for a rewritten paragraph (`None` keeps the paragraph's own).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockStyle {
    /// A standard font family, bold, italic.
    pub family: Option<(crate::added::Family, bool, bool)>,
    /// Override weight without choosing a different family (`None` keeps the source weight).
    pub bold: Option<bool>,
    /// Font size in points (user space).
    pub size: Option<f64>,
    /// Fill colour (RGB 0–1).
    pub color: Option<[f64; 3]>,
    pub align: Option<crate::added::Align>,
    /// Draw a line under each line of text.
    pub underline: Option<bool>,
    /// Line spacing as a multiple of the font size (1.2 is ordinary).
    pub line_spacing: Option<f64>,
    /// Character spacing (points) and horizontal scaling (percent).
    pub char_spacing: Option<f64>,
    pub scale: Option<f64>,
    /// Move the paragraph by `[dx, dy]` in user space (dragging its box).
    pub offset: Option<[f64; 2]>,
    /// Rewrap to this width in user space (dragging the box's edge).
    pub width: Option<f64>,
}

/// Rewrite paragraph `block` with new text (or its own) and formatting, rewrapped to its width.
pub fn rewrite_block(doc: &mut Document, page: usize, block: usize, text: Option<&str>, style: &BlockStyle) -> Result<LineEdit, EditError> {
    rewrite_block_with_limit(doc, page, block, text, style, MAX_TYPE3_GLYPHS)
}

/// [`rewrite_block`] with the most unique characters one generated font may hold (tests lower it).
pub(crate) fn rewrite_block_with_limit(
    doc: &mut Document,
    page: usize,
    block: usize,
    text: Option<&str>,
    style: &BlockStyle,
    max_glyphs: usize,
) -> Result<LineEdit, EditError> {
    let lines = text_lines(doc, page)?;
    let blocks = group_blocks(&lines);
    let b = blocks.get(block).cloned().ok_or_else(|| EditError::Invalid(format!("page {} has no paragraph {}", page + 1, block + 1)))?;
    let members: Vec<&TextLine> = b.lines.iter().map(|i| &lines[*i]).collect();
    let first = members[0];
    let p = page_dict(doc, page)?;
    let mut res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut fonts_res = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
    let streams = content_streams(doc, &p.dict);
    let pieces = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let text = text.unwrap_or(&b.text).split(paragraph_separator).filter(|word| !word.is_empty()).collect::<Vec<_>>().join(" ");
    let o = &first.origin;
    let (font_name, old_size) = o.state.font.clone().ok_or_else(|| EditError::Invalid("the paragraph has no font".into()))?;
    let k = o.k.max(1e-6);
    // The size in text space: points ÷ the text-to-user scale.
    let size = style.size.map_or(old_size, |pt| (pt / k).max(0.1));
    // Spacing and scaling (text state), with the chosen values.
    let mut ts_state = o.state.clone();
    if let Some(c) = style.char_spacing {
        ts_state.char_spacing = c / k;
    }
    if let Some(sc) = style.scale {
        ts_state.scale = (sc / 100.0).clamp(0.1, 10.0);
    }
    let o_state = ts_state.clone();
    let metrics = fonts_res.get(font_name.as_bytes()).and_then(|f| doc.resolve(f).as_dict().cloned()).map(|d| Metrics::from_dict(doc, &d));
    let reuse = style.family.is_none() && style.bold.is_none() && metrics.as_ref().is_some_and(|m| m.encode(&text).is_some());
    // The chosen style also determines the real Japanese fallback outlines and advances.
    let (family, bold, italic) = style.family.unwrap_or((source_family(&b.base_font), b.bold, b.italic));
    let bold = style.bold.unwrap_or(bold);
    let std_width = move |s: &str, size: f64| -> f64 {
        match family {
            crate::added::Family::Courier => s.chars().count() as f64 * 0.6 * size,
            crate::added::Family::Times => pdfcraft_fonts::helvetica_width(s, size) * 0.92,
            crate::added::Family::Helvetica => pdfcraft_fonts::helvetica_width(s, size) * if bold { 1.05 } else { 1.0 },
        }
    };
    let [dx, dy] = style.offset.unwrap_or([0.0, 0.0]);
    if !(dx.is_finite() && dy.is_finite()) {
        return Err(EditError::Invalid("the paragraph can't be moved that far".into()));
    }
    // A paragraph keeps its width (or takes the one asked for); a single line grows to the right,
    // up to the page's margin.
    let width = match style.width {
        // Capped at the largest page PDF allows (14 400 pt).
        Some(w) if w.is_finite() => w.clamp(size * k, 14_400.0),
        Some(_) => return Err(EditError::Invalid("the paragraph's width must be a number".into())),
        None if members.len() == 1 => (b.rect[2] - b.rect[0]).max(size * k).max(p.crop(doc)[2] - 36.0 - (b.rect[0] + dx)),
        None => (b.rect[2] - b.rect[0]).max(size * k),
    };
    // The move and the width are checked here, before the fallback fonts are written: a refused
    // paragraph leaves the document as it was. The move is in page space, so it is taken back through
    // the CTM's linear part into the space the paragraph is drawn in.
    let movement = if dx != 0.0 || dy != 0.0 {
        let c = o.ctm;
        let back = Matrix([c[0], c[1], c[2], c[3], 0.0, 0.0])
            .invert()
            .ok_or_else(|| EditError::Invalid("the paragraph is drawn in a space it can't be moved in".into()))?;
        Some(back.apply(dx, dy))
    } else {
        None
    };
    let type3 = if !reuse && needs_type3(&text) { Some(type3_font(doc, &mut fonts_res, &text, family, bold, max_glyphs)?) } else { None };
    let advance = |s: &str| -> f64 {
        let t = match (&metrics, reuse) {
            (Some(m), true) => m
                .encode(s)
                .map(|bytes| {
                    m.codes(&bytes)
                        .iter()
                        .map(|(c, l)| m.width(*c) * size + o_state.char_spacing + if m.is_space(*c, *l) { o_state.word_spacing } else { 0.0 })
                        .sum::<f64>()
                })
                .unwrap_or(0.0),
            _ if let Some(fallback) = &type3 => {
                // Each character's count in `s`, from one pass over `s`.
                let mut counts: HashMap<char, usize> = HashMap::new();
                for c in s.chars() {
                    let count = counts.entry(c).or_insert(0);
                    *count = count.saturating_add(1);
                }
                fallback.widths.iter().map(|(ch, width)| counts.get(ch).copied().unwrap_or(0) as f64 * width * size).sum::<f64>()
                    + s.chars().count() as f64 * o_state.char_spacing
            }
            _ => std_width(s, size) + s.chars().count() as f64 * o_state.char_spacing,
        };
        t * o_state.scale * k
    };
    let wrapped = wrap(&text, width + 0.5, advance);
    let mut substituted = None;
    let new_font = !reuse;
    let (show_font, encode): (String, Encoder) = if let Some(m) = metrics.clone().filter(|_| reuse) {
        let name = font_name.clone();
        (font_name.clone(), Box::new(move |s: &str| m.encode(s).map(|bytes| vec![(name.clone(), bytes)])))
    } else if let Some(fallback) = type3.clone() {
        substituted = Some(format!("{} Type3", fallback.family));
        let first = fallback.names.first().cloned().ok_or_else(no_japanese_font)?;
        (first, Box::new(move |s: &str| type3_encode(&fallback, s)))
    } else {
        let win = pdfcraft_fonts::win_ansi(&text);
        let back: String = win.iter().map(|c| char::from_u32(u32::from(*c)).unwrap_or('?')).collect();
        let base = family.base_font(bold, italic);
        if text.chars().zip(back.chars()).any(|(a, c)| c == '?' && a != '?') {
            return Err(EditError::Invalid(format!("\"{text}\" has characters neither {} nor {base} can show", b.base_font)));
        }
        let mut f = Dict::new();
        f.set(b"Type".to_vec(), Object::name("Font"));
        f.set(b"Subtype".to_vec(), Object::name("Type1"));
        f.set(b"BaseFont".to_vec(), Object::name(base));
        f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        let name = if style.family.is_none() { String::from_utf8_lossy(SUBSTITUTE).into_owned() } else { format!("PCEd{}", base.replace('-', "")) };
        fonts_res.set(name.clone().into_bytes(), Object::Dict(f));
        if style.family.is_none() {
            substituted = Some(base.to_string());
        }
        let shown = name.clone();
        (name, Box::new(move |s: &str| Some(vec![(shown.clone(), pdfcraft_fonts::win_ansi(s))])))
    };
    // Line spacing in text space: the paragraph's own (scaled with the size), or 1.2 × the size.
    let lead = match style.line_spacing {
        Some(m) => size * m.clamp(0.5, 5.0),
        None if members.len() > 1 => (members[0].origin.baseline - members[1].origin.baseline) / k * size / old_size,
        None if o_state.leading > 0.0 => o_state.leading,
        None => size * 1.2,
    };
    let n = pdfcraft_content::num;
    // In its own graphics state, so a new colour (or anything else) stops at the paragraph.
    let mut block_ops = vec![Op::new("q", vec![])];
    // Moved: a translation inside that state.
    if let Some((ax, ay)) = movement {
        block_ops.push(Op::new("cm", vec![n(1.0), n(0.0), n(0.0), n(1.0), n(ax), n(ay)]));
    }
    block_ops.push(Op::new("BT", vec![]));
    let mut state = o_state.clone();
    state.word_spacing = 0.0;
    // The font the next run is shown in: the one the BT sets, until a run of another fallback subset.
    let mut shown_font = show_font.clone();
    state.font = Some((show_font, size));
    if let Some([r, g, bl]) = style.color {
        state.fill = vec![Op::new("rg", vec![n(r), n(g), n(bl)])];
    }
    block_ops.extend(state.ops());
    block_ops.push(Op::new("Tm", o.tm.iter().map(|v| n(*v)).collect()));
    // Alignment: each line's offset from the left edge, in text space.
    let offset = |line: &str| -> f64 {
        let free = (width - advance(line)).max(0.0) / k;
        match style.align {
            Some(crate::added::Align::Center) => free / 2.0,
            Some(crate::added::Align::Right) => free,
            _ => 0.0,
        }
    };
    // Justify: word spacing (single-byte code 32 only) so each line but the last fills the width.
    let single_byte = !reuse || metrics.as_ref().is_some_and(|m| !m.composite);
    let justify = style.align == Some(crate::added::Align::Justify) && single_byte;
    let mut x = 0.0;
    let mut tw_set = 0.0;
    let mut underlines: Vec<(f64, f64, f64)> = Vec::new(); // (x0, x1, y) in text space
    for (i, line) in wrapped.iter().enumerate() {
        let dx = offset(line);
        if i > 0 || dx != 0.0 {
            block_ops.push(Op::new("Td", vec![n(dx - x), n(if i > 0 { -lead } else { 0.0 })]));
        }
        x = dx;
        let spaces = line.matches(' ').count();
        let tw =
            if justify && i + 1 < wrapped.len() && spaces > 0 { (width - advance(line)).max(0.0) / k / o_state.scale / spaces as f64 } else { 0.0 };
        if tw != tw_set {
            block_ops.push(Op::new("Tw", vec![n(tw)]));
            tw_set = tw;
        }
        for (font, bytes) in encode(line).ok_or_else(|| EditError::Invalid(format!("\"{line}\" can't be shown")))? {
            if font != shown_font {
                block_ops.push(Op::new("Tf", vec![Object::name(&font), n(size)]));
                shown_font = font;
            }
            block_ops.push(Op::new("Tj", vec![Object::String(PdfString::literal(bytes))]));
        }
        let w = (advance(line) + tw * spaces as f64 * o_state.scale * k) / k;
        underlines.push((dx, dx + w, -(i as f64) * lead - size * 0.12));
    }
    block_ops.push(Op::new("ET", vec![]));
    if style.underline == Some(true) {
        // In text space (the paragraph's text matrix), in its fill colour.
        let colour: Vec<Op> = state
            .fill
            .iter()
            .map(|f| match f.op.as_slice() {
                b"g" => Op::new("G", f.operands.clone()),
                b"rg" => Op::new("RG", f.operands.clone()),
                b"k" => Op::new("K", f.operands.clone()),
                b"cs" => Op::new("CS", f.operands.clone()),
                b"sc" => Op::new("SC", f.operands.clone()),
                _ => Op::new("SCN", f.operands.clone()),
            })
            .collect();
        block_ops.push(Op::new("q", vec![]));
        block_ops.push(Op::new("cm", o.tm.iter().map(|v| n(*v)).collect()));
        block_ops.extend(colour);
        block_ops.push(Op::new("w", vec![n(size * 0.06)]));
        for (x0, x1, y) in underlines {
            block_ops.push(Op::new("m", vec![n(x0), n(y)]));
            block_ops.push(Op::new("l", vec![n(x1), n(y)]));
        }
        block_ops.push(Op::new("S", vec![]));
        block_ops.push(Op::new("Q", vec![]));
    }
    block_ops.push(Op::new("Q", vec![]));
    // The paragraph's operators go, and so does anything drawn in the same area (a fake-bold
    // second copy, an invisible text layer): the new text is all that may show.
    let member_rects: Vec<[f64; 4]> = members.iter().map(|l| l.rect).collect();
    let mut drop = coincident_ops(&lines, &member_rects);
    drop.extend(members.iter().flat_map(|l| l.ops.iter().copied()));
    // Text shown earlier in the same text object stays first: the paragraph then goes where it
    // was, between two halves of the text object, so it keeps its place in reading order.
    let start = first.ops.first().copied().unwrap_or(o.bt_op);
    let split = ops
        .get(o.bt_op..start)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .any(|(i, op)| matches!(op.op.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"") && !drop.contains(&(o.bt_op + i)));
    // What the text after the paragraph expects: the state at its BT (or where it was split).
    let after = if split { &o.state } else { &o.bt_state };
    block_ops.extend(after.ops().into_iter().filter(|op| !op.is("Tf") || after.font.is_some()));
    if split {
        block_ops.insert(0, Op::new("ET", vec![]));
        block_ops.push(Op::new("BT", vec![]));
        block_ops.push(Op::new("Tm", o.tlm.iter().map(|v| n(*v)).collect()));
    }
    let at = if split { start } else { o.bt_op };
    let new_data = splice(&pieces, &ops, |i| {
        let insert = if i == at { std::mem::take(&mut block_ops) } else { Vec::new() };
        (insert, !drop.contains(&i))
    });
    let contents = rewritten_contents(doc, &streams, new_data);
    if new_font {
        res.set(b"Font".to_vec(), Object::Dict(fonts_res));
    }
    let page_ref = p.obj;
    doc.update_dict(page_ref, |d| {
        d.set(b"Contents".to_vec(), contents);
        if new_font {
            d.set(b"Resources".to_vec(), Object::Dict(res));
        }
    })?;
    Ok(LineEdit { substituted })
}

/// Existing paragraph indexes, excluding streams owned by an added-content item.
pub(crate) fn selectable_blocks(
    doc: &Document,
    page: usize,
    excluded: &std::collections::BTreeSet<pdfcraft_cos::ObjRef>,
) -> Result<Vec<(usize, TextBlock)>, EditError> {
    let p = page_dict(doc, page)?;
    let streams = content_streams(doc, &p.dict);
    let lines = text_lines(doc, page)?;
    Ok(group_blocks(&lines)
        .into_iter()
        .enumerate()
        .filter(|(_, b)| {
            !b.lines
                .iter()
                .filter_map(|i| lines.get(*i))
                .any(|l| streams.get(l.stream).and_then(|(o, _)| o.as_ref()).is_some_and(|r| excluded.contains(&r)))
        })
        .collect())
}

/// Detect vertical CMaps before moving horizontal text. A broken/cyclic embedded CMap is
/// refused rather than guessing which direction TJ advances its text matrix.
fn vertical_encoding(doc: &Document, font: &Dict) -> Result<bool, EditError> {
    if font.name(b"Subtype") != Some(b"Type0") {
        return Ok(false);
    }
    let mut next = font.get(b"Encoding").cloned();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..8 {
        let Some(o) = next.take() else { return Ok(false) };
        if let Some(r) = o.as_ref()
            && !seen.insert(r)
        {
            return Err(EditError::Invalid("the text encoding is cyclic".into()));
        }
        match &*doc.resolve(&o) {
            Object::Name(n) => return Ok(n.ends_with(b"-V")),
            Object::Stream(stream) => {
                if let Some(mode) = stream.dict.get(b"WMode") {
                    match mode.as_int() {
                        Some(1) => return Ok(true),
                        Some(0) => {}
                        _ => return Err(EditError::Invalid("the text writing mode is invalid".into())),
                    }
                }
                let bytes = stream
                    .decoded_within(8 * 1024 * 1024)
                    .map_err(|_| EditError::Invalid("the text encoding cannot be decoded within its size limit".into()))?;
                let ops = parse(&bytes).ops;
                if let Some(mode) = ops.iter().rev().find(|op| op.is("def") && op.name(0) == Some(b"WMode")) {
                    return match mode.num(1) {
                        Some(0.0) => Ok(false),
                        Some(1.0) => Ok(true),
                        _ => Err(EditError::Invalid("the text writing mode is invalid".into())),
                    };
                }
                next = stream
                    .dict
                    .get(b"UseCMap")
                    .cloned()
                    .or_else(|| ops.iter().rev().find(|op| op.is("usecmap")).and_then(|op| op.operands.first().cloned()));
            }
            _ => return Err(EditError::Invalid("the text encoding is invalid".into())),
        }
    }
    Err(EditError::Invalid("the text encoding is nested too deeply".into()))
}

/// Translate paragraphs together without re-encoding glyphs, changing fonts or reflowing.
/// Numeric TJ offsets and text rise temporarily move each selected show operation, then
/// restore both advances and rise. The PDF reader advances the original glyph bytes itself:
/// no approximate font metrics are used to position following, unselected text (§9.4.4).
pub(crate) fn translate_blocks(doc: &mut Document, page: usize, indexes: &[usize], offset: [f64; 2]) -> Result<(), EditError> {
    if indexes.is_empty() {
        return Ok(());
    }
    let p = page_dict(doc, page)?;
    let streams = content_streams(doc, &p.dict);
    let lines = text_lines(doc, page)?;
    let blocks = group_blocks(&lines);
    let mut selected = std::collections::BTreeSet::new();
    for index in indexes {
        let b = blocks.get(*index).ok_or_else(|| EditError::Invalid("a selected paragraph no longer exists".into()))?;
        for line in &b.lines {
            let l = lines.get(*line).ok_or_else(|| EditError::Invalid("a selected text line no longer exists".into()))?;
            selected.extend(l.ops.iter().copied());
        }
    }
    let res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let fonts = res.get(b"Font").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let pieces = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let shown = interpret(doc, &ops, &fonts, &mut HashMap::new(), &mut Carry::new(), None);
    let mut patches = HashMap::new();
    // Keep full precision: a tiny rounded text-space change can be magnified by a CTM,
    // and restoring rise/spacing must not alter following unselected text.
    let n = Object::Real;
    for s in shown.into_iter().filter(|s| selected.contains(&s.op)) {
        if !(0..=3).contains(&s.state.render_mode) {
            return Err(EditError::Invalid("text used as a clipping path cannot be moved in a group".into()));
        }
        let font = fonts
            .get(&s.font)
            .map(|f| doc.resolve(f))
            .and_then(|f| f.as_dict().cloned())
            .ok_or_else(|| EditError::Invalid("a selected paragraph has no font".into()))?;
        if vertical_encoding(doc, &font)? {
            return Err(EditError::Invalid("vertical text cannot be moved in a group".into()));
        }
        let inverse = s.tm.then(&s.state.ctm).invert().ok_or_else(|| EditError::Invalid("a selected paragraph has no area".into()))?;
        let [a, b, c, d, _, _] = inverse.0;
        let (dx, dy) = (a * offset[0] + c * offset[1], b * offset[0] + d * offset[1]);
        let denominator = s.state.size * s.state.scale;
        let shift = if dx == 0.0 {
            0.0
        } else if denominator.abs() > 1e-12 {
            -1000.0 * dx / denominator
        } else {
            return Err(EditError::Invalid("a selected paragraph has no horizontal advance".into()));
        };
        let rise = s.state.rise + dy;
        if ![shift, rise, dx, dy].iter().all(|v| v.is_finite() && v.abs() <= 1e12) {
            return Err(EditError::Invalid("the paragraph move is too large".into()));
        }
        let original = ops.get(s.op).ok_or_else(|| EditError::Invalid("a selected text operation is missing".into()))?;
        let mut replacement = Vec::new();
        let show = if original.is("'") || original.is("\"") {
            if original.is("\"") {
                let spacing = original
                    .operands
                    .get(..2)
                    .filter(|a| a.iter().all(|o| o.as_f64().is_some()))
                    .ok_or_else(|| EditError::Invalid("a text operation has invalid spacing".into()))?;
                for (operator, operand) in ["Tw", "Tc"].into_iter().zip(spacing) {
                    replacement.push(Op::new(operator, vec![operand.clone()]));
                }
            }
            replacement.push(Op::new("T*", vec![]));
            Op::new("Tj", vec![original.operands.last().cloned().ok_or_else(|| EditError::Invalid("a text operation has no string".into()))?])
        } else {
            original.clone()
        };
        if dy != 0.0 {
            replacement.push(Op::new("Ts", vec![n(rise)]));
        }
        if shift != 0.0 {
            replacement.push(Op::new("TJ", vec![Object::Array(vec![n(shift)])]));
        }
        replacement.push(show);
        if shift != 0.0 {
            replacement.push(Op::new("TJ", vec![Object::Array(vec![n(-shift)])]));
        }
        if dy != 0.0 {
            replacement.push(Op::new("Ts", vec![n(s.state.rise)]));
        }
        patches.insert(s.op, replacement);
    }
    if patches.len() != selected.len() {
        return Err(EditError::Invalid("a selected text operation cannot be moved".into()));
    }
    let data = splice(&pieces, &ops, |i| patches.remove(&i).map_or((Vec::new(), true), |v| (v, false)));
    let contents = rewritten_contents(doc, &streams, data);
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraph_wrapping_keeps_ideographic_spaces() {
        let width = |s: &str| s.chars().count() as f64;
        assert_eq!(wrap("A　　B C", 4.0, width), ["A　　B", "C"]);
        assert_eq!(wrap("A\u{a0}B C", 3.0, width), ["A B", "C"]);
        assert_eq!(wrap("A\u{2028}B C", 3.0, width), ["A B", "C"]);
        assert_eq!(wrap(" \tA  B\r\nC ", 3.0, width), ["A B", "C"]);
    }

    #[test]
    fn fallback_face_moves_on_when_the_best_face_lacks_a_character() {
        static JPAN: &[&str] = &["Jpan"];
        let mincho = CraftFont { family: "Mincho", style: "Regular", scripts: JPAN, bytes: b"" };
        let gothic = CraftFont { family: "Gothic", style: "Regular", scripts: JPAN, bytes: b"" };
        let faces = [&mincho, &gothic];
        // Only the Gothic face has the Cyrillic letter.
        let has = |face: &CraftFont, ch: char| ch.is_ascii() || face.family == "Gothic";
        assert_eq!(fallback_face(&faces, &['a', 'ф'], has).map(|f| f.family), Some("Gothic"));
        // Characters the best face has keep it.
        assert_eq!(fallback_face(&faces, &['a', 'b'], has).map(|f| f.family), Some("Mincho"));
        // No face has them all: the best face stays, and names the missing glyph.
        assert_eq!(fallback_face(&faces, &['a', 'ф'], |_, ch| ch.is_ascii()).map(|f| f.family), Some("Mincho"));
        assert!(fallback_face(&[], &['a'], has).is_none());
    }
}
