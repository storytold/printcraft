//! Typed signatures and bounded glyph outlines from approved fonts (bundled, or craft-fonts).

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};

static FONT: &[u8] = include_bytes!("../../../assets/fonts/DancingScript.ttf");
/// Characters the script face lacks (Cyrillic, Greek, ...) draw in the bundled Inter face
/// instead of vanishing: it carries Latin, Greek and Cyrillic including the Serbian letters
/// (#846), and it is already shipped, so AGENTS.md §1.4's no-system-fonts-in-PDFs rule holds.
static FALLBACK: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");

/// Bound signature work while allowing long personal names.
pub const MAX_SIGNATURE_CHARS: usize = 256;

/// Outlines of a line of existing text at a font size of 1 (em units).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptOutline {
    pub contours: Vec<Vec<[f64; 2]>>,
    pub width: f64,
    pub ascent: f64,
    pub descent: f64,
}

impl ScriptOutline {
    /// Layout bounds including glyph overhangs, so previews and PDF appearances never clip ink.
    pub fn bounds(&self) -> [f64; 4] {
        self.contours.iter().flatten().fold([0.0, self.descent, self.width, self.ascent], |mut b, p| {
            b[0] = b[0].min(p[0]);
            b[1] = b[1].min(p[1]);
            b[2] = b[2].max(p[0]);
            b[3] = b[3].max(p[1]);
            b
        })
    }
}

