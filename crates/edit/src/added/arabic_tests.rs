//! Arabic items too big for one Type 3 font. Shaped clusters and glyph outlines are injected here,
//! so these run without the craft-fonts Arabic face. Shaping itself is covered by the face tests in
//! `tests.rs`, which run where the face is built in.

use std::cell::Cell;

use super::*;

/// The fingerprint of an item below the old limit, from the code before the change: its content
/// stream, its new fonts and every object of the document.
const SMALL_ITEM_FINGERPRINT: u64 = 14_135_892_979_695_233_112;

/// A cluster that shows one character with the single glyph `id`, at `advance` em.
fn cluster(id: u32, advance: f64) -> ShapedCluster {
    ShapedCluster { glyphs: vec![(id, [0.0, 0.0])], advance, text: "ب".into() }
}

/// One run of `clusters` as a line of an item at `item()`'s size.
fn line(clusters: Vec<ShapedCluster>, rtl: bool) -> LaidLine {
    let text = clusters.iter().map(|c| c.text.as_str()).collect();
    let width = clusters.iter().map(|c| c.advance).sum::<f64>() * item().size;
    (text, rtl, vec![Piece::Arabic(clusters)], width)
}

/// A glyph outline: a box on the baseline, `advance` em wide.
fn boxed(advance: f64) -> GlyphOutline {
    let contours = vec![vec![[0.0, 0.0], [advance, 0.0], [advance, 0.5], [0.0, 0.5]]];
    GlyphOutline { contours, width: advance, bbox: [0.0, 0.0, advance, 0.5] }
}

fn item() -> AddedText {
    AddedText { rect: [72.0, 600.0, 540.0, 700.0], ..AddedText::default() }
}

/// Every object of `doc`, serialised: equal snapshots mean the document did not change.
fn snapshot(doc: &Document) -> Vec<u8> {
    let mut bytes = Vec::new();
    for num in doc.object_numbers() {
        let obj = doc.get(ObjRef { num, generation: doc.generation(num) });
        pdfcraft_cos::serialize(&obj, &mut bytes);
        bytes.push(b'\n');
    }
    bytes
}

/// The fonts an item drew, in creation order, with their glyph counts.
fn fonts_of(doc: &Document, fonts: &Dict) -> Vec<(Vec<u8>, usize)> {
    fonts
        .iter()
        .map(|(name, font)| {
            let glyphs = match doc.dict(font).as_ref().and_then(|f| f.get(b"CharProcs")) {
                Some(Object::Dict(procs)) => procs.len(),
                _ => 0,
            };
            (name.clone(), glyphs)
        })
        .collect()
}

/// The font and code of every glyph the content stream shows, in drawing order.
fn glyphs_shown(out: &[u8]) -> Vec<(Vec<u8>, u8)> {
    let mut font = Vec::new();
    let mut shown = Vec::new();
    for op in pdfcraft_content::parse(out).ops {
        if op.is("Tf") {
            font = op.name(0).unwrap_or_default().to_vec();
        } else if op.is("Tj")
            && let Some(Object::String(s)) = op.operands.first()
        {
            shown.push((font.clone(), s.bytes.first().copied().unwrap_or_default()));
        }
    }
    shown
}

/// 64-bit FNV-1a: a stable fingerprint for the golden check.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

