//! Type 3 fallback fonts for text written into generated appearances (no font is embedded).
//!
//! The glyphs come from the craft-fonts Japanese faces, the same source the editor uses for
//! replacement text (`pdfcraft-edit`). Choosing the face and deciding which characters it can
//! draw happen once, in [`japanese_type3_plan`]; [`build_type3`] and the width helpers read only
//! that plan, so what is measured is what is drawn.

use std::collections::HashMap;

use pdfcraft_cos::{Dict, Object, PdfString, Stream};

use crate::{CraftFont, GlyphError, GlyphOutline, document_japanese_fonts_for_style, japanese_glyph_from};

/// Codes 1..=240 of one Type 3 font (the same bound the editor uses).
pub const MAX_TYPE3_GLYPHS: usize = 240;
/// Fonts one plan fills before the rest of its characters are left undrawn.
pub const MAX_TYPE3_FONTS: usize = 4;
/// Entries per `beginbfchar` block (ISO 32000-2 §9.10.3 allows at most 100).
const BFCHAR_BLOCK: usize = 100;
/// Side bearing (em) given to a respaced glyph on each side.
const SIDE_BEARING: f64 = 0.05;

/// The face chosen for a run of text, and what it can and cannot draw.
pub struct Type3Plan {
    face: &'static CraftFont,
    /// Drawable characters in order of first use; character `i` is code `i % 240 + 1` of font `i / 240`.
    glyphs: Vec<(char, f64)>,
    index: HashMap<char, usize>,
    missing: Vec<char>,
}

impl Type3Plan {
    /// The face the glyphs come from.
    pub fn face(&self) -> &'static CraftFont {
        self.face
    }

    /// Characters the plan can't draw (no glyph, too complex, or past the font limit).
    pub fn missing(&self) -> &[char] {
        &self.missing
    }

    /// Whether `ch` is drawn by the plan's fonts.
    pub fn can_draw(&self, ch: char) -> bool {
        self.index.contains_key(&ch)
    }

    /// Advance of `ch` in em, as the font's `/Widths` will state it. `None` if it isn't drawn.
    pub fn width(&self, ch: char) -> Option<f64> {
        self.glyphs.get(*self.index.get(&ch)?).map(|(_, w)| *w)
    }

    /// Which font (0-based) and which code in it draw `ch`.
    pub fn code(&self, ch: char) -> Option<(usize, u8)> {
        let i = *self.index.get(&ch)?;
        Some((i / MAX_TYPE3_GLYPHS, u8::try_from(i % MAX_TYPE3_GLYPHS + 1).ok()?))
    }

    /// How many fonts [`build_type3`] makes.
    pub fn font_count(&self) -> usize {
        self.glyphs.len().div_ceil(MAX_TYPE3_GLYPHS)
    }
}

/// One Type 3 font of a plan: its dictionary, and the characters behind codes 1, 2, …
#[derive(Debug)]
pub struct Type3Font {
    pub font: Dict,
    pub chars: Vec<char>,
}

