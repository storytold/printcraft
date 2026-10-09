//! The independent redaction proof: what it catches, what it refuses to call clean.

use std::collections::HashSet;
use std::sync::Arc;

use pdfcraft_annot::{Meta, NewAnnotation, Shape, Style, add_annotation, rect_quad};
use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_incremental};
use sha2::Digest;

use super::*;
use crate::tests_failclosed::apply_keeping;
use crate::verify::tests_support::{count_text_operators, survivor_encodings};
use crate::verify::{Status, Surface, SurfaceReport, Verdict};

const SECRET: &str = "SECRETVALUE";

pub(super) fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut v = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    v.extend_from_slice(data);
    v.extend_from_slice(b"\nendstream");
    v
}

pub(super) fn pdf_bytes(objs: Vec<Vec<u8>>, trailer: &str) -> Vec<u8> {
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
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R {trailer} >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

pub(super) fn open(bytes: Vec<u8>) -> Document {
    Document::open(Arc::new(bytes)).unwrap()
}

/// Every glyph is 500 units wide (5 pt at 10 pt).
pub(super) const FONT: &str =
    "<< /Type /Font /Subtype /TrueType /BaseFont /Arial /FirstChar 32 /LastChar 126 /Widths 6 0 R /FontDescriptor << /Ascent 800 /Descent -200 >> >>";

/// A composite font without a /ToUnicode map: its text can't be decoded.
const OPAQUE_FONT: &str = "<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-H /DescendantFonts [<< /Type /Font /Subtype /CIDFontType2 /BaseFont /X /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /DW 1000 >>] >>";

/// Options of a one-page file: catalog, page and trailer entries, page resources, and more
/// objects (numbered from 7).
#[derive(Default)]
struct Spec<'a> {
    catalog: &'a str,
    page: &'a str,
    trailer: &'a str,
    resources: &'a str,
    extra: Vec<Vec<u8>>,
}

/// "Public" at the top, the secret lower down at x 10..65, y 100..110.
const CONTENT: &[u8] = b"BT /F1 10 Tf 10 200 Td (Public) Tj ET BT /F1 10 Tf 10 100 Td (SECRETVALUE) Tj ET";

pub(super) const REGION: [f64; 4] = [8.0, 95.0, 70.0, 112.0];

fn build_with(content: &[u8], spec: Spec<'_>) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {} >>", spec.catalog).into_bytes(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R {} /Resources << /Font << /F1 5 0 R >> {} >> >>",
            spec.page, spec.resources
        )
        .into_bytes(),
        stream("", content),
        FONT.as_bytes().to_vec(),
        format!("[{}]", vec!["500"; 95].join(" ")).into_bytes(),
    ];
    objs.extend(spec.extra);
    pdf_bytes(objs, spec.trailer)
}

pub(super) fn mark(doc: &mut Document, rects: &[[f64; 4]]) {
    let shape = Shape::Redact { quads: rects.iter().map(|r| rect_quad(*r)).collect(), overlay: String::new(), look: Default::default() };
    let style = Style::default_for(&shape);
    add_annotation(doc, &NewAnnotation { page: 0, shape, style, contents: String::new(), author: "Tester".into() }, &Meta::default()).unwrap();
}

/// The standard page with `spec`, the secret's region marked.
fn marked(spec: Spec<'_>) -> Document {
    marked_content(CONTENT, spec)
}

fn marked_content(content: &[u8], spec: Spec<'_>) -> Document {
    let mut doc = open(build_with(content, spec));
    mark(&mut doc, &[REGION]);
    doc
}

/// Apply, and tell the snapshot which objects the apply created (its overlay streams).
fn applied(doc: &mut Document, snap: &mut Snapshot) -> Result<Report, RedactError> {
    let known: HashSet<u32> = doc.object_numbers().into_iter().collect();
    let result = apply_keeping(doc);
    let created: Vec<ObjRef> = doc
        .object_numbers()
        .into_iter()
        .filter(|n| !known.contains(n))
        .map(|n| ObjRef::new(n, doc.generation(n)))
        .filter(|r| doc.get(*r).as_dict().and_then(|d| d.name(b"PCMark")) == Some(b"Redaction"))
        .collect();
    snap.set_overlay_streams(doc, &created).unwrap();
    result
}

/// Capture, apply, prove.
pub(super) fn run(doc: &mut Document) -> (Result<Report, RedactError>, Proof) {
    let mut snap = Snapshot::capture(doc, None).unwrap();
    let result = applied(doc, &mut snap);
    let proof = snap.prove(doc, &ProofOptions::default());
    (result, proof)
}

/// Prove a document the redaction was never applied to (as a full save would write it).
pub(super) fn prove_unapplied(doc: &mut Document) -> Proof {
    let snap = Snapshot::capture(doc, None).unwrap();
    doc.require_full_save();
    snap.prove(doc, &ProofOptions::default())
}

pub(super) fn surface(p: &Proof, s: Surface) -> &SurfaceReport {
    p.surfaces.iter().find(|r| r.surface == s).unwrap()
}

fn page_content(doc: &Document) -> String {
    let p = &pdfcraft_model::pages(doc)[0];
    let (_, data) = page_streams(doc, &p.dict, 0).unwrap();
    data.iter().map(|d| String::from_utf8_lossy(d).into_owned()).collect::<Vec<_>>().join("\n")
}

fn residue(r: &Result<Report, RedactError>) -> bool {
    matches!(r, Err(RedactError::Residue(n)) if *n > 0)
}

#[test]
fn the_proof_reads_a_pages_streams_as_the_one_stream_they_are() {
    // The TJ array of the secret ends the first piece and its operator starts the second
    // (#153): the proof reads the pieces as the one stream they are, so the removed text is
    // neither reported as a survivor nor does the page count as unreadable.
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents [4 0 R 7 0 R] /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", b"BT /F1 10 Tf 10 100 Td [(SECRETVALUE)]"),
        FONT.as_bytes().to_vec(),
        format!("[{}]", vec!["500"; 95].join(" ")).into_bytes(),
        stream("", b"TJ ET BT /F1 10 Tf 10 200 Td (Public) Tj ET"),
    ];
    let mut doc = open(pdf_bytes(objs, "/Root 1 0 R"));
    mark(&mut doc, &[REGION]);
    let (r, proof) = run(&mut doc);
    let r = r.unwrap();
    assert_eq!(r.glyphs, 11, "every glyph of the split string is removed: {r:?}");
    assert!(proof.passed(), "{}", proof.to_text());
    assert_eq!(proof.survivors(), 0);
    let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
    assert!(!saved.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()), "the secret is gone from the file");
}

