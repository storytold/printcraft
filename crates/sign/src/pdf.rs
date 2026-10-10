//! Signatures in a PDF (ISO 32000-2 §12.8): listing and validating signature fields, and
//! signing with an incremental save.
//!
//! **Validation** reads `/ByteRange` and the CMS from the file's own bytes (not from the parsed
//! string, which a security handler would have transformed), recomputes the digest, checks the
//! signature and the signer's chain against the trust store, and diffs later revisions against
//! the signed one to classify changes made after signing (form fill, comments, further
//! signatures, or anything else) under the DocMDP permissions.
//!
//! **Signing** adds or fills a signature field, writes the document incrementally with a
//! zero-filled `/Contents` and a fixed-width `/ByteRange`, then patches both in place: the
//! PAdES B-B recipe (`ETSI.CAdES.detached`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, SaveOptions, Stream};

use crate::SignError;
use crate::cms::SignedData;
use crate::der::Time;
use crate::keys::DigestAlg;
use crate::pkcs12::DigitalId;
use crate::x509::{Certificate, build_chain, build_chain_noted};

#[cfg(test)]
#[path = "pdf_discovery_tests.rs"]
mod discovery_tests;

/// Digests of signed byte ranges (and parsed signed revisions), kept across revalidations of
/// one document: incremental edits only append to the file, so the signed bytes and their
/// digest don't change, and rehashing a large file on every edit was the expensive part of
/// validation (120 MB: 84 ms per comment edit before, 6 ms after). Keyed by the ranges, the
/// algorithm and a sampled fingerprint of the covered bytes; use one cache per document, whose
/// bytes only ever grow.
#[derive(Debug, Default)]
pub struct DigestCache {
    map: std::sync::Mutex<HashMap<RangeKey, Vec<u8>>>,
    /// Signed revisions opened for change classification, by length and fingerprint.
    revisions: std::sync::Mutex<HashMap<(usize, [u8; 32]), Document>>,
    /// Candidate identities only: validation still uses current bytes and current trust.
    discovery: std::sync::Mutex<Option<TimestampCandidates>>,
    #[cfg(test)]
    discovery_scans: std::sync::atomic::AtomicUsize,
}

/// Keep only the most recently visited source/overlay. In particular, this must not retain
/// a Document, its bytes, or parsed signature dictionaries across edits and saves.
#[derive(Debug)]
struct TimestampCandidates {
    source: pdfcraft_cos::SourceIdentity,
    modified: Vec<u32>,
    candidates: Vec<ObjRef>,
}

const DISCOVERY_CACHE_LIMIT: usize = 4096;

fn is_document_timestamp(object: &Object) -> bool {
    object.as_dict().is_some_and(|d| is_doc_timestamp(d) && d.contains(b"ByteRange"))
}

/// Only dictionary/null edits have a local effect on candidate membership. Changes to an
/// object stream or a scalar used as an indirect /Length can affect unedited objects too.
/// Checking both the original and current objects also covers freeing those dependencies.
/// Unsupported or unreadable edits deliberately fall back to the complete bounded scan.
fn changed_timestamp_candidates(doc: &Document, numbers: &[u32]) -> Option<Vec<ObjRef>> {
    for (reference, object) in doc.scan_original_objects_subset(numbers.to_vec()) {
        match object.ok()?.as_ref() {
            Object::Dict(_) => {}
            Object::Null if matches!(doc.xref_entry(reference.num), None | Some(pdfcraft_cos::XrefEntry::Free { .. })) => {}
            _ => return None,
        }
    }
    let mut candidates = Vec::new();
    for (reference, object) in doc.scan_objects_subset(numbers.to_vec()) {
        let object = object.ok()?;
        if !matches!(object.as_ref(), Object::Dict(_) | Object::Null) {
            return None;
        }
        if is_document_timestamp(&object) {
            candidates.push(reference);
        }
    }
    Some(candidates)
}

/// (end of first range, start of second, end of second, algorithm, fingerprint).
type RangeKey = (usize, usize, usize, DigestAlg, [u8; 32]);

/// 64 KiB sampled from across `covered`, plus its length.
fn fingerprint(covered: &[u8]) -> [u8; 32] {
    let mut parts: Vec<&[u8]> = (0..64)
        .map(|i| {
            let at = covered.len().saturating_sub(1024) * i / 63;
            &covered[at..(at + 1024).min(covered.len())]
        })
        .collect();
    let len = (covered.len() as u64).to_be_bytes();
    parts.push(&len);
    DigestAlg::Sha256.digest(&parts).try_into().unwrap_or([0; 32])
}

impl DigestCache {
    fn timestamp_candidates(&self, doc: &Document) -> Vec<ObjRef> {
        let mut slot = self.discovery.lock().ok();
        let previous = slot.as_mut().and_then(|slot| slot.take());
        let source = doc.source_identity();
        let modified = doc.modified_objects();
        // Reconstructed/unknown input and output-security changes take the original path.
        let eligible = slot.is_some()
            && doc.repair_log().is_empty()
            && !doc.revisions().is_empty()
            && !doc.encryption_changed()
            && modified.len() <= DISCOVERY_CACHE_LIMIT;
        if eligible && let Some(previous) = previous.filter(|previous| previous.source == source) {
            // Undo/redo can remove overlays as well as add them. Revisit the union so an
            // original candidate replaced or freed in the last snapshot can reappear.
            let mut changed = previous.modified;
            changed.extend_from_slice(&modified);
            changed.sort_unstable();
            changed.dedup();
            if changed.len() <= DISCOVERY_CACHE_LIMIT
                && let Some(mut candidates) = changed_timestamp_candidates(doc, &changed)
            {
                candidates.extend(previous.candidates.into_iter().filter(|r| changed.binary_search(&r.num).is_err()));
                candidates.sort_unstable_by_key(|r| r.num);
                if candidates.len() <= DISCOVERY_CACHE_LIMIT
                    && let Some(slot) = slot.as_mut()
                {
                    **slot = Some(TimestampCandidates { source, modified, candidates: candidates.clone() });
                }
                return candidates;
            }
        }
        #[cfg(test)]
        self.discovery_scans.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut complete = true;
        let candidates: Vec<_> = doc
            .scan_objects_checked()
            .filter_map(|(reference, object)| match object {
                Ok(object) => is_document_timestamp(&object).then_some(reference),
                Err(_) => {
                    complete = false;
                    None
                }
            })
            .collect();
        if eligible
            && complete
            && candidates.len() <= DISCOVERY_CACHE_LIMIT
            && changed_timestamp_candidates(doc, &modified).is_some()
            && let Some(slot) = slot.as_mut()
        {
            **slot = Some(TimestampCandidates { source, modified, candidates: candidates.clone() });
        }
        candidates
    }

    /// The signed revision `bytes[..end]`, parsed (and kept).
    fn revision(&self, bytes: &[u8], end: usize) -> Option<Document> {
        let key = (end, fingerprint(&bytes[..end]));
        if let Some(d) = self.revisions.lock().ok().and_then(|m| m.get(&key).cloned()) {
            return Some(d);
        }
        let d = Document::open(Arc::new(bytes[..end].to_vec())).ok()?;
        if let Ok(mut m) = self.revisions.lock() {
            if m.len() > 8 {
                m.clear();
            }
            m.insert(key, d.clone());
        }
        Some(d)
    }

    fn digest(&self, alg: DigestAlg, bytes: &[u8], l0: usize, o1: usize, end: usize) -> Vec<u8> {
        let key = (l0, o1, end, alg, fingerprint(&bytes[..end]));
        if let Some(d) = self.map.lock().ok().and_then(|m| m.get(&key).cloned()) {
            return d;
        }
        let d = alg.digest(&[&bytes[..l0], &bytes[o1..end]]);
        if let Ok(mut m) = self.map.lock() {
            if m.len() > 64 {
                m.clear();
            }
            m.insert(key, d.clone());
        }
        d
    }
}

/// Certificates trusted for signing (Acrobat: Trusted Certificates). Only `certs`, the user's own
/// list, is trusted by default; the others are sets the user has to switch on.
#[derive(Clone, Debug, Default)]
pub struct TrustStore {
    pub certs: Vec<Certificate>,
    /// Also trust the embedded roots of commercial CAs ([`crate::trust::builtin_roots`]).
    pub builtin_roots: bool,
    /// Also trust the embedded roots of India's PKI ([`crate::trust::cca_india_roots`]), behind
    /// e-Aadhaar and India's other licensed CAs.
    pub cca_india: bool,
    /// Also trust these lists the user loaded (e.g. the EU Trusted Lists' qualified CAs).
    pub lists: Vec<crate::trust::TrustList>,
}

/// Why a certificate in a chain is trusted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrustSource {
    /// The user's own list of trusted certificates.
    User,
    /// A root embedded in PdfCraft ([`crate::trust::builtin_roots`]).
    BuiltinRoots,
    /// A root of India's PKI embedded in PdfCraft ([`crate::trust::cca_india_roots`]).
    CcaIndia,
    /// The named list the user loaded.
    List(String),
}

impl TrustStore {
    /// Load `list`, replacing a list of the same name, or with `None` remove the list `name`.
    pub fn set_list(&mut self, name: &str, list: Option<crate::trust::TrustList>) {
        self.lists.retain(|l| l.name != name);
        self.lists.extend(list);
    }

    /// Every certificate that is a trust anchor right now: the user's, then the sets switched on.
    fn anchors(&self) -> impl Iterator<Item = &Certificate> + Clone {
        let builtin: &[Certificate] = if self.builtin_roots { crate::trust::builtin_roots() } else { &[] };
        let cca: &[Certificate] = if self.cca_india { crate::trust::cca_india_roots() } else { &[] };
        self.certs.iter().chain(builtin).chain(cca).chain(self.lists.iter().flat_map(|l| l.certs.iter()))
    }

    /// What makes `c` trusted, if anything. The user's own list wins over the sets.
    pub fn source_of(&self, c: &Certificate) -> Option<TrustSource> {
        if self.certs.iter().any(|t| t.raw == c.raw) {
            Some(TrustSource::User)
        } else if self.builtin_roots && crate::trust::is_builtin_root(c) {
            Some(TrustSource::BuiltinRoots)
        } else if self.cca_india && crate::trust::is_cca_india_root(c) {
            Some(TrustSource::CcaIndia)
        } else {
            self.lists.iter().find(|l| l.certs.iter().any(|t| t.raw == c.raw)).map(|l| TrustSource::List(l.name.clone()))
        }
    }