/// Pick the Japanese face for `chars` and sort them into drawable and missing.
///
/// The face is chosen as the editor does: of the document faces for this style, the first that
/// has a glyph for every character, else the first face (it then draws what it can). `None` when
/// the build has no Japanese face (no craft-fonts). Duplicates are ignored. At most
/// `MAX_TYPE3_GLYPHS * MAX_TYPE3_FONTS` distinct characters are considered, so hostile text can't
/// make the work unbounded; the rest are reported as missing.
pub fn japanese_type3_plan(chars: &[char], serif: bool, bold: bool) -> Option<Type3Plan> {
    let faces = document_japanese_fonts_for_style(serif, bold);
    let mut unique: Vec<char> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut over = Vec::new();
    for &ch in chars {
        if !seen.insert(ch) {
            continue;
        }
        if unique.len() < MAX_TYPE3_GLYPHS * MAX_TYPE3_FONTS {
            unique.push(ch);
        } else {
            over.push(ch);
        }
    }
    // Each face's glyphs are read once; the chosen face's are used below. The first face that has
    // every character wins, else the first face (it then draws what it can). Later faces stop
    // at their first missing glyph, as only the first face's partial result can be needed.
    let mut chosen = None;
    let mut first = None;
    for (i, face) in faces.iter().copied().enumerate() {
        if i == 0 {
            let results: Vec<_> = unique.iter().map(|ch| japanese_glyph_from(face, *ch)).collect();
            if results.iter().all(Result::is_ok) {
                chosen = Some((face, results));
                break;
            }
            first = Some((face, results));
        } else if let Ok(glyphs) = unique.iter().map(|ch| japanese_glyph_from(face, *ch)).collect::<Result<Vec<_>, _>>() {
            chosen = Some((face, glyphs.into_iter().map(Ok).collect()));
            break;
        }
    }
    let (face, results) = chosen.or(first)?;
    let mut plan = Type3Plan { face, glyphs: Vec::new(), index: HashMap::new(), missing: over };
    for (ch, result) in unique.into_iter().zip(results) {
        match result {
            Ok(glyph) => {
                let (_, width) = proportional(ch, &glyph);
                plan.index.insert(ch, plan.glyphs.len());
                plan.glyphs.push((ch, (width * 1000.0).round() / 1000.0));
            }
            Err(GlyphError::NoFont | GlyphError::Missing | GlyphError::TooComplex) => plan.missing.push(ch),
        }
    }
    Some(plan)
}

