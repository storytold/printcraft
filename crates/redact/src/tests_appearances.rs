//! Annotation appearance streams taking part in redaction: what a surviving annotation's `/AP`
//! paints under a mark is redacted (placed the way ISO 32000-2 §12.5.5 places an appearance on
//! its `/Rect`), what can't be placed refuses the operation, and the proof's geometry covers
//! appearances so a surviving leak fails.

use pdfcraft_cos::{ObjRef, Object, SaveOptions, write_full};

use super::tests::{FONT, content, mark, pdf, stream, widths};
use super::*;
use crate::verify::tests_support;

/// One 300×300 page with the usual marked text (`AB1234CD` at 10 pt from x 10), and room for
/// the annotation at object 7, its appearance at 8, and whatever else after that.
fn base(annots: &str) -> Vec<Vec<u8>> {
    vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Annots [{annots}] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>")
            .into_bytes(),
        stream("", b"BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
    ]
}

/// A 4×4 grey image whose samples are unmistakable in the saved bytes.
fn image() -> Vec<u8> {
    stream("/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceGray /BitsPerComponent 8", &[0xAA; 16])
}

const MARK: [[f64; 4]; 1] = [[20.0, 95.0, 40.0, 110.0]];

/// The object the annotation's `/AP` `key` (or the named state under it) points at.
fn appearance_ref(doc: &Document, annot: ObjRef, key: &[u8], state: Option<&[u8]>) -> Option<ObjRef> {
    let d = doc.get(annot);
    let ap_raw = d.as_dict()?.get(b"AP")?.clone();
    let ap = doc.resolve(&ap_raw);
    let raw = ap.as_dict()?.get(key)?.clone();
    let raw = match state {
        Some(name) => {
            let states = doc.resolve(&raw);
            states.as_dict()?.get(name)?.clone()
        }
        None => raw,
    };
    raw.as_ref()
}

/// The decoded bytes of the annotation's `/AP` `key` (or the named state under it).
fn appearance(doc: &Document, annot: ObjRef, key: &[u8], state: Option<&[u8]>) -> Vec<u8> {
    let r = appearance_ref(doc, annot, key, state).unwrap();
    let Object::Stream(s) = &*doc.get(r) else {
        panic!("the appearance is not a stream: {r}");
    };
    s.decoded_strict_within(crate::limits::MAX_STREAM).unwrap()
}

/// An annotation away from the mark whose appearance draws the image outside its own `/BBox`,
/// so that its §12.5.5 placement lands exactly on the mark (while the coverage scan, which sees
/// only the `/Rect` and the mapped `/BBox`, keeps the annotation).
fn image_over_mark() -> Document {
    let mut o = base("7 0 R");
    o.push(b"<< /Type /Annot /Subtype /Square /Rect [200 200 230 230] /AP << /N 8 0 R >> >>".to_vec());
    o.push(stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 30 30] /Resources << /XObject << /Im0 9 0 R >> >>",
        b"q 20 0 0 15 -180 -105 cm /Im0 Do Q",
    ));
    o.push(image());
    pdf(o)
}

#[test]
fn appearance_image_over_a_mark_is_removed() {
    let mut doc = image_over_mark();
    mark(&mut doc, 0, &MARK, "");
    let (report, proof) = apply_with_proof(&mut doc, None, &ApplyOptions::default(), &ProofOptions::default()).unwrap();
    assert_eq!(report.images_removed, 1, "the appearance image goes");
    assert!(proof.passed(), "{}", proof.to_text());
    // The mark went; the square stayed, and its appearance no longer draws the image.
    let annots = annots_of(&doc, &pdfcraft_model::pages(&doc)[0].dict);
    assert_eq!(annots.len(), 1, "the square stays");
    assert!(!String::from_utf8_lossy(&appearance(&doc, ObjRef::new(7, 0), b"N", None)).contains("Im0"));
    // The original pixels left the saved file.
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!bytes.windows(4).any(|w| w == b"\xAA\xAA\xAA\xAA"), "the original image bytes left the save");
}

#[test]
fn annotations_sharing_an_appearance_all_move_off_it() {
    let mut o = base("7 0 R 10 0 R");
    o.push(b"<< /Type /Annot /Subtype /Square /Rect [200 200 230 230] /AP << /N 8 0 R >> >>".to_vec());
    o.push(stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 30 30] /Resources << /XObject << /Im0 9 0 R >> >>",
        b"q 20 0 0 15 -180 -105 cm /Im0 Do Q",
    ));
    o.push(image());
    o.push(b"<< /Type /Annot /Subtype /Square /Rect [200 100 230 130] /AP << /N 8 0 R >> >>".to_vec());
    let mut doc = pdf(o);
    mark(&mut doc, 0, &MARK, "");
    let (report, proof) = apply_with_proof(&mut doc, None, &ApplyOptions::default(), &ProofOptions::default()).unwrap();
    assert_eq!(report.images_removed, 1, "the shared stream is interpreted once");
    assert!(proof.passed(), "{}", proof.to_text());
    let a = appearance_ref(&doc, ObjRef::new(7, 0), b"N", None).unwrap();
    let b = appearance_ref(&doc, ObjRef::new(10, 0), b"N", None).unwrap();
    assert_ne!(a, ObjRef::new(8, 0), "neither annotation names the original any more");
    assert_eq!(a, b, "both name the same rewritten stream");
}