    /// The first trusted certificate going up `chain` (the leaf first), and what makes it trusted.
    ///
    /// The user's own certificates always vouch. The sets (built-in roots, loaded lists) trust a CA
    /// for everything it issues, and most of those CAs issue TLS certificates, so they vouch only
    /// when `sets_vouch` says the leaf is meant for the purpose at hand. The flag returned says a
    /// set would have vouched but for that, so the details can say why it didn't.
    fn anchor<'a>(&self, chain: impl IntoIterator<Item = &'a Certificate>, sets_vouch: bool) -> (Option<(String, TrustSource)>, bool) {
        let mut refused = false;
        for c in chain {
            match self.source_of(c) {
                Some(TrustSource::User) => return (Some((c.display_name(), TrustSource::User)), false),
                Some(set) if sets_vouch => return (Some((c.display_name(), set)), false),
                Some(_) => refused = true,
                None => {}
            }
        }
        (None, refused)
    }

    /// Whether the time-stamp authority `tsa` is trusted, building its chain from `embedded` and
    /// the anchors as of `at`; and whether a set would have vouched for it but for its purpose. A
    /// set vouches only for an authority that says it is one (RFC 3161 §2.3, [`Certificate::may_timestamp`]).
    fn trusts_timestamp_authority(&self, tsa: &Certificate, embedded: &[Certificate], at: Time) -> (bool, bool) {
        let chain = build_chain(tsa, embedded.iter().chain(self.anchors()), Some(at));
        let (anchor, refused) = self.anchor(chain.iter().copied(), tsa.may_timestamp());
        (anchor.is_some(), refused)
    }
}

const NOT_A_TSA: &str = "The timestamp authority's certificate chains to a root in a trust set you switched on, but it is not meant for time stamps (RFC 3161 §2.3 asks for the time-stamping purpose), so the set does not vouch for it.";

/// Acrobat's three verdicts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Intact, signed by a trusted identity, and any later changes are permitted.
    Valid,
    /// Intact, but the signer's identity can't be established (not trusted), or validity
    /// couldn't be determined.
    Unknown,
    /// The signed bytes changed, the signature doesn't verify, or later changes aren't permitted.
    Invalid,
}

/// Changes made after signing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Modification {
    /// The signature covers the whole file.
    None,
    /// Later revisions only make changes the signature permits (listed).
    Allowed(Vec<String>),
    /// Later revisions make changes it doesn't permit (listed).
    Disallowed(Vec<String>),
}

/// One signature field.
#[derive(Clone, Debug)]
pub struct SignatureInfo {
    /// Fully qualified field name.
    pub field: String,
    pub signed: bool,
    /// 0-based page of its widget, and the widget rectangle (user space).
    pub page: Option<usize>,
    pub rect: Option<[f64; 4]>,
    pub visible: bool,
    /// The signer: the certificate's common name, else `/Name`.
    pub signer: Option<String>,
    pub certificate: Option<Certificate>,
    /// The chain as built (signer first).
    pub chain: Vec<Certificate>,
    /// `/M` as written, and the signing time used for checks.
    pub date: Option<String>,
    pub signing_time: Option<Time>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact: Option<String>,
    pub sub_filter: Option<String>,
    /// True for a standalone document timestamp (`/ETSI.RFC3161`), not a field signature.
    pub doc_timestamp: bool,
    /// DocMDP permissions when this is a certification signature (1–3).
    pub certify: Option<u8>,
    /// 1-based revision the signature covers.
    pub revision: usize,
    /// Length of the signed revision (bytes) — "View signed version".
    pub signed_len: usize,
    /// Digest algorithm used for the document's signed bytes.
    pub digest: Option<DigestAlg>,
    pub algorithm: Option<String>,
    pub timestamp: bool,
    /// RFC 3161 generation time, when the token was validated and (for a signature's embedded
    /// token) its authority chains to the trust store. An untrusted token's time is never
    /// used as the validation time.
    pub timestamp_time: Option<Time>,
    pub status: Status,
    pub modification: Modification,
    /// Acrobat-style sentences explaining the verdict.
    pub details: Vec<String>,
}

impl SignatureInfo {
    /// The one-line summary under the signature in the Signatures panel.
    pub fn summary(&self) -> &'static str {
        if !self.signed {
            return "Unsigned signature field";
        }
        match self.status {
            Status::Valid => "Signature is valid",
            Status::Unknown => "Signature validity is unknown",
            Status::Invalid => "Signature is invalid",
        }
    }
}

fn text(doc: &Document, d: &Dict, key: &[u8]) -> Option<String> {
    let o = doc.resolve(d.get(key)?);
    let s = match &*o {
        Object::String(s) => s.to_text(),
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        _ => return None,
    };
    let s = s.trim_matches('\0').trim().to_string();
    (!s.is_empty()).then_some(s)
}

struct Field {
    name: String,
    /// The field dictionary (and its widget(s)).
    r: Option<ObjRef>,
    dict: Dict,
    widgets: Vec<(Option<ObjRef>, Dict)>,
}

/// Every signature field, depth first in `/Fields` order.
fn sig_fields(doc: &Document) -> Vec<Field> {
    let mut out = Vec::new();
    let Some(root) = doc.root() else { return out };
    let Some(af) = doc.get(root).as_dict().and_then(|c| c.get(b"AcroForm").map(|a| doc.resolve(a))).and_then(|a| a.as_dict().cloned()) else {
        return out;
    };
    let Some(fields) = af.get(b"Fields").map(|f| doc.resolve(f)).and_then(|f| f.as_array().cloned()) else { return out };
    let mut seen = HashSet::new();
    let mut stack: Vec<(Object, String, Option<Vec<u8>>)> = fields.into_iter().rev().map(|f| (f, String::new(), None)).collect();
    while let Some((o, prefix, inherited_ft)) = stack.pop() {
        if let Some(r) = o.as_ref()
            && !seen.insert(r)
        {
            continue;
        }
        let Some(d) = doc.resolve(&o).as_dict().cloned() else { continue };
        let part = text(doc, &d, b"T");
        let name = match (&part, prefix.is_empty()) {
            (Some(p), true) => p.clone(),
            (Some(p), false) => format!("{prefix}.{p}"),
            (None, _) => prefix.clone(),
        };
        let ft = d.name(b"FT").map(<[u8]>::to_vec).or(inherited_ft);
        let kids: Vec<Object> = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
        // Kids with a /T are fields; others are this field's widgets.
        let (sub, widgets): (Vec<Object>, Vec<Object>) = kids.into_iter().partition(|k| doc.resolve(k).as_dict().is_some_and(|kd| kd.contains(b"T")));
        for k in sub.into_iter().rev() {
            stack.push((k, name.clone(), ft.clone()));
        }
        if ft.as_deref() == Some(b"Sig") && part.is_some() {
            let mut ws: Vec<(Option<ObjRef>, Dict)> =
                widgets.iter().filter_map(|w| doc.resolve(w).as_dict().cloned().map(|wd| (w.as_ref(), wd))).collect();
            if d.name(b"Subtype") == Some(b"Widget") {
                ws.insert(0, (o.as_ref(), d.clone()));
            }
            out.push(Field { name, r: o.as_ref(), dict: d, widgets: ws });
        }
    }
    out
}

/// Which page holds each annotation.
fn annot_pages(doc: &Document) -> HashMap<ObjRef, usize> {
    let mut map = HashMap::new();
    for (i, p) in pdfcraft_annot::page_refs(doc).unwrap_or_default().into_iter().enumerate() {
        if let Some(a) = doc.get(p).as_dict().and_then(|d| d.get(b"Annots").map(|a| doc.resolve(a))).and_then(|a| a.as_array().cloned()) {
            for r in a.iter().filter_map(Object::as_ref) {
                map.insert(r, i);
            }
        }
    }
    map
}

fn nums(doc: &Document, d: &Dict, key: &[u8]) -> Option<Vec<f64>> {
    doc.resolve(d.get(key)?).as_array().map(|a| a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect())
}

/// List and validate every signature field. `bytes` is the file as stored (the ranges index it).
pub fn list(doc: &Document, bytes: &[u8], trust: &TrustStore) -> Vec<SignatureInfo> {
    list_cached(doc, bytes, trust, &DigestCache::default())
}

/// [`list`], reusing digests and standalone-timestamp discovery from `cache`. Field/widget
/// structure and validation against `bytes` and `trust` are read again on every call.
pub fn list_cached(doc: &Document, bytes: &[u8], trust: &TrustStore, cache: &DigestCache) -> Vec<SignatureInfo> {
    let pages = annot_pages(doc);
    let mut out = Vec::new();
    let mut field_values: HashSet<ObjRef> = HashSet::new();
    for f in sig_fields(doc) {
        if let Some(v_ref) = f.dict.get(b"V").and_then(Object::as_ref) {
            field_values.insert(v_ref);
        }
        let widget = f.widgets.first();
        let page = widget.and_then(|(r, w)| r.and_then(|r| pages.get(&r).copied()).or_else(|| w.reference(b"P").and_then(|p| page_index(doc, p))));
        let rect = widget
            .and_then(|(_, w)| nums(doc, w, b"Rect"))
            .filter(|r| r.len() == 4)
            .map(|r| [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]);
        let visible = rect.is_some_and(|r| r[2] - r[0] > 0.5 && r[3] - r[1] > 0.5);
        let v = f.dict.get(b"V").map(|v| doc.resolve(v)).and_then(|v| v.as_dict().cloned());
        let mut info = SignatureInfo {
            field: f.name.clone(),
            signed: v.is_some(),
            page,
            rect,
            visible,
            signer: None,
            certificate: None,
            chain: Vec::new(),
            date: None,
            signing_time: None,
            reason: None,
            location: None,
            contact: None,
            sub_filter: None,
            doc_timestamp: false,
            certify: None,
            revision: 0,
            signed_len: 0,
            digest: None,
            algorithm: None,
            timestamp: false,
            timestamp_time: None,
            status: Status::Unknown,
            modification: Modification::None,
            details: Vec::new(),
        };
        if let Some(v) = v {
            validate_into(doc, bytes, trust, &v, &mut info, cache);
        }
        out.push(info);
    }
    // Standalone document timestamps (ISO 32000-2 §12.8.2.2) live outside AcroForm fields.
    let candidates = cache.timestamp_candidates(doc).into_iter().map(|r| r.num).collect();
    for (r, o) in doc.scan_objects_subset(candidates) {
        if field_values.contains(&r) {
            continue;
        }
        let Ok(o) = o else { continue };
        let Some(d) = o.as_dict() else { continue };
        if !is_document_timestamp(&o) {
            continue;
        }
        let mut info = SignatureInfo {
            field: "DocumentTimestamp".to_string(),
            signed: true,
            page: None,
            rect: None,
            visible: false,
            signer: None,
            certificate: None,
            chain: Vec::new(),
            date: None,
            signing_time: None,
            reason: None,
            location: None,
            contact: None,
            sub_filter: Some("ETSI.RFC3161".to_string()),
            doc_timestamp: true,
            certify: None,
            revision: 0,
            signed_len: 0,
            digest: None,
            algorithm: None,
            timestamp: true,
            timestamp_time: None,
            status: Status::Unknown,
            modification: Modification::None,
            details: Vec::new(),
        };
        validate_doc_timestamp(doc, bytes, d, &mut info, cache, trust);
        out.push(info);
    }
    out
}