/// One glyph outline in em units, with the baseline at y = 0.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphOutline {
    pub contours: Vec<Vec<[f64; 2]>>,
    pub width: f64,
    pub bbox: [f64; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlyphError {
    /// This build has no Japanese face: it was built without craft-fonts (`CRAFT_FONTS_DIR`).
    NoFont,
    Missing,
    TooComplex,
}

struct Flatten {
    contours: Vec<Vec<[f64; 2]>>,
    cur: Vec<[f64; 2]>,
    scale: f64,
    dx: f64,
    points: usize,
    too_complex: bool,
    max_points: usize,
}

impl Flatten {
    fn new(scale: f64) -> Self {
        Self { contours: Vec::new(), cur: Vec::new(), scale, dx: 0.0, points: 0, too_complex: false, max_points: 4096 }
    }

    fn pt(&self, x: f32, y: f32) -> [f64; 2] {
        [self.dx + x as f64 * self.scale, y as f64 * self.scale]
    }

    fn last(&self) -> [f64; 2] {
        self.cur.last().copied().unwrap_or([self.dx, 0.0])
    }

    fn push(&mut self, p: [f64; 2]) {
        if self.points >= self.max_points {
            self.too_complex = true;
            return;
        }
        self.points += 1;
        self.cur.push(p);
    }

    fn close(&mut self) {
        if self.cur.len() > 2 {
            self.contours.push(std::mem::take(&mut self.cur));
        } else {
            self.cur.clear();
        }
    }
}

const STEPS: usize = 8;

impl OutlinePen for Flatten {
    fn move_to(&mut self, x: f32, y: f32) {
        self.close();
        self.push(self.pt(x, y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.push(self.pt(x, y));
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let (p0, c, p) = (self.last(), self.pt(cx, cy), self.pt(x, y));
        for i in 1..=STEPS {
            let t = i as f64 / STEPS as f64;
            let u = 1.0 - t;
            self.push([u * u * p0[0] + 2.0 * u * t * c[0] + t * t * p[0], u * u * p0[1] + 2.0 * u * t * c[1] + t * t * p[1]]);
        }
    }

    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        let (p0, c0, c1, p) = (self.last(), self.pt(c0x, c0y), self.pt(c1x, c1y), self.pt(x, y));
        for i in 1..=STEPS {
            let t = i as f64 / STEPS as f64;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            self.push([a * p0[0] + b * c0[0] + c * c1[0] + d * p[0], a * p0[1] + b * c0[1] + c * c1[1] + d * p[1]]);
        }
    }

    fn close(&mut self) {
        Flatten::close(self);
    }
}

/// Return one glyph of the Japanese document face ([`crate::document_japanese_font`], Shippori
/// Mincho from craft-fonts), bounded so hostile replacement text cannot allocate unbounded
/// outline data. [`GlyphError::NoFont`] when the build has no Japanese face.
pub fn japanese_glyph(ch: char) -> Result<GlyphOutline, GlyphError> {
    let Some(face) = crate::document_japanese_font() else { return Err(GlyphError::NoFont) };
    japanese_glyph_from(face, ch)
}

/// The bounded outline and advance from the explicitly selected craft-fonts face.
/// Uses the same complexity and missing-glyph checks as [`japanese_glyph`].
pub fn japanese_glyph_from(face: &crate::CraftFont, ch: char) -> Result<GlyphOutline, GlyphError> {
    let Ok(font) = FontRef::new(face.bytes) else { return Err(GlyphError::Missing) };
    let loc = LocationRef::default();
    let metrics = font.metrics(Size::unscaled(), loc);
    let scale = 1.0 / metrics.units_per_em.max(1) as f64;
    let Some(gid) = font.charmap().map(ch) else { return Err(GlyphError::Missing) };
    let advances = font.glyph_metrics(Size::unscaled(), loc);
    let width = advances.advance_width(gid).unwrap_or(0.0) as f64 * scale;
    if !width.is_finite() || width <= 0.0 || width > 2.0 {
        return Err(GlyphError::Missing);
    }
    bounded_outline(&font, gid, width)
}

/// Glyph `gid` of `font` in em units with advance `width`, bounded like [`japanese_glyph`].
pub(crate) fn bounded_outline(font: &FontRef, gid: GlyphId, width: f64) -> Result<GlyphOutline, GlyphError> {
    let loc = LocationRef::default();
    let scale = 1.0 / font.metrics(Size::unscaled(), loc).units_per_em.max(1) as f64;
    let Some(glyph) = font.outline_glyphs().get(gid) else { return Err(GlyphError::Missing) };
    let mut pen = Flatten::new(scale);
    let _ = glyph.draw(DrawSettings::unhinted(Size::unscaled(), loc), &mut pen);
    pen.close();
    if pen.too_complex || pen.contours.len() > 256 {
        return Err(GlyphError::TooComplex);
    }
    let mut bbox = [0.0, 0.0, width, 0.0];
    let mut any = false;
    for p in pen.contours.iter().flatten() {
        if !p[0].is_finite() || !p[1].is_finite() {
            return Err(GlyphError::Missing);
        }
        if !any {
            bbox = [p[0], p[1], p[0], p[1]];
            any = true;
        } else {
            bbox[0] = bbox[0].min(p[0]);
            bbox[1] = bbox[1].min(p[1]);
            bbox[2] = bbox[2].max(p[0]);
            bbox[3] = bbox[3].max(p[1]);
        }
    }
    Ok(GlyphOutline { contours: pen.contours, width, bbox })
}

/// The outlines of `text` in the script font (characters it lacks draw in the bundled Inter
/// face, so Cyrillic and Greek signatures stay visible). Over-limit input returns an empty
/// outline instead of a silently truncated signature.
pub fn script_outline(text: &str) -> ScriptOutline {
    if text.chars().take(MAX_SIGNATURE_CHARS + 1).count() > MAX_SIGNATURE_CHARS {
        return ScriptOutline::default();
    }
    let Ok(font) = FontRef::new(FONT) else { return ScriptOutline::default() };
    let loc = LocationRef::default();
    let metrics = font.metrics(Size::unscaled(), loc);
    let scale = 1.0 / metrics.units_per_em.max(1) as f64;
    let charmap = font.charmap();
    let glyphs = font.outline_glyphs();
    let advances = font.glyph_metrics(Size::unscaled(), loc);
    // The fallback face: its glyphs are drawn in its own em scale.
    let fallback = FontRef::new(FALLBACK).ok();
    let mut pen = Flatten::new(scale);
    // The 4096-point budget is for one untrusted document glyph (japanese_glyph), not a
    // whole signature in our bundled font. Keep a separate bounded budget for the line.
    pen.max_points = 262_144;
    for ch in text.chars() {
        let Some(gid) = charmap.map(ch) else {
            // Not in the script face: draw it in the fallback face, in its own em scale, and
            // advance by its own width. Characters neither face has are still skipped.
            let Some(f) = &fallback else { continue };
            let cm = f.charmap();
            let Some(gid) = cm.map(ch) else { continue };
            let gl = f.outline_glyphs();
            let ad = f.glyph_metrics(Size::unscaled(), loc);
            let sc = 1.0 / f.metrics(Size::unscaled(), loc).units_per_em.max(1) as f64;
            if let Some(g) = gl.get(gid) {
                let line_scale = pen.scale;
                pen.scale = sc;
                let _ = g.draw(DrawSettings::unhinted(Size::unscaled(), loc), &mut pen);
                pen.scale = line_scale;
                pen.close();
            }
            pen.dx += ad.advance_width(gid).unwrap_or(0.0) as f64 * sc;
            continue;
        };
        if let Some(g) = glyphs.get(gid) {
            let _ = g.draw(DrawSettings::unhinted(Size::unscaled(), loc), &mut pen);
            pen.close();
        }
        pen.dx += advances.advance_width(gid).unwrap_or(0.0) as f64 * scale;
    }
    pen.close();
    if pen.too_complex {
        return ScriptOutline::default();
    }
    ScriptOutline { contours: pen.contours, width: pen.dx, ascent: metrics.ascent as f64 * scale, descent: metrics.descent as f64 * scale }
}

#[cfg(test)]
mod tests {
    #[test]
    fn long_names_keep_every_glyph_with_bounded_outline_work() {
        let text = "Alexandria Catherine Elizabeth Montgomery-Wellington";
        let o = super::script_outline(text);
        let expected: usize = text.chars().map(|c| super::script_outline(&c.to_string()).contours.len()).sum();
        assert_eq!(o.contours.len(), expected, "the entire name must be drawn");
        let max_x = o.contours.iter().flatten().map(|p| p[0]).fold(0.0, f64::max);
        assert!(max_x > o.width - 0.5, "ink reaches the last letter: {max_x} / {}", o.width);
        assert!(!super::script_outline(&"W".repeat(super::MAX_SIGNATURE_CHARS)).contours.is_empty());
        assert!(super::script_outline(&"W".repeat(super::MAX_SIGNATURE_CHARS + 1)).contours.is_empty());
    }

    #[test]
    fn serbian_cyrillic_draws_in_the_fallback_face() {
        // Dancing Script has no Cyrillic at all (#846): before the fallback, every character
        // of a Serbian name was skipped and the signature came out empty.
        let o = super::script_outline("Tamara Ђорђевић");
        assert!(!o.contours.is_empty(), "the Cyrillic part must draw in the fallback face");
        assert!(o.width > 2.0, "{}", o.width);
        // A pure-Cyrillic name is not empty either.
        assert!(!super::script_outline("Ђорђе Јовановић").contours.is_empty());
        // Characters neither face has are still skipped, not drawn as boxes.
        assert_eq!(super::script_outline("\u{1F984}").contours.len(), 0);
    }

    #[test]
    fn a_name_becomes_outlines() {
        let o = super::script_outline("Ada L.");
        assert!(o.contours.len() >= 5, "{}", o.contours.len());
        assert!(o.width > 1.0 && o.width < 6.0, "{}", o.width);
        assert!(o.ascent > 0.5 && o.descent < 0.0);
        let max_x = o.contours.iter().flatten().map(|p| p[0]).fold(0.0, f64::max);
        assert!(max_x <= o.width + 0.2);
        assert!(super::script_outline("").contours.is_empty());
    }

    #[test]
    fn japanese_glyphs_come_from_craft_fonts() {
        if crate::document_japanese_font().is_none() {
            eprintln!("skipping the glyph checks: built without craft-fonts (set CRAFT_FONTS_DIR)");
            assert_eq!(super::japanese_glyph('こ'), Err(super::GlyphError::NoFont));
            return;
        }
        for ch in "日本語の文字こ".chars() {
            let g = super::japanese_glyph(ch).expect("craft-fonts face has the Japanese glyph");
            assert!(g.width > 0.2 && g.width < 2.0);
            assert!(!g.contours.is_empty());
        }
        assert_eq!(super::japanese_glyph('\u{1f4a9}'), Err(super::GlyphError::Missing));
    }
}
