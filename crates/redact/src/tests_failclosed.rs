//! Fail-closed behaviour of `apply`: damaged or unreadable content, signed documents, atomicity,
//! malformed marks, bounded overlays and scans, thumbnails, resource-property text.

use pdfcraft_annot::{Meta, NewAnnotation, Shape, Style, add_annotation, rect_quad};
use pdfcraft_cos::{Dict, Document, Object, PdfString, Stream};

use super::tests::*;
use super::*;

/// `apply` on a copy that is kept in `doc` even when it fails, to look at what the attempt did
/// (a failed [`apply`] leaves its document as it was).
pub(crate) fn apply_keeping(doc: &mut Document) -> Result<Report, RedactError> {
    let opts = ApplyOptions { sanitize: Sanitize::None, ..ApplyOptions::default() };
    let done = attempt(doc, None, &opts, &ProofOptions::default());
    *doc = done.document;
    done.result.map(|(report, _)| report)
}

const SECRET_TEXT: &[u8] = b"BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET";
/// Where "1234" of `SECRET_TEXT` is (see `tests.rs`).
const SECRET_AREA: [[f64; 4]; 1] = [[20.0, 95.0, 40.0, 110.0]];

/// A page whose object 4 (the content stream) is `contents` as written (so it may be damaged),
/// with font /F1 and the extra page entries `page_extra`.
fn page_with(contents: Vec<u8>, page_extra: &str, extra: Vec<Vec<u8>>) -> Document {
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> {page_extra} >>")
            .into_bytes(),
        contents,
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
    ];
    objs.extend(extra);
    pdf(objs)
}

/// The document as a full save writes it, objects in the open (a signed one too).
pub(crate) fn saved(doc: &Document) -> Vec<u8> {
    let opts = pdfcraft_cos::SaveOptions { object_streams: false, allow_signed_rewrite: true, ..Default::default() };
    pdfcraft_cos::write_full(doc, &opts).unwrap()
}

fn same_file(a: &Document, b: &Document) -> bool {
    saved(a) == saved(b)
}

pub(crate) fn opts_none() -> ApplyOptions {
    ApplyOptions { sanitize: Sanitize::None, ..ApplyOptions::default() }
}

// ---- 2: strict decoding ----

fn flate_stream(dict: &str, data: &[u8], keep: usize) -> Vec<u8> {
    // Cut the compressed data short: a tolerant decoder returns the part before the damage.
    let s = Stream::flate(Dict::new(), data);
    let mut raw = s.raw.as_ref().to_vec();
    raw.truncate(keep.min(raw.len()));
    let mut v = format!("<< /Filter /FlateDecode {dict} /Length {} >>\nstream\n", raw.len()).into_bytes();
    v.extend_from_slice(&raw);
    v.extend_from_slice(b"\nendstream");
    v
}

/// Content that doesn't compress well, so cutting the data short loses a real tail.
fn long_content() -> Vec<u8> {
    let mut c = Vec::new();
    for i in 0..400u32 {
        c.extend(format!("BT /F1 10 Tf 10 {} Td ({:08x}) Tj ET\n", 5 + i % 280, i.wrapping_mul(2_654_435_761)).bytes());
    }
    c.extend_from_slice(SECRET_TEXT);
    c
}

#[test]
fn a_truncated_content_stream_is_unreadable_not_a_shorter_page() {
    let c = long_content();
    let whole = Stream::flate(Dict::new(), &c).raw.len();
    let mut doc = page_with(flate_stream("", &c, whole * 2 / 3), "", vec![]);
    mark(&mut doc, 0, &SECRET_AREA, "");
    let before = doc.clone();
    assert_eq!(apply(&mut doc, None), Err(RedactError::Unreadable(1)));
    assert!(same_file(&doc, &before), "a failed apply leaves the document alone");
}

