//! The CCA India trust set end to end, with a synthetic chain shaped like an e-Aadhaar signature
//! (no real Aadhaar data, and nobody has the CCA's keys): a root, a licensed CA under it, a
//! sub-CA whose basicConstraints are not critical, and a UIDAI-like signer, all RSA-2048 with
//! SHA-256; an `adbe.pkcs7.detached` CMS with no signing-time attribute and no timestamp, no
//! `/DSS`, in a password-protected PDF. The synthetic root stands in for the embedded roots on the
//! test's thread ([`super::tests::CCA_INDIA_STAND_IN`]); everything else is the real code path.

use std::sync::Arc;

use pdfcraft_cos::{Algorithm, Document, NewEncryption, SaveOptions};

use super::tests::CCA_INDIA_STAND_IN;
use crate::cms::{DATA, SIGNED_DATA, SignedData};
use crate::der::{self, Time, tag};
use crate::keys::{DigestAlg, PrivateKey};
use crate::pdf::{SignOptions, Status, TrustSource, TrustStore, list as signatures};
use crate::pkcs12::DigitalId;
use crate::x509::{Certificate, Name};

const PASSWORD: &str = "SYNTH2026";

/// India's certificate policy arc (CCA's Certificate Policy, OID 2.16.356.100.2) and the class of
/// the policy e-Aadhaar signer certificates carry.
const POLICY_INDIA_PKI: &str = "2.16.356.100.2";
const POLICY_SIGNER: &str = "2.16.356.100.2.2";

/// keyUsage bits as the BIT STRING's first byte.
const DIGITAL_SIGNATURE: u8 = 0x80;
const NON_REPUDIATION: u8 = 0x40;
const KEY_CERT_SIGN: u8 = 0x04;
const CRL_SIGN: u8 = 0x02;

fn time(year: u32) -> Time {
    Time { year, month: 6, day: 1, hour: 12, minute: 0, second: 0 }
}

fn ext(oid: &str, critical: bool, value: &[u8]) -> Vec<u8> {
    if critical { der::seq(&[&der::oid(oid), &der::boolean(true), &der::octets(value)]) } else { der::seq(&[&der::oid(oid), &der::octets(value)]) }
}

fn basic_constraints(critical: bool, path_len: u64) -> Vec<u8> {
    ext("2.5.29.19", critical, &der::seq(&[&der::boolean(true), &der::int(path_len)]))
}

/// keyUsage, critical, with the bits of `bits` (unused trailing bits counted, as DER asks).
fn key_usage(bits: u8) -> Vec<u8> {
    ext("2.5.29.15", true, &der::tlv(tag::BIT_STRING, &[bits.trailing_zeros() as u8, bits]))
}

fn policies(oid: &str) -> Vec<u8> {
    ext("2.5.29.32", false, &der::seq(&[&der::seq(&[&der::oid(oid)])]))
}

fn extended_key_usage(oids: &[&str]) -> Vec<u8> {
    let encoded: Vec<Vec<u8>> = oids.iter().map(|o| der::oid(o)).collect();
    let refs: Vec<&[u8]> = encoded.iter().map(Vec::as_slice).collect();
    ext("2.5.29.37", false, &der::seq(&refs))
}

struct Party {
    name: Name,
    key: PrivateKey,
    cert: Certificate,
}

/// A certificate for a fresh RSA-2048 key, signed with SHA-256 by `issuer` (or by itself).
fn issue(cn: &str, issuer: Option<&Party>, valid: (u32, u32), extensions: &[Vec<u8>]) -> Party {
    let key = PrivateKey::generate_rsa(2048).unwrap();
    let name = Name::build(cn, "", "Synthetic India PKI Test", "", "IN");
    let signer = issuer.map_or(&key, |i| &i.key);
    let issuer_name = issuer.map_or(&name, |i| &i.name);
    let sig_alg = signer.signature_algorithm(DigestAlg::Sha256);
    let exts: Vec<&[u8]> = extensions.iter().map(Vec::as_slice).collect();
    let tbs = der::seq(&[
        &der::explicit(0, &der::int(2)),
        &der::uint(&[0x11, cn.len() as u8]),
        &sig_alg,
        &issuer_name.raw,
        &der::seq(&[&time(valid.0).encode(), &time(valid.1).encode()]),
        &name.raw,
        &key.public_key().spki(),
        &der::explicit(3, &der::seq(&exts)),
    ]);
    let signature = signer.sign(DigestAlg::Sha256, &tbs).unwrap();
    let cert = Certificate::parse(&der::seq(&[&tbs, &sig_alg, &der::bit_string(&signature)])).unwrap();
    Party { name, key, cert }
}

