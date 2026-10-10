//! Signing and validating whole documents.

use std::sync::Arc;

use pdfcraft_cos::{Document, Object, PdfString, SaveOptions, write_incremental};
use pdfcraft_sign::{Modification, SignError, SignOptions, Status, Time, TimestampAuthority, TrustStore, der, pkcs12, signatures};

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

#[test]
fn standalone_timestamp_scan_survives_cache_batches_and_object_streams() {
    let mut doc = open(&fixture());
    let filler: Vec<_> = (0..300).map(|i| Object::Ref(doc.add(Object::Int(i)))).collect();
    let mut stamp = pdfcraft_cos::Dict::new();
    stamp.set(b"Type".to_vec(), Object::name("Sig"));
    stamp.set(b"SubFilter".to_vec(), Object::name("ETSI.RFC3161"));
    stamp.set(b"ByteRange".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(1), Object::Int(2), Object::Int(1)]));
    stamp.set(b"Contents".to_vec(), Object::String(PdfString::literal("invalid-token")));
    let stamp = doc.add(Object::Dict(stamp));
    doc.update_dict(doc.root().unwrap(), |catalog| {
        catalog.set(b"UninspectedObjects".to_vec(), Object::Array(filler));
        catalog.set(b"UninspectedTimestamp".to_vec(), Object::Ref(stamp));
    })
    .unwrap();
    let bytes = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
    let doc = open(&bytes);
    assert!(doc.object_numbers().len() > 300);
    let listed = signatures(&doc, &bytes, &TrustStore::default());
    assert!(listed.iter().any(|s| s.doc_timestamp && s.status != Status::Valid));
    assert!(listed.iter().any(|s| s.field == "Approval" && !s.signed));
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

#[test]
fn signing_then_validating_with_and_without_trust() {
    for file in ["rsa-aes.p12", "ec-p256.p12", "ec-p384.p12", "chain.p12"] {
        let id = pkcs12::open(&data(file), "test").unwrap();
        let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
        assert!(signed.starts_with(&fixture()), "{file}: an incremental update");
        let doc = open(&signed);
        let sigs = signatures(&doc, &signed, &TrustStore::default());
        let s = sigs.iter().find(|s| s.signed).expect("a signed field");
        assert_eq!(s.field, "Signature1");
        assert_eq!(s.status, Status::Unknown, "{file}: {:?}", s.details);
        assert_eq!(s.modification, Modification::None);
        assert_eq!(s.signer.as_deref(), id.certificate.subject.common_name());
        assert_eq!((s.reason.as_deref(), s.location.as_deref(), s.page), (Some("I approve this document"), Some("London"), Some(0)));
        assert!(s.visible && s.signed_len == signed.len() && s.revision == 2);
        assert!(s.details.iter().any(|d| d.contains("identity is unknown")));
        // The unsigned field is listed too.
        assert!(sigs.iter().any(|s| s.field == "Approval" && !s.signed));
        // Trusting the signer (or its root) makes it valid.
        let anchor = id.chain.first().cloned().unwrap_or_else(|| id.certificate.clone());
        let trusted = signatures(&doc, &signed, &TrustStore { certs: vec![anchor] });
        let s = trusted.iter().find(|s| s.signed).unwrap();
        assert_eq!(s.status, Status::Valid, "{file}: {:?}", s.details);
        assert_eq!(s.chain.len(), if file == "chain.p12" { 2 } else { 1 });
        // Changing one signed byte breaks it.
        let mut tampered = signed.clone();
        let i = tampered.windows(13).position(|w| w == b"Contract text").unwrap();
        tampered[i] = b'K';
        let s = signatures(&open(&tampered), &tampered, &TrustStore::default()).into_iter().find(|s| s.signed).unwrap();
        assert_eq!(s.status, Status::Invalid);
        assert!(s.details[0].contains("altered or corrupted"), "{:?}", s.details);
    }
}

#[test]
fn signing_an_existing_field_and_counter_signing() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let first = pdfcraft_sign::sign(&open(&fixture()), &id, &SignOptions { field: Some("Approval".into()), ..opts() }).unwrap();
    let id2 = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let second = pdfcraft_sign::sign(&open(&first), &id2, &opts()).unwrap();
    let doc = open(&second);
    let sigs = signatures(&doc, &second, &TrustStore::default());
    let a = sigs.iter().find(|s| s.field == "Approval").unwrap();
    assert!(a.signed);
    assert_eq!(a.rect, Some([150.0, 20.0, 280.0, 70.0]));
    assert_eq!(a.revision, 2);
    assert_eq!(a.modification, Modification::Allowed(vec!["signature".into()]), "{:?}", a.details);
    assert_eq!(a.status, Status::Unknown);
    let b = sigs.iter().find(|s| s.field == "Signature1").unwrap();
    assert_eq!((b.revision, b.modification.clone()), (3, Modification::None));
    assert!(matches!(
        pdfcraft_sign::sign(&doc, &id, &SignOptions { field: Some("Approval".into()), ..opts() }),
        Err(pdfcraft_sign::SignError::Pdf(_))
    ));
}

/// Append a revision that changes object `num` with `f`.
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

#[test]
fn later_changes_are_classified_under_the_signature_permissions() {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    let check = |bytes: &[u8]| signatures(&open(bytes), bytes, &trust).into_iter().find(|s| s.signed).unwrap();
    // Approval signature: comments are permitted, rewriting page content is not.
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    let s = check(&edit_after(&signed, add_comment));
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    assert_eq!(s.modification, Modification::Allowed(vec!["comments".into()]));
    let s = check(&edit_after(&signed, change_text));
    assert_eq!(s.status, Status::Invalid);
    assert_eq!(s.modification, Modification::Disallowed(vec!["page content".into()]));
    // Certified with "no changes allowed": even a comment invalidates it.
    let certified = pdfcraft_sign::sign(&open(&fixture()), &id, &SignOptions { certify: Some(1), ..opts() }).unwrap();
    let s = check(&certified);
    assert_eq!((s.certify, s.status), (Some(1), Status::Valid));
    assert_eq!(check(&edit_after(&certified, add_comment)).status, Status::Invalid);
    // Certified allowing comments: fine.
    let certified3 = pdfcraft_sign::sign(&open(&fixture()), &id, &SignOptions { certify: Some(3), ..opts() }).unwrap();
    assert_eq!(check(&edit_after(&certified3, add_comment)).status, Status::Valid);
}

#[test]
fn invisible_signatures_and_refusals() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &SignOptions { rect: None, ..opts() }).unwrap();
    let s = signatures(&open(&signed), &signed, &TrustStore::default()).into_iter().find(|s| s.signed).unwrap();
    assert!(!s.visible);
    assert!(pdfcraft_sign::sign(&open(&fixture()), &id, &SignOptions { page: 5, ..opts() }).is_err());
    assert_eq!(pdfcraft_sign::pdf::display_date("D:20261002120000+01'00'"), "2026.10.02 12:00:00 +01'00'");
}

/// A deterministic TSA: signs an RFC 3161 response locally with a test digital ID at a fixed
/// time. No sockets; the whole stamping path runs in-process.
struct TestTsa {
    id: pkcs12::DigitalId,
    time: Time,
}

impl TimestampAuthority for TestTsa {
    fn timestamp(&self, request: &[u8]) -> Result<Vec<u8>, SignError> {
        let q = pdfcraft_sign::timestamp::parse_request(request)?;
        pdfcraft_sign::timestamp::respond(
            &self.id.key,
            &self.id.certificate,
            &self.id.chain,
            pdfcraft_sign::DigestAlg::Sha256,
            &q,
            "1.2.3.4",
            self.time,
            7,
        )
    }
}

#[test]
fn signing_with_a_timestamp_embeds_a_verified_rfc3161_token() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let tsa = TestTsa {
        id: pkcs12::open(&data("rsa-aes.p12"), "test").unwrap(),
        time: Time { year: 2026, month: 10, day: 6, hour: 12, minute: 0, second: 0 },
    };
    let signed = pdfcraft_sign::sign_with_timestamp(&open(&fixture()), &id, &opts(), &tsa).unwrap();
    assert!(signed.starts_with(&fixture()), "still an incremental update");
    let anchor = id.chain.first().cloned().unwrap_or_else(|| id.certificate.clone());
    let trust = TrustStore { certs: vec![anchor.clone(), tsa.id.certificate.clone()] };
    let s = signatures(&open(&signed), &signed, &trust).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    assert!(s.timestamp, "{:?}", s.details);
    assert_eq!(s.timestamp_time, Some(tsa.time), "the token's generation time, verified over the signature value");
    assert!(s.details.iter().any(|d| d.contains("trusted time")), "{:?}", s.details);
    // A trusted signer with an untrusted TSA: still valid, but the token's time is unverified.
    let s = signatures(&open(&signed), &signed, &TrustStore { certs: vec![anchor] }).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    assert!(s.timestamp);
    assert_eq!(s.timestamp_time, None);
    assert!(s.details.iter().any(|d| d.contains("unverified timestamp")), "{:?}", s.details);
    // Untrusted, the signature stays intact-but-unknown; the timestamp is still noted.
    let s = signatures(&open(&signed), &signed, &TrustStore::default()).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.status, Status::Unknown);
    assert!(s.timestamp && s.timestamp_time.is_none());
}

#[test]
fn a_malformed_timestamp_response_fails_signing_without_a_file() {
    struct Bad;
    impl TimestampAuthority for Bad {
        fn timestamp(&self, _request: &[u8]) -> Result<Vec<u8>, SignError> {
            Ok(b"not der".to_vec())
        }
    }
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    assert!(pdfcraft_sign::sign_with_timestamp(&open(&fixture()), &id, &opts(), &Bad).is_err());
}

