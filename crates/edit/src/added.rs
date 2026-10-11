//! Edit a PDF ▸ Add content: text and images added as page content (execution plan M7.4).
//!
//! Each added item is its own content stream on the page, tagged `/PCMark /Added`, with its
//! parameters (text, font, size, colour, alignment, box; or image and box) kept in the stream
//! dictionary under `/PCAdded`. That keeps the item editable later (move, resize, retype,
//! reformat, delete) without content-stream surgery, while every viewer sees ordinary page
//! content. Boxes are in display space (points, origin at the bottom-left of the page as shown,
//! after `/UserUnit` and `/Rotate`), so items stay upright on rotated pages; an item records the
//! `/UserUnit` it was written for.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};
use pdfcraft_fonts::{GlyphError, GlyphOutline, ShapedCluster, arabic_glyph, helvetica_width, literal, shape_arabic, win_ansi};
use unicode_bidi::{Level, ParagraphBidiInfo};

use crate::{EditError, check, contents, n, page_list, place_tagged};

const TAG: &str = "Added";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Family {
    #[default]
    Helvetica,
    Times,
    Courier,
}

impl Family {
    pub fn label(self) -> &'static str {
        match self {
            Family::Helvetica => "Helvetica",
            Family::Times => "Times",
            Family::Courier => "Courier",
        }
    }

    /// The standard-14 font name for this family and style.
    pub fn base_font(self, bold: bool, italic: bool) -> &'static str {
        match (self, bold, italic) {
            (Family::Helvetica, false, false) => "Helvetica",
            (Family::Helvetica, true, false) => "Helvetica-Bold",
            (Family::Helvetica, false, true) => "Helvetica-Oblique",
            (Family::Helvetica, true, true) => "Helvetica-BoldOblique",
            (Family::Times, false, false) => "Times-Roman",
            (Family::Times, true, false) => "Times-Bold",
            (Family::Times, false, true) => "Times-Italic",
            (Family::Times, true, true) => "Times-BoldItalic",
            (Family::Courier, false, false) => "Courier",
            (Family::Courier, true, false) => "Courier-Bold",
            (Family::Courier, false, true) => "Courier-Oblique",
            (Family::Courier, true, true) => "Courier-BoldOblique",
        }
    }

    fn from_base(base: &[u8]) -> (Family, bool, bool) {
        let s = String::from_utf8_lossy(base);
        let family = if s.starts_with("Times") {
            Family::Times
        } else if s.starts_with("Courier") {
            Family::Courier
        } else {
            Family::Helvetica
        };
        (family, s.contains("Bold"), s.contains("Italic") || s.contains("Oblique"))
    }

    /// Approximate advance width of `s` (standard-14 metrics are not bundled: Helvetica widths,
    /// scaled for Times; Courier is monospaced).
    pub fn width(self, s: &str, size: f64, bold: bool) -> f64 {
        let w = match self {
            Family::Courier => s.chars().count() as f64 * 0.6 * size,
            Family::Times => helvetica_width(s, size) * 0.9,
            Family::Helvetica => helvetica_width(s, size),
        };
        if bold { w * 1.05 } else { w }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    /// Lines (but the last) stretched to the box's width.
    Justify,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AddedText {
    /// The text box in display space; its width wraps the text and its top is the first line's
    /// top. The height follows the text.
    pub rect: [f64; 4],
    pub text: String,
    pub family: Family,
    pub bold: bool,
    pub italic: bool,
    pub size: f64,
    pub color: [f64; 3],
    pub align: Align,
}

impl Default for AddedText {
    fn default() -> Self {
        AddedText {
            rect: [0.0; 4],
            text: String::new(),
            family: Family::Helvetica,
            bold: false,
            italic: false,
            size: 12.0,
            color: [0.0; 3],
            align: Align::Left,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AddedImage {
    /// Display space.
    pub rect: [f64; 4],
    /// The image XObject.
    pub image: ObjRef,
    /// Quarter turns counter-clockwise (0–3).
    pub rotation: u8,
    pub flip_h: bool,
    pub flip_v: bool,
    /// The fraction trimmed from each side of the image: left, bottom, right, top.
    pub crop: [f64; 4],
}

impl AddedImage {
    pub fn new(rect: [f64; 4], image: ObjRef) -> Self {
        AddedImage { rect, image, rotation: 0, flip_h: false, flip_v: false, crop: [0.0; 4] }
    }

    /// The matrix from image space (the unit square) to display space: crop, flip, rotate,
    /// then fill the box.
    fn matrix(&self) -> pdfcraft_content::Matrix {
        use pdfcraft_content::Matrix as M;
        let [l, b, r, t] = self.crop.map(|v| v.clamp(0.0, 0.45));
        let (cw, ch) = ((1.0 - l - r).max(0.05), (1.0 - b - t).max(0.05));
        let mut m = M([1.0 / cw, 0.0, 0.0, 1.0 / ch, -l / cw, -b / ch]);
        if self.flip_h {
            m = m.then(&M([-1.0, 0.0, 0.0, 1.0, 1.0, 0.0]));
        }
        if self.flip_v {
            m = m.then(&M([1.0, 0.0, 0.0, -1.0, 0.0, 1.0]));
        }
        for _ in 0..self.rotation % 4 {
            // A quarter turn counter-clockwise within the unit square.
            m = m.then(&M([0.0, 1.0, -1.0, 0.0, 1.0, 0.0]));
        }
        let [x0, y0, x1, y1] = norm(self.rect);
        m.then(&M([x1 - x0, 0.0, 0.0, y1 - y0, x0, y0]))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Text(AddedText),
    Image(AddedImage),
}

impl Content {
    pub fn rect(&self) -> [f64; 4] {
        match self {
            Content::Text(t) => t.rect,
            Content::Image(i) => i.rect,
        }
    }

    /// The same item with its box and text size scaled by `factor`.
    fn scaled(self, factor: f64) -> Content {
        match self {
            _ if factor == 1.0 => self,
            Content::Text(t) => Content::Text(AddedText { rect: t.rect.map(|v| v * factor), size: t.size * factor, ..t }),
            Content::Image(i) => Content::Image(AddedImage { rect: i.rect.map(|v| v * factor), ..i }),
        }
    }

    /// The same item moved/resized to `rect` (text keeps its computed height).
    pub fn with_rect(&self, rect: [f64; 4]) -> Content {
        match self {
            Content::Text(t) => Content::Text(AddedText { rect, ..t.clone() }),
            Content::Image(i) => Content::Image(AddedImage { rect, ..i.clone() }),
        }
    }
}

/// An added item on a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Added {
    pub page: usize,
    /// The item's content stream.
    pub obj: ObjRef,
    pub content: Content,
}

fn nums(doc: &Document, o: Option<&Object>) -> Vec<f64> {
    o.map(|o| doc.resolve(o)).and_then(|a| a.as_array().map(|a| a.iter().filter_map(Object::as_f64).collect())).unwrap_or_default()
}

fn norm(r: [f64; 4]) -> [f64; 4] {
    [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]
}

/// The lines of a text item after wrapping to its box width.
pub fn lines(t: &AddedText) -> Vec<String> {
    wrapped(t).into_iter().map(|(line, _)| line).collect()
}

/// [`lines`], each with whether its paragraph runs right to left (its first strong character
/// does).
fn wrapped(t: &AddedText) -> Vec<(String, bool)> {
    let width = (t.rect[2] - t.rect[0]).max(t.size);
    let mut out = Vec::new();
    for para in t.text.split('\n') {
        let arabic = para.chars().any(is_arabic);
        let rtl = arabic && unicode_bidi::get_base_direction(para) == unicode_bidi::Direction::Rtl;
        // Arabic words are shaped one by one (joining stops at spaces), so a line's width is the
        // sum of its words' and shaping stays linear in the paragraph's length. The space is the
        // paragraph's; a space between Latin words is drawn in the item's font, a few hundredths
        // of an em apart.
        let measure = |s: &str| if arabic { arabic_width(t, s, rtl) } else { t.family.width(s, t.size, t.bold) };
        let space = measure(" ");
        let (mut line, mut line_w) = (String::new(), 0.0);
        for word in para.split(' ') {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            let candidate_w = if arabic { if line.is_empty() { measure(word) } else { line_w + space + measure(word) } } else { measure(&candidate) };
            if !line.is_empty() && candidate_w > width {
                out.push((std::mem::take(&mut line), rtl));
                line = word.to_string();
                line_w = measure(word);
            } else {
                line = candidate;
                line_w = candidate_w;
            }
        }
        out.push((line, rtl));
    }
    out
}

/// Arabic letters, marks, digits and punctuation: an item with any of them is shaped with the
/// craft-fonts Arabic face.
fn is_arabic(c: char) -> bool {
    matches!(u32::from(c), 0x0600..=0x06FF | 0x0750..=0x077F | 0x0870..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFC)
}

/// The first character of `t` that would be drawn as `?`: outside WinAnsiEncoding and not drawn
/// with the craft-fonts Arabic face (#125), or Arabic that the face can't draw (or that has no
/// face to draw it). Page-content tools refuse such text; the Add-text editor keeps it open.
pub fn first_undrawable(t: &AddedText) -> Option<char> {
    // Arabic that nothing can draw: there's no face, or the face lacks it.
    if let Some(c) = t.text.chars().find(|c| shaped_arabic(*c) && !pdfcraft_fonts::arabic_has(*c)) {
        return Some(c);
    }
    if pdfcraft_fonts::document_arabic_font().is_none() || !t.text.chars().any(is_arabic) {
        // Drawn line by line in the item's standard font (`\n` splits lines). Without the face
        // that is also how `draw` redraws an existing item, so it's checked that way.
        return t.text.split('\n').find_map(pdfcraft_fonts::first_non_win_ansi);
    }
    // What `arabic_layout` leaves to the standard font, exactly as `draw_arabic` splits it.
    wrapped(t).into_iter().find_map(|(line, rtl)| {
        let (pieces, _) = arabic_layout(t, &line, rtl).ok()?;
        pieces.iter().find_map(|p| match p {
            Piece::Latin(s) => pdfcraft_fonts::first_non_win_ansi(s),
            Piece::Arabic(_) => None,
        })
    })
}

/// An Arabic character the face shapes and draws (U+061C, a direction mark, is dropped).
fn shaped_arabic(c: char) -> bool {
    is_arabic(c) && !is_bidi_control(c)
}

/// Direction marks, embeddings, overrides and isolates: they steer the order and are not drawn.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

fn mirrored(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        other => other,
    }
}

/// A piece of a line with Arabic text, in drawing order.
enum Piece {
    /// Shown in the item's own standard font.
    Latin(String),
    Arabic(Vec<ShapedCluster>),
}

/// A line of an Arabic item in drawing order (the Unicode bidirectional algorithm, UAX #9), and
/// its width at the item's size. `rtl` is its paragraph's direction.
fn arabic_layout(t: &AddedText, line: &str, rtl: bool) -> Result<(Vec<Piece>, f64), EditError> {
    // unicode-bidi indexes the first level of a line, so an empty one would panic.
    if line.is_empty() {
        return Ok((Vec::new(), 0.0));
    }
    let base = if rtl { Level::rtl() } else { Level::ltr() };
    let info = ParagraphBidiInfo::new(line, Some(base));
    let (levels, runs) = info.visual_runs(0..line.len());
    let mut pieces = Vec::new();
    for run in runs {
        let run_rtl = levels.get(run.start).is_some_and(Level::is_rtl);
        // Runs lie on character boundaries; a missing slice is skipped, never a panic.
        let Some(text) = line.get(run) else { continue };
        // Arabic letters take the Arabic face; so do spaces and punctuation between them, which the
        // shaper then mirrors. Latin text (and what the face lacks) keeps the item's font.
        let mut segments: Vec<(bool, String)> = Vec::new();
        for c in text.chars().filter(|c| !is_bidi_control(*c)) {
            let arabic = is_arabic(c) || (run_rtl && !c.is_alphanumeric() && pdfcraft_fonts::arabic_has(c));
            match segments.last_mut() {
                Some((kind, s)) if *kind == arabic => s.push(c),
                _ => segments.push((arabic, c.to_string())),
            }
        }
        if run_rtl {
            segments.reverse();
        }
        for (arabic, s) in segments {
            pieces.push(if arabic {
                Piece::Arabic(shape_arabic(&s, run_rtl).map_err(|e| arabic_error(e, &s))?)
            } else if run_rtl {
                Piece::Latin(s.chars().rev().map(mirrored).collect())
            } else {
                Piece::Latin(s)
            });
        }
    }
    let width = pieces
        .iter()
        .map(|p| match p {
            Piece::Latin(s) => t.family.width(s, t.size, t.bold),
            Piece::Arabic(clusters) => clusters.iter().map(|c| c.advance).sum::<f64>() * t.size,
        })
        .sum();
    Ok((pieces, width))
}

/// The width of `s` in an Arabic item; the item's font's estimate if it can't be shaped (drawing
/// then reports why).
fn arabic_width(t: &AddedText, s: &str, rtl: bool) -> f64 {
    arabic_layout(t, s, rtl).map(|(_, w)| w).unwrap_or_else(|_| t.family.width(s, t.size, t.bold))
}

fn arabic_error(e: GlyphError, text: &str) -> EditError {
    EditError::Invalid(match e {
        GlyphError::NoFont => "Arabic text needs PdfCraft's Arabic font, which this build doesn't include \
                               (to build it in, set CRAFT_FONTS_DIR to a craft-fonts checkout that has an Arab face)"
            .into(),
        GlyphError::Missing => format!("the Arabic font can't show \"{text}\""),
        GlyphError::TooComplex => format!("the Arabic font's glyphs for \"{text}\" are too complex"),
    })
}

/// The box a text item occupies (height from its lines).
pub fn text_rect(t: &AddedText) -> [f64; 4] {
    let r = norm(t.rect);
    let h = lines(t).len().max(1) as f64 * t.size * 1.2;
    [r[0], r[3] - h, r[2], r[3]]
}

fn font_name(base: &str) -> String {
    format!("PCF{}", base.replace('-', ""))
}

/// Most glyphs in one Type 3 font: its codes are single bytes, 1–240. An item gets one such font
/// per 240 distinct glyphs it needs.
const GLYPHS_PER_FONT: usize = 240;

/// A line of an Arabic item after layout: its text, whether its paragraph runs right to left, its
/// pieces in drawing order and its width at the item's size.
type LaidLine = (String, bool, Vec<Piece>, f64);

/// Draw a text item that has Arabic in it. Each line is laid out in display order; Arabic glyphs
/// come from Type 3 fonts drawn from the craft-fonts face's outlines (no font file is embedded),
/// with a ToUnicode map for copy and search. Other text uses the item's standard font.
fn draw_arabic(doc: &mut Document, t: &AddedText, taken: &Dict, fonts: &mut Dict, out: &mut Vec<u8>) -> Result<(), EditError> {
    let laid = wrapped(t)
        .into_iter()
        .map(|(line, rtl)| arabic_layout(t, &line, rtl).map(|(pieces, w)| (line, rtl, pieces, w)))
        .collect::<Result<Vec<_>, _>>()?;
    draw_laid_arabic(doc, t, &laid, taken, fonts, out, arabic_glyph)
}

/// [`draw_arabic`] for lines that are already laid out. `outline` is the source of glyph outlines
/// (the craft-fonts face, or a stand-in in tests).
fn draw_laid_arabic(
    doc: &mut Document,
    t: &AddedText,
    laid: &[LaidLine],
    taken: &Dict,
    fonts: &mut Dict,
    out: &mut Vec<u8>,
    outline: impl Fn(u32) -> Result<GlyphOutline, GlyphError>,
) -> Result<(), EditError> {
    let r = text_rect(t);
    let latin = font_name(t.family.base_font(t.bold, t.italic));
    // Every distinct cluster (a letter with its dots and the text it stands for) is one glyph of
    // one of the fonts, so copy and search get each character exactly once.
    let mut keys: Vec<&ShapedCluster> = Vec::new();
    let mut index: HashMap<ClusterKey, usize> = HashMap::new();
    for c in laid.iter().flat_map(|(_, _, pieces, _)| pieces).flat_map(|p| match p {
        Piece::Arabic(clusters) => clusters.as_slice(),
        Piece::Latin(_) => &[],
    }) {
        index.entry(cluster_key(c)).or_insert_with(|| {
            keys.push(c);
            keys.len() - 1
        });
    }
    // Every outline is read before anything is written, so a failure leaves the document as it was.
    let mut outlines = HashMap::new();
    for c in &keys {
        for (id, _) in &c.glyphs {
            if !outlines.contains_key(id) {
                outlines.insert(*id, outline(*id).map_err(|e| arabic_error(e, &c.text))?);
            }
        }
    }
    // The names the page and this item already use: a new font never takes one of them.
    let mut taken_names: HashSet<Vec<u8>> = taken.iter().chain(fonts.iter()).map(|(name, _)| name.clone()).collect();
    let mut names = Vec::new();
    for chunk in keys.chunks(GLYPHS_PER_FONT) {
        let font = type3_arabic(doc, chunk, &outlines);
        // A name no font on the page has, so items never replace each other's fonts (object
        // numbers alone don't do: a full save renumbers objects but keeps resource names).
        let mut name = format!("PCAr{}", font.num);
        let mut suffix = 0usize;
        while taken_names.contains(name.as_bytes()) {
            suffix = suffix.saturating_add(1);
            name = format!("PCAr{}_{suffix}", font.num);
        }
        taken_names.insert(name.clone().into_bytes());
        fonts.set(name.clone().into_bytes(), Object::Ref(font));
        names.push(name);
    }
    let [cr, cg, cb] = t.color.map(|v| v.clamp(0.0, 1.0));
    out.extend(format!("BT {} {} {} rg\n", n(cr), n(cg), n(cb)).bytes());
    for (i, (line, rtl, pieces, w)) in laid.iter().enumerate() {
        let spaces = line.matches(' ').count();
        // Justified: the spaces of every line but the last stretch to fill the box.
        let tw = if t.align == Align::Justify && i + 1 < laid.len() && spaces > 0 { ((r[2] - r[0]) - w).max(0.0) / spaces as f64 } else { 0.0 };
        let x = match t.align {
            // A right-to-left paragraph's unstretched last line ends at the right edge.
            Align::Justify if *rtl => r[2] - w - tw * spaces as f64,
            Align::Left | Align::Justify => r[0],
            Align::Center => r[0] + ((r[2] - r[0]) - w) / 2.0,
            Align::Right => r[2] - w,
        };
        let y = r[3] - (i as f64 * 1.2 + 0.95) * t.size;
        let mut pen = x;
        let mut current: Option<&str> = None;
        for piece in pieces {
            match piece {
                Piece::Latin(s) => {
                    out.extend(format!("/{latin} {} Tf {} Tw 1 0 0 1 {} {} Tm ", n(t.size), n(tw), n(pen), n(y)).bytes());
                    out.extend(literal(&win_ansi(s)));
                    out.extend_from_slice(b" Tj\n");
                    current = Some(latin.as_str());
                    pen += t.family.width(s, t.size, t.bold) + tw * s.matches(' ').count() as f64;
                }
                Piece::Arabic(clusters) => {
                    for c in clusters {
                        let Some(&k) = index.get(&cluster_key(c)) else { continue };
                        let Some(name) = names.get(k / GLYPHS_PER_FONT) else { continue };
                        // Codes start at 1; k % 240 + 1 is at most 240.
                        let code = u8::try_from(k % GLYPHS_PER_FONT + 1).unwrap_or(1);
                        if current != Some(name.as_str()) {
                            out.extend(format!("/{name} {} Tf ", n(t.size)).bytes());
                            current = Some(name.as_str());
                        }
                        out.extend(format!("1 0 0 1 {} {} Tm ", n(pen), n(y)).bytes());
                        out.extend(literal(&[code]));
                        out.extend_from_slice(b" Tj\n");
                        pen += c.advance * t.size + if c.text == " " { tw } else { 0.0 };
                    }
                }
            }
        }
    }
    out.extend_from_slice(b"ET\n");
    Ok(())
}

/// What makes two clusters the same Type 3 glyph: text, glyphs, offsets and advance (bit for bit;
/// equal clusters come from the same font units).
type ClusterKey<'a> = (&'a str, Vec<(u32, u64, u64)>, u64);

fn cluster_key(c: &ShapedCluster) -> ClusterKey<'_> {
    (c.text.as_str(), c.glyphs.iter().map(|(id, [x, y])| (*id, x.to_bits(), y.to_bits())).collect(), c.advance.to_bits())
}

/// A Type 3 font with `clusters` as codes 1…, each drawn from the Arabic face's glyph `outlines`.
fn type3_arabic(doc: &mut Document, clusters: &[&ShapedCluster], outlines: &HashMap<u32, GlyphOutline>) -> ObjRef {
    let mut charprocs = Dict::new();
    let mut differences = vec![Object::Int(1)];
    let mut widths = Vec::with_capacity(clusters.len());
    let mut bbox = [0.0f64; 4];
    let mut to_unicode = Vec::new();
    for (i, c) in clusters.iter().enumerate() {
        let code = i + 1;
        // Glyph space is 1000 units per em.
        let mut paths = Vec::new();
        let mut ink: Option<[f64; 4]> = None;
        for (id, [dx, dy]) in &c.glyphs {
            let Some(g) = outlines.get(id) else { continue };
            for contour in &g.contours {
                for (j, p) in contour.iter().enumerate() {
                    paths.extend(format!("{} {} {}\n", n((p[0] + dx) * 1000.0), n((p[1] + dy) * 1000.0), if j == 0 { "m" } else { "l" }).bytes());
                }
                paths.extend_from_slice(b"h\n");
                let b = [g.bbox[0] + dx, g.bbox[1] + dy, g.bbox[2] + dx, g.bbox[3] + dy];
                ink = Some(ink.map_or(b, |a| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]));
            }
        }
        // Rounded outwards so no ink is clipped.
        let [x0, y0, x1, y1] =
            ink.map_or([0.0; 4], |b| [(b[0] * 1000.0).floor(), (b[1] * 1000.0).floor(), (b[2] * 1000.0).ceil(), (b[3] * 1000.0).ceil()]);
        bbox = [bbox[0].min(x0), bbox[1].min(y0), bbox[2].max(x1), bbox[3].max(y1)];
        let mut proc = format!("{} 0 {} {} {} {} d1\n", n(c.advance * 1000.0), n(x0), n(y0), n(x1), n(y1)).into_bytes();
        if !paths.is_empty() {
            proc.extend(paths);
            proc.extend_from_slice(b"f\n");
        }
        let glyph = format!("g{code:02X}");
        charprocs.set(glyph.clone().into_bytes(), Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), &proc)))));
        differences.push(Object::name(&glyph));
        widths.push(Object::Real((c.advance * 1000.0).round()));
        if !c.text.is_empty() {
            let hex: String = c.text.encode_utf16().map(|u| format!("{u:04X}")).collect();
            to_unicode.push(format!("<{code:02X}> <{hex}>\n"));
        }
    }
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapType 2 def\n1 begincodespacerange\n<01> <FF>\nendcodespacerange\n",
    );
    // A CMap block holds at most 100 entries.
    for block in to_unicode.chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n{}endbfchar\n", block.len(), block.concat()));
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let cmap = doc.add(Object::Stream(Stream::flate(Dict::new(), cmap.as_bytes())));
    let family = pdfcraft_fonts::document_arabic_font().map_or("Arabic", |f| f.family);
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name(&family.replace(' ', "")));
    descriptor.set(b"FontFamily".to_vec(), Object::String(PdfString::literal(family.as_bytes().to_vec())));
    descriptor.set(b"Flags".to_vec(), Object::Int(4));
    descriptor.set(b"ItalicAngle".to_vec(), Object::Int(0));
    let mut encoding = Dict::new();
    encoding.set(b"Type".to_vec(), Object::name("Encoding"));
    encoding.set(b"Differences".to_vec(), Object::Array(differences));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type3"));
    font.set(b"FontDescriptor".to_vec(), Object::Ref(doc.add(Object::Dict(descriptor))));
    font.set(b"FontBBox".to_vec(), Object::Array(bbox.iter().map(|v| Object::Real(*v)).collect()));
    font.set(
        b"FontMatrix".to_vec(),
        Object::Array(vec![Object::Real(0.001), Object::Int(0), Object::Int(0), Object::Real(0.001), Object::Int(0), Object::Int(0)]),
    );
    font.set(b"FirstChar".to_vec(), Object::Int(1));
    font.set(b"LastChar".to_vec(), Object::Int(clusters.len() as i64));
    font.set(b"Widths".to_vec(), Object::Array(widths));
    font.set(b"Encoding".to_vec(), Object::Dict(encoding));
    font.set(b"CharProcs".to_vec(), Object::Dict(charprocs));
    font.set(b"ToUnicode".to_vec(), Object::Ref(cmap));
    doc.add(Object::Dict(font))
}