#[test]
fn an_item_past_the_old_glyph_limit_gets_a_font_per_240_glyphs() {
    // Sixteen fonts of 240 glyphs was the most an item could use. This one needs 3,841 glyphs.
    let n = 16 * GLYPHS_PER_FONT + 1;
    let laid = vec![line((0..n).map(|i| cluster(i as u32 + 1, 0.5)).collect(), false)];
    let mut doc = Document::new_empty();
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    let drawn = draw_laid_arabic(&mut doc, &item(), &laid, &Dict::new(), &mut fonts, &mut out, |_| Ok(boxed(0.5)));
    assert!(drawn.is_ok(), "{drawn:?}");
    let made = fonts_of(&doc, &fonts);
    let mut expected = vec![GLYPHS_PER_FONT; 16];
    expected.push(1);
    assert_eq!(made.iter().map(|(_, glyphs)| *glyphs).collect::<Vec<_>>(), expected);
    let shown = glyphs_shown(&out);
    assert_eq!(shown.len(), n);
    for (k, (font, code)) in shown.iter().enumerate() {
        assert_eq!(font, &made[k / GLYPHS_PER_FONT].0, "glyph {k} is drawn from the wrong font");
        assert_eq!(usize::from(*code), k % GLYPHS_PER_FONT + 1, "glyph {k} has the wrong code");
    }
}

#[test]
fn an_outline_missing_from_a_later_font_leaves_the_document_unchanged() {
    // Two fonts' worth of glyphs; the glyph in the second font has no outline.
    let n = 2 * GLYPHS_PER_FONT;
    let laid = vec![line((0..n).map(|i| cluster(i as u32 + 1, 0.5)).collect(), false)];
    let mut doc = Document::new_empty();
    let before = snapshot(&doc);
    let missing = n as u32;
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    let drawn = draw_laid_arabic(&mut doc, &item(), &laid, &Dict::new(), &mut fonts, &mut out, |id| {
        if id == missing { Err(GlyphError::Missing) } else { Ok(boxed(0.5)) }
    });
    assert!(matches!(&drawn, Err(EditError::Invalid(msg)) if msg.contains("can't show")), "{drawn:?}");
    assert_eq!(snapshot(&doc), before);
    assert!(fonts.is_empty() && out.is_empty());
}

#[test]
fn an_item_below_the_old_limit_is_drawn_exactly_as_before() {
    // A left-to-right line with a Latin run, a space and a two-glyph cluster, then a right-to-left
    // line. Glyphs repeat across the two lines and share one font.
    let space = ShapedCluster { glyphs: vec![(6, [0.0, 0.0])], advance: 0.25, text: " ".into() };
    let ligature = ShapedCluster { glyphs: vec![(4, [0.0, 0.0]), (5, [-0.1, 0.2])], advance: 0.6, text: "لا".into() };
    let mixed: LaidLine =
        ("Hi ب".into(), false, vec![Piece::Latin("Hi ".into()), Piece::Arabic(vec![cluster(1, 0.5), cluster(2, 0.5), space, ligature])], 4.0);
    let rtl = line(vec![cluster(2, 0.5), cluster(9, 0.75)], true);
    let laid = vec![mixed, rtl];
    let mut doc = Document::new_empty();
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    let drawn = draw_laid_arabic(&mut doc, &item(), &laid, &Dict::new(), &mut fonts, &mut out, |id| Ok(boxed(0.5 + f64::from(id) * 0.01)));
    assert!(drawn.is_ok(), "{drawn:?}");
    assert_eq!(fonts.len(), 1);
    let mut bytes = out.clone();
    pdfcraft_cos::serialize(&Object::Dict(fonts.clone()), &mut bytes);
    bytes.extend(snapshot(&doc));
    assert_eq!(fnv1a(&bytes), SMALL_ITEM_FINGERPRINT, "output changed; the golden is the code before the sharding change");
}

#[test]
fn long_text_with_few_distinct_glyphs_needs_one_font() {
    // 5,000 clusters that repeat three glyphs: one font, and one outline read per glyph.
    let clusters: Vec<ShapedCluster> = (0..5_000u32).map(|i| cluster(1 + i % 3, 0.5)).collect();
    let laid = vec![line(clusters, false)];
    let reads = Cell::new(0usize);
    let mut doc = Document::new_empty();
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    let drawn = draw_laid_arabic(&mut doc, &item(), &laid, &Dict::new(), &mut fonts, &mut out, |_| {
        reads.set(reads.get() + 1);
        Ok(boxed(0.5))
    });
    assert!(drawn.is_ok(), "{drawn:?}");
    assert_eq!(fonts_of(&doc, &fonts).iter().map(|(_, glyphs)| *glyphs).collect::<Vec<_>>(), [3]);
    assert_eq!(reads.get(), 3);
    assert_eq!(glyphs_shown(&out).len(), 5_000);
}