#[test]
fn appearance_text_under_a_mark_is_removed() {
    // "SECR" placed on the /Rect sits exactly on the mark; the page's own "1234" sits beside it.
    let mut o = base("7 0 R");
    o.push(b"<< /Type /Annot /Subtype /Square /Rect [200 200 230 230] /AP << /N 8 0 R >> >>".to_vec());
    o.push(stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 30 30] /Resources << /Font << /F1 5 0 R >> >>",
        b"BT /F1 10 Tf 1 0 0 1 -180 -105 Tm (SECR) Tj ET",
    ));
    let mut doc = pdf(o);
    mark(&mut doc, 0, &MARK, "");
    let (report, proof) = apply_with_proof(&mut doc, None, &ApplyOptions::default(), &ProofOptions::default()).unwrap();
    assert_eq!(report.glyphs, 8, "the page's 1234 and the appearance's SECR");
    assert!(proof.passed(), "{}", proof.to_text());
    let ap = appearance(&doc, ObjRef::new(7, 0), b"N", None);
    assert!(!ap.windows(4).any(|w| w == b"SECR"), "the glyphs left the appearance");
    assert!(String::from_utf8_lossy(&ap).contains("TJ"), "the line keeps its width");
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!bytes.windows(4).any(|w| w == b"SECR"), "the text left the saved file");
    assert!(content(&doc, 0).contains("(AB)"), "the page's own kept text stays put");
}

#[test]
fn an_appearance_that_cannot_be_placed_refuses_the_operation() {
    // No /BBox: where the content paints would be a guess, so nothing may be changed.
    let mut o = base("7 0 R");
    o.push(b"<< /Type /Annot /Subtype /Square /Rect [200 200 230 230] /AP << /N 8 0 R >> >>".to_vec());
    o.push(stream("/Type /XObject /Subtype /Form /Resources << >>", b"0 0 1 rg 10 10 5 5 re f"));
    let mut doc = pdf(o);
    mark(&mut doc, 0, &MARK, "");
    let err = apply(&mut doc, None).unwrap_err();
    assert_eq!(err, RedactError::Unsupported { page: 1, reason: Unsupported::AppearanceUnplaced });
    assert!(content(&doc, 0).contains("1234"), "nothing was changed");
}

#[test]
fn an_empty_appearance_needs_no_placement() {
    // No /BBox, but nothing the stream could paint either: the annotation stays as it is.
    let mut o = base("7 0 R");
    o.push(b"<< /Type /Annot /Subtype /Square /Rect [200 200 230 230] /AP << /N 8 0 R >> >>".to_vec());
    o.push(stream("/Type /XObject /Subtype /Form /Resources << >>", b""));
    let mut doc = pdf(o);
    mark(&mut doc, 0, &MARK, "");
    let report = apply(&mut doc, None).unwrap();
    assert_eq!(report.annotations, 0, "the annotation stays");
    assert_eq!(appearance_ref(&doc, ObjRef::new(7, 0), b"N", None), Some(ObjRef::new(8, 0)), "the stream is untouched");
    assert_eq!(appearance(&doc, ObjRef::new(7, 0), b"N", None), Vec::<u8>::new());
}

#[test]
fn an_appearance_away_from_the_marks_is_untouched() {
    // The image is inside the /BBox, which is placed away from the mark: it keeps rendering.
    let mut o = base("7 0 R");
    o.push(b"<< /Type /Annot /Subtype /Square /Rect [200 200 230 230] /AP << /N 8 0 R >> >>".to_vec());
    o.push(stream("/Type /XObject /Subtype /Form /BBox [0 0 30 30] /Resources << /XObject << /Im0 9 0 R >> >>", b"q 20 0 0 15 5 7 cm /Im0 Do Q"));
    o.push(image());
    let mut doc = pdf(o);
    let before = appearance(&doc, ObjRef::new(7, 0), b"N", None);
    mark(&mut doc, 0, &MARK, "");
    let (report, proof) = apply_with_proof(&mut doc, None, &ApplyOptions::default(), &ProofOptions::default()).unwrap();
    assert_eq!((report.glyphs, report.images_removed), (4, 0), "only the page's own 1234 goes");
    assert!(proof.passed(), "{}", proof.to_text());
    assert_eq!(appearance_ref(&doc, ObjRef::new(7, 0), b"N", None), Some(ObjRef::new(8, 0)), "no rewrite, no new object");
    assert_eq!(appearance(&doc, ObjRef::new(7, 0), b"N", None), before);
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(bytes.windows(4).any(|w| w == b"\xAA\xAA\xAA\xAA"), "the appearance image is still in the file");
}

