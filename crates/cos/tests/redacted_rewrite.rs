//! What a full rewrite after redaction must guarantee: a new file identity, one revision and no
//! leftovers, the signed-document guard (fail closed), bounded decoding, and encryption kept.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use pdfcraft_cos::{Algorithm, CosError, Dict, Document, NewEncryption, ObjRef, Object, SaveOptions, Stream, write_full, write_incremental};

fn count(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

/// Classic one-revision file from object bodies (object i+1 = bodies[i]).
fn classic(bodies: &[String], trailer: &str) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in bodies.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R {trailer} >>\nstartxref\n{xref}\n%%EOF\n", bodies.len() + 1).as_bytes());
    out
}

fn stream_obj(body: &str) -> String {
    format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len())
}

const SECRET: &str = "TOPSECRET";

/// Catalog 1, Pages 2, Page 3, Contents 4 (holds the secret), Font 5, Info 6, plus `extra`.
fn base(catalog_extra: &str, extra: &[String], trailer: &str) -> Vec<u8> {
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".into(),
        stream_obj(&format!("BT /F1 12 Tf 20 100 Td ({SECRET}) Tj ET")),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Title (Report) >>".into(),
    ];
    objs.extend(extra.iter().cloned());
    classic(&objs, &format!("/Info 6 0 R {trailer}"))
}

fn id_strings(doc: &Document) -> (Vec<u8>, Vec<u8>) {
    let Some(Object::Array(a)) = doc.trailer().get(b"ID") else { return (Vec::new(), Vec::new()) };
    let s = |i: usize| a.get(i).and_then(Object::as_string).map(|s| s.bytes.clone()).unwrap_or_default();
    (s(0), s(1))
}

fn replace_contents(doc: &mut Document, text: &str) {
    doc.set(ObjRef::new(4, 0), Object::Stream(Stream::from_raw(Dict::new(), text.as_bytes().to_vec())));
}

/// Every object number a document can reach from `/Root`, `/Info` and `/Encrypt`.
fn reachable(doc: &Document) -> HashSet<u32> {
    let mut seen = HashSet::new();
    let mut queue: VecDeque<ObjRef> = [b"Root".as_slice(), b"Info", b"Encrypt"].iter().filter_map(|k| doc.trailer().reference(k)).collect();
    while let Some(r) = queue.pop_front() {
        if !seen.insert(r.num) {
            continue;
        }
        let mut refs = Vec::new();
        collect(&doc.get(r), &mut refs);
        queue.extend(refs);
    }
    seen
}

fn collect(o: &Object, out: &mut Vec<ObjRef>) {
    match o {
        Object::Ref(r) => out.push(*r),
        Object::Array(a) => a.iter().for_each(|x| collect(x, out)),
        Object::Dict(d) => d.iter().for_each(|(_, v)| collect(v, out)),
        Object::Stream(s) => s.dict.iter().for_each(|(_, v)| collect(v, out)),
        _ => {}
    }
}

/// All of a document's objects serialized and decoded, as text (what an attacker can recover).
fn everything(doc: &Document) -> String {
    let mut text = String::new();
    for n in doc.object_numbers() {
        match &*doc.get(ObjRef::new(n, 0)) {
            Object::Stream(s) => {
                text.push_str(&String::from_utf8_lossy(&s.decoded().unwrap_or_default()));
                text.push_str(&String::from_utf8_lossy(&s.raw));
            }
            o => {
                let mut b = Vec::new();
                pdfcraft_cos::serialize(o, &mut b);
                text.push_str(&String::from_utf8_lossy(&b));
            }
        }
    }
    text
}

// ── file identity ──────────────────────────────────────────────────────────────────────────────