#[test]
fn a_clean_redaction_passes_and_the_neighbour_is_untouched() {
    let mut doc = marked(Spec::default());
    let (r, proof) = run(&mut doc);
    r.unwrap();
    assert!(proof.passed(), "{}", proof.to_text());
    assert_eq!(proof.entries.len(), 1);
    let e = &proof.entries[0];
    assert_eq!((e.status, e.page, e.region), (Status::Verified, 0, REGION));
    assert_eq!((e.runs_removed, e.codes_removed), (1, SECRET.len()));
    let content = page_content(&doc);
    assert!(content.contains("(Public) Tj") && !content.contains(SECRET), "{content}");
    for s in &proof.surfaces {
        assert!(matches!(s.verdict, Verdict::Clean | Verdict::Absent), "{s:?}");
    }
    assert_eq!(surface(&proof, Surface::Revisions).verdict, Verdict::Clean);
}

#[test]
fn the_manifest_has_digests_and_counts_but_no_plaintext() {
    let mut doc = marked(Spec::default());
    // The digest of the original content: a shared proof must not carry it.
    let original: String = sha2::Sha256::digest(page_content(&doc).as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    let (r, proof) = run(&mut doc);
    r.unwrap();
    let e = &proof.entries[0];
    let after = e.page_sha256_after.as_deref().unwrap();
    assert!(after.len() == 64 && after.bytes().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(after, original);
    for shown in [proof.to_text(), format!("{proof:?}")] {
        assert!(!shown.contains(&original), "{shown}");
    }
    // Two glyph-carrying operators before; the removed one left a number-only TJ, which carries
    // no glyphs and is not counted.
    assert_eq!((e.text_ops_before, e.text_ops_after), (Some(2), Some(1)));
    assert!(e.removed_text.is_none());
    for shown in [proof.to_text(), format!("{proof:?}")] {
        assert!(!shown.contains(SECRET), "{shown}");
    }

    // Plaintext only on request.
    let mut doc = marked(Spec::default());
    let mut snap = Snapshot::capture(&doc, None).unwrap();
    assert!(!format!("{snap:?}").contains(SECRET));
    applied(&mut doc, &mut snap).unwrap();
    let with = snap.prove(&doc, &ProofOptions { include_plaintext: true, ..Default::default() });
    assert_eq!(with.entries[0].removed_text.as_deref(), Some(&[SECRET.to_string()][..]));
}

#[test]
fn survivors_do_not_echo_the_text_either() {
    let mut doc = marked(Spec { trailer: "/Info 7 0 R", extra: vec![b"<< /Title (SECRETVALUE) >>".to_vec()], ..Spec::default() });
    let (r, proof) = run(&mut doc);
    assert!(residue(&r));
    assert!(!proof.passed());
    for shown in [proof.to_text(), format!("{proof:?}"), r.unwrap_err().to_string()] {
        assert!(!shown.contains(SECRET), "{shown}");
    }
}

#[test]
fn numeric_only_tj_is_not_glyph_carrying() {
    assert_eq!(count_text_operators(b"BT (Tj) Tj [ -250 ] TJ [(x) -3 (y)] TJ () Tj [] TJ ET"), 2);
    assert_eq!(count_text_operators(b"BT (a) ' 1 2 (b) \" [ 10 -10 ] TJ ET"), 2);
}

#[test]
fn a_survivor_in_the_info_dictionary() {
    let mut doc =
        marked(Spec { trailer: "/Info 7 0 R", extra: vec![b"<< /Title (Report SECRETVALUE) /Author (Someone) >>".to_vec()], ..Spec::default() });
    let (r, proof) = run(&mut doc);
    assert!(residue(&r), "apply fails closed on a survivor");
    assert_eq!(surface(&proof, Surface::Info).verdict, Verdict::Survivor);
    assert_eq!(proof.entries[0].status, Status::Failed);
}

#[test]
fn a_survivor_in_xmp() {
    let xmp = stream("/Type /Metadata /Subtype /XML", b"<x:xmpmeta><dc:title>Quarterly &amp; SECRET&#86;ALUE</dc:title></x:xmpmeta>");
    let mut doc = marked(Spec { catalog: "/Metadata 7 0 R", extra: vec![xmp], ..Spec::default() });
    let (r, proof) = run(&mut doc);
    assert!(residue(&r));
    assert_eq!(surface(&proof, Surface::Xmp).verdict, Verdict::Survivor, "{}", proof.to_text());
}

#[test]
fn a_survivor_in_an_annotation_string() {
    let annot = b"<< /Type /Annot /Subtype /Text /Rect [200 200 220 220] /Contents (note: SECRETVALUE) >>".to_vec();
    let mut doc = marked(Spec { page: "/Annots [7 0 R]", extra: vec![annot], ..Spec::default() });
    let (r, proof) = run(&mut doc);
    assert!(residue(&r));
    assert_eq!(surface(&proof, Surface::ObjectStrings).verdict, Verdict::Survivor);
    assert_eq!(proof.entries[0].status, Status::Failed);
}

fn attachment_spec(payload: &[u8]) -> Spec<'static> {
    Spec {
        catalog: "/Names << /EmbeddedFiles << /Names [(a.bin) 7 0 R] >> >>",
        extra: vec![b"<< /Type /Filespec /F (a.bin) /EF << /F 8 0 R >> >>".to_vec(), stream("/Type /EmbeddedFile", payload)],
        ..Spec::default()
    }
}

#[test]
fn a_survivor_in_an_attachment() {
    let mut doc = marked(attachment_spec(b"the SECRETVALUE is attached"));
    let (r, proof) = run(&mut doc);
    assert!(residue(&r));
    assert_eq!(surface(&proof, Surface::EmbeddedFiles).verdict, Verdict::Survivor);

    // UTF-16LE with a byte order mark.
    let mut le = vec![0xFF, 0xFE];
    le.extend(SECRET.encode_utf16().flat_map(u16::to_le_bytes));
    let mut doc = marked(attachment_spec(&le));
    let (r, proof) = run(&mut doc);
    assert!(residue(&r));
    assert_eq!(surface(&proof, Surface::EmbeddedFiles).verdict, Verdict::Survivor);
}

#[test]
fn a_survivor_in_a_nested_attachment() {
    let inner = build_with(b"BT /F1 10 Tf 10 100 Td (SECRETVALUE) Tj ET", Spec::default());
    let middle = build_with(b"", attachment_spec(&inner));
    let mut doc = marked(attachment_spec(&middle));
    let (r, proof) = run(&mut doc);
    assert!(residue(&r), "{}", proof.to_text());
    let s = surface(&proof, Surface::EmbeddedFiles);
    assert_eq!(s.verdict, Verdict::Survivor);
    assert!(s.locations.iter().any(|l| l.matches(" > ").count() >= 1), "{:?}", s.locations);
}

#[test]
fn a_clean_nested_attachment_is_swept_and_clean() {
    let inner = build_with(b"BT /F1 10 Tf 10 100 Td (harmless) Tj ET", Spec::default());
    let mut doc = marked(attachment_spec(&inner));
    let (r, proof) = run(&mut doc);
    r.unwrap();
    assert!(proof.passed(), "{}", proof.to_text());
    assert_eq!(surface(&proof, Surface::EmbeddedFiles).verdict, Verdict::Clean);
}

#[test]
fn unsweepable_attachments_fail_closed() {
    for payload in [&b"PK\x03\x04 zipped"[..], b"%PDF-1.7 not really a pdf", b"\x1f\x8b gzip"] {
        let mut doc = marked(attachment_spec(payload));
        let (r, proof) = run(&mut doc);
        assert!(matches!(r, Err(RedactError::ProofIncomplete { .. })), "an incomplete proof fails the operation: {r:?}");
        assert_eq!(surface(&proof, Surface::EmbeddedFiles).verdict, Verdict::Unswept, "{}", String::from_utf8_lossy(payload));
        assert!(!proof.passed(), "a proof that could not sweep must not pass");
    }
}

#[test]
fn an_undecodable_stream_fails_closed() {
    let junk = stream("/Filter /NoSuchDecode", b"\x00\x01\x02 junk");
    let mut doc = marked(Spec { catalog: "/Extra 7 0 R", extra: vec![junk], ..Spec::default() });
    let (r, proof) = run(&mut doc);
    assert!(matches!(r, Err(RedactError::ProofIncomplete { .. })), "an incomplete proof fails the operation: {r:?}");
    assert_eq!(surface(&proof, Surface::DecodedStreams).verdict, Verdict::Unswept, "{}", proof.to_text());
    assert!(!proof.passed());
}

#[test]
fn an_incremental_remnant_fails_the_proof() {
    let mut doc = marked(Spec::default());
    let mut snap = Snapshot::capture(&doc, None).unwrap();
    // A "redaction" saved as an incremental update: the page shows something else now, but the
    // first revision still holds the original content.
    let mut other = open(write_incremental(&doc, &SaveOptions::default()).unwrap());
    let new = other.add(Object::Stream(Stream::flate(Dict::new(), b"BT /F1 10 Tf 10 200 Td (Public) Tj ET")));
    let page = pdfcraft_model::pages(&other).swap_remove(0);
    other.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Ref(new))).unwrap();
    let saved = write_incremental(&other, &SaveOptions::default()).unwrap();
    let proof = snap.prove_saved(&saved, &ProofOptions::default());
    assert!(!proof.passed());
    assert_eq!(surface(&proof, Surface::Revisions).verdict, Verdict::Survivor);
    assert_eq!(surface(&proof, Surface::RawBytes).verdict, Verdict::Survivor);
    // The page text itself is clean: only the file structure gives it away.
    assert_eq!(surface(&proof, Surface::ExtractedText).verdict, Verdict::Clean);

    // The real redaction, saved as apply() requires, is a single clean revision.
    applied(&mut doc, &mut snap).unwrap();
    let full = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let proof = snap.prove_saved(&full, &ProofOptions::default());
    assert!(proof.passed(), "{}", proof.to_text());
}

