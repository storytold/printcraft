//! Verification reports: separated verdicts for signed PDFs (e-Aadhaar and others).
//!
//! All fixtures are synthetic documents signed in-process with test keys. No real
//! Aadhaar data is used anywhere.

use std::sync::Arc;

use pdfcraft_cos::{Document, Object, PdfString, SaveOptions, write_incremental};
use pdfcraft_sign::verify::{self, CertValidity, Integrity, Revocation, TimeSource, Trust};
use pdfcraft_sign::x509::Name;
use pdfcraft_sign::{Certificate, DigitalId, Modification, PrivateKey, SignOptions, Status, Time, TrustStore, pkcs12, signatures};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// One page with text and an unsigned signature field "Approval".
fn fixture() -> Vec<u8> {
    let objs: Vec<&[u8]> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>",
        b"<< /Length 44 >>\nstream\nBT /F1 14 Tf 20 250 Td (Contract text) Tj ET\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        b"<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Rect [150 20 280 70] /P 3 0 R /F 4 >>",
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn open(b: &[u8]) -> Document {
    Document::open(Arc::new(b.to_vec())).unwrap()
}

fn opts() -> SignOptions {
    SignOptions {
        page: 0,
        rect: Some([20.0, 100.0, 220.0, 150.0]),
        reason: Some("I approve this document".into()),
        location: Some("London".into()),
        date: "D:20261002120000+01'00'".into(),
        ..SignOptions::default()
    }
}

/// Append a revision that changes the document with `f`.
fn edit_after(signed: &[u8], f: impl FnOnce(&mut Document)) -> Vec<u8> {
    let mut doc = open(signed);
    f(&mut doc);
    write_incremental(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap()
}

fn add_comment(doc: &mut Document) {
    let mut d = pdfcraft_cos::Dict::new();
    d.set(b"Type".to_vec(), Object::name("Annot"));
    d.set(b"Subtype".to_vec(), Object::name("Text"));
    d.set(b"Rect".to_vec(), Object::Array(vec![Object::Int(10), Object::Int(10), Object::Int(30), Object::Int(30)]));
    d.set(b"Contents".to_vec(), PdfString::text("A note"));
    let r = doc.add(Object::Dict(d));
    let page = pdfcraft_cos::ObjRef { num: 3, generation: 0 };
    doc.update_dict(page, |p| {
        if let Some(Object::Array(a)) = p.get_mut(b"Annots") {
            a.push(Object::Ref(r));
        }
    })
    .unwrap();
}

fn change_text(doc: &mut Document) {
    let mut d = pdfcraft_cos::Dict::new();
    d.set(b"Length".to_vec(), Object::Int(40));
    let s = pdfcraft_cos::Stream::from_raw(d, b"BT /F1 14 Tf 20 250 Td (Other text) Tj ET".to_vec());
    doc.set(pdfcraft_cos::ObjRef { num: 4, generation: 0 }, Object::Stream(s));
}

/// A self-signed test identity valid from `from_year` for `years`.
fn self_signed_id(cn: &str, from_year: u32, years: u32) -> DigitalId {
    let key = PrivateKey::generate_p256().unwrap();
    let name = Name::build(cn, "", "Verification Tests", "", "IN");
    let from = Time { year: from_year, month: 1, day: 1, hour: 0, minute: 0, second: 0 };
    let cert = Certificate::self_signed(&name, &key, from, years, &[7u8, 8, 9]).unwrap();
    DigitalId { key, certificate: cert, chain: Vec::new(), friendly_name: Some(cn.to_string()) }
}

#[test]
fn intact_but_untrusted_signature_reports_separate_verdicts() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    let doc = open(&signed);
    // Nobody trusted: intact, but untrusted, revocation unknown.
    let reports = verify::verify_doc(&doc, &signed, &TrustStore::default());
    let r = reports.iter().find(|r| r.signed).expect("a signed report");
    assert_eq!(r.status, Status::Unknown);
    assert_eq!(r.integrity, Integrity::Intact);
    assert_eq!(r.trust, Trust::Untrusted);
    assert_eq!(r.cert_validity, CertValidity::Valid);
    assert_eq!(r.revocation, Revocation::Unknown);
    assert!(r.revocation_note.as_deref().unwrap_or("").contains("offline"), "{:?}", r.revocation_note);
    assert_eq!(r.time_source, TimeSource::SignerClaim, "no timestamp token: the signer clock");
    assert!(r.signing_time.is_some());
    assert_eq!(r.modification, Modification::None);
    assert_eq!(r.digest.as_deref(), Some("SHA-256"));
    assert!(!r.next_steps.is_empty(), "an untrusted signature tells the user what to do");
    let summary = verify::summarize(&reports);
    assert_eq!((summary.signed, summary.overall.as_str()), (1, "untrusted"), "{summary:?}");
}

#[test]
fn trusted_signature_reports_valid() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    let doc = open(&signed);
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    let reports = verify::verify_doc(&doc, &signed, &trust);
    let r = reports.iter().find(|r| r.signed).unwrap();
    assert_eq!(r.status, Status::Valid);
    assert_eq!(r.integrity, Integrity::Intact);
    assert!(matches!(&r.trust, Trust::Trusted { via_cca: false, .. }));
    assert_eq!(r.revocation, Revocation::Unknown, "no /DSS embedded: revocation stays unknown");
    let summary = verify::summarize(&reports);
    assert_eq!(summary.overall.as_str(), "valid");
    assert_eq!((summary.valid, summary.revocation_unknown), (1, 1));
}