fn page_index(doc: &Document, p: ObjRef) -> Option<usize> {
    pdfcraft_annot::page_refs(doc).ok()?.iter().position(|r| *r == p)
}

/// Validate one signature dictionary.
pub fn validate(doc: &Document, bytes: &[u8], trust: &TrustStore, field: &str) -> Option<SignatureInfo> {
    list(doc, bytes, trust).into_iter().find(|s| s.field == field)
}

fn unhex(s: &[u8]) -> Option<Vec<u8>> {
    let digits: Vec<u8> = s.iter().copied().filter(|c| !c.is_ascii_whitespace()).collect();
    let val = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    digits.chunks(2).map(|p| Some(val(p[0])? << 4 | p.get(1).map_or(Some(0), |c| val(*c))?)).collect()
}

/// A document timestamp dictionary: `/SubFilter /ETSI.RFC3161`, typed `/DocTimeStamp`
/// (ISO 32000-2 §12.8.5) or, as older writers have it, `/Sig`.
fn is_doc_timestamp(d: &Dict) -> bool {
    d.name(b"SubFilter") == Some(b"ETSI.RFC3161") && matches!(d.name(b"Type"), Some(b"DocTimeStamp" | b"Sig"))
}

fn validate_into(doc: &Document, bytes: &[u8], trust: &TrustStore, v: &Dict, info: &mut SignatureInfo, cache: &DigestCache) {
    // A document timestamp can also be the value of a signature field (Documenso writes
    // `Timestamp_1` that way). Its `/Contents` is a bare RFC 3161 token, not a signature over the
    // document, so it must not be read as one.
    if is_doc_timestamp(v) {
        info.sub_filter = Some("ETSI.RFC3161".to_string());
        info.doc_timestamp = true;
        info.timestamp = true;
        validate_doc_timestamp(doc, bytes, v, info, cache, trust);
        return;
    }
    info.date = text(doc, v, b"M");
    info.reason = text(doc, v, b"Reason");
    info.location = text(doc, v, b"Location");
    info.contact = text(doc, v, b"ContactInfo");
    info.sub_filter = v.name(b"SubFilter").map(|s| String::from_utf8_lossy(s).into_owned());
    info.signer = text(doc, v, b"Name");
    info.certify = v
        .get(b"Reference")
        .map(|r| doc.resolve(r))
        .and_then(|r| r.as_array().cloned())
        .into_iter()
        .flatten()
        .filter_map(|r| doc.resolve(&r).as_dict().cloned())
        .find(|r| r.name(b"TransformMethod") == Some(b"DocMDP"))
        .map(|r| {
            r.get(b"TransformParams").map(|p| doc.resolve(p)).and_then(|p| p.as_dict().and_then(|p| p.int(b"P"))).unwrap_or(2).clamp(1, 3) as u8
        });
    let invalid = |info: &mut SignatureInfo, why: &str| {
        info.status = Status::Invalid;
        info.details.push(why.to_string());
    };
    // Byte ranges: [0 a b c] with the gap holding exactly the hex /Contents.
    let Some(br) = nums(doc, v, b"ByteRange").filter(|b| b.len() == 4 && b.iter().all(|x| *x >= 0.0)) else {
        return invalid(info, "The signature has no valid byte range.");
    };
    let [o0, l0, o1, l1] = [br[0] as usize, br[1] as usize, br[2] as usize, br[3] as usize];
    if o0 != 0 || o1 < l0 || o1.checked_add(l1).is_none_or(|end| end > bytes.len()) || o1 - l0 < 2 {
        return invalid(info, "The signature's byte range does not match the file.");
    }
    let gap = &bytes[l0..o1];
    if gap.first() != Some(&b'<') || gap.last() != Some(&b'>') {
        return invalid(info, "The signature's byte range does not exclude exactly its contents.");
    }
    let Some(contents) = unhex(&gap[1..gap.len() - 1]) else { return invalid(info, "The signature contents are not hexadecimal.") };
    let covered = o1 + l1;
    info.signed_len = covered;
    // The signed revision's number: its cross-reference sections (1 for a reconstructed file).
    info.revision = cache.revision(bytes, covered).map_or(1, |d| d.revisions().len().max(1));
    // What can't be checked is unknown, not invalid: only data that is wrong or fails its check
    // is "invalid".
    let unsupported = |info: &mut SignatureInfo, why: &str| {
        info.status = Status::Unknown;
        info.details.push(format!("PdfCraft can't check this signature yet: {why}."));
    };
    if info.sub_filter.as_deref() == Some("adbe.x509.rsa_sha1") {
        return validate_x509_rsa_sha1(doc, bytes, trust, v, info, cache, (l0, o1, covered), &contents);
    }
    let sd = match SignedData::parse(&contents) {
        Ok(sd) => sd,
        Err(SignError::Unsupported(e)) => return unsupported(info, &e),
        Err(e) => return invalid(info, &format!("The signature could not be read ({e}).")),
    };
    let s = &sd.signer;
    info.details.extend(sd.quirks.iter().map(|q| q.to_string()));
    info.digest = Some(s.digest);
    info.timestamp = s.timestamp;
    let mut unverified_time = None;
    if let Some(raw) = &s.timestamp_token {
        match crate::timestamp::parse_token(raw) {
            Ok(t) => {
                let imprint = t.digest.digest(&[&s.signature]);
                if imprint != t.imprint {
                    info.details.push("The signature's timestamp token does not cover the signature value.".into());
                } else {
                    // Only a token from a trusted authority (valid when it stamped) is trusted
                    // time; otherwise its time is reported but validation uses the signer's
                    // own claimed time, as without a timestamp.
                    let embedded = crate::timestamp::token_certs(raw);
                    let (trusted_tsa, wrong_purpose) = t
                        .signer_certificate()
                        .filter(|c| c.valid_at(t.gen_time))
                        .map_or((false, false), |c| trust.trusts_timestamp_authority(&c, &embedded, t.gen_time));
                    if trusted_tsa {
                        info.timestamp_time = Some(t.gen_time);
                    } else {
                        unverified_time = Some(t.gen_time);
                        if wrong_purpose {
                            info.details.push(NOT_A_TSA.into());
                        }
                    }
                }
            }
            Err(e) => info.details.push(format!("The signature's timestamp token could not be verified ({e}).")),
        }
    }
    // The digest the signature commits to.
    let content_digest = match &sd.content {
        // adbe.pkcs7.sha1: the document's SHA-1 digest is the signed content.
        Some(c) => {
            if *c != cache.digest(DigestAlg::Sha1, bytes, l0, o1, covered) {
                return invalid(info, "The document has been altered or corrupted since the signature was applied.");
            }
            s.digest.digest(&[c])
        }
        None => cache.digest(s.digest, bytes, l0, o1, covered),
    };
    if let Some(md) = &s.message_digest
        && *md != content_digest
    {
        return invalid(info, "The document has been altered or corrupted since the signature was applied.");
    }
    let Some(cert) = sd.signer_certificate().cloned() else {
        return invalid(info, "The signer's certificate is not in the signature.");
    };
    info.algorithm = Some(format!("{} with {}", cert.public_key.describe(), s.scheme_digest.unwrap_or(s.digest).name()));
    info.signer = Some(cert.display_name());
    let mut notes = Vec::new();
    match sd.verify_signature_noting(&cert, &content_digest, &mut notes) {
        Ok(true) => info.details.extend(notes.into_iter().map(String::from)),
        Ok(false) => {
            info.certificate = Some(cert);
            return invalid(info, "The signature value does not match the signer's certificate: the signature is corrupt.");
        }
        Err(SignError::Unsupported(e)) => {
            info.certificate = Some(cert);
            return unsupported(info, &e);
        }
        Err(e) => {
            info.certificate = Some(cert);
            return invalid(info, &format!("The signature could not be checked ({e})."));
        }
    }
    finish_validation(doc, bytes, trust, info, cache, covered, cert, &sd.certificates, s.signing_time, unverified_time);
}

