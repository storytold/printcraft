//! Text extraction (bootstrap for `pdfcraft-text`, architecture §8).
//!
//! A hayro `Device` that ignores paint and records every glyph: its Unicode value (ToUnicode →
//! encoding → glyph name fallbacks, handled by hayro) and its box in *page view space*, i.e.
//! points with the origin at the top-left of the displayed page, rotation applied, y down. The UI
//! maps view space to the screen with a single scale.
//!
//! On top of the glyph list: line grouping (baseline clustering), word gaps, plain-text output,
//! case-insensitive search and reading-order selection.

use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint, PathDrawMode, SoftMask, interpret_page,
};
use kurbo::{Affine, BezPath, Rect, Shape};

/// One glyph on the page.
#[derive(Clone, Debug, PartialEq)]
pub struct TextGlyph {
    /// The glyph's Unicode text: one character, or several for a ligature.
    pub text: String,
    /// Box in page view space (points, y down): [x0, y0, x1, y1]. The bounding box of `quad`.
    pub rect: [f32; 4],
    /// The glyph's em box in page view space, as it is drawn (rotated, skewed or mirrored): the
    /// start and end of its edge towards the next line, then the end and start of the opposite
    /// edge. `quad[0]` → `quad[1]` runs in the writing direction.
    pub quad: [[f32; 2]; 4],
    /// Unit vector of the writing direction in page view space (`[1, 0]`: left to right).
    pub direction: [f32; 2],
}

/// `s` folded for case-insensitive search, one character for one character.
///
/// The dotted and dotless i of Turkish and Azerbaijani (`İ` U+0130, `ı` U+0131) and the Latin
/// `I`/`i` all fold to `i`. `str::to_lowercase` follows no locale: it turns `İ` into `i` plus a
/// combining dot (two characters) and leaves `ı` alone, so neither `İSTANBUL` nor `IRMAK` could
/// be found by typing `istanbul` or `ırmak`. Search can't know the document's language, so the
/// four are treated as one letter; this only adds matches where an `ı` or `İ` is involved.
fn fold_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'I' | 'i' | '\u{130}' | '\u{131}' => out.push('i'),
            _ => out.extend(c.to_lowercase()),
        }
    }
    out
}

/// The text of one page, in content order, with line structure.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageText {
    pub glyphs: Vec<TextGlyph>,
    /// For each glyph: its line index (lines are in reading order, top to bottom).
    pub line_of: Vec<u32>,
    /// `true` when a word break precedes the glyph on the same line.
    pub space_before: Vec<bool>,
}

impl PageText {
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    /// Text of glyphs `range` with spaces and line breaks reconstructed.
    pub fn text_of(&self, range: std::ops::Range<usize>) -> String {
        let mut s = String::new();
        let mut prev_line = None;
        for i in range.clone() {
            let Some(g) = self.glyphs.get(i) else { break };
            if i != range.start {
                if prev_line != Some(self.line_of[i]) {
                    s.push('\n');
                } else if self.space_before[i] {
                    s.push(' ');
                }
            }
            s.push_str(&g.text);
            prev_line = Some(self.line_of[i]);
        }
        s
    }

    pub fn plain_text(&self) -> String {
        self.text_of(0..self.glyphs.len())
    }

    /// Case-insensitive search. Returns glyph ranges of every match (words may span line breaks,
    /// which count as a single space).
    pub fn find(&self, needle: &str) -> Vec<std::ops::Range<usize>> {
        self.find_opts(needle, false, false)
    }

    /// Search with Acrobat's find options: case-sensitive, whole words only.
    pub fn find_opts(&self, needle: &str, case_sensitive: bool, whole_words: bool) -> Vec<std::ops::Range<usize>> {
        let fold = |s: &str| if case_sensitive { s.to_string() } else { fold_case(s) };
        let needle: Vec<char> = fold(needle).split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
        if needle.is_empty() {
            return Vec::new();
        }
        // Flatten to chars with the glyph index each char came from.
        let mut chars: Vec<(char, usize)> = Vec::new();
        for (i, g) in self.glyphs.iter().enumerate() {
            if i > 0 && (self.space_before[i] || self.line_of[i] != self.line_of[i - 1]) && chars.last().is_some_and(|c| c.0 != ' ') {
                chars.push((' ', i));
            }
            for c in fold(&g.text).chars() {
                let c = if c.is_whitespace() { ' ' } else { c };
                // The query collapses a run of whitespace to one space, so collapse the page too.
                if c == ' ' && chars.last().is_some_and(|k| k.0 == ' ') {
                    continue;
                }
                chars.push((c, i));
            }
        }
        let word = |k: Option<&(char, usize)>| k.is_some_and(|c| c.0.is_alphanumeric());
        let mut out = Vec::new();
        let mut i = 0;
        while i + needle.len() <= chars.len() {
            let bounded = !whole_words || (!word(i.checked_sub(1).and_then(|p| chars.get(p))) && !word(chars.get(i + needle.len())));
            if bounded && chars[i..i + needle.len()].iter().map(|c| c.0).eq(needle.iter().copied()) {
                let start = chars[i].1;
                let end = chars[i + needle.len() - 1].1 + 1;
                out.push(start..end);
                i += needle.len();
            } else {
                i += 1;
            }
        }
        out
    }

    /// Glyph ranges of the matches a character matcher finds in the page text (words and lines
    /// separated by one space, original case). Used for pattern search (Search & Redact).
    pub fn find_with(&self, matcher: impl Fn(&[char]) -> Vec<std::ops::Range<usize>>) -> Vec<std::ops::Range<usize>> {
        let mut chars: Vec<char> = Vec::new();
        let mut owner: Vec<usize> = Vec::new();
        for (i, g) in self.glyphs.iter().enumerate() {
            if i > 0 && (self.space_before[i] || self.line_of[i] != self.line_of[i - 1]) && chars.last().is_some_and(|c| *c != ' ') {
                chars.push(' ');
                owner.push(i);
            }
            for c in g.text.chars() {
                chars.push(if c.is_whitespace() { ' ' } else { c });
                owner.push(i);
            }
        }
        matcher(&chars).into_iter().filter(|r| r.start < r.end && r.end <= chars.len()).map(|r| owner[r.start]..owner[r.end - 1] + 1).collect()
    }

