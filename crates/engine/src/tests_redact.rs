//! Redaction in the session: history purge, refusals, transactional apply, safe saving.

use std::sync::Arc;

use super::tests::fixture;
use super::*;

fn session_with(bytes: Vec<u8>) -> (Session, DocId) {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("fixture.pdf", None, Arc::new(bytes), None).expect("opens");
    (s, id)
}

/// A redact mark over `rect` (user space) on `page`.
fn mark(page: usize, rect: [f64; 4]) -> Edit {
    let shape = Shape::Redact { quads: vec![rect_quad(rect)], overlay: String::new(), look: Default::default() };
    Edit::AddAnnotation(NewAnnotation { page, style: Style::default_for(&shape), shape, contents: String::new(), author: "Ada".into() })
}

/// "Page 1" at 24 pt from x 20: the "1" starts near x 82.7 (Helvetica widths).
const ONE: [f64; 4] = [80.0, 140.0, 100.0, 180.0];

/// The font of the fixtures: Helvetica with the widths of "Page 1" (and a flat 556 elsewhere),
/// since redaction won't place a standard font whose widths it would have to guess.
const FONT: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 122 /Widths [278 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 667 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556] >>";

/// A PDF from object bodies (object `n + 1` is `objs[n]`), with `/Root 1 0 R`.
fn pdf(objs: &[&str]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn text_stream(body: &str) -> String {
    format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len())
}

/// Two pages; page 2's /Contents is not a stream, so redacting it fails.
fn second_page_unreadable() -> Vec<u8> {
    let broken = "42";
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [4 0 R 6 0 R] /Count 2 /MediaBox [0 0 200 300] >>",
        FONT,
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>",
        &text_stream("BT /F1 24 Tf 20 150 Td (Page 1) Tj ET"),
        "<< /Type /Page /Parent 2 0 R /Contents 7 0 R /Resources << /Font << /F1 3 0 R >> >> >>",
        broken,
    ])
}

fn with_xfa() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [] /XFA [(template) 5 0 R] >> >>",
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
        FONT,
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F1 3 0 R >> >> >>",
        &text_stream("<form/>"),
        &text_stream("BT /F1 24 Tf 20 150 Td (Page 1) Tj ET"),
    ])
}

fn text_of(s: &Session, id: DocId, page: usize) -> String {
    let doc = s.get(id).unwrap();
    let config = pdfcraft_render::RenderConfig { password: doc.password.as_deref().map(Arc::from), ..Default::default() };
    let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
    let req = pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() };
    r.render(req).text.map(|t| t.plain_text().trim().to_string()).unwrap_or_default()
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack.windows(needle.len()).filter(|w| *w == needle).count()
}

#[test]
fn applying_redactions_purges_undo_and_redo() {
    let (mut s, id) = session_with(fixture(2));
    s.apply(id, mark(0, ONE)).unwrap();
    s.apply(id, mark(1, ONE)).unwrap();
    s.undo(id).unwrap();
    assert!(s.get(id).unwrap().can_redo().is_some());
    s.apply(id, mark(1, ONE)).unwrap();
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    let d = s.get(id).unwrap();
    assert_eq!((d.can_undo(), d.can_redo()), (None, None), "no state from before the redaction is kept");
    assert!(d.has_unsaved_redaction() && d.dirty);
    assert_eq!(s.undo(id), Err(EditError::NothingToUndo));
    assert_eq!(s.redo(id), Err(EditError::NothingToRedo));
    assert_eq!(text_of(&s, id, 0), "Page");
    // Edits after the redaction are undoable again, but only back to the redacted state.
    s.apply(id, mark(0, ONE)).unwrap();
    s.undo(id).unwrap();
    assert_eq!(text_of(&s, id, 0), "Page");
}

