//! Chain building: which certificates may vouch for which (RFC 5280 §6.1, the parts that matter
//! for trust). Certificates are issued in-process with chosen extensions, so every refusal is
//! tested on its own.

use std::sync::Arc;

use pdfcraft_cos::Document;
use pdfcraft_sign::der::{self, Time, tag};
use pdfcraft_sign::pkcs12::DigitalId;
use pdfcraft_sign::x509::{Certificate, Name, build_chain, build_chain_noted};
use pdfcraft_sign::{PrivateKey, SignOptions, Status, TrustStore, signatures};

fn time(year: u32) -> Time {
    Time { year, month: 6, day: 1, hour: 12, minute: 0, second: 0 }
}

fn ext(oid: &str, value: &[u8]) -> Vec<u8> {
    der::seq(&[&der::oid(oid), &der::boolean(true), &der::octets(value)])
}

/// basicConstraints: `cA` (omitted when false, as DER does) and an optional `pathLenConstraint`.
fn basic_constraints(ca: bool, path_len: Option<u64>) -> Vec<u8> {
    let mut parts: Vec<Vec<u8>> = Vec::new();
    if ca {
        parts.push(der::boolean(true));
    }
    if let Some(n) = path_len {
        parts.push(der::int(n));
    }
    let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    ext("2.5.29.19", &der::seq(&refs))
}

const DIGITAL_SIGNATURE: u8 = 0x80;
const KEY_CERT_SIGN: u8 = 0x04;

fn key_usage(bits: u8) -> Vec<u8> {
    ext("2.5.29.15", &der::tlv(tag::BIT_STRING, &[1, bits]))
}

struct Party {
    name: Name,
    key: PrivateKey,
    cert: Certificate,
}

/// A certificate for a fresh key, signed by `issuer` (or by itself).
fn issue(cn: &str, issuer: Option<&Party>, valid: (u32, u32), extensions: &[Vec<u8>]) -> Party {
    let key = PrivateKey::generate_p256().unwrap();
    let name = Name::build(cn, "", "Chain Tests", "", "US");
    let signer = issuer.map_or(&key, |i| &i.key);
    let issuer_name = issuer.map_or(&name, |i| &i.name);
    let alg = signer.preferred_digest();
    let sig_alg = signer.signature_algorithm(alg);
    let exts: Vec<&[u8]> = extensions.iter().map(Vec::as_slice).collect();
    let mut fields: Vec<Vec<u8>> = Vec::new();
    // v3 when there are extensions; with none, a v1 certificate (no version field), as old roots.
    if !exts.is_empty() {
        fields.push(der::explicit(0, &der::int(2)));
    }
    fields.extend([
        der::uint(&[1 + cn.len() as u8]),
        sig_alg.clone(),
        issuer_name.raw.clone(),
        der::seq(&[&time(valid.0).encode(), &time(valid.1).encode()]),
        name.raw.clone(),
        key.public_key().spki(),
    ]);
    if !exts.is_empty() {
        fields.push(der::explicit(3, &der::seq(&exts)));
    }
    let refs: Vec<&[u8]> = fields.iter().map(Vec::as_slice).collect();
    let tbs = der::seq(&refs);
    let signature = signer.sign(alg, &tbs).unwrap();
    let cert = Certificate::parse(&der::seq(&[&tbs, &sig_alg, &der::bit_string(&signature)])).unwrap();
    Party { name, key, cert }
}

const FOREVER: (u32, u32) = (2020, 2040);

fn root() -> Party {
    issue("Test Root", None, FOREVER, &[basic_constraints(true, None), key_usage(KEY_CERT_SIGN)])
}

fn ca(cn: &str, under: &Party, path_len: Option<u64>) -> Party {
    issue(cn, Some(under), FOREVER, &[basic_constraints(true, path_len), key_usage(KEY_CERT_SIGN)])
}

fn end_entity(cn: &str, under: &Party) -> Party {
    issue(cn, Some(under), FOREVER, &[basic_constraints(false, None), key_usage(DIGITAL_SIGNATURE)])
}

fn names(chain: &[&Certificate]) -> Vec<String> {
    chain.iter().map(|c| c.subject.common_name().unwrap_or_default().to_string()).collect()
}

