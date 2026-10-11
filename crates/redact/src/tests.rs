use std::sync::Arc;

use pdfcraft_annot::{Meta, NewAnnotation, Shape, Style, add_annotation, rect_quad};
use pdfcraft_cos::{Document, SaveOptions, write_full, write_incremental};

use super::*;

pub(super) fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut v = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    v.extend_from_slice(data);
    v.extend_from_slice(b"\nendstream");
    v
}

pub(super) fn pdf(objs: Vec<Vec<u8>>) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

/// A font where every glyph is 500 units wide (so a 10 pt glyph advances 5 pt).
pub(super) const FONT: &str = "<< /Type /Font /Subtype /TrueType /BaseFont /Arial /FirstChar 32 /LastChar 126 /Widths 95 0 R /FontDescriptor << /Ascent 800 /Descent -200 >> >>";

pub(super) fn widths() -> Vec<u8> {
    format!("[{}]", vec!["500"; 95].join(" ")).into_bytes()
}

/// One 300×300 page with `content`, font /F1, and extra resources/objects.
pub(super) fn one_page(content: &[u8], extra_res: &str, extra: Vec<Vec<u8>>) -> Document {
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> {extra_res} >> >>")
            .into_bytes(),
        stream("", content),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
    ];
    objs.extend(extra);
    pdf(objs)
}

pub(super) fn mark(doc: &mut Document, page: usize, rects: &[[f64; 4]], overlay: &str) {
    let shape = Shape::Redact { quads: rects.iter().map(|r| rect_quad(*r)).collect(), overlay: overlay.into(), look: Default::default() };
    let style = Style::default_for(&shape);
    add_annotation(doc, &NewAnnotation { page, shape, style, contents: String::new(), author: "Tester".into() }, &Meta::default()).unwrap();
}

/// The decoded content streams of a page, joined.
pub(super) fn content(doc: &Document, page: usize) -> String {
    let p = &pdfcraft_model::pages(doc)[page];
    let (_, data) = page_streams(doc, &p.dict, page).unwrap();
    data.iter().map(|d| String::from_utf8_lossy(d).into_owned()).collect::<Vec<_>>().join("\n")
}

/// Glyphs (and inline images) of page `page` under `rects` (the verifier's count).
pub(super) fn under(doc: &mut Document, page: usize, rects: &[[f64; 4]]) -> usize {
    let p = pdfcraft_model::pages(doc).swap_remove(page);
    let (_, data) = page_streams(doc, &p.dict, page).unwrap();
    let res = p.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    let mut rep = Report::default();
    let mut scope = Scope::new(rects, Mode::Verify, &mut rep);
    process(doc, &mut scope, &data, &res, pdfcraft_content::Matrix::IDENTITY).residue
}

pub(super) fn reopen(doc: &Document) -> Document {
    Document::open(Arc::new(write_incremental(doc, &SaveOptions::default()).unwrap())).unwrap()
}

#[test]
fn glyphs_under_a_mark_go_and_the_rest_stays_put() {
    // "AB1234CD" from x = 10 at 10 pt: each glyph 5 pt wide; 1234 spans x 20–40.
    let mut doc = one_page(b"BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET", "", vec![]);
    assert_eq!(under(&mut doc, 0, &[[40.0, 95.0, 50.0, 110.0]]), 2, "C and D before");
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.marks, r.glyphs), (1, 4));
    let c = content(&doc, 0);
    assert!(c.contains("[(AB) -2000 (CD)] TJ"), "{c}");
    assert!(!c.contains("1234"));
    assert_eq!(under(&mut doc, 0, &[[40.0, 95.0, 50.0, 110.0]]), 2, "C and D are where they were");
    assert_eq!(under(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]]), 0);
    // The mark became a black box drawn into the page; the annotation is gone.
    assert!(marks(&doc).is_empty());
    assert!(c.contains("0 0 0 rg") && c.contains("20 95 20 15 re f"), "{c}");
    let doc = reopen(&doc);
    assert!(!content(&doc, 0).contains("1234"));
}

#[test]
fn added_text_parameters_do_not_keep_redacted_text() {
    // An Edit ▸ Add content item: its stream dictionary keeps the source text under /PCAdded.
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents [4 0 R] /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("/PCMark /Added /PCAdded << /Kind /Text /Text (AB1234CD) >>", b"BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
    ]);
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
    apply(&mut doc, None).unwrap();
    assert!(!content(&doc, 0).contains("1234"));
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!bytes.windows(4).any(|w| w == b"1234"), "redacted text left in the saved file");
}

#[test]
fn operators_split_across_content_streams_are_redacted_whole() {
    // One content stream in three pieces, split between tokens: a marked-content dictionary
    // ends in the second piece, and the TJ array that ends it shows its glyphs with the
    // operator in the third.
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents [4 0 R 7 0 R 8 0 R] /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", b"/P << /MCID 0"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        stream("", b">> BDC BT /F1 10 Tf 10 100 Td [(AB1234CD)]"),
        stream("", b"TJ ET EMC BT /F1 10 Tf 10 200 Td (KEEP) Tj ET"),
    ]);
    assert_eq!(under(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]]), 4, "the verifier sees the split TJ");
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 4, "{r:?}");
    let c = content(&doc, 0);
    assert!(!c.contains("1234"), "redacted glyphs left in the page: {c}");
    assert!(c.contains("KEEP"), "{c}");
    assert_eq!(under(&mut doc, 0, &[[40.0, 95.0, 50.0, 110.0]]), 2, "C and D are where they were");
    // Every operator still has its operands: none was cut off from them.
    for op in pdfcraft_content::parse(c.as_bytes()).ops {
        let want = match op.op.as_slice() {
            b"TJ" | b"Tj" => 1,
            b"BDC" | b"Tf" | b"Td" => 2,
            _ => continue,
        };
        assert_eq!(op.operands.len(), want, "{} lost its operands in {c}", String::from_utf8_lossy(&op.op));
    }
    let doc = reopen(&doc);
    assert!(!content(&doc, 0).contains("1234"));
}