#[test]
fn a_document_timestamp_covers_the_file_and_validates() {
    let tsa = TestTsa {
        id: pkcs12::open(&data("rsa-aes.p12"), "test").unwrap(),
        time: Time { year: 2026, month: 10, day: 6, hour: 12, minute: 0, second: 0 },
    };
    let stamped = pdfcraft_sign::timestamp_document(&open(&fixture()), &tsa, "D:20261006120000Z").unwrap();
    assert!(stamped.starts_with(&fixture()), "an incremental update");
    let trust_tsa = TrustStore { certs: vec![tsa.id.certificate.clone()] };
    let s = signatures(&open(&stamped), &stamped, &trust_tsa).into_iter().find(|s| s.doc_timestamp).unwrap();
    assert!(s.doc_timestamp && s.timestamp);
    assert_eq!(s.sub_filter.as_deref(), Some("ETSI.RFC3161"));
    assert_eq!(s.timestamp_time, Some(tsa.time));
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    assert_eq!(s.modification, Modification::None);
    assert_eq!(s.signed_len, stamped.len());
    // A later byte change breaks the timestamp's imprint.
    let mut tampered = stamped.clone();
    let i = tampered.windows(13).position(|w| w == b"Contract text").unwrap();
    tampered[i] = b'K';
    let s = signatures(&open(&tampered), &tampered, &TrustStore::default()).into_iter().find(|s| s.doc_timestamp).unwrap();
    assert_eq!(s.status, Status::Invalid);
    // Combined with a field signature: both are listed, the stamp covers both revisions.
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    let both = pdfcraft_sign::timestamp_document(&open(&signed), &tsa, "D:20261006130000Z").unwrap();
    let all = signatures(&open(&both), &both, &TrustStore { certs: vec![tsa.id.certificate.clone()] });
    let field = all.iter().find(|s| s.signed && !s.doc_timestamp).unwrap();
    let allowed = match &field.modification {
        Modification::Allowed(k) => k,
        other => panic!("{other:?}"),
    };
    assert!(allowed.contains(&"signature".to_string()), "{allowed:?}");
    let stamp = all.iter().find(|s| s.doc_timestamp).unwrap();
    assert_eq!(stamp.status, Status::Valid, "{:?}", stamp.details);
}

#[test]
fn cached_discovery_still_validates_current_timestamp_bytes_ranges_and_trust() {
    let tsa = TestTsa {
        id: pkcs12::open(&data("rsa-aes.p12"), "test").unwrap(),
        time: Time { year: 2026, month: 10, day: 6, hour: 12, minute: 0, second: 0 },
    };
    let mut source = open(&fixture());
    source
        .update_dict(source.root().unwrap(), |d| {
            d.remove(b"AcroForm");
        })
        .unwrap();
    let bytes = pdfcraft_sign::timestamp_document(&source, &tsa, "D:20261006120000Z").unwrap();
    let doc = open(&bytes);
    let cache = pdfcraft_sign::DigestCache::default();
    let trusted = TrustStore { certs: vec![tsa.id.certificate.clone()] };
    let check = |doc: &Document, bytes: &[u8], trust: &TrustStore| {
        let listed = pdfcraft_sign::pdf::list_cached(doc, bytes, trust, &cache);
        assert_eq!(listed.len(), 1);
        assert!(listed[0].doc_timestamp);
        listed.into_iter().next().unwrap()
    };
    assert_eq!(check(&doc, &bytes, &TrustStore::default()).status, Status::Unknown);
    assert_eq!(check(&doc, &bytes, &trusted).status, Status::Valid);
    assert_eq!(check(&doc, &bytes, &TrustStore::default()).status, Status::Unknown);
    let mut tampered = bytes.clone();
    let at = tampered.windows(13).position(|w| w == b"Contract text").unwrap();
    tampered[at] = b'K';
    assert_eq!(check(&doc, &tampered, &trusted).status, Status::Invalid);
    assert_eq!(check(&doc, &[], &trusted).status, Status::Invalid);
    let stamp = doc.scan_objects().find(|(_, o)| o.as_dict().is_some_and(|d| d.name(b"SubFilter") == Some(b"ETSI.RFC3161"))).unwrap().0;
    let mut edited = doc.clone();
    edited.update_dict(stamp, |d| d.set(b"ByteRange".to_vec(), Object::Array(vec![Object::Int(-1)]))).unwrap();
    assert_eq!(check(&edited, &bytes, &trusted).status, Status::Invalid);
    assert_eq!(check(&doc, &bytes, &trusted).status, Status::Valid);
    // Even a malformed token in the same byte ranges must be read again after discovery.
    let br = doc.get(stamp).as_dict().unwrap().get(b"ByteRange").unwrap().as_array().unwrap().clone();
    let gap_start = br[1].as_int().unwrap() as usize;
    tampered = bytes.clone();
    tampered[gap_start + 1] = b'F';
    tampered[gap_start + 2] = b'F';
    assert_eq!(check(&doc, &tampered, &trusted).status, Status::Invalid);
}

#[test]
fn embedding_ltv_evidence_adds_a_dss_and_keeps_signatures_valid() {
    use pdfcraft_sign::dss::{self, Evidence};
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    let evidence =
        Evidence { certs: vec![id.certificate.raw.clone()], ocsps: vec![b"synthetic ocsp".to_vec()], crls: vec![b"synthetic crl".to_vec()] };
    let ltv = dss::embed(&open(&signed), &evidence).unwrap();
    assert!(ltv.starts_with(&signed), "incremental");
    let doc = open(&ltv);
    // The store is in the catalog with one certificate and the /VRI entry for the signature.
    let root = doc.root().unwrap();
    let dss_dict: pdfcraft_cos::Dict =
        doc.get(root).as_dict().unwrap().get(b"DSS").map(|d| doc.resolve(d)).and_then(|d| d.as_dict().cloned()).unwrap();
    assert_eq!(dss_dict.name(b"Type"), Some(b"DSS".as_slice()));
    assert!(dss_dict.contains(b"Certs") && dss_dict.contains(b"OCSPs") && dss_dict.contains(b"CRLs") && dss_dict.contains(b"VRI"));
    // The earlier signature still validates; the DSS counts as a permitted change.
    let s = signatures(&doc, &ltv, &TrustStore::default()).into_iter().find(|s| s.signed && !s.doc_timestamp).unwrap();
    assert_eq!(s.status, Status::Unknown);
    let allowed = match &s.modification {
        Modification::Allowed(k) => k,
        other => panic!("{other:?}"),
    };
    assert!(allowed.contains(&"document security store".to_string()), "{allowed:?}");
    // Embedding twice keeps one certificate (byte-identical dedup) and stays valid.
    let twice = dss::embed(&doc, &evidence).unwrap();
    let doc2 = open(&twice);
    let dss2: pdfcraft_cos::Dict =
        doc2.get(doc2.root().unwrap()).as_dict().unwrap().get(b"DSS").map(|d| doc2.resolve(d)).and_then(|d| d.as_dict().cloned()).unwrap();
    let certs = dss2.get(b"Certs").map(|c| doc2.resolve(c)).and_then(|c| c.as_array().cloned()).unwrap();
    assert_eq!(certs.len(), 1, "byte-identical evidence is not duplicated");
}

#[test]
fn sign_then_ltv_then_timestamp_makes_a_b_lta_file() {
    use pdfcraft_sign::dss::{self, Evidence};
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let tsa = TestTsa {
        id: pkcs12::open(&data("rsa-aes.p12"), "test").unwrap(),
        time: Time { year: 2026, month: 10, day: 6, hour: 12, minute: 0, second: 0 },
    };
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    let ltv = dss::embed(&open(&signed), &Evidence { certs: vec![id.certificate.raw.clone()], ocsps: Vec::new(), crls: Vec::new() }).unwrap();
    let lta = pdfcraft_sign::timestamp_document(&open(&ltv), &tsa, "D:20261006120000Z").unwrap();
    let all = signatures(&open(&lta), &lta, &TrustStore { certs: vec![tsa.id.certificate.clone()] });
    assert_eq!(all.iter().filter(|s| s.signed).count(), 2);
    let field = all.iter().find(|s| s.signed && !s.doc_timestamp).unwrap();
    assert_eq!(field.status, Status::Unknown);
    let allowed = match &field.modification {
        Modification::Allowed(k) => k,
        other => panic!("{other:?}"),
    };
    assert!(allowed.contains(&"document security store".to_string()) && allowed.contains(&"signature".to_string()), "{allowed:?}");
    assert!(!allowed.contains(&"page content".to_string()), "{allowed:?}");
    let stamp = all.iter().find(|s| s.doc_timestamp).unwrap();
    assert_eq!(stamp.status, Status::Valid, "{:?}", stamp.details);
}

#[test]
fn an_embedded_verified_revocation_invalidates_the_signature() {
    use pdfcraft_sign::dss::{self, Evidence};
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    // The signer's own CRL (self-signed test identity) revokes its certificate, in a window
    // that covers the signing date (October 2026).
    let (this, next) =
        (Time { year: 2026, month: 1, day: 1, hour: 0, minute: 0, second: 0 }, Time { year: 2027, month: 1, day: 1, hour: 0, minute: 0, second: 0 });
    let alg = id.key.signature_algorithm(pdfcraft_sign::DigestAlg::Sha256);
    let tbs = der::seq(&[
        &der::int(1),
        &alg,
        &id.certificate.subject.raw,
        &this.encode(),
        &next.encode(),
        &der::seq(&[&der::seq(&[
            &der::uint(&id.certificate.serial),
            &Time { year: 2026, month: 6, day: 1, hour: 8, minute: 0, second: 0 }.encode(),
        ])]),
    ]);
    let sig = id.key.sign(pdfcraft_sign::DigestAlg::Sha256, &tbs).unwrap();
    let crl = der::seq(&[&tbs, &alg, &der::bit_string(&sig)]);
    let ltv = dss::embed(&open(&signed), &Evidence { certs: Vec::new(), ocsps: Vec::new(), crls: vec![crl] }).unwrap();
    let s = signatures(&open(&ltv), &ltv, &TrustStore::default()).into_iter().find(|s| s.signed && !s.doc_timestamp).unwrap();
    assert_eq!(s.status, Status::Invalid, "{:?}", s.details);
    assert!(s.details.iter().any(|d| d.contains("revoked") && d.contains("CRL")), "{:?}", s.details);
}