#[test]
fn a_damaged_piece_of_split_content_is_unreadable_not_hidden_by_the_join() {
    // The page's content is two streams and the TJ operator of the string starts the second
    // (#153): the pieces are read joined, but joining never turns a damaged piece into a
    // shorter page.
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents [4 0 R 7 0 R] /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", b"BT /F1 10 Tf 10 100 Td [(AB1234CD)]"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
    ];
    let tail = b"TJ ET".to_vec();
    objs.push(flate_stream("", &tail, Stream::flate(Dict::new(), &tail).raw.len() * 2 / 3));
    let mut doc = pdf(objs);
    mark(&mut doc, 0, &SECRET_AREA, "");
    let before = doc.clone();
    assert_eq!(apply(&mut doc, None), Err(RedactError::Unreadable(1)));
    assert!(same_file(&doc, &before), "a failed apply leaves the document alone");
}

#[test]
fn contents_that_fail_to_load_are_unreadable_not_empty() {
    // Object 4 is not an object that can be parsed; a reference to nothing at all is null (empty).
    let mut doc = page_with(b"<< /Length ( >>".to_vec(), "", vec![]);
    mark(&mut doc, 0, &SECRET_AREA, "");
    assert_eq!(apply(&mut doc, None), Err(RedactError::Unreadable(1)));
}

#[test]
fn content_the_parser_skips_is_refused_not_left_in_the_file() {
    // The second `(` nests inside the first unterminated string, so the whole tail — the secret
    // operator included — reads as one string: nothing is parsed, nothing would be removed, and
    // the untouched bytes would keep the secret in the saved file. The operation must refuse.
    let src = b"BT /F1 10 Tf 1 0 0 1 100 100 Tm (x\n(SECRET01) Tj ET";
    let mut doc = page_with(stream("", src), "", vec![]);
    mark(&mut doc, 0, &[[100.0, 95.0, 120.0, 110.0]], "");
    let before = doc.clone();
    assert_eq!(apply(&mut doc, None), Err(RedactError::Unsupported { page: 1, reason: Unsupported::UnparsedContent }));
    assert!(same_file(&doc, &before), "a failed apply leaves the document alone");
}

#[test]
fn a_damaged_form_is_removed_whole() {
    let c = long_content();
    let whole = Stream::flate(Dict::new(), &c).raw.len();
    let form = flate_stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >>", &c, whole * 2 / 3);
    let mut doc = page_with(stream("", b"/Fm0 Do"), "", vec![form]);
    // The page's resources name the form.
    let page = pdfcraft_model::pages(&doc).swap_remove(0);
    doc.update_dict(page.obj, |d| {
        let mut res = d.get(b"Resources").and_then(Object::as_dict).cloned().unwrap_or_default();
        let mut xo = Dict::new();
        xo.set(b"Fm0".to_vec(), Object::Ref(pdfcraft_cos::ObjRef::new(7, 0)));
        res.set(b"XObject".to_vec(), Object::Dict(xo));
        d.set(b"Resources".to_vec(), Object::Dict(res));
    })
    .unwrap();
    mark(&mut doc, 0, &[[0.0, 0.0, 300.0, 300.0]], "");
    // The pass drops the form, but the proof can't read what the source form held, so it can't
    // vouch for the page: the operation fails and the document is left as it was.
    let before = saved(&doc);
    assert!(matches!(apply(&mut doc, None), Err(RedactError::ProofIncomplete { .. })));
    assert_eq!(saved(&doc), before);
    let kept = attempt(&doc, None, &ApplyOptions::default(), &ProofOptions::default());
    assert!(kept.failed_proof.is_some_and(|p| p.unswept() > 0));
}

// ---- 5: signed documents ----

fn signed(flags: &str, extra: Vec<Vec<u8>>) -> Document {
    let mut doc = page_with(stream("", SECRET_TEXT), "", extra);
    let root = doc.trailer().reference(b"Root").unwrap();
    doc.update_dict(root, |d| {
        let mut form = Dict::new();
        form.set(b"SigFlags".to_vec(), if flags == "ref" { Object::Ref(pdfcraft_cos::ObjRef::new(7, 0)) } else { Object::Int(3) });
        form.set(b"Fields".to_vec(), Object::Array(vec![]));
        d.set(b"AcroForm".to_vec(), Object::Dict(form));
    })
    .unwrap();
    mark(&mut doc, 0, &SECRET_AREA, "");
    doc
}