#[test]
fn a_proper_chain_is_built() {
    let r = root();
    let i = ca("Intermediate", &r, None);
    let l = end_entity("Leaf", &i);
    let pool = [i.cert.clone(), r.cert.clone()];
    assert_eq!(names(&build_chain(&l.cert, &pool, None)), ["Leaf", "Intermediate", "Test Root"]);
    assert_eq!(names(&build_chain(&l.cert, &pool, Some(time(2026)))), ["Leaf", "Intermediate", "Test Root"]);
}

#[test]
fn an_end_entity_certificate_cannot_issue() {
    let r = root();
    let e = end_entity("Ordinary Subscriber", &r);
    // A certificate its holder made with the end-entity key.
    let forged = end_entity("Bank AG", &e);
    let pool = [e.cert.clone(), r.cert.clone()];
    let (chain, why) = build_chain_noted(&forged.cert, &pool, None);
    assert_eq!(names(&chain), ["Bank AG"], "the subscriber certificate does not vouch for what it signed");
    assert!(why.is_some_and(|w| w.contains("not a CA")), "the reason is reported");
    // Neither does one without basicConstraints (not a v1 root: it has an issuer).
    let plain = issue("No Constraints", Some(&r), FOREVER, &[]);
    let under_plain = end_entity("Under Plain", &plain);
    let pool = [plain.cert.clone(), r.cert.clone()];
    assert_eq!(names(&build_chain(&under_plain.cert, &pool, None)), ["Under Plain"]);
    // CA:TRUE does not help when keyUsage leaves out keyCertSign…
    let signing_only = issue("Signing Only CA", Some(&r), FOREVER, &[basic_constraints(true, None), key_usage(DIGITAL_SIGNATURE)]);
    let below = end_entity("Below Signing Only", &signing_only);
    let pool = [signing_only.cert.clone(), r.cert.clone()];
    assert_eq!(names(&build_chain(&below.cert, &pool, None)), ["Below Signing Only"]);
    // …and a CA with no keyUsage at all is fine.
    let any_usage = issue("Any Usage CA", Some(&r), FOREVER, &[basic_constraints(true, None)]);
    let below = end_entity("Below Any Usage", &any_usage);
    let pool = [any_usage.cert.clone(), r.cert.clone()];
    assert_eq!(names(&build_chain(&below.cert, &pool, None)), ["Below Any Usage", "Any Usage CA", "Test Root"]);
}

#[test]
fn a_path_length_constraint_limits_the_cas_below() {
    let r = root();
    let i1 = ca("Constrained", &r, Some(0));
    let i2 = ca("Sub CA", &i1, None);
    let l = end_entity("Leaf", &i2);
    let pool = [i1.cert.clone(), i2.cert.clone(), r.cert.clone()];
    // pathLen 0: no CA may follow it, so Sub CA's issuer is refused and the chain stops there.
    assert_eq!(names(&build_chain(&l.cert, &pool, None)), ["Leaf", "Sub CA"]);
    // pathLen 0 directly above the leaf is fine, and pathLen 1 allows one CA below.
    let direct = end_entity("Direct Leaf", &i1);
    assert_eq!(names(&build_chain(&direct.cert, &pool, None)), ["Direct Leaf", "Constrained", "Test Root"]);
    let i1 = ca("Constrained 1", &r, Some(1));
    let i2 = ca("Sub CA 1", &i1, None);
    let l = end_entity("Leaf 1", &i2);
    let pool = [i1.cert.clone(), i2.cert.clone(), r.cert.clone()];
    assert_eq!(names(&build_chain(&l.cert, &pool, None)), ["Leaf 1", "Sub CA 1", "Constrained 1", "Test Root"]);
}

#[test]
fn issuers_must_have_been_valid_when_it_matters() {
    let r = root();
    let old = issue("Short Lived CA", Some(&r), (2020, 2022), &[basic_constraints(true, None), key_usage(KEY_CERT_SIGN)]);
    let l = end_entity("Leaf", &old);
    let pool = [old.cert.clone(), r.cert.clone()];
    assert_eq!(names(&build_chain(&l.cert, &pool, Some(time(2021)))), ["Leaf", "Short Lived CA", "Test Root"]);
    let (chain, why) = build_chain_noted(&l.cert, &pool, Some(time(2023)));
    assert_eq!(names(&chain), ["Leaf"], "expired by then");
    assert!(why.is_some_and(|w| w.contains("not valid")));
    assert_eq!(names(&build_chain(&l.cert, &pool, Some(time(2019)))), ["Leaf"], "not yet valid");
    // With no time to judge by, nothing is refused on validity.
    assert_eq!(names(&build_chain(&l.cert, &pool, None)).len(), 3);
}