#[test]
fn a_redacted_rewrite_gets_a_new_file_identity() {
    let original = base("", &[], "/ID [<00112233445566778899AABBCCDDEEFF> <00112233445566778899AABBCCDDEEFF>]");
    let opts = SaveOptions::default();
    // An ordinary full save keeps the identity.
    let plain = Document::open(Arc::new(original.clone())).unwrap();
    let kept = Document::open(Arc::new(write_full(&plain, &opts).unwrap())).unwrap();
    assert_eq!(id_strings(&kept), id_strings(&plain));
    assert_eq!(id_strings(&kept).0.len(), 16);
    // A redacting one does not share either string, and is not repeatable.
    let mut doc = Document::open(Arc::new(original)).unwrap();
    doc.require_full_save_with_new_id();
    assert!(doc.full_save_required() && doc.new_id_required());
    let a = Document::open(Arc::new(write_incremental(&doc, &opts).unwrap())).unwrap();
    let b = Document::open(Arc::new(write_full(&doc, &opts).unwrap())).unwrap();
    let (orig0, orig1) = id_strings(&plain);
    for d in [&a, &b] {
        let (i0, i1) = id_strings(d);
        assert_eq!((i0.len(), i1.len()), (16, 16));
        assert!(i0 != orig0 && i1 != orig1);
    }
    assert_ne!(id_strings(&a), id_strings(&b), "every save is a new identity");
    // Caller entropy is mixed in.
    let with = |e| write_full(&doc, &SaveOptions { id_entropy: Some([e; 16]), ..opts.clone() }).unwrap();
    assert_ne!(id_strings(&Document::open(Arc::new(with(1))).unwrap()), id_strings(&Document::open(Arc::new(with(2))).unwrap()));
}

#[test]
fn a_document_without_an_id_still_gets_one_when_redacted() {
    let mut doc = Document::open(Arc::new(base("", &[], ""))).unwrap();
    doc.require_full_save_with_new_id();
    let out = Document::open(Arc::new(write_full(&doc, &SaveOptions::default()).unwrap())).unwrap();
    assert_eq!(id_strings(&out).0.len(), 16);
}

// ── full rewrite guarantees ────────────────────────────────────────────────────────────────────