#[test]
fn a_batch_that_applies_redactions_purges_too() {
    let (mut s, id) = session_with(fixture(1));
    s.apply(id, mark(0, ONE)).unwrap();
    s.apply(
        id,
        Edit::Batch {
            label: "Redact and rotate".into(),
            edits: vec![Edit::ApplyRedactions { pages: None }, Edit::RotatePages { pages: vec![0], degrees: 90 }],
        },
    )
    .unwrap();
    assert_eq!(s.get(id).unwrap().can_undo(), None);
}

#[test]
fn the_report_says_what_was_removed() {
    let (mut s, id) = session_with(fixture(1));
    s.apply(id, mark(0, ONE)).unwrap();
    assert_eq!(s.apply_reporting(id, Edit::RotatePages { pages: vec![0], degrees: 90 }), Ok(None));
    let report = s.apply_reporting(id, Edit::ApplyRedactions { pages: None }).unwrap().expect("a report").report;
    assert_eq!((report.marks, report.pages), (1, 1));
    assert!(report.glyphs > 0);
}

#[test]
fn signed_documents_refuse_redaction_without_changing() {
    let p12 = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sign/tests/data/ec-p256.p12")).unwrap();
    let digital_id = sign::pkcs12::open(&p12, "test").unwrap();
    let mut s = Session::new().with_clock(|| 1_800_000_000);
    let id = s.open("fixture.pdf", None, Arc::new(fixture(2)), None).unwrap();
    let opts = SignOptions { page: 1, rect: Some([20.0, 20.0, 180.0, 60.0]), ..SignOptions::default() };
    let signed = s.sign(id, &digital_id, opts).unwrap();
    s.mark_signed(id, signed, None).unwrap();
    s.apply(id, mark(0, ONE)).unwrap();
    let (bytes, undo) = (s.get(id).unwrap().bytes.clone(), s.get(id).unwrap().can_undo().map(str::to_owned));
    assert_eq!(s.apply(id, Edit::ApplyRedactions { pages: None }), Err(EditError::SignedRedaction));
    let batch = Edit::Batch { label: "x".into(), edits: vec![Edit::ApplyRedactions { pages: None }] };
    assert_eq!(s.apply(id, batch), Err(EditError::SignedRedaction));
    let d = s.get(id).unwrap();
    assert_eq!((&d.bytes, d.can_undo().map(str::to_owned), d.redaction_marks(), d.has_unsaved_redaction()), (&bytes, undo, 1, false));
}

/// Unsigned apart from a catalog `/Perms /UR3` usage-rights entry.
fn with_ur3_only() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /Perms << /UR3 5 0 R >> >>",
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
        FONT,
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F1 3 0 R >> >> >>",
        "<< /Type /Sig /Filter /Adobe.PPKLite >>",
        &text_stream("BT /F1 24 Tf 20 150 Td (Page 1) Tj ET"),
    ])
}

#[test]
fn a_document_with_only_usage_rights_is_refused_up_front_for_redaction() {
    let (mut s, id) = session_with(with_ur3_only());
    assert!(!s.get(id).unwrap().is_signed(), "no signature field");
    s.apply(id, mark(0, ONE)).unwrap();
    let (bytes, undo) = (s.get(id).unwrap().bytes.clone(), s.get(id).unwrap().can_undo().map(str::to_owned));
    assert_eq!(s.get(id).unwrap().redaction_refusal(), Some(EditError::SignedRedaction));
    assert_eq!(s.apply(id, Edit::ApplyRedactions { pages: None }), Err(EditError::SignedRedaction));
    let d = s.get(id).unwrap();
    assert_eq!((&d.bytes, d.can_undo().map(str::to_owned), d.has_unsaved_redaction()), (&bytes, undo, false));
    // Ordinary saves of the same document are unaffected.
    assert!(s.save_full_bytes(id).is_ok(), "Save As");
    assert!(s.save_bytes(id).is_ok(), "Save");
}