#[test]
fn chained_signature_reports_trusted_anchor() {
    // chain.p12 holds a leaf plus its issuing CA: trusting the anchor (as an
    // e-Aadhaar chain trusts its CCA root) validates green.
    let id = pkcs12::open(&data("chain.p12"), "test").unwrap();
    assert!(!id.chain.is_empty());
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    let doc = open(&signed);
    let untrusted = verify::verify_doc(&doc, &signed, &TrustStore::default());
    assert_eq!(verify::summarize(&untrusted).overall.as_str(), "untrusted");
    let trust = TrustStore { certs: vec![id.chain[0].clone()] };
    let reports = verify::verify_doc(&doc, &signed, &trust);
    let r = reports.iter().find(|r| r.signed).unwrap();
    assert_eq!(r.status, Status::Valid);
    assert_eq!(r.integrity, Integrity::Intact);
    assert!(matches!(&r.trust, Trust::Trusted { via_cca: false, .. }), "{:?}", r.trust);
    assert_eq!(r.summary, "Signature is valid and the signer is trusted.");
    assert_eq!(verify::summarize(&reports).overall.as_str(), "valid");
}

#[test]
fn tampered_bytes_report_altered_and_invalid() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    let mut tampered = signed.clone();
    let i = tampered.windows(13).position(|w| w == b"Contract text").unwrap();
    tampered[i] = b'K';
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    let reports = verify::verify_doc(&open(&tampered), &tampered, &trust);
    let r = reports.iter().find(|r| r.signed).unwrap();
    assert_eq!(r.status, Status::Invalid);
    assert!(matches!(r.integrity, Integrity::Altered { .. }), "{:?}", r.integrity);
    assert!(r.next_steps.iter().any(|n| n.contains("fresh copy")), "{:?}", r.next_steps);
    let summary = verify::summarize(&reports);
    assert_eq!((summary.overall.as_str(), summary.altered), ("invalid", 1));
}

#[test]
fn disallowed_later_changes_report_altered_while_comments_stay_allowed() {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    let commented = edit_after(&signed, add_comment);
    let r = verify::verify_doc(&open(&commented), &commented, &trust).into_iter().find(|r| r.signed).unwrap();
    assert_eq!(r.status, Status::Valid);
    assert_eq!(r.integrity, Integrity::Intact);
    assert_eq!(r.modification, Modification::Allowed(vec!["comments".into()]));
    let rewritten = edit_after(&signed, change_text);
    let r = verify::verify_doc(&open(&rewritten), &rewritten, &trust).into_iter().find(|r| r.signed).unwrap();
    assert_eq!(r.status, Status::Invalid);
    assert!(matches!(r.integrity, Integrity::Altered { .. }));
    assert_eq!(r.modification, Modification::Disallowed(vec!["page content".into()]));
    let summary = verify::summarize(&verify::verify_doc(&open(&rewritten), &rewritten, &trust));
    assert_eq!((summary.overall.as_str(), summary.modified_disallowed), ("invalid", 1));
}