/// Content and resources for an item; `view` maps display space to user space. `taken` holds the
/// page's font names. `existing`: the item is being replaced, so a build without the Arabic face
/// keeps drawing its Arabic as `?`, as before Arabic was supported, rather than refusing to move it.
fn draw(doc: &mut Document, c: &Content, view: [f64; 6], taken: &Dict, existing: bool) -> Result<(Vec<u8>, Dict), EditError> {
    let mut res = Dict::new();
    let mut out = format!("q {} {} {} {} {} {} cm\n", n(view[0]), n(view[1]), n(view[2]), n(view[3]), n(view[4]), n(view[5])).into_bytes();
    match c {
        Content::Text(t) => {
            let base = t.family.base_font(t.bold, t.italic);
            let name = font_name(base);
            let mut font = Dict::new();
            font.set(b"Type".to_vec(), Object::name("Font"));
            font.set(b"Subtype".to_vec(), Object::name("Type1"));
            font.set(b"BaseFont".to_vec(), Object::name(base));
            if t.family != Family::Courier || !t.text.is_ascii() {
                font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
            }
            let mut fonts = Dict::new();
            fonts.set(name.clone().into_bytes(), Object::Dict(font));
            if t.text.chars().any(is_arabic) && (pdfcraft_fonts::document_arabic_font().is_some() || !existing) {
                draw_arabic(doc, t, taken, &mut fonts, &mut out)?;
                res.set(b"Font".to_vec(), Object::Dict(fonts));
                out.extend_from_slice(b"Q\n");
                return Ok((out, res));
            }
            res.set(b"Font".to_vec(), Object::Dict(fonts));
            let r = text_rect(t);
            let [cr, cg, cb] = t.color.map(|v| v.clamp(0.0, 1.0));
            out.extend(format!("BT /{name} {} Tf {} {} {} rg\n", n(t.size), n(cr), n(cg), n(cb)).bytes());
            for (i, line) in lines(t).iter().enumerate() {
                let w = t.family.width(line, t.size, t.bold);
                let x = match t.align {
                    Align::Left | Align::Justify => r[0],
                    Align::Center => r[0] + ((r[2] - r[0]) - w) / 2.0,
                    Align::Right => r[2] - w,
                };
                // Baseline: 0.8 em below the line top.
                let y = r[3] - (i as f64 * 1.2 + 0.95) * t.size;
                // Justified: word spacing makes every line but the last fill the box.
                let all = lines(t);
                let spaces = line.matches(' ').count();
                let tw =
                    if t.align == Align::Justify && i + 1 < all.len() && spaces > 0 { ((r[2] - r[0]) - w).max(0.0) / spaces as f64 } else { 0.0 };
                out.extend(format!("1 0 0 1 {} {} Tm {} Tw ", n(x), n(y), n(tw)).bytes());
                out.extend(literal(&win_ansi(line)));
                out.extend_from_slice(b" Tj\n");
            }
            out.extend_from_slice(b"ET\n");
        }
        Content::Image(i) => {
            if !matches!(&*doc.get(i.image), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image")) {
                return Err(EditError::Invalid("not an image".into()));
            }
            let r = norm(i.rect);
            let name = format!("PCImg{}", i.image.num);
            let mut xo = Dict::new();
            xo.set(name.clone().into_bytes(), Object::Ref(i.image));
            res.set(b"XObject".to_vec(), Object::Dict(xo));
            let [a, b, c, d, e, f] = i.matrix().0;
            // Clip to the box: the cropped-away parts stay hidden.
            out.extend(
                format!(
                    "{} {} {} {} re W n {} {} {} {} {} {} cm /{name} Do\n",
                    n(r[0]),
                    n(r[1]),
                    n(r[2] - r[0]),
                    n(r[3] - r[1]),
                    n(a),
                    n(b),
                    n(c),
                    n(d),
                    n(e),
                    n(f)
                )
                .bytes(),
            );
        }
    }
    out.extend_from_slice(b"Q\n");
    Ok((out, res))
}