#[test]
fn xfa_documents_are_redacted_with_the_xfa_data_sanitized_away() {
    let (mut s, id) = session_with(with_xfa());
    assert!(s.get(id).unwrap().info.xfa.is_some());
    assert!(s.get(id).unwrap().redaction_refusal().is_none());
    s.apply(id, mark(0, ONE)).unwrap();
    let out = s.apply_reporting(id, Edit::ApplyRedactions { pages: None }).unwrap().expect("an outcome");
    assert!(out.proof.passed(), "{}", out.proof.to_text());
    let d = s.get(id).unwrap();
    assert!(d.info.xfa.is_none(), "the XFA packets were removed");
    assert_eq!(count(&s.save_bytes(id).unwrap(), b"<form/>"), 0);
}

#[test]
fn a_failure_on_a_later_page_leaves_the_document_byte_identical() {
    let (mut s, id) = session_with(second_page_unreadable());
    s.apply(id, mark(0, ONE)).unwrap();
    s.apply(id, mark(1, ONE)).unwrap();
    let (bytes, saved) = (s.get(id).unwrap().bytes.clone(), s.save_bytes(id).unwrap());
    let err = s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap_err();
    assert!(matches!(err, EditError::Redact(pdfcraft_redact::RedactError::Unreadable(2))), "{err:?}");
    let d = s.get(id).unwrap();
    assert_eq!(d.bytes, bytes, "the working file is untouched");
    assert_eq!(s.save_bytes(id).unwrap(), saved, "and so is what Save would write");
    assert_eq!((d.redaction_marks(), d.can_undo(), d.has_unsaved_redaction()), (2, Some("Add redaction mark"), false));
    assert_eq!(text_of(&s, id, 0), "Page 1", "page 1 was not redacted on its own");
    // Redacting only the readable page isn't enough either: the proof sweeps the whole file and
    // can't read page 2, so it won't vouch for the output. The error text has no page content.
    let err = s.apply(id, Edit::ApplyRedactions { pages: Some(vec![0]) }).unwrap_err();
    assert!(matches!(err, EditError::Redact(pdfcraft_redact::RedactError::ProofIncomplete { .. })), "{err:?}");
    assert_eq!(s.get(id).unwrap().bytes, bytes);
    assert_eq!(text_of(&s, id, 0), "Page 1");
    assert!(!err.to_string().contains("Page 1"));
}

#[test]
fn the_saved_output_is_a_full_rewrite_without_an_earlier_revision() {
    let (mut s, id) = session_with(fixture(2));
    // An earlier saved revision (a comment) makes the file multi-revision before redacting.
    s.apply(id, mark(0, ONE)).unwrap();
    let first = s.save_bytes(id).unwrap();
    s.mark_saved(id, first, None).unwrap();
    s.apply(id, mark(1, ONE)).unwrap();
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    for bytes in [s.save_bytes(id).unwrap(), s.get(id).unwrap().bytes.clone()] {
        assert_eq!(count(&bytes, b"%%EOF"), 1, "one revision only");
        assert_eq!(count(&bytes, b"(Page 1)") + count(&bytes, b"(Page 2)"), 0, "the removed text is nowhere in the file");
        let mut s2 = Session::new();
        let id2 = s2.open("r.pdf", None, bytes, None).unwrap();
        assert_eq!(s2.get(id2).unwrap().revision_ends().len(), 1);
        assert_eq!((text_of(&s2, id2, 0), text_of(&s2, id2, 1)), ("Page".into(), "Page".into()));
    }
}

#[test]
fn saving_clears_the_unsaved_redaction_flag() {
    let (mut s, id) = session_with(fixture(1));
    s.apply(id, mark(0, ONE)).unwrap();
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    let bytes = s.save_bytes(id).unwrap();
    s.mark_saved(id, bytes, Some("/tmp/redacted-copy.pdf".into())).unwrap();
    let d = s.get(id).unwrap();
    assert!(!d.has_unsaved_redaction() && !d.dirty);
    assert_eq!(d.path.as_deref(), Some("/tmp/redacted-copy.pdf"));
}