/// Two-revision file whose first revision holds the secret in the content stream, an orphan object
/// and (in the xref-stream variant) an unreferenced object inside an object stream.
fn layered() -> Vec<u8> {
    let extra = [stream_obj("ORPHANSTREAM"), "<< /Orphan (ORPHANDICT) >>".to_string()];
    let mut bytes = base("", &extra, "/ID [<AA> <AA>]");
    // Revision 2 (incremental): rewrites the content stream — the old bytes stay in revision 1.
    let doc = Document::open(Arc::new(bytes.clone())).unwrap();
    let mut doc = doc;
    replace_contents(&mut doc, "BT ET");
    bytes = write_incremental(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    assert_eq!(count(&bytes, b"%%EOF"), 2);
    assert!(count(&bytes, SECRET.as_bytes()) >= 1);
    bytes
}

#[test]
fn a_full_rewrite_after_redaction_has_one_revision_and_no_orphans() {
    for object_streams in [false, true] {
        let mut doc = Document::open(Arc::new(layered())).unwrap();
        replace_contents(&mut doc, "BT ET");
        doc.require_full_save_with_new_id();
        let out = write_incremental(&doc, &SaveOptions { object_streams, ..SaveOptions::default() }).unwrap();
        assert_eq!(count(&out, b"startxref"), 1, "single cross-reference section (object_streams={object_streams})");
        assert_eq!(count(&out, b"%%EOF"), 1);
        assert_eq!(count(&out, b"/Prev"), 0);
        let back = Document::open(Arc::new(out.clone())).unwrap();
        assert!(back.repair_log().is_empty(), "{:?}", back.repair_log());
        assert_eq!(back.revisions().len(), 1);
        // Everything that is in the file is reachable from the trailer's roots.
        let live = reachable(&back);
        let all: HashSet<u32> = back
            .object_numbers()
            .into_iter()
            .filter(|n| !matches!(&*back.get(ObjRef::new(*n, 0)), Object::Stream(s) if matches!(s.dict.name(b"Type"), Some(b"ObjStm" | b"XRef"))))
            .collect();
        assert_eq!(all, live, "no orphaned objects (object_streams={object_streams})");
        // The old bytes are nowhere, raw or decoded, in or out of object streams.
        let text = everything(&back);
        for old in [SECRET, "ORPHANSTREAM", "ORPHANDICT"] {
            assert!(!text.contains(old) && count(&out, old.as_bytes()) == 0, "{old} survived (object_streams={object_streams})");
        }
    }
}

#[test]
fn unreferenced_objects_inside_an_old_object_stream_are_purged() {
    // Object streams come from a full save: link an object from the catalog so it is packed
    // into one, then unlink it in a later revision and rewrite.
    let mut doc = Document::open(Arc::new(base("", &[], ""))).unwrap();
    let kept = doc.add(Dict::from_iter([(b"Orphan".to_vec(), Object::String(pdfcraft_cos::PdfString::text("INSTREAMORPHAN")))]));
    let root = doc.root().unwrap();
    doc.update_dict(root, |d| d.set(b"Extra".to_vec(), Object::Ref(kept))).unwrap();
    let stage = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(!String::from_utf8_lossy(&stage).contains("INSTREAMORPHAN"), "compressed");
    let mut doc = Document::open(Arc::new(stage)).unwrap();
    assert!(everything(&doc).contains("INSTREAMORPHAN"), "the object lives in an object stream");
    doc.update_dict(root, |d| {
        d.remove(b"Extra");
    })
    .unwrap();
    doc.require_full_save_with_new_id();
    let out = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let back = Document::open(Arc::new(out.clone())).unwrap();
    assert!(!everything(&back).contains("INSTREAMORPHAN"));
    assert_eq!(count(&out, b"%%EOF"), 1);
}

// ── signed documents ───────────────────────────────────────────────────────────────────────────

fn with_form(form: &str, extra: &[&str]) -> Document {
    let extra: Vec<String> = extra.iter().map(|s| (*s).to_string()).collect();
    Document::open(Arc::new(base(&format!("/AcroForm {form}"), &extra, ""))).unwrap()
}

#[test]
fn unsigned_documents_are_not_signed_and_not_xfa() {
    let plain = Document::open(Arc::new(base("", &[], ""))).unwrap();
    assert!(!plain.is_signed() && !plain.has_xfa());
    let empty_sig = with_form("<< /Fields [7 0 R] >>", &["<< /FT /Sig /T (s) /Subtype /Widget >>"]);
    assert!(!empty_sig.is_signed(), "an empty signature field is not a signature");
    let text = with_form("<< /Fields [7 0 R] >>", &["<< /FT /Tx /T (t) /V (hello) >>"]);
    assert!(!text.is_signed() && !text.has_xfa());
}

#[test]
fn a_filled_signature_field_makes_the_document_signed() {
    let sig = "<< /Type /Sig /Filter /Adobe.PPKLite /ByteRange [0 1 2 3] /Contents <00> >>";
    let direct = with_form("<< /Fields [7 0 R] >>", &["<< /FT /Sig /T (s) /V 8 0 R >>", sig]);
    assert!(direct.is_signed());
    // /FT inherited from a parent, value on the kid.
    let nested = with_form("<< /Fields [7 0 R] >>", &["<< /FT /Sig /Kids [8 0 R] >>", "<< /T (k) /V 9 0 R >>", sig]);
    assert!(nested.is_signed());
    assert!(with_form("<< /SigFlags 3 /Fields [] >>", &[]).is_signed(), "SignaturesExist");
    let certified = Document::open(Arc::new(base("/Perms << /DocMDP 7 0 R >>", &["<< /Type /Sig >>".into()], ""))).unwrap();
    assert!(certified.is_signed());
}

#[test]
fn malformed_forms_fail_closed_as_signed() {
    let cases: [(&str, &[&str]); 8] = [
        ("<< /Fields 5 >>", &[]),                                   // /Fields not an array
        ("<< /Fields [42] >>", &[]),                                // entry not a dictionary
        ("<< /Fields [99 0 R] >>", &[]),                            // dangling entry
        ("<< /Fields [7 0 R] >>", &["<< /Kids 3 >>"]),              // /Kids not an array
        ("<< /Fields [7 0 R] >>", &["<< /Kids [7 0 R] /T (x) >>"]), // loop
        ("<< /Fields [7 0 R 7 0 R] >>", &["<< /T (x) >>"]),         // reachable twice
        ("[1 2 3]", &[]),                                           // /AcroForm itself malformed
        ("<< /Fields [null] >>", &[]),                              // null entry
    ];
    for (form, extra) in cases {
        let mut doc = with_form(form, extra);
        assert!(doc.is_signed(), "{form}");
        // Only a rewrite that redaction asked for fails closed; see `signed_guard.rs` for the
        // ordinary rewrites of such a document.
        doc.require_full_save();
        assert!(matches!(write_full(&doc, &SaveOptions::default()), Err(CosError::SignedDocument)), "{form}");
    }
    // A catalog that cannot be read is treated the same way.
    let mut broken = Document::open(Arc::new(base("", &[], ""))).unwrap();
    broken.trailer_mut().set(b"Root".to_vec(), Object::Ref(ObjRef::new(99, 0)));
    assert!(broken.is_signed() && broken.has_xfa());
}

#[test]
fn xfa_is_detected_and_malformed_forms_fail_closed() {
    assert!(with_form("<< /Fields [] /XFA [(preamble) 7 0 R] >>", &[stream_obj("<xfa/>").as_str()]).has_xfa());
    assert!(!with_form("<< /Fields [] >>", &[]).has_xfa());
    assert!(with_form("(not a dict)", &[]).has_xfa());
}

#[test]
fn rewriting_a_signed_document_is_refused_unless_overridden_and_incremental_still_works() {
    let sig = "<< /Type /Sig /ByteRange [0 1 2 3] /Contents <00> >>";
    let mut doc = with_form("<< /Fields [7 0 R] >>", &["<< /FT /Sig /T (s) /V 8 0 R >>", sig]);
    // Incremental saves (what signing and form filling use) are untouched.
    let original = doc.bytes().as_ref().clone();
    let info = doc.trailer().reference(b"Info").unwrap();
    doc.update_dict(info, |d| d.set(b"Title".to_vec(), Object::String(pdfcraft_cos::PdfString::text("T")))).unwrap();
    let appended = write_incremental(&doc, &SaveOptions::default()).unwrap();
    assert_eq!(&appended[..original.len()], &original[..]);
    // A full rewrite, directly or through a redaction flag, is an error by default.
    assert_eq!(write_full(&doc, &SaveOptions::default()), Err(CosError::SignedDocument));
    doc.require_full_save_with_new_id();
    assert_eq!(write_incremental(&doc, &SaveOptions::default()), Err(CosError::SignedDocument));
    let allowed = SaveOptions { allow_signed_rewrite: true, ..SaveOptions::default() };
    assert!(write_full(&doc, &allowed).is_ok());
    assert!(write_incremental(&doc, &allowed).is_ok());
}

// ── bounded decoding ───────────────────────────────────────────────────────────────────────────

#[test]
fn a_decompression_bomb_is_refused_not_expanded() {
    let bomb = Stream::flate(Dict::new(), &vec![0u8; 16 << 20]);
    assert!(bomb.raw.len() < 64 << 10, "{} bytes of compressed zeros", bomb.raw.len());
    assert!(matches!(bomb.decoded_within(1 << 20), Err(CosError::Filter(_))));
    assert_eq!(bomb.decoded_within(32 << 20).map(|v| v.len()), Ok(16 << 20));
    // `decoded()` keeps the hard cap, which this stream stays well below.
    assert_eq!(bomb.decoded().map(|v| v.len()), Ok(16 << 20));
}

// ── encryption ─────────────────────────────────────────────────────────────────────────────────

#[test]
fn a_redacted_encrypted_document_stays_encrypted_and_reopens_with_its_password() {
    for alg in [Algorithm::Rc4_128, Algorithm::Aes128, Algorithm::Aes256] {
        let mut doc = Document::open(Arc::new(base("", &[], ""))).unwrap();
        doc.set_encryption(&NewEncryption {
            algorithm: alg,
            user_password: "user",
            owner_password: "owner",
            permissions: -1,
            encrypt_metadata: true,
            seed: [5; 32],
        })
        .unwrap();
        let protected = write_full(&doc, &SaveOptions::default()).unwrap();
        let before = Document::open_with_password(Arc::new(protected.clone()), Some("user")).unwrap();
        assert!(everything(&before).contains(SECRET));

        // Policy: redaction keeps the document's protection; only the content changes.
        let mut doc = before.clone();
        replace_contents(&mut doc, "BT (REDACTED) Tj ET");
        doc.require_full_save_with_new_id();
        let out = write_incremental(&doc, &SaveOptions::default()).unwrap();
        assert!(!out.starts_with(&protected), "rewritten, not appended");
        assert_eq!((count(&out, b"startxref"), count(&out, b"%%EOF")), (1, 1), "{alg:?}");
        assert!(matches!(Document::open(Arc::new(out.clone())), Err(CosError::NeedsPassword)), "{alg:?}: still encrypted");
        assert_eq!(count(&out, b"REDACTED"), 0, "{alg:?}: content encrypted");
        let back = Document::open_with_password(Arc::new(out), Some("user")).unwrap();
        let text = everything(&back);
        assert!(text.contains("REDACTED") && !text.contains(SECRET), "{alg:?}");
        // The key depends on the first identifier string: it is kept, the second is fresh.
        let (b0, b1) = id_strings(&before);
        let (a0, a1) = id_strings(&back);
        assert_eq!(a0, b0, "{alg:?}");
        assert_ne!(a1, b1, "{alg:?}");
    }
}