fn params(c: &Content) -> Dict {
    let mut d = Dict::new();
    let arr = |v: &[f64]| Object::Array(v.iter().map(|x| Object::Real(*x)).collect());
    match c {
        Content::Text(t) => {
            d.set(b"Kind".to_vec(), Object::name("Text"));
            d.set(b"Text".to_vec(), PdfString::text(&t.text));
            d.set(b"Font".to_vec(), Object::name(t.family.base_font(t.bold, t.italic)));
            d.set(b"Size".to_vec(), Object::Real(t.size));
            d.set(b"Color".to_vec(), arr(&t.color));
            d.set(
                b"Align".to_vec(),
                Object::Int(match t.align {
                    Align::Left => 0,
                    Align::Center => 1,
                    Align::Right => 2,
                    Align::Justify => 3,
                }),
            );
            d.set(b"Rect".to_vec(), arr(&text_rect(t)));
        }
        Content::Image(i) => {
            d.set(b"Kind".to_vec(), Object::name("Image"));
            d.set(b"Image".to_vec(), Object::Ref(i.image));
            d.set(b"Rect".to_vec(), arr(&norm(i.rect)));
            if i.rotation % 4 != 0 {
                d.set(b"Rotate".to_vec(), Object::Int(i64::from(i.rotation % 4) * 90));
            }
            if i.flip_h {
                d.set(b"FlipH".to_vec(), Object::Bool(true));
            }
            if i.flip_v {
                d.set(b"FlipV".to_vec(), Object::Bool(true));
            }
            if i.crop != [0.0; 4] {
                d.set(b"Crop".to_vec(), arr(&i.crop));
            }
        }
    }
    d
}