#[test]
fn validates_a_signature_made_by_openssl() {
    let bytes = data("openssl-signed.pdf");
    let doc = open(&bytes);
    let s = signatures(&doc, &bytes, &TrustStore::default()).into_iter().next().unwrap();
    assert_eq!((s.field.as_str(), s.sub_filter.as_deref()), ("OpenSSL", Some("adbe.pkcs7.detached")));
    assert_eq!(s.status, Status::Unknown, "{:?}", s.details);
    assert_eq!(s.signer.as_deref(), Some("Test Signer RSA"));
    assert!(s.signing_time.is_some_and(|t| t.year == 2026), "from the CMS signing-time attribute");
    assert!(!s.visible);
    let rsa = pdfcraft_sign::Certificate::parse(&pkcs12::open(&data("rsa-aes.p12"), "test").unwrap().certificate.raw).unwrap();
    let s = signatures(&doc, &bytes, &TrustStore { certs: vec![rsa] }).into_iter().next().unwrap();
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    // A later comment is allowed for this approval signature.
    let edited = edit_after(&bytes, add_comment);
    let s = signatures(&open(&edited), &edited, &TrustStore::default()).into_iter().next().unwrap();
    assert_eq!(s.modification, Modification::Allowed(vec!["comments".into()]));
}

#[test]
fn files_without_a_cross_reference_table_are_signed_with_a_full_write() {
    // No xref: the document is reconstructed, so saving rewrites (and renumbers) everything.
    let bytes = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >> endobj
trailer << /Root 1 0 R >>
%%EOF"
        .to_vec();
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    for certify in [None, Some(2)] {
        let signed = pdfcraft_sign::sign(&open(&bytes), &id, &SignOptions { certify, rect: None, ..opts() }).unwrap();
        let s = signatures(&open(&signed), &signed, &TrustStore::default()).into_iter().find(|s| s.signed).unwrap();
        assert_eq!((s.status, s.certify, s.modification.clone()), (Status::Unknown, certify, Modification::None), "{:?}", s.details);
    }
}

/// Signing with Keychain identities, in a throwaway keychain file. Ignored by default: creating
/// a keychain touches the user's keychain search list (restored afterwards).
#[cfg(target_os = "macos")]
#[test]
#[ignore = "creates a temporary macOS keychain; run with --ignored"]
fn signing_with_keychain_identities() {
    use std::process::Command;
    let dir = std::env::temp_dir().join(format!("pdfcraft-keychain-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let kc = dir.join("test.keychain-db");
    let sec = |args: &[&str]| Command::new("security").args(args).output().unwrap();
    let list = String::from_utf8(sec(&["list-keychains", "-d", "user"]).stdout).unwrap();
    let saved: Vec<String> = list.lines().map(|l| l.trim().trim_matches('"').to_string()).filter(|l| !l.is_empty()).collect();
    let k = kc.to_str().unwrap();
    assert!(sec(&["create-keychain", "-p", "pc-test", k]).status.success());
    let restore = || {
        let mut args = vec!["list-keychains", "-d", "user", "-s"];
        args.extend(saved.iter().map(String::as_str));
        sec(&args);
    };
    restore();
    // Everything that can fail runs before the clean-up below.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        sec(&["unlock-keychain", "-p", "pc-test", k]);
        // macOS imports only legacy-format PKCS #12 (SHA-1 MAC).
        for f in ["rsa-legacy.p12", "ec-legacy.p12"] {
            let p = format!("{}/tests/data/{f}", env!("CARGO_MANIFEST_DIR"));
            let out = sec(&["import", &p, "-k", k, "-P", "test", "-A"]);
            assert!(out.status.success(), "{f}: {}", String::from_utf8_lossy(&out.stderr));
        }
        let ids = pdfcraft_sign::keychain::identities(Some(&kc)).unwrap();
        assert_eq!(ids.len(), 2, "{ids:?}");
        for id in &ids {
            assert!(id.key.is_external());
            let signed = pdfcraft_sign::sign(&open(&fixture()), id, &opts()).unwrap();
            let trusted = signatures(&open(&signed), &signed, &TrustStore { certs: vec![id.certificate.clone()] });
            let s = trusted.iter().find(|s| s.signed).unwrap();
            assert_eq!(s.status, Status::Valid, "{:?}", s.details);
        }
    }));
    sec(&["delete-keychain", k]);
    restore();
    let _ = std::fs::remove_dir_all(&dir);
    result.unwrap();
}

/// `fixture()` plus a Form XObject (object 7) that the page draws.
fn fixture_with_xobject() -> Vec<u8> {
    let objs: Vec<&[u8]> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /X1 7 0 R >> >> /Annots [6 0 R] >>",
        b"<< /Length 8 >>\nstream\n/X1 Do\n\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        b"<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Rect [150 20 280 70] /P 3 0 R /F 4 >>",
        b"<< /Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >> /Length 44 >>\nstream\nBT /F1 14 Tf 20 250 Td (Contract text) Tj ET\nendstream",
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

/// An update that declares the form dynamic XFA (/XFA + /NeedsRendering) and swaps the whole
/// page tree for a new one. Marking a form XFA must not turn a page replacement into "form
/// fill": the signed pages are gone.
fn xfa_page_swap(doc: &mut Document) {
    let xdp = b"<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\"><template/></xdp:xdp>".to_vec();
    let mut xd = pdfcraft_cos::Dict::new();
    xd.set(b"Length".to_vec(), Object::Int(xdp.len() as i64));
    let xfa = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(xd, xdp)));
    let body = b"BT /F1 14 Tf 20 250 Td (Forged text) Tj ET".to_vec();
    let mut cd = pdfcraft_cos::Dict::new();
    cd.set(b"Length".to_vec(), Object::Int(body.len() as i64));
    let contents = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(cd, body)));
    let pages = doc.add(Object::Null);
    let mut page = pdfcraft_cos::Dict::new();
    page.set(b"Type".to_vec(), Object::name("Page"));
    page.set(b"Parent".to_vec(), Object::Ref(pages));
    page.set(b"MediaBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(300), Object::Int(300)]));
    page.set(b"Contents".to_vec(), Object::Ref(contents));
    let mut font = pdfcraft_cos::Dict::new();
    font.set(b"F1".to_vec(), Object::Ref(pdfcraft_cos::ObjRef { num: 5, generation: 0 }));
    let mut res = pdfcraft_cos::Dict::new();
    res.set(b"Font".to_vec(), Object::Dict(font));
    page.set(b"Resources".to_vec(), Object::Dict(res));
    let page = doc.add(Object::Dict(page));
    let mut tree = pdfcraft_cos::Dict::new();
    tree.set(b"Type".to_vec(), Object::name("Pages"));
    tree.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
    tree.set(b"Count".to_vec(), Object::Int(1));
    doc.set(pages, Object::Dict(tree));
    let root = doc.root().unwrap();
    doc.update_dict(root, |c| {
        c.set(b"Pages".to_vec(), Object::Ref(pages));
        if let Some(Object::Dict(form)) = c.get_mut(b"AcroForm") {
            form.set(b"XFA".to_vec(), Object::Ref(xfa));
            form.set(b"NeedsRendering".to_vec(), Object::Bool(true));
        }
    })
    .unwrap();
}

#[test]
fn declaring_a_form_xfa_does_not_excuse_replacing_the_pages() {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    let check = |bytes: &[u8]| signatures(&open(bytes), bytes, &trust).into_iter().find(|s| s.signed).unwrap();
    for certify in [None, Some(2), Some(3)] {
        let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &SignOptions { certify, ..opts() }).unwrap();
        assert_eq!(check(&signed).status, Status::Valid);
        let s = check(&edit_after(&signed, xfa_page_swap));
        assert_eq!(s.status, Status::Invalid, "{certify:?}: {:?}", s.details);
        assert!(matches!(&s.modification, Modification::Disallowed(k) if k.contains(&"document structure".to_string())), "{:?}", s.modification);
    }
}

/// Re-encode a DER CMS the way Windows CryptoAPI / Adobe PPKMS / `openssl cms -stream` write it:
/// indefinite lengths on ContentInfo, [0] and SignedData (the signed attributes stay DER).
fn to_ber(der_cms: &[u8]) -> Vec<u8> {
    use pdfcraft_sign::der::Tlv;
    let ci = Tlv::parse(der_cms).unwrap().0.children().unwrap();
    let sd = ci[1].inner().unwrap().children().unwrap();
    let mut out = vec![0x30, 0x80];
    out.extend_from_slice(ci[0].raw);
    out.extend_from_slice(&[0xA0, 0x80, 0x30, 0x80]);
    for c in sd {
        out.extend_from_slice(c.raw);
    }
    out.extend_from_slice(&[0; 6]);
    out
}

/// The signature `/Contents` hex of the only signed field, and its position in `pdf`.
fn contents_span(pdf: &[u8]) -> (usize, usize) {
    let key = b"/Contents <";
    let start = pdf.windows(key.len()).position(|w| w == key).map(|i| i + key.len()).unwrap();
    (start, start + pdf[start..].iter().position(|b| *b == b'>').unwrap())
}

#[test]
fn validates_signatures_written_with_ber_indefinite_lengths() {
    for file in ["rsa-aes.p12", "ec-p256.p12"] {
        let id = pkcs12::open(&data(file), "test").unwrap();
        let mut signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
        let (a, b) = contents_span(&signed);
        let hex = |s: &[u8]| s.iter().map(|x| format!("{x:02x}")).collect::<String>();
        let der: Vec<u8> = (a..b).step_by(2).map(|i| u8::from_str_radix(std::str::from_utf8(&signed[i..i + 2]).unwrap(), 16).unwrap()).collect();
        let der_len = pdfcraft_sign::der::Tlv::parse(&der).unwrap().0.raw.len();
        let mut ber = to_ber(&der[..der_len]);
        assert_eq!(ber[1], 0x80, "indefinite");
        ber.resize((b - a) / 2, 0);
        signed[a..b].copy_from_slice(hex(&ber).as_bytes());
        let anchor = id.certificate.clone();
        let s = signatures(&open(&signed), &signed, &TrustStore { certs: vec![anchor] }).into_iter().find(|s| s.signed).unwrap();
        assert_eq!(s.status, Status::Valid, "{file}: {:?}", s.details);
        assert!(s.details.iter().any(|d| d.contains("BER")), "the tolerance is reported: {:?}", s.details);
        // Tampering is still caught: the digest check is not relaxed.
        let mut tampered = signed.clone();
        let i = tampered.windows(13).position(|w| w == b"Contract text").unwrap();
        tampered[i] = b'K';
        let s = signatures(&open(&tampered), &tampered, &TrustStore::default()).into_iter().find(|s| s.signed).unwrap();
        assert_eq!(s.status, Status::Invalid);
    }
}