/// Root → licensed CA (pathLen 1) → sub-CA (pathLen 0, basicConstraints not critical) → signer.
struct Hierarchy {
    root: Party,
    ca: Party,
    sub_ca: Party,
    signer: Party,
}

fn root(cn: &str) -> Party {
    issue(cn, None, (2020, 2042), &[ext("2.5.29.19", true, &der::seq(&[&der::boolean(true)])), key_usage(KEY_CERT_SIGN | CRL_SIGN)])
}

fn hierarchy_under(root: Party) -> Hierarchy {
    let ca = issue(
        "Synthetic Licensed CA",
        Some(&root),
        (2021, 2040),
        &[basic_constraints(true, 1), key_usage(KEY_CERT_SIGN | CRL_SIGN), policies(POLICY_INDIA_PKI)],
    );
    let sub_ca = issue(
        "Synthetic Sub-CA for Class 3 Organisation",
        Some(&ca),
        (2022, 2035),
        &[basic_constraints(false, 0), key_usage(KEY_CERT_SIGN | CRL_SIGN), policies(POLICY_INDIA_PKI)],
    );
    let signer = issue(
        "Synthetic Identity Authority Signer",
        Some(&sub_ca),
        (2025, 2028),
        &[
            key_usage(DIGITAL_SIGNATURE | NON_REPUDIATION),
            extended_key_usage(&["1.3.6.1.5.5.7.3.4", "1.3.6.1.4.1.311.10.3.12", "1.2.840.113583.1.1.5"]),
            policies(POLICY_SIGNER),
        ],
    );
    Hierarchy { root, ca, sub_ca, signer }
}

/// A one-page PDF protected with a user password, as e-Aadhaar downloads are.
fn protected_pdf() -> Vec<u8> {
    let content = b"BT /F1 12 Tf 72 720 Td (Synthetic identity document) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_string(),
        format!("<< /Length {} >>\nstream\n{}\nendstream", content.len(), String::from_utf8_lossy(content)),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    let mut doc = Document::open(Arc::new(bytes)).unwrap();
    doc.set_encryption(&NewEncryption {
        algorithm: Algorithm::Aes256,
        user_password: PASSWORD,
        owner_password: "synthetic-owner",
        permissions: -1,
        encrypt_metadata: true,
        seed: [7; 32],
    })
    .unwrap();
    pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap()
}