/// The verdict once the signature value has been checked: chain and trust, the signing time
/// against the certificate's validity, and what later revisions changed.
#[allow(clippy::too_many_arguments)]
fn finish_validation(
    doc: &Document,
    bytes: &[u8],
    trust: &TrustStore,
    info: &mut SignatureInfo,
    cache: &DigestCache,
    covered: usize,
    cert: Certificate,
    embedded: &[Certificate],
    cms_time: Option<Time>,
    unverified_time: Option<Time>,
) {
    info.signing_time = cms_time.or_else(|| info.date.as_deref().and_then(Time::from_pdf));
    // The time the chain is judged at: trusted timestamp, else the signer's claimed time.
    let at = info.timestamp_time.or(info.signing_time);
    let (chain, refused) = build_chain_noted(&cert, embedded.iter().chain(trust.anchors()), at);
    let chain: Vec<Certificate> = chain.into_iter().cloned().collect();
    // The first trusted certificate going up the chain, and what makes it trusted.
    let (anchor, _) = trust.anchor(&chain, true);
    let trusted = anchor.is_some();
    info.chain = chain;
    info.certificate = Some(cert.clone());
    if let Some(why) = refused
        && !trusted
    {
        info.details.push(why);
    }
    // Revocation evidence embedded in the document security store, if any. A verified
    // revocation is final: later blocks must not soften the verdict.
    let signer = info.chain.first().cloned();
    let revoked = check_dss_revocation(doc, signer.as_ref(), &at, info);
    // Changes after signing.
    info.modification = if covered == bytes.len() || bytes[covered..].iter().all(|b| b.is_ascii_whitespace() || *b == 0) {
        Modification::None
    } else {
        classify_changes(doc, cache.revision(bytes, covered), info.certify)
    };
    let mut problems = false;
    match &info.modification {
        Modification::None => info.details.push("This document has not been modified since this signature was applied.".into()),
        Modification::Allowed(kinds) => info
            .details
            .push(format!("The document has been modified since this signature was applied, but the changes are permitted ({}).", kinds.join(", "))),
        Modification::Disallowed(kinds) => {
            info.status = Status::Invalid;
            info.details
                .push(format!("The document has been altered since this signature was applied in ways it does not permit ({}).", kinds.join(", ")));
            problems = true;
        }
    }
    if let Some(t) = info.timestamp_time.or(info.signing_time)
        && !cert.valid_at(t)
    {
        info.details.push("The signer's certificate was not valid at the time of signing.".into());
        // A verified revocation already made the signature invalid; keep it that way.
        if !problems && !revoked {
            info.status = Status::Unknown;
        }
        problems = true;
    }
    if let Some(why) = cert.signing_problem() {
        info.details.push(why);
        if !problems && !revoked {
            info.status = Status::Unknown;
        }
        problems = true;
    }
    if !trusted {
        info.details.push(
            "The signer's identity is unknown because it has not been included in your list of trusted certificates and none of its parent certificates are trusted certificates."
                .into(),
        );
        if !problems && !revoked {
            info.status = Status::Unknown;
        }
    } else {
        info.details.push("The signer's identity is valid.".into());
        match anchor {
            Some((root, TrustSource::BuiltinRoots)) => {
                info.details.push(format!("Its certificate chains to a root built into PdfCraft that you switched on ({root})."))
            }
            Some((root, TrustSource::CcaIndia)) => info.details.push(format!(
                "Its certificate chains to {root}, a root of India's PKI in the {} trust set that you switched on.",
                crate::trust::CCA_INDIA
            )),
            Some((root, TrustSource::List(list))) => info.details.push(format!("Its certificate chains to {root} in the trust list \"{list}\".")),
            _ => {}
        }
        if !problems && !revoked {
            info.status = Status::Valid;
        }
    }
    if info.timestamp {
        info.details.push("The signature includes an embedded timestamp.".into());
    } else {
        info.details.push("Signing time is from the clock on the signer's computer.".into());
    }
    if let Some(t) = info.timestamp_time {
        info.details.push(format!("The embedded timestamp token is valid and its authority is trusted; trusted time is {t}."));
    } else if let Some(t) = unverified_time {
        info.details.push(format!(
            "The signature carries an unverified timestamp ({t}): the timestamp authority is not in your list of trusted certificates, so the signing time claimed by the signer is used instead."
        ));
    }
}

/// Check the revocation evidence in the catalog's `/DSS` against the signer's chain at `at`.
/// Only evidence whose signature verifies against the issuer counts; returns whether the
/// certificate is revoked (which invalidates the signature).
fn check_dss_revocation(doc: &Document, signer: Option<&Certificate>, at: &Option<Time>, info: &mut SignatureInfo) -> bool {
    use crate::revocation::{CertificateList, OcspResponse, RevocationStatus};
    let (Some(signer), Some(at)) = (signer, *at) else { return false };
    let Some(root) = doc.root() else { return false };
    let Some(dss) = doc.get(root).as_dict().and_then(|c| c.get(b"DSS").cloned()).map(|d| doc.resolve(&d)).and_then(|d| d.as_dict().cloned()) else {
        return false;
    };
    let pull = |key: &[u8]| -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for item in dss.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default() {
            let o = doc.resolve(&item);
            if let pdfcraft_cos::Object::Stream(s) = &*o
                && let Ok(bytes) = s.decoded()
            {
                out.push(bytes);
            }
        }
        out
    };
    let issuer = info.chain.get(1).unwrap_or(signer);
    // A verified revocation wins; "good" from any covering evidence is reported, but never
    // contradicts a revocation.
    let mut revoked = None;
    let mut covered = false;
    let mut sources = Vec::new();
    for r in pull(b"CRLs") {
        let Ok(list) = CertificateList::parse(&r) else { continue };
        sources.push("CRL");
        match list.check(signer, issuer, at) {
            RevocationStatus::Revoked { at } => {
                revoked = Some(at);
                break;
            }
            RevocationStatus::Good => covered = true,
            RevocationStatus::Unknown => {}
        }
    }
    if revoked.is_none() {
        for r in pull(b"OCSPs") {
            let Ok(resp) = OcspResponse::parse(&r) else { continue };
            sources.push("OCSP");
            match resp.check(signer, issuer, at) {
                RevocationStatus::Revoked { at } => {
                    revoked = Some(at);
                    break;
                }
                RevocationStatus::Good => covered = true,
                RevocationStatus::Unknown => {}
            }
        }
    }
    match revoked {
        Some(at) => {
            info.status = Status::Invalid;
            info.details.push(format!("The signer's certificate has been revoked ({}, {}).", sources.join(", "), at));
            true
        }
        None if covered => {
            info.details.push("The document's embedded revocation information shows that the signer's certificate has not been revoked.".into());
            false
        }
        None => false,
    }
}

/// Validate a standalone document timestamp dictionary (`/ETSI.RFC3161`): the token must be
/// cryptographically valid and its message imprint must cover the file's signed bytes.
fn validate_doc_timestamp(doc: &Document, bytes: &[u8], v: &Dict, info: &mut SignatureInfo, cache: &DigestCache, trust: &TrustStore) {
    info.date = text(doc, v, b"M");
    let invalid = |info: &mut SignatureInfo, why: &str| {
        info.status = Status::Invalid;
        info.details.push(why.to_string());
    };
    let Some(br) = nums(doc, v, b"ByteRange").filter(|b| b.len() == 4 && b.iter().all(|x| *x >= 0.0)) else {
        return invalid(info, "The timestamp has no valid byte range.");
    };
    let [o0, l0, o1, l1] = [br[0] as usize, br[1] as usize, br[2] as usize, br[3] as usize];
    if o0 != 0 || o1 < l0 || o1.checked_add(l1).is_none_or(|end| end > bytes.len()) || o1 - l0 < 2 {
        return invalid(info, "The timestamp's byte range does not match the file.");
    }
    let gap = &bytes[l0..o1];
    if gap.first() != Some(&b'<') || gap.last() != Some(&b'>') {
        return invalid(info, "The timestamp's byte range does not exclude exactly its contents.");
    }
    let Some(contents) = unhex(&gap[1..gap.len() - 1]) else { return invalid(info, "The timestamp contents are not hexadecimal.") };
    let covered = o1 + l1;
    info.signed_len = covered;
    info.revision = cache.revision(bytes, covered).map_or(1, |d| d.revisions().len().max(1));
    let token = match crate::timestamp::parse_token(&contents) {
        Ok(t) => t,
        Err(e) => return invalid(info, &format!("The timestamp token could not be read ({e}).")),
    };
    info.digest = Some(token.digest);
    info.timestamp_time = Some(token.gen_time);
    info.algorithm = Some(format!("RFC 3161 timestamp ({})", token.digest.name()));
    let doc_digest = token.digest.digest(&[&bytes[..l0], &bytes[o1..covered]]);
    if doc_digest != token.imprint {
        return invalid(info, "The document has been altered or corrupted since the timestamp was applied.");
    }
    if let Some(cert) = token.signer_certificate() {
        info.signer = Some(cert.display_name());
        info.certificate = Some(cert.clone());
        if !cert.valid_at(token.gen_time) {
            info.status = Status::Unknown;
            info.details.push("The timestamp authority's certificate was not valid at the time of timestamping.".into());
        }
    }
    info.modification = if covered == bytes.len() || bytes[covered..].iter().all(|b| b.is_ascii_whitespace() || *b == 0) {
        Modification::None
    } else {
        classify_changes(doc, cache.revision(bytes, covered), None)
    };
    match &info.modification {
        Modification::None => info.details.push("This document has not been modified since this timestamp was applied.".into()),
        Modification::Allowed(kinds) => info
            .details
            .push(format!("The document has been modified since this timestamp was applied, but the changes are permitted ({}).", kinds.join(", "))),
        Modification::Disallowed(kinds) => {
            info.status = Status::Invalid;
            info.details
                .push(format!("The document has been altered since this timestamp was applied in ways it does not permit ({}).", kinds.join(", ")));
        }
    }
    if info.status != Status::Invalid {
        // An untrusted TSA is not proof of anything: the verdict stays Unknown, like an
        // ordinary signature from an unknown signer.
        let embedded = crate::timestamp::token_certs(&contents);
        // A trusted authority whose certificate was not valid at the time it stamped proves
        // nothing either (its detail is already recorded above): never upgrade that to Valid.
        let in_validity = token.signer_certificate().is_some_and(|c| c.valid_at(token.gen_time));
        let (trusted_tsa, wrong_purpose) =
            token.signer_certificate().map_or((false, false), |c| trust.trusts_timestamp_authority(&c, &embedded, token.gen_time));
        if trusted_tsa && in_validity {
            info.status = Status::Valid;
            info.details.push("The timestamp token is valid and its authority is trusted.".into());
        } else if trusted_tsa {
            info.status = Status::Unknown;
        } else {
            info.status = Status::Unknown;
            info.details.push("The timestamp token is valid, but the timestamp authority is not in your list of trusted certificates.".into());
            if wrong_purpose {
                info.details.push(NOT_A_TSA.into());
            }
        }
    }
}