#[test]
fn a_signed_document_is_refused_up_front_and_left_alone() {
    for doc in [signed("direct", vec![]), signed("ref", vec![b"3".to_vec()])] {
        let mut doc = doc;
        let before = doc.clone();
        assert_eq!(apply(&mut doc, None), Err(RedactError::Signed));
        assert_eq!(apply_with(&mut doc, None, &ApplyOptions::default()), Err(RedactError::Signed));
        assert_eq!(apply_with_proof(&mut doc, None, &opts_none(), &ProofOptions::default()).map(|_| ()), Err(RedactError::Signed));
        assert!(same_file(&doc, &before));
        assert!(!doc.full_save_required());
        assert_eq!(marks(&doc).len(), 1, "the mark is still there");
    }
}

#[test]
fn a_signed_document_is_redacted_when_the_caller_allows_it() {
    let mut doc = signed("direct", vec![]);
    let opts = ApplyOptions { allow_signed: true, ..opts_none() };
    // The proof serializes the signed document too (the rewrite is the caller's decision).
    let (report, proof) = apply_with_proof(&mut doc, None, &opts, &ProofOptions::default()).unwrap();
    assert!(report.glyphs > 0 && proof.passed(), "{}", proof.to_text());
    assert!(!content(&doc, 0).contains("1234"));
    assert!(doc.full_save_required());
}

// ---- 7: atomicity ----

#[test]
fn a_failure_after_the_work_started_leaves_the_document_unchanged() {
    // An object with a codec nobody can decode can't be swept, so the proof fails after the
    // content was already rewritten on the copy.
    let junk = stream("/Filter /NoSuchDecode", b"\x00\x01\x02 junk");
    let mut doc = page_with(stream("", SECRET_TEXT), "/Extra 7 0 R", vec![junk]);
    mark(&mut doc, 0, &SECRET_AREA, "");
    let before = doc.clone();
    let err = apply(&mut doc, None).unwrap_err();
    assert!(matches!(err, RedactError::ProofIncomplete { .. }), "{err:?}");
    assert!(same_file(&doc, &before));
    assert!(content(&doc, 0).contains("1234") && marks(&doc).len() == 1 && !doc.full_save_required());
    // The attempt's copy is there for diagnosis, with the proof that failed.
    let a = attempt(&doc, None, &opts_none(), &ProofOptions::default());
    assert!(a.result.is_err() && a.failed_proof.is_some_and(|p| !p.passed()));
    assert!(!content(&a.document, 0).contains("1234"));
    assert!(same_file(&doc, &before), "attempt never changes its input");
}

// ---- 9: malformed QuadPoints ----

fn quad_variants(doc: &mut Document, quads: Object) -> Vec<[f64; 4]> {
    let m = marks(doc).swap_remove(0);
    doc.update_dict(m.obj, |d| {
        d.set(b"QuadPoints".to_vec(), quads.clone());
    })
    .unwrap();
    marks(doc).swap_remove(0).rects
}

fn reals(v: &[f64]) -> Vec<Object> {
    v.iter().map(|x| Object::Real(*x)).collect()
}