#[test]
fn distinct_clusters_past_the_old_limit_share_their_glyph_outlines() {
    // 3,841 different clusters (their advances differ) built from four glyphs: four outlines are read.
    let clusters: Vec<ShapedCluster> = (0..3_841u32)
        .map(|i| ShapedCluster { glyphs: vec![(1 + i % 4, [0.0, 0.0])], advance: 0.5 + f64::from(i) * 1e-6, text: "ب".into() })
        .collect();
    let laid = vec![line(clusters, false)];
    let reads = Cell::new(0usize);
    let mut doc = Document::new_empty();
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    let drawn = draw_laid_arabic(&mut doc, &item(), &laid, &Dict::new(), &mut fonts, &mut out, |_| {
        reads.set(reads.get() + 1);
        Ok(boxed(0.5))
    });
    assert!(drawn.is_ok(), "{drawn:?}");
    assert_eq!(fonts.len(), 17);
    assert_eq!(reads.get(), 4);
    assert_eq!(glyphs_shown(&out).len(), 3_841);
}

#[test]
fn hostile_clusters_draw_empty_or_fail_without_changing_the_document() {
    // A cluster with no glyphs and no text draws nothing, but it is still one glyph of a font.
    let empty = ShapedCluster { glyphs: Vec::new(), advance: 0.0, text: String::new() };
    let laid = vec![line(vec![empty, cluster(1, 0.5)], false)];
    let mut doc = Document::new_empty();
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    let drawn = draw_laid_arabic(&mut doc, &item(), &laid, &Dict::new(), &mut fonts, &mut out, |_| Ok(boxed(0.5)));
    assert!(drawn.is_ok(), "{drawn:?}");
    assert_eq!(glyphs_shown(&out).len(), 2);
    // A glyph id the face doesn't have fails the item before anything is written.
    let before = snapshot(&doc);
    let laid = vec![line(vec![cluster(1, 0.5), cluster(u32::MAX, 0.5)], false)];
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    let drawn = draw_laid_arabic(&mut doc, &item(), &laid, &Dict::new(), &mut fonts, &mut out, |id| {
        if id == u32::MAX { Err(GlyphError::Missing) } else { Ok(boxed(0.5)) }
    });
    assert!(drawn.is_err());
    assert_eq!(snapshot(&doc), before);
    assert!(fonts.is_empty() && out.is_empty());
}

#[test]
fn a_font_name_the_page_already_uses_is_not_reused() {
    let laid = vec![line(vec![cluster(1, 0.5)], false)];
    let mut doc = Document::new_empty();
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    let drawn = draw_laid_arabic(&mut doc, &item(), &laid, &Dict::new(), &mut fonts, &mut out, |_| Ok(boxed(0.5)));
    assert!(drawn.is_ok(), "{drawn:?}");
    let first = fonts_of(&doc, &fonts)[0].0.clone();
    // The same item on a page whose resources already use that name.
    let mut taken = Dict::new();
    taken.set(first.clone(), Object::Null);
    let mut doc = Document::new_empty();
    let (mut fonts, mut out) = (Dict::new(), Vec::new());
    let drawn = draw_laid_arabic(&mut doc, &item(), &laid, &taken, &mut fonts, &mut out, |_| Ok(boxed(0.5)));
    assert!(drawn.is_ok(), "{drawn:?}");
    let second = fonts_of(&doc, &fonts)[0].0.clone();
    assert_ne!(second, first);
    assert!(second.starts_with(b"PCAr"), "{}", String::from_utf8_lossy(&second));
}