    /// Index of the glyph nearest to a view-space point (for selection anchors).
    pub fn nearest(&self, x: f32, y: f32) -> Option<usize> {
        self.glyphs
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let dx = if x < g.rect[0] {
                    g.rect[0] - x
                } else if x > g.rect[2] {
                    x - g.rect[2]
                } else {
                    0.0
                };
                let dy = if y < g.rect[1] {
                    g.rect[1] - y
                } else if y > g.rect[3] {
                    y - g.rect[3]
                } else {
                    0.0
                };
                // Prefer the same line: vertical distance weighs more.
                (i, dx + dy * 3.0)
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// The word around glyph `i` as its first and last glyph (double-click): it stops at word
    /// gaps, blank glyphs and line ends.
    pub fn word_at(&self, i: usize) -> Option<(usize, usize)> {
        self.glyphs.get(i)?;
        let blank = |k: usize| self.glyphs.get(k).is_none_or(|g| g.text.trim().is_empty());
        // Glyph `k` (≥ 1) follows glyph `k - 1` with no word gap or line break between them.
        let joined = |k: usize| !self.space_before.get(k).copied().unwrap_or(true) && self.line_of.get(k - 1) == self.line_of.get(k);
        let mut first = i;
        while first > 0 && joined(first) && !blank(first - 1) {
            first -= 1;
        }
        let mut last = i;
        while joined(last + 1) && !blank(last + 1) {
            last += 1;
        }
        Some((first, last))
    }

    /// The line around glyph `i` as its first and last glyph (triple-click). A line's glyphs are
    /// consecutive.
    pub fn line_at(&self, i: usize) -> Option<(usize, usize)> {
        self.glyphs.get(i)?;
        let line = self.line_of.get(i)?;
        let same = |k: usize| self.line_of.get(k) == Some(line);
        let mut first = i;
        while first > 0 && same(first - 1) {
            first -= 1;
        }
        let mut last = i;
        while same(last + 1) {
            last += 1;
        }
        Some((first, last))
    }

    /// Merge the boxes of `range` into one rectangle per line (for highlighting).
    pub fn line_rects(&self, range: std::ops::Range<usize>) -> Vec<[f32; 4]> {
        self.merged_rects(range)
    }

    /// One rectangle per run of consecutive glyphs on one line, for glyph indices in ascending
    /// order (a reading-order range, or [`Self::glyphs_in`]). A run stops at a skipped glyph, so
    /// the rectangle never covers text that is not selected.
    pub fn glyph_rects(&self, glyphs: &[usize]) -> Vec<[f32; 4]> {
        self.merged_rects(glyphs.iter().copied())
    }

    fn merged_rects(&self, glyphs: impl IntoIterator<Item = usize>) -> Vec<[f32; 4]> {
        let mut out: Vec<(u32, usize, [f32; 4])> = Vec::new();
        for i in glyphs {
            let (Some(g), Some(&line)) = (self.glyphs.get(i), self.line_of.get(i)) else { break };
            match out.last_mut() {
                Some((l, last, r)) if *l == line && last.checked_add(1) == Some(i) => {
                    *last = i;
                    union(r, &g.rect);
                }
                _ => out.push((line, i, g.rect)),
            }
        }
        out.into_iter().map(|(_, _, r)| r).collect()
    }

    /// The glyphs whose centre lies inside `rect` (view space, corners in either order), in
    /// reading order: what a column selection covers (Alt/Option-drag, Acrobat's column select).
    /// A glyph only partly inside counts when its centre is, so a rectangle drawn a little into
    /// the next column does not pick up the edge of its text.
    pub fn glyphs_in(&self, rect: [f32; 4]) -> Vec<usize> {
        let (x0, x1) = (rect[0].min(rect[2]), rect[0].max(rect[2]));
        let (y0, y1) = (rect[1].min(rect[3]), rect[1].max(rect[3]));
        let inside = |g: &TextGlyph| {
            let (cx, cy) = ((g.rect[0] + g.rect[2]) / 2.0, (g.rect[1] + g.rect[3]) / 2.0);
            (x0..=x1).contains(&cx) && (y0..=y1).contains(&cy)
        };
        self.glyphs.iter().enumerate().filter(|(_, g)| inside(g)).map(|(i, _)| i).collect()
    }

    /// The text of a column selection ([`Self::glyphs_in`]) as it looks on the page: one row per
    /// visual line, top to bottom. Pieces of different lines that sit side by side (table cells,
    /// newspaper columns) share a row and are separated by a tab, so a pasted table lands in
    /// spreadsheet cells. Rows of right-to-left text put their pieces right to left. When any
    /// selected glyph does not run horizontally (CJK vertical writing, turned text, a page shown
    /// turned by `/Rotate`) rows mean nothing, and the lines come out one per row in reading order
    /// instead.
    pub fn column_text(&self, glyphs: &[usize]) -> String {
        // The selected glyphs of each line, with their box: (glyphs, bbox).
        let mut lines: Vec<(Vec<usize>, [f32; 4])> = Vec::new();
        let mut line_ids: Vec<u32> = Vec::new();
        for &i in glyphs {
            let (Some(g), Some(&line)) = (self.glyphs.get(i), self.line_of.get(i)) else { continue };
            match (lines.last_mut(), line_ids.last()) {
                (Some((members, bb)), Some(&l)) if l == line => {
                    members.push(i);
                    union(bb, &g.rect);
                }
                _ => {
                    lines.push((vec![i], g.rect));
                    line_ids.push(line);
                }
            }
        }
        // Within the layout's own tolerance for reading a glyph as unturned.
        let vertical = glyphs.iter().any(|&i| self.glyphs.get(i).is_some_and(|g| g.direction[1].abs() > WORD_GAP));
        let piece = |members: &[usize]| {
            let mut s = String::new();
            let mut prev: Option<usize> = None;
            for &i in members {
                let Some(g) = self.glyphs.get(i) else { continue };
                let gap = self.space_before.get(i).copied().unwrap_or(false) || prev.and_then(|p| p.checked_add(1)) != Some(i);
                if prev.is_some() && gap {
                    s.push(' ');
                }
                s.push_str(&g.text);
                prev = Some(i);
            }
            s
        };
        if vertical {
            return lines.iter().map(|(members, _)| piece(members)).collect::<Vec<_>>().join("\n");
        }
        // Rows: a line joins the row whose first line its vertical centre falls within.
        let mut order: Vec<usize> = (0..lines.len()).collect();
        order.sort_by(|a, b| {
            let cy = |k: usize| lines.get(k).map_or(0.0, |(_, bb)| (bb[1] + bb[3]) / 2.0);
            cy(*a).total_cmp(&cy(*b))
        });
        let mut rows: Vec<(f32, f32, Vec<usize>)> = Vec::new();
        for k in order {
            let Some((_, bb)) = lines.get(k) else { continue };
            let cy = (bb[1] + bb[3]) / 2.0;
            match rows.last_mut() {
                Some((top, bottom, members)) if cy >= *top && cy <= *bottom => members.push(k),
                _ => rows.push((bb[1], bb[3], vec![k])),
            }
        }
        let rtl = |members: &[usize]| members.iter().any(|&i| self.glyphs.get(i).is_some_and(|g| g.text.chars().any(is_rtl)));
        let mut out = Vec::with_capacity(rows.len());
        for (_, _, mut members) in rows {
            let right_to_left = members.iter().all(|&k| lines.get(k).is_some_and(|(m, _)| rtl(m)));
            members.sort_by(|a, b| {
                let x = |k: usize| lines.get(k).map_or(0.0, |(_, bb)| bb[0]);
                if right_to_left { x(*b).total_cmp(&x(*a)) } else { x(*a).total_cmp(&x(*b)) }
            });
            let cells: Vec<String> = members.iter().filter_map(|&k| lines.get(k)).map(|(m, _)| piece(m)).collect();
            out.push(cells.join("\t"));
        }
        out.join("\n")
    }
}

/// Largest sine of the angle between a glyph's writing direction and a flow's for the glyph to be
/// laid out in that flow's frame. Turning an em box by θ moves its edges along the line by up to
/// h·sin θ, so below the layout's word-gap tolerance (`WORD_GAP` · h) the boxes still read as they
/// would unturned.
const DIRECTION_TOLERANCE: f64 = WORD_GAP as f64;

/// A word gap is wider than the line's typical letter gap by this fraction of the line height.
const WORD_GAP: f32 = 0.15;

/// A glyph with its writing direction to the nearest quarter turn (right, down, left, up) and as
/// a unit vector.
type Drawn = (usize, [f64; 2], TextGlyph);

struct TextDevice {
    /// Every glyph in content order.
    glyphs: Vec<Drawn>,
}

impl<'a> Device<'a> for TextDevice {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(&mut self, glyph: &Glyph<'a>, transform: Affine, glyph_transform: Affine, _: &Paint<'a>, _: &GlyphDrawMode) {
        let Some(u) = glyph.as_unicode() else {
            if std::env::var_os("PDFCRAFT_TEXT_DEBUG").is_some() {
                eprintln!("NOUNICODE {} {:?}", if matches!(glyph, Glyph::Type3(_)) { "t3" } else { "ol" }, (transform * glyph_transform).as_coeffs());
            }
            return;
        };
        let text = match u {
            hayro::hayro_interpret::hayro_cmap::BfString::Char(c) => c.to_string(),
            hayro::hayro_interpret::hayro_cmap::BfString::String(s) => s,
        };
        if text.chars().all(|c| c.is_control()) {
            return;
        }
        // Glyph space → view space. Glyph space uses 1000 units per em for outline glyphs; the em
        // box spans descender (−200) to ascender (800), the advance gives the width.
        let t = transform * glyph_transform;
        if std::env::var_os("PDFCRAFT_TEXT_DEBUG").is_some() {
            let kind = if matches!(glyph, Glyph::Type3(_)) { "t3" } else { "ol" };
            eprintln!("{kind} {text:?} transform={:?} glyph_transform={:?}", transform.as_coeffs(), glyph_transform.as_coeffs());
        }
        let advance = match glyph {
            Glyph::Outline(g) => {
                g.advance_width().filter(|a| *a > 0.0).map(f64::from).unwrap_or_else(|| g.outline().bounding_box().width().max(500.0))
            }
            Glyph::Type3(g) => g.advance_width().filter(|a| a.is_finite() && *a > 0.0).map(f64::from).unwrap_or(600.0),
        };
        // The baseline direction in view space (y down).
        let coefficients = t.as_coeffs();
        let (dx, dy) = if matches!(glyph, Glyph::Outline(g) if g.is_vertical()) {
            // PDF vertical fonts advance along negative glyph-space y (WMode 1), while
            // keeping the glyph outline upright. Its x axis is not the writing direction.
            (-coefficients[2], -coefficients[3])
        } else {
            (coefficients[0], coefficients[1])
        };
        let dir = if dx.abs() >= dy.abs() {
            if dx >= 0.0 { 0 } else { 2 }
        } else if dy > 0.0 {
            1
        } else {
            3
        };
        let em = Rect::new(0.0, -200.0, advance, 800.0);
        let corners = if matches!(glyph, Glyph::Outline(g) if g.is_vertical()) {
            [(em.x0, em.y1), (em.x0, em.y0), (em.x1, em.y0), (em.x1, em.y1)]
        } else {
            [(em.x0, em.y0), (em.x1, em.y0), (em.x1, em.y1), (em.x0, em.y1)]
        }
        .map(|(x, y)| t * kurbo::Point::new(x, y));
        let b = corners.iter().fold(Rect::new(f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY), |b, p| {
            Rect::new(b.x0.min(p.x), b.y0.min(p.y), b.x1.max(p.x), b.y1.max(p.y))
        });
        if !(b.x0.is_finite() && b.y0.is_finite() && b.x1.is_finite() && b.y1.is_finite()) || b.width() > 10_000.0 || b.height() > 10_000.0 {
            return;
        }
        let length = dx.hypot(dy);
        let direction = if length.is_finite() && length > 0.0 { [dx / length, dy / length] } else { [1.0, 0.0] };
        // A glyph whose Unicode is several characters (a ligature such as "ffi") stays one glyph
        // with its own box: the PDF gives no position for the characters inside it.
        let glyph = TextGlyph {
            text,
            rect: [b.x0 as f32, b.y0 as f32, b.x1 as f32, b.y1 as f32],
            quad: corners.map(|p| [p.x as f32, p.y as f32]),
            direction: direction.map(|d| d as f32),
        };
        self.glyphs.push((dir, direction, glyph));
    }
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

/// Extract the text of one page. Panics inside the interpreter are the caller's to catch.
pub(crate) fn extract_page(pdf: &Pdf, page: usize, settings: &InterpreterSettings) -> Option<PageText> {
    let pages = pdf.pages();
    let p = pages.get(page)?;
    let (w, h) = p.render_dimensions();
    let cache = InterpreterCache::new();
    // Annotation appearances are included: form-field values and comment text are searchable,
    // as in Acrobat, and match what other extractors report.
    let settings = settings.clone();
    let initial = p.initial_transform(true).to_kurbo();
    let mut ctx = Context::new(initial, Rect::new(0.0, 0.0, w as f64, h as f64), &cache, p.xref(), settings);
    let mut dev = TextDevice { glyphs: Vec::new() };
    interpret_page(p, &mut ctx, &mut dev);
    let glyphs = dev.glyphs;
    // Text on a curved path: glyphs that each continue the previous one's line while the
    // direction turns. Each such run is one line.
    let turned = |u: [f64; 2]| u[0].abs().min(u[1].abs()) > DIRECTION_TOLERANCE;
    let mut in_curve = vec![false; glyphs.len()];
    // Turned text: curved runs, and straight flows along one writing direction. The two are kept
    // apart so finding a glyph's direction scans only the directions (a few dozen at most: each
    // differs from the others by more than the tolerance), not every curved run.
    let mut curves: Vec<Vec<usize>> = Vec::new();
    let mut straight: Vec<([f64; 2], Vec<usize>)> = Vec::new();
    let mut start = 0;
    for end in 1..=glyphs.len() {
        if end < glyphs.len() && continues(&glyphs[end - 1], &glyphs[end]) {
            continue;
        }
        let run = &glyphs[start..end];
        if run.iter().any(|g| turned(g.1)) && run.iter().any(|g| !compatible(run[0].1, g.1)) {
            in_curve[start..end].fill(true);
            curves.push((start..end).collect());
        }
        start = end;
    }
    let mut quarters: [Vec<usize>; 4] = Default::default();
    for (i, (quarter, u, _)) in glyphs.iter().enumerate() {
        if in_curve[i] {
            continue;
        }
        if !turned(*u) {
            if let Some(q) = quarters.get_mut(*quarter) {
                q.push(i);
            }
        } else if let Some((_, ids)) = straight.iter_mut().find(|(d, _)| compatible(*d, *u)) {
            ids.push(i);
        } else {
            straight.push((*u, vec![i]));
        }
    }
    // A horizontal heading must not force a vertical body back into horizontal line grouping.
    // Group each writing direction independently, then join flows from their topmost position.
    let upright = |quarters: [Vec<usize>; 4]| {
        let mut flows: Vec<Flow> = quarters
            .into_iter()
            .enumerate()
            .filter(|(_, ids)| !ids.is_empty())
            .map(|(quarter, ids)| {
                let rects = ids.iter().map(|&i| to_upright(glyphs[i].2.rect, quarter as u8, w, h)).collect();
                arrange_flow(&glyphs, ids, rects, Some(quarter as u8))
            })
            .collect();
        flows.sort_by(|a, b| a.top.total_cmp(&b.top));
        flows
    };
    let flows = upright(quarters);
    let mut extra: Vec<Flow> = curves
        .into_iter()
        .map(|ids| (None, ids))
        .chain(straight.into_iter().map(|(u, ids)| (Some(u), ids)))
        .map(|(u, ids)| {
            let rects = match u {
                // Turned text is laid out along its own baseline.
                Some(u) => ids.iter().map(|&i| along_baseline(&glyphs[i].2.quad, u).map(|c| c as f32)).collect(),
                None => unrolled(ids.iter().map(|&i| &glyphs[i])),
            };
            arrange_flow(&glyphs, ids, rects, None)
        })
        .collect();
    // Turned text takes the place in the reading order where laying it out among the nearest
    // quarter turn's text puts its first glyph.
    let mut rank = Vec::new();
    if !extra.is_empty() {
        rank = vec![usize::MAX; glyphs.len()];
        let mut nearest: [Vec<usize>; 4] = Default::default();
        for (i, g) in glyphs.iter().enumerate() {
            if let Some(q) = nearest.get_mut(g.0) {
                q.push(i);
            }
        }
        for (r, i) in upright(nearest).into_iter().flat_map(|f| f.ids).enumerate() {
            rank[i] = r;
        }
    }
    let first = |ids: &[usize]| ids.iter().filter_map(|&i| rank.get(i).copied()).min().unwrap_or(usize::MAX);
    extra.sort_by_key(|f| first(&f.ids));
    let mut extra = extra.into_iter().peekable();
    let mut slots: Vec<Option<TextGlyph>> = glyphs.into_iter().map(|g| Some(g.2)).collect();
    let mut text = PageText::default();
    let mut emit = |text: &mut PageText, ids: &[usize], line_of: &[u32], space_before: &[bool], quarter: Option<u8>| {
        let offset = text.line_of.last().map_or(0, |line| line.saturating_add(1));
        for ((&i, &line), &space) in ids.iter().zip(line_of).zip(space_before) {
            let Some(mut g) = slots.get_mut(i).and_then(Option::take) else { continue };
            if let Some(quarter) = quarter {
                g.rect = from_upright(to_upright(g.rect, quarter, w, h), quarter, w, h);
            }
            text.glyphs.push(g);
            text.line_of.push(line.saturating_add(offset));
            text.space_before.push(space);
        }
    };
    for flow in flows {
        let mut k = 0;
        while k < flow.ids.len() {
            let line = flow.line_of[k];
            let end = flow.line_of[k..].iter().position(|l| *l != line).map_or(flow.ids.len(), |n| k + n);
            let before = first(&flow.ids[k..end]);
            while let Some(f) = extra.next_if(|f| first(&f.ids) < before) {
                emit(&mut text, &f.ids, &f.line_of, &f.space_before, None);
            }
            emit(&mut text, &flow.ids[k..end], &vec![0; end - k], &flow.space_before[k..end], flow.quarter);
            k = end;
        }
    }
    for f in extra {
        emit(&mut text, &f.ids, &f.line_of, &f.space_before, None);
    }
    Some(text)
}

/// Glyphs in reading order, as indices into the page's glyphs, with their lines and word gaps.
struct Flow {
    /// The top of the flow's topmost glyph in view space.
    top: f32,
    /// The quarter turn whose frame the flow was laid out in.
    quarter: Option<u8>,
    ids: Vec<usize>,
    line_of: Vec<u32>,
    space_before: Vec<bool>,
}

/// Lay out the page's glyphs `ids`, giving each the box at the same position in `rects`.
fn arrange_flow(glyphs: &[Drawn], ids: Vec<usize>, rects: Vec<[f32; 4]>, quarter: Option<u8>) -> Flow {
    let top = ids.iter().filter_map(|&i| glyphs.get(i)).map(|g| g.2.rect[1]).fold(f32::INFINITY, f32::min);
    let framed: Vec<TextGlyph> = ids.iter().zip(rects).filter_map(|(&i, rect)| glyphs.get(i).map(|g| TextGlyph { rect, ..g.2.clone() })).collect();
    let (order, line_of, space_before) = arrange(&framed);
    let ids = order.into_iter().filter_map(|k| ids.get(k).copied()).collect();
    Flow { top, quarter, ids, line_of, space_before }
}

/// Boxes for a run of glyphs set along a curve: each glyph's box along its own direction, placed
/// where the pen moved in the previous glyph's direction. The path straightened out.
fn unrolled<'g>(run: impl Iterator<Item = &'g Drawn>) -> Vec<[f32; 4]> {
    let mut pen = [0.0, 0.0];
    let mut previous: Option<([f64; 2], [f64; 4])> = None;
    run.map(|(_, u, g)| {
        let b = along_baseline(&g.quad, *u);
        if let Some((pu, pb)) = previous {
            let at = along_baseline(&g.quad, pu);
            pen = [pen[0] + at[0] - pb[0], pen[1] + (at[1] + at[3] - pb[1] - pb[3]) / 2.0];
        }
        previous = Some((*u, b));
        let half = (b[3] - b[1]) / 2.0;
        [pen[0], pen[1] - half, pen[0] + b[2] - b[0], pen[1] + half].map(|c| c as f32)
    })
    .collect()
}

/// View-space box → the frame where text runs left to right, for text running `quarter` × 90°
/// clockwise from that in a `w` × `h` view (1: text runs downwards).
fn to_upright(r: [f32; 4], quarter: u8, w: f32, h: f32) -> [f32; 4] {
    let [x0, y0, x1, y1] = r;
    match quarter {
        1 => [y0, w - x1, y1, w - x0],
        2 => [w - x1, h - y1, w - x0, h - y0],
        3 => [h - y1, x0, h - y0, x1],
        _ => r,
    }
}

/// Whether two writing directions are close enough to share a frame ([`DIRECTION_TOLERANCE`]).
fn compatible(a: [f64; 2], b: [f64; 2]) -> bool {
    a[0] * b[0] + a[1] * b[1] > 0.0 && (a[0] * b[1] - a[1] * b[0]).abs() <= DIRECTION_TOLERANCE
}

/// Whether glyph `g` continues the line of glyph `p` (content order), by the test `layout` uses
/// for consecutive glyphs, taken in `p`'s own frame. The direction may turn by less than half a
/// quarter turn, as text set along a curve does.
fn continues(p: &Drawn, g: &Drawn) -> bool {
    let (pu, gu) = (p.1, g.1);
    if (pu[0] * gu[1] - pu[1] * gu[0]).abs() >= pu[0] * gu[0] + pu[1] * gu[1] {
        return false;
    }
    let (pb, gb, own) = (along_baseline(&p.2.quad, pu), along_baseline(&g.2.quad, pu), along_baseline(&g.2.quad, gu));
    let h = (pb[3] - pb[1]).min(own[3] - own[1]).max(0.1);
    let cy = |b: [f64; 4]| (b[1] + b[3]) / 2.0;
    (cy(gb) - cy(pb)).abs() < h * 0.5 && gb[0] - pb[2] < h * 3.0 && gb[0] > pb[0] - h * 2.0
}

/// A view-space quad → its box in the frame where `u` runs left to right and `u` turned a
/// quarter clockwise runs downwards. Along `u` it spans the middles of the quad's start and end
/// sides, so a skewed glyph is no wider than its advance; across, it spans every corner.
fn along_baseline(quad: &[[f32; 2]; 4], u: [f64; 2]) -> [f64; 4] {
    let p = quad.map(|[x, y]| [f64::from(x) * u[0] + f64::from(y) * u[1], f64::from(y) * u[0] - f64::from(x) * u[1]]);
    let (a0, a1) = ((p[0][0] + p[3][0]) / 2.0, (p[1][0] + p[2][0]) / 2.0);
    let (c0, c1) = p.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), q| (lo.min(q[1]), hi.max(q[1])));
    [a0.min(a1), c0, a0.max(a1), c1]
}