#[test]
fn an_old_v1_self_signed_root_without_constraints_still_anchors() {
    // No extensions at all: basicConstraints absent, as in v1 roots.
    let r = issue("Old V1 Root", None, FOREVER, &[]);
    assert!(r.cert.is_self_signed() && !r.cert.has_basic_constraints && r.cert.may_issue());
    let i = ca("Under V1", &r, None);
    let l = end_entity("Leaf", &i);
    let pool = [i.cert.clone(), r.cert.clone()];
    assert_eq!(names(&build_chain(&l.cert, &pool, None)), ["Leaf", "Under V1", "Old V1 Root"]);
    // A self-signed certificate that says CA:FALSE is not a root that can issue.
    let not_a_ca = issue("Self Signed Leaf", None, FOREVER, &[basic_constraints(false, None)]);
    assert!(!not_a_ca.cert.may_issue());
}

fn fixture() -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/openssl-signed.pdf", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// Sign a small PDF with `leaf`, embedding `embedded`, and validate it trusting only `anchor`.
fn verdict(leaf: Party, embedded: Vec<Certificate>, anchor: &Certificate) -> (Status, Vec<String>) {
    let id = DigitalId { key: leaf.key, certificate: leaf.cert, chain: embedded, friendly_name: None };
    let doc = Document::open(Arc::new(fixture())).unwrap();
    let opts = SignOptions { page: 0, rect: None, date: "D:20260601120000Z".into(), ..SignOptions::default() };
    let signed = pdfcraft_sign::sign(&doc, &id, &opts).unwrap();
    let trust = TrustStore { certs: vec![anchor.clone()], ..TrustStore::default() };
    let s = signatures(&Document::open(Arc::new(signed.clone())).unwrap(), &signed, &trust).into_iter().rfind(|s| s.signed).unwrap();
    (s.status, s.details)
}

#[test]
fn a_signature_chaining_through_an_ordinary_certificate_is_not_trusted() {
    let r = root();
    // The root vouches for a CA, which vouches for the signer: valid.
    let i = ca("Issuing CA", &r, None);
    let l = end_entity("Honest Signer", &i);
    let (status, details) = verdict(l, vec![i.cert.clone()], &r.cert);
    assert_eq!(status, Status::Valid, "{details:?}");
    // The root vouched for an ordinary subscriber, who signed the "signer" certificate with the
    // subscriber's own key and embedded it: the root must not vouch for that.
    let subscriber = end_entity("Ordinary Subscriber", &r);
    let forged = end_entity("Forged Identity", &subscriber);
    let (status, details) = verdict(forged, vec![subscriber.cert.clone()], &r.cert);
    assert_eq!(status, Status::Unknown, "{details:?}");
    assert!(details.iter().any(|d| d.contains("not a CA")), "{details:?}");
}

#[test]
fn a_v3_self_signed_certificate_without_constraints_is_not_a_root_that_issues() {
    // A self-signed v3 ID with no basicConstraints and no key usage (only a key identifier): it
    // may sign documents, but it may not vouch for anyone else.
    let id = issue("Self Signed V3 ID", None, FOREVER, &[ext("2.5.29.14", &der::octets(&[1, 2, 3, 4]))]);
    assert!(id.cert.is_self_signed() && !id.cert.has_basic_constraints && id.cert.version == 2);
    assert!(!id.cert.may_issue());
    let l = end_entity("Leaf", &id);
    assert_eq!(names(&build_chain(&l.cert, std::slice::from_ref(&id.cert), None)), ["Leaf"]);
}

const KEY_ENCIPHERMENT: u8 = 0x20;
const SERVER_AUTH: &str = "1.3.6.1.5.5.7.3.1";