#[test]
fn malformed_quad_points_never_shrink_or_shift_the_mark() {
    let mut doc = page_with(stream("", SECRET_TEXT), "", vec![]);
    mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
    let rect = marks(&doc).swap_remove(0).rects[0];
    let quad = [20.0, 110.0, 30.0, 110.0, 20.0, 95.0, 30.0, 95.0];
    // Well formed: just the quadrilateral.
    assert_eq!(quad_variants(&mut doc, Object::Array(reals(&quad))), [[20.0, 95.0, 30.0, 110.0]]);
    // Cut short (ten numbers): the whole quadrilateral and the annotation's rectangle.
    let mut ten = quad.to_vec();
    ten.extend([1.0, 2.0]);
    assert_eq!(quad_variants(&mut doc, Object::Array(reals(&ten))), [[20.0, 95.0, 30.0, 110.0], rect]);
    // A non-number among the entries shifts every later one: only the rectangle is trusted.
    let mut odd = reals(&quad);
    odd.insert(1, Object::name("x"));
    assert_eq!(quad_variants(&mut doc, Object::Array(odd)), [rect]);
    // Not an array at all.
    assert_eq!(quad_variants(&mut doc, Object::name("oops")), [rect]);
}

#[test]
fn a_mark_with_no_readable_area_fails_instead_of_vanishing() {
    let mut doc = page_with(stream("", SECRET_TEXT), "", vec![]);
    mark(&mut doc, 0, &SECRET_AREA, "");
    let m = marks(&doc).swap_remove(0);
    doc.update_dict(m.obj, |d| {
        d.set(b"QuadPoints".to_vec(), Object::name("oops"));
        d.remove(b"Rect");
    })
    .unwrap();
    assert!(marks(&doc).is_empty());
    assert_eq!(apply(&mut doc, None), Err(RedactError::UnreadableMark(1)));
}

// ---- 10: overlay bounds ----

fn repeat_mark(rect: [f64; 4], overlay: &str) -> Mark {
    let look = pdfcraft_annot::OverlayLook { repeat: true, size: 6.0, ..Default::default() };
    Mark { page: 0, obj: pdfcraft_cos::ObjRef::new(1, 0), rects: vec![rect], fill: Some([0.0; 3]), overlay: overlay.into(), look }
}

#[test]
fn a_huge_repeat_overlay_is_bounded_by_the_page_and_a_cell_cap() {
    let page = [0.0, 0.0, 300.0, 300.0];
    let m = repeat_mark([-1e9, -1e9, 1e9, 1e9], "REDACTED");
    let c = overlay_content(&[&m], page);
    assert!(c.len() < 256 << 10, "{} bytes", c.len());
    let text = String::from_utf8_lossy(&c);
    assert!(text.contains("0 0 300 300 re f"), "the box is cut to the page: {text}");
    assert!(!text.contains("NaN") && !text.contains("inf"));
    // A very long label is cut too.
    let long = "x".repeat(1 << 20);
    let c = overlay_content(&[&repeat_mark([0.0, 0.0, 300.0, 300.0], &long)], page);
    assert!(c.len() < 2 << 20, "{} bytes", c.len());
    // A mark wholly outside the page draws nothing.
    assert!(!String::from_utf8_lossy(&overlay_content(&[&repeat_mark([400.0, 0.0, 500.0, 50.0], "X")], page)).contains("re f"));
}

#[test]
fn da_numbers_that_are_not_finite_are_ignored() {
    let doc = page_with(stream("", SECRET_TEXT), "", vec![]);
    let mut d = Dict::new();
    d.set(b"DA".to_vec(), Object::String(PdfString::literal(b"nan inf 1 rg /Helv inf Tf".to_vec())));
    let look = overlay_look(&doc, &d);
    assert_eq!(look.color, pdfcraft_annot::OverlayLook::default().color);
    assert!(look.size.is_finite() && look.size == 0.0, "{}", look.size);
    d.set(b"DA".to_vec(), Object::String(PdfString::literal(b"0.5 g /Helv 1e30 Tf".to_vec())));
    let look = overlay_look(&doc, &d);
    assert_eq!((look.color, look.size), ([0.5; 3], MAX_OVERLAY_SIZE));
}

// ---- 11: resource-property text ----