fn parse(doc: &Document, d: &Dict) -> Option<Content> {
    let r = nums(doc, d.get(b"Rect"));
    let rect: [f64; 4] = r.try_into().ok()?;
    match d.name(b"Kind")? {
        b"Text" => {
            let (family, bold, italic) = Family::from_base(d.name(b"Font").unwrap_or(b"Helvetica"));
            let c = nums(doc, d.get(b"Color"));
            Some(Content::Text(AddedText {
                rect,
                text: d.get(b"Text").and_then(|t| doc.resolve(t).as_string().map(|s| s.to_text())).unwrap_or_default(),
                family,
                bold,
                italic,
                size: d.get(b"Size").and_then(Object::as_f64).unwrap_or(12.0),
                color: if c.len() == 3 { [c[0], c[1], c[2]] } else { [0.0; 3] },
                align: match d.int(b"Align") {
                    Some(1) => Align::Center,
                    Some(2) => Align::Right,
                    Some(3) => Align::Justify,
                    _ => Align::Left,
                },
            }))
        }
        b"Image" => {
            let crop = nums(doc, d.get(b"Crop"));
            Some(Content::Image(AddedImage {
                rect,
                image: d.get(b"Image")?.as_ref()?,
                rotation: (d.int(b"Rotate").unwrap_or(0).rem_euclid(360) / 90) as u8,
                flip_h: matches!(d.get(b"FlipH"), Some(Object::Bool(true))),
                flip_v: matches!(d.get(b"FlipV"), Some(Object::Bool(true))),
                crop: crop.try_into().unwrap_or([0.0; 4]),
            }))
        }
        _ => None,
    }
}