/// The DER `/Contents` of every signature dictionary in the file (field signatures and
/// document timestamps), for `/VRI` keys and evidence selection.
pub(crate) fn signature_contents(doc: &Document) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut seen: HashSet<ObjRef> = HashSet::new();
    let mut add = |d: &Dict, r: Option<ObjRef>| {
        if let Some(r) = r
            && !seen.insert(r)
        {
            return;
        }
        let contents = d.get(b"Contents").map(|c| doc.resolve(c));
        let Some(s) = contents.as_deref().and_then(Object::as_string) else { return };
        // Hex strings decode to the DER when parsed; a literal string carries hex text.
        let der = if s.hex { Some(s.bytes.clone()) } else { unhex(&s.bytes) };
        if let Some(der) = der {
            out.push(der);
        }
    };
    for f in sig_fields(doc) {
        if let Some(v) = f.dict.get(b"V") {
            let d = doc.resolve(v);
            if let Some(dd) = d.as_dict() {
                add(dd, v.as_ref());
            }
        }
    }
    for (r, o) in doc.scan_objects() {
        let Some(d) = o.as_dict() else { continue };
        if is_doc_timestamp(d) {
            add(d, Some(r));
        }
    }
    out
}

/// `adbe.x509.rsa_sha1` (ISO 32000-2 §12.8.3.2, deprecated): `/Contents` is the PKCS #1
/// signature as a DER OCTET STRING over the SHA-1 digest of the byte ranges, and `/Cert` holds the
/// signer's certificate (then any others) as DER byte strings.
#[allow(clippy::too_many_arguments)]
fn validate_x509_rsa_sha1(
    doc: &Document,
    bytes: &[u8],
    trust: &TrustStore,
    v: &Dict,
    info: &mut SignatureInfo,
    cache: &DigestCache,
    (l0, o1, covered): (usize, usize, usize),
    contents: &[u8],
) {
    let fail = |info: &mut SignatureInfo, why: &str| {
        info.status = Status::Invalid;
        info.details.push(why.to_string());
    };
    let certs: Vec<Certificate> = match v.get(b"Cert").map(|c| doc.resolve(c)).as_deref() {
        Some(Object::Array(a)) => a.iter().filter_map(|c| doc.resolve(c).as_string().map(|s| s.bytes.clone())).collect::<Vec<_>>(),
        Some(Object::String(s)) => vec![s.bytes.clone()],
        _ => Vec::new(),
    }
    .iter()
    .filter_map(|raw| Certificate::parse(raw).ok())
    .collect();
    let Some(cert) = certs.first().cloned() else { return fail(info, "The signer's certificate is not in the signature.") };
    let signature = match crate::der::Tlv::parse_ber(contents).and_then(|(t, _)| t.octets("signature").map(|o| o.into_owned())) {
        Ok(sig) => sig,
        Err(e) => return fail(info, &format!("The signature could not be read ({e}).")),
    };
    info.digest = Some(DigestAlg::Sha1);
    info.algorithm = Some(format!("{} with SHA-1", cert.public_key.describe()));
    info.signer = Some(cert.display_name());
    let digest = cache.digest(DigestAlg::Sha1, bytes, l0, o1, covered);
    let mut notes = Vec::new();
    match cert.public_key.verify_noting(crate::keys::Scheme::RsaPkcs1, DigestAlg::Sha1, &digest, &signature, &mut notes) {
        Ok(true) => info.details.extend(notes.into_iter().map(String::from)),
        Ok(false) => {
            info.certificate = Some(cert);
            return fail(info, "The document has been altered or corrupted since the signature was applied.");
        }
        // Only what PdfCraft doesn't support yet is "can't check"; a key or value that is
        // malformed is a signature that doesn't verify (as on the CMS path).
        Err(SignError::Unsupported(e)) => {
            info.certificate = Some(cert);
            info.status = Status::Unknown;
            info.details.push(format!("PdfCraft can't check this signature yet: {e}."));
            return;
        }
        Err(e) => {
            info.certificate = Some(cert);
            return fail(info, &format!("The signature could not be verified ({e})."));
        }
    }
    info.details.push("The signature uses the legacy adbe.x509.rsa_sha1 format.".into());
    info.details.extend(DigestAlg::Sha1.weakness().map(String::from));
    finish_validation(doc, bytes, trust, info, cache, covered, cert, &certs, None, None);
}

/// What later revisions changed, classified as Acrobat reports it, under DocMDP `p` (or none:
/// an approval signature permits form fill, comments and further signatures).
fn classify_changes(doc: &Document, old: Option<Document>, p: Option<u8>) -> Modification {
    let Some(old) = old else {
        return Modification::Disallowed(vec!["the signed version could not be read".into()]);
    };
    let old_content: HashSet<ObjRef> = pdfcraft_annot::page_refs(&old)
        .unwrap_or_default()
        .into_iter()
        .flat_map(|pg| {
            let c = old.get(pg).as_dict().and_then(|d| d.get(b"Contents").cloned());
            match c {
                Some(Object::Ref(r)) => match &*old.get(r) {
                    Object::Array(a) => a.iter().filter_map(Object::as_ref).chain([r]).collect::<Vec<_>>(),
                    _ => vec![r],
                },
                Some(Object::Array(a)) => a.iter().filter_map(Object::as_ref).collect(),
                _ => Vec::new(),
            }
        })
        .collect();
    // The signed revision's document-level XMP stream.
    let old_metadata = old.root().and_then(|root| old.get(root).as_dict().and_then(|c| c.get(b"Metadata").and_then(Object::as_ref)));
    let (mut allowed, mut disallowed): (Vec<&str>, Vec<&str>) = (Vec::new(), Vec::new());
    let dss = dss_parts(doc, &old);
    // Stored at the same place in both (and not edited since): the same bytes, unchanged.
    let same_place = |num: u32| -> bool {
        use pdfcraft_cos::XrefEntry;
        if doc.is_edited(num) {
            return false;
        }
        match (doc.xref_entry(num), old.xref_entry(num)) {
            (Some(a @ XrefEntry::InFile { .. }), Some(b)) => a == b,
            (Some(a @ XrefEntry::InStream { stream, .. }), Some(b)) => {
                a == b && !doc.is_edited(stream) && doc.xref_entry(stream) == old.xref_entry(stream)
            }
            _ => false,
        }
    };
    for num in doc.object_numbers() {
        if same_place(num) {
            continue;
        }
        let r = ObjRef { num, generation: doc.generation(num) };
        let new = doc.get(r);
        let before = old.try_get(num).ok().filter(|o| !matches!(**o, Object::Null));
        if before.as_deref() == Some(&*new) {
            continue;
        }
        let kind = match &*new {
            // The Document Security Store's own arrays and dictionaries, when they only grew.
            _ if dss.contains(&num) => "document security store",
            Object::Dict(d) => change_kind(doc, &old, d, before.as_deref().and_then(Object::as_dict)),
            Object::Stream(s) => {
                if old_content.contains(&r) {
                    "page content"
                } else if s.dict.name(b"Type") == Some(b"XRef") || s.dict.name(b"Type") == Some(b"ObjStm") || before.is_none() {
                    continue;
                } else if s.dict.name(b"Type") == Some(b"Metadata") && Some(r) == old_metadata {
                    // Only the signed catalog's XMP stream. A /Type /Metadata label on anything
                    // else, even one it already had when signed (a page's Form XObject: the
                    // renderer ignores /Type), does not make rewriting it a metadata change.
                    "metadata"
                } else {
                    "other changes"
                }
            }
            _ if before.is_none() => continue,
            _ => "other changes",
        };
        let ok = match (kind, p) {
            ("signature" | "document security store", _) => true,
            ("form fill", Some(2 | 3) | None) => true,
            ("comments", Some(3) | None) => true,
            // Every save updates /Info (ModDate), signing included.
            ("metadata", _) => true,
            _ => false,
        };
        let list = if ok { &mut allowed } else { &mut disallowed };
        if !list.contains(&kind) {
            list.push(kind);
        }
    }
    let own = |v: Vec<&str>| v.into_iter().map(str::to_string).collect::<Vec<_>>();
    if disallowed.is_empty() { Modification::Allowed(own(allowed)) } else { Modification::Disallowed(own(disallowed)) }
}

/// Where an object sits in the Document Security Store (ISO 32000-2 §12.8.4.3).
#[derive(Clone, Copy, PartialEq, Eq)]
enum DssRole {
    /// The `/DSS` dictionary.
    Dss,
    /// A `/Certs`, `/CRLs`, `/OCSPs` (or a VRI entry's `/Cert`, `/CRL`, `/OCSP`) array.
    DataArray,
    /// A certificate, CRL, OCSP response or timestamp token stream.
    Data,
    /// The `/VRI` dictionary.
    VriMap,
    /// One entry of `/VRI`.
    VriEntry,
}