#[test]
fn the_secret_in_each_encoding_is_found() {
    // Spelled as PDF syntax in a catalog entry.
    let utf16: String = SECRET.encode_utf16().flat_map(u16::to_be_bytes).map(|b| format!("{b:02X}")).collect();
    let utf16le: String = SECRET.encode_utf16().flat_map(u16::to_le_bytes).map(|b| format!("{b:02X}")).collect();
    let hex8: String = SECRET.bytes().map(|b| format!("{b:02x}")).collect();
    let forms = [
        format!("({SECRET})"),
        format!("<FEFF{utf16}>"),
        format!("<FFFE{utf16le}>"),
        format!("<EFBBBF{}>", hex8.to_uppercase()),
        format!("<{hex8}>"),
        format!("<{utf16}>"),
        "(\\123ECRETVALUE)".to_string(),
    ];
    for f in forms {
        let mut doc = marked(Spec { catalog: &format!("/Extra {f}"), ..Spec::default() });
        let (r, proof) = run(&mut doc);
        assert!(residue(&r), "{f}: {}", proof.to_text());
    }

    // Spelled as bytes in the file, including the backslash-escaped literal form.
    let paren = "SECR(ET)X\\Y";
    let enc = survivor_encodings(paren);
    assert!(enc.len() >= 10);
    for spelling in enc {
        let mut hay = b"junk ".to_vec();
        hay.extend(&spelling);
        hay.extend(b" junk");
        assert!(verify::tests_support::found(paren, &hay), "{}", String::from_utf8_lossy(&spelling));
    }
    assert!(!verify::tests_support::found(paren, b"nothing to see"));
}

#[test]
fn a_rotated_page_is_proven_too() {
    let mut doc = marked(Spec { page: "/Rotate 90", ..Spec::default() });
    let (r, proof) = run(&mut doc);
    r.unwrap();
    assert!(proof.passed(), "{}", proof.to_text());
    assert_eq!(proof.entries[0].status, Status::Verified);
}

#[test]
fn a_region_without_text_is_recorded_as_such() {
    let mut doc = open(build_with(CONTENT, Spec::default()));
    mark(&mut doc, &[[150.0, 20.0, 200.0, 40.0]]);
    let (r, proof) = run(&mut doc);
    r.unwrap();
    assert_eq!(proof.entries[0].status, Status::VerifiedNoTextInRegion);
    assert!(proof.passed());
}

fn opaque_spec() -> Spec<'static> {
    Spec { resources: "/Font << /F1 5 0 R /F2 7 0 R >>", extra: vec![OPAQUE_FONT.as_bytes().to_vec()], ..Spec::default() }
}

#[test]
fn text_that_cannot_be_decoded_makes_a_region_unverifiable() {
    let mut doc = open(build_with(b"BT /F2 10 Tf 10 200 Td <00410042> Tj ET", opaque_spec()));
    mark(&mut doc, &[[150.0, 20.0, 200.0, 40.0]]);
    let (r, proof) = run(&mut doc);
    assert!(matches!(r, Err(RedactError::ProofIncomplete { .. })), "an incomplete proof fails the operation: {r:?}");
    assert_eq!(proof.entries[0].status, Status::Unverifiable);
    assert!(!proof.passed(), "no PASS while a region is unverifiable");
}