#[test]
fn verify_save_checks_the_bytes_before_anything_is_written() {
    let (mut s, id) = session_with(fixture(2));
    s.apply(id, mark(0, ONE)).unwrap();
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    let good = s.save_bytes(id).unwrap();
    assert_eq!(s.verify_save(id, &good), Ok(()));
    let truncated = Arc::new(good[..good.len() / 2].to_vec());
    assert!(matches!(s.verify_save(id, &truncated), Err(EditError::SaveCheck(_))));
    let one_page = Arc::new(fixture(1));
    assert!(matches!(s.verify_save(id, &one_page), Err(EditError::SaveCheck(m)) if m.contains("1 page")));
}

fn scratch_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-safe-save-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn private_atomic_writes_replace_the_destination_and_leave_nothing_behind() {
    let dir = scratch_dir("ok");
    let target = dir.join("out.pdf");
    std::fs::write(&target, b"old").unwrap();
    write_private_atomic(&target, b"new contents").unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"new contents");
    let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(leftovers, [std::ffi::OsString::from("out.pdf")], "the temporary file is gone");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn new_files_are_owner_only_and_replaced_files_keep_their_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let dir = scratch_dir("mode");
    let fresh = dir.join("fresh.pdf");
    write_private_atomic(&fresh, b"x").unwrap();
    assert_eq!(mode(&fresh), 0o600, "owner only");
    let shared = dir.join("shared.pdf");
    std::fs::write(&shared, b"old").unwrap();
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o640)).unwrap();
    write_private_atomic(&shared, b"new").unwrap();
    assert_eq!((std::fs::read(&shared).unwrap(), mode(&shared)), (b"new".to_vec(), 0o640), "the owner's choice is kept");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_destination_is_refused_and_left_alone() {
    let dir = scratch_dir("link");
    let real = dir.join("real.pdf");
    std::fs::write(&real, b"original").unwrap();
    let link = dir.join("link.pdf");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(write_private_atomic(&link, b"new").is_err());
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link is still a link");
    assert_eq!(std::fs::read(&real).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2, "no temporary file left");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_private_write_keeps_the_destination() {
    let dir = scratch_dir("fail");
    // The destination is a directory, so the final rename fails.
    let target = dir.join("out.pdf");
    std::fs::create_dir(&target).unwrap();
    assert!(write_private_atomic(&target, b"new").is_err());
    assert!(target.is_dir(), "the destination was not removed");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary file left");
    // A missing directory fails without creating anything.
    assert!(write_private_atomic(&dir.join("missing").join("out.pdf"), b"x").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// One page with a thumbnail, document information and XMP that all repeat "Page 1".
fn with_leaks() -> Vec<u8> {
    let xmp = "<x:xmpmeta><dc:title>Leaky Page 1 title</dc:title></x:xmpmeta>";
    let bytes = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /Metadata 8 0 R >>",
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
        FONT,
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Thumb 6 0 R /Resources << /Font << /F1 3 0 R >> >> >>",
        &text_stream("BT /F1 24 Tf 20 150 Td (Page 1) Tj ET"),
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 11 >>\nstream\nTHUMBPIXELS\nendstream",
        "<< /Title (Leaky Page 1 title) /Author (Ada) >>",
        &format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{xmp}\nendstream", xmp.len()),
    ]);
    // The trailer follows the cross-reference table, so naming /Info there moves no offsets.
    String::from_utf8(bytes).unwrap().replace("/Root 1 0 R", "/Root 1 0 R /Info 7 0 R").into_bytes()
}

#[test]
fn applying_in_the_session_sanitizes_and_proves() {
    let (mut s, id) = session_with(with_leaks());
    s.apply(id, mark(0, ONE)).unwrap();
    let out = s.apply_reporting(id, Edit::ApplyRedactions { pages: None }).unwrap().expect("an outcome");
    assert!(out.proof.passed() && !out.proof.entries.is_empty(), "{}", out.proof.to_text());
    assert!(out.report.sanitized.iter().any(|(h, n)| *h == Hidden::Metadata && *n > 0), "{:?}", out.report.sanitized);
    // The last outcome stays with the document, for the interface.
    assert_eq!(s.get(id).unwrap().last_redaction(), Some(&out));
    for bytes in [s.save_bytes(id).unwrap(), s.get(id).unwrap().bytes.clone()] {
        for gone in [&b"Leaky"[..], b"THUMBPIXELS", b"(Page 1)"] {
            assert_eq!(count(&bytes, gone), 0, "{}", String::from_utf8_lossy(gone));
        }
    }
}

#[test]
fn a_failed_proof_fails_the_edit_and_changes_nothing() {
    // A stream nobody can decode can't be swept: the proof is incomplete.
    let junk = "<< /Filter /NoSuchDecode /Length 8 >>\nstream\n\u{1}\u{2} junk\nendstream";
    let bytes = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
        FONT,
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Extra 6 0 R /Resources << /Font << /F1 3 0 R >> >> >>",
        &text_stream("BT /F1 24 Tf 20 150 Td (Page 1) Tj ET"),
        junk,
    ]);
    let (mut s, id) = session_with(bytes);
    s.apply(id, mark(0, ONE)).unwrap();
    let (working, saved) = (s.get(id).unwrap().bytes.clone(), s.save_bytes(id).unwrap());
    let err = s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap_err();
    assert!(matches!(err, EditError::Redact(pdfcraft_redact::RedactError::ProofIncomplete { .. })), "{err:?}");
    assert!(!err.to_string().contains("Page 1"));
    let d = s.get(id).unwrap();
    assert_eq!(d.bytes, working);
    assert_eq!(s.save_bytes(id).unwrap(), saved);
    assert_eq!((d.redaction_marks(), d.can_undo(), d.has_unsaved_redaction()), (1, Some("Add redaction mark"), false));
    assert!(d.last_redaction().is_none());
    assert_eq!(text_of(&s, id, 0), "Page 1");
}