#[test]
fn alternate_text_in_a_named_property_list_goes_with_the_content() {
    let content = b"/Span /MC0 BDC BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET EMC";
    let mut doc = page_with(stream("", content), "", vec![b"<< /ActualText (AB1234CD) /MCID 0 >>".to_vec()]);
    let page = pdfcraft_model::pages(&doc).swap_remove(0);
    doc.update_dict(page.obj, |d| {
        let mut res = d.get(b"Resources").and_then(Object::as_dict).cloned().unwrap_or_default();
        let mut props = Dict::new();
        props.set(b"MC0".to_vec(), Object::Ref(pdfcraft_cos::ObjRef::new(7, 0)));
        res.set(b"Properties".to_vec(), Object::Dict(props));
        d.set(b"Resources".to_vec(), Object::Dict(res));
    })
    .unwrap();
    mark(&mut doc, 0, &SECRET_AREA, "");
    apply(&mut doc, None).unwrap();
    let bytes = saved(&doc);
    assert!(!bytes.windows(4).any(|w| w == b"1234"), "the alternate text stayed in the file");
    // The marked-content id is kept for the structure tree.
    assert!(bytes.windows(5).any(|w| w == b"/MCID"));
}

// ---- 6, 12, 8: bounded and fail-closed scans ----