#[test]
fn validation_data_and_xmp_added_after_signing_are_not_tampering() {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts()).unwrap();
    // As DocuSign / Acrobat LTV do: a DSS whose /Certs and /VRI are indirect objects, and XMP.
    let edited = edit_after(&signed, |doc| {
        let mut cert = pdfcraft_cos::Dict::new();
        cert.set(b"Length".to_vec(), Object::Int(3));
        let cert = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(cert, vec![1, 2, 3])));
        let certs = doc.add(Object::Array(vec![Object::Ref(cert)]));
        let vri = doc.add(Object::Dict(pdfcraft_cos::Dict::new()));
        let mut dss = pdfcraft_cos::Dict::new();
        dss.set(b"Certs".to_vec(), Object::Ref(certs));
        dss.set(b"VRI".to_vec(), Object::Ref(vri));
        let dss = doc.add(Object::Dict(dss));
        let mut xmp = pdfcraft_cos::Dict::new();
        xmp.set(b"Type".to_vec(), Object::name("Metadata"));
        xmp.set(b"Subtype".to_vec(), Object::name("XML"));
        xmp.set(b"Length".to_vec(), Object::Int(1));
        let xmp = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(xmp, b"x".to_vec())));
        let root = doc.root().unwrap();
        doc.update_dict(root, |c| {
            c.set(b"DSS".to_vec(), Object::Ref(dss));
            c.set(b"Metadata".to_vec(), Object::Ref(xmp));
        })
        .unwrap();
    });
    let s = signatures(&open(&edited), &edited, &TrustStore::default()).into_iter().find(|s| s.signed).unwrap();
    assert!(matches!(s.modification, Modification::Allowed(_)), "{:?}", s.modification);
    assert_eq!(s.status, Status::Unknown, "{:?}", s.details);
    // Page content changes are still refused.
    let edited = edit_after(&signed, change_text);
    let s = signatures(&open(&edited), &edited, &TrustStore::default()).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.status, Status::Invalid);
}

// ── DSS and metadata: what later revisions may add ─────────────────────────────────────────

fn oref(num: u32) -> Object {
    Object::Ref(pdfcraft_cos::ObjRef { num, generation: 0 })
}

fn dict_of(entries: Vec<(&str, Object)>) -> pdfcraft_cos::Dict {
    let mut d = pdfcraft_cos::Dict::new();
    for (k, v) in entries {
        d.set(k.as_bytes().to_vec(), v);
    }
    d
}

fn stream_of(body: &[u8]) -> Object {
    let d = dict_of(vec![("Length", Object::Int(body.len() as i64))]);
    Object::Stream(pdfcraft_cos::Stream::from_raw(d, body.to_vec()))
}

/// Set the catalog's `/DSS`.
fn set_dss(doc: &mut Document, dss: Object) {
    let root = doc.root().unwrap();
    doc.update_dict(root, |c| c.set(b"DSS".to_vec(), dss)).unwrap();
}

/// Rewrite object `num` (the page's contents) so the page shows other text.
fn forge_contents(doc: &mut Document, num: u32) {
    doc.set(pdfcraft_cos::ObjRef { num, generation: 0 }, stream_of(b"BT /F1 14 Tf 20 250 Td (Forged) Tj ET"));
}

/// `fixture()` with the page's `/Contents` an indirect array (object 7) holding object 4.
fn fixture_with_contents_array() -> Vec<u8> {
    let objs: Vec<&[u8]> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 7 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>",
        b"<< /Length 44 >>\nstream\nBT /F1 14 Tf 20 250 Td (Contract text) Tj ET\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        b"<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Rect [150 20 280 70] /P 3 0 R /F 4 >>",
        b"[4 0 R]",
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

/// Every DocMDP level: no certification (an approval signature), then P=1, 2 and 3.
const LEVELS: [Option<u8>; 4] = [None, Some(1), Some(2), Some(3)];

/// Each tampering `attack` of the signed `base` is refused at every level, with `kind` among the
/// reasons, while the untouched file stays valid.
fn assert_refused_at_every_level(base: &[u8], kind: &str, attack: impl Fn(&mut Document)) {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    let check = |bytes: &[u8]| signatures(&open(bytes), bytes, &trust).into_iter().find(|s| s.signed).unwrap();
    for certify in LEVELS {
        let signed = pdfcraft_sign::sign(&open(base), &id, &SignOptions { certify, ..opts() }).unwrap();
        assert_eq!(check(&signed).status, Status::Valid, "{certify:?}: untouched");
        let s = check(&edit_after(&signed, &attack));
        assert_eq!(s.status, Status::Invalid, "{certify:?}: {:?} {:?}", s.modification, s.details);
        assert!(matches!(&s.modification, Modification::Disallowed(k) if k.iter().any(|k| k == kind)), "{certify:?}: {:?}", s.modification);
    }
}

#[test]
fn a_dss_entry_does_not_make_the_object_it_names_part_of_the_store() {
    // `/DSS << /X 4 0 R >>` and a rewrite of object 4, the page's contents: the contents are
    // still page content.
    assert_refused_at_every_level(&fixture(), "page content", |doc| {
        set_dss(doc, Object::Dict(dict_of(vec![("X", oref(4))])));
        forge_contents(doc, 4);
    });
    // The same through an indirect store, and through every key the store does define.
    for key in ["X", "Certs", "CRLs", "OCSPs"] {
        assert_refused_at_every_level(&fixture(), "page content", |doc| {
            let dss = doc.add(Object::Dict(dict_of(vec![(key, Object::Array(vec![oref(4)]))])));
            set_dss(doc, Object::Ref(dss));
            forge_contents(doc, 4);
        });
    }
    // A VRI entry's /Cert array, and its /TS.
    for key in ["Cert", "CRL", "OCSP", "TS"] {
        assert_refused_at_every_level(&fixture(), "page content", |doc| {
            let value = if key == "TS" { oref(4) } else { Object::Array(vec![oref(4)]) };
            let entry = doc.add(Object::Dict(dict_of(vec![(key, value)])));
            let vri = doc.add(Object::Dict(dict_of(vec![("0123456789ABCDEF0123456789ABCDEF01234567", Object::Ref(entry))])));
            let dss = doc.add(Object::Dict(dict_of(vec![("VRI", Object::Ref(vri))])));
            set_dss(doc, Object::Ref(dss));
            forge_contents(doc, 4);
        });
    }
}

#[test]
fn an_existing_dictionary_is_not_a_store_part_because_the_store_names_it() {
    // The font dictionary (object 5), rewritten and then listed as the store, its /VRI, a VRI
    // entry, or simply labelled /Type /DSS.
    let rewrite_font = |doc: &mut Document, extra: Option<(&str, Object)>| {
        let mut d = dict_of(vec![("Type", Object::name("Font")), ("Subtype", Object::name("Type1")), ("BaseFont", Object::name("Courier"))]);
        if let Some((k, v)) = extra {
            d.set(k.as_bytes().to_vec(), v);
        }
        doc.set(pdfcraft_cos::ObjRef { num: 5, generation: 0 }, Object::Dict(d));
    };
    assert_refused_at_every_level(&fixture(), "other changes", |doc| {
        rewrite_font(doc, Some(("Type", Object::name("DSS"))));
    });
    assert_refused_at_every_level(&fixture(), "other changes", |doc| {
        set_dss(doc, oref(5));
        rewrite_font(doc, None);
    });
    assert_refused_at_every_level(&fixture(), "other changes", |doc| {
        set_dss(doc, Object::Dict(dict_of(vec![("VRI", oref(5))])));
        rewrite_font(doc, None);
    });
    // An existing dictionary with only DSS-looking keys that the signed file never had as a store.
    assert_refused_at_every_level(&fixture(), "other changes", |doc| {
        doc.set(
            pdfcraft_cos::ObjRef { num: 5, generation: 0 },
            Object::Dict(dict_of(vec![("Type", Object::name("Font")), ("Cert", Object::Array(vec![]))])),
        );
        set_dss(doc, Object::Dict(dict_of(vec![("VRI", Object::Dict(dict_of(vec![("0123456789ABCDEF0123456789ABCDEF01234567", oref(5))])))])));
    });
}

#[test]
fn an_existing_array_stays_what_it_is_when_the_store_points_at_it() {
    // The page's /Contents is an indirect array (object 7). Pointing /Certs at it and appending
    // a new stream still changes what the page shows.
    assert_refused_at_every_level(&fixture_with_contents_array(), "other changes", |doc| {
        let extra = doc.add(stream_of(b"BT /F1 14 Tf 20 200 Td (Forged) Tj ET"));
        doc.set(pdfcraft_cos::ObjRef { num: 7, generation: 0 }, Object::Array(vec![oref(4), Object::Ref(extra)]));
        set_dss(doc, Object::Dict(dict_of(vec![("Certs", oref(7))])));
    });
}

#[test]
fn a_dictionary_labelled_metadata_is_not_a_metadata_change() {
    // Only the catalog's XMP stream is metadata (a stream, not a dictionary a label was put on).
    assert_refused_at_every_level(&fixture(), "other changes", |doc| {
        doc.set(
            pdfcraft_cos::ObjRef { num: 5, generation: 0 },
            Object::Dict(dict_of(vec![
                ("Type", Object::name("Metadata")),
                ("Subtype", Object::name("Type1")),
                ("BaseFont", Object::name("Courier")),
            ])),
        );
    });
}

/// A one-page document like `fixture()` (objects 1–6) plus a stream, object 7, with the given
/// dictionary entries and body. `catalog_extra` goes into the catalog; the page draws object 7
/// as a Form XObject when `drawn`.
fn fixture_with_stream_7(catalog_extra: &str, drawn: bool, dict: &str, body: &[u8]) -> Vec<u8> {
    let page_content: &[u8] =
        if drawn { b"BT /F1 14 Tf 20 250 Td (Contract text) Tj ET /Fm1 Do" } else { b"BT /F1 14 Tf 20 250 Td (Contract text) Tj ET" };
    let xobject = if drawn { " /XObject << /Fm1 7 0 R >>" } else { "" };
    let stream = |dict: &str, body: &[u8]| {
        let mut o = format!("<< {dict} /Length {} >>\nstream\n", body.len()).into_bytes();
        o.extend_from_slice(body);
        o.extend_from_slice(b"\nendstream");
        o
    };
    let objs: Vec<Vec<u8>> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] >> {catalog_extra} >>").into_bytes(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >>{xobject} >> /Annots [6 0 R] >>"
        )
        .into_bytes(),
        stream("", page_content),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        b"<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Rect [150 20 280 70] /P 3 0 R /F 4 >>".to_vec(),
        stream(dict, body),
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