#[test]
fn removed_text_of_an_undecodable_font_is_swept_by_its_codes() {
    let content = b"BT /F2 10 Tf 10 100 Td <0041004200430044> Tj ET";
    let mut doc = open(build_with(content, opaque_spec()));
    mark(&mut doc, &[[0.0, 90.0, 100.0, 120.0]]);
    let (r, proof) = run(&mut doc);
    r.unwrap();
    assert_eq!(proof.entries[0].runs_removed, 1);
    assert_eq!(proof.entries[0].status, Status::Verified, "{}", proof.to_text());

    // Left in place, the codes are still on the page and the proof says so.
    let mut doc = open(build_with(content, opaque_spec()));
    mark(&mut doc, &[[0.0, 90.0, 100.0, 120.0]]);
    let proof = prove_unapplied(&mut doc);
    assert!(!proof.passed());
    assert_eq!(surface(&proof, Surface::ExtractedText).verdict, Verdict::Survivor);
}

#[test]
fn nothing_applied_means_survivors_where_the_text_was() {
    let mut doc = marked(Spec::default());
    let proof = prove_unapplied(&mut doc);
    assert!(!proof.passed());
    assert_eq!(surface(&proof, Surface::ExtractedText).verdict, Verdict::Survivor);
    assert_eq!(surface(&proof, Surface::DecodedStreams).verdict, Verdict::Survivor);
    assert_eq!(proof.entries[0].status, Status::Failed);
}

#[test]
fn a_document_that_would_save_incrementally_is_not_proven() {
    let mut doc = marked(Spec::default());
    let mut snap = Snapshot::capture(&doc, None).unwrap();
    applied(&mut doc, &mut snap).unwrap();
    // A reopened copy no longer carries the full-save requirement.
    let reopened = open(write_incremental(&doc, &SaveOptions::default()).unwrap());
    let proof = snap.prove(&reopened, &ProofOptions::default());
    assert_eq!(surface(&proof, Surface::Revisions).verdict, Verdict::Survivor);
}

fn image_page() -> Document {
    let image = stream("/Type /XObject /Subtype /Image /Width 20 /Height 20 /ColorSpace /DeviceGray /BitsPerComponent 8", &[0xFFu8; 400]);
    let spec = Spec { resources: "/XObject << /Im1 7 0 R >>", extra: vec![image], ..Spec::default() };
    marked_region(b"q 100 0 0 100 100 100 cm /Im1 Do Q 0 0 1 rg 120 120 60 60 re f", spec)
}

fn marked_region(content: &[u8], spec: Spec<'_>) -> Document {
    let mut doc = open(build_with(content, spec));
    mark(&mut doc, &[[100.0, 100.0, 150.0, 150.0]]);
    doc
}

#[test]
fn images_and_paths_under_a_region_are_checked_independently() {
    let mut doc = image_page();
    let (r, proof) = run(&mut doc);
    r.unwrap();
    assert!(proof.passed(), "{}", proof.to_text());
    assert_eq!(proof.entries[0].status, Status::VerifiedNoTextInRegion);

    // The same page unredacted: the image and the path are both still there.
    let proof = prove_unapplied(&mut image_page());
    let e = &proof.entries[0];
    assert_eq!(e.status, Status::Failed);
    assert!(e.detail.contains("image") && e.detail.contains("path"), "{}", e.detail);
}

#[test]
fn a_shading_under_a_region_must_be_clipped() {
    let sh =
        b"<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 300 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> >>"
            .to_vec();
    let spec = || Spec { resources: "/Shading << /Sh1 7 0 R >>", extra: vec![sh.clone()], ..Spec::default() };
    let mut doc = marked_region(b"/Sh1 sh", spec());
    let (r, proof) = run(&mut doc);
    r.unwrap();
    assert!(proof.passed(), "{}", proof.to_text());

    let proof = prove_unapplied(&mut marked_region(b"/Sh1 sh", spec()));
    assert_eq!(proof.entries[0].status, Status::Failed);
}

#[test]
fn text_kept_elsewhere_does_not_fail_the_proof() {
    // The same word is redacted in one place and legitimately remains in another.
    let content = b"BT /F1 10 Tf 10 200 Td (SECRETVALUE) Tj ET BT /F1 10 Tf 10 100 Td (SECRETVALUE) Tj ET";
    let mut doc = marked_content(content, Spec::default());
    let (r, proof) = run(&mut doc);
    r.unwrap();
    assert!(proof.passed(), "{}", proof.to_text());
    assert!(proof.entries[0].detail.contains("remain in retained content"));

    // A copy that should have gone is still caught, by count.
    let proof = prove_unapplied(&mut marked_content(content, Spec::default()));
    assert_eq!(surface(&proof, Surface::ExtractedText).verdict, Verdict::Survivor);
}

#[test]
fn the_proof_text_keeps_its_honesty_wording() {
    // The closing caveat of a proof is a promise to whoever reads a shared proof: pin it so the
    // wording cannot silently drop out of the text for either verdict.
    let honesty = "Evidence, not a guarantee";
    let passed = Proof { entries: vec![], surfaces: vec![], failures: vec![] };
    assert!(passed.passed());
    assert!(passed.to_text().contains(honesty), "{}", passed.to_text());

    let failed = Proof { entries: vec![], surfaces: vec![], failures: vec!["the marked text is still readable".into()] };
    assert!(!failed.passed());
    assert!(failed.to_text().contains(honesty), "{}", failed.to_text());
}

// ── Independence, scope and bounds of the sweep ─────────────────────────────────────────────

use crate::verify::tests_support;

/// Hang `obj` off the catalog (as `/Extra`) so the saved file keeps it.
pub(super) fn attach(doc: &mut Document, obj: Object) {
    let r = doc.add(obj);
    let root = doc.root().unwrap();
    doc.update_dict(root, |d| d.set(b"Extra".to_vec(), Object::Ref(r))).unwrap();
}

/// Add a content stream (with `dict`) after the page's own.
fn append_content(doc: &mut Document, dict: Dict, data: &[u8]) {
    let new = doc.add(Object::Stream(Stream::from_raw(dict, data.to_vec())));
    let page = pdfcraft_model::pages(doc).swap_remove(0);
    let first = page.dict.get(b"Contents").cloned().unwrap();
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(vec![first, Object::Ref(new)]))).unwrap();
}

#[test]
fn a_glyph_the_interpreter_kept_is_found_by_placement_alone() {
    // The snapshot is taken where the region holds no text, so it has nothing to search for:
    // only the independent placement of glyphs can notice the text in the output.
    let blank = marked_content(b"BT /F1 10 Tf 10 200 Td (Public) Tj ET", Spec::default());
    let snap = Snapshot::capture(&blank, None).unwrap();
    let mut shown = marked(Spec::default());
    shown.require_full_save();
    let proof = snap.prove(&shown, &ProofOptions::default());
    assert!(!proof.passed());
    let e = &proof.entries[0];
    assert_eq!(e.status, Status::Failed, "{}", proof.to_text());
    assert!(e.detail.contains("glyph(s) still show"), "{}", e.detail);
    assert_eq!(proof.survivors(), 0, "no needle was involved");
    // The neighbour at y 200 is outside the region and is not reported.
    assert!(!e.detail.contains("12 glyph"), "{}", e.detail);
}