fn validate(c: &Content) -> Result<(), EditError> {
    let r = c.rect();
    if !r.iter().all(|v| v.is_finite()) {
        return Err(EditError::Invalid("invalid position".into()));
    }
    match c {
        Content::Text(t) => {
            if t.text.trim().is_empty() {
                return Err(EditError::Invalid("type some text first".into()));
            }
            if !(t.size.is_finite() && (1.0..=500.0).contains(&t.size)) {
                return Err(EditError::Invalid("the font size must be between 1 and 500 points".into()));
            }
            if (r[2] - r[0]).abs() < 1.0 {
                return Err(EditError::Invalid("the text box is too narrow".into()));
            }
            // Without the Arabic face, `draw` decides for the Arabic letters themselves (#403): a new
            // item is refused with what's missing, and one that already holds Arabic stays movable.
            // Everything else in it is drawn in the standard font, so it still has to fit.
            let undrawable = if t.text.chars().any(shaped_arabic) && pdfcraft_fonts::document_arabic_font().is_none() {
                t.text.split('\n').find_map(|line| line.chars().find(|c| !shaped_arabic(*c) && !pdfcraft_fonts::win_ansi_encodable(*c)))
            } else {
                first_undrawable(t)
            };
            if let Some(c) = undrawable {
                return Err(crate::undrawable(c));
            }
        }
        Content::Image(_) => {
            if (r[2] - r[0]).abs() < 1.0 || (r[3] - r[1]).abs() < 1.0 {
                return Err(EditError::Invalid("the image is too small".into()));
            }
        }
    }
    Ok(())
}

