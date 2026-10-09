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