#[test]
fn unsigned_document_reports_no_signatures() {
    let bytes = fixture();
    let reports = verify::verify_doc(&open(&bytes), &bytes, &TrustStore::default());
    assert_eq!(reports.len(), 1, "the empty Approval field is listed");
    assert!(!reports[0].signed);
    assert_eq!(reports[0].integrity, Integrity::NotApplicable);
    let summary = verify::summarize(&reports);
    assert_eq!((summary.signed, summary.overall.as_str()), (0, "no-signatures"));
    // An ordinary unsigned PDF still opens and works normally.
    assert_eq!(verify::summarize(&[]).overall.as_str(), "no-signatures");
}

#[test]
fn counter_signed_document_reports_each_signature() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let first = pdfcraft_sign::sign(&open(&fixture()), &id, &SignOptions { field: Some("Approval".into()), ..opts() }).unwrap();
    let id2 = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let second = pdfcraft_sign::sign(&open(&first), &id2, &opts()).unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone(), id2.certificate.clone()] };
    let reports = verify::verify_doc(&open(&second), &second, &trust);
    let signed: Vec<_> = reports.iter().filter(|r| r.signed).collect();
    assert_eq!(signed.len(), 2);
    let approval = signed.iter().find(|r| r.field == "Approval").unwrap();
    assert_eq!(approval.modification, Modification::Allowed(vec!["signature".into()]));
    assert!(signed.iter().all(|r| r.status == Status::Valid), "{reports:?}");
    assert_eq!(verify::summarize(&reports).overall.as_str(), "valid");
}

#[test]
fn expired_signer_certificate_is_reported_not_collapsed() {
    // Valid 2020-2021, signed in 2026: intact and trusted, but not valid then.
    let id = self_signed_id("Expired Signer", 2020, 1);
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &SignOptions { date: "D:20260601000000Z".into(), ..opts() }).unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    let reports = verify::verify_doc(&open(&signed), &signed, &trust);
    let r = reports.iter().find(|r| r.signed).unwrap();
    assert_eq!(r.integrity, Integrity::Intact);
    assert!(matches!(&r.trust, Trust::Trusted { .. }), "{:?}", r.trust);
    assert!(matches!(&r.cert_validity, CertValidity::NotValidAtSigning { .. }), "{:?}", r.cert_validity);
    let summary = verify::summarize(&reports);
    assert_eq!((summary.overall.as_str(), summary.expired_or_invalid_cert), ("expired-or-invalid-cert", 1));
}

#[test]
fn subject_name_spoof_does_not_gain_cca_trust() {
    // An attacker's self-signed certificate wearing the CCA subject name.
    let spoof = self_signed_id("CCA India 2022", 2025, 10);
    assert!(spoof.certificate.subject.display().contains("CCA India 2022"));
    let signed = pdfcraft_sign::sign(&open(&fixture()), &spoof, &opts()).unwrap();
    let (reports, combined) = verify::verify_doc_with_cca(&open(&signed), &signed, &TrustStore::default());
    assert_eq!(combined.certs.len(), 2, "the bundled CCA roots are consulted");
    let r = reports.iter().find(|r| r.signed).unwrap();
    assert_eq!(r.integrity, Integrity::Intact, "cryptography is fine; trust is not");
    assert_eq!(r.trust, Trust::Untrusted, "a matching subject name alone trusts nothing");
    assert_eq!(verify::summarize(&reports).overall.as_str(), "untrusted");
}

#[test]
fn unsupported_signature_maps_to_unknown_integrity() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    let mut info = signatures(&open(&signed), &signed, &trust).into_iter().find(|s| s.signed).unwrap();
    info.status = Status::Unknown;
    info.details.push("PdfCraft can't check this signature yet: test seam.".to_string());
    let r = verify::report_for(&info, &trust);
    assert!(matches!(r.integrity, Integrity::Unknown { .. }), "{:?}", r.integrity);
    assert_eq!(verify::summarize(std::slice::from_ref(&r)).overall.as_str(), "unsupported");
}

#[test]
fn malformed_bytes_fail_to_open_without_a_crash() {
    assert!(Document::open(Arc::new(b"not a pdf at all".to_vec())).is_err());
    assert!(Document::open(Arc::new(b"%PDF-1.7\ntrailer\n<< >>\n".to_vec())).is_err());
}