/// The indirect objects the catalog's `/DSS` reaches by the keys the standard defines, and the
/// role each plays there. Any other key is not followed: `/DSS << /X 5 0 R >>` does not make
/// object 5 part of the store.
fn dss_walk(doc: &Document) -> Vec<(ObjRef, DssRole)> {
    const LIMIT: usize = 100_000;
    let mut out = Vec::new();
    let Some(dss) = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|c| c.get(b"DSS").cloned())) else { return out };
    // The roles nest at most four deep (DSS, VRI, entry, array, data), so no seen-set is needed.
    let mut stack = vec![(dss, DssRole::Dss)];
    let mut steps = 0usize;
    while let Some((o, role)) = stack.pop() {
        steps += 1;
        if steps > LIMIT {
            break;
        }
        if let Object::Ref(r) = &o {
            out.push((*r, role));
        }
        match (role, &*doc.resolve(&o)) {
            (DssRole::Dss, Object::Dict(d)) => {
                for (k, v) in d.iter() {
                    match k.as_slice() {
                        b"Certs" | b"CRLs" | b"OCSPs" => stack.push((v.clone(), DssRole::DataArray)),
                        b"VRI" => stack.push((v.clone(), DssRole::VriMap)),
                        _ => {}
                    }
                }
            }
            (DssRole::DataArray, Object::Array(a)) => stack.extend(a.iter().map(|v| (v.clone(), DssRole::Data))),
            (DssRole::VriMap, Object::Dict(d)) => stack.extend(d.iter().map(|(_, v)| (v.clone(), DssRole::VriEntry))),
            (DssRole::VriEntry, Object::Dict(d)) => {
                for (k, v) in d.iter() {
                    match k.as_slice() {
                        b"Cert" | b"CRL" | b"OCSP" => stack.push((v.clone(), DssRole::DataArray)),
                        b"TS" => stack.push((v.clone(), DssRole::Data)),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Whether `now` only adds to `before`: an array keeps every element, a dictionary keeps every
/// entry (an entry that is an array or dictionary may itself have grown).
fn only_grew(doc: &Document, old: &Document, before: &Object, now: &Object, depth: u8) -> bool {
    /// Arrays of anything but references are compared element by element (quadratic), so only
    /// up to this size; a store's `/Certs`/`/CRLs`/`/OCSPs` are references, compared as sets.
    const MAX_COMPARED: usize = 10_000;
    match (before, now) {
        (Object::Array(b), Object::Array(n)) => {
            if b.len() > n.len() {
                return false;
            }
            let refs = |a: &[Object]| a.iter().map(Object::as_ref).collect::<Option<HashSet<ObjRef>>>();
            match (refs(b), refs(n)) {
                (Some(b), Some(n)) => b.is_subset(&n),
                _ => b.len() <= MAX_COMPARED && n.len() <= MAX_COMPARED && b.iter().all(|x| n.contains(x)),
            }
        }
        (Object::Dict(b), Object::Dict(n)) => b.iter().all(|(k, v)| match n.get(k) {
            Some(nv) => nv == v || (depth < 4 && only_grew(doc, old, &old.resolve(v), &doc.resolve(nv), depth + 1)),
            None => false,
        }),
        _ => before == now,
    }
}

/// The objects of the Document Security Store that count as validation data added after
/// signing: its dictionary, `/Certs`/`/CRLs`/`/OCSPs` arrays, `/VRI` and its entries, when
/// - they are reached by the standard's keys and have the shape of their role, and
/// - they are new, or were already part of the signed store and have only grown.
///
/// An existing object is never made "document security store" by pointing the store at it: the
/// page's `/Contents` array, or a dictionary labelled `/Type /DSS`, keep being judged as what they
/// are. The data streams need no entry here: new objects are not changes, and rewriting an
/// existing one is judged on its own.
fn dss_parts(doc: &Document, old: &Document) -> HashSet<u32> {
    let signed: HashSet<u32> = dss_walk(old).into_iter().map(|(r, _)| r.num).collect();
    let is_stream = |o: &Object| matches!(&*doc.resolve(o), Object::Stream(_));
    let mut parts = HashSet::new();
    for (r, role) in dss_walk(doc) {
        let now = doc.get(r);
        let shaped = match (role, &*now) {
            (DssRole::Dss, Object::Dict(d)) => d.iter().all(|(k, _)| matches!(k.as_slice(), b"Type" | b"Certs" | b"CRLs" | b"OCSPs" | b"VRI")),
            (DssRole::DataArray, Object::Array(a)) => a.iter().all(|e| matches!(e, Object::Ref(_)) && is_stream(e)),
            (DssRole::VriMap, Object::Dict(d)) => d.iter().all(|(_, v)| matches!(&*doc.resolve(v), Object::Dict(_))),
            (DssRole::VriEntry, Object::Dict(d)) => {
                d.iter().all(|(k, _)| matches!(k.as_slice(), b"Type" | b"Cert" | b"CRL" | b"OCSP" | b"TU" | b"TS"))
            }
            _ => false,
        };
        let before = old.try_get(r.num).ok().filter(|o| !matches!(**o, Object::Null));
        let history = match before {
            None => true,
            Some(b) => signed.contains(&r.num) && only_grew(doc, old, &b, &now, 0),
        };
        if shaped && history {
            parts.insert(r.num);
        }
    }
    parts
}

/// What adding `added` annotations amounts to: signing (signature widgets only), form fill
/// (other widgets) or comments.
fn added_annots(doc: &Document, added: &[ObjRef]) -> &'static str {
    let kinds: Vec<(bool, bool)> = added
        .iter()
        .map(|r| {
            let o = doc.get(*r);
            let d = o.as_dict();
            (d.is_some_and(|d| d.name(b"Subtype") == Some(b"Widget")), d.is_some_and(|d| d.name(b"FT") == Some(b"Sig")))
        })
        .collect();
    if !kinds.is_empty() && kinds.iter().all(|(w, s)| *w && *s) {
        "signature"
    } else if !kinds.is_empty() && kinds.iter().all(|(w, _)| *w) {
        "form fill"
    } else {
        "comments"
    }
}

fn refs(doc: &Document, o: Option<&Object>) -> Vec<ObjRef> {
    o.map(|o| doc.resolve(o)).and_then(|a| a.as_array().map(|a| a.iter().filter_map(Object::as_ref).collect())).unwrap_or_default()
}

/// The interactive form dictionary of a document, resolved.
fn acroform(doc: &Document) -> Option<Dict> {
    let c = doc.get(doc.root()?).as_dict()?.get(b"AcroForm").cloned()?;
    doc.resolve(&c).as_dict().cloned()
}

/// An AcroForm change that only adds signature fields (and sets `/SigFlags`) is signing.
/// Without `before`, the signed revision's AcroForm (wherever it was) is the reference.
fn acroform_kind(doc: &Document, old: &Document, now: &Dict, before: Option<&Dict>) -> &'static str {
    let fallback = acroform(old).unwrap_or_default();
    let before = before.unwrap_or(&fallback);
    let strip = |d: &Dict| -> Vec<(Vec<u8>, Object)> {
        let mut v: Vec<(Vec<u8>, Object)> =
            d.iter().filter(|(k, _)| !matches!(k.as_slice(), b"Fields" | b"SigFlags")).map(|(k, o)| (k.to_vec(), o.clone())).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    };
    if strip(now) != strip(before) {
        return "form fill";
    }
    let old_fields = refs(old, before.get(b"Fields"));
    let added: Vec<ObjRef> = refs(doc, now.get(b"Fields")).into_iter().filter(|r| !old_fields.contains(r)).collect();
    if added.is_empty() || added_annots(doc, &added) == "signature" { "signature" } else { "form fill" }
}

fn change_kind(doc: &Document, old: &Document, d: &Dict, before: Option<&Dict>) -> &'static str {
    let ty = d.name(b"Type");
    let without = |x: &Dict, keys: &[&[u8]]| -> Vec<(Vec<u8>, Object)> {
        let mut v: Vec<(Vec<u8>, Object)> = x.iter().filter(|(k, _)| !keys.contains(&k.as_slice())).map(|(k, o)| (k.to_vec(), o.clone())).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    };
    match ty {
        Some(b"Sig" | b"DocTimeStamp" | b"SigRef" | b"TransformParams") => return "signature",
        Some(b"Catalog") => {
            let keys: &[&[u8]] = &[b"AcroForm", b"DSS", b"Perms", b"Metadata", b"NeedsRendering"];
            return match before {
                Some(b) if without(d, keys) != without(b, keys) => "document structure",
                Some(b) => {
                    if d.get(b"AcroForm") != b.get(b"AcroForm") {
                        // Compare the forms themselves, inline or not.
                        let now = acroform(doc).unwrap_or_default();
                        acroform_kind(doc, old, &now, None)
                    } else if b.get(b"Perms") != d.get(b"Perms") {
                        "signature"
                    } else {
                        "metadata"
                    }
                }
                None => "form fill",
            };
        }
        Some(b"Page") => {
            return match before {
                Some(b) if without(d, &[b"Annots"]) != without(b, &[b"Annots"]) => "page content",
                Some(b) => {
                    let was = refs(old, b.get(b"Annots"));
                    let added: Vec<ObjRef> = refs(doc, d.get(b"Annots")).into_iter().filter(|r| !was.contains(r)).collect();
                    added_annots(doc, &added)
                }
                None => "comments",
            };
        }
        Some(b"Pages") => return if before.is_some() { "pages added or removed" } else { "document structure" },
        _ => {}
    }
    match d.name(b"Subtype") {
        Some(b"Widget") => return if d.name(b"FT") == Some(b"Sig") { "signature" } else { "form fill" },
        Some(b"Link") => return "links",
        Some(_) if ty == Some(b"Annot") || d.contains(b"Rect") => return "comments",
        _ => {}
    }
    if d.name(b"FT") == Some(b"Sig") {
        return "signature";
    }
    if d.contains(b"Fields") {
        return acroform_kind(doc, old, d, before);
    }
    if d.contains(b"FT") || (d.contains(b"T") && d.contains(b"Kids")) || d.contains(b"DR") {
        return "form fill";
    }
    if d.contains(b"Producer") || d.contains(b"ModDate") || d.contains(b"CreationDate") {
        return "metadata";
    }
    if before.is_none() {
        // New objects referenced by changed ones (fonts, appearance resources).
        return "form fill";
    }
    "other changes"
}

// ── signing ─────────────────────────────────────────────────────────────────────────────────

/// What the visible signature shows (Acrobat: Configure Signature Appearance).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Appearance {
    pub name: bool,
    pub date: bool,
    pub reason: bool,
    pub location: bool,
    pub distinguished_name: bool,
    pub labels: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self { name: true, date: true, reason: false, location: false, distinguished_name: false, labels: true }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SignOptions {
    /// Sign this existing, unsigned signature field.
    pub field: Option<String>,
    /// Or create a field on this page (0-based) with this rectangle (`None`: invisible).
    pub page: usize,
    pub rect: Option<[f64; 4]>,
    /// Name for a new field (default `Signature1`, `Signature2`…).
    pub new_field_name: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact: Option<String>,
    /// The signing time as a PDF date (`/M`), from the caller's clock.
    pub date: String,
    /// Certify with these DocMDP permissions (1 no changes, 2 form fill and signing, 3 also comments).
    pub certify: Option<u8>,
    pub appearance: Appearance,
}

/// What an encrypted document's permissions (ISO 32000-2 §7.6.4.2, Table 22) allow when it was
/// opened with the user password (the owner may do anything): signing an existing, empty
/// signature field is filling in a form field (bit 9, or bit 6); creating a new signature
/// field needs bits 6 and 4; certifying sets the catalog's `/Perms`, a document change (bit 4).
/// The signature's `/Contents` stays unencrypted (`pdfcraft-cos` exempts it, §7.6.2), so the
/// placeholder can be patched after hashing exactly as in an unencrypted file.
fn check_signing_permissions(doc: &Document, opts: &SignOptions) -> Result<(), SignError> {
    let Some(p) = doc.permissions() else { return Ok(()) };
    let refuse = |what: &str| {
        Err(SignError::Pdf(format!(
            "the document's security settings don't allow {what}; open it with the permissions password, or sign an existing signature field"
        )))
    };
    if opts.field.is_some() && !p.fill_forms() {
        return Err(SignError::Pdf(
            "the document's security settings don't allow filling in form fields or signing; open it with the permissions password to sign it".into(),
        ));
    }
    if opts.field.is_none() && !(p.annotate() && p.modify()) {
        return refuse("adding new signature fields");
    }
    if opts.certify.is_some() && !p.modify() {
        return refuse("certifying it");
    }
    Ok(())
}

/// Placeholder `/ByteRange` values: fixed width, patched after writing.
const BR_MARK: [i64; 3] = [1_111_111_111, 2_222_222_222, 3_333_333_333];

/// Sign `doc` with `id`; returns the signed file (an incremental update when possible).
pub fn sign(doc: &Document, id: &DigitalId, opts: &SignOptions) -> Result<Vec<u8>, SignError> {
    sign_inner(doc, id, opts, None)
}

/// Sign and embed an RFC 3161 signature timestamp from `tsa` (PAdES B-T). The transport is the
/// caller's: this crate never opens a socket, and a rejected or malformed token fails the
/// signing before any file is produced.
pub fn sign_with_timestamp(
    doc: &Document,
    id: &DigitalId,
    opts: &SignOptions,
    tsa: &dyn crate::timestamp::TimestampAuthority,
) -> Result<Vec<u8>, SignError> {
    sign_inner(doc, id, opts, Some(tsa))
}

/// Extra `/Contents` headroom for an attached RFC 3161 token (a TSA response with its
/// certificate chain is typically 4–8 KiB; the token is also size-capped on parsing).
const TOKEN_RESERVE: usize = 16 * 1024;

fn sign_inner(
    doc: &Document,
    id: &DigitalId,
    opts: &SignOptions,
    tsa: Option<&dyn crate::timestamp::TimestampAuthority>,
) -> Result<Vec<u8>, SignError> {
    // A pending security change would make the incremental save a full rewrite that applies
    // or removes protection as it signs (and breaks existing signatures): save it first.
    if doc.encryption_changed() {
        return Err(SignError::Pdf("save the document's new security settings before signing it".into()));
    }
    check_signing_permissions(doc, opts)?;
    let mut doc = doc.clone();
    let root = doc.root().ok_or_else(|| SignError::Pdf("the document has no catalog".into()))?;
    let alg = id.key.preferred_digest();
    let reserve =
        8192 + id.certificate.raw.len() + id.chain.iter().map(|c| c.raw.len()).sum::<usize>() + if tsa.is_some() { TOKEN_RESERVE } else { 0 };
    let name = id.certificate.display_name();
    // The signature dictionary.
    let mut v = Dict::new();
    v.set(b"Type".to_vec(), Object::name("Sig"));
    v.set(b"Filter".to_vec(), Object::name("Adobe.PPKLite"));
    v.set(b"SubFilter".to_vec(), Object::name("ETSI.CAdES.detached"));
    v.set(b"ByteRange".to_vec(), Object::Array([0].iter().chain(BR_MARK.iter()).map(|n| Object::Int(*n)).collect()));
    v.set(b"Contents".to_vec(), Object::String(PdfString { bytes: vec![0; reserve], hex: true }));
    v.set(b"M".to_vec(), PdfString::literal(opts.date.as_bytes().to_vec()));
    v.set(b"Name".to_vec(), PdfString::text(&name));
    for (k, val) in [(&b"Reason"[..], &opts.reason), (b"Location", &opts.location), (b"ContactInfo", &opts.contact)] {
        if let Some(s) = val.as_deref().filter(|s| !s.trim().is_empty()) {
            v.set(k.to_vec(), PdfString::text(s));
        }
    }
    let mut app = Dict::new();
    app.set(b"Name".to_vec(), Object::name("PdfCraft"));
    let mut build = Dict::new();
    build.set(b"App".to_vec(), Object::Dict(app));
    v.set(b"Prop_Build".to_vec(), Object::Dict(build));
    if let Some(p) = opts.certify {
        let mut tp = Dict::new();
        tp.set(b"Type".to_vec(), Object::name("TransformParams"));
        tp.set(b"P".to_vec(), Object::Int(p.clamp(1, 3) as i64));
        tp.set(b"V".to_vec(), Object::name("1.2"));
        let mut sr = Dict::new();
        sr.set(b"Type".to_vec(), Object::name("SigRef"));
        sr.set(b"TransformMethod".to_vec(), Object::name("DocMDP"));
        sr.set(b"TransformParams".to_vec(), Object::Dict(tp));
        v.set(b"Reference".to_vec(), Object::Array(vec![Object::Dict(sr)]));
    }
    let sig = doc.add(Object::Dict(v.clone()));
    if opts.certify.is_some() {
        // Catalog /Perms /DocMDP points at the certification signature.
        let mut perms = Dict::new();
        perms.set(b"DocMDP".to_vec(), Object::Ref(sig));
        doc.update_dict(root, |c| c.set(b"Perms".to_vec(), Object::Dict(perms)))?;
    }
    // The field and its widget.
    let pages = pdfcraft_annot::page_refs(&doc).map_err(|e| SignError::Pdf(e.to_string()))?;
    let existing = opts
        .field
        .as_deref()
        .map(|n| sig_fields(&doc).into_iter().find(|f| f.name == n).ok_or_else(|| SignError::Pdf(format!("there is no signature field {n:?}"))));
    let (widget, rect) = match existing {
        Some(f) => {
            let f = f?;
            if f.dict.contains(b"V") {
                return Err(SignError::Pdf(format!("the field {:?} is already signed", f.name)));
            }
            let fr = f.r.ok_or_else(|| SignError::Pdf("the field is not an indirect object".into()))?;
            doc.update_dict(fr, |d| d.set(b"V".to_vec(), Object::Ref(sig)))?;
            let (wr, wd) = f.widgets.first().cloned().ok_or_else(|| SignError::Pdf("the field has no widget".into()))?;
            let rect = nums(&doc, &wd, b"Rect").filter(|r| r.len() == 4).map(|r| [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]);
            (wr.unwrap_or(fr), rect.unwrap_or([0.0; 4]))
        }
        None => {
            let page = *pages.get(opts.page).ok_or_else(|| SignError::Pdf(format!("page {} does not exist", opts.page + 1)))?;
            let taken: HashSet<String> = sig_fields(&doc).into_iter().map(|f| f.name).collect();
            let fname = opts
                .new_field_name
                .clone()
                .unwrap_or_else(|| (1..).map(|i| format!("Signature{i}")).find(|n| !taken.contains(n)).unwrap_or_default());
            let rect = opts.rect.unwrap_or([0.0; 4]);
            let mut w = Dict::new();
            w.set(b"Type".to_vec(), Object::name("Annot"));
            w.set(b"Subtype".to_vec(), Object::name("Widget"));
            w.set(b"FT".to_vec(), Object::name("Sig"));
            w.set(b"T".to_vec(), PdfString::text(&fname));
            w.set(b"V".to_vec(), Object::Ref(sig));
            w.set(b"Rect".to_vec(), Object::Array(rect.iter().map(|x| Object::Real(*x)).collect()));
            w.set(b"P".to_vec(), Object::Ref(page));
            // Print + Locked.
            w.set(b"F".to_vec(), Object::Int(4 | 128));
            let wr = doc.add(Object::Dict(w));
            // Page /Annots (indirect arrays are updated in place).
            let pd = doc.get(page).as_dict().cloned().unwrap_or_default();
            match pd.get(b"Annots") {
                Some(Object::Ref(ar)) => {
                    let mut a = doc.get(*ar).as_array().cloned().unwrap_or_default();
                    a.push(Object::Ref(wr));
                    doc.set(*ar, Object::Array(a));
                }
                other => {
                    let mut a = other.and_then(|o| o.as_array().cloned()).unwrap_or_default();
                    a.push(Object::Ref(wr));
                    doc.update_dict(page, |d| d.set(b"Annots".to_vec(), Object::Array(a)))?;
                }
            }
            // AcroForm /Fields and /SigFlags (signatures exist, append only).
            let catalog = doc.get(root).as_dict().cloned().unwrap_or_default();
            let (af_ref, mut af) = match catalog.get(b"AcroForm") {
                Some(Object::Ref(r)) => (Some(*r), doc.get(*r).as_dict().cloned().unwrap_or_default()),
                Some(Object::Dict(d)) => (None, d.clone()),
                _ => (None, Dict::new()),
            };
            let mut fields = match af.get(b"Fields") {
                Some(Object::Ref(r)) => doc.get(*r).as_array().cloned().unwrap_or_default(),
                Some(Object::Array(a)) => a.clone(),
                _ => Vec::new(),
            };
            fields.push(Object::Ref(wr));
            af.set(b"Fields".to_vec(), Object::Array(fields));
            af.set(b"SigFlags".to_vec(), Object::Int(3));
            match af_ref {
                Some(r) => doc.set(r, Object::Dict(af)),
                // Inline (or new) AcroForms stay in the catalog.
                None => doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(af)))?,
            }
            (wr, rect)
        }
    };
    if opts.field.is_some() {
        doc.update_dict(root, |c| {
            if let Some(Object::Dict(af)) = c.get_mut(b"AcroForm") {
                af.set(b"SigFlags".to_vec(), Object::Int(3));
            }
        })?;
        if let Some(Object::Ref(r)) = doc.get(root).as_dict().and_then(|c| c.get(b"AcroForm").cloned()) {
            doc.update_dict(r, |af| af.set(b"SigFlags".to_vec(), Object::Int(3)))?;
        }
    }
    // The appearance.
    let ap = appearance(rect, &name, &id.certificate, opts);
    let ap_ref = doc.add(Object::Stream(ap));
    let mut apd = Dict::new();
    apd.set(b"N".to_vec(), Object::Ref(ap_ref));
    doc.update_dict(widget, |w| w.set(b"AP".to_vec(), Object::Dict(apd)))?;
    // Write with the placeholders, then patch them.
    let save = SaveOptions { mod_date: Some(opts.date.clone()), object_streams: false, ..SaveOptions::default() };
    let mut out = pdfcraft_cos::write_incremental(&doc, &save)?;
    let (br_at, gap) = locate(&out, reserve)?;
    let (start, end) = gap;
    let ranges = [0usize, start, end, out.len() - end];
    let mut br_text = format!("0 {} {} {}", ranges[1], ranges[2], ranges[3]).into_bytes();
    let width = format!("0 {} {} {}", BR_MARK[0], BR_MARK[1], BR_MARK[2]).len();
    if br_text.len() > width {
        return Err(SignError::Pdf("the document is too large to sign".into()));
    }
    br_text.resize(width, b' ');
    out[br_at..br_at + width].copy_from_slice(&br_text);
    let digest = alg.digest(&[&out[..start], &out[end..]]);
    let base = crate::cms::sign_detached(&id.key, &id.certificate, &id.chain, alg, &digest)?;
    let cms = match tsa {
        None => base,
        Some(t) => {
            // The imprint covers the signature value, which attaching an unsigned attribute
            // does not change (PAdES B-T).
            let sd = SignedData::parse(&base)?;
            let q = crate::timestamp::TimestampQuery::new(DigestAlg::Sha256, DigestAlg::Sha256.digest(&[&sd.signer.signature]))?;
            let resp = t.timestamp(&q.encode()?)?;
            let token = crate::timestamp::parse_response(&resp, &q)?;
            crate::cms::attach_timestamp_token(&base, &token.raw)?
        }
    };
    if cms.len() > reserve {
        return Err(SignError::Pdf("the signature does not fit its placeholder".into()));
    }
    let hex: Vec<u8> = cms.iter().flat_map(|b| format!("{b:02X}").into_bytes()).collect();
    out[start + 1..start + 1 + hex.len()].copy_from_slice(&hex);
    Ok(out)
}