#[test]
fn revert_and_later_edits_drop_the_stale_redaction_claims() {
    let (mut s, id) = session_with(fixture(1));
    s.apply(id, mark(0, ONE)).unwrap();
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    assert!(s.get(id).unwrap().last_redaction().is_some() && s.get(id).unwrap().has_unsaved_redaction());
    // Any later edit makes the proof a statement about an earlier document.
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    assert!(s.get(id).unwrap().last_redaction().is_none());
    assert!(s.get(id).unwrap().has_unsaved_redaction(), "the redaction itself is still unsaved");
    s.undo(id).unwrap();
    assert!(s.get(id).unwrap().last_redaction().is_none());
    // Revert goes back to the file as opened: nothing is redacted, nothing is claimed.
    s.apply(id, mark(0, ONE)).unwrap();
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    assert!(s.get(id).unwrap().last_redaction().is_some());
    s.revert(id).unwrap();
    let d = s.get(id).unwrap();
    assert!(d.last_redaction().is_none() && !d.has_unsaved_redaction());
    assert_eq!(text_of(&s, id, 0), "Page 1");
}

#[test]
fn sanitizing_needs_a_new_name_like_a_redaction() {
    let (mut s, id) = session_with(with_leaks());
    assert!(!s.get(id).unwrap().has_unsaved_redaction());
    s.apply(id, Edit::Sanitize).unwrap();
    assert!(s.get(id).unwrap().has_unsaved_redaction());
    s.revert(id).unwrap();
    s.apply(id, Edit::RemoveHidden { which: vec![Hidden::Metadata] }).unwrap();
    assert!(s.get(id).unwrap().has_unsaved_redaction());
    let (mut s, id) = session_with(with_leaks());
    s.apply(id, Edit::Batch { label: "x".into(), edits: vec![Edit::Sanitize] }).unwrap();
    assert!(s.get(id).unwrap().has_unsaved_redaction());
}