#[test]
fn a_stream_already_labelled_metadata_when_signed_is_not_metadata_unless_the_catalog_names_it() {
    // A Form XObject the page draws, labelled /Type /Metadata before signing (the renderer ignores
    // /Type). Rewriting it later changes what the page shows, so it is not a metadata change.
    let base = fixture_with_stream_7("", true, "/Type /Metadata /Subtype /Form /BBox [0 0 300 300]", b"0 0 1 rg 10 10 100 100 re f");
    assert_refused_at_every_level(&base, "other changes", |doc| {
        let d = dict_of(vec![
            ("Type", Object::name("Metadata")),
            ("Subtype", Object::name("Form")),
            ("BBox", Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(300), Object::Int(300)])),
            ("Length", Object::Int(27)),
        ]);
        doc.set(
            pdfcraft_cos::ObjRef { num: 7, generation: 0 },
            Object::Stream(pdfcraft_cos::Stream::from_raw(d, b"1 0 0 rg 10 10 100 100 re f".to_vec())),
        );
    });
}

#[test]
fn the_catalogs_own_xmp_stream_may_still_be_rewritten_after_signing() {
    // The control for the test above: the stream the signed catalog's /Metadata names is the
    // document's XMP, and rewriting it is allowed at every level.
    let base = fixture_with_stream_7("/Metadata 7 0 R", false, "/Type /Metadata /Subtype /XML", b"<x:xmpmeta/>");
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    for certify in LEVELS {
        let signed = pdfcraft_sign::sign(&open(&base), &id, &SignOptions { certify, ..opts() }).unwrap();
        let edited = edit_after(&signed, |doc| {
            let d = dict_of(vec![("Type", Object::name("Metadata")), ("Subtype", Object::name("XML")), ("Length", Object::Int(13))]);
            doc.set(pdfcraft_cos::ObjRef { num: 7, generation: 0 }, Object::Stream(pdfcraft_cos::Stream::from_raw(d, b"<x:xmpmeta />".to_vec())));
        });
        let s = signatures(&open(&edited), &edited, &trust).into_iter().find(|s| s.signed).unwrap();
        assert!(matches!(&s.modification, Modification::Allowed(k) if k.iter().any(|k| k == "metadata")), "{certify:?}: {:?}", s.modification);
        assert_eq!(s.status, Status::Valid, "{certify:?}: {:?}", s.details);
    }
}

/// A document whose catalog already has a store: `/Certs` (object `.0`) with one certificate,
/// and `/VRI` (object `.1`).
fn fixture_with_dss() -> (Vec<u8>, u32, u32) {
    let mut ids = (0, 0);
    let bytes = edit_after(&fixture(), |doc| {
        let cert = doc.add(stream_of(&[1, 2, 3]));
        let certs = doc.add(Object::Array(vec![Object::Ref(cert)]));
        let vri = doc.add(Object::Dict(pdfcraft_cos::Dict::new()));
        let dss = doc.add(Object::Dict(dict_of(vec![("Type", Object::name("DSS")), ("Certs", Object::Ref(certs)), ("VRI", Object::Ref(vri))])));
        set_dss(doc, Object::Ref(dss));
        ids = (certs.num, vri.num);
    });
    (bytes, ids.0, ids.1)
}

#[test]
fn a_signed_store_may_grow_but_not_lose_or_swap_entries() {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    let check = |bytes: &[u8]| signatures(&open(bytes), bytes, &trust).into_iter().find(|s| s.signed).unwrap();
    let (base, certs, vri) = fixture_with_dss();
    for certify in LEVELS {
        let signed = pdfcraft_sign::sign(&open(&base), &id, &SignOptions { certify, ..opts() }).unwrap();
        assert_eq!(check(&signed).status, Status::Valid, "{certify:?}");
        // Validation data appended to the arrays, and a VRI entry: what Acrobat's LTV does.
        let grown = edit_after(&signed, |doc| {
            let more = doc.add(stream_of(&[4, 5, 6]));
            let entry = doc.add(Object::Dict(dict_of(vec![("Cert", Object::Array(vec![Object::Ref(more)]))])));
            let old = open(&signed);
            let Object::Array(mut kept) = (*old.get(pdfcraft_cos::ObjRef { num: certs, generation: 0 })).clone() else { panic!("array") };
            kept.push(Object::Ref(more));
            doc.set(pdfcraft_cos::ObjRef { num: certs, generation: 0 }, Object::Array(kept));
            doc.set(
                pdfcraft_cos::ObjRef { num: vri, generation: 0 },
                Object::Dict(dict_of(vec![("0123456789ABCDEF0123456789ABCDEF01234567", Object::Ref(entry))])),
            );
        });
        let s = check(&grown);
        assert_eq!(s.status, Status::Valid, "{certify:?}: {:?} {:?}", s.modification, s.details);
        assert!(matches!(&s.modification, Modification::Allowed(k) if k.iter().any(|k| k == "document security store")), "{:?}", s.modification);
        // Dropping what was signed is a change, not an addition.
        let dropped = edit_after(&signed, |doc| {
            doc.set(pdfcraft_cos::ObjRef { num: certs, generation: 0 }, Object::Array(vec![]));
        });
        let s = check(&dropped);
        assert_eq!(s.status, Status::Invalid, "{certify:?}: {:?}", s.modification);
        assert!(matches!(&s.modification, Modification::Disallowed(k) if k.iter().any(|k| k == "other changes")), "{:?}", s.modification);
    }
}

#[test]
fn a_form_xobject_relabelled_metadata_is_not_a_metadata_change() {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    let signed = pdfcraft_sign::sign(&open(&fixture_with_xobject()), &id, &opts()).unwrap();
    let relabel = |doc: &mut Document| {
        let mut d = pdfcraft_cos::Dict::new();
        d.set(b"Type".to_vec(), Object::name("Metadata"));
        d.set(b"Subtype".to_vec(), Object::name("Form"));
        d.set(b"BBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(300), Object::Int(300)]));
        let body = b"BT /F1 14 Tf 20 250 Td (Forged text) Tj ET".to_vec();
        d.set(b"Length".to_vec(), Object::Int(body.len() as i64));
        doc.set(pdfcraft_cos::ObjRef { num: 7, generation: 0 }, Object::Stream(pdfcraft_cos::Stream::from_raw(d, body)));
    };
    let edited = edit_after(&signed, relabel);
    let s = signatures(&open(&edited), &edited, &trust).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.status, Status::Invalid, "{:?}", s.details);
    assert!(matches!(&s.modification, Modification::Disallowed(k) if !k.contains(&"metadata".to_string())), "{:?}", s.modification);
    // The catalog's own XMP stream may still be updated.
    let with_xmp = edit_after(&fixture_with_xobject(), |doc| {
        let xmp = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>".to_vec();
        let mut d = pdfcraft_cos::Dict::new();
        d.set(b"Type".to_vec(), Object::name("Metadata"));
        d.set(b"Subtype".to_vec(), Object::name("XML"));
        d.set(b"Length".to_vec(), Object::Int(xmp.len() as i64));
        let r = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(d, xmp)));
        let root = doc.root().unwrap();
        doc.update_dict(root, |c| c.set(b"Metadata".to_vec(), Object::Ref(r))).unwrap();
    });
    let signed_xmp = pdfcraft_sign::sign(&open(&with_xmp), &id, &opts()).unwrap();
    let rewrite_xmp = edit_after(&signed_xmp, |doc| {
        let root = doc.root().unwrap();
        let r = doc.get(root).as_dict().unwrap().get(b"Metadata").and_then(Object::as_ref).unwrap();
        let xmp = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><!-- edited --></x:xmpmeta>".to_vec();
        let mut d = pdfcraft_cos::Dict::new();
        d.set(b"Type".to_vec(), Object::name("Metadata"));
        d.set(b"Subtype".to_vec(), Object::name("XML"));
        d.set(b"Length".to_vec(), Object::Int(xmp.len() as i64));
        doc.set(r, Object::Stream(pdfcraft_cos::Stream::from_raw(d, xmp)));
    });
    let s = signatures(&open(&rewrite_xmp), &rewrite_xmp, &trust).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    assert_eq!(s.modification, Modification::Allowed(vec!["metadata".into()]));
}

/// A self-signed CRL from `id` revoking its own certificate, valid from `this` to `next`.
fn self_crl(id: &pkcs12::DigitalId, this: Time, next: Time, revoked_at: Time) -> Vec<u8> {
    let alg = id.key.signature_algorithm(pdfcraft_sign::DigestAlg::Sha256);
    let tbs = der::seq(&[
        &der::int(1),
        &alg,
        &id.certificate.subject.raw,
        &this.encode(),
        &next.encode(),
        &der::seq(&[&der::seq(&[&der::uint(&id.certificate.serial), &revoked_at.encode()])]),
    ]);
    let sig = id.key.sign(pdfcraft_sign::DigestAlg::Sha256, &tbs).unwrap();
    der::seq(&[&tbs, &alg, &der::bit_string(&sig)])
}