/// The inverse of [`to_upright`].
fn from_upright(r: [f32; 4], quarter: u8, w: f32, h: f32) -> [f32; 4] {
    let [u0, v0, u1, v1] = r;
    match quarter {
        1 => [w - v1, u0, w - v0, u1],
        2 => [w - u1, h - v1, w - u0, h - v0],
        3 => [v0, h - u1, v1, h - u0],
        _ => r,
    }
}

use hayro::hayro_interpret::TransformExt;

fn is_rtl(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF)
}

/// Scripts written with explicit spaces whose combining signs can sit apart from their base
/// (Indic, Thai, Lao, Myanmar, Khmer): never infer a space inside a tight cluster.
fn is_complex(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x0900..=0x0DFF | 0x0E00..=0x0EFF | 0x1000..=0x109F | 0x1780..=0x17FF)
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x3134F | 0x1100..=0x11FF)
}

struct Seg {
    idx: Vec<usize>,
    bbox: [f32; 4],
    h: f32,
}

impl Seg {
    fn cy(&self) -> f32 {
        (self.bbox[1] + self.bbox[3]) / 2.0
    }
}

fn union(a: &mut [f32; 4], b: &[f32; 4]) {
    a[0] = a[0].min(b[0]);
    a[1] = a[1].min(b[1]);
    a[2] = a[2].max(b[2]);
    a[3] = a[3].max(b[3]);
}