#[test]
fn kerning_spacing_scaling_and_line_operators_are_honoured() {
    // TJ kerning, character and word spacing, 50% horizontal scaling, ' and ".
    let src = b"BT /F1 10 Tf 2 Tc 4 Tw 50 Tz 12 TL 0 200 Td [(AB) -1000 (C D)] TJ (EF) ' 1 0 (GH) \" ET";
    let mut doc = one_page(src, "", vec![]);
    // Line 1 (y 200): A at 0, B at 3.5 (advance (5+2)×0.5), kern +5 → C at 12, space at 15.5,
    // D at 21 (space advance (5+2+4)×0.5 = 5.5). Remove C only.
    mark(&mut doc, 0, &[[12.2, 195.0, 14.8, 210.0]], "");
    // Line 2 (y 188) "EF" and line 3 (y 176) "GH" (Tw 1, Tc 0): remove F and G.
    mark(&mut doc, 0, &[[3.8, 186.0, 6.0, 190.0], [0.2, 171.0, 2.3, 180.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 3, "{r:?}");
    let c = content(&doc, 0);
    assert!(c.contains("[(AB) -1700 ( D)] TJ"), "{c}");
    assert!(c.contains("T*\n[(E) -700] TJ"), "{c}");
    assert!(c.contains("1 Tw\n0 Tc\nT*\n[-500 (H)] TJ"), "{c}");
    // Everything else stays readable where it was.
    assert_eq!(under(&mut doc, 0, &[[20.0, 195.0, 30.0, 210.0]]), 1, "D");
    assert_eq!(under(&mut doc, 0, &[[2.6, 171.0, 6.0, 180.0]]), 1, "H");
}

#[test]
fn rotated_text_and_composite_fonts() {
    // A Type0 Identity-H font (two-byte codes) with /W widths, and a 90° text matrix.
    let t0 = b"<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-H /DescendantFonts [<< /Type /Font /Subtype /CIDFontType2 /BaseFont /X /DW 1000 /W [1 [500 500] 3 4 250] >>] >>".to_vec();
    let src = b"BT /F2 10 Tf 0 1 -1 0 100 50 Tm <0001000200030004> Tj ET";
    let mut doc = one_page(src, "", vec![]);
    let font_ref = doc.add(Object::Dict(Dict::new()));
    let parsed = {
        let mut lx = pdfcraft_cos::Lexer::new(&t0, 0);
        lx.object().unwrap()
    };
    doc.set(font_ref, parsed);
    let page = pdfcraft_model::pages(&doc)[0].obj;
    doc.update_dict(page, |d| {
        let mut res = d.get(b"Resources").and_then(Object::as_dict).cloned().unwrap();
        let mut fonts = res.get(b"Font").and_then(Object::as_dict).cloned().unwrap();
        fonts.set(b"F2".to_vec(), Object::Ref(font_ref));
        res.set(b"Font".to_vec(), Object::Dict(fonts));
        d.set(b"Resources".to_vec(), Object::Dict(res));
    })
    .unwrap();
    // Glyphs run upwards from y 50: CID1 50–55, CID2 55–60, CID3 60–62.5, CID4 62.5–65.
    mark(&mut doc, 0, &[[85.0, 55.5, 105.0, 61.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.glyphs, 2, "CIDs 2 and 3");
    let c = content(&doc, 0);
    assert!(c.contains("[<0001> -750 <0004>] TJ") || c.contains("[(\\000\\001) -750 (\\000\\004)] TJ"), "{c}");
}

#[test]
fn images_are_removed_or_have_their_pixels_cleared() {
    // Image 1 is fully covered; image 2 (4×1 gray, all white) is half covered.
    let img = stream("/Type /XObject /Subtype /Image /Width 4 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", &[255, 255, 255, 255]);
    let src = b"q 10 0 0 10 10 10 cm /Im1 Do Q q 40 0 0 10 100 100 cm /Im2 Do Q";
    let mut doc = one_page(src, "/XObject << /Im1 7 0 R /Im2 7 0 R >>", vec![img]);
    mark(&mut doc, 0, &[[5.0, 5.0, 25.0, 25.0], [95.0, 95.0, 120.0, 115.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.images_removed, r.images_cleared), (1, 1), "{r:?}");
    let c = content(&doc, 0);
    assert!(!c.contains("/Im1 Do") && !c.contains("/Im2 Do"), "{c}");
    let p = pdfcraft_model::pages(&doc).swap_remove(0);
    let res = doc.resolve(p.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
    let new = xo.iter().find(|(k, _)| k.starts_with(b"PCRedacted")).map(|(_, v)| v.clone()).expect("a cleared copy");
    let Object::Stream(s) = &*doc.resolve(&new) else { panic!() };
    assert_eq!(s.decoded().unwrap(), [0, 0, 255, 255], "pixels 1–2 (x 100–120) cleared");
    let Object::Stream(orig) = &*doc.get(ObjRef::new(7, 0)) else { panic!() };
    assert_eq!(orig.decoded().unwrap(), [255; 4], "the shared original is untouched");
}

/// The page's /Resources /XObject dictionary (empty when there is none).
fn xobjects_of(doc: &Document, page: usize) -> Dict {
    let p = &pdfcraft_model::pages(doc)[page];
    let res = doc.resolve(p.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    res.get(b"XObject").and_then(|x| doc.resolve(x).as_dict().cloned()).unwrap_or_default()
}

/// An 8×1 Flate gray scan of `scan` samples: the object bytes to put in a page's /XObject, and
/// the compressed data the saved file must not keep once the object is retired.
fn flate_scan(scan: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let s = Stream::flate(Dict::new(), scan);
    let mut v = format!(
        "<< /Type /XObject /Subtype /Image /Width 8 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
        s.raw.len()
    )
    .into_bytes();
    v.extend_from_slice(&s.raw);
    v.extend_from_slice(b"\nendstream");
    (v, s.raw.as_ref().to_vec())
}

// #389: a partly redacted scanned page stayed extractable: the redaction dropped the `Do` or
// drew a cleared copy, but the original scan image stayed in the page's resources and in the
// file, so the "removed" pixels were one object extraction away. These pin the retirement:
// once no `Do` draws the scan under its old name, the name leaves /Resources /XObject and a
// full save drops the image object itself.

#[test]
fn a_superseded_scan_image_is_gone_from_the_saved_file() {
    // #389, fully covered scan: the `Do` goes, /Im0 leaves the resources, and the saved file
    // keeps neither the name nor the scan's bytes.
    let scan = [0xCAu8, 0xFE, 0xBA, 0xBE, 0x12, 0x34, 0x56, 0x78];
    let img = stream("/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8", &scan);
    let mut doc = one_page(b"q 8 0 0 8 10 10 cm /Im0 Do Q", "/XObject << /Im0 7 0 R >>", vec![img]);
    mark(&mut doc, 0, &[[5.0, 5.0, 25.0, 25.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.images_removed, r.images_cleared), (1, 0), "{r:?}");
    assert!(!content(&doc, 0).contains("/Im0"), "{}", content(&doc, 0));
    let xo = xobjects_of(&doc, 0);
    assert!(!xo.contains(b"Im0"), "the retired name left the resources");
    let saved = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!saved.windows(4).any(|w| w == b"/Im0"), "the name is not in the saved file");
    assert!(!saved.windows(scan.len()).any(|w| w == scan.as_slice()), "the scan's bytes are not in the saved file");
}

#[test]
fn a_cleared_scan_copy_replaces_the_original_in_resources() {
    // #389, partly covered scan: the page draws a cleared copy under a fresh name, /Im0 leaves
    // the resources, and the saved file keeps the copy's bytes but not the original's.
    let scan = [0x5Au8, 0x6B, 0x7C, 0x8D, 0x9E, 0xAF, 0xC0, 0xD1];
    let (img, orig_raw) = flate_scan(&scan);
    let mut doc = one_page(b"q 8 0 0 8 10 10 cm /Im0 Do Q", "/XObject << /Im0 7 0 R >>", vec![img]);
    // The mark ends at x 15: its edge touches pixel 4's cell (x 14–15), and any overlap clears
    // the pixel, so pixels 0–4 go and 5–7 keep their samples.
    mark(&mut doc, 0, &[[5.0, 5.0, 15.0, 25.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.images_removed, r.images_cleared), (0, 1), "{r:?}");
    let c = content(&doc, 0);
    assert!(!c.contains("/Im0") && c.contains("/PCRedacted1 Do"), "{c}");
    let xo = xobjects_of(&doc, 0);
    assert!(!xo.contains(b"Im0"), "the retired name left the resources");
    let copy = xo.iter().find(|(k, _)| k.starts_with(b"PCRedacted")).map(|(_, v)| v.clone()).expect("the cleared copy");
    let Object::Stream(s) = &*doc.resolve(&copy) else { panic!() };
    assert_eq!(s.decoded().unwrap(), [0, 0, 0, 0, 0, scan[5], scan[6], scan[7]], "pixels 0–4 cleared");
    let saved = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(saved.windows(s.raw.len()).any(|w| w == s.raw.as_ref()), "the cleared copy is in the saved file");
    assert!(!saved.windows(scan.len()).any(|w| w == scan.as_slice()), "the original scan's samples are not");
    assert!(!saved.windows(orig_raw.len()).any(|w| w == orig_raw.as_slice()), "the original image object is not");
}

#[test]
fn a_second_draw_of_the_scan_does_not_keep_the_original_alive() {
    // #389, scan drawn twice with one draw covered: the covered draw is dropped, the surviving
    // draw is remapped to the cleared copy, and the original (its name and its bytes) is gone
    // from the saved file — the second draw must not keep the superseded image reachable.
    let scan = [0x5Au8, 0x6B, 0x7C, 0x8D, 0x9E, 0xAF, 0xC0, 0xD1];
    let (img, orig_raw) = flate_scan(&scan);
    let mut doc = one_page(b"q 8 0 0 8 10 10 cm /Im0 Do Q q 8 0 0 8 50 50 cm /Im0 Do Q", "/XObject << /Im0 7 0 R >>", vec![img]);
    // The mark covers the first draw whole; it only clips the second's left pixels.
    mark(&mut doc, 0, &[[5.0, 5.0, 55.0, 60.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.images_removed, r.images_cleared), (1, 1), "{r:?}");
    // Exactly one `Do` survives, and it draws the cleared copy.
    let c = content(&doc, 0);
    let dos: Vec<Vec<u8>> = pdfcraft_content::parse(c.as_bytes())
        .ops
        .iter()
        .filter(|op| op.op.as_slice() == b"Do")
        .filter_map(|op| op.operands.first().and_then(Object::as_name).map(<[u8]>::to_vec))
        .collect();
    assert_eq!(dos, [b"PCRedacted1".to_vec()], "{c}");
    let xo = xobjects_of(&doc, 0);
    assert!(!xo.contains(b"Im0"), "the retired name left the resources");
    let copy = xo.iter().find(|(k, _)| k.starts_with(b"PCRedacted")).map(|(_, v)| v.clone()).expect("the cleared copy");
    let Object::Stream(s) = &*doc.resolve(&copy) else { panic!() };
    assert_eq!(s.decoded().unwrap(), [0, 0, 0, 0, 0, scan[5], scan[6], scan[7]], "the surviving draw's covered pixels");
    let saved = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(saved.windows(s.raw.len()).any(|w| w == s.raw.as_ref()), "the cleared copy is in the saved file");
    assert!(!saved.windows(4).any(|w| w == b"/Im0"), "the name is not in the saved file");
    assert!(!saved.windows(scan.len()).any(|w| w == scan.as_slice()), "the original scan's samples are not");
    assert!(!saved.windows(orig_raw.len()).any(|w| w == orig_raw.as_slice()), "the original image object is not");
}

// The same class again, inherited: /Resources is inheritable (ISO 32000-2, 7.8.3, Table 30), so
// a scan may sit in a page-tree node while only the page below it draws it. The reads follow the
// tree (pdfcraft_model::pages resolves inherited entries into the page's dictionary); these pin
// that retirement reaches the node that OWNS the entry, so a full save drops the original with
// it, and that a page-own entry shadows an inherited one of the same name.

/// The page-tree root's /Resources /XObject dictionary.
fn root_xobjects(doc: &Document) -> Dict {
    let cat = doc.get(doc.root().unwrap()).as_dict().cloned().unwrap();
    let pages = cat.get(b"Pages").and_then(|p| doc.resolve(p).as_dict().cloned()).unwrap();
    let res = pages.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap();
    res.get(b"XObject").and_then(|x| doc.resolve(x).as_dict().cloned()).unwrap_or_default()
}

/// Two pages that carry no /Resources of their own: both inherit `/Resources` from the page-tree
/// root, whose /XObject holds `root_xo` (mapping names to the objects in `extra`, which follow
/// the pages' content streams, so the first extra object is 7).
fn inherited_pages(root_xo: &str, page1: &[u8], page2: &[u8], extra: Vec<Vec<u8>>) -> Document {
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 300] /Resources << /XObject << {root_xo} >> >> >>").into_bytes(),
        b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>".to_vec(),
        stream("", page1),
        stream("", page2),
    ];
    objs.extend(extra);
    pdf(objs)
}

#[test]
fn an_inherited_scan_image_is_gone_from_the_saved_file() {
    // The scan lives in the page-tree root's /XObject and only page 2 draws it. Covering it must
    // retire /Im0 from the root too — the node that owns the entry — or the full save keeps the
    // original image reachable through the tree.
    let scan = [0xCAu8, 0xFE, 0xBA, 0xBE, 0x12, 0x34, 0x56, 0x78];
    let img = stream("/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8", &scan);
    let mut doc = inherited_pages("/Im0 7 0 R", b"0 g", b"q 8 0 0 8 10 10 cm /Im0 Do Q", vec![img]);
    mark(&mut doc, 1, &[[5.0, 5.0, 25.0, 25.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.images_removed, r.images_cleared), (1, 0), "{r:?}");
    assert!(!content(&doc, 1).contains("/Im0"), "{}", content(&doc, 1));
    assert!(!root_xobjects(&doc).contains(b"Im0"), "the root's own /XObject lost the retired name");
    let saved = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!saved.windows(4).any(|w| w == b"/Im0"), "the name is not in the saved file");
    assert!(!saved.windows(scan.len()).any(|w| w == scan.as_slice()), "the scan's bytes are not in the saved file");
}

#[test]
fn an_inherited_flate_scan_is_cleared_and_the_original_is_gone() {
    // Partly covered inherited scan: the page draws a cleared copy under a fresh name, and the
    // original — reachable only through the root's /XObject — leaves the saved file with its name.
    let scan = [0x5Au8, 0x6B, 0x7C, 0x8D, 0x9E, 0xAF, 0xC0, 0xD1];
    let (img, orig_raw) = flate_scan(&scan);
    let mut doc = inherited_pages("/Im0 7 0 R", b"0 g", b"q 8 0 0 8 10 10 cm /Im0 Do Q", vec![img]);
    mark(&mut doc, 1, &[[5.0, 5.0, 15.0, 25.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.images_removed, r.images_cleared), (0, 1), "{r:?}");
    let c = content(&doc, 1);
    assert!(!c.contains("/Im0") && c.contains("/PCRedacted1 Do"), "{c}");
    let xo = xobjects_of(&doc, 1);
    assert!(!xo.contains(b"Im0") && xo.iter().any(|(k, _)| k.starts_with(b"PCRedacted")), "the copy replaced the name on the page");
    assert!(!root_xobjects(&doc).contains(b"Im0"), "the root's own /XObject lost the retired name");
    let copy = xo.iter().find(|(k, _)| k.starts_with(b"PCRedacted")).map(|(_, v)| v.clone()).expect("the cleared copy");
    let Object::Stream(s) = &*doc.resolve(&copy) else { panic!() };
    assert_eq!(s.decoded().unwrap(), [0, 0, 0, 0, 0, scan[5], scan[6], scan[7]], "pixels 0–4 cleared");
    let saved = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(saved.windows(s.raw.len()).any(|w| w == s.raw.as_ref()), "the cleared copy is in the saved file");
    assert!(!saved.windows(scan.len()).any(|w| w == scan.as_slice()), "the original scan's samples are not");
    assert!(!saved.windows(orig_raw.len()).any(|w| w == orig_raw.as_slice()), "the original image object is not");
}

#[test]
fn a_form_that_inherits_the_pages_resources_redacts_what_it_draws() {
    // A form XObject without /Resources of its own draws the inherited scan. Rewriting the form
    // must retire both names — the form's and the scan's — from the node that owns them, so the
    // form and the image leave the saved file together.
    let scan = [0xCAu8, 0xFE, 0xBA, 0xBE, 0x12, 0x34, 0x56, 0x78];
    let img = stream("/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8", &scan);
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"q 8 0 0 8 10 10 cm /Im0 Do Q");
    let mut doc = inherited_pages("/Fm 7 0 R /Im0 8 0 R", b"/Fm Do", b"0 g", vec![form, img]);
    mark(&mut doc, 0, &[[5.0, 5.0, 25.0, 25.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.images_removed, r.forms_rewritten), (1, 1), "{r:?}");
    let c = content(&doc, 0);
    assert!(!c.contains("/Fm") && c.contains("/PCRedacted1 Do"), "{c}");
    assert!(!root_xobjects(&doc).contains(b"Fm") && !root_xobjects(&doc).contains(b"Im0"), "the root lost both retired names");
    assert_eq!(under(&mut doc, 0, &[[5.0, 5.0, 25.0, 25.0]]), 0);
    let saved = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!saved.windows(4).any(|w| w == b"/Im0"), "the scan's name is not in the saved file");
    assert!(!saved.windows(scan.len()).any(|w| w == scan.as_slice()), "the scan's bytes are not in the saved file");
}

#[test]
fn a_page_own_resource_shadows_the_inherited_name() {
    // Page 1 maps /Im0 in its own /Resources (the tiny image); the root's /Im0 (the scan) is what
    // page 2 inherits and draws. Page-own wins per name: the tiny image is the one processed, and
    // the sibling's scan keeps both its root entry and its place in the file.
    let scan = [0xCAu8, 0xFE, 0xBA, 0xBE, 0x12, 0x34, 0x56, 0x78];
    let tiny = [0xDEu8, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03, 0x04];
    let img = |samples: &[u8]| stream("/Type /XObject /Subtype /Image /Width 8 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", samples);
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 300] /Resources << /XObject << /Im0 8 0 R >> >> >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /XObject << /Im0 7 0 R >> >> >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>".to_vec(),
        stream("", b"q 10 0 0 10 10 10 cm /Im0 Do Q"),
        stream("", b"q 8 0 0 8 10 10 cm /Im0 Do Q"),
        img(&tiny),
        img(&scan),
    ]);
    // The mark covers the drawn tiny image's left half: the pixel centres from 10.625 to 14.375
    // go, the ones from 15.625 on stay.
    mark(&mut doc, 0, &[[5.0, 5.0, 15.0, 25.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!(r.images_cleared, 1, "only the page-own image is processed: {r:?}");
    let xo = xobjects_of(&doc, 0);
    assert!(!xo.contains(b"Im0"));
    let copy = xo.iter().find(|(k, _)| k.starts_with(b"PCRedacted")).map(|(_, v)| v.clone()).expect("the cleared copy");
    let Object::Stream(s) = &*doc.resolve(&copy) else { panic!() };
    assert_eq!(s.decoded().unwrap(), [0, 0, 0, 0, tiny[4], tiny[5], tiny[6], tiny[7]], "the cleared copy comes from the page-own image");
    assert!(!content(&doc, 0).contains("/Im0") && content(&doc, 1).contains("/Im0"), "the sibling still draws the root's /Im0");
    assert!(root_xobjects(&doc).contains(b"Im0"), "the root's entry stays while a page still draws it");
    let saved = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(saved.windows(scan.len()).any(|w| w == scan.as_slice()), "the sibling's scan stays in the file");
    assert!(!saved.windows(tiny.len()).any(|w| w == tiny.as_slice()), "the retired page-own image does not");
}

#[test]
fn the_proof_does_not_pass_while_an_inherited_scan_still_draws() {
    // The proof side reads the tree the same way: while the inherited scan still paints under a
    // region (nothing was applied), the geometry check fails the region — it never passes silently.
    // 8×8 with full sample data, so the proof's own image check can inspect every pixel.
    let scan: Vec<u8> = (0..64u32).map(|i| (0x41u32 + i * 7 % 120) as u8).collect();
    let img = stream("/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8", &scan);
    let mut doc = inherited_pages("/Im0 7 0 R", b"0 g", b"q 8 0 0 8 10 10 cm /Im0 Do Q", vec![img]);
    mark(&mut doc, 1, &[[5.0, 5.0, 25.0, 25.0]], "");
    let snapshot = Snapshot::capture(&doc, None).unwrap();
    let proof = snapshot.prove(&doc, &ProofOptions::default());
    assert!(!proof.passed());
    assert!(proof.entries.iter().any(|e| e.status == crate::verify::Status::Failed && e.detail.contains("image")), "{}", proof.to_text());
}

// FF-1b (critical, found by the adversarial audit): a form XObject WITH its own /Resources that
// gets rewritten strips the retired names from its own frozen copy of /Resources, but the
// propagation of that retirement upward was gated on the form having no own resources. So the
// page's own /XObject — or a page-tree node — kept mapping the original image name to the
// original object: apply succeeded, the shipped proof passed, and the original image stayed in
// every full save. The leak reproduces with and without page-tree inheritance: the boundary is
// the root cause, not inheritance.

/// Three pages inheriting /Resources from the page-tree root; `c3_dict`/`c3` build page 3's
/// content stream (extra objects follow the three content streams, so the first extra is 9).
fn hostile_three_pages(root_xo: &str, c1: &[u8], c2: &[u8], c3_dict: &str, c3: &[u8], extra: Vec<Vec<u8>>) -> Document {
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 300] /Resources << /XObject << {root_xo} >> >> >>")
            .into_bytes(),
        b"<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 8 0 R >>".to_vec(),
        stream("", c1),
        stream("", c2),
        stream(c3_dict, c3),
    ];
    objs.extend(extra);
    pdf(objs)
}

#[test]
fn ff1b_an_own_resources_form_retires_the_scan_from_its_owners_above() {
    // The outer form's OWN /Resources map both /InnerFm and /Im0 (the scan), so the inner form's
    // draw resolves; the page-tree root also owns an /Im0 entry for the same object. Rewriting
    // the outer form strips /Im0 from its frozen copy — and must report the retirement up, so
    // the entry nothing draws any more leaves the root and the original leaves the saved file.
    let scan = [0xCAu8, 0xFE, 0xBA, 0xBE, 0x12, 0x34, 0x56, 0x78];
    let img = stream("/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8", &scan);
    let outer = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /XObject << /InnerFm 11 0 R /Im0 10 0 R >> >>",
        b"q 8 0 0 8 0 0 cm /InnerFm Do Q",
    );
    let inner = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"q 8 0 0 8 0 0 cm /Im0 Do Q");
    let mut doc = hostile_three_pages("/Fm 9 0 R /Im0 10 0 R", b"0 g", b"q 8 0 0 8 10 10 cm /Fm Do Q", "", b"0 g", vec![outer, img, inner]);
    mark(&mut doc, 1, &[[5.0, 5.0, 25.0, 25.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.images_removed, r.forms_rewritten), (1, 2), "{r:?}");
    let c = content(&doc, 1);
    assert!(!c.contains("/Im0") && c.contains("/PCRedacted"), "{c}");
    assert!(!xobjects_of(&doc, 1).contains(b"Im0"), "the page's copy lost the retired name");
    assert!(!root_xobjects(&doc).contains(b"Im0"), "the root's entry — which nothing draws any more — went with it");
    assert_eq!(under(&mut doc, 1, &[[5.0, 5.0, 25.0, 25.0]]), 0);
    let saved = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!saved.windows(4).any(|w| w == b"/Im0"), "the scan's name is not in the saved file");
    assert!(!saved.windows(scan.len()).any(|w| w == scan.as_slice()), "the scan's bytes are not in the saved file");
}

#[test]
fn ff1b_the_own_resources_boundary_leaks_without_inheritance_too() {
    // The same construction entirely in the page's OWN resources — no page-tree inheritance
    // anywhere. The rewritten outer form froze a clean copy of its own resources, but the
    // page's own /XObject kept mapping /Im0 to the original scan: the leak is the
    // own-resources boundary itself.
    let scan = [0xCAu8, 0xFE, 0xBA, 0xBE, 0x12, 0x34, 0x56, 0x78];
    let img = stream("/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8", &scan);
    let outer = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /XObject << /InnerFm 9 0 R /Im0 8 0 R >> >>",
        b"q 8 0 0 8 0 0 cm /InnerFm Do Q",
    );
    let inner = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"q 8 0 0 8 0 0 cm /Im0 Do Q");
    let mut doc = one_page(b"q 8 0 0 8 10 10 cm /Fm Do Q", "/XObject << /Fm 7 0 R /Im0 8 0 R >>", vec![outer, img, inner]);
    mark(&mut doc, 0, &[[5.0, 5.0, 25.0, 25.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.images_removed, r.forms_rewritten), (1, 2), "{r:?}");
    assert!(!xobjects_of(&doc, 0).contains(b"Im0"), "the page's own /XObject lost the retired name");
    assert_eq!(under(&mut doc, 0, &[[5.0, 5.0, 25.0, 25.0]]), 0);
    let saved = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!saved.windows(scan.len()).any(|w| w == scan.as_slice()), "the scan's bytes are not in the saved file");
}

#[test]
fn ff1b_a_scan_an_own_resources_form_never_drew_is_not_retired() {
    // The audit's literal construction: the outer form's own /Resources map only /InnerFm, so
    // the inner form's /Im0 resolves against nothing — the scan is never drawn and never
    // processed. Nothing may be retired then: no `Do` disappeared, and the root's unsuperseded
    // entry keeps the object in the file.
    let scan = [0xCAu8, 0xFE, 0xBA, 0xBE, 0x12, 0x34, 0x56, 0x78];
    let img = stream("/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8", &scan);
    let outer = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /XObject << /InnerFm 11 0 R >> >>",
        b"q 8 0 0 8 0 0 cm /InnerFm Do Q",
    );
    let inner = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"q 8 0 0 8 0 0 cm /Im0 Do Q");
    let mut doc = hostile_three_pages("/Fm 9 0 R /Im0 10 0 R", b"0 g", b"q 8 0 0 8 10 10 cm /Fm Do Q", "", b"0 g", vec![outer, img, inner]);
    mark(&mut doc, 1, &[[5.0, 5.0, 25.0, 25.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.images_removed, r.images_cleared, r.forms_rewritten), (0, 0, 0), "{r:?}");
    assert!(root_xobjects(&doc).contains(b"Im0"), "the root's entry was never superseded");
    let saved = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(saved.windows(scan.len()).any(|w| w == scan.as_slice()), "the scan was never redacted, so it stays");
}