#[test]
fn glyphs_are_placed_with_the_text_matrix_and_the_form_matrix() {
    // The secret drawn through a rotated/translated text matrix inside a scaled form: the
    // placement has to follow `Tm` and the form's `/Matrix`, not the raw operands.
    let form = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Matrix [1 0 0 1 0 0] /Resources << /Font << /F1 5 0 R >> >>",
        b"BT /F1 10 Tf 1 0 0 1 10 100 Tm (SECRETVALUE) Tj ET",
    );
    let blank = marked_content(b"BT /F1 10 Tf 10 200 Td (Public) Tj ET", Spec::default());
    let snap = Snapshot::capture(&blank, None).unwrap();
    let mut doc = marked_content(b"/Fm1 Do", Spec { resources: "/XObject << /Fm1 7 0 R >>", extra: vec![form], ..Spec::default() });
    doc.require_full_save();
    let proof = snap.prove(&doc, &ProofOptions::default());
    assert_eq!(proof.entries[0].status, Status::Failed, "{}", proof.to_text());
    assert!(proof.entries[0].detail.contains("glyph(s) still show"));
}

#[test]
fn an_ambiguous_needle_is_still_swept_outside_page_content() {
    // The word also stays on the page legitimately (so page text, the raw file and decoded
    // streams can only count it), but a copy in the Info dictionary is not page content.
    let content = b"BT /F1 10 Tf 10 200 Td (SECRETVALUE) Tj ET BT /F1 10 Tf 10 100 Td (SECRETVALUE) Tj ET";
    let spec = Spec { trailer: "/Info 7 0 R", extra: vec![b"<< /Title (SECRETVALUE) >>".to_vec()], ..Spec::default() };
    let mut doc = marked_content(content, spec);
    let (r, proof) = run(&mut doc);
    assert!(residue(&r), "{r:?}");
    assert_eq!(surface(&proof, Surface::Info).verdict, Verdict::Survivor, "{}", proof.to_text());

    // Likewise an XMP packet and an annotation string.
    let xmp = stream("/Type /Metadata /Subtype /XML", b"<x:xmpmeta>SECRETVALUE</x:xmpmeta>");
    let mut doc = marked_content(content, Spec { catalog: "/Metadata 7 0 R", extra: vec![xmp], ..Spec::default() });
    assert_eq!(surface(&run(&mut doc).1, Surface::Xmp).verdict, Verdict::Survivor);
    let annot = b"<< /Type /Annot /Subtype /Text /Rect [200 200 220 220] /Contents (SECRETVALUE) >>".to_vec();
    let mut doc = marked_content(content, Spec { page: "/Annots [7 0 R]", extra: vec![annot], ..Spec::default() });
    assert_eq!(surface(&run(&mut doc).1, Surface::ObjectStrings).verdict, Verdict::Survivor);
}

#[test]
fn a_pcmark_key_in_the_input_does_not_exempt_a_stream() {
    // A content stream that claims to be a redaction box paints inside the region, and was not
    // created by this run: it counts as paint like any other.
    let blank = b"BT /F1 10 Tf 10 200 Td (Public) Tj ET";
    let mut doc = marked_content(blank, Spec::default());
    let mut d = Dict::new();
    d.set(b"PCMark".to_vec(), Object::Name(b"Redaction".to_vec()));
    append_content(&mut doc, d, b"0 0 0 rg 8 95 62 17 re f");
    let proof = prove_unapplied(&mut doc);
    let e = &proof.entries[0];
    assert_eq!(e.status, Status::Failed, "{}", proof.to_text());
    assert!(e.detail.contains("path"), "{}", e.detail);
}

#[test]
fn only_the_streams_this_run_created_are_exempt() {
    // The same stream, declared as created by this run, is the box that was drawn on purpose.
    let blank = b"BT /F1 10 Tf 10 200 Td (Public) Tj ET";
    let mut doc = marked_content(blank, Spec::default());
    let mut snap = Snapshot::capture(&doc, None).unwrap();
    let new = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"0 0 0 rg 8 95 62 17 re f".to_vec())));
    let page = pdfcraft_model::pages(&doc).swap_remove(0);
    let first = page.dict.get(b"Contents").cloned().unwrap();
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(vec![first, Object::Ref(new)]))).unwrap();
    doc.require_full_save();
    snap.set_overlay_streams(&doc, &[new]).unwrap();
    let proof = snap.prove(&doc, &ProofOptions::default());
    assert!(proof.passed(), "{}", proof.to_text());
}

#[test]
fn overlay_text_does_not_collide_with_what_was_removed() {
    // "Account" is redacted and the label says "Account [redacted]": the label is not a survivor.
    let content = b"BT /F1 10 Tf 10 100 Td (Account 1234) Tj ET";
    let mut doc = open(build_with(content, Spec::default()));
    let shape = Shape::Redact { quads: vec![rect_quad([8.0, 95.0, 100.0, 112.0])], overlay: "Account [redacted]".into(), look: Default::default() };
    let style = Style::default_for(&shape);
    add_annotation(&mut doc, &NewAnnotation { page: 0, shape, style, contents: String::new(), author: "T".into() }, &Meta::default()).unwrap();
    let (r, proof) = run(&mut doc);
    r.unwrap();
    assert!(proof.passed(), "{}", proof.to_text());
    assert_eq!(surface(&proof, Surface::ExtractedText).verdict, Verdict::Clean);
}

#[test]
fn removed_annotation_and_field_text_is_searched_for_too() {
    let spec = || Spec { trailer: "/Info 7 0 R", extra: vec![b"<< /Subject (call back ANNOTWORDS) >>".to_vec()], ..Spec::default() };
    // Not asked for: the proof does not know about it.
    let mut doc = marked(spec());
    let (r, proof) = run(&mut doc);
    r.unwrap();
    assert!(proof.passed(), "{}", proof.to_text());
    // Asked for: it is found where it was left.
    let mut doc = marked(spec());
    let mut snap = Snapshot::capture(&doc, None).unwrap();
    snap.add_removed_strings(&["  call back ANNOTWORDS ".to_string(), "x".to_string(), String::new()]);
    let result = applied(&mut doc, &mut snap);
    let proof = snap.prove(&doc, &ProofOptions::default());
    assert!(residue(&result) || !proof.passed());
    assert_eq!(surface(&proof, Surface::Info).verdict, Verdict::Survivor, "{}", proof.to_text());
    // And an absent string is clean.
    let mut doc = marked(Spec::default());
    let mut snap = Snapshot::capture(&doc, None).unwrap();
    snap.add_removed_strings(&["never present anywhere".to_string()]);
    applied(&mut doc, &mut snap).unwrap();
    assert!(snap.prove(&doc, &ProofOptions::default()).passed());
}