fn extended_key_usage(oids: &[&str]) -> Vec<u8> {
    let encoded: Vec<Vec<u8>> = oids.iter().map(|o| der::oid(o)).collect();
    let refs: Vec<&[u8]> = encoded.iter().map(Vec::as_slice).collect();
    der::seq(&[&der::oid("2.5.29.37"), &der::octets(&der::seq(&refs))])
}

fn signer_with(cn: &str, under: &Party, extensions: &[Vec<u8>]) -> Party {
    let mut all = vec![basic_constraints(false, None)];
    all.extend_from_slice(extensions);
    issue(cn, Some(under), FOREVER, &all)
}

#[test]
fn the_signer_certificate_must_allow_document_signing() {
    let r = root();
    let problem = |exts: &[Vec<u8>]| signer_with("Signer", &r, exts).cert.signing_problem();
    // Key usage without digitalSignature or nonRepudiation.
    assert!(problem(&[key_usage(KEY_CERT_SIGN)]).is_some_and(|w| w.contains("key usage")));
    assert!(problem(&[key_usage(KEY_ENCIPHERMENT)]).is_some());
    assert!(problem(&[key_usage(DIGITAL_SIGNATURE)]).is_none());
    // nonRepudiation alone is enough.
    assert!(problem(&[key_usage(0x40)]).is_none());
    // Extended key usage without a document-signing purpose.
    assert!(problem(&[extended_key_usage(&[SERVER_AUTH])]).is_some_and(|w| w.contains("extended key usage")));
    for ok in ["2.5.29.37.0", "1.3.6.1.5.5.7.3.4", "1.3.6.1.5.5.7.3.36", "1.2.840.113583.1.1.5", "1.3.6.1.4.1.311.10.3.12"] {
        assert!(problem(&[extended_key_usage(&[SERVER_AUTH, ok])]).is_none(), "{ok}");
    }
    // An unknown critical extension refuses; the same extension non-critical does not.
    let unknown = "1.2.3.4.5.6";
    assert!(problem(&[ext(unknown, &der::octets(&[0]))]).is_some_and(|w| w.contains(unknown)));
    assert!(problem(&[der::seq(&[&der::oid(unknown), &der::octets(&der::octets(&[0]))])]).is_none());
    // Critical extensions PdfCraft knows, or may accept as they are, don't refuse.
    assert!(problem(&[ext("2.5.29.32", &der::seq(&[]))]).is_none());
    // No extensions at all, and the digital IDs PdfCraft creates, may sign.
    assert!(issue("Plain", Some(&r), FOREVER, &[]).cert.signing_problem().is_none());
    let key = PrivateKey::generate_p256().unwrap();
    let name = Name::build("Own ID", "", "", "", "US");
    assert!(Certificate::self_signed(&name, &key, time(2026), 5, &[1]).unwrap().signing_problem().is_none());
}

#[test]
fn a_trusted_signer_whose_certificate_may_not_sign_is_not_valid() {
    let r = root();
    let (status, details) =
        verdict(signer_with("TLS Server", &r, &[key_usage(DIGITAL_SIGNATURE), extended_key_usage(&[SERVER_AUTH])]), vec![], &r.cert);
    assert_eq!(status, Status::Unknown, "{details:?}");
    assert!(details.iter().any(|d| d.contains("not issued for signing documents")), "{details:?}");
    let (status, details) = verdict(signer_with("Encryption Only", &r, &[key_usage(KEY_ENCIPHERMENT)]), vec![], &r.cert);
    assert_eq!(status, Status::Unknown, "{details:?}");
    assert!(details.iter().any(|d| d.contains("does not allow digital signatures")), "{details:?}");
    let (status, details) = verdict(signer_with("Document Signer", &r, &[key_usage(DIGITAL_SIGNATURE)]), vec![], &r.cert);
    assert_eq!(status, Status::Valid, "{details:?}");
}