#[test]
fn ff1c_an_unreadable_sibling_page_fails_apply_closed() {
    // Page 2 draws the inherited scan and is redacted; page 3's content stream is unreadable.
    // The proof cannot verify such a page (it could be drawing anything), so apply refuses
    // rather than ship a file it cannot vouch for — it must not succeed with a half-done job.
    let scan = [0xCAu8, 0xFE, 0xBA, 0xBE, 0x12, 0x34, 0x56, 0x78];
    let img = stream("/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8", &scan);
    let mut doc = hostile_three_pages(
        "/Im0 9 0 R",
        b"0 g",
        b"q 8 0 0 8 10 10 cm /Im0 Do Q",
        "/Filter /DCTDecode",
        b"\xff\xd8\xff\xe0 not-really-jpeg \x00\x01\xff",
        vec![img],
    );
    mark(&mut doc, 1, &[[5.0, 5.0, 25.0, 25.0]], "");
    assert!(apply(&mut doc, None).is_err(), "apply must refuse what the proof cannot verify");
}

#[test]
fn ff1c_an_unreadable_form_on_an_untouched_page_fails_apply_closed() {
    // The same fail-closed path through content apply may tolerate: page 3 is readable but
    // draws a corrupt (undecodable) form. A form that can't be read in full could be drawing
    // anything, so the run still refuses.
    let scan = [0xCAu8, 0xFE, 0xBA, 0xBE, 0x12, 0x34, 0x56, 0x78];
    let img = stream("/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8", &scan);
    // A form whose raw Flate data is garbage: /Length 8 raw bytes that do not inflate.
    let bad_form = {
        let mut v = b"<< /Type /XObject /Subtype /Form /BBox [0 0 300 300] /Filter /FlateDecode /Length 8 >>\nstream\n".to_vec();
        v.extend_from_slice(b"\x00\x01\x02\x03\x04\x05\x06\x07");
        v.extend_from_slice(b"\nendstream");
        v
    };
    let mut doc = hostile_three_pages("/Im0 9 0 R /Fm3 10 0 R", b"0 g", b"q 8 0 0 8 10 10 cm /Im0 Do Q", "/Fm3 Do", b"", vec![img, bad_form]);
    mark(&mut doc, 1, &[[5.0, 5.0, 25.0, 25.0]], "");
    assert!(apply(&mut doc, None).is_err(), "apply must refuse what the proof cannot verify");
}