#[test]
fn a_revoked_certificate_stays_invalid_when_it_was_also_expired_at_signing() {
    use pdfcraft_sign::dss::{self, Evidence};
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    // Signed in 2040, after the test certificate expired (2036).
    let late = SignOptions { date: "D:20400101120000Z".into(), ..opts() };
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &late).unwrap();
    let t = |year| Time { year, month: 1, day: 1, hour: 0, minute: 0, second: 0 };
    let crl = self_crl(&id, t(2039), t(2041), t(2039));
    let ltv = dss::embed(&open(&signed), &Evidence { certs: Vec::new(), ocsps: Vec::new(), crls: vec![crl] }).unwrap();
    let anchor = id.chain.first().cloned().unwrap_or_else(|| id.certificate.clone());
    for trust in [TrustStore::default(), TrustStore { certs: vec![anchor] }] {
        let s = signatures(&open(&ltv), &ltv, &trust).into_iter().find(|s| s.signed && !s.doc_timestamp).unwrap();
        assert!(s.details.iter().any(|d| d.contains("not valid at the time of signing")), "{:?}", s.details);
        assert_eq!(s.status, Status::Invalid, "{:?}", s.details);
        assert!(s.details.iter().any(|d| d.contains("revoked")), "{:?}", s.details);
    }
}

#[test]
fn an_untrusted_timestamp_does_not_set_the_validation_time() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    // The TSA claims 2027 (inside the signer's validity) but is not trusted; the signer's own
    // clock says 2040, after the certificate expired.
    let tsa = TestTsa {
        id: pkcs12::open(&data("rsa-aes.p12"), "test").unwrap(),
        time: Time { year: 2027, month: 1, day: 1, hour: 0, minute: 0, second: 0 },
    };
    let late = SignOptions { date: "D:20400101120000Z".into(), ..opts() };
    let signed = pdfcraft_sign::sign_with_timestamp(&open(&fixture()), &id, &late, &tsa).unwrap();
    let anchor = id.chain.first().cloned().unwrap_or_else(|| id.certificate.clone());
    let s = signatures(&open(&signed), &signed, &TrustStore { certs: vec![anchor.clone()] }).into_iter().find(|s| s.signed).unwrap();
    assert_ne!(s.status, Status::Valid, "{:?}", s.details);
    assert_eq!(s.timestamp_time, None, "an untrusted token's time is not a trusted time");
    assert!(s.details.iter().any(|d| d.contains("not valid at the time of signing")), "{:?}", s.details);
    assert!(!s.details.iter().any(|d| d.contains("trusted time")), "{:?}", s.details);
    assert!(s.details.iter().any(|d| d.contains("unverified timestamp")), "{:?}", s.details);
    // Trusting the TSA makes its time authoritative: the certificate was valid then.
    let trust = TrustStore { certs: vec![anchor, tsa.id.certificate.clone()] };
    let s = signatures(&open(&signed), &signed, &trust).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.timestamp_time, Some(tsa.time));
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
}

#[test]
fn a_timestamp_token_may_sign_with_a_different_digest_than_its_imprint() {
    let tsa = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let imprint = pdfcraft_sign::DigestAlg::Sha1.digest(&[b"document"]);
    let q = pdfcraft_sign::timestamp::TimestampQuery::new(pdfcraft_sign::DigestAlg::Sha1, imprint.clone()).unwrap();
    let time = Time { year: 2026, month: 10, day: 6, hour: 12, minute: 0, second: 0 };
    let resp =
        pdfcraft_sign::timestamp::respond(&tsa.key, &tsa.certificate, &tsa.chain, pdfcraft_sign::DigestAlg::Sha256, &q, "1.2.3.4", time, 9).unwrap();
    let token = pdfcraft_sign::timestamp::parse_response(&resp, &q).unwrap();
    assert_eq!((token.digest, token.imprint, token.gen_time), (pdfcraft_sign::DigestAlg::Sha1, imprint, time));
}

#[cfg(windows)]
#[test]
fn windows_store_enumeration_and_missing_identity() {
    let listing = pdfcraft_sign::windows::list().unwrap();
    assert_eq!(listing.ids.len(), pdfcraft_sign::windows::identities().unwrap().len());
    // Whatever this machine's store holds, every certificate left out is named and explained.
    for u in &listing.unusable {
        assert!(!u.subject.is_empty() && !u.reason.is_empty() && u.fingerprint.len() == 95, "{u:?}");
    }
    assert!(pdfcraft_sign::windows::find("windows:no such signer").is_err());
}

#[cfg(windows)]
fn powershell(script: &str) -> std::process::Output {
    // Load the certificate provider explicitly in a profile-free Windows PowerShell child.
    let script = format!(
        r#"$ErrorActionPreference='Stop'; Import-Module "$PSHOME\Modules\Microsoft.PowerShell.Security\Microsoft.PowerShell.Security.psd1"; {script}"#
    );
    std::process::Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &script]).output().unwrap()
}

/// Thumbprints of temporary Current User Personal certificates, removed with their keys on drop.
#[cfg(windows)]
struct Certificates(Vec<String>);

#[cfg(windows)]
impl Drop for Certificates {
    fn drop(&mut self) {
        for thumbprint in &self.0 {
            // New-SelfSignedCertificate also leaves a public copy of a self-signed or CA
            // certificate in Intermediate Certification Authorities (CA). -DeleteKey fails on
            // a certificate without a key; the plain removal then runs.
            let out = powershell(&format!(
                "$p='Cert:\\CurrentUser\\My\\{thumbprint}'; try {{ Remove-Item -LiteralPath $p -DeleteKey }} catch {{ Remove-Item -LiteralPath $p }}; $copy='Cert:\\CurrentUser\\CA\\{thumbprint}'; if (Test-Path -LiteralPath $copy) {{ Remove-Item -LiteralPath $copy }}"
            ));
            if !out.status.success() {
                eprintln!("test certificate cleanup failed: {}", String::from_utf8_lossy(&out.stderr));
            }
        }
    }
}

/// Uses only newly created software-backed CNG keys, removed even when signing fails.
#[cfg(windows)]
#[test]
#[ignore = "creates temporary certificates in the Windows Current User Personal store"]
fn signing_with_windows_store_identities() {
    let mut created = Certificates(Vec::new());
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).unwrap();
    let unique: String = random.iter().map(|b| format!("{b:02x}")).collect();
    for algorithm in ["RSA", "ECDSA_nistP256", "ECDSA_nistP384"] {
        let name = format!("PdfCraft Test {unique} {algorithm}");
        let rsa = if algorithm == "RSA" { "-KeyLength 2048" } else { "" };
        let out = powershell(&format!(
            "$ErrorActionPreference='Stop'; $c=New-SelfSignedCertificate -Subject 'CN={name}' -CertStoreLocation 'Cert:\\CurrentUser\\My' -Provider 'Microsoft Software Key Storage Provider' -KeyAlgorithm {algorithm} {rsa} -KeyUsage DigitalSignature -KeyExportPolicy NonExportable; $c.Thumbprint"
        ));
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let thumbprint = String::from_utf8(out.stdout).unwrap().trim().to_string();
        assert_eq!(thumbprint.len(), 40);
        assert!(thumbprint.bytes().all(|b| b.is_ascii_hexdigit()));
        created.0.push(thumbprint);
        let listed = pdfcraft_sign::windows::identities().unwrap();
        let listed_id = listed.iter().find(|id| id.certificate.subject.common_name() == Some(name.as_str())).expect("new identity listed");
        let reference = pdfcraft_sign::windows::reference(&listed_id.certificate);
        let id = pdfcraft_sign::windows::find(&reference).unwrap();
        assert_eq!(pdfcraft_sign::windows::find(&format!("windows:{name}")).unwrap().certificate.raw, id.certificate.raw);
        assert!(id.key.is_external());
        let mut options = opts();
        let now = powershell("(Get-Date).ToUniversalTime().ToString('yyyyMMddHHmmss')");
        assert!(now.status.success());
        options.date = format!("D:{}Z", String::from_utf8(now.stdout).unwrap().trim());
        let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &options).unwrap();
        let validated = signatures(&open(&signed), &signed, &TrustStore { certs: vec![id.certificate.clone()] });
        let signature = validated.iter().find(|s| s.signed).unwrap();
        assert_eq!(signature.status, Status::Valid, "{:?}", signature.details);
        assert_eq!(signature.modification, Modification::None);
        assert_eq!(signature.signer.as_deref(), Some(name.as_str()));
    }
    // A certificate without a private key (issued to someone else, filed under Personal) is
    // reported as such rather than listed or dropped silently (issue #179).
    let ada = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap().certificate;
    let cer = std::env::temp_dir().join(format!("pdfcraft-windows-store-{unique}.cer"));
    std::fs::write(&cer, &ada.raw).unwrap();
    let out = powershell(&format!("(Import-Certificate -FilePath '{}' -CertStoreLocation 'Cert:\\CurrentUser\\My').Thumbprint", cer.display()));
    let _ = std::fs::remove_file(&cer);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    created.0.push(String::from_utf8(out.stdout).unwrap().trim().to_string());
    let listing = pdfcraft_sign::windows::list().unwrap();
    let reported = listing.unusable.iter().find(|u| u.fingerprint == ada.fingerprint()).expect("key-less certificate reported");
    assert!(reported.no_private_key, "{reported:?}");
    assert_eq!(reported.subject, ada.display_name());
    assert!(listing.ids.iter().all(|id| id.certificate.raw != ada.raw));
    let thumbprints = created.0.clone();
    drop(created);
    for thumbprint in thumbprints {
        for store in ["My", "CA"] {
            let out = powershell(&format!("if (Test-Path -LiteralPath 'Cert:\\CurrentUser\\{store}\\{thumbprint}') {{ exit 1 }}"));
            assert!(out.status.success(), "test certificate remains in {store}: {thumbprint}");
        }
    }
}