#[test]
fn unreadable_constraint_extensions_fail_closed() {
    // keyUsage present but not a BIT STRING: no usage at all, so no keyCertSign.
    let bad_usage = ext("2.5.29.15", &[0xff, 0x00]);
    let c = issue("Bad Usage CA", Some(&root()), FOREVER, &[basic_constraints(true, None), bad_usage]);
    assert_eq!(c.cert.key_usage, Some(0));
    assert!(!c.cert.may_issue());
    // pathLenConstraint present but negative: no CA may follow, not any number.
    let negative = ext("2.5.29.19", &der::seq(&[&der::boolean(true), &der::tlv(tag::INTEGER, &[0xff])]));
    let c = issue("Negative PathLen CA", Some(&root()), FOREVER, &[negative]);
    assert!(c.cert.is_ca);
    assert_eq!(c.cert.path_len, Some(0));
    // basicConstraints present but unparsable: present, not a CA, so not an "old v1 root".
    let garbage = ext("2.5.29.19", &[0x30, 0x05, 0x01]);
    let c = issue("Garbled Constraints", None, FOREVER, &[garbage]);
    assert!(c.cert.has_basic_constraints && !c.cert.is_ca && !c.cert.may_issue());
}

/// Like `verdict`, but with a whole `TrustStore`.
fn verdict_with(leaf: Party, embedded: Vec<Certificate>, trust: &TrustStore) -> (Status, Vec<String>) {
    let id = DigitalId { key: leaf.key, certificate: leaf.cert, chain: embedded, friendly_name: None };
    let doc = Document::open(Arc::new(fixture())).unwrap();
    let opts = SignOptions { page: 0, rect: None, date: "D:20260601120000Z".into(), ..SignOptions::default() };
    let signed = pdfcraft_sign::sign(&doc, &id, &opts).unwrap();
    let s = signatures(&Document::open(Arc::new(signed.clone())).unwrap(), &signed, trust).into_iter().rfind(|s| s.signed).unwrap();
    (s.status, s.details)
}

#[test]
fn trust_sets_are_off_by_default_and_a_loaded_list_is_named_when_it_vouches() {
    use pdfcraft_sign::trust::TrustList;
    let r = root();
    let i = ca("Issuing CA", &r, None);
    let leaf = |cn: &str| end_entity(cn, &i);
    let list = TrustList::from_bytes("Test List", &r.cert.raw).unwrap();

    // Nothing is trusted unless asked for: no list, no built-in roots.
    let none = TrustStore::default();
    assert!(!none.builtin_roots && none.lists.is_empty() && none.certs.is_empty());
    let (status, details) = verdict_with(leaf("Signer A"), vec![i.cert.clone()], &none);
    assert_eq!(status, Status::Unknown, "{details:?}");

    // With the list loaded, the same signature is valid and says which list vouched.
    let with_list = TrustStore { lists: vec![list.clone()], ..TrustStore::default() };
    let (status, details) = verdict_with(leaf("Signer B"), vec![i.cert.clone()], &with_list);
    assert_eq!(status, Status::Valid, "{details:?}");
    assert!(details.iter().any(|d| d.contains("Test List")), "{details:?}");

    // The built-in roots are not these roots, so switching them on changes nothing here.
    let builtin = TrustStore { builtin_roots: true, ..TrustStore::default() };
    let (status, _) = verdict_with(leaf("Signer C"), vec![i.cert.clone()], &builtin);
    assert_eq!(status, Status::Unknown);

    // A list does not make a subscriber a CA: the forged identity stays unknown.
    let subscriber = end_entity("Ordinary Subscriber", &r);
    let forged = end_entity("Forged Identity", &subscriber);
    let (status, details) = verdict_with(forged, vec![subscriber.cert.clone()], &with_list);
    assert_eq!(status, Status::Unknown, "{details:?}");
}

#[test]
fn the_builtin_roots_only_anchor_when_switched_on() {
    // Trust is decided by `source_of`, which is what validation uses.
    let first = pdfcraft_sign::trust::builtin_roots().first().unwrap();
    let off = TrustStore::default();
    assert_eq!(off.source_of(first), None);
    let on = TrustStore { builtin_roots: true, ..TrustStore::default() };
    assert_eq!(on.source_of(first), Some(pdfcraft_sign::TrustSource::BuiltinRoots));
    // The user's own list wins when both apply.
    let both = TrustStore { certs: vec![first.clone()], builtin_roots: true, ..TrustStore::default() };
    assert_eq!(both.source_of(first), Some(pdfcraft_sign::TrustSource::User));
}