/// Build the Type 3 fonts of `plan`.
///
/// Streams and the font descriptor are handed to `add`, which returns the object to put in their
/// place: `doc.add` for an indirect object (the form a font in a page must take), or the object
/// itself when the caller embeds the font inline and makes it indirect later.
pub fn build_type3(plan: &Type3Plan, mut add: impl FnMut(Object) -> Object) -> Vec<Type3Font> {
    let mut fonts = Vec::new();
    for chunk in plan.glyphs.chunks(MAX_TYPE3_GLYPHS) {
        let chars: Vec<char> = chunk.iter().map(|(ch, _)| *ch).collect();
        let mut charprocs = Dict::new();
        let mut differences = vec![Object::Int(1)];
        let mut widths = Vec::with_capacity(chars.len());
        for (i, ch) in chars.iter().enumerate() {
            let name = format!("g{:02X}", i + 1);
            // A glyph that planned fine and fails now is drawn empty rather than dropping the font.
            let (path, width) = glyph_path(plan.face, *ch).unwrap_or_else(|| (b"0 0 0 0 0 0 d1\n".to_vec(), 0.0));
            let stream = Stream::flate(Dict::new(), &path);
            charprocs.set(name.as_bytes().to_vec(), add(Object::Stream(stream)));
            differences.push(Object::name(&name));
            widths.push(Object::Real((width * 1000.0).round()));
        }
        let mut encoding = Dict::new();
        encoding.set(b"Type".to_vec(), Object::name("Encoding"));
        encoding.set(b"Differences".to_vec(), Object::Array(differences));
        let mut descriptor = Dict::new();
        descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
        descriptor.set(b"FontName".to_vec(), Object::name(&format!("{}-{}", plan.face.family, plan.face.style).replace(' ', "")));
        descriptor.set(b"FontFamily".to_vec(), Object::String(PdfString::literal(plan.face.family.as_bytes().to_vec())));
        descriptor.set(b"Flags".to_vec(), Object::Int(if plan.face.family.contains("Mincho") { 6 } else { 4 }));
        descriptor.set(b"ItalicAngle".to_vec(), Object::Int(0));
        let mut font = Dict::new();
        font.set(b"Type".to_vec(), Object::name("Font"));
        font.set(b"Subtype".to_vec(), Object::name("Type3"));
        // PDF 1.7 tables 5.9 and 5.19: a Type 3 descriptor is indirect; Ascent/Descent may be omitted.
        font.set(b"FontDescriptor".to_vec(), add(Object::Dict(descriptor)));
        font.set(b"FontBBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(-300), Object::Int(1000), Object::Int(1000)]));
        font.set(
            b"FontMatrix".to_vec(),
            Object::Array(vec![Object::Real(0.001), Object::Int(0), Object::Int(0), Object::Real(0.001), Object::Int(0), Object::Int(0)]),
        );
        font.set(b"FirstChar".to_vec(), Object::Int(1));
        font.set(b"LastChar".to_vec(), Object::Int(chars.len() as i64));
        font.set(b"Widths".to_vec(), Object::Array(widths));
        font.set(b"Encoding".to_vec(), Object::Dict(encoding));
        font.set(b"CharProcs".to_vec(), Object::Dict(charprocs));
        font.set(b"ToUnicode".to_vec(), add(Object::Stream(Stream::flate(Dict::new(), to_unicode(&chars).as_bytes()))));
        fonts.push(Type3Font { font, chars });
    }
    fonts
}

fn to_unicode(chars: &[char]) -> String {
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapType 2 def\n1 begincodespacerange\n<01> <FF>\nendcodespacerange\n",
    );
    for (block, chunk) in chars.chunks(BFCHAR_BLOCK).enumerate() {
        cmap.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (i, ch) in chunk.iter().enumerate() {
            let code = block * BFCHAR_BLOCK + i + 1;
            cmap.push_str(&format!("<{code:02X}> <{}>\n", unicode_hex(*ch)));
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    cmap
}

fn unicode_hex(ch: char) -> String {
    let mut units = [0u16; 2];
    ch.encode_utf16(&mut units).iter().map(|u| format!("{u:04X}")).collect()
}

fn num(v: f64) -> String {
    if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v:.4}").trim_end_matches('0').trim_end_matches('.').to_string() }
}

/// Shift and advance for a fallback glyph. Japanese faces draw Cyrillic and Greek full-width
/// (one em each, as in JIS X 0208) and have no proportional (`palt`) metrics for them, so a
/// word set that way reads as letter-spaced. Those letters get their ink width plus a side
/// bearing instead; everything else keeps the face's own advance.
fn proportional(ch: char, glyph: &GlyphOutline) -> (f64, f64) {
    let respace = matches!(ch, '\u{0370}'..='\u{03FF}' | '\u{0400}'..='\u{052F}' | '\u{1F00}'..='\u{1FFF}');
    let ink = glyph.bbox[2] - glyph.bbox[0];
    if !respace || glyph.contours.is_empty() || !(ink > 0.0 && ink < glyph.width) {
        return (0.0, glyph.width);
    }
    (SIDE_BEARING - glyph.bbox[0], ink + 2.0 * SIDE_BEARING)
}

/// The glyph program of `ch` and its advance (em, rounded to the `/Widths` value).
fn glyph_path(face: &CraftFont, ch: char) -> Option<(Vec<u8>, f64)> {
    let glyph = japanese_glyph_from(face, ch).ok()?;
    let (dx, width) = proportional(ch, &glyph);
    let width = (width * 1000.0).round() / 1000.0;
    let scale = 1000.0;
    // d1 is `wx wy llx lly urx ury` (ISO 32000-2 §9.6.4) with a box enclosing the glyph;
    // Acrobat draws a bullet in place of a glyph whose d1 is malformed.
    let b = if glyph.contours.is_empty() { [0.0; 4] } else { glyph.bbox };
    let mut out = format!(
        "{} 0 {} {} {} {} d1\n",
        num(width * scale),
        num(((b[0] + dx) * scale).floor()),
        num((b[1] * scale).floor()),
        num(((b[2] + dx) * scale).ceil()),
        num((b[3] * scale).ceil())
    )
    .into_bytes();
    for contour in &glyph.contours {
        let Some(first) = contour.first() else { continue };
        out.extend_from_slice(format!("{} {} m\n", num((first[0] + dx) * scale), num(first[1] * scale)).as_bytes());
        for p in contour.iter().skip(1) {
            out.extend_from_slice(format!("{} {} l\n", num((p[0] + dx) * scale), num(p[1] * scale)).as_bytes());
        }
        out.extend_from_slice(b"h\n");
    }
    out.extend_from_slice(b"f\n");
    Some((out, width))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The faces come from craft-fonts, an optional build input.
    fn without_craft_fonts(test: &str) -> bool {
        if !document_japanese_fonts_for_style(false, false).is_empty() {
            return false;
        }
        eprintln!("skipping {test}: built without craft-fonts (set CRAFT_FONTS_DIR to run it)");
        true
    }

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn no_plan_without_a_japanese_face() {
        if document_japanese_fonts_for_style(false, false).is_empty() {
            assert!(japanese_type3_plan(&chars("日本語"), false, false).is_none());
        }
    }

    #[test]
    fn codes_follow_first_use_and_ignore_duplicates() {
        if without_craft_fonts("codes_follow_first_use_and_ignore_duplicates") {
            return;
        }
        let plan = japanese_type3_plan(&chars("日本日語"), false, false).expect("plan");
        assert_eq!(plan.code('日'), Some((0, 1)));
        assert_eq!(plan.code('本'), Some((0, 2)));
        assert_eq!(plan.code('語'), Some((0, 3)));
        assert_eq!(plan.code('x'), None);
        assert!(plan.missing().is_empty());
        assert_eq!(plan.font_count(), 1);
        let w = plan.width('日').expect("width");
        assert!((w - 1.0).abs() < 0.05, "an ideograph is about one em wide: {w}");
    }

    #[test]
    fn undrawable_characters_are_listed_and_have_no_width() {
        if without_craft_fonts("undrawable_characters_are_listed_and_have_no_width") {
            return;
        }
        // U+10FFFE is a noncharacter no face has a glyph for.
        let plan = japanese_type3_plan(&chars("日\u{10fffe}"), false, false).expect("plan");
        assert_eq!(plan.missing(), ['\u{10fffe}']);
        assert!(!plan.can_draw('\u{10fffe}') && plan.width('\u{10fffe}').is_none() && plan.code('\u{10fffe}').is_none());
        assert!(plan.can_draw('日'));
    }

    #[test]
    fn a_face_with_every_glyph_is_preferred() {
        if without_craft_fonts("a_face_with_every_glyph_is_preferred") {
            return;
        }
        let text = "Ελληνικά Привет Dvořák 日本";
        let plan = japanese_type3_plan(&chars(text), true, false).expect("plan");
        let faces = document_japanese_fonts_for_style(true, false);
        let covering = faces.iter().find(|f| text.chars().filter(|c| *c != ' ').all(|c| japanese_glyph_from(f, c).is_ok()));
        if let Some(covering) = covering {
            assert!(std::ptr::eq(plan.face(), *covering), "the first face that has everything is used");
            assert!(plan.missing().is_empty());
        }
    }

    #[test]
    fn greek_and_cyrillic_are_set_proportionally() {
        if without_craft_fonts("greek_and_cyrillic_are_set_proportionally") {
            return;
        }
        let plan = japanese_type3_plan(&chars("ιiи日"), false, false).expect("plan");
        for ch in ['ι', 'и'] {
            if let Some(w) = plan.width(ch) {
                assert!(w < 1.0, "{ch} is respaced to its ink width, not a full em: {w}");
            }
        }
    }

    #[test]
    fn fonts_have_indirect_parts_a_tounicode_and_compressed_glyphs() {
        if without_craft_fonts("fonts_have_indirect_parts_a_tounicode_and_compressed_glyphs") {
            return;
        }
        let plan = japanese_type3_plan(&chars("日本語😀"), false, false).expect("plan");
        let mut added = 0;
        let fonts = build_type3(&plan, |o| {
            added += 1;
            o
        });
        assert_eq!(fonts.len(), 1);
        // One stream per glyph, the ToUnicode stream and the descriptor.
        assert_eq!(added, plan.glyphs.len() + 2);
        let f = &fonts[0];
        assert_eq!(f.chars, chars("日本語"));
        assert_eq!(f.font.name(b"Subtype"), Some(&b"Type3"[..]));
        assert_eq!(f.font.int(b"LastChar"), Some(3));
        let procs = f.font.get(b"CharProcs").and_then(Object::as_dict).expect("charprocs");
        let Some(Object::Stream(g)) = procs.get(b"g01") else { panic!("g01 is a stream") };
        assert_eq!(g.dict.name(b"Filter"), Some(&b"FlateDecode"[..]));
        let program = String::from_utf8(g.decoded().expect("decodes")).expect("ascii");
        assert!(program.contains(" d1\n") && program.ends_with("f\n"), "{program}");
        let Some(Object::Stream(tu)) = f.font.get(b"ToUnicode") else { panic!("ToUnicode is a stream") };
        let cmap = String::from_utf8(tu.decoded().expect("decodes")).expect("ascii");
        assert!(cmap.contains("<01> <65E5>") && cmap.contains("<03> <8A9E>"), "{cmap}");
        assert!(matches!(f.font.get(b"FontDescriptor"), Some(Object::Dict(_))));
    }

    #[test]
    fn more_than_240_characters_split_into_fonts_and_the_rest_is_missing() {
        if without_craft_fonts("more_than_240_characters_split_into_fonts_and_the_rest_is_missing") {
            return;
        }
        // 0x4E00.. is a run of common ideographs; the first 300 are all in the faces.
        let text: Vec<char> = (0x4E00u32..0x4E00 + 300).filter_map(char::from_u32).collect();
        let plan = japanese_type3_plan(&text, false, false).expect("plan");
        let drawn = text.iter().filter(|c| plan.can_draw(**c)).count();
        assert_eq!(drawn + plan.missing().len(), text.len());
        assert_eq!(plan.font_count(), drawn.div_ceil(MAX_TYPE3_GLYPHS));
        if drawn > MAX_TYPE3_GLYPHS {
            assert_eq!(plan.code(text[MAX_TYPE3_GLYPHS]).map(|c| c.0), Some(1));
        }
        let fonts = build_type3(&plan, |o| o);
        assert!(fonts.iter().all(|f| f.chars.len() <= MAX_TYPE3_GLYPHS));
        // The ToUnicode of a full font keeps each block within 100 entries.
        if let Some(Object::Stream(tu)) = fonts[0].font.get(b"ToUnicode") {
            let cmap = String::from_utf8(tu.decoded().expect("decodes")).expect("ascii");
            assert_eq!(cmap.matches("beginbfchar").count(), fonts[0].chars.len().div_ceil(BFCHAR_BLOCK), "{cmap}");
            assert!(cmap.lines().filter_map(|l| l.strip_suffix(" beginbfchar")).all(|n| n.parse::<usize>().is_ok_and(|n| n <= BFCHAR_BLOCK)));
            if fonts[0].chars.len() == MAX_TYPE3_GLYPHS {
                assert!(cmap.contains("<F0> <"), "codes run on across blocks");
            }
        }
    }

    #[test]
    fn characters_past_the_work_limit_are_missing() {
        if without_craft_fonts("characters_past_the_work_limit_are_missing") {
            return;
        }
        let text: Vec<char> = (0x4E00u32..0x4E00 + 2000).filter_map(char::from_u32).collect();
        let plan = japanese_type3_plan(&text, false, false).expect("plan");
        assert!(plan.font_count() <= MAX_TYPE3_FONTS);
        assert!(plan.missing().len() >= 2000 - MAX_TYPE3_GLYPHS * MAX_TYPE3_FONTS);
        assert!(!plan.can_draw(text[1999]));
    }
}