/// A page whose resources hold `forms` trivial forms (objects 7..) plus `extra_res`.
fn many_forms(forms: usize, text_in_last: bool) -> Document {
    let mut extra = Vec::new();
    let mut names = String::new();
    for i in 0..forms {
        let body: &[u8] = if text_in_last && i + 1 == forms { b"BT /F1 10 Tf 0 0 Td (HIDDEN) Tj ET" } else { b"0 0 1 1 re f" };
        extra.push(stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /Font << /F1 5 0 R >> >>", body));
        names.push_str(&format!("/F{i} {} 0 R ", 7 + i));
    }
    page_with(stream("", SECRET_TEXT), &format!("/Extra << {names} >>"), extra)
}

#[test]
fn a_pattern_with_more_forms_than_the_cap_fails_instead_of_being_skipped() {
    // A tiling pattern whose resources name more forms than a scan follows, the last one holding
    // text: skipping the rest quietly would hide it.
    let forms = limits::MAX_FORMS + 4;
    let mut extra = Vec::new();
    let mut names = String::new();
    for i in 0..forms {
        let body: &[u8] = if i + 1 == forms { b"BT /F1 10 Tf 0 0 Td (HIDDEN) Tj ET" } else { b"0 0 1 1 re f" };
        extra.push(stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /Font << /F1 5 0 R >> >>", body));
        names.push_str(&format!("/X{i} {} 0 R ", 8 + i));
    }
    let pattern = stream(
        &format!("/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << /XObject << {names} >> >>"),
        b"0 0 5 5 re f",
    );
    let mut all = vec![pattern];
    all.extend(extra);
    let mut doc = page_with(stream("", SECRET_TEXT), "", all);
    let page = pdfcraft_model::pages(&doc).swap_remove(0);
    doc.update_dict(page.obj, |d| {
        let mut res = d.get(b"Resources").and_then(Object::as_dict).cloned().unwrap_or_default();
        let mut pats = Dict::new();
        pats.set(b"P0".to_vec(), Object::Ref(pdfcraft_cos::ObjRef::new(7, 0)));
        res.set(b"Pattern".to_vec(), Object::Dict(pats));
        d.set(b"Resources".to_vec(), Object::Dict(res));
    })
    .unwrap();
    mark(&mut doc, 0, &SECRET_AREA, "");
    let err = apply(&mut doc, None).unwrap_err();
    assert_eq!(err, RedactError::Unsupported { page: 1, reason: Unsupported::PatternUnreadable });
}

#[test]
fn more_form_resources_than_the_cap_fail_instead_of_going_unscanned() {
    let mut doc = many_forms(limits::MAX_FORMS + 4, true);
    let page = pdfcraft_model::pages(&doc).swap_remove(0);
    doc.update_dict(page.obj, |d| {
        let mut res = d.get(b"Resources").and_then(Object::as_dict).cloned().unwrap_or_default();
        let xo = d.get(b"Extra").and_then(Object::as_dict).cloned().unwrap_or_default();
        res.set(b"XObject".to_vec(), Object::Dict(xo));
        d.set(b"Resources".to_vec(), Object::Dict(res));
    })
    .unwrap();
    mark(&mut doc, 0, &SECRET_AREA, "");
    let err = apply(&mut doc, None).unwrap_err();
    assert_eq!(err, RedactError::Unsupported { page: 1, reason: Unsupported::PatternUnreadable });
}

#[test]
fn the_decode_budget_runs_out_loudly() {
    let s = Stream::flate(Dict::new(), &[7u8; 100]);
    let mut budget = Budget::new(150);
    assert_eq!(budget.decode(&s).map(|d| d.len()), Ok(100));
    assert_eq!(budget.decode(&s), Err(limits::Refused::OverBudget));
    // Damage is told apart from a lack of room.
    let mut cut = s.clone();
    cut.raw = std::sync::Arc::new(cut.raw.get(..4).unwrap_or_default().to_vec()).into();
    assert_eq!(Budget::new(1 << 20).decode(&cut), Err(limits::Refused::Unreadable));
}

// ---- 12: images painted without a Do ----

fn page_with_pattern(pattern_body: &[u8], pattern_res: &str) -> Document {
    let image = stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"\x00");
    let pattern = stream(
        &format!("/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << {pattern_res} >>"),
        pattern_body,
    );
    let mut doc = page_with(stream("", SECRET_TEXT), "", vec![pattern, image]);
    let page = pdfcraft_model::pages(&doc).swap_remove(0);
    doc.update_dict(page.obj, |d| {
        let mut res = d.get(b"Resources").and_then(Object::as_dict).cloned().unwrap_or_default();
        let mut pats = Dict::new();
        pats.set(b"P0".to_vec(), Object::Ref(pdfcraft_cos::ObjRef::new(7, 0)));
        res.set(b"Pattern".to_vec(), Object::Dict(pats));
        d.set(b"Resources".to_vec(), Object::Dict(res));
    })
    .unwrap();
    mark(&mut doc, 0, &SECRET_AREA, "");
    doc
}

#[test]
fn images_in_patterns_fail_closed() {
    for (body, res) in
        [(&b"q 10 0 0 10 0 0 cm /Im0 Do Q"[..], "/XObject << /Im0 8 0 R >>"), (b"q 10 0 0 10 0 0 cm BI /W 1 /H 1 /CS /G /BPC 8 ID \x00 EI Q", "")]
    {
        let mut doc = page_with_pattern(body, res);
        assert_eq!(apply(&mut doc, None), Err(RedactError::Unsupported { page: 1, reason: Unsupported::HiddenImage }));
    }
    // A pattern that paints no image or text is fine.
    let mut doc = page_with_pattern(b"0 0 5 5 re f", "");
    assert!(apply(&mut doc, None).is_ok());
}

// ---- 8: form matrices ----

fn page_with_form(dict: &str, body: &[u8]) -> Document {
    let form = stream(&format!("/Type /XObject /Subtype /Form {dict} /Resources << /Font << /F1 5 0 R >> >>"), body);
    let mut doc = page_with(stream("", b"/Fm0 Do"), "", vec![form]);
    let page = pdfcraft_model::pages(&doc).swap_remove(0);
    doc.update_dict(page.obj, |d| {
        let mut res = d.get(b"Resources").and_then(Object::as_dict).cloned().unwrap_or_default();
        let mut xo = Dict::new();
        xo.set(b"Fm0".to_vec(), Object::Ref(pdfcraft_cos::ObjRef::new(7, 0)));
        res.set(b"XObject".to_vec(), Object::Dict(xo));
        d.set(b"Resources".to_vec(), Object::Dict(res));
    })
    .unwrap();
    mark(&mut doc, 0, &SECRET_AREA, "");
    doc
}

#[test]
fn a_form_with_an_unreadable_matrix_fails_and_a_bad_bbox_is_walked() {
    for matrix in ["/Matrix [1 0 0 1 0]", "/Matrix [1 0 0 1 0 (x)]", "/Matrix /Identity"] {
        let mut doc = page_with_form(&format!("/BBox [0 0 300 300] {matrix}"), SECRET_TEXT);
        assert_eq!(apply(&mut doc, None), Err(RedactError::Unsupported { page: 1, reason: Unsupported::FormMatrix }), "{matrix}");
    }
    // No bounds to test: the form is walked whole and the text under the mark is cut out, but the
    // proof can't place the form's text, so it does not vouch for the page.
    let mut doc = page_with_form("/BBox [0 0 300]", SECRET_TEXT);
    assert!(matches!(apply_keeping(&mut doc), Err(RedactError::ProofIncomplete { .. })));
    let mut doc = page_with_form("/BBox [0 0 300]", SECRET_TEXT);
    assert!(matches!(apply(&mut doc, None), Err(RedactError::ProofIncomplete { .. })));
}

// ---- 24: vertical writing is read as tokens ----

#[test]
fn wmode_is_found_however_it_is_spelled() {
    use crate::interp::cmap_vertical;
    for vertical in ["/WMode 1 def", "/WMode\n1 def", "/WMode\t01 def", "/WMode 1.0 def", "/CMapName /X def /WMode 1 def", "/WMode (x) def"] {
        assert!(cmap_vertical(vertical.as_bytes()), "{vertical:?}");
    }
    for horizontal in ["/WMode 0 def", "% /WMode 1 def\n/WMode 0 def", "(/WMode 1) show /WMode 0 def", "/CMapName /X def"] {
        assert!(!cmap_vertical(horizontal.as_bytes()), "{horizontal:?}");
    }
}

// ---- 19: what removed annotations said ----

#[test]
fn removed_annotation_text_is_collected() {
    let doc = page_with(
        stream("", SECRET_TEXT),
        "",
        vec![b"<< /V (parent value) /DV (parent default) >>".to_vec(), b"<< /Parent 7 0 R /V [(first) (second)] >>".to_vec()],
    );
    let mut d = Dict::new();
    d.set(b"Contents".to_vec(), Object::String(PdfString::text("a note")));
    d.set(b"RC".to_vec(), Object::String(PdfString::text("<body><p>rich <b>text</b> &amp; more</p></body>")));
    let strings = annotation_strings(&doc, &d, false);
    assert_eq!(strings[0], "a note");
    assert!(strings[1].contains("rich") && strings[1].contains("text") && strings[1].contains("& more"), "{strings:?}");
    // A widget: its field's value and default, inherited through /Parent.
    let mut w = Dict::new();
    w.set(b"Parent".to_vec(), Object::Ref(pdfcraft_cos::ObjRef::new(8, 0)));
    let strings = annotation_strings(&doc, &w, true);
    for expected in ["first", "second", "parent value", "parent default"] {
        assert!(strings.iter().any(|s| s == expected), "{expected}: {strings:?}");
    }
}

// ---- 18: overlay text is not removed text ----

#[test]
fn an_overlay_that_repeats_the_removed_words_is_not_a_survivor() {
    let mut doc = page_with(stream("", b"BT /F1 10 Tf 10 100 Td (Account 1234) Tj ET"), "", vec![]);
    let shape = Shape::Redact { quads: vec![rect_quad([8.0, 95.0, 120.0, 112.0])], overlay: "Account [redacted]".into(), look: Default::default() };
    let style = Style::default_for(&shape);
    add_annotation(&mut doc, &NewAnnotation { page: 0, shape, style, contents: String::new(), author: "T".into() }, &Meta::default()).unwrap();
    let (report, proof) = apply_with_proof(&mut doc, None, &opts_none(), &ProofOptions::default()).unwrap();
    assert!(report.glyphs > 0 && proof.passed(), "{}", proof.to_text());
    assert!(!content(&doc, 0).contains("1234"));
}