#[test]
fn a_damaged_stream_tail_is_unswept_not_truncated() {
    let text: Vec<u8> = (0..4000u32).map(|i| b'a' + (i * 7 % 23) as u8).collect();
    let mut s = Stream::flate(Dict::new(), &text);
    let cut = s.raw.len() - 8;
    s.raw = Arc::new(s.raw[..cut].to_vec()).into();
    // Nothing was removed (the snapshot's region is blank), so the only issue is the stream.
    let snap = Snapshot::capture(&marked_content(b"BT /F1 10 Tf 10 200 Td (Public) Tj ET", Spec::default()), None).unwrap();
    let mut doc = marked_content(b"BT /F1 10 Tf 10 200 Td (Public) Tj ET", Spec::default());
    attach(&mut doc, Object::Stream(s));
    doc.require_full_save();
    let proof = snap.prove(&doc, &ProofOptions::default());
    assert_eq!(surface(&proof, Surface::DecodedStreams).verdict, Verdict::Unswept, "{}", proof.to_text());
    assert!(!proof.passed());
}

#[test]
fn a_damaged_page_content_tail_makes_the_page_unreadable() {
    let text: Vec<u8> = b"BT /F1 10 Tf 10 100 Td (filler) Tj ET\n".repeat(200);
    let mut s = Stream::flate(Dict::new(), &text);
    let cut = s.raw.len() - 8;
    s.raw = Arc::new(s.raw[..cut].to_vec()).into();
    let mut doc = marked(Spec::default());
    let page = pdfcraft_model::pages(&doc).swap_remove(0);
    let new = doc.add(Object::Stream(s));
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Ref(new))).unwrap();
    doc.require_full_save();
    let snap = Snapshot::capture(&marked(Spec::default()), None).unwrap();
    let proof = snap.prove(&doc, &ProofOptions::default());
    assert!(!proof.passed());
    assert_eq!(surface(&proof, Surface::ExtractedText).verdict, Verdict::Unswept, "{}", proof.to_text());
}

#[test]
fn a_malformed_form_matrix_or_bbox_is_unswept() {
    for dict in ["/Matrix [1 0 0]", "/Matrix [1 0 0 1 0 (x)]", "/BBox [0 0 300]"] {
        let form = stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] {dict}"), b"BT /F1 10 Tf 10 100 Td (x) Tj ET");
        let spec = Spec { resources: "/XObject << /Fm1 7 0 R >>", extra: vec![form], ..Spec::default() };
        let mut doc = marked_content(b"/Fm1 Do", spec);
        let proof = prove_unapplied(&mut doc);
        assert_eq!(surface(&proof, Surface::ExtractedText).verdict, Verdict::Unswept, "{dict}: {}", proof.to_text());
        assert!(!proof.passed());
        assert!(proof.entries[0].detail.contains("malformed"), "{dict}: {}", proof.to_text());
    }
}

#[test]
fn images_hidden_in_patterns_soft_masks_and_type3_glyphs_are_not_skipped() {
    let image = "BI /W 1 /H 1 /CS /G /BPC 8 ID \u{ff} EI";
    let tile =
        |data: &str| stream("/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << >>", data.as_bytes());
    let group = stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Group << /S /Transparency >> /Resources << >>", image.as_bytes());
    let proc = stream("", format!("1000 0 d0 {image}").as_bytes());
    let cases: Vec<(&str, Spec)> = vec![
        ("pattern", Spec { resources: "/Pattern << /P1 7 0 R >>", extra: vec![tile(image)], ..Spec::default() }),
        (
            "soft mask",
            Spec { resources: "/ExtGState << /G1 << /SMask << /Type /Mask /S /Luminosity /G 7 0 R >> >> >>", extra: vec![group], ..Spec::default() },
        ),
        (
            "type3",
            Spec {
                resources: "/Font << /F3 << /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 7 0 R >> /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] >> >>",
                extra: vec![proc],
                ..Spec::default()
            },
        ),
    ];
    for (what, spec) in cases {
        // The region holds no text and no visible image: only the hidden image is in question.
        let mut doc = marked_content(b"BT /F1 10 Tf 10 200 Td (Public) Tj ET", spec);
        let proof = prove_unapplied(&mut doc);
        let e = &proof.entries[0];
        assert_eq!(e.status, Status::Unverifiable, "{what}: {}", proof.to_text());
        assert!(e.detail.contains("pattern, soft mask or Type3"), "{what}: {}", e.detail);
        assert!(!proof.passed());
    }
    // The same resources without an image are fine.
    let spec = Spec { resources: "/Pattern << /P1 7 0 R >>", extra: vec![tile("0 0 5 5 re f")], ..Spec::default() };
    let mut doc = marked_content(b"BT /F1 10 Tf 10 200 Td (Public) Tj ET", spec);
    let proof = prove_unapplied(&mut doc);
    assert_eq!(proof.entries[0].status, Status::VerifiedNoTextInRegion, "{}", proof.to_text());
}

#[test]
fn a_signed_document_is_proven_only_when_allowed() {
    let spec = || Spec { catalog: "/AcroForm << /SigFlags 3 /Fields [] >>", ..Spec::default() };
    let mut doc = marked(spec());
    let snap = Snapshot::capture(&doc, None).unwrap();
    doc.require_full_save();
    let refused = snap.prove(&doc, &ProofOptions::default());
    assert!(refused.failures.iter().any(|f| f.contains("could not be serialized")), "{}", refused.to_text());
    let allowed = snap.prove(&doc, &ProofOptions { allow_signed: true, ..Default::default() });
    assert!(!allowed.failures.iter().any(|f| f.contains("could not be serialized")), "{}", allowed.to_text());
    // (The text was never removed here, so the proof still fails, by a survivor.)
    assert!(allowed.survivors() > 0);
}

#[test]
fn the_sweep_is_bounded() {
    // A CMap whose ranges rewrite the same 64Ki codes again and again costs work, whatever the map size.
    let mut cmap = b"1 begincodespacerange <0000> <FFFF> endcodespacerange\n".to_vec();
    for _ in 0..200 {
        cmap.extend_from_slice(b"100 beginbfrange\n");
        for _ in 0..100 {
            cmap.extend_from_slice(b"<0000> <FFFF> <0041>\n");
        }
        cmap.extend_from_slice(b"endbfrange\n");
    }
    let start = std::time::Instant::now();
    assert!(!tests_support::cmap_is_complete(&cmap));
    assert!(start.elapsed().as_secs() < 20, "bounded work");
    // A small CMap is read in full.
    assert!(tests_support::cmap_is_complete(b"1 beginbfchar <01> <0041> endbfchar"));
    // Nested objects, cumulative decoding and searching all stop and say so.
    assert!(tests_support::walks_in_full(60));
    assert!(!tests_support::walks_in_full(200));
    assert_eq!(tests_support::decode_budget_runs_out(), (true, true));
    assert_eq!(tests_support::search_budget_runs_out(), (true, true));
}