#[test]
fn a_name_a_form_retires_but_the_page_still_draws_keeps_its_entry() {
    // The page draws /Im0 out in the open and also through a form whose OWN /Resources map the
    // same /Im0 to the same image; only the form's draw is covered. The rewrite retires /Im0
    // inside the form and the name now propagates up — but the page's own entry must stay:
    // the page still draws the name by another route, and stripping it would blank content
    // nobody redacted and drop the image from the file.
    let scan = [0xCAu8, 0xFE, 0xBA, 0xBE, 0x12, 0x34, 0x56, 0x78];
    let img = stream("/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8", &scan);
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /XObject << /Im0 7 0 R >> >>", b"q 8 0 0 8 0 0 cm /Im0 Do Q");
    let mut doc = one_page(b"q 8 0 0 8 100 100 cm /Im0 Do Q /Fm Do", "/XObject << /Im0 7 0 R /Fm 8 0 R >>", vec![img, form]);
    mark(&mut doc, 0, &[[0.0, 0.0, 20.0, 20.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.images_removed, r.forms_rewritten), (1, 1), "{r:?}");
    assert!(xobjects_of(&doc, 0).contains(b"Im0"), "the page still draws /Im0: the entry stays");
    assert!(content(&doc, 0).contains("/Im0 Do"), "the open draw is untouched");
    let saved = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(saved.windows(scan.len()).any(|w| w == scan.as_slice()), "the still-drawn image stays in the file");
}

#[test]
fn a_resource_less_form_retiring_a_drawn_name_keeps_the_entry_too() {
    // The same corner through a form WITHOUT own /Resources, which drew the page's /Im0. Its
    // retired name always propagated up — but the page's own entry may still only go when the
    // page itself no longer draws the name: the old unconditional strip blanked the open draw
    // and dropped the still-drawn image from the saved file.
    let scan = [0xCAu8, 0xFE, 0xBA, 0xBE, 0x12, 0x34, 0x56, 0x78];
    let img = stream("/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8", &scan);
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"q 8 0 0 8 0 0 cm /Im0 Do Q");
    let mut doc = one_page(b"q 8 0 0 8 100 100 cm /Im0 Do Q /Fm Do", "/XObject << /Im0 7 0 R /Fm 8 0 R >>", vec![img, form]);
    mark(&mut doc, 0, &[[0.0, 0.0, 20.0, 20.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.images_removed, r.forms_rewritten), (1, 1), "{r:?}");
    assert!(xobjects_of(&doc, 0).contains(b"Im0"), "the page still draws /Im0: the entry stays");
    assert!(content(&doc, 0).contains("/Im0 Do"), "the open draw is untouched");
    let saved = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(saved.windows(scan.len()).any(|w| w == scan.as_slice()), "the still-drawn image stays in the file");
}

#[test]
fn vectors_are_removed_or_clipped_and_inline_images_go() {
    let src = b"0 g 10 10 20 20 re f 0 0 300 300 re f q 10 0 0 10 50 50 cm BI /W 1 /H 1 /CS /G /BPC 8 ID \x80 EI Q 1 0 0 RG 150 150 m 160 160 l S";
    let mut doc = one_page(src, "", vec![]);
    mark(&mut doc, 0, &[[5.0, 5.0, 35.0, 35.0], [45.0, 45.0, 65.0, 65.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.paths_removed, r.paths_clipped, r.images_removed), (1, 1, 1), "{r:?}");
    let c = content(&doc, 0);
    assert!(!c.contains("10 10 20 20 re"), "{c}");
    assert!(c.contains("W*\nn\n0 0 300 300 re\nf\nQ"), "the page-size rect is clipped: {c}");
    assert!(!c.contains("BI"), "{c}");
    assert!(c.contains("150 150 m"), "the line elsewhere stays");
}

#[test]
fn shared_form_xobjects_are_copied_not_changed() {
    let form =
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >>", b"BT /F1 10 Tf 10 100 Td (SECRET) Tj ET");
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 /MediaBox [0 0 300 300] /Resources << /XObject << /Fm 7 0 R >> >> >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        stream("", b"/Fm Do"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        form,
    ];
    objs.push(b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec());
    let mut doc = pdf(objs);
    mark(&mut doc, 0, &[[0.0, 90.0, 300.0, 120.0]], "");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.glyphs, r.forms_rewritten), (6, 1), "{r:?}");
    assert!(content(&doc, 0).contains("/PCRedacted1 Do"));
    assert_eq!(content(&doc, 1), "/Fm Do", "page 2 shares the stream and the form: untouched");
    assert_eq!(under(&mut doc, 1, &[[0.0, 90.0, 300.0, 120.0]]), 6);
    assert_eq!(under(&mut doc, 0, &[[0.0, 90.0, 300.0, 120.0]]), 0);
}