/// An unsigned document whose /AcroForm/Fields can't be read (a field that is its own kid).
fn with_broken_form() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>",
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
        FONT,
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F1 3 0 R >> >> >>",
        "<< /FT /Tx /T (a) /Kids [5 0 R] >>",
        &text_stream("BT /F1 24 Tf 20 150 Td (Page 1) Tj ET"),
    ])
}

#[test]
fn a_malformed_form_does_not_make_ordinary_rewrites_look_signed() {
    let (mut s, id) = session_with(with_broken_form());
    assert!(!s.get(id).unwrap().is_signed());
    assert!(s.save_full_bytes(id).is_ok(), "Save As / Optimize");
    assert!(s.reduced_bytes(id).is_ok());
    let src = (String::from("a.pdf"), Arc::new(with_broken_form()));
    assert!(s.combine(&[src.clone(), src.clone()]).is_ok(), "Combine");
    assert!(s.split(id, &pdfcraft_organize::SplitBy::PageCount(1)).is_ok(), "Split");
    assert!(s.extract(id, &[0]).is_ok(), "Extract");
    // Redaction is the one operation that must not guess: it refuses a form it can't read.
    s.apply(id, mark(0, ONE)).unwrap();
    assert!(s.apply(id, Edit::ApplyRedactions { pages: None }).is_err());
}

#[test]
fn accented_text_in_a_created_document_can_be_redacted() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let bytes = s.create_from_text("notes", "Señor café ñandú").unwrap();
    let id = s.open("created.pdf", None, bytes, None).unwrap();
    assert_eq!(text_of(&s, id, 0), "Señor café ñandú");
    // The widths of é and ñ are in the font, so the glyphs can be placed and removed exactly.
    s.apply(id, mark(0, [0.0, 0.0, 612.0, 792.0])).unwrap();
    s.apply(id, Edit::ApplyRedactions { pages: None }).expect("accented text is redactable");
    assert_eq!(text_of(&s, id, 0), "");
}

#[test]
fn verify_save_sweeps_the_bytes_against_the_redaction_snapshot() {
    let original = fixture(1);
    let (mut s, id) = session_with(original.clone());
    s.apply(id, mark(0, ONE)).unwrap();
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    let good = s.save_bytes(id).unwrap();
    assert_eq!(s.verify_save(id, &good), Ok(()));
    // The unredacted file has the page count but still holds the removed text: refused, and the
    // message names counts only.
    let err = s.verify_save(id, &Arc::new(original)).unwrap_err();
    assert!(matches!(&err, EditError::SaveCheck(m) if m.contains("survivor") && !m.contains("Page")), "{err:?}");
    // After a later edit the snapshot is dropped and only reopening is checked.
    s.apply(id, Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    assert_eq!(s.verify_save(id, &good), Ok(()));
}

#[test]
fn the_redaction_snapshot_is_gone_after_a_successful_save() {
    let (mut s, id) = session_with(fixture(1));
    s.apply(id, mark(0, ONE)).unwrap();
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    assert!(s.get(id).unwrap().redaction_snapshot.is_some());
    let bytes = s.save_bytes(id).unwrap();
    assert_eq!(s.verify_save(id, &bytes), Ok(()));
    assert!(s.get(id).unwrap().redaction_snapshot.is_some(), "verify_save only borrows it");
    s.mark_saved(id, bytes, None).unwrap();
    assert!(s.get(id).unwrap().redaction_snapshot.is_none());
}

#[test]
fn debug_output_of_an_outcome_never_prints_layer_names() {
    let (mut s, id) = session_with(fixture(1));
    s.apply(id, mark(0, ONE)).unwrap();
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    let mut outcome = s.get(id).unwrap().last_redaction().unwrap().clone();
    outcome.report.layers = vec!["Confidential Draft Layer".into(), "Salaries".into()];
    let shown = format!("{outcome:?} {outcome:#?}");
    assert!(!shown.contains("Confidential") && !shown.contains("Salaries"), "{shown}");
    assert!(shown.contains("2 layer(s)"), "{shown}");
}