#[test]
fn the_search_finds_what_is_there_whatever_the_prefix() {
    assert!(tests_support::found("aaaab", b"xxaaaaaaab"));
    assert!(tests_support::found("needle", b"needle"));
    assert!(!tests_support::found("needle", b"needl"));
    assert!(!tests_support::found("aaab", b"aaaaaaaaaa"));
    assert!(tests_support::found("abc", b"zabc"));
}

#[test]
fn an_unreadable_overlay_stream_is_its_own_error() {
    let mut doc = marked(Spec::default());
    let mut snap = Snapshot::capture(&doc, None).unwrap();
    let mut dict = Dict::new();
    dict.set(b"Filter".to_vec(), Object::Name(b"FlateDecode".to_vec()));
    let broken = doc.add(Object::Stream(Stream::from_raw(dict, b"not deflate data".to_vec())));
    let err = snap.set_overlay_streams(&doc, &[broken]).unwrap_err();
    assert_eq!(err, RedactError::OverlayUnreadable);
    assert!(!err.to_string().contains("page 0"), "{err}");
}

// ── Compressed copies inside binary streams ──────────────────────────────────────────────────

/// A stream object whose stored bytes are the Flate encoding of `data` (as `Stream::flate`
/// writes them), with the extra dictionary entries `dict`.
fn flate_object(dict: &str, data: &[u8]) -> Vec<u8> {
    stream(&format!("{dict} /Filter /FlateDecode"), &Stream::flate(Dict::new(), data).raw)
}

/// A one-page file whose resources point at a fake font: /F2 is a /TrueType font whose
/// /FontDescriptor carries `font_file` as its /FontFile2 (objects 7, 8 and 9). The font carries
/// `/Widths` (object 6) so the interpreter resolves it; nothing on the page draws with it.
fn font_file_spec(font_file: Vec<u8>) -> Spec<'static> {
    Spec {
        resources: "/Font << /F1 5 0 R /F2 7 0 R >>",
        extra: vec![
            b"<< /Type /Font /Subtype /TrueType /BaseFont /X /FirstChar 32 /LastChar 126 /Widths 6 0 R /FontDescriptor 8 0 R >>".to_vec(),
            b"<< /Type /FontDescriptor /FontName /X /Flags 4 /FontFile2 9 0 R >>".to_vec(),
            font_file,
        ],
        ..Spec::default()
    }
}

#[test]
fn a_compressed_copy_in_a_fake_font_file_is_found() {
    // The visible copy is redacted normally, but a Flate-compressed copy of the secret hides in
    // a fake /FontFile2. /Length1 makes the sweep class the stream as a binary font program, so
    // its stored bytes hold no plaintext: only the decoded data gives the copy away.
    let spec = font_file_spec(flate_object("/Length1 900", SECRET.as_bytes()));
    let mut doc = marked(spec);
    let (r, proof) = run(&mut doc);
    assert!(residue(&r), "apply refuses to ship a survivor: {}", proof.to_text());
    assert_eq!(surface(&proof, Surface::DecodedStreams).verdict, Verdict::Survivor, "{}", proof.to_text());
    let s = surface(&proof, Surface::DecodedStreams);
    assert!(s.locations.iter().any(|l| l.contains("(decoded)")), "{:?}", s.locations);
    assert!(!proof.passed());
}

#[test]
fn a_legitimate_font_file_is_swept_and_clean() {
    // A font program that holds none of what was removed (as real ones do not) is searched and
    // lets the redaction pass: no false survivor.
    let spec = font_file_spec(flate_object("/Length1 900", b"\x00\x01\x00\x00\x00\x10glyph tables"));
    let mut doc = marked(spec);
    let (r, proof) = run(&mut doc);
    r.unwrap();
    assert!(proof.passed(), "{}", proof.to_text());
    assert_eq!(surface(&proof, Surface::DecodedStreams).verdict, Verdict::Clean, "{}", proof.to_text());
}

#[test]
fn a_font_program_that_will_not_decode_is_searched_as_stored_bytes() {
    // An unrelated, damaged font program the page reaches is not a reason to refuse: nothing
    // redacted touched it, so its stored bytes are searched and it is counted as raw only.
    let raw = Stream::flate(Dict::new(), b"a font program without the secret").raw.as_ref().to_vec();
    let cut = raw.len() - 8;
    let spec = font_file_spec(stream("/Length1 900 /Filter /FlateDecode", &raw[..cut]));
    let mut doc = marked(spec);
    let (r, proof) = run(&mut doc);
    r.unwrap();
    assert!(proof.passed(), "{}", proof.to_text());
    let s = surface(&proof, Surface::DecodedStreams);
    assert_eq!((s.verdict, s.raw_only), (Verdict::Clean, 1), "{}", proof.to_text());
}

/// The standard page plus an image (object 7) drawn away from the secret.
fn image_doc(image: Vec<u8>) -> Document {
    let mut content = CONTENT.to_vec();
    content.extend_from_slice(b" q 20 0 0 20 200 200 cm /Im1 Do Q");
    marked_content(&content, Spec { resources: "/XObject << /Im1 7 0 R >>", extra: vec![image], ..Spec::default() })
}

const IMG: &str = "/Type /XObject /Subtype /Image /Width 20 /Height 20 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode";

fn assert_redacted_cleanly(mut doc: Document) -> Proof {
    let (r, proof) = run(&mut doc);
    r.unwrap();
    assert!(proof.passed(), "{}", proof.to_text());
    let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
    assert!(!saved.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()), "the secret is gone from the file");
    proof
}

#[test]
fn an_unrelated_truncated_flate_image_does_not_fail_the_proof() {
    let raw = Stream::flate(Dict::new(), &[0x80u8; 400]).raw.as_ref().to_vec();
    let proof = assert_redacted_cleanly(image_doc(stream(IMG, &raw[..raw.len() - 8])));
    assert_eq!(surface(&proof, Surface::DecodedStreams).raw_only, 1, "{}", proof.to_text());
}