/// An identity found for signing brings the issuers Windows chains it to (not the root), so the
/// signature validates against the root alone. Uses only temporary software-backed CNG keys.
#[cfg(windows)]
#[test]
#[ignore = "creates temporary certificates in the Windows Current User Personal store"]
fn a_windows_store_identity_signs_with_its_issuers() {
    let mut created = Certificates(Vec::new());
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).unwrap();
    let unique: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let [root, issuer, signer] = ["Root", "Issuer", "Signer"].map(|role| format!("PdfCraft Test {unique} {role}"));
    let mut create = |name: &str, signed_by: Option<&str>, extra: &str| {
        let by = signed_by.map(|t| format!("-Signer (Get-Item -LiteralPath 'Cert:\\CurrentUser\\My\\{t}')")).unwrap_or_default();
        let out = powershell(&format!(
            "$c=New-SelfSignedCertificate -Subject 'CN={name}' -CertStoreLocation 'Cert:\\CurrentUser\\My' -Provider 'Microsoft Software Key Storage Provider' -KeyAlgorithm RSA -KeyLength 2048 -KeyExportPolicy NonExportable {by} {extra}; $c.Thumbprint"
        ));
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let thumbprint = String::from_utf8(out.stdout).unwrap().trim().to_string();
        assert_eq!(thumbprint.len(), 40);
        created.0.push(thumbprint.clone());
        thumbprint
    };
    let ca = "-KeyUsage CertSign -TextExtension @('2.5.29.19={text}CA=true')";
    let root_thumbprint = create(&root, None, ca);
    let issuer_thumbprint = create(&issuer, Some(&root_thumbprint), ca);
    create(&signer, Some(&issuer_thumbprint), "-KeyUsage DigitalSignature");

    let listed = pdfcraft_sign::windows::identities().unwrap();
    let anchor = listed.iter().find(|id| id.certificate.subject.common_name() == Some(root.as_str())).expect("root listed").certificate.clone();
    // Listing builds no chains; finding the identity to sign with does.
    assert!(listed.iter().all(|id| id.chain.is_empty()));
    let id = pdfcraft_sign::windows::find(&format!("windows:{signer}")).unwrap();
    let chain: Vec<_> = id.chain.iter().map(|c| c.subject.common_name()).collect();
    assert_eq!(chain, [Some(issuer.as_str())], "the issuer, without the signer's own certificate or the root");

    let mut options = opts();
    let now = powershell("(Get-Date).ToUniversalTime().ToString('yyyyMMddHHmmss')");
    assert!(now.status.success());
    options.date = format!("D:{}Z", String::from_utf8(now.stdout).unwrap().trim());
    let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &options).unwrap();
    let validated = signatures(&open(&signed), &signed, &TrustStore { certs: vec![anchor] });
    let signature = validated.iter().find(|s| s.signed).unwrap();
    assert_eq!(signature.status, Status::Valid, "{:?}", signature.details);
    assert_eq!(signature.signer.as_deref(), Some(signer.as_str()));
}

/// Documenso-style: an AcroForm signature field `Timestamp_1` whose `/V` is the document
/// timestamp dictionary (`/Type /DocTimeStamp`, `/SubFilter /ETSI.RFC3161`).
fn fixture_timestamped_in_a_field(tsa: &TestTsa) -> Vec<u8> {
    let mut value_num = 0;
    let base = edit_after(&fixture(), |doc| {
        let field = doc.add(Object::Null);
        // `timestamp_document` adds its dictionary next.
        value_num = field.num + 1;
        let at = |num| Object::Ref(pdfcraft_cos::ObjRef { num, generation: 0 });
        let mut widget = pdfcraft_cos::Dict::new();
        for (k, v) in [
            ("Type", Object::name("Annot")),
            ("Subtype", Object::name("Widget")),
            ("FT", Object::name("Sig")),
            ("T", Object::String(PdfString::text("Timestamp_1"))),
            ("Rect", Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(0), Object::Int(0)])),
            ("P", at(3)),
            ("F", Object::Int(132)),
            ("V", at(value_num)),
        ] {
            widget.set(k.as_bytes().to_vec(), v);
        }
        doc.set(field, Object::Dict(widget));
        let root = doc.root().unwrap();
        doc.update_dict(root, |c| {
            if let Some(Object::Dict(form)) = c.get_mut(b"AcroForm")
                && let Some(Object::Array(fields)) = form.get_mut(b"Fields")
            {
                fields.push(Object::Ref(field));
            }
        })
        .unwrap();
    });
    let stamped = pdfcraft_sign::timestamp_document(&open(&base), tsa, "D:20261006120000Z").unwrap();
    let held = open(&stamped).get(pdfcraft_cos::ObjRef { num: value_num, generation: 0 });
    assert_eq!(held.as_dict().and_then(|d| d.name(b"Type")), Some(&b"DocTimeStamp"[..]), "the field holds the timestamp");
    stamped
}

#[test]
fn a_document_timestamp_held_by_a_signature_field_is_checked_as_a_timestamp() {
    let tsa = TestTsa {
        id: pkcs12::open(&data("rsa-aes.p12"), "test").unwrap(),
        time: Time { year: 2026, month: 10, day: 6, hour: 12, minute: 0, second: 0 },
    };
    let stamped = fixture_timestamped_in_a_field(&tsa);
    let trust = TrustStore { certs: vec![tsa.id.certificate.clone()] };
    let all = signatures(&open(&stamped), &stamped, &trust);
    let stamps: Vec<_> = all.iter().filter(|s| s.signed).collect();
    assert_eq!(stamps.len(), 1, "listed once, not as field and as standalone: {:?}", all.iter().map(|s| &s.field).collect::<Vec<_>>());
    let s = stamps[0];
    assert_eq!(s.field, "Timestamp_1");
    assert!(s.doc_timestamp && s.timestamp);
    assert_eq!(s.sub_filter.as_deref(), Some("ETSI.RFC3161"));
    assert_eq!(s.timestamp_time, Some(tsa.time));
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    // Without trusting the TSA the stamp is intact but unknown, never "altered".
    let s = signatures(&open(&stamped), &stamped, &TrustStore::default()).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.status, Status::Unknown, "{:?}", s.details);
    assert!(!s.details.iter().any(|d| d.contains("altered or corrupted")), "{:?}", s.details);
    // A changed byte in what it covers is still caught, and reported as the timestamp's.
    let mut tampered = stamped.clone();
    let i = tampered.windows(13).position(|w| w == b"Contract text").unwrap();
    tampered[i] = b'K';
    let s = signatures(&open(&tampered), &tampered, &trust).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.status, Status::Invalid);
    assert!(s.details.iter().any(|d| d.contains("since the timestamp was applied")), "{:?}", s.details);
}

/// A trusted timestamp authority whose certificate was not yet valid when it stamped proves
/// nothing: the field timestamp stays Unknown, never Valid.
#[test]
fn a_field_timestamp_from_a_trusted_tsa_outside_its_validity_is_not_valid() {
    let tsa = TestTsa {
        id: pkcs12::open(&data("rsa-aes.p12"), "test").unwrap(),
        // Before the TSA certificate's validity (it starts in October 2026).
        time: Time { year: 2020, month: 1, day: 1, hour: 0, minute: 0, second: 0 },
    };
    let stamped = fixture_timestamped_in_a_field(&tsa);
    let trust = TrustStore { certs: vec![tsa.id.certificate.clone()] };
    let s = signatures(&open(&stamped), &stamped, &trust).into_iter().find(|s| s.signed).unwrap();
    assert!(s.doc_timestamp);
    assert_eq!(s.status, Status::Unknown, "{:?}", s.details);
    assert!(s.details.iter().any(|d| d.contains("was not valid at the time of timestamping")), "{:?}", s.details);
    assert!(!s.details.iter().any(|d| d.contains("its authority is trusted")), "{:?}", s.details);
}

#[test]
fn a_standalone_timestamp_typed_sig_by_an_older_writer_is_still_a_timestamp() {
    let tsa = TestTsa {
        id: pkcs12::open(&data("rsa-aes.p12"), "test").unwrap(),
        time: Time { year: 2026, month: 10, day: 6, hour: 12, minute: 0, second: 0 },
    };
    let stamped = pdfcraft_sign::timestamp_document(&open(&fixture()), &tsa, "D:20261006120000Z").unwrap();
    let s = signatures(&open(&stamped), &stamped, &TrustStore::default()).into_iter().find(|s| s.doc_timestamp).unwrap();
    assert_eq!(s.status, Status::Unknown, "{:?}", s.details);
    // The same bytes with `/Type /Sig` in place of `/Type /DocTimeStamp` (padded to the same
    // length): the imprint no longer matches, but it is still found as a timestamp.
    let mut old = stamped.clone();
    let at = old.windows(19).position(|w| w == b"/Type /DocTimeStamp").unwrap();
    old[at..at + 19].copy_from_slice(b"/Type /Sig         ");
    let s = signatures(&open(&old), &old, &TrustStore::default()).into_iter().find(|s| s.doc_timestamp).expect("found as a timestamp");
    assert_eq!(s.status, Status::Invalid);
}

/// A legacy adbe.x509.rsa_sha1 signature whose certificate carries an RSA key the verifier
/// rejects (public exponent 1) is Invalid, not "can't check yet".
#[test]
fn a_legacy_x509_signature_with_a_malformed_rsa_key_is_invalid() {
    let pdf = data("x509-rsa-sha1.pdf");
    let at = pdf.windows(10).position(|w| w == b"0203010001").expect("the certificate's exponent 65537");
    let mut bad = pdf.clone();
    bad[at..at + 10].copy_from_slice(b"0203000001");
    let s = signatures(&open(&bad), &bad, &TrustStore::default()).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.sub_filter.as_deref(), Some("adbe.x509.rsa_sha1"));
    assert_eq!(s.status, Status::Invalid, "{:?}", s.details);
    assert!(!s.details.iter().any(|d| d.contains("can't check this signature yet")), "{:?}", s.details);
}