/// Append a document timestamp (ISO 32000-2 §12.8.2.2): a standalone signature dictionary
/// whose `/Contents` is an RFC 3161 token covering the whole current file (`/ETSI.RFC3161`).
/// The transport is the caller's; a rejected or malformed token produces no file.
pub fn timestamp_document(doc: &Document, tsa: &dyn crate::timestamp::TimestampAuthority, date: &str) -> Result<Vec<u8>, SignError> {
    // A pending security change would make the incremental save a full rewrite that applies
    // or removes protection as it signs (and breaks existing signatures): save it first.
    if doc.encryption_changed() {
        return Err(SignError::Pdf("save the document's new security settings before signing it".into()));
    }
    if doc.permissions().is_some_and(|p| !p.fill_forms()) {
        return Err(SignError::Pdf(
            "the document's security settings don't allow signing; open it with the permissions password to timestamp it".into(),
        ));
    }
    let mut doc = doc.clone();
    let mut v = Dict::new();
    v.set(b"Type".to_vec(), Object::name("DocTimeStamp"));
    v.set(b"Filter".to_vec(), Object::name("Adobe.PPKLite"));
    v.set(b"SubFilter".to_vec(), Object::name("ETSI.RFC3161"));
    v.set(b"ByteRange".to_vec(), Object::Array([0].iter().chain(BR_MARK.iter()).map(|n| Object::Int(*n)).collect()));
    v.set(b"Contents".to_vec(), Object::String(PdfString { bytes: vec![0; TOKEN_RESERVE], hex: true }));
    v.set(b"M".to_vec(), PdfString::literal(date.as_bytes().to_vec()));
    let mut app = Dict::new();
    app.set(b"Name".to_vec(), Object::name("PdfCraft"));
    let mut build = Dict::new();
    build.set(b"App".to_vec(), Object::Dict(app));
    v.set(b"Prop_Build".to_vec(), Object::Dict(build));
    doc.add(Object::Dict(v));
    let save = SaveOptions { mod_date: Some(date.to_string()), object_streams: false, ..SaveOptions::default() };
    let mut out = pdfcraft_cos::write_incremental(&doc, &save)?;
    let (br_at, gap) = locate(&out, TOKEN_RESERVE)?;
    let (start, end) = gap;
    let ranges = [0usize, start, end, out.len() - end];
    let mut br_text = format!("0 {} {} {}", ranges[1], ranges[2], ranges[3]).into_bytes();
    let width = format!("0 {} {} {}", BR_MARK[0], BR_MARK[1], BR_MARK[2]).len();
    if br_text.len() > width {
        return Err(SignError::Pdf("the document is too large to timestamp".into()));
    }
    br_text.resize(width, b' ');
    out[br_at..br_at + width].copy_from_slice(&br_text);
    let q = crate::timestamp::TimestampQuery::new(DigestAlg::Sha256, DigestAlg::Sha256.digest(&[&out[..start], &out[end..]]))?;
    let resp = tsa.timestamp(&q.encode()?)?;
    let token = crate::timestamp::parse_response(&resp, &q)?;
    let hex: Vec<u8> = token.raw.iter().flat_map(|b| format!("{b:02X}").into_bytes()).collect();
    if hex.len() > TOKEN_RESERVE * 2 {
        return Err(SignError::Pdf("the timestamp token does not fit its placeholder".into()));
    }
    out[start + 1..start + 1 + hex.len()].copy_from_slice(&hex);
    Ok(out)
}