#[test]
fn an_unrelated_image_over_the_stream_cap_does_not_fail_the_proof() {
    // Highly compressible: stored in a few dozen KiB, decoded past MAX_STREAM.
    let big = vec![0u8; crate::limits::MAX_STREAM + (6 << 20)];
    let raw = Stream::flate(Dict::new(), &big).raw.as_ref().to_vec();
    drop(big);
    let proof = assert_redacted_cleanly(image_doc(stream(IMG, &raw)));
    assert_eq!(surface(&proof, Surface::DecodedStreams).raw_only, 1, "{}", proof.to_text());
}

#[test]
fn an_unrelated_truncated_font_program_does_not_fail_the_redaction() {
    let raw = Stream::flate(Dict::new(), b"glyph tables").raw.as_ref().to_vec();
    let doc = marked(font_file_spec(stream("/Length1 900 /Filter /FlateDecode", &raw[..raw.len() - 6])));
    assert_redacted_cleanly(doc);
}

#[test]
fn a_secret_in_the_stored_bytes_of_a_font_program_is_still_found() {
    // The font program cannot be decoded (it claims Flate but holds plain bytes), so only its
    // stored bytes are searched: the copy of the secret in them is a survivor.
    let spec = font_file_spec(stream("/Length1 900 /Filter /FlateDecode", format!("not deflate {SECRET} not deflate").as_bytes()));
    let mut doc = marked(spec);
    let (r, proof) = run(&mut doc);
    assert!(residue(&r), "{r:?}: {}", proof.to_text());
    let s = surface(&proof, Surface::DecodedStreams);
    assert_eq!((s.verdict, s.raw_only), (Verdict::Survivor, 1), "{}", proof.to_text());
    assert!(s.locations.iter().any(|l| l.contains("(raw)")), "{:?}", s.locations);
    assert!(!proof.to_text().contains(SECRET));
    assert!(!proof.passed());
}

/// The standard page plus an /Indexed colour space whose lookup table is object 7.
fn indexed_doc(lookup: Vec<u8>) -> Document {
    marked_content(CONTENT, Spec { resources: "/ColorSpace << /CS1 [/Indexed /DeviceRGB 255 7 0 R] >>", extra: vec![lookup], ..Spec::default() })
}

#[test]
fn an_indexed_lookup_stream_is_searched_as_stored_bytes_only() {
    // Unrelated and damaged: nothing redacted touched it, so it must not fail the proof.
    let raw = Stream::flate(Dict::new(), &[0x40u8; 300]).raw.as_ref().to_vec();
    let proof = assert_redacted_cleanly(indexed_doc(stream("/Filter /FlateDecode", &raw[..raw.len() - 8])));
    assert_eq!(surface(&proof, Surface::DecodedStreams).raw_only, 1, "{}", proof.to_text());
    // A secret stored raw inside one that will not decode is still found.
    let mut doc = indexed_doc(stream("/Filter /FlateDecode", format!("not deflate {SECRET} not deflate").as_bytes()));
    let (r, proof) = run(&mut doc);
    assert!(residue(&r), "{r:?}: {}", proof.to_text());
    let s = surface(&proof, Surface::DecodedStreams);
    assert_eq!((s.verdict, s.raw_only), (Verdict::Survivor, 1), "{}", proof.to_text());
    assert!(!proof.passed());
}

/// Capture a sound document, then damage the stream `num` (a Flate stream cut short, with the
/// extra dictionary entries `dict`) and prove it.
fn prove_damaged(mut doc: Document, num: u32, dict: &str, data: &[u8]) -> Proof {
    let snap = Snapshot::capture(&doc, None).unwrap();
    let raw = Stream::flate(Dict::new(), data).raw.as_ref().to_vec();
    let cut = raw[..raw.len() - 8].to_vec();
    let broken = open(pdf_bytes(vec![b"<< /Type /Catalog >>".to_vec(), stream(&format!("{dict} /Filter /FlateDecode"), &cut)], ""));
    let obj = broken.get(ObjRef::new(2, 0)).as_ref().clone();
    doc.set(ObjRef::new(num, 0), obj);
    doc.require_full_save();
    snap.prove(&doc, &ProofOptions::default())
}

#[test]
fn damaged_content_appearance_and_metadata_still_fail_closed() {
    // A truncated second content stream.
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents [4 0 R 7 0 R] /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", CONTENT),
        FONT.as_bytes().to_vec(),
        format!("[{}]", vec!["500"; 95].join(" ")).into_bytes(),
        stream("", b"BT /F1 10 Tf 10 200 Td (more text here) Tj ET"),
    ];
    let mut doc = open(pdf_bytes(objs, "/Root 1 0 R"));
    mark(&mut doc, &[REGION]);
    let proof = prove_damaged(doc, 7, "", b"BT /F1 10 Tf 10 200 Td (more text here) Tj ET");
    assert!(!proof.passed());
    assert!(!surface(&proof, Surface::ExtractedText).problems.is_empty(), "{}", proof.to_text());
    assert!(!surface(&proof, Surface::DecodedStreams).problems.is_empty(), "{}", proof.to_text());

    // A truncated appearance stream.
    let annot = b"<< /Type /Annot /Subtype /Square /Rect [200 200 250 250] /AP << /N 8 0 R >> >>".to_vec();
    let ap = stream("/Type /XObject /Subtype /Form /BBox [0 0 50 50]", b"0 0 50 50 re f 0 0 1 rg 1 0 0 RG 2 w");
    let mut doc = marked(Spec { page: "/Annots [7 0 R]", extra: vec![annot, ap], ..Spec::default() });
    doc.require_full_save();
    let proof = prove_damaged(doc, 8, "/Type /XObject /Subtype /Form /BBox [0 0 50 50]", b"0 0 50 50 re f 0 0 1 rg 1 0 0 RG 2 w");
    assert!(!proof.passed());
    assert!(!surface(&proof, Surface::ExtractedText).problems.is_empty(), "{}", proof.to_text());

    // A truncated XMP packet.
    let xmp = stream("/Type /Metadata /Subtype /XML", b"<x:xmpmeta>some metadata of the file</x:xmpmeta>");
    let doc = marked(Spec { catalog: "/Metadata 7 0 R", extra: vec![xmp], ..Spec::default() });
    let proof = prove_damaged(doc, 7, "/Type /Metadata /Subtype /XML", b"<x:xmpmeta>some metadata of the file</x:xmpmeta>");
    assert!(!proof.passed());
    assert!(!surface(&proof, Surface::Xmp).problems.is_empty(), "{}", proof.to_text());
}

#[test]
fn a_null_entry_in_the_contents_array_is_an_empty_piece() {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents [4 0 R null] /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", CONTENT),
        FONT.as_bytes().to_vec(),
        format!("[{}]", vec!["500"; 95].join(" ")).into_bytes(),
    ];
    let mut doc = open(pdf_bytes(objs, "/Root 1 0 R"));
    mark(&mut doc, &[REGION]);
    assert_redacted_cleanly(doc);
}