#[test]
fn comments_links_and_fields_under_a_mark_go() {
    let mut doc = one_page(b"", "", vec![]);
    // A comment under the mark, a link elsewhere, and a text field under the mark.
    let square = Shape::Rectangle { rect: [20.0, 20.0, 40.0, 40.0] };
    add_annotation(
        &mut doc,
        &NewAnnotation { page: 0, style: Style::default_for(&square), shape: square, contents: "x".into(), author: "a".into() },
        &Meta::default(),
    )
    .unwrap();
    let link = Shape::Rectangle { rect: [200.0, 200.0, 220.0, 220.0] };
    add_annotation(
        &mut doc,
        &NewAnnotation { page: 0, style: Style::default_for(&link), shape: link, contents: "keep".into(), author: "a".into() },
        &Meta::default(),
    )
    .unwrap();
    pdfcraft_forms::add_field(&mut doc, 0, [10.0, 50.0, 100.0, 70.0], &pdfcraft_forms::NewField::Text { multiline: false }, Some("ssn")).unwrap();
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 80.0]], "REDACTED");
    let r = apply(&mut doc, None).unwrap();
    assert_eq!((r.annotations, r.fields), (1, 1), "{r:?}");
    assert!(pdfcraft_forms::fields(&doc).is_empty());
    let p = pdfcraft_model::pages(&doc).swap_remove(0);
    assert_eq!(annots_of(&doc, &p.dict).len(), 1, "only the far rectangle stays");
    let c = content(&doc, 0);
    assert!(c.contains("(REDACTED) Tj"), "overlay text: {c}");
}