/// Find the placeholders in the written file: the offset of the ByteRange numbers, and the
/// `<…>` hex string's span. The ByteRange marker is unique; the Contents placeholder is the one
/// in the same object (a full save may renumber objects, so they are not found by number).
fn locate(out: &[u8], reserve: usize) -> Result<(usize, (usize, usize)), SignError> {
    let mark = format!("0 {} {} {}", BR_MARK[0], BR_MARK[1], BR_MARK[2]);
    let hits: Vec<usize> = out.windows(mark.len()).enumerate().filter(|(_, w)| *w == mark.as_bytes()).map(|(i, _)| i).collect();
    let [br] = hits.as_slice() else { return Err(SignError::Pdf("ByteRange placeholder not found".into())) };
    let start = out[..*br].windows(3).rposition(|w| w == b"obj").ok_or_else(|| SignError::Pdf("signature object not found".into()))?;
    let end = br + out[*br..].windows(6).position(|w| w == b"endobj").ok_or_else(|| SignError::Pdf("unterminated signature object".into()))?;
    let region = &out[start..end];
    let mut zeros = vec![b'<'];
    zeros.extend(std::iter::repeat_n(b'0', reserve * 2));
    zeros.push(b'>');
    let c = region.windows(zeros.len()).position(|w| w == zeros.as_slice()).ok_or_else(|| SignError::Pdf("Contents placeholder not found".into()))?;
    Ok((*br, (start + c, start + c + zeros.len())))
}

/// "2026.10.02 14:03:11 +02'00'" from a PDF date.
pub fn display_date(pdf: &str) -> String {
    let s = pdf.strip_prefix("D:").unwrap_or(pdf);
    let g = |r: std::ops::Range<usize>| s.get(r).unwrap_or("00");
    let tz = match s.get(14..).filter(|z| !z.is_empty()) {
        Some(z) if z.starts_with('Z') => " Z".to_string(),
        Some(z) => format!(" {z}"),
        None => String::new(),
    };
    format!("{}.{}.{} {}:{}:{}{tz}", g(0..4), g(4..6), g(6..8), g(8..10), g(10..12), g(12..14))
}

/// The visible signature: the signer's name large on the left, the details on the right
/// (Acrobat's standard layout), in Helvetica.
fn appearance(rect: [f64; 4], name: &str, cert: &Certificate, opts: &SignOptions) -> Stream {
    use pdfcraft_fonts::{helvetica_width, literal, win_ansi, wrap};
    let (w, h) = ((rect[2] - rect[0]).max(0.0), (rect[3] - rect[1]).max(0.0));
    let a = &opts.appearance;
    let mut lines: Vec<String> = Vec::new();
    let label = |l: &str, v: &str| if a.labels { format!("{l}{v}") } else { v.to_string() };
    if a.name {
        lines.push(if a.labels { "Digitally signed by".into() } else { String::new() });
        lines.push(name.to_string());
    }
    if a.distinguished_name {
        lines.push(label("DN: ", &cert.subject.display()));
    }
    if a.reason
        && let Some(r) = opts.reason.as_deref().filter(|r| !r.is_empty())
    {
        lines.push(label("Reason: ", r));
    }
    if a.location
        && let Some(l) = opts.location.as_deref().filter(|l| !l.is_empty())
    {
        lines.push(label("Location: ", l));
    }
    if a.date {
        lines.push(label("Date: ", &display_date(&opts.date)));
    }
    lines.retain(|l| !l.is_empty());
    let mut out = Vec::new();
    if w > 1.0 && h > 1.0 {
        let pad = (h * 0.06).clamp(1.0, 6.0);
        let (left_w, right_x) = if a.name { (w * 0.5, w * 0.5 + pad) } else { (0.0, pad) };
        let right_w = (w - right_x - pad).max(1.0);
        out.extend_from_slice(b"BT\n0 g\n");
        if a.name {
            // The name fills the left half.
            let mut size = (h * 0.4).min(36.0);
            let fit = (left_w - 2.0 * pad).max(1.0);
            let longest = helvetica_width(name, 1.0).max(0.01);
            size = size.min(fit / longest).max(4.0);
            let words = wrap(name, size, fit);
            let total = words.len() as f64 * size * 1.1;
            let mut y = (h + total) / 2.0 - size * 0.85;
            for l in &words {
                out.extend(format!("/Helv {} Tf 1 0 0 1 {} {} Tm ", fmt(size), fmt(pad), fmt(y)).bytes());
                out.extend(literal(&win_ansi(l)));
                out.extend_from_slice(b" Tj\n");
                y -= size * 1.1;
            }
        }
        // The details, shrunk to fit the right side.
        let mut size = (h / (lines.len().max(1) as f64 * 1.15)).min(12.0);
        let wrapped = loop {
            let all: Vec<String> = lines.iter().flat_map(|l| wrap(l, size, right_w)).collect();
            if all.len() as f64 * size * 1.15 <= h - 2.0 * pad || size <= 3.0 {
                break all;
            }
            size *= 0.9;
        };
        let mut y = h - pad - size * 0.9;
        for l in &wrapped {
            out.extend(format!("/Helv {} Tf 1 0 0 1 {} {} Tm ", fmt(size), fmt(right_x), fmt(y)).bytes());
            out.extend(literal(&win_ansi(l)));
            out.extend_from_slice(b" Tj\n");
            y -= size * 1.15;
        }
        out.extend_from_slice(b"ET\n");
    }
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), Object::Array([0.0, 0.0, w, h].iter().map(|x| Object::Real(*x)).collect()));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type1"));
    font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
    font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    let mut fonts = Dict::new();
    fonts.set(b"Helv".to_vec(), Object::Dict(font));
    let mut res = Dict::new();
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    d.set(b"Resources".to_vec(), Object::Dict(res));
    Stream::flate(d, &out)
}

fn fmt(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}