/// Reading-order layout. Glyphs are reordered so that `glyphs` is in reading order, lines are
/// numbered in that order, and word gaps are inferred per line.
///
/// Heuristics (documented so they can be tuned and tested; plan/architecture.md §8):
/// 1. *Segments:* consecutive glyphs (content order) on one baseline with no column-sized gap.
///    Segments on the same baseline that nearly touch are merged (text drawn out of order).
/// 2. *Blocks:* segments stacked with line-sized gaps and overlapping horizontally.
/// 3. *Block order:* repeatedly take the top-most remaining block, preferring a block to its left
///    that overlaps it vertically (so columns read left, then right).
/// 4. Within a segment glyphs run left to right; runs of right-to-left script are reversed so the
///    text comes out in logical order.
pub fn layout(glyphs: Vec<TextGlyph>) -> PageText {
    let (order, line_of, space_before) = arrange(&glyphs);
    let mut slots: Vec<Option<TextGlyph>> = glyphs.into_iter().map(Some).collect();
    // Each glyph is emitted exactly once.
    let glyphs = order.iter().filter_map(|i| slots.get_mut(*i).and_then(Option::take)).collect();
    PageText { glyphs, line_of, space_before }
}

/// [`layout`] as the reading order of indices into `glyphs`, with each one's line and word gap.
fn arrange(glyphs: &[TextGlyph]) -> (Vec<usize>, Vec<u32>, Vec<bool>) {
    // Drop "fake bold" duplicates: the same character redrawn at (almost) the same place.
    let mut keep = vec![true; glyphs.len()];
    for i in 1..glyphs.len() {
        let g = &glyphs[i];
        let h = (g.rect[3] - g.rect[1]).max(0.1);
        for j in (i.saturating_sub(4)..i).rev() {
            let p = &glyphs[j];
            if keep[j] && p.text == g.text && (p.rect[0] - g.rect[0]).abs() < h * 0.2 && (p.rect[1] - g.rect[1]).abs() < h * 0.2 {
                keep[i] = false;
                break;
            }
        }
    }
    let kept: Vec<usize> = (0..glyphs.len()).filter(|i| keep[*i]).collect();
    let glyphs: Vec<&TextGlyph> = kept.iter().map(|i| &glyphs[*i]).collect();
    let n = glyphs.len();
    if n == 0 {
        return Default::default();
    }
    let height = |i: usize| (glyphs[i].rect[3] - glyphs[i].rect[1]).max(0.1);
    let cy = |i: usize| (glyphs[i].rect[1] + glyphs[i].rect[3]) / 2.0;

    // 1. Segments in content order.
    let mut segs: Vec<Seg> = Vec::new();
    for i in 0..n {
        let g = glyphs[i].rect;
        let cont = segs.last().is_some_and(|s| {
            let Some(&p) = s.idx.last() else { return false };
            let ph = height(p).min(height(i));
            let gap = g[0] - glyphs[p].rect[2];
            (cy(i) - cy(p)).abs() < ph * 0.5 && gap < ph * 3.0 && g[0] > glyphs[p].rect[0] - ph * 2.0
        });
        if cont && let Some(s) = segs.last_mut() {
            s.idx.push(i);
            union(&mut s.bbox, &g);
            s.h = s.h.max(height(i));
        } else {
            segs.push(Seg { idx: vec![i], bbox: g, h: height(i) });
        }
    }
    // Merge same-baseline segments that nearly touch.
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for a in 0..segs.len() {
            for b in 0..segs.len() {
                if a == b {
                    continue;
                }
                let (sa, sb) = (&segs[a], &segs[b]);
                let h = sa.h.min(sb.h);
                let gap = (sb.bbox[0] - sa.bbox[2]).max(sa.bbox[0] - sb.bbox[2]);
                if (sa.cy() - sb.cy()).abs() < h * 0.3 && (sa.h / sb.h - 1.0).abs() < 0.35 && gap < h * 1.2 {
                    let sb = segs.remove(b);
                    let a = if b < a { a - 1 } else { a };
                    let sa = &mut segs[a];
                    sa.idx.extend(sb.idx);
                    union(&mut sa.bbox, &sb.bbox);
                    sa.h = sa.h.max(sb.h);
                    merged = true;
                    break 'outer;
                }
            }
        }
    }
    for s in &mut segs {
        s.idx.sort_by(|a, b| glyphs[*a].rect[0].total_cmp(&glyphs[*b].rect[0]));
    }

    // 2. Blocks.
    let mut by_top: Vec<usize> = (0..segs.len()).collect();
    by_top.sort_by(|a, b| segs[*a].bbox[1].total_cmp(&segs[*b].bbox[1]));
    let mut blocks: Vec<(Vec<usize>, [f32; 4])> = Vec::new();
    for si in by_top {
        let s = &segs[si];
        let target = blocks.iter().position(|(members, bb)| {
            let Some(&m) = members.last() else { return false };
            let last = &segs[m];
            let vgap = s.bbox[1] - last.bbox[3];
            let overlap = s.bbox[2].min(bb[2]) - s.bbox[0].max(bb[0]);
            let minw = (s.bbox[2] - s.bbox[0]).min(bb[2] - bb[0]).max(1.0);
            vgap > -last.h * 0.5 && vgap < last.h.max(s.h) * 1.1 && overlap > minw * 0.3 && (last.h / s.h - 1.0).abs() < 0.6
        });
        match target {
            Some(b) => {
                blocks[b].0.push(si);
                let bb = s.bbox;
                union(&mut blocks[b].1, &bb);
            }
            None => blocks.push((vec![si], s.bbox)),
        }
    }

    // 3. Block order.
    let mut remaining: Vec<usize> = (0..blocks.len()).collect();
    let mut block_order = Vec::with_capacity(blocks.len());
    while let Some(&top) = remaining.iter().min_by(|a, b| blocks[**a].1[1].total_cmp(&blocks[**b].1[1])) {
        let tb = blocks[top].1;
        let pick = remaining
            .iter()
            .copied()
            .filter(|c| {
                let cb = blocks[*c].1;
                let v = cb[3].min(tb[3]) - cb[1].max(tb[1]);
                cb[2] <= tb[0] + 1.0 && v > 0.5 * (cb[3] - cb[1]).min(tb[3] - tb[1])
            })
            .min_by(|a, b| blocks[*a].1[0].total_cmp(&blocks[*b].1[0]))
            .unwrap_or(top);
        remaining.retain(|r| *r != pick);
        block_order.push(pick);
    }

    // 4. Emit glyphs in reading order with line numbers and word gaps.
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut line_of = Vec::with_capacity(n);
    let mut space_before = Vec::with_capacity(n);
    let mut line = 0u32;
    for b in block_order {
        for si in &blocks[b].0 {
            let s = &segs[*si];
            let mut idx = s.idx.clone();
            // Reverse right-to-left runs (visual → logical).
            let rtl = |i: usize| glyphs[i].text.chars().any(is_rtl);
            let mut k = 0;
            while k < idx.len() {
                if rtl(idx[k]) {
                    let mut e = k;
                    while e + 1 < idx.len()
                        && (rtl(idx[e + 1]) || (glyphs[idx[e + 1]].text.trim().is_empty() && e + 2 < idx.len() && rtl(idx[e + 2])))
                    {
                        e += 1;
                    }
                    idx[k..=e].reverse();
                    k = e + 1;
                } else {
                    k += 1;
                }
            }
            // Word gaps relative to this line's typical letter gap (handles tracking).
            let mut gaps: Vec<f32> = s.idx.windows(2).map(|w| glyphs[w[1]].rect[0] - glyphs[w[0]].rect[2]).collect();
            gaps.sort_by(f32::total_cmp);
            let typical = gaps.get(gaps.len() / 3).copied().unwrap_or(0.0).max(0.0);
            let threshold = (typical + s.h * WORD_GAP).max(s.h * WORD_GAP);
            let mut spaces = vec![false; idx.len()];
            for w in 1..s.idx.len() {
                let (p, c) = (s.idx[w - 1], s.idx[w]);
                let gap = glyphs[c].rect[0] - glyphs[p].rect[2];
                let (pt, ct) = (&glyphs[p].text, &glyphs[c].text);
                let cjk = pt.chars().any(is_cjk) && ct.chars().any(is_cjk) && gap < s.h * 0.5;
                let tight_cluster = (pt.chars().any(is_complex) || ct.chars().any(is_complex)) && gap < s.h * 0.6;
                if gap > threshold && !cjk && !tight_cluster && !pt.trim().is_empty() && !ct.trim().is_empty() {
                    // Mark the space before glyph `c` wherever it ended up after RTL reversal.
                    if let Some(pos) = idx.iter().position(|x| *x == c.max(p)) {
                        spaces[pos] = true;
                    }
                }
            }
            for (k, g) in idx.iter().enumerate() {
                order.push(*g);
                line_of.push(line);
                space_before.push(k > 0 && spaces[k]);
            }
            line += 1;
        }
    }
    (order.into_iter().map(|i| kept[i]).collect(), line_of, space_before)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_v_columns_are_searchable_in_reading_order() {
        use lopdf::{Document, Object, Stream, dictionary};
        for encoding in ["Identity-V", "UniJIS-UCS2-V"] {
            let mut doc = Document::with_version("1.7");
            let cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def /CMapName /TestUnicode def /CMapType 2 def 1 begincodespacerange <0000> <FFFF> endcodespacerange 5 beginbfchar <0001> <65E5> <0002> <672C> <0003> <8A9E> <0004> <6587> <0005> <7AE0> endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
            let cmap = if encoding == "Identity-V" {
                cmap.to_vec()
            } else {
                let mut mapping = String::from_utf8(cmap.to_vec()).unwrap();
                for (code, unicode) in [("0001", "65E5"), ("0002", "672C"), ("0003", "8A9E"), ("0004", "6587"), ("0005", "7AE0")] {
                    mapping = mapping.replace(&format!("<{code}> <{unicode}>"), &format!("<{unicode}> <{unicode}>"));
                }
                mapping.into_bytes()
            };
            let unicode = doc.add_object(Stream::new(dictionary! {}, cmap));
            let cid = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => "TestVertical",
            "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Identity"), "Supplement" => 0 },
            "DW" => 1000, "DW2" => vec![Object::Integer(880), Object::Integer(-1000)], "CIDToGIDMap" => "Identity"
        });
            let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "TestVertical", "Encoding" => encoding, "DescendantFonts" => vec![Object::Reference(cid)], "ToUnicode" => unicode });
            let horizontal = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "TestVertical", "Encoding" => "Identity-H", "DescendantFonts" => vec![Object::Reference(cid)], "ToUnicode" => unicode });
            let (first, second) = if encoding == "Identity-V" { ("000100020003", "00040005") } else { ("65E5672C8A9E", "65877AE0") };
            let operators =
                format!("BT /H1 20 Tf 1 0 0 1 40 280 Tm <{first}> Tj /F1 20 Tf 1 0 0 1 200 250 Tm <{first}> Tj 1 0 0 1 170 250 Tm <{second}> Tj ET");
            let content = doc.add_object(Stream::new(dictionary! {}, operators.into_bytes()));
            let pages = doc.new_object_id();
            let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(300), Object::Integer(300)], "Resources" => dictionary! {"Font" => dictionary! {"F1" => font, "H1" => horizontal}}, "Contents" => content });
            doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1}.into());
            let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
            doc.trailer.set("Root", catalog);
            let mut bytes = Vec::new();
            doc.save_to(&mut bytes).unwrap();
            let pdf = Pdf::new(std::sync::Arc::new(bytes)).unwrap();
            let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
            assert_eq!(text.plain_text(), "日本語\n日本語\n文章");
            assert_eq!(text.find("日本語"), vec![0..3, 3..6]);
            assert_eq!(text.find("文章"), vec![6..8]);
            let rects = text.line_rects(3..6);
            assert_eq!(rects.len(), 1);
            assert!(rects[0][3] - rects[0][1] > rects[0][2] - rects[0][0], "a vertical search highlight: {rects:?}");
            assert!(text.glyphs[3].rect[0] > text.glyphs[6].rect[0], "columns read right to left in page view coordinates");
        }
    }

    /// One page per text matrix, each showing `codes` in Helvetica whose `/ToUnicode` maps the
    /// codes in `unicode` (a `bfchar` body) to several characters.
    fn ligature_pages(unicode: &str, codes: &str, matrices: &[&str]) -> Pdf {
        use lopdf::{Document, Object, Stream, dictionary};
        let mut doc = Document::with_version("1.7");
        let cmap = format!(
            "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def /CMapName /TestLigatures def /CMapType 2 def 1 begincodespacerange <00> <FF> endcodespacerange {unicode} endcmap CMapName currentdict /CMap defineresource pop end end"
        );
        let to_unicode = doc.add_object(Stream::new(dictionary! {}, cmap.into_bytes()));
        let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "ToUnicode" => to_unicode });
        let pages = doc.new_object_id();
        let mut kids = Vec::new();
        for m in matrices {
            let content = doc.add_object(Stream::new(dictionary! {}, format!("BT /F1 20 Tf {m} Tm ({codes}) Tj ET").into_bytes()));
            let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(300), Object::Integer(300)], "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}}, "Contents" => content });
            kids.push(Object::Reference(page));
        }
        let count = kids.len() as i64;
        doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => kids, "Count" => count}.into());
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
        doc.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        Pdf::new(std::sync::Arc::new(bytes)).unwrap()
    }

    #[test]
    fn a_ligature_is_one_glyph_in_every_writing_direction() {
        // "o", one glyph for "ffi", "c", "e": "office", turned 0°, 90°, 180° and 270°. The same
        // codes without the ligature mapping ("oWce") are the control for the glyph box.
        let turns = ["1 0 0 1 100 150", "0 1 -1 0 150 100", "-1 0 0 -1 200 150", "0 -1 1 0 150 200"];
        let ligature = ligature_pages("1 beginbfchar <57> <006600660069> endbfchar", "oWce", &turns);
        let control = ligature_pages("", "oWce", &turns);
        for (page, turn) in turns.iter().enumerate() {
            let text = extract_page(&ligature, page, &InterpreterSettings::default()).unwrap();
            let plain = extract_page(&control, page, &InterpreterSettings::default()).unwrap();
            assert_eq!(plain.plain_text(), "oWce", "Tm {turn}");
            assert_eq!(text.plain_text(), "office", "Tm {turn}");
            assert_eq!(text.glyphs.iter().map(|g| g.text.as_str()).collect::<Vec<_>>(), ["o", "ffi", "c", "e"], "Tm {turn}");
            assert_eq!(text.glyphs[1].rect, plain.glyphs[1].rect, "the painted glyph's box, Tm {turn}");
            assert_eq!(text.find("ffi"), vec![1..2], "Tm {turn}");
            assert_eq!(text.find("of"), vec![0..2], "a match ending inside the ligature covers it, Tm {turn}");
            assert_eq!(text.line_rects(0..4).len(), 1, "one line, Tm {turn}");
        }
    }

    #[test]
    fn narrow_ligatures_keep_every_letter() {
        // Split evenly, this "ffi" put its two "f"s so close that the second was dropped as a
        // fake-bold copy of the first.
        let pdf = ligature_pages("1 beginbfchar <62> <006600660069> endbfchar", "obce", &["1 0 0 1 100 150"]);
        let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
        assert_eq!(text.plain_text(), "office");
    }

    #[test]
    fn right_to_left_ligatures_keep_their_logical_order() {
        // "سلام" drawn left to right as meem, alef, lam, seen; then with lam-alef as one glyph.
        let letters = ligature_pages("4 beginbfchar <61> <0645> <62> <0644> <63> <0633> <64> <0627> endbfchar", "adbc", &["1 0 0 1 100 150"]);
        let ligature = ligature_pages("3 beginbfchar <61> <0645> <62> <06440627> <63> <0633> endbfchar", "abc", &["1 0 0 1 100 150"]);
        for pdf in [letters, ligature] {
            assert_eq!(extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap().plain_text(), "سلام");
        }
        // "بَاب" with beh and its fatha as one glyph, as Add text writes shaped Arabic: the mark
        // stays after its letter.
        let marked = ligature_pages("3 beginbfchar <61> <0628064E> <62> <0627> <63> <0628> endbfchar", "cba", &["1 0 0 1 100 150"]);
        assert_eq!(extract_page(&marked, 0, &InterpreterSettings::default()).unwrap().plain_text(), "بَاب");
    }

    #[test]
    fn vertical_ligatures_stay_in_their_column() {
        use lopdf::{Document, Object, Stream, dictionary};
        let mut doc = Document::with_version("1.7");
        // CID 2 is one glyph for "株式".
        let cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def /CMapName /TestUnicode def /CMapType 2 def 1 begincodespacerange <0000> <FFFF> endcodespacerange 3 beginbfchar <0001> <65E5> <0002> <682A5F0F> <0003> <8A9E> endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
        let unicode = doc.add_object(Stream::new(dictionary! {}, cmap.to_vec()));
        let cid = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => "TestVertical",
            "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Identity"), "Supplement" => 0 },
            "DW" => 1000, "DW2" => vec![Object::Integer(880), Object::Integer(-1000)], "CIDToGIDMap" => "Identity"
        });
        let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "TestVertical", "Encoding" => "Identity-V", "DescendantFonts" => vec![Object::Reference(cid)], "ToUnicode" => unicode });
        let content = doc.add_object(Stream::new(dictionary! {}, b"BT /F1 20 Tf 1 0 0 1 200 250 Tm <000100020003> Tj ET".to_vec()));
        let pages = doc.new_object_id();
        let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(300), Object::Integer(300)], "Resources" => dictionary! {"Font" => dictionary! {"F1" => font}}, "Contents" => content });
        doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1}.into());
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
        doc.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        let pdf = Pdf::new(std::sync::Arc::new(bytes)).unwrap();
        let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
        assert_eq!(text.plain_text(), "日株式語");
        assert_eq!(text.glyphs.len(), 3);
        assert_eq!(text.line_rects(0..3).len(), 1, "one column");
    }

    #[test]
    fn upright_mapping_round_trips() {
        let r = [10.0, 20.0, 30.0, 25.0];
        for q in 0..4 {
            assert_eq!(from_upright(to_upright(r, q, 300.0, 200.0), q, 300.0, 200.0), r, "quarter {q}");
        }
        // A 90° page: the view's top-right corner is the upright top-left.
        assert_eq!(to_upright([290.0, 0.0, 300.0, 10.0], 1, 300.0, 200.0), [0.0, 0.0, 10.0, 10.0]);
    }

    #[test]
    fn turkish_dotted_and_dotless_i_match_in_any_case() {
        // "İSTANBUL IRMAK KIŞ" on one line: each word typed in lower case must find it, and the
        // reverse. `to_lowercase` alone made `İ` two characters and left `ı` unmatched.
        let mut v = Vec::new();
        word(&mut v, "İSTANBUL", 10.0, 10.0, 7.0);
        word(&mut v, "IRMAK", 80.0, 10.0, 7.0);
        word(&mut v, "KIŞ", 130.0, 10.0, 7.0);
        let upper = layout(v);
        assert_eq!(upper.plain_text(), "İSTANBUL IRMAK KIŞ");
        assert_eq!(upper.find("istanbul"), vec![0..8]);
        assert_eq!(upper.find("İstanbul"), vec![0..8]);
        assert_eq!(upper.find("ırmak"), vec![8..13]);
        assert_eq!(upper.find("kış"), vec![13..16]);
        assert_eq!(upper.find("istanbul ırmak kış"), vec![0..16], "a phrase across the words");
        assert_eq!(upper.find_opts("ırmak", false, true), vec![8..13], "whole words");
        assert!(upper.find_opts("istanbul", true, false).is_empty(), "case-sensitive search stays exact");
        assert_eq!(upper.find_opts("İSTANBUL", true, false), vec![0..8]);

        let mut v = Vec::new();
        word(&mut v, "istanbul", 10.0, 10.0, 7.0);
        word(&mut v, "ırmak", 80.0, 10.0, 7.0);
        word(&mut v, "kış", 130.0, 10.0, 7.0);
        let lower = layout(v);
        assert_eq!(lower.find("İSTANBUL"), vec![0..8]);
        assert_eq!(lower.find("IRMAK"), vec![8..13]);
        assert_eq!(lower.find("KIŞ"), vec![13..16]);
    }

    #[test]
    fn case_folding_keeps_one_character_per_character() {
        for s in ["İSTANBUL", "ırmak", "Hello WORLD", "ÇAĞLAR", "Straße", "ΣΟΦΙΑ", "日本語"] {
            assert_eq!(fold_case(s).chars().count(), s.chars().count(), "{s}");
        }
        assert_eq!(fold_case("Hello WORLD"), "hello world");
        assert_eq!(fold_case("ÇAĞLAR Şişli"), "çağlar şişli");
        assert_eq!(fold_case("İIıi"), "iiii");
    }

    fn g(t: &str, x0: f32, y0: f32, x1: f32) -> TextGlyph {
        let rect = [x0, y0, x1, y0 + 10.0];
        TextGlyph { text: t.into(), rect, quad: [[x0, y0], [x1, y0], [x1, rect[3]], [x0, rect[3]]], direction: [1.0, 0.0] }
    }

    #[test]
    fn lines_words_and_search() {
        // "Hello world" on line 1, "Next" on line 2.
        let mut v = Vec::new();
        for (i, c) in "Hello".chars().enumerate() {
            v.push(g(&c.to_string(), 10.0 + i as f32 * 6.0, 10.0, 16.0 + i as f32 * 6.0));
        }
        for (i, c) in "world".chars().enumerate() {
            v.push(g(&c.to_string(), 50.0 + i as f32 * 6.0, 10.0, 56.0 + i as f32 * 6.0));
        }
        for (i, c) in "Next".chars().enumerate() {
            v.push(g(&c.to_string(), 10.0 + i as f32 * 6.0, 30.0, 16.0 + i as f32 * 6.0));
        }
        let t = layout(v);
        assert_eq!(t.plain_text(), "Hello world\nNext");
        assert_eq!(t.find("WORLD next"), vec![5..14]);
        assert_eq!(t.find("o"), vec![4..5, 6..7]);
        assert_eq!(t.line_rects(3..12).len(), 2);
        assert_eq!(t.nearest(52.0, 14.0), Some(5));
    }

    #[test]
    fn phrase_search_ignores_repeated_spaces() {
        // "quick  brown" drawn with two space glyphs in a row. Text extraction keeps both spaces,
        // but the phrase search has always collapsed runs of whitespace in the query, so the page
        // text has to be collapsed the same way or the phrase is never found (#814).
        let mut v = Vec::new();
        word(&mut v, "quick", 10.0, 10.0, 6.0);
        v.push(g(" ", 46.0, 10.0, 52.0));
        v.push(g(" ", 52.0, 10.0, 58.0));
        word(&mut v, "brown", 58.0, 10.0, 6.0);
        let n = v.len();
        let t = PageText { glyphs: v, line_of: vec![0; n], space_before: vec![false; n] };
        assert_eq!(t.plain_text(), "quick  brown");
        assert_eq!(t.find("quick brown"), vec![0..12]);
        assert_eq!(t.find("quick  brown"), vec![0..12], "the query's own run of spaces is collapsed too");
        assert_eq!(t.find("quick\tbrown"), vec![0..12]);
        assert_eq!(t.find("quick"), vec![0..5]);
        assert_eq!(t.find("brown"), vec![7..12]);
        assert!(t.find("quick absent").is_empty());
    }

    fn word(v: &mut Vec<TextGlyph>, s: &str, x: f32, y: f32, advance: f32) {
        for (i, c) in s.chars().enumerate() {
            let x0 = x + i as f32 * advance;
            v.push(g(&c.to_string(), x0, y, x0 + 6.0));
        }
    }

    #[test]
    fn fake_bold_duplicates_are_removed() {
        let mut v = Vec::new();
        for (i, c) in "Bold".chars().enumerate() {
            let x = 10.0 + i as f32 * 7.0;
            v.push(g(&c.to_string(), x, 10.0, x + 6.0));
            v.push(g(&c.to_string(), x + 0.3, 10.0, x + 6.3));
        }
        assert_eq!(layout(v).plain_text(), "Bold");
    }

    #[test]
    fn two_columns_read_left_then_right_even_if_drawn_interleaved() {
        let mut v = Vec::new();
        // Drawn row by row across both columns, as some producers do.
        word(&mut v, "Left1", 10.0, 10.0, 6.0);
        word(&mut v, "Right1", 200.0, 10.0, 6.0);
        word(&mut v, "Left2", 10.0, 24.0, 6.0);
        word(&mut v, "Right2", 200.0, 24.0, 6.0);
        let t = layout(v);
        assert_eq!(t.plain_text(), "Left1\nLeft2\nRight1\nRight2");
    }

    #[test]
    fn tracked_text_keeps_letters_together() {
        let mut v = Vec::new();
        word(&mut v, "CHAPTER", 10.0, 10.0, 9.0); // 3pt tracking between 6pt-wide letters
        word(&mut v, "FOUR", 10.0 + 7.0 * 9.0 + 5.0, 10.0, 9.0);
        assert_eq!(layout(v).plain_text(), "CHAPTER FOUR");
    }

    #[test]
    fn rtl_runs_come_out_in_logical_order_and_cjk_has_no_spaces() {
        let mut v = Vec::new();
        // Hebrew "שלום" drawn visually right-to-left: ם ו ל ש from left to right.
        word(&mut v, "םולש", 10.0, 10.0, 6.0);
        assert_eq!(layout(v).plain_text(), "שלום");
        let mut v = Vec::new();
        word(&mut v, "每个字", 10.0, 10.0, 9.0);
        assert_eq!(layout(v).plain_text(), "每个字");
    }

    /// Two lines, "ab cd" and "e fg": words stop at blank glyphs, word gaps and line ends.
    #[test]
    fn words_and_lines_around_a_glyph() {
        let t = PageText {
            glyphs: Vec::from(["a", "b", " ", "c", "d", "e", "f", "g"].map(|s| TextGlyph {
                text: s.into(),
                rect: [0.0; 4],
                quad: [[0.0; 2]; 4],
                direction: [1.0, 0.0],
            })),
            line_of: vec![0, 0, 0, 0, 0, 1, 1, 1],
            space_before: vec![false, false, false, false, false, false, true, false],
        };
        assert_eq!(t.word_at(1), Some((0, 1)));
        assert_eq!(t.word_at(3), Some((3, 4)));
        assert_eq!(t.word_at(4), Some((3, 4)));
        assert_eq!(t.word_at(5), Some((5, 5)));
        assert_eq!(t.word_at(7), Some((6, 7)));
        assert_eq!(t.line_at(2), Some((0, 4)));
        assert_eq!(t.line_at(6), Some((5, 7)));
        assert_eq!(t.word_at(8), None);
        assert_eq!(t.line_at(8), None);
        assert_eq!(PageText::default().line_at(0), None);
    }

    /// A 400 × 400 page drawing `ops` with Helvetica as /F1 and Courier as /F2.
    fn text_page(ops: &str) -> Pdf {
        use lopdf::{Document, Object, Stream, dictionary};
        let mut doc = Document::with_version("1.7");
        let helvetica = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
        let courier = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier" });
        let content = doc.add_object(Stream::new(dictionary! {}, ops.as_bytes().to_vec()));
        let pages = doc.new_object_id();
        let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(400), Object::Integer(400)], "Resources" => dictionary! {"Font" => dictionary! {"F1" => helvetica, "F2" => courier}}, "Contents" => content });
        doc.objects.insert(pages, dictionary! {"Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1}.into());
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages});
        doc.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        Pdf::new(std::sync::Arc::new(bytes)).unwrap()
    }

    /// `body` shown in /F`font` at `size` with its baseline turned `degrees` counter-clockwise.
    fn turned(degrees: f64, font: u8, size: f64, x: f64, y: f64, body: &str) -> String {
        let (s, c) = degrees.to_radians().sin_cos();
        format!("BT /F{font} {size} Tf {c:.6} {s:.6} {:.6} {c:.6} {x:.4} {y:.4} Tm {body} ET\n", -s)
    }

    fn page_text(ops: &str) -> String {
        extract_page(&text_page(ops), 0, &InterpreterSettings::default()).unwrap().plain_text()
    }

    #[test]
    fn turned_lines_group_along_their_baseline() {
        // Two lines 13 pt apart, words separated by TJ offsets (no space glyphs), the second
        // line drawn first. Turned away from the quarter turns, the lines' boxes overlap.
        for degrees in [12.0, 30.0, 37.0, 45.0, 60.0, 135.0, 200.0, 300.0, -45.0] {
            let (s, c) = f64::to_radians(degrees).sin_cos();
            let second = turned(degrees, 1, 12.0, 200.0 + s * 13.0, 200.0 - c * 13.0, "[(Second) -400 (line) -400 (here)] TJ");
            let first = turned(degrees, 1, 12.0, 200.0, 200.0, "[(First) -400 (line) -400 (text)] TJ");
            assert_eq!(page_text(&(second + &first)), "First line text\nSecond line here", "{degrees}°");
        }
    }

    #[test]
    fn crossing_turned_lines_stay_apart() {
        // Lines at 30° and −30° crossing each other, their glyphs drawn alternately.
        let (up, down) = ("Upward words here", "Downward words now");
        let mut ops = String::new();
        let (s, c) = f64::to_radians(30.0).sin_cos();
        for (k, (a, b)) in up.chars().zip(down.chars()).enumerate() {
            let along = 7.2 * k as f64;
            ops += &turned(30.0, 2, 12.0, 100.0 + c * along, 150.0 + s * along, &format!("({a}) Tj"));
            ops += &turned(-30.0, 2, 12.0, 100.0 + c * along, 250.0 - s * along, &format!("({b}) Tj"));
        }
        ops += &turned(-30.0, 2, 12.0, 100.0 + c * 7.2 * 17.0, 250.0 - s * 7.2 * 17.0, "(w) Tj");
        assert_eq!(page_text(&ops), "Downward words now\nUpward words here");
    }

    #[test]
    fn text_along_a_curve_is_one_line() {
        // Courier glyphs (7.2 pt advance) set on circles of radius 90 around (200, 200): over
        // the top reading clockwise, under the bottom reading counter-clockwise, and a full ring.
        let arc = |text: &str, from: f64, clockwise: bool, radius: f64| {
            let step = (7.2 / radius).to_degrees() * if clockwise { -1.0 } else { 1.0 };
            let mut ops = String::new();
            for (k, ch) in text.chars().enumerate() {
                let a = from + step * k as f64;
                let (s, c) = a.to_radians().sin_cos();
                ops += &turned(if clockwise { a - 90.0 } else { a + 90.0 }, 2, 12.0, 200.0 + radius * c, 200.0 + radius * s, &format!("({ch}) Tj"));
            }
            ops
        };
        assert_eq!(page_text(&arc("CURVED STAMP TEXT", 150.0, true, 90.0)), "CURVED STAMP TEXT");
        assert_eq!(page_text(&arc("BOTTOM ARC WORDS", 220.0, false, 90.0)), "BOTTOM ARC WORDS");
        assert_eq!(page_text(&arc("ROUND SEAL OF THE COMPANY LIMITED", 180.0, true, 50.0)), "ROUND SEAL OF THE COMPANY LIMITED");
    }

    #[test]
    fn sheared_text_keeps_its_word_gaps() {
        // The baseline climbs at about 31° while the glyphs stay upright.
        let ops = "BT /F1 12 Tf 1 0.6 0 1 100 200 Tm [(Sheared) -400 (first)] TJ 1 0.6 0 1 100 187 Tm [(Sheared) -400 (second)] TJ ET";
        assert_eq!(page_text(ops), "Sheared first\nSheared second");
    }

    #[test]
    fn turned_text_keeps_its_place_in_the_reading_order() {
        // A turned label between a heading and its caption reads between them, as it did when
        // it was laid out with the horizontal text; a vertical body still follows its heading.
        let ops = "BT /F1 14 Tf 1 0 0 1 50 360 Tm (Heading above) Tj ET\n".to_string()
            + &turned(30.0, 1, 12.0, 80.0, 250.0, "(Turned label) Tj")
            + "BT /F1 12 Tf 1 0 0 1 50 150 Tm (Caption below) Tj ET\n";
        assert_eq!(page_text(&ops), "Heading above\nTurned label\nCaption below");
    }

    #[test]
    fn turned_glyphs_keep_their_shape_and_direction() {
        // 30° counter-clockwise on the page is 30° up from the x axis in view space (y down).
        let pdf = text_page(&(turned(30.0, 1, 20.0, 100.0, 100.0, "(Ab) Tj") + "BT /F1 20 Tf 1 0 0.5 1 100 300 Tm (Ab) Tj ET"));
        let text = extract_page(&pdf, 0, &InterpreterSettings::default()).unwrap();
        assert_eq!(text.plain_text(), "Ab\nAb");
        for g in &text.glyphs {
            assert!(g.quad.iter().flatten().chain(&g.direction).chain(&g.rect).all(|c| c.is_finite()), "{g:?}");
            let bounds = g.quad.iter().fold([f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY], |r, p| {
                [r[0].min(p[0]), r[1].min(p[1]), r[2].max(p[0]), r[3].max(p[1])]
            });
            assert_eq!(g.rect, bounds, "the box bounds the quad");
            let [x, y] = [g.quad[1][0] - g.quad[0][0], g.quad[1][1] - g.quad[0][1]];
            assert!(
                (x * g.direction[1] - y * g.direction[0]).abs() < 1e-3 && x * g.direction[0] + y * g.direction[1] > 0.0,
                "quad[0] → quad[1] runs along the direction: {g:?}"
            );
        }
        let (skewed, turned) = (&text.glyphs[0], &text.glyphs[2]);
        assert!((turned.direction[0] - 30f32.to_radians().cos()).abs() < 1e-6 && (turned.direction[1] + 0.5).abs() < 1e-6, "{turned:?}");
        assert_eq!(skewed.direction, [1.0, 0.0]);
        let slant = skewed.quad[3][0] - skewed.quad[0][0];
        assert!((slant - 10.0).abs() < 1e-3, "the skewed glyph's upright edge leans by half its 20 pt height: {skewed:?}");
    }

    /// A three-row table, "Name Qty / Apple 12 / Pear 7": the columns sit far enough apart that
    /// reading order takes one column, then the other.
    fn table() -> PageText {
        let mut v = Vec::new();
        for (row, (name, qty)) in [("Name", "Qty"), ("Apple", "12"), ("Pear", "7")].into_iter().enumerate() {
            let y = 10.0 + row as f32 * 14.0;
            word(&mut v, name, 10.0, y, 6.0);
            word(&mut v, qty, 100.0, y, 6.0);
        }
        layout(v)
    }

    /// Issue #740: a column selection takes only what is inside the rectangle.
    #[test]
    fn a_column_selection_takes_one_table_column() {
        let t = table();
        assert_eq!(t.plain_text(), "Name\nApple\nPear\nQty\n12\n7", "reading order is column by column");
        let qty = t.glyphs_in([95.0, 0.0, 130.0, 60.0]);
        assert_eq!(t.column_text(&qty), "Qty\n12\n7");
        assert_eq!(t.glyph_rects(&qty).len(), 3, "one highlight per row");
        // Corners in either order.
        assert_eq!(t.glyphs_in([130.0, 60.0, 95.0, 0.0]), qty);
    }

    /// Across columns the text comes out row by row, cells separated by tabs.
    #[test]
    fn a_column_selection_across_columns_reads_row_by_row() {
        let t = table();
        let all = t.glyphs_in([0.0, 0.0, 200.0, 60.0]);
        assert_eq!(t.column_text(&all), "Name\tQty\nApple\t12\nPear\t7");
        // The middle row only, and only the start of "Apple": centres decide.
        let part = t.glyphs_in([0.0, 25.0, 31.0, 33.0]);
        assert_eq!(t.column_text(&part), "Appl");
        assert_eq!(t.glyph_rects(&part), vec![[10.0, 24.0, 34.0, 34.0]]);
    }

    /// A rectangle that misses every glyph, or is not a number, selects nothing.
    #[test]
    fn an_empty_or_degenerate_column_selection_is_empty() {
        let t = table();
        for r in [[300.0, 300.0, 400.0, 400.0], [f32::NAN, 0.0, 200.0, 60.0], [50.0, 50.0, 50.0, 50.0]] {
            let none = t.glyphs_in(r);
            assert!(none.is_empty(), "{r:?}");
            assert_eq!(t.column_text(&none), "");
            assert!(t.glyph_rects(&none).is_empty());
        }
        assert_eq!(PageText::default().column_text(&[0, 7, usize::MAX]), "", "indices past the text are ignored");
    }

    /// Right-to-left rows put their cells right to left.
    #[test]
    fn right_to_left_rows_read_right_to_left() {
        let mut v = Vec::new();
        // "שלום" (right) and "עולם" (left), each drawn visually.
        word(&mut v, "םולש", 100.0, 10.0, 6.0);
        word(&mut v, "םלוע", 10.0, 10.0, 6.0);
        let t = layout(v);
        let all = t.glyphs_in([0.0, 0.0, 200.0, 30.0]);
        assert_eq!(t.column_text(&all), "שלום\tעולם");
    }

    /// Vertical lines (CJK vertical writing) come out one per row, in reading order.
    #[test]
    fn vertical_lines_keep_reading_order() {
        // Written downwards: the em box's edge towards the next line is its left one.
        let cell = |t: &str, x: f32, y: f32| TextGlyph {
            text: t.into(),
            rect: [x, y, x + 10.0, y + 10.0],
            quad: [[x, y], [x, y + 10.0], [x + 10.0, y + 10.0], [x + 10.0, y]],
            direction: [0.0, 1.0],
        };
        let t = PageText {
            // Right column 日本 first, then the left column 語文, each top to bottom.
            glyphs: vec![cell("日", 40.0, 10.0), cell("本", 40.0, 20.0), cell("語", 20.0, 10.0), cell("文", 20.0, 20.0)],
            line_of: vec![0, 0, 1, 1],
            space_before: vec![false; 4],
        };
        let all = t.glyphs_in([0.0, 0.0, 60.0, 40.0]);
        assert_eq!(t.column_text(&all), "日本\n語文");
    }
}