#[test]
fn nothing_to_apply_and_clearing_marks() {
    let mut doc = one_page(b"BT /F1 10 Tf 10 100 Td (AB) Tj ET", "", vec![]);
    assert_eq!(apply(&mut doc, None), Err(RedactError::NothingToApply));
    mark(&mut doc, 0, &[[0.0, 0.0, 50.0, 50.0]], "");
    assert_eq!(marks(&doc).len(), 1);
    assert_eq!(clear_marks(&mut doc, None), Ok(1));
    assert!(marks(&doc).is_empty());
    assert!(content(&doc, 0).contains("(AB) Tj"));
}

#[test]
fn unreadable_content_fails_closed() {
    let mut doc = one_page(b"", "", vec![]);
    let page = pdfcraft_model::pages(&doc)[0].obj;
    let bad = doc.add(Object::Stream(Stream::from_raw(
        {
            let mut d = Dict::new();
            d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
            d
        },
        b"garbage".to_vec(),
    )));
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Ref(bad))).unwrap();
    mark(&mut doc, 0, &[[0.0, 0.0, 50.0, 50.0]], "");
    assert_eq!(apply(&mut doc, None), Err(RedactError::Unreadable(1)));
}

/// A document with something in every hidden-information category.
fn hidden_fixture() -> Document {
    let content = b"BT /F1 10 Tf 10 100 Td (Visible) Tj 3 Tr (Hidden) Tj 0 Tr ET BT /F1 10 Tf 500 500 Td (Offpage) Tj ET /OC /L1 BDC BT /F1 10 Tf 10 50 Td (Layer) Tj ET EMC";
    let objs: Vec<Vec<u8>> = vec![
        // 1 catalog
        b"<< /Type /Catalog /Pages 2 0 R /Metadata 7 0 R /Names << /EmbeddedFiles << /Names [(a.txt) 8 0 R] >> /JavaScript << /Names [(init) 9 0 R] >> >> /OpenAction 9 0 R /Outlines 10 0 R /PageMode /UseOutlines /PieceInfo << /App << /Private 1 >> >> /OCProperties << /OCGs [13 0 R 14 0 R] /D << /OFF [13 0 R] /Order [13 0 R 14 0 R] >> >> /AcroForm << /Fields [17 0 R] >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        // 3 page
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /Properties << /L1 13 0 R >> >> /Annots [15 0 R 16 0 R 17 0 R 18 0 R 19 0 R] /PieceInfo << /App << /Private 2 >> >> /AA << /O 9 0 R >> >>".to_vec(),
        stream("", content),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        stream("/Type /Metadata /Subtype /XML", b"<x:xmpmeta/>"),
        b"<< /Type /Filespec /F (a.txt) /EF << /F 20 0 R >> >>".to_vec(),
        b"<< /S /JavaScript /JS (app.alert(1)) >>".to_vec(),
        b"<< /Type /Outlines /First 11 0 R /Last 12 0 R /Count 2 >>".to_vec(),
        b"<< /Title (One) /Parent 10 0 R /Next 12 0 R >>".to_vec(),
        b"<< /Title (Two) /Parent 10 0 R /Prev 11 0 R >>".to_vec(),
        b"<< /Type /OCG /Name (Secret layer) >>".to_vec(),
        b"<< /Type /OCG /Name (Shown layer) >>".to_vec(),
        // 15 comment + 16 its pop-up, 17 widget, 18 link, 19 attachment
        b"<< /Type /Annot /Subtype /Square /Rect [10 10 50 50] /C [1 0 0] /Popup 16 0 R >>".to_vec(),
        b"<< /Type /Annot /Subtype /Popup /Rect [60 10 160 60] /Parent 15 0 R >>".to_vec(),
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Ada) /Rect [10 200 150 220] /A 9 0 R >>".to_vec(),
        b"<< /Type /Annot /Subtype /Link /Rect [10 230 50 240] /A << /S /URI /URI (https://example.org) >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /FileAttachment /Rect [200 10 210 20] /FS 8 0 R >>".to_vec(),
        stream("", b"attached"),
    ];
    let mut doc = pdf(objs);
    let info = doc.add(Object::Dict({
        let mut d = Dict::new();
        d.set(b"Title".to_vec(), pdfcraft_cos::PdfString::text("Secret plan"));
        d.set(b"Author".to_vec(), pdfcraft_cos::PdfString::text("Ada"));
        d
    }));
    doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(info));
    doc
}

