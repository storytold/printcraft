//! Tests for the content interpreter's fail-closed behaviour: font metrics, writing modes,
//! glyph overlap, patterns / Type 3 / soft masks, inline images, alternate text, image masks and
//! the depth caps.

use pdfcraft_cos::{Dict, Object, Stream};

use super::tests::*;
use super::*;
use crate::tests_failclosed::{apply_keeping, opts_none, saved};

/// A 300×300 page with `content`, fonts /F1 (500-unit widths, as in `one_page`) plus `fonts`,
/// resources `res`, and `extra` objects numbered from 7.
fn page(fonts: &str, res: &str, content: &[u8], extra: Vec<Vec<u8>>) -> Document {
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R {fonts} >> {res} >> >>")
            .into_bytes(),
        stream("", content),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
    ];
    objs.extend(extra);
    pdf(objs)
}

/// Mark `rects` and apply; the reason the operation was refused, if it was.
fn refused(doc: &mut Document, rects: &[[f64; 4]]) -> Option<Unsupported> {
    mark(doc, 0, rects, "");
    match apply(doc, None) {
        Err(RedactError::Unsupported { page: 1, reason }) => Some(reason),
        other => {
            assert!(other.is_ok(), "{other:?}");
            None
        }
    }
}

/// The page's /XObject resources, resolved.
fn xobjects(doc: &Document) -> Dict {
    let p = pdfcraft_model::pages(doc).swap_remove(0);
    let res = doc.resolve(p.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap()
}

fn stream_of(doc: &Document, o: &Object) -> Stream {
    match &*doc.resolve(o) {
        Object::Stream(s) => s.clone(),
        other => panic!("not a stream: {other:?}"),
    }
}

/// Replace the page's /XObject resources with `name` → `r`.
fn set_xobject(doc: &mut Document, name: &[u8], r: ObjRef) {
    let page_obj = pdfcraft_model::pages(doc)[0].obj;
    doc.update_dict(page_obj, |p| {
        let mut res = p.get(b"Resources").and_then(Object::as_dict).cloned().unwrap();
        let mut xo = Dict::new();
        xo.set(name.to_vec(), Object::Ref(r));
        res.set(b"XObject".to_vec(), Object::Dict(xo));
        p.set(b"Resources".to_vec(), Object::Dict(res));
    })
    .unwrap();
}

// ---- fonts whose metrics can't be resolved ----

#[test]
fn text_in_an_unresolved_font_fails_closed_when_it_could_reach_a_mark() {
    // /F9 isn't in the resources, so the widths of its text are unknown.
    let src = b"BT /F9 10 Tf 50 100 Td (SECRET) Tj ET";
    let mut doc = page("", "", src, vec![]);
    assert_eq!(refused(&mut doc, &[[50.0, 98.0, 80.0, 108.0]]), Some(Unsupported::UnresolvedFont));
    // A mark on another line can't touch it.
    let mut doc = page("", "", src, vec![]);
    assert_eq!(refused(&mut doc, &[[50.0, 200.0, 80.0, 210.0]]), None);
    // Text before any font was selected is just as unplaceable.
    let mut doc = page("", "", b"BT 50 100 Td (SECRET) Tj ET", vec![]);
    assert_eq!(refused(&mut doc, &[[50.0, 98.0, 80.0, 108.0]]), Some(Unsupported::UnresolvedFont));
}

#[test]
fn standard_fonts_without_widths_are_not_approximated() {
    let times = "/F2 << /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >>";
    let src = b"BT /F2 10 Tf 50 100 Td (SECRET) Tj ET";
    let mut doc = page(times, "", src, vec![]);
    assert_eq!(refused(&mut doc, &[[50.0, 98.0, 80.0, 108.0]]), Some(Unsupported::UnresolvedFont));
    let helv = "/F2 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";
    let mut doc = page(helv, "", src, vec![]);
    assert_eq!(refused(&mut doc, &[[50.0, 98.0, 80.0, 108.0]]), Some(Unsupported::UnresolvedFont));
    // Courier's widths are exact (600 units), so it is redacted normally.
    let courier = "/F2 << /Type /Font /Subtype /Type1 /BaseFont /Courier >>";
    let mut doc = page(courier, "", b"BT /F2 10 Tf 50 100 Td (AB) Tj ET", vec![]);
    mark(&mut doc, 0, &[[58.0, 90.0, 70.0, 110.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 1, "{r:?}");
    assert!(content(&doc, 0).contains("[(A) -600] TJ"), "{}", content(&doc, 0));
}

#[test]
fn codes_without_a_width_fail_closed() {
    // /Widths covers codes 65–66 only; 'C' (67) has no width.
    let f = "/F2 << /Type /Font /Subtype /TrueType /BaseFont /Arial /FirstChar 65 /LastChar 66 /Widths [500 500] >>";
    let mut doc = page(f, "", b"BT /F2 10 Tf 50 100 Td (ABC) Tj ET", vec![]);
    assert_eq!(refused(&mut doc, &[[50.0, 98.0, 80.0, 108.0]]), Some(Unsupported::UnresolvedFont));
    // Text within the table is placed from it.
    let mut doc = page(f, "", b"BT /F2 10 Tf 50 100 Td (AB) Tj ET", vec![]);
    assert_eq!(refused(&mut doc, &[[50.0, 98.0, 56.0, 108.0]]), None);
    // A /Widths entry that isn't a number makes the table unusable.
    let f = "/F2 << /Type /Font /Subtype /TrueType /BaseFont /Arial /FirstChar 65 /LastChar 66 /Widths [500 /X] >>";
    let mut doc = page(f, "", b"BT /F2 10 Tf 50 100 Td (AB) Tj ET", vec![]);
    assert_eq!(refused(&mut doc, &[[50.0, 98.0, 56.0, 108.0]]), Some(Unsupported::UnresolvedFont));
}

#[test]
fn composite_fonts_need_descendant_widths_and_a_known_encoding() {
    let src = b"BT /F2 10 Tf 50 100 Td <00410042> Tj ET";
    let area = [[50.0, 98.0, 80.0, 108.0]];
    let no_widths =
        "/F2 << /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-H /DescendantFonts [<< /Subtype /CIDFontType2 /BaseFont /X >>] >>";
    assert_eq!(refused(&mut page(no_widths, "", src, vec![]), &area), Some(Unsupported::UnresolvedFont));
    let no_descendant = "/F2 << /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-H >>";
    assert_eq!(refused(&mut page(no_descendant, "", src, vec![]), &area), Some(Unsupported::UnresolvedFont));
    // A predefined CMap other than Identity maps codes to CIDs we don't have.
    let predefined =
        "/F2 << /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /UniJIS-UCS2-H /DescendantFonts [<< /Subtype /CIDFontType2 /DW 1000 >>] >>";
    assert_eq!(refused(&mut page(predefined, "", src, vec![]), &area), Some(Unsupported::UnresolvedFont));
    let good = "/F2 << /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-H /DescendantFonts [<< /Subtype /CIDFontType2 /DW 1000 >>] >>";
    let mut doc = page(good, "", src, vec![]);
    mark(&mut doc, 0, &[[60.5, 90.0, 80.0, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1, "the second glyph (60–70)");
}

#[test]
fn unplaceable_text_taints_what_follows_on_the_line() {
    // The unknown font's width is unknown, so the known-font text after it can't be placed...
    let src = b"BT /F9 10 Tf 10 100 Td (x) Tj /F1 10 Tf (ABCD) Tj ET";
    let mut doc = page("", "", src, vec![]);
    assert_eq!(refused(&mut doc, &[[200.0, 98.0, 250.0, 108.0]]), Some(Unsupported::UnresolvedFont));
    // ...until the line position is set afresh.
    let src = b"BT /F9 10 Tf 10 100 Td (x) Tj /F1 10 Tf 1 0 0 1 10 200 Tm (ABCD) Tj ET";
    let mut doc = page("", "", src, vec![]);
    mark(&mut doc, 0, &[[10.0, 198.0, 15.0, 208.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
}

// ---- writing direction and matrices ----

#[test]
fn vertical_writing_fails_closed() {
    let area = [[50.0, 20.0, 80.0, 260.0]];
    let src = b"BT /F2 10 Tf 60 250 Td <00410042> Tj ET";
    let by_name = "/F2 << /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-V /DescendantFonts [<< /Subtype /CIDFontType2 /DW 1000 >>] >>";
    assert_eq!(refused(&mut page(by_name, "", src, vec![]), &area), Some(Unsupported::VerticalWriting));
    let by_cmap = "/F2 << /Type /Font /Subtype /Type0 /BaseFont /X /Encoding 7 0 R /DescendantFonts [<< /Subtype /CIDFontType2 /DW 1000 >>] >>";
    let cmap = stream("/Type /CMap /WMode 1", b"1 begincodespacerange <0000> <FFFF> endcodespacerange");
    assert_eq!(refused(&mut page(by_cmap, "", src, vec![cmap]), &area), Some(Unsupported::VerticalWriting));
    // A column elsewhere on the page isn't affected by the interpreter, but the proof can't
    // decode the vertical text to check that nothing was there, so the operation still fails.
    let mut doc = page(by_name, "", src, vec![]);
    mark(&mut doc, 0, &[[150.0, 20.0, 180.0, 260.0]], "");
    assert!(matches!(apply(&mut doc, None), Err(RedactError::ProofIncomplete { unverifiable: 1, .. })));
}

#[test]
fn rotated_and_skewed_text_matrices_place_glyphs_through_tm_and_ctm() {
    // 90° text: A runs up from y 100 (x 92–102), B 105–110, C 110–115, D 115–120.
    let src = b"BT /F1 10 Tf 0 1 -1 0 100 100 Tm (ABCD) Tj ET";
    let mut doc = page("", "", src, vec![]);
    mark(&mut doc, 0, &[[90.0, 106.0, 105.0, 109.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 1, "{r:?}");
    assert!(content(&doc, 0).contains("[(A) -500 (CD)] TJ"), "{}", content(&doc, 0));
    assert_eq!(under(&mut doc, 0, &[[90.0, 100.0, 105.0, 105.0]]), 1, "A is still there");

    // The same text under a rotating CTM: (x, y) → (-y, x), so the text origin (100, -100)
    // lands at (100, 100) and the baseline follows Tm × CTM.
    let src = b"q 0 1 -1 0 0 0 cm BT /F1 10 Tf 1 0 0 1 100 -100 Tm (ABCD) Tj ET Q";
    let mut doc = page("", "", src, vec![]);
    mark(&mut doc, 0, &[[90.0, 106.0, 105.0, 109.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);

    // A shear: boxes are the bounding box of the sheared glyph, so a mark on the sheared top
    // corner of the first glyph removes it (and the second, which the shear also reaches).
    let src = b"BT /F1 10 Tf 1 0 1 1 50 100 Tm (AB) Tj ET";
    let mut doc = page("", "", src, vec![]);
    mark(&mut doc, 0, &[[60.0, 104.0, 70.0, 108.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 2, "{r:?}");
    assert_eq!(under(&mut doc, 0, &[[40.0, 90.0, 80.0, 110.0]]), 0);
}

#[test]
fn negative_sizes_and_scaling_are_handled() {
    // A negative size mirrors the text: A runs left from x 100 (95–100), B 90–95.
    let mut doc = page("", "", b"BT /F1 -10 Tf 1 0 0 1 100 100 Tm (AB) Tj ET", vec![]);
    mark(&mut doc, 0, &[[91.0, 90.0, 94.0, 110.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 1, "{r:?}");
    assert!(content(&doc, 0).contains("[(A) -500] TJ"), "{}", content(&doc, 0));
    // Negative horizontal scaling mirrors too.
    let mut doc = page("", "", b"BT /F1 10 Tf -100 Tz 1 0 0 1 100 100 Tm (AB) Tj ET", vec![]);
    mark(&mut doc, 0, &[[96.0, 90.0, 99.0, 110.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 1);
    // Zero scaling or size collapses the glyphs; they go when their origin is in a mark.
    let mut doc = page("", "", b"BT /F1 10 Tf 0 Tz 1 0 0 1 100 100 Tm (AB) Tj ET", vec![]);
    mark(&mut doc, 0, &[[99.0, 99.0, 101.0, 101.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 2);
    let mut doc = page("", "", b"BT /F1 0 Tf 1 0 0 1 100 100 Tm (AB) Tj ET", vec![]);
    mark(&mut doc, 0, &[[99.0, 99.0, 101.0, 101.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 2);
    // Absurd values overflow to non-finite boxes: such a glyph counts as under any mark.
    let mut doc = page("", "", b"BT /F1 1e308 Tf 1 0 0 1 100 100 Tm 1e308 Tc (A) Tj ET", vec![]);
    mark(&mut doc, 0, &[[10.0, 10.0, 20.0, 20.0]], "");
    assert!(apply(&mut doc, None).is_ok());
}

// ---- glyph overlap ----

#[test]
fn any_overlap_removes_a_glyph_and_touching_does_not() {
    // A is 50–55, B 55–60 (y 98–108).
    let src = b"BT /F1 10 Tf 50 100 Td (ABC) Tj ET";
    // The mark starts exactly where A ends: only B and C.
    let mut doc = page("", "", src, vec![]);
    mark(&mut doc, 0, &[[55.0, 90.0, 70.0, 120.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 2);
    assert!(content(&doc, 0).contains("[(A) -1000] TJ"), "{}", content(&doc, 0));
    // A tenth of a point into A takes A as well: no sliver allowance.
    let mut doc = page("", "", src, vec![]);
    mark(&mut doc, 0, &[[54.9, 90.0, 70.0, 120.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 3);
    // The same in the vertical direction: the box tops out at y 108.
    let mut doc = page("", "", src, vec![]);
    mark(&mut doc, 0, &[[40.0, 107.9, 70.0, 120.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 3);
    let mut doc = page("", "", src, vec![]);
    mark(&mut doc, 0, &[[40.0, 108.0, 70.0, 120.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().glyphs, 0);
}

// ---- patterns, Type 3, soft masks ----

const TILING: &str = "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 20 20] /XStep 20 /YStep 20";

#[test]
fn tiling_patterns_that_show_text_fail_closed() {
    let area = [[50.0, 50.0, 100.0, 100.0]];
    let src = b"/Pattern cs /P1 scn 0 0 300 300 re f";
    let res = "/Pattern << /P1 7 0 R >>";
    let text = stream(&format!("{TILING} /Resources << /Font << /F1 5 0 R >> >>"), b"BT /F1 8 Tf 0 5 Td (SECRET) Tj ET");
    assert_eq!(refused(&mut page("", res, src, vec![text]), &area), Some(Unsupported::PatternText));

    // A plain-paint pattern is fine: the fill is clipped like any path.
    let plain = stream(&format!("{TILING} /Resources << >>"), b"0 0 10 10 re f");
    assert_eq!(refused(&mut page("", res, src, vec![plain]), &area), None);

    // A pattern that can't be decoded can't be shown text-free.
    let broken = stream(&format!("{TILING} /Filter /JBIG2Decode"), b"x");
    assert_eq!(refused(&mut page("", res, src, vec![broken]), &area), Some(Unsupported::PatternUnreadable));

    // Text reached through a form inside the pattern...
    let inner = stream("/Type /XObject /Subtype /Form /BBox [0 0 20 20] /Resources << /Font << /F1 5 0 R >> >>", b"BT /F1 8 Tf (SECRET) Tj ET");
    let outer = stream(&format!("{TILING} /Resources << /XObject << /X 8 0 R >> >>"), b"/X Do");
    assert_eq!(refused(&mut page("", res, src, vec![outer, inner]), &area), Some(Unsupported::PatternText));

    // ...or a pattern that only a nested form's resources reach.
    let text = stream(&format!("{TILING} /Resources << /Font << /F1 5 0 R >> >>"), b"BT /F1 8 Tf (SECRET) Tj ET");
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Pattern << /P1 8 0 R >> >>", b"q Q");
    let mut doc = page("", "/XObject << /Fm0 7 0 R >>", b"/Fm0 Do", vec![form, text]);
    assert_eq!(refused(&mut doc, &area), Some(Unsupported::PatternText));
}

#[test]
fn type3_glyph_procedures_with_text_fail_closed_and_plain_glyphs_are_removed() {
    let font = "/F3 << /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 97 /LastChar 97 /Widths [1000] /CharProcs << /a 7 0 R >> /Encoding << /Type /Encoding /Differences [97 /a] >> >>";
    let src = b"BT /F3 10 Tf 50 100 Td (aa) Tj ET";
    let plain = stream("", b"1000 0 0 0 1000 1000 d1 0 0 1000 1000 re f");
    let mut doc = page(font, "", src, vec![plain]);
    mark(&mut doc, 0, &[[61.0, 90.0, 70.0, 120.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 1, "the second glyph (60–70): {r:?}");
    assert!(content(&doc, 0).contains("[(a) -1000] TJ"), "{}", content(&doc, 0));

    let texty = stream("", b"1000 0 d0 BT /F1 100 Tf (SECRET) Tj ET");
    let font = font.replace("/Encoding <<", "/Resources << /Font << /F1 5 0 R >> >> /Encoding <<");
    let mut doc = page(&font, "", src, vec![texty]);
    assert_eq!(refused(&mut doc, &[[61.0, 90.0, 70.0, 120.0]]), Some(Unsupported::Type3Text));
    // Without a usable /FontBBox the glyph geometry is unknown.
    let bad = "/F3 << /Type /Font /Subtype /Type3 /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 97 /Widths [1000] /CharProcs << /a 7 0 R >> >>";
    let mut doc = page(bad, "", src, vec![stream("", b"1000 0 d0")]);
    assert_eq!(refused(&mut doc, &[[61.0, 90.0, 70.0, 120.0]]), Some(Unsupported::UnresolvedFont));
}

#[test]
fn soft_mask_groups_with_text_fail_closed() {
    let src = b"/GS1 gs 0 0 300 300 re f";
    let res = "/ExtGState << /GS1 << /SMask << /Type /Mask /S /Luminosity /G 7 0 R >> >> >>";
    let area = [[50.0, 50.0, 100.0, 100.0]];
    let group = "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Group << /S /Transparency /CS /DeviceGray >>";
    let text = stream(&format!("{group} /Resources << /Font << /F1 5 0 R >> >>"), b"BT /F1 20 Tf 60 60 Td (SECRET) Tj ET");
    assert_eq!(refused(&mut page("", res, src, vec![text]), &area), Some(Unsupported::SoftMask));
    let plain = stream(group, b"1 g 0 0 300 300 re f");
    assert_eq!(refused(&mut page("", res, src, vec![plain]), &area), None);
    let broken = stream(&format!("{group} /Filter /JBIG2Decode"), b"x");
    assert_eq!(refused(&mut page("", res, src, vec![broken]), &area), Some(Unsupported::SoftMask));
}

// ---- inline images ----

#[test]
fn inline_images_whose_extent_is_ambiguous_fail_closed() {
    let area = [[200.0, 200.0, 250.0, 250.0]];
    // "ab EI cd" is 8 bytes of image data with an `EI` token inside it.
    let tricky = b"q 10 0 0 10 50 50 cm BI /W 8 /H 1 /CS /G /BPC 8 ID ab EI cd EI Q";
    assert_eq!(refused(&mut page("", "", tricky, vec![]), &area), Some(Unsupported::InlineImage));
    // The same however far into the stream it sits.
    let mut padded = b"0 g ".repeat(5000);
    padded.extend_from_slice(tricky);
    assert_eq!(refused(&mut page("", "", &padded, vec![]), &area), Some(Unsupported::InlineImage));
    // The stream ends inside the data.
    let cut = b"q 10 0 0 10 50 50 cm BI /W 8 /H 1 /CS /G /BPC 8 ID abcdefgh";
    assert_eq!(refused(&mut page("", "", cut, vec![]), &area), Some(Unsupported::InlineImage));
    // A named colour space can't be sized here.
    let named = b"BI /W 1 /H 1 /CS /Cs1 /BPC 8 ID a EI";
    assert_eq!(refused(&mut page("", "", named, vec![]), &area), Some(Unsupported::InlineImage));
    // An exact length is fine (and the image under a mark goes).
    let fine = b"q 10 0 0 10 50 50 cm BI /W 8 /H 1 /CS /G /BPC 8 ID abcdefgh EI Q";
    let mut doc = page("", "", fine, vec![]);
    mark(&mut doc, 0, &[[45.0, 45.0, 65.0, 65.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
}

#[test]
fn an_inline_image_split_across_pieces_is_handled_whole_or_refused() {
    // One inline image whose `BI … ID …data…` ends the first piece and whose `EI` starts the
    // second (#153): the joined parse sees the image whole, so the image under the mark goes,
    // and a page that ends inside the data (no `EI` anywhere) still fails closed.
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents [4 0 R 7 0 R] /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", b"q 10 0 0 10 50 50 cm BI /W 8 /H 1 /CS /G /BPC 8 ID abcdef"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        stream("", b"gh EI Q"),
    ];
    let mut doc = pdf(objs);
    mark(&mut doc, 0, &[[45.0, 45.0, 65.0, 65.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
    let c = content(&doc, 0);
    assert!(!c.contains("abcdefgh"), "the image data is gone: {c}");
    assert!(c.contains("Q"), "the rest of the content stays: {c}");

    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents [4 0 R 7 0 R] /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", b"q 10 0 0 10 50 50 cm BI /W 8 /H 1 /CS /G /BPC 8 ID abcdef"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        stream("", b"gh Q"),
    ];
    let mut doc = pdf(objs);
    assert_eq!(refused(&mut doc, &[[45.0, 45.0, 65.0, 65.0]]), Some(Unsupported::InlineImage));
}

// ---- alternate text ----

#[test]
fn inline_actual_text_of_removed_content_is_scrubbed() {
    let src = b"/Span << /ActualText (SECRETA) /Lang (en) >> BDC BT /F1 10 Tf 50 100 Td (ABC) Tj ET /Artifact << /Alt (SECRETB) >> DP EMC \
                /Span << /ActualText (KEEPME) >> BDC BT /F1 10 Tf 50 200 Td (XYZ) Tj ET EMC";
    let mut doc = page("", "", src, vec![]);
    mark(&mut doc, 0, &[[50.0, 90.0, 70.0, 120.0]], "");
    apply(&mut doc, None).unwrap();
    let c = content(&doc, 0);
    assert!(!c.contains("SECRETA") && !c.contains("SECRETB"), "{c}");
    assert!(c.contains("KEEPME"), "text elsewhere keeps its alternate: {c}");
    assert!(c.contains("/Lang"), "other properties stay: {c}");
    assert!(c.contains("BDC") && c.contains("EMC"), "{c}");
}

/// A two-piece page: object 4 is piece one, object 7 piece two (both object bodies, as
/// [`stream`] or [`flate`] write them).
fn two_pieces(first: Vec<u8>, second: Vec<u8>, extra_res: &str, extra: Vec<Vec<u8>>) -> Document {
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents [4 0 R 7 0 R] /Resources << /Font << /F1 5 0 R >> {extra_res} >> >>")
            .into_bytes(),
        first,
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        second,
    ];
    objs.extend(extra);
    pdf(objs)
}

/// An object body holding `data` as a Flate-compressed stream.
fn flate(data: &[u8]) -> Vec<u8> {
    let s = Stream::flate(Dict::new(), data);
    let raw = s.raw.as_ref();
    let mut v = format!("<< /Filter /FlateDecode /Length {} >>\nstream\n", raw.len()).into_bytes();
    v.extend_from_slice(raw);
    v.extend_from_slice(b"\nendstream");
    v
}

#[test]
fn an_inline_alternate_text_in_another_piece_is_scrubbed() {
    // The BDC's inline /Alt dict sits in piece one, the removal under its block in piece two
    // (F3): piece one is rewritten without the alternate text, and no saved byte keeps it.
    let mut doc =
        two_pieces(stream("", b"/Span << /Alt (ALTSECRET01) >> BDC"), stream("", b"BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET EMC"), "", vec![]);
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
    let (r, proof) = apply_with_proof(&mut doc, None, &opts_none(), &ProofOptions::default()).unwrap();
    assert_eq!(r.glyphs, 4, "{r:?}");
    assert!(proof.passed(), "{}", proof.to_text());
    let c = content(&doc, 0);
    assert!(!c.contains("1234"), "{c}");
    assert!(!c.contains("ALTSECRET01"), "the alternate text stayed in the page: {c}");
    assert!(c.contains("BDC") && c.contains("EMC"), "the marked content stays: {c}");
    let bytes = saved(&doc);
    assert!(!bytes.windows(4).any(|w| w == b"1234"), "removed text left in the saved file");
    assert!(!bytes.windows(11).any(|w| w == b"ALTSECRET01"), "the alternate text stayed in the saved file");
}

#[test]
fn a_named_property_list_in_another_piece_is_scrubbed() {
    // The property list is named by the BDC in piece one, the removal under its block happens in
    // piece two: the page's /Properties entry is rewritten without its /Alt.
    let mut doc = two_pieces(
        stream("", b"/P /P1 BDC"),
        stream("", b"BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET EMC"),
        "/Properties << /P1 8 0 R >>",
        vec![b"<< /Alt (ALTSECRET01) /MCID 0 >>".to_vec()],
    );
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
    let (r, proof) = apply_with_proof(&mut doc, None, &opts_none(), &ProofOptions::default()).unwrap();
    assert_eq!(r.glyphs, 4, "{r:?}");
    assert!(proof.passed(), "{}", proof.to_text());
    let p = pdfcraft_model::pages(&doc).swap_remove(0);
    let res = doc.resolve(p.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let props = doc.resolve(res.get(b"Properties").unwrap()).as_dict().cloned().unwrap();
    let list = doc.resolve(props.get(b"P1").unwrap()).as_dict().cloned().unwrap();
    assert!(list.get(b"Alt").is_none(), "{list:?}");
    assert!(list.get(b"MCID").is_some(), "the marked-content id stays for the structure tree: {list:?}");
    let bytes = saved(&doc);
    assert!(!bytes.windows(4).any(|w| w == b"1234"), "removed text left in the saved file");
    assert!(!bytes.windows(11).any(|w| w == b"ALTSECRET01"), "the alternate text stayed in the saved file");
}

#[test]
fn a_short_alternate_text_in_a_compressed_piece_is_scrubbed() {
    // A five-character alternate text is under the proof's needle floor and piece one is
    // compressed, so only the interpreter's own scrub can be relied on (F3): the decoded output
    // holds no trace of it.
    let first = flate(b"/Span << /Alt (abcde) >> BDC");
    let mut doc = two_pieces(first, stream("", b"BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET EMC"), "", vec![]);
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
    let (r, proof) = apply_with_proof(&mut doc, None, &opts_none(), &ProofOptions::default()).unwrap();
    assert_eq!(r.glyphs, 4, "{r:?}");
    assert!(proof.passed(), "{}", proof.to_text());
    let c = content(&doc, 0);
    assert!(!c.contains("1234"), "{c}");
    assert!(!c.contains("abcde"), "the alternate text stayed in the decoded output: {c}");
    assert!(c.contains("BDC") && c.contains("EMC"), "the marked content stays: {c}");
}

// ---- image masks ----

fn gray4(extra: &str, data: &[u8]) -> Vec<u8> {
    stream(&format!("/Type /XObject /Subtype /Image /Width 4 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 {extra}"), data)
}

/// Apply a mark over the left half of the 40-point wide image drawn at (100, 100).
fn clear_left_half(doc: &mut Document) -> Report {
    mark(doc, 0, &[[95.0, 95.0, 120.0, 115.0]], "");
    apply(doc, None).unwrap()
}

fn cleared_copy(doc: &Document) -> Stream {
    let xo = xobjects(doc);
    let (_, v) = xo.iter().find(|(k, _)| k.starts_with(b"PCRedacted")).expect("a cleared copy");
    stream_of(doc, v)
}

const DRAW: &[u8] = b"q 40 0 0 10 100 100 cm /Im1 Do Q";
const IM1: &str = "/XObject << /Im1 7 0 R >>";

#[test]
fn soft_masks_are_cleared_in_a_copy_and_nothing_recoverable_stays() {
    let img = gray4("/SMask 8 0 R /Alternates [<< /Image 9 0 R >>] /Metadata 9 0 R", &[200, 200, 200, 200]);
    let smask = gray4("", &[255, 255, 255, 255]);
    let alt = gray4("", &[7, 7, 7, 7]);
    let mut doc = page("", IM1, DRAW, vec![img, smask, alt]);
    let r = clear_left_half(&mut doc);
    assert_eq!((r.images_cleared, r.images_removed), (1, 0), "{r:?}");
    let copy = cleared_copy(&doc);
    assert_eq!(copy.decoded().unwrap(), [0, 0, 200, 200]);
    assert!(!copy.dict.contains(b"Alternates") && !copy.dict.contains(b"Metadata"), "{:?}", copy.dict);
    let mask = copy.dict.get(b"SMask").and_then(Object::as_ref).expect("the mask is kept, as a copy");
    assert_ne!(mask, ObjRef::new(8, 0));
    assert_eq!(stream_of(&doc, &Object::Ref(mask)).decoded().unwrap(), [0, 0, 255, 255], "transparent where cleared");
    // The shared originals are untouched.
    assert_eq!(stream_of(&doc, &Object::Ref(ObjRef::new(7, 0))).decoded().unwrap(), [200; 4]);
    assert_eq!(stream_of(&doc, &Object::Ref(ObjRef::new(8, 0))).decoded().unwrap(), [255; 4]);
}

#[test]
fn stencil_and_colour_key_masks_are_neutralised() {
    // An explicit /Mask stream (1 = masked out): the covered half becomes 1s.
    let img = gray4("/Mask 8 0 R", &[200, 200, 200, 200]);
    let mask = stream("/Type /XObject /Subtype /Image /Width 4 /Height 1 /ImageMask true /BitsPerComponent 1", &[0b0000_0000]);
    let mut doc = page("", IM1, DRAW, vec![img, mask]);
    clear_left_half(&mut doc);
    let copy = cleared_copy(&doc);
    let m = copy.dict.get(b"Mask").and_then(Object::as_ref).expect("mask copy");
    assert_eq!(stream_of(&doc, &Object::Ref(m)).decoded().unwrap(), [0b1100_0000]);
    assert_eq!(stream_of(&doc, &Object::Ref(ObjRef::new(8, 0))).decoded().unwrap(), [0], "original mask untouched");

    // A colour-key mask goes with the pixels it keyed.
    let img = gray4("/Mask [0 10]", &[200, 200, 200, 200]);
    let mut doc = page("", IM1, DRAW, vec![img]);
    clear_left_half(&mut doc);
    assert!(!cleared_copy(&doc).dict.contains(b"Mask"));

    // A mask that can't be cleared (a JPEG) means the image can't be: it is removed whole.
    let img = gray4("/SMask 8 0 R", &[200, 200, 200, 200]);
    let jpeg = gray4("/Filter /DCTDecode", &[1, 2, 3]);
    let mut doc = page("", IM1, DRAW, vec![img, jpeg]);
    let r = clear_left_half(&mut doc);
    assert_eq!((r.images_removed, r.images_cleared), (1, 0), "{r:?}");
    assert!(!content(&doc, 0).contains("Do"), "{}", content(&doc, 0));
}

#[test]
fn indexed_lab_and_devicen_images_are_cleared_to_a_fixed_fill() {
    let head = "/Type /XObject /Subtype /Image /Width 4 /Height 1 /BitsPerComponent 8";
    let indexed = stream(&format!("{head} /ColorSpace [/Indexed /DeviceRGB 2 <ff0000 00ff00 0000ff>]"), &[2, 2, 1, 1]);
    let mut doc = page("", IM1, DRAW, vec![indexed]);
    clear_left_half(&mut doc);
    assert_eq!(cleared_copy(&doc).decoded().unwrap(), [0, 0, 1, 1]);

    let lab = stream(&format!("{head} /ColorSpace [/Lab << /WhitePoint [0.95 1 1.09] >>]"), &[0x80; 12]);
    let mut doc = page("", IM1, DRAW, vec![lab]);
    clear_left_half(&mut doc);
    assert_eq!(cleared_copy(&doc).decoded().unwrap(), [0, 0, 0, 0, 0, 0, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80]);

    // DeviceN: one component per colorant.
    let devn = stream(&format!("{head} /ColorSpace [/DeviceN [/A /B] /DeviceRGB << >>]"), &[9; 8]);
    let mut doc = page("", IM1, DRAW, vec![devn]);
    clear_left_half(&mut doc);
    assert_eq!(cleared_copy(&doc).decoded().unwrap(), [0, 0, 0, 0, 9, 9, 9, 9]);
}

#[test]
fn partly_covered_pixels_are_cleared_whole() {
    // The mark reaches 0.5 points into the third pixel (x 120–130): it goes too.
    let mut doc = page("", IM1, DRAW, vec![gray4("", &[255; 4])]);
    mark(&mut doc, 0, &[[95.0, 95.0, 120.5, 115.0]], "");
    apply(&mut doc, None).unwrap();
    assert_eq!(cleared_copy(&doc).decoded().unwrap(), [0, 0, 0, 255]);
}

#[test]
fn images_that_inflate_past_their_size_are_removed() {
    // A 4×1 image whose data inflates to 1 MiB isn't the image it says it is.
    let mut doc = page("", IM1, DRAW, vec![]);
    let mut d = Dict::new();
    for (k, v) in [("Type", "XObject"), ("Subtype", "Image"), ("ColorSpace", "DeviceGray")] {
        d.set(k.as_bytes().to_vec(), Object::name(v));
    }
    d.set(b"Width".to_vec(), Object::Int(4));
    d.set(b"Height".to_vec(), Object::Int(1));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    let big = doc.add(Object::Stream(Stream::flate(d, &vec![255u8; 1 << 20])));
    set_xobject(&mut doc, b"Im1", big);
    let r = clear_left_half(&mut doc);
    assert_eq!((r.images_removed, r.images_cleared), (1, 0), "{r:?}");
}

// ---- depth caps ----

/// A page drawing `/Fm0` (form `7 0 R`), the head of a chain of `forms` forms each drawing the
/// next as `/Next`; `body` is each form's own content.
fn nested(forms: usize, body: &dyn Fn(usize) -> String) -> Document {
    let mut extra = Vec::new();
    for i in 0..forms {
        let res = if i + 1 < forms { format!("/XObject << /Next {} 0 R >>", 8 + i) } else { String::new() };
        let dict = format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> {res} >>");
        extra.push(stream(&dict, body(i).as_bytes()));
    }
    page("", "/XObject << /Fm0 7 0 R >>", b"/Fm0 Do", extra)
}

#[test]
fn cyclic_and_fanning_out_forms_terminate() {
    let area = [[50.0, 90.0, 120.0, 120.0]];
    // A form that draws itself ten times (and has text under the mark).
    let selfref = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> /XObject << /Fm0 7 0 R >> >>",
        format!("BT /F1 10 Tf 50 100 Td (SECRET) Tj ET {}", "/Fm0 Do ".repeat(10)).as_bytes(),
    );
    let mut doc = page("", "/XObject << /Fm0 7 0 R >>", b"/Fm0 Do", vec![selfref]);
    mark(&mut doc, 0, &area, "");
    let r = apply(&mut doc, None).unwrap();
    assert!(r.glyphs >= 1 && r.forms_removed >= 1, "{r:?}");
    assert_eq!(under(&mut doc, 0, &area), 0);

    // Each of 14 nested forms draws the next ten times: 10^14 expansions if not capped.
    let mut doc = nested(14, &|_| format!("BT /F1 10 Tf 50 100 Td (SECRET) Tj ET {}", "/Next Do ".repeat(10)));
    mark(&mut doc, 0, &area, "");
    // The budget removes what it can't walk; the page is too deep for the proof to read.
    assert!(matches!(apply_keeping(&mut doc), Err(RedactError::ProofIncomplete { .. })));
    assert_eq!(under(&mut doc, 0, &area), 0);
}

#[test]
fn forms_nested_too_deep_are_removed_whole() {
    let area = [[50.0, 90.0, 120.0, 120.0]];
    // Text only in the deepest form, 20 levels down: past the cap, the form goes whole.
    let mut doc = nested(20, &|i| if i == 19 { "BT /F1 10 Tf 50 100 Td (SECRET) Tj ET".into() } else { "/Next Do".into() });
    mark(&mut doc, 0, &area, "");
    // The page can't be read past the cap, so the proof is incomplete and the operation fails
    // (the document is to be discarded); what the pass did is still checkable.
    assert!(matches!(apply_keeping(&mut doc), Err(RedactError::ProofIncomplete { .. })));
    assert_eq!(under(&mut doc, 0, &area), 0);
    // Within the cap, the text is cut out in place.
    let mut doc = nested(5, &|i| if i == 4 { "BT /F1 10 Tf 50 100 Td (SECRET) Tj ET".into() } else { "/Next Do".into() });
    mark(&mut doc, 0, &area, "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.glyphs, r.forms_removed), (6, 0), "{r:?}");
}

#[test]
fn form_streams_that_inflate_past_the_bound_are_removed() {
    let mut doc = page("", "/XObject << /Fm0 7 0 R >>", b"/Fm0 Do", vec![]);
    let mut d = Dict::new();
    for (k, v) in [("Type", "XObject"), ("Subtype", "Form")] {
        d.set(k.as_bytes().to_vec(), Object::name(v));
    }
    d.set(b"BBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(300), Object::Int(300)]));
    let big = doc.add(Object::Stream(Stream::flate(d, &vec![b' '; 70 << 20])));
    set_xobject(&mut doc, b"Fm0", big);
    mark(&mut doc, 0, &[[50.0, 90.0, 120.0, 120.0]], "");
    // The form is dropped, but its source can't be read for the proof: the operation fails.
    assert!(matches!(apply(&mut doc, None), Err(RedactError::ProofIncomplete { .. })));
}