#[test]
fn down_state_appearances_take_part_too() {
    let mut o = base("7 0 R");
    o.push(b"<< /Type /Annot /Subtype /Square /Rect [200 200 230 230] /AP << /N 10 0 R /D 8 0 R >> >>".to_vec());
    o.push(stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 30 30] /Resources << /XObject << /Im0 9 0 R >> >>",
        b"q 20 0 0 15 -180 -105 cm /Im0 Do Q",
    ));
    o.push(image());
    o.push(stream("/Type /XObject /Subtype /Form /BBox [0 0 30 30] /Resources << >>", b"0 0 1 rg 2 2 26 26 re f"));
    let mut doc = pdf(o);
    let normal = appearance(&doc, ObjRef::new(7, 0), b"N", None);
    mark(&mut doc, 0, &MARK, "");
    let (report, proof) = apply_with_proof(&mut doc, None, &ApplyOptions::default(), &ProofOptions::default()).unwrap();
    assert_eq!(report.images_removed, 1, "the down state's image goes");
    assert!(proof.passed(), "{}", proof.to_text());
    assert!(!appearance(&doc, ObjRef::new(7, 0), b"D", None).windows(2).any(|w| w == b"Do"));
    assert_eq!(appearance(&doc, ObjRef::new(7, 0), b"N", None), normal, "the normal state never painted under the mark");
}

#[test]
fn an_annotation_under_a_mark_still_goes_whole() {
    // The /Rect overlaps the mark: the coverage scan removes the annotation, appearance and all,
    // and the appearance pass has nothing left to interpret.
    let mut o = base("7 0 R");
    o.push(b"<< /Type /Annot /Subtype /Square /Rect [22 97 38 108] /AP << /N 8 0 R >> >>".to_vec());
    o.push(stream("/Type /XObject /Subtype /Form /BBox [0 0 30 30] /Resources << >>", b"0 0 1 rg 5 5 20 20 re f"));
    let mut doc = pdf(o);
    mark(&mut doc, 0, &MARK, "");
    let report = apply(&mut doc, None).unwrap();
    assert_eq!(report.annotations, 1);
    assert!(annots_of(&doc, &pdfcraft_model::pages(&doc)[0].dict).is_empty());
}

#[test]
fn the_proofs_geometry_inspects_appearances() {
    // An appearance image over the region is found on the unredacted document: a surviving leak
    // would fail the proof. The same image away from the region is found nowhere.
    let over = image_over_mark();
    let found = tests_support::geometry_findings(&over, 0, MARK[0]);
    assert!(found.iter().any(|f| f.contains("Image")), "{found:?}");
    let mut o = base("7 0 R");
    o.push(b"<< /Type /Annot /Subtype /Square /Rect [200 200 230 230] /AP << /N 8 0 R >> >>".to_vec());
    o.push(stream("/Type /XObject /Subtype /Form /BBox [0 0 30 30] /Resources << /XObject << /Im0 9 0 R >> >>", b"q 20 0 0 15 5 7 cm /Im0 Do Q"));
    o.push(image());
    let away = pdf(o);
    assert!(tests_support::geometry_findings(&away, 0, MARK[0]).is_empty(), "nothing paints under the region");
}

/// The unused-import lint has no taste for fixtures kept for readability.
#[allow(unused_imports)]
use std::collections::HashSet as _HashSet;

#[test]
fn appearance_proofs_fail_closed_on_unreadable_output() {
    // A document whose appearance stream is damaged: the operation refuses (the proof would not
    // be able to vouch for what it paints).
    let mut o = base("7 0 R");
    o.push(b"<< /Type /Annot /Subtype /Square /Rect [200 200 230 230] /AP << /N 8 0 R >> >>".to_vec());
    o.push(stream("/Type /XObject /Subtype /Form /BBox [0 0 30 30] /Resources << >> /Filter /FlateDecode", &[0xFF; 12]));
    let mut doc = pdf(o);
    mark(&mut doc, 0, &MARK, "");
    let err = apply(&mut doc, None).unwrap_err();
    assert_eq!(err, RedactError::Unsupported { page: 1, reason: Unsupported::UnparsedContent });
}