/// Write an item's stream (new, or replacing `obj`) and make its resources available to the page.
fn write(doc: &mut Document, page: usize, c: &Content, obj: Option<ObjRef>) -> Result<ObjRef, EditError> {
    validate(c)?;
    let all = page_list(doc);
    check(&[page], all.len())?;
    let p = &all[page];
    let view = p.view_matrix(doc);
    let user_unit = p.user_unit(doc);
    // The page gets its own copy of its resources with the item's fonts and images added, and
    // without the Arabic fonts of the version it replaces.
    let mut pres = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    if let Some(r) = obj {
        drop_fonts(doc, &mut pres, &own_fonts(doc, r));
    }
    let taken = pres.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
    let (content, res) = draw(doc, c, view, &taken, obj.is_some())?;
    for (k, v) in res.iter() {
        let mut sub = pres.get(k).map(|s| doc.resolve(s)).and_then(|s| s.as_dict().cloned()).unwrap_or_default();
        for (n2, o) in v.as_dict().into_iter().flat_map(|d| d.iter()) {
            sub.set(n2.clone(), o.clone());
        }
        pres.set(k.clone(), Object::Dict(sub));
    }
    doc.update_dict(p.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(pres)))?;
    let mut sd = Dict::new();
    sd.set(b"PCMark".to_vec(), Object::name(TAG));
    let mut added = params(c);
    if user_unit != 1.0 {
        added.set(b"UserUnit".to_vec(), Object::Real(user_unit));
    }
    sd.set(b"PCAdded".to_vec(), Object::Dict(added));
    let stream = Stream::flate(sd, &content);
    match obj {
        Some(r) => {
            doc.set(r, Object::Stream(stream));
            Ok(r)
        }
        None => {
            // place_tagged appends a new stream; recover its reference.
            let p = page_list(doc).swap_remove(page);
            place_tagged(doc, &p, TAG, content.clone(), false)?;
            let p = page_list(doc).swap_remove(page);
            let r = contents(doc, &p)?.last().and_then(Object::as_ref).ok_or_else(|| EditError::Invalid("could not add the content".into()))?;
            doc.set(r, Object::Stream(stream));
            Ok(r)
        }
    }
}