#[test]
fn validates_the_legacy_adbe_x509_rsa_sha1_format() {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    // tests/data/x509-rsa-sha1.pdf: made with OpenSSL (README.md) as Acrobat 4-era signers wrote it.
    let pdf = data("x509-rsa-sha1.pdf");
    let s = signatures(&open(&pdf), &pdf, &TrustStore::default()).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.sub_filter.as_deref(), Some("adbe.x509.rsa_sha1"));
    assert_eq!(s.status, Status::Unknown, "intact, identity not trusted: {:?}", s.details);
    assert_eq!(s.signer.as_deref(), Some("Test Signer RSA"));
    assert_eq!(s.algorithm.as_deref(), Some("RSA 2048-bit with SHA-1"));
    assert_eq!(s.modification, Modification::None);
    // SHA-1 still validates, but the panel says it is weak.
    let weak = "This signature uses SHA-1, a weak hash algorithm: collisions in it have been demonstrated. It still validates because many documents signed years ago use it, but it should not be relied on.";
    assert_eq!(s.details.iter().filter(|d| *d == weak).count(), 1, "{:?}", s.details);
    assert!(s.details.iter().any(|d| d.contains("legacy adbe.x509.rsa_sha1 format")), "{:?}", s.details);
    let trusted = TrustStore { certs: vec![id.certificate.clone()] };
    let s = signatures(&open(&pdf), &pdf, &trusted).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    // One changed byte inside the signed range.
    let mut bad = pdf.clone();
    let i = bad.windows(8).position(|w| w == b"(Legacy)").unwrap() + 1;
    bad[i] = b'l';
    let s = signatures(&open(&bad), &bad, &trusted).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.status, Status::Invalid, "{:?}", s.details);
}

/// `fixture()` protected with an empty user password (opens without asking, like most
/// "secured" forms) and an owner password, then saved.
fn protected(alg: pdfcraft_cos::Algorithm, permissions: i32) -> Vec<u8> {
    let mut doc = open(&fixture());
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: alg,
        user_password: "",
        owner_password: "owner",
        permissions,
        encrypt_metadata: true,
        seed: [17; 32],
    })
    .unwrap();
    pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap()
}

fn open_pw(b: &[u8], pw: Option<&str>) -> Document {
    Document::open_with_password(Arc::new(b.to_vec()), pw).unwrap()
}

/// Filling in forms and signing existing fields only (bits 3, 7–10): what Acrobat's "Filling
/// in form fields, and signing existing signature fields" writes.
const FILL_AND_SIGN_ONLY: i32 = -3132;

const ALL_ALGORITHMS: [pdfcraft_cos::Algorithm; 4] =
    [pdfcraft_cos::Algorithm::Rc4_40, pdfcraft_cos::Algorithm::Rc4_128, pdfcraft_cos::Algorithm::Aes128, pdfcraft_cos::Algorithm::Aes256];

#[test]
fn encrypted_documents_are_signed_with_an_unencrypted_contents() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let id2 = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone(), id2.certificate.clone()] };
    for alg in ALL_ALGORITHMS {
        let base = protected(alg, -1);
        let doc = open_pw(&base, None);
        assert_eq!(doc.security().unwrap().auth(), pdfcraft_cos::Auth::User);
        let signed = pdfcraft_sign::sign(&doc, &id, &opts()).unwrap();
        assert!(signed.starts_with(&base), "{alg:?}: an incremental update");
        let added = String::from_utf8_lossy(&signed[base.len()..]);
        assert!(added.contains("/Encrypt") && added.contains("/ID"), "{alg:?}: the update keeps the file's security");
        assert!(!added.contains("I approve") && !added.contains("London"), "{alg:?}: the new strings are encrypted");
        assert!(added.contains("/Contents <30"), "{alg:?}: the CMS is written in the clear");
        let reopened = open_pw(&signed, None);
        assert!(reopened.security().is_some());
        let sigs = signatures(&reopened, &signed, &trust);
        let s = sigs.iter().find(|s| s.signed).unwrap();
        assert_eq!(s.status, Status::Valid, "{alg:?}: {:?}", s.details);
        assert_eq!(s.modification, Modification::None);
        assert_eq!((s.reason.as_deref(), s.location.as_deref()), (Some("I approve this document"), Some("London")), "{alg:?}");
        assert!(s.visible && s.revision == 2 && s.signed_len == signed.len());
        // The parsed /Contents is the CMS itself, not "decrypted" noise.
        let cms = reopened
            .scan_objects()
            .filter_map(|(_, o)| o.as_dict().filter(|d| d.name(b"Type") == Some(b"Sig")).and_then(|d| d.get(b"Contents").cloned()))
            .next()
            .unwrap();
        assert_eq!(cms.as_string().unwrap().bytes.first(), Some(&0x30), "{alg:?}");
        // Counter-signing the existing field keeps the first signature valid.
        let both = pdfcraft_sign::sign(&reopened, &id2, &SignOptions { field: Some("Approval".into()), ..opts() }).unwrap();
        let sigs = signatures(&open_pw(&both, None), &both, &trust);
        let first = sigs.iter().find(|s| s.field == "Signature1").unwrap();
        assert_eq!(first.status, Status::Valid, "{alg:?}: {:?}", first.details);
        assert_eq!(first.modification, Modification::Allowed(vec!["signature".into()]), "{alg:?}: {:?}", first.details);
        let second = sigs.iter().find(|s| s.field == "Approval").unwrap();
        assert_eq!((second.status, second.revision), (Status::Valid, 3), "{alg:?}: {:?}", second.details);
        // The page text is still encrypted, and tampering with its ciphertext breaks both.
        assert!(!String::from_utf8_lossy(&both).contains("Contract text"));
        let mut tampered = both.clone();
        let at = base.len() / 2;
        tampered[at] ^= 1;
        let tampered_sigs = signatures(&open_pw(&tampered, None), &tampered, &trust);
        assert!(tampered_sigs.iter().filter(|s| s.signed).all(|s| s.status == Status::Invalid), "{alg:?}");
    }
}

#[test]
fn encrypted_documents_are_signed_only_as_their_permissions_allow() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    let base = protected(pdfcraft_cos::Algorithm::Aes256, FILL_AND_SIGN_ONLY);
    let user = open_pw(&base, None);
    let refused = |r: Result<Vec<u8>, SignError>, what: &str| match r {
        Err(SignError::Pdf(m)) => assert!(m.contains(what) && m.contains("permissions password"), "{m}"),
        other => panic!("expected a refusal, got {:?}", other.map(|b| b.len())),
    };
    // A new field is a form change the settings don't allow.
    refused(pdfcraft_sign::sign(&user, &id, &opts()), "adding new signature fields");
    // So is certifying, even in an existing field.
    refused(pdfcraft_sign::sign(&user, &id, &SignOptions { field: Some("Approval".into()), certify: Some(2), ..opts() }), "certifying");
    // Signing the existing field is allowed.
    let signed = pdfcraft_sign::sign(&user, &id, &SignOptions { field: Some("Approval".into()), ..opts() }).unwrap();
    let s = signatures(&open_pw(&signed, None), &signed, &trust).into_iter().find(|s| s.signed).unwrap();
    assert_eq!((s.field.as_str(), s.status), ("Approval", Status::Valid), "{:?}", s.details);
    // Settings that allow no form filling refuse even that (print only).
    let print_only = open_pw(&protected(pdfcraft_cos::Algorithm::Aes256, -3900), None);
    refused(pdfcraft_sign::sign(&print_only, &id, &SignOptions { field: Some("Approval".into()), ..opts() }), "signing");
    // The owner may do anything: a new, certifying signature.
    let owner = open_pw(&base, Some("owner"));
    let certified = pdfcraft_sign::sign(&owner, &id, &SignOptions { certify: Some(2), ..opts() }).unwrap();
    let s = signatures(&open_pw(&certified, None), &certified, &trust).into_iter().find(|s| s.signed).unwrap();
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    assert_eq!(s.certify, Some(2), "{:?}", s.details);
}

/// A security change that hasn't been saved would turn the signing save into a full rewrite that
/// applies or removes protection: signing refuses until it is saved.
#[test]
fn signing_waits_for_a_pending_security_change_to_be_saved() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let mut owner = open_pw(&protected(pdfcraft_cos::Algorithm::Aes256, FILL_AND_SIGN_ONLY), Some("owner"));
    owner.remove_encryption();
    match pdfcraft_sign::sign(&owner, &id, &opts()) {
        Err(SignError::Pdf(m)) => assert!(m.contains("save the document's new security settings"), "{m}"),
        other => panic!("expected a refusal, got {:?}", other.map(|b| b.len())),
    }
}

#[test]
fn encrypted_documents_take_a_document_timestamp() {
    let tsa = TestTsa {
        id: pkcs12::open(&data("rsa-aes.p12"), "test").unwrap(),
        time: Time { year: 2026, month: 10, day: 6, hour: 12, minute: 0, second: 0 },
    };
    let base = protected(pdfcraft_cos::Algorithm::Aes128, FILL_AND_SIGN_ONLY);
    let stamped = pdfcraft_sign::timestamp_document(&open_pw(&base, None), &tsa, "D:20261006120000Z").unwrap();
    assert!(stamped.starts_with(&base));
    let s = signatures(&open_pw(&stamped, None), &stamped, &TrustStore { certs: vec![tsa.id.certificate.clone()] })
        .into_iter()
        .find(|s| s.doc_timestamp)
        .unwrap();
    assert_eq!((s.status, s.timestamp_time), (Status::Valid, Some(tsa.time)), "{:?}", s.details);
    let print_only = open_pw(&protected(pdfcraft_cos::Algorithm::Aes128, -3900), None);
    assert!(matches!(pdfcraft_sign::timestamp_document(&print_only, &tsa, "D:20261006120000Z"), Err(SignError::Pdf(_))));
}