#[test]
fn hidden_information_is_counted_and_removed() {
    use crate::sanitize::{Hidden, remove_hidden, sanitize, scan};
    let mut doc = hidden_fixture();
    let counts: std::collections::HashMap<Hidden, usize> = scan(&doc).into_iter().collect();
    assert_eq!(counts[&Hidden::Metadata], 3, "two Info entries and the XMP stream");
    assert_eq!(counts[&Hidden::Attachments], 2, "the embedded file and the attachment annotation");
    assert_eq!(counts[&Hidden::Comments], 1);
    assert_eq!(counts[&Hidden::FormFields], 1);
    assert_eq!(counts[&Hidden::HiddenText], 6 + 7, "Hidden (render mode 3) and Offpage (off the page)");
    assert_eq!(counts[&Hidden::HiddenLayers], 2, "one off layer and its block");
    assert_eq!(counts[&Hidden::Bookmarks], 2);
    assert_eq!(counts[&Hidden::LinksActionsScripts], 5, "link, open action, page actions, widget action, document script");
    assert_eq!(counts[&Hidden::PrivateData], 2);

    // Only hidden text first.
    let done = remove_hidden(&mut doc, &[Hidden::HiddenText]).unwrap();
    assert_eq!(done, [(Hidden::HiddenText, 13)]);
    let c = content(&doc, 0);
    assert!(c.contains("(Visible)") && !c.contains("Hidden") && !c.contains("Offpage") && c.contains("(Layer)"), "{c}");
    assert!(doc.full_save_required());

    // Then everything.
    let done = sanitize(&mut doc).unwrap();
    assert!(done.iter().all(|(h, _)| *h != Hidden::HiddenText));
    let c = content(&doc, 0);
    assert!(!c.contains("Layer") && c.contains("(Visible)"), "{c}");
    let doc = reopen(&doc);
    let after: Vec<(Hidden, usize)> = scan(&doc).into_iter().filter(|(_, n)| *n > 0).collect();
    // A full save writes a fresh /Info with the modification date only.
    assert!(after.iter().all(|(h, _)| *h == Hidden::Metadata), "{after:?}");
    let cat = doc.get(doc.root().unwrap()).as_dict().cloned().unwrap();
    for k in [&b"Outlines"[..], b"OpenAction", b"PieceInfo", b"AcroForm", b"Metadata"] {
        assert!(!cat.contains(k), "{}", String::from_utf8_lossy(k));
    }
    assert!(pdfcraft_forms::fields(&doc).is_empty());
    let p = pdfcraft_model::pages(&doc).swap_remove(0);
    assert!(annots_of(&doc, &p.dict).is_empty());
    let shown = doc.object_numbers().into_iter().any(|n| match &*doc.get(ObjRef::new(n, doc.generation(n))) {
        Object::Stream(s) => s.decoded().is_ok_and(|d| d.windows(5).any(|w| w == b"(Ada)")),
        _ => false,
    });
    assert!(shown, "the field's value stays visible as page content");
}