fn open(bytes: &[u8]) -> Document {
    Document::open_with_password(Arc::new(bytes.to_vec()), Some(PASSWORD)).unwrap()
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Sign `protected_pdf()` as `h.signer`, with `/M` = `date`, the way e-Aadhaar is signed:
/// `/SubFilter /adbe.pkcs7.detached`, a CMS whose signed attributes are only content-type and
/// message-digest (no signing-time, no ESS signing-certificate), no timestamp, no `/DSS`.
/// The issuers (not the root) are embedded.
fn sign_like_eaadhaar(h: Hierarchy, date: &str) -> (Vec<u8>, Hierarchy) {
    let embedded = vec![h.sub_ca.cert.clone(), h.ca.cert.clone()];
    let Party { name, key, cert } = h.signer;
    let id = DigitalId { key, certificate: cert, chain: embedded.clone(), friendly_name: None };
    let doc = open(&protected_pdf());
    let opts = SignOptions { page: 0, rect: Some([400.0, 50.0, 580.0, 110.0]), date: date.into(), ..SignOptions::default() };
    let mut pdf = crate::pdf::sign(&doc, &id, &opts).unwrap();
    let h = Hierarchy { signer: Party { name, key: id.key, cert: id.certificate }, ..h };
    // The same length, so the byte range stays where it is; the signature is made again below.
    let at = find(&pdf, b"/ETSI.CAdES.detached").expect("the signature dictionary is written in the clear");
    pdf[at..at + 20].copy_from_slice(b"/adbe.pkcs7.detached");
    let br = find(&pdf, b"/ByteRange").unwrap();
    let open_bracket = br + pdf[br..].iter().position(|&b| b == b'[').unwrap();
    let close_bracket = open_bracket + pdf[open_bracket..].iter().position(|&b| b == b']').unwrap();
    let range: Vec<usize> = String::from_utf8_lossy(&pdf[open_bracket + 1..close_bracket]).split_whitespace().map(|n| n.parse().unwrap()).collect();
    let [a, b, c, d] = range[..] else { panic!("{range:?}") };
    let digest = DigestAlg::Sha256.digest(&[&pdf[a..a + b], &pdf[c..c + d]]);
    let cms = pkcs7_detached(&h.signer, &embedded, &digest);
    let hex: String = cms.iter().map(|x| format!("{x:02X}")).collect();
    let slot = &mut pdf[b + 1..c - 1];
    assert!(hex.len() <= slot.len());
    slot.fill(b'0');
    slot[..hex.len()].copy_from_slice(hex.as_bytes());
    (pdf, h)
}

/// A detached CMS SignedData as e-Aadhaar's: signed attributes content-type and message-digest only.
fn pkcs7_detached(signer: &Party, chain: &[Certificate], digest: &[u8]) -> Vec<u8> {
    let attribute = |o: &str, value: &[u8]| der::seq(&[&der::oid(o), &der::set_of(&[value])]);
    let attrs = [attribute("1.2.840.113549.1.9.3", &der::oid(DATA)), attribute("1.2.840.113549.1.9.4", &der::octets(digest))];
    let refs: Vec<&[u8]> = attrs.iter().map(Vec::as_slice).collect();
    let set = der::set_of(&refs);
    let signature = signer.key.sign(DigestAlg::Sha256, &set).unwrap();
    let mut signed_attrs = set;
    signed_attrs[0] = tag::ctx(0);
    let cert = &signer.cert;
    let sid = der::seq(&[&cert.issuer.raw, &der::uint(&cert.serial)]);
    let info = der::seq(&[
        &der::int(1),
        &sid,
        &DigestAlg::Sha256.algorithm(),
        &signed_attrs,
        &signer.key.signature_algorithm(DigestAlg::Sha256),
        &der::octets(&signature),
    ]);
    let mut certs: Vec<&[u8]> = vec![&cert.raw];
    certs.extend(chain.iter().map(|c| c.raw.as_slice()));
    let signed_data = der::seq(&[
        &der::int(1),
        &der::set_of(&[&DigestAlg::Sha256.algorithm()]),
        &der::seq(&[&der::oid(DATA)]),
        &der::tlv(tag::ctx(0), &certs.concat()),
        &der::set_of(&[&info]),
    ]);
    der::seq(&[&der::oid(SIGNED_DATA), &der::explicit(0, &signed_data)])
}

fn validate(pdf: &[u8], trust: &TrustStore) -> crate::pdf::SignatureInfo {
    signatures(&open(pdf), pdf, trust).into_iter().find(|s| s.signed).unwrap()
}

/// Let `roots` stand in for the embedded CCA India roots on this thread.
fn stand_in(roots: &[&Party]) {
    let certs: Vec<Certificate> = roots.iter().map(|p| p.cert.clone()).collect();
    CCA_INDIA_STAND_IN.with(|r| r.set(Some(Box::leak(certs.into_boxed_slice()))));
}

const NOT_CHECKED: &str = "Revocation was not checked";
const SIGNED_AT: &str = "D:20260601120000+05'30'";

#[test]
fn an_eaadhaar_shaped_signature_is_valid_only_under_the_cca_india_set() {
    let h = hierarchy_under(root("Synthetic CCA Root"));
    stand_in(&[&h.root]);
    let (pdf, h) = sign_like_eaadhaar(h, SIGNED_AT);

    // What the file is: encrypted, adbe.pkcs7.detached, no signing time or timestamp in the CMS,
    // no /DSS, the issuers embedded but not the root.
    let doc = open(&pdf);
    assert!(doc.security().is_some());
    let root_dict = doc.get(doc.root().unwrap()).as_dict().cloned().unwrap();
    assert!(root_dict.get(b"DSS").is_none());
    let s = validate(&pdf, &TrustStore::default());
    assert_eq!(s.sub_filter.as_deref(), Some("adbe.pkcs7.detached"));
    let cms = doc
        .scan_objects()
        .filter_map(|(_, o)| o.as_dict().filter(|d| d.name(b"Type") == Some(b"Sig")).and_then(|d| d.get(b"Contents").cloned()))
        .next()
        .unwrap();
    let cms = SignedData::parse(&cms.as_string().unwrap().bytes).unwrap();
    assert_eq!((cms.signer.signing_time, cms.signer.timestamp, cms.signer.signing_certificate), (None, false, false));
    assert_eq!(cms.certificates.len(), 3);

    // Off by default: the signature is intact, but nobody vouches for the signer.
    assert!(!TrustStore::default().cca_india);
    assert_eq!(s.status, Status::Unknown, "{:?}", s.details);
    assert_eq!(s.modification, crate::pdf::Modification::None);
    assert!(s.details.iter().any(|d| d.contains("identity is unknown")), "{:?}", s.details);

    // With the set on: valid, the set is named, the signing time is /M, and revocation is said
    // not to have been checked (there is no evidence and PdfCraft stays offline).
    let cca = TrustStore { cca_india: true, ..TrustStore::default() };
    let s = validate(&pdf, &cca);
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    assert!(s.details.iter().any(|d| d.contains("Synthetic CCA Root") && d.contains("CCA India trust set")), "{:?}", s.details);
    assert!(s.details.iter().any(|d| d.starts_with(NOT_CHECKED)), "{:?}", s.details);
    assert_eq!(s.signing_time, Some(Time { year: 2026, month: 6, day: 1, hour: 6, minute: 30, second: 0 }));
    let names: Vec<String> = s.chain.iter().map(Certificate::display_name).collect();
    assert_eq!(
        names,
        ["Synthetic Identity Authority Signer", "Synthetic Sub-CA for Class 3 Organisation", "Synthetic Licensed CA", "Synthetic CCA Root"]
    );
    assert_eq!(cca.source_of(&h.root.cert), Some(TrustSource::CcaIndia));
    assert!(s.algorithm.as_deref().is_some_and(|a| a.contains("RSA") && a.contains("SHA-256")), "{:?}", s.algorithm);

    // The other sets don't vouch for it.
    let others = TrustStore { builtin_roots: true, ..TrustStore::default() };
    assert_eq!(validate(&pdf, &others).status, Status::Unknown);

    // The user's own trust in the root reports the same verdict without naming the set.
    let user = TrustStore { certs: vec![h.root.cert.clone()], ..TrustStore::default() };
    let s = validate(&pdf, &user);
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    assert!(!s.details.iter().any(|d| d.contains("CCA India")), "{:?}", s.details);
    assert!(s.details.iter().any(|d| d.starts_with(NOT_CHECKED)), "{:?}", s.details);
}

#[test]
fn a_changed_byte_of_signed_content_makes_it_invalid() {
    let h = hierarchy_under(root("Synthetic CCA Root"));
    stand_in(&[&h.root]);
    let (pdf, _) = sign_like_eaadhaar(h, SIGNED_AT);
    let cca = TrustStore { cca_india: true, ..TrustStore::default() };
    assert_eq!(validate(&pdf, &cca).status, Status::Valid);
    // A byte inside the file's first (encrypted) stream, the page content: signed bytes.
    let mut tampered = pdf.clone();
    let original = protected_pdf().len();
    let stream = find(&tampered, b"stream").unwrap() + "stream\n".len() + 5;
    assert!(stream < original, "in the file as it was before signing: {stream} of {original}");
    tampered[stream] ^= 0x01;
    let s = validate(&tampered, &cca);
    assert_eq!(s.status, Status::Invalid, "{:?}", s.details);
}

#[test]
fn a_chain_under_another_root_stays_unknown_with_the_set_on() {
    let cca = TrustStore { cca_india: true, ..TrustStore::default() };
    let genuine = root("Synthetic CCA Root");
    // Same name as the root in the set, a different key: a look-alike hierarchy.
    let lookalike = hierarchy_under(root("Synthetic CCA Root"));
    assert_eq!(lookalike.root.cert.subject.raw, genuine.cert.subject.raw);
    stand_in(&[&genuine]);
    let s = validate(&sign_like_eaadhaar(lookalike, SIGNED_AT).0, &cca);
    assert_eq!(s.status, Status::Unknown, "{:?}", s.details);
    assert!(!s.details.iter().any(|d| d.contains("CCA India")), "{:?}", s.details);

    // And with the real embedded CCA roots, a synthetic hierarchy is nobody's.
    CCA_INDIA_STAND_IN.with(|r| r.set(None));
    let h = hierarchy_under(root("Synthetic CCA Root"));
    let s = validate(&sign_like_eaadhaar(h, SIGNED_AT).0, &cca);
    assert_eq!(s.status, Status::Unknown, "{:?}", s.details);
}

#[test]
fn the_signer_certificate_has_to_be_valid_when_it_signed() {
    let h = hierarchy_under(root("Synthetic CCA Root"));
    stand_in(&[&h.root]);
    let cca = TrustStore { cca_india: true, ..TrustStore::default() };
    // The signer's certificate runs 2025 to 2028; a signature claiming 2030 is not valid.
    let s = validate(&sign_like_eaadhaar(h, "D:20300601120000Z").0, &cca);
    assert_eq!(s.status, Status::Unknown, "{:?}", s.details);
    assert!(s.details.iter().any(|d| d.contains("not valid at the time of signing")), "{:?}", s.details);
}