/// Every added item, page by page, in drawing order.
pub fn list_added(doc: &Document) -> Vec<Added> {
    let mut out = Vec::new();
    for (pi, p) in page_list(doc).iter().enumerate() {
        let list: Vec<Object> = match p.dict.get(b"Contents") {
            Some(c) => match &*doc.resolve(c) {
                Object::Array(a) => a.clone(),
                _ => vec![c.clone()],
            },
            None => Vec::new(),
        };
        for o in list {
            let Some(r) = o.as_ref() else { continue };
            let obj = doc.get(r);
            let Some(d) = obj.as_dict() else { continue };
            if d.name(b"PCMark") != Some(TAG.as_bytes()) {
                continue;
            }
            let Some(params) = d.get(b"PCAdded").and_then(|o| doc.resolve(o).as_dict().cloned()) else { continue };
            if let Some(c) = parse(doc, &params) {
                // Display points depend on the page's /UserUnit. An item records the one it was
                // written for; none means 1, as for items written before display space followed
                // /UserUnit.
                let written = params
                    .get(b"UserUnit")
                    .and_then(Object::as_f64)
                    .filter(|u| u.is_finite() && *u >= 1.0)
                    .map_or(1.0, |u| u.min(pdfcraft_model::MAX_USER_UNIT));
                out.push(Added { page: pi, obj: r, content: c.scaled(p.user_unit(doc) / written) });
            }
        }
    }
    out
}