#[test]
fn overlay_text_takes_its_font_size_colour_alignment_and_repeats() {
    let mut doc = one_page(b"BT /F1 12 Tf 20 250 Td (Secret salary figures) Tj ET", "", Vec::new());
    let look = pdfcraft_annot::OverlayLook { font: pdfcraft_annot::OverlayFont::Courier, size: 8.0, color: [0.0, 0.0, 1.0], align: 0, repeat: true };
    let shape = Shape::Redact { quads: vec![rect_quad([10.0, 200.0, 290.0, 270.0])], overlay: "REDACTED".into(), look };
    let style = Style::default_for(&shape);
    add_annotation(&mut doc, &NewAnnotation { page: 0, shape, style, contents: String::new(), author: "T".into() }, &Meta::default()).unwrap();
    // Written as Acrobat writes it.
    let m = &marks(&doc)[0];
    assert_eq!(m.look, look, "the look round-trips through /DA, /Q and /Repeat");
    apply(&mut doc, None).unwrap();
    let doc = reopen(&doc);
    let c = content(&doc, 0);
    assert!(c.contains("0 0 1 rg /PCCour 8 Tf"), "{c}");
    // 70 pt high at 8 pt × 1.2 leading: several lines, each the word repeated.
    assert!(c.matches(" Tm (REDACTED REDACTED").count() >= 5, "{c}");
    assert!(c.contains("1 0 0 1 11 "), "left aligned at the area's edge: {c}");
    let p = &pdfcraft_model::pages(&doc)[0];
    let fonts = p
        .dict
        .get(b"Resources")
        .and_then(|r| doc.resolve(r).as_dict().cloned())
        .and_then(|r| r.get(b"Font").and_then(|f| doc.resolve(f).as_dict().cloned()))
        .unwrap();
    assert!(fonts.contains(b"PCCour") && !fonts.contains(b"PCTimes"));
}

#[test]
fn redaction_codes_join_in_set_order_and_reject_strangers() {
    use crate::codes::{CODE_SETS, CodeSet};
    let foia = CodeSet::from_id("foia").unwrap();
    assert_eq!(foia.overlay(&["(b)(6)", "(b)(1)(A)", "(b)(6)"]), Ok("(b)(1)(A), (b)(6)".into()));
    assert_eq!(foia.overlay(&[]), Ok(String::new()));
    assert_eq!(foia.overlay(&["(b)(6)", "(k)(1)"]), Err("(k)(1)"));
    let privacy = CodeSet::from_id("privacy-act").unwrap();
    assert_eq!(privacy.overlay(&["(k)(7)", "(d)(5)"]), Ok("(d)(5), (k)(7)".into()));
    assert_eq!(CodeSet::from_id("gdpr"), None);
    assert!(CODE_SETS.iter().all(|s| !s.codes.is_empty() && s.codes.iter().all(|c| !c.is_empty())));
}

#[test]
fn tags_lose_what_redaction_removed() {
    let content = b"/P <</MCID 0>> BDC BT /F1 10 Tf 10 200 Td (SECRET) Tj ET EMC /P <</MCID 1>> BDC BT /F1 10 Tf 10 100 Td (Public) Tj ET EMC";
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 7 0 R /MarkInfo << /Marked true >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", content),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Type /StructTreeRoot /K 8 0 R >>".to_vec(),
        b"<< /S /Document /P 7 0 R /K [9 0 R 10 0 R] >>".to_vec(),
        b"<< /S /P /P 8 0 R /Pg 3 0 R /ActualText (SECRET) /Alt (the secret) /K 0 >>".to_vec(),
        b"<< /S /P /P 8 0 R /Pg 3 0 R /ActualText (Public) /K 1 >>".to_vec(),
    ]);
    mark(&mut doc, 0, &[[5.0, 195.0, 60.0, 212.0]], "");
    let report = apply(&mut doc, None).unwrap();
    assert_eq!(report.tags, 1);
    let secret = doc.get(pdfcraft_cos::ObjRef::new(9, 0)).as_dict().cloned().unwrap();
    assert!(secret.get(b"ActualText").is_none() && secret.get(b"Alt").is_none());
    assert!(secret.get(b"K").is_none(), "its marked content is empty now");
    let public = doc.get(pdfcraft_cos::ObjRef::new(10, 0)).as_dict().cloned().unwrap();
    assert_eq!(public.get(b"K").and_then(Object::as_int), Some(1));
    assert!(public.get(b"ActualText").is_some(), "untouched content keeps its tags");
}

#[test]
fn debug_output_of_a_report_never_prints_layer_names() {
    let r = Report { layers: vec!["Secret Merger Layer".into(), "Payroll".into()], glyphs: 3, ..Report::default() };
    let shown = format!("{r:?} {:#?}", r);
    assert!(!shown.contains("Secret") && !shown.contains("Payroll"), "{shown}");
    assert!(shown.contains("2 layer(s)") && shown.contains("glyphs: 3"), "{shown}");
}

#[test]
fn many_content_stream_pieces_are_joined_in_linear_time() {
    // 20 000 pieces, each with a few operators, one of them showing text: the operator-to-piece
    // lookup must not scan every piece for every operator.
    let n = 20_000usize;
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        Vec::new(),
        FONT.replace("95 0 R", "5 0 R").into_bytes(),
        widths(),
    ];
    let refs: Vec<String> = (0..n).map(|i| format!("{} 0 R", 6 + i)).collect();
    objs[2] =
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents [{}] /Resources << /Font << /F1 4 0 R >> >> >>", refs.join(" "))
            .into_bytes();
    for i in 0..n {
        let body: &[u8] = if i == n / 2 { b"BT /F1 10 Tf 10 100 Td (ABCD) Tj ET q Q" } else { b"q 1 0 0 1 0 0 cm Q" };
        objs.push(stream("", body));
    }
    let mut doc = pdf(objs);
    let started = std::time::Instant::now();
    assert_eq!(under(&mut doc, 0, &[[0.0, 0.0, 300.0, 300.0]]), 4);
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "took {:?}", started.elapsed());
}

/// OCR text is invisible but must be removed under a mark even when sanitization is off.
#[test]
fn redaction_removes_hidden_ocr_text_and_preserves_unmarked_words() {
    let mut doc = one_page(b"/OCR BMC BT /F1 10 Tf 3 Tr 10 100 Td (AB1234CD) Tj ET EMC", "", vec![]);
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
    apply(&mut doc, None).unwrap();
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    let mut saved = Document::open(Arc::new(bytes)).unwrap();
    let text = content(&saved, 0);
    assert!(!text.contains("1234"), "redacted OCR text survived: {text}");
    assert!(text.contains("(AB)") && text.contains("(CD)"), "unmarked OCR text was lost: {text}");
    assert_eq!(under(&mut saved, 0, &[[20.0, 95.0, 40.0, 110.0]]), 0);
    assert_eq!(under(&mut saved, 0, &[[40.0, 95.0, 50.0, 110.0]]), 2);
}