/// Add an item to a page (0-based). Returns its index among the page's added items.
pub fn add_content(doc: &mut Document, page: usize, c: &Content) -> Result<usize, EditError> {
    write(doc, page, c, None)?;
    Ok(list_added(doc).iter().filter(|a| a.page == page).count() - 1)
}

fn find(doc: &Document, page: usize, index: usize) -> Result<Added, EditError> {
    list_added(doc)
        .into_iter()
        .filter(|a| a.page == page)
        .nth(index)
        .ok_or_else(|| EditError::Invalid(format!("page {} has no added item {}", page + 1, index + 1)))
}

/// Replace item `index` of `page` (moved, resized, retyped or reformatted).
pub fn update_content(doc: &mut Document, page: usize, index: usize, c: &Content) -> Result<(), EditError> {
    let a = find(doc, page, index)?;
    if std::mem::discriminant(&a.content) != std::mem::discriminant(c) {
        return Err(EditError::Invalid("an item can't change between text and image".into()));
    }
    write(doc, page, c, Some(a.obj))?;
    Ok(())
}

/// Remove item `index` of `page`.
pub fn delete_content(doc: &mut Document, page: usize, index: usize) -> Result<(), EditError> {
    let a = find(doc, page, index)?;
    let p = page_list(doc).swap_remove(page);
    let list: Vec<Object> = contents(doc, &p)?.into_iter().filter(|o| o.as_ref() != Some(a.obj)).collect();
    let fonts = own_fonts(doc, a.obj);
    // Only the page's own resources carry an item's fonts (`write` gives the page a copy).
    let res = match p.dict.get(b"Resources") {
        Some(Object::Dict(d)) if !fonts.is_empty() => Some(d.clone()),
        _ => None,
    };
    let res = res.map(|mut r| {
        drop_fonts(doc, &mut r, &fonts);
        r
    });
    doc.update_dict(p.obj, |d| {
        d.set(b"Contents".to_vec(), Object::Array(list));
        if let Some(r) = res {
            d.set(b"Resources".to_vec(), Object::Dict(r));
        }
    })?;
    Ok(())
}

/// The Type 3 fonts an Arabic item's stream shows text with. Each item has its own (`draw_arabic`
/// names them so), so they go when the item is replaced or deleted.
fn own_fonts(doc: &Document, obj: ObjRef) -> Vec<Vec<u8>> {
    let Object::Stream(s) = &*doc.get(obj) else { return Vec::new() };
    let Ok(data) = s.decoded() else { return Vec::new() };
    data.split(u8::is_ascii_whitespace).filter_map(|w| w.strip_prefix(b"/")).filter(|w| w.starts_with(b"PCAr")).map(<[u8]>::to_vec).collect()
}

fn drop_fonts(doc: &Document, res: &mut Dict, names: &[Vec<u8>]) {
    if names.is_empty() {
        return;
    }
    let Some(mut fonts) = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()) else { return };
    for name in names {
        fonts.remove(name);
    }
    res.set(b"Font".to_vec(), Object::Dict(fonts));
}

#[cfg(test)]
mod arabic_tests;
