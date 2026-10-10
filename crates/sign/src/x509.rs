//! X.509 certificates (RFC 5280): the fields signing and validation need, signature checks
//! along a chain, and a self-signed certificate builder for new digital IDs.

use crate::SignError;
use crate::der::{self, Time, Tlv, tag};
use crate::keys::{self, DigestAlg, PrivateKey, PublicKey};

const CN: &str = "2.5.4.3";
const C: &str = "2.5.4.6";
const O: &str = "2.5.4.10";
const OU: &str = "2.5.4.11";
const EMAIL: &str = "1.2.840.113549.1.9.1";

/// A distinguished name: (attribute OID, value) in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Name {
    pub attrs: Vec<(String, String)>,
    /// The DER encoding, compared byte for byte when matching issuers to subjects.
    pub raw: Vec<u8>,
}

impl Name {
    fn parse(t: &Tlv<'_>) -> Result<Name, SignError> {
        let mut attrs = Vec::new();
        for rdn in t.expect(tag::SEQUENCE, "Name")?.children()? {
            for atv in rdn.children()? {
                let parts = atv.children()?;
                if let [o, v] = parts.as_slice() {
                    attrs.push((o.oid()?, v.text().unwrap_or_default()));
                }
            }
        }
        Ok(Name { attrs, raw: t.raw.to_vec() })
    }

    fn get(&self, oid: &str) -> Option<&str> {
        self.attrs.iter().find(|(o, _)| o == oid).map(|(_, v)| v.as_str())
    }

    pub fn common_name(&self) -> Option<&str> {
        self.get(CN)
    }

    pub fn organization(&self) -> Option<&str> {
        self.get(O)
    }

    pub fn unit(&self) -> Option<&str> {
        self.get(OU)
    }

    pub fn email(&self) -> Option<&str> {
        self.get(EMAIL)
    }

    pub fn country(&self) -> Option<&str> {
        self.get(C)
    }

    /// The name Acrobat shows: the common name, else the organization, else the DN.
    pub fn display_name(&self) -> String {
        self.common_name().or(self.organization()).map(str::to_string).unwrap_or_else(|| self.display())
    }

    /// "CN=Ada Lovelace, O=Example, E=ada@example.com"
    pub fn display(&self) -> String {
        self.attrs
            .iter()
            .map(|(o, v)| {
                let k = match o.as_str() {
                    CN => "CN",
                    C => "C",
                    O => "O",
                    OU => "OU",
                    EMAIL => "E",
                    "2.5.4.7" => "L",
                    "2.5.4.8" => "ST",
                    other => other,
                };
                format!("{k}={v}")
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// A name from its parts (empty ones left out), in the usual order.
    pub fn build(cn: &str, ou: &str, o: &str, email: &str, country: &str) -> Name {
        let mut rdns: Vec<Vec<u8>> = Vec::new();
        let mut attrs = Vec::new();
        let mut add = |oid: &str, enc: Vec<u8>, v: &str| {
            if !v.trim().is_empty() {
                rdns.push(der::set_of(&[&der::seq(&[&der::oid(oid), &enc])]));
                attrs.push((oid.to_string(), v.trim().to_string()));
            }
        };
        let country = country.trim().to_ascii_uppercase();
        if country.len() == 2 {
            add(C, der::printable(&country), &country);
        }
        add(O, der::utf8(o.trim()), o);
        add(OU, der::utf8(ou.trim()), ou);
        add(CN, der::utf8(cn.trim()), cn);
        add(EMAIL, der::ia5(email.trim()), email);
        let raw = der::tlv(tag::SEQUENCE, &rdns.concat());
        Name { attrs, raw }
    }
}

/// A parsed certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Certificate {
    /// The whole DER encoding.
    pub raw: Vec<u8>,
    tbs: Vec<u8>,
    sig_alg: Vec<u8>,
    signature: Vec<u8>,
    pub serial: Vec<u8>,
    pub issuer: Name,
    pub subject: Name,
    pub not_before: Time,
    pub not_after: Time,
    pub public_key: PublicKey,
    /// Basic constraints: a CA certificate.
    pub is_ca: bool,
    /// Whether the certificate has a basicConstraints extension at all (old v1 roots don't).
    pub has_basic_constraints: bool,
    /// Basic constraints `pathLenConstraint`: how many CA certificates may follow this one
    /// before the end-entity certificate.
    pub path_len: Option<u32>,
    /// Key usage bits (bit 0 = digitalSignature, 1 = nonRepudiation, 5 = keyCertSign), if present.
    pub key_usage: Option<u16>,
    pub subject_key_id: Option<Vec<u8>>,
    /// Authority key identifier (2.5.29.35), when present.
    pub authority_key_id: Option<Vec<u8>>,
    /// Extended key usage OIDs (2.5.29.37), when present.
    pub extended_key_usage: Option<Vec<String>>,
    /// OCSP responder URLs from the Authority Information Access (1.3.6.1.5.5.7.1.1).
    pub ocsp_urls: Vec<String>,
    /// CRL distribution point URLs (2.5.29.31).
    pub crl_urls: Vec<String>,
    /// The `version` field: 0 for v1, 1 for v2, 2 for v3.
    pub version: u64,
    /// OIDs of extensions marked critical that PdfCraft does not process (RFC 5280 §4.2).
    pub unknown_critical: Vec<String>,
}

/// Extensions PdfCraft reads or may safely ignore when they are critical: the ones
/// `Extensions::read` handles, subject/issuer alternative names, and certificate policies
/// (accepted as any policy).
/// qcStatements (1.3.6.1.5.5.7.1.3) is included: RFC 3739 lets qualified (eIDAS and other national)
/// certificates mark it critical, and it declares the certificate's status rather than limiting
/// what the key may sign.
const PROCESSED_EXTENSIONS: [&str; 11] = [
    "2.5.29.19",
    "2.5.29.15",
    "2.5.29.14",
    "2.5.29.35",
    "2.5.29.37",
    "1.3.6.1.5.5.7.1.1",
    "2.5.29.31",
    "2.5.29.17",
    "2.5.29.18",
    "2.5.29.32",
    "1.3.6.1.5.5.7.1.3",
];

/// Extended key usages that allow signing documents: anyExtendedKeyUsage, emailProtection,
/// codeSigning, documentSigning (RFC 9336), Adobe Authentic Documents and Microsoft document
/// signing.
const DOCUMENT_SIGNING_EKUS: [&str; 6] =
    ["2.5.29.37.0", "1.3.6.1.5.5.7.3.4", "1.3.6.1.5.5.7.3.3", "1.3.6.1.5.5.7.3.36", "1.2.840.113583.1.1.5", "1.3.6.1.4.1.311.10.3.12"];

/// The certificate extensions `Certificate::parse` reads, gathered tolerantly: an
/// extension that does not parse leaves its field at the default instead of failing the
/// certificate (real-world issuers use encodings with edge cases, e.g. a single-URI CRL
/// distribution point whose `fullName [0]` is IMPLICIT over a bare GeneralName).
#[derive(Default)]
struct Extensions {
    is_ca: bool,
    has_basic_constraints: bool,
    path_len: Option<u32>,
    key_usage: Option<u16>,
    subject_key_id: Option<Vec<u8>>,
    authority_key_id: Option<Vec<u8>>,
    extended_key_usage: Option<Vec<String>>,
    ocsp_urls: Vec<String>,
    crl_urls: Vec<String>,
}

impl Extensions {
    fn read(&mut self, oid: &str, value: &Tlv<'_>) {
        match oid {
            "2.5.29.19" => {
                // Present at all, even when it doesn't parse: a malformed basicConstraints must
                // not make a certificate look like an old v1 root (which may issue). Unparsed,
                // it stays "not a CA".
                self.has_basic_constraints = true;
                let Ok(bc) = Tlv::parse_all(value.value) else { return };
                let fields = bc.children().unwrap_or_default();
                self.is_ca = fields.first().is_some_and(|b| b.tag == tag::BOOLEAN && b.value != [0]);
                // pathLenConstraint is the INTEGER after the optional cA BOOLEAN. One that is
                // present but unreadable (negative, too long) allows no CA below, not any number.
                self.path_len = fields.iter().find(|t| t.tag == tag::INTEGER).map(|t| t.u64().map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX)));
            }
            "2.5.29.15" => {
                // A keyUsage that is present restricts the key even when it doesn't parse: an
                // unreadable one allows no usage at all, never every usage.
                self.key_usage = Some(0);
                let Ok(bits) = Tlv::parse_all(value.value) else { return };
                let Some((_, b)) = bits.value.split_first() else { return };
                // Bit 0 is the most significant bit of the first byte.
                let mut u = 0u16;
                for (i, byte) in b.iter().take(2).enumerate() {
                    for j in 0..8 {
                        if byte & (0x80 >> j) != 0 {
                            u |= 1 << (i * 8 + j);
                        }
                    }
                }
                self.key_usage = Some(u);
            }
            "2.5.29.14" => {
                if let Ok(ski) = Tlv::parse_all(value.value) {
                    self.subject_key_id = Some(ski.value.to_vec());
                }
            }
            // AuthorityKeyIdentifier: the [0] keyIdentifier inside.
            "2.5.29.35" => {
                if let Ok(aki) = Tlv::parse_all(value.value) {
                    self.authority_key_id = aki.children().ok().and_then(|c| c.into_iter().find(|t| t.tag == tag::ctx(0))).map(|t| t.value.to_vec());
                }
            }
            // ExtendedKeyUsage: a SEQUENCE OF OID.
            "2.5.29.37" => {
                // Present means restricted, even when it doesn't parse: an unreadable one allows no
                // purpose at all, never every purpose (as for keyUsage). An entry that isn't an OID
                // grants nothing.
                self.extended_key_usage = Some(Vec::new());
                let Ok(eku) = Tlv::parse_all(value.value) else { return };
                self.extended_key_usage = Some(eku.children().unwrap_or_default().into_iter().filter_map(|t| t.oid().ok()).collect());
            }
            // Authority Information Access: OCSP and CA-issuer locations.
            "1.3.6.1.5.5.7.1.1" => {
                let Ok(aia) = Tlv::parse_all(value.value) else { return };
                for access in aia.children().unwrap_or_default() {
                    let Ok(a) = access.children() else { continue };
                    if a.len() >= 2 && a[0].oid().ok().as_deref() == Some("1.3.6.1.5.5.7.48.1") && a[1].tag == tag::ctx_prim(6) {
                        self.ocsp_urls.push(String::from_utf8_lossy(a[1].value).into_owned());
                    }
                }
            }
            // CRL distribution points: full-name URIs.
            "2.5.29.31" => {
                let Ok(dps) = Tlv::parse_all(value.value) else { return };
                for dp in dps.children().unwrap_or_default() {
                    let Ok(fields) = dp.children() else { continue };
                    for field in fields {
                        if field.tag != tag::ctx(0) {
                            continue;
                        }
                        // distributionPoint [0] DistributionPointName, whose fullName choice is
                        // [0] IMPLICIT GeneralNames: the value is a run of GeneralNames, not a
                        // wrapped SEQUENCE (and often a single URI).
                        let Ok(name) = field.inner() else { continue };
                        if name.tag != tag::ctx(0) {
                            continue;
                        }
                        let mut rest = name.value;
                        while !rest.is_empty() {
                            let Ok((n, r)) = Tlv::parse(rest) else { break };
                            rest = r;
                            if n.tag == tag::ctx_prim(6) {
                                self.crl_urls.push(String::from_utf8_lossy(n.value).into_owned());
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

impl Certificate {
    pub fn parse(raw: &[u8]) -> Result<Certificate, SignError> {
        let cert = Tlv::parse_all(raw)?.expect(tag::SEQUENCE, "Certificate")?;
        let parts = cert.children()?;
        let [tbs, sig_alg, sig] = parts.as_slice() else { return Err(SignError::Malformed("Certificate".into())) };
        let mut f = tbs.children()?.into_iter().peekable();
        // version [0] EXPLICIT INTEGER DEFAULT v1 (0); v3 is 2. Unreadable counts as v3, the
        // stricter reading (no v1-root exception in `may_issue`).
        let mut version = 0u64;
        if f.peek().is_some_and(|t| t.tag == tag::ctx(0)) {
            version = f.next().and_then(|v| v.inner().ok()).and_then(|v| v.u64().ok()).unwrap_or(2);
        }
        let serial = f.next().ok_or_else(|| SignError::Malformed("serial".into()))?.expect(tag::INTEGER, "serial")?.value.to_vec();
        let _inner_alg = f.next();
        let issuer = Name::parse(&f.next().ok_or_else(|| SignError::Malformed("issuer".into()))?)?;
        let validity = f.next().ok_or_else(|| SignError::Malformed("validity".into()))?.children()?;
        let [nb, na] = validity.as_slice() else { return Err(SignError::Malformed("validity".into())) };
        let subject = Name::parse(&f.next().ok_or_else(|| SignError::Malformed("subject".into()))?)?;
        let public_key = PublicKey::from_spki(&f.next().ok_or_else(|| SignError::Malformed("public key".into()))?)?;
        let mut ext = Extensions::default();
        let mut unknown_critical = Vec::new();
        for t in f {
            if t.tag != tag::ctx(3) {
                continue;
            }
            for ext_tlv in t.inner()?.children()? {
                let e = ext_tlv.children()?;
                let Some(o) = e.first().and_then(|o| o.oid().ok()) else { continue };
                let Some(value) = e.last().filter(|v| v.tag == tag::OCTET_STRING) else { continue };
                let critical = e.len() == 3 && e.get(1).is_some_and(|b| b.tag == tag::BOOLEAN && b.value != [0]);
                if critical && !PROCESSED_EXTENSIONS.contains(&o.as_str()) {
                    unknown_critical.push(o.clone());
                }
                // A malformed or unreadable extension must never reject the whole certificate:
                // unreadable ones are skipped and the fields they carry keep their defaults.
                ext.read(o.as_str(), value);
            }
        }
        let Extensions {
            is_ca,
            has_basic_constraints,
            path_len,
            key_usage,
            subject_key_id,
            authority_key_id,
            extended_key_usage,
            ocsp_urls,
            crl_urls,
        } = ext;
        Ok(Certificate {
            raw: raw.to_vec(),
            tbs: tbs.raw.to_vec(),
            sig_alg: sig_alg.raw.to_vec(),
            signature: sig.bits()?.to_vec(),
            serial,
            issuer,
            subject,
            not_before: nb.time()?,
            not_after: na.time()?,
            public_key,
            is_ca,
            has_basic_constraints,
            path_len,
            key_usage,
            subject_key_id,
            authority_key_id,
            extended_key_usage,
            ocsp_urls,
            crl_urls,
            version,
            unknown_critical,
        })
    }

    /// Why this certificate may not sign documents, if it may not (RFC 5280 §4.2.1.3,
    /// §4.2.1.12, §4.2): a key usage without digitalSignature or nonRepudiation, an extended key
    /// usage without a document-signing purpose, or a critical extension PdfCraft doesn't process.
    pub fn signing_problem(&self) -> Option<String> {
        /// keyUsage bits 0 (digitalSignature) and 1 (nonRepudiation).
        const SIGNING_BITS: u16 = 0b11;
        if self.key_usage.is_some_and(|u| u & SIGNING_BITS == 0) {
            Some(
                "The signer's certificate does not allow digital signatures (its key usage has neither digital signature nor non-repudiation)."
                    .into(),
            )
        } else if self.extended_key_usage.as_ref().is_some_and(|e| !e.iter().any(|o| DOCUMENT_SIGNING_EKUS.contains(&o.as_str()))) {
            Some("The signer's certificate is not issued for signing documents (its extended key usage has no document-signing purpose).".into())
        } else if !self.unknown_critical.is_empty() {
            Some(format!("The signer's certificate has critical extensions PdfCraft does not recognize ({}).", self.unknown_critical.join(", ")))
        } else {
            None
        }
    }

    /// Issued by itself (subject = issuer and its own key verifies it).
    pub fn is_self_signed(&self) -> bool {
        self.issuer.raw == self.subject.raw && self.signed_by(&self.public_key)
    }

    /// Whether `key` verifies this certificate's signature.
    pub fn signed_by(&self, key: &PublicKey) -> bool {
        let Ok(alg) = Tlv::parse_all(&self.sig_alg) else { return false };
        match keys::signature_algorithm(&alg) {
            Ok((scheme @ keys::Scheme::Ed25519, _)) => key.verify_message(scheme, &self.tbs, &self.signature).unwrap_or(false),
            Ok((scheme, Some(digest))) => key.verify(scheme, digest, &digest.digest(&[&self.tbs]), &self.signature).unwrap_or(false),
            _ => false,
        }
    }

    /// Whether this certificate may issue certificates (RFC 5280 §4.2.1.9, §4.2.1.3): it is a
    /// CA by its basic constraints, and if it has a key usage, that includes `keyCertSign`. A
    /// self-signed v1 or v2 certificate (an old root, which predates extensions) with no basic
    /// constraints counts as a CA; a v3 certificate must say it is one.
    pub fn may_issue(&self) -> bool {
        /// keyUsage bit 5.
        const KEY_CERT_SIGN: u16 = 1 << 5;
        let old_root = self.version < 2 && !self.has_basic_constraints && self.is_self_signed();
        (self.is_ca || old_root) && self.key_usage.is_none_or(|u| u & KEY_CERT_SIGN != 0)
    }

    /// Valid at `t`.
    pub fn valid_at(&self, t: Time) -> bool {
        self.not_before <= t && t <= self.not_after
    }

    /// The display name Acrobat uses: the common name, else the organization, else the DN.
    pub fn display_name(&self) -> String {
        self.subject.display_name()
    }

    /// SHA-256 fingerprint as upper-case hex pairs.
    pub fn fingerprint(&self) -> String {
        DigestAlg::Sha256.digest(&[&self.raw]).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
    }

    pub fn serial_hex(&self) -> String {
        self.serial.iter().map(|b| format!("{b:02X}")).collect()
    }

    /// A self-signed certificate for `key`: what Acrobat's "Create a new digital ID ▸ save to
    /// file" makes. Valid from `from` for `years`; usable for signing (digital signature and
    /// non-repudiation).
    pub fn self_signed(name: &Name, key: &PrivateKey, from: Time, years: u32, serial: &[u8]) -> Result<Certificate, SignError> {
        let alg = key.preferred_digest();
        let sig_alg = key.signature_algorithm(alg);
        let until = Time { year: from.year + years, ..from };
        let spki = key.public_key().spki();
        let ski = DigestAlg::Sha1.digest(&[&spki]);
        let ext = |o: &str, critical: bool, value: &[u8]| {
            if critical {
                der::seq(&[&der::oid(o), &der::boolean(true), &der::octets(value)])
            } else {
                der::seq(&[&der::oid(o), &der::octets(value)])
            }
        };
        // keyUsage: digitalSignature (bit 0) and nonRepudiation (bit 1) → 0b1100_0000, 6 unused bits.
        let key_usage = der::tlv(tag::BIT_STRING, &[6, 0b1100_0000]);
        let extensions = der::seq(&[
            &ext("2.5.29.15", true, &key_usage),
            &ext("2.5.29.14", false, &der::octets(&ski)),
            // extKeyUsage: emailProtection and Adobe's document signing (1.2.840.113583.1.1.5).
            &ext("2.5.29.37", false, &der::seq(&[&der::oid("1.3.6.1.5.5.7.3.4"), &der::oid("1.2.840.113583.1.1.5")])),
        ]);
        let tbs = der::seq(&[
            &der::explicit(0, &der::int(2)),
            &der::uint(serial),
            &sig_alg,
            &name.raw,
            &der::seq(&[&from.encode(), &until.encode()]),
            &name.raw,
            &spki,
            &der::explicit(3, &extensions),
        ]);
        let signature = key.sign(alg, &tbs)?;
        let cert = der::seq(&[&tbs, &sig_alg, &der::bit_string(&signature)]);
        Certificate::parse(&cert)
    }
}

/// The subject alone, to name a certificate `Certificate::parse` rejects (one with a key type
/// PdfCraft doesn't support, say). `None` when even that can't be read.
pub fn subject_of(raw: &[u8]) -> Option<Name> {
    let cert = Tlv::parse_all(raw).ok()?.expect(tag::SEQUENCE, "Certificate").ok()?;
    let tbs = cert.children().ok()?.into_iter().next()?;
    let mut f = tbs.children().ok()?.into_iter().peekable();
    if f.peek().is_some_and(|t| t.tag == tag::ctx(0)) {
        f.next();
    }
    // serial, signature algorithm, issuer, validity, then the subject.
    Name::parse(&f.nth(4)?).ok()
}

/// The chain from `leaf` up through `pool`, as far as issuers can be found and may issue: each
/// issuer's key verifies the certificate below it, it is a CA allowed to sign certificates, its
/// `pathLenConstraint` allows the CA certificates below it, and (when `at` is given) it was valid
/// then. Stops at a self-signed certificate. Certificates embedded in a document are in `pool`
/// too, so an ordinary end-entity certificate must never be accepted as an issuer.
pub fn build_chain<'a>(leaf: &'a Certificate, pool: impl IntoIterator<Item = &'a Certificate> + Clone, at: Option<Time>) -> Vec<&'a Certificate> {
    build_chain_noted(leaf, pool, at).0
}

/// [`build_chain`], and why it stopped where it did if a certificate that matched the issuer by
/// name and signature was refused.
pub fn build_chain_noted<'a>(
    leaf: &'a Certificate,
    pool: impl IntoIterator<Item = &'a Certificate> + Clone,
    at: Option<Time>,
) -> (Vec<&'a Certificate>, Option<String>) {
    let mut chain = vec![leaf];
    let mut refused = None;
    while chain.len() < 10 {
        let Some(&last) = chain.last() else { break };
        if last.issuer.raw == last.subject.raw {
            break;
        }
        // CA certificates between `last` and the leaf: what a candidate's pathLenConstraint limits.
        let below = chain.len() - 1;
        let mut found = None;
        for c in pool.clone().into_iter().filter(|c| c.subject.raw == last.issuer.raw && !chain.contains(c) && last.signed_by(&c.public_key)) {
            let why = if !c.may_issue() {
                Some("is not a CA certificate that may issue certificates")
            } else if c.path_len.is_some_and(|n| below > n as usize) {
                Some("does not allow this many CA certificates below it (path length)")
            } else if at.is_some_and(|t| !c.valid_at(t)) {
                Some("was not valid at the time of signing")
            } else {
                None
            };
            match why {
                None => {
                    found = Some(c);
                    break;
                }
                Some(why) => {
                    refused = Some(format!("The certificate of {} {why}, so it is not used to vouch for {}.", c.display_name(), last.display_name()))
                }
            }
        }
        let Some(issuer) = found else { break };
        refused = None;
        chain.push(issuer);
    }
    (chain, refused)
}

/// Certificates from a file: DER, or PEM with one or more `CERTIFICATE` blocks (`.cer`, `.crt`,
/// `.pem`, Acrobat's `.fdf`-free exports).
pub fn load_certificates(bytes: &[u8]) -> Result<Vec<Certificate>, SignError> {
    if bytes.first() == Some(&0x30) {
        return Ok(vec![Certificate::parse(bytes)?]);
    }
    let text = String::from_utf8_lossy(bytes);
    let mut out = Vec::new();
    let mut block: Option<String> = None;
    for line in text.lines().map(str::trim) {
        if line == "-----BEGIN CERTIFICATE-----" {
            block = Some(String::new());
        } else if line == "-----END CERTIFICATE-----" {
            if let Some(b) = block.take() {
                out.push(Certificate::parse(&base64(&b).ok_or_else(|| SignError::Malformed("PEM base64".into()))?)?);
            }
        } else if let Some(b) = block.as_mut() {
            b.push_str(line);
        }
    }
    if out.is_empty() {
        return Err(SignError::Malformed("no certificate found (expected DER or PEM)".into()));
    }
    Ok(out)
}

/// The certificate as PEM text (Export certificate).
pub fn to_pem(cert: &Certificate) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in cert.raw.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            s.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    let body: Vec<String> = s.as_bytes().chunks(64).map(|l| String::from_utf8_lossy(l).into_owned()).collect();
    format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n", body.join("\n"))
}

fn base64(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let digits: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=').collect();
    let mut out = Vec::with_capacity(digits.len() * 3 / 4);
    for chunk in digits.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= val(*c)? << (18 - 6 * i);
        }
        for i in 0..chunk.len().saturating_sub(1) {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod xfa_ext_tests {
    use super::*;

    #[test]
    fn a_single_uri_crl_distribution_point_is_read() {
        // The real extnValue of the Entrust Class 3 Client CA: DistributionPoints with one
        // full-name URI. `fullName [0]` is IMPLICIT over GeneralNames, so with a single
        // GeneralName there is no inner SEQUENCE — a run of one element.
        let uri = b"http://crl.entrust.net/2048ca.crl";
        let value = der::seq(&[&der::seq(&[&der::tlv(tag::ctx(0), &der::tlv(tag::ctx(0), &der::tlv(tag::ctx_prim(6), uri)))])]);
        let wrapped = der::tlv(tag::OCTET_STRING, &value);
        let extn = Tlv::parse_all(&wrapped).unwrap();
        let mut ext = Extensions::default();
        ext.read("2.5.29.31", &extn);
        assert_eq!(ext.crl_urls, vec!["http://crl.entrust.net/2048ca.crl".to_string()]);
        // Two URIs in one full-name run.
        let names = [der::tlv(tag::ctx_prim(6), uri), der::tlv(tag::ctx_prim(6), b"http://crl2.example/x.crl")].concat();
        let two = der::seq(&[&der::seq(&[&der::tlv(tag::ctx(0), &der::tlv(tag::ctx(0), &names))])]);
        let wrapped = der::tlv(tag::OCTET_STRING, &two);
        let extn = Tlv::parse_all(&wrapped).unwrap();
        let mut ext = Extensions::default();
        ext.read("2.5.29.31", &extn);
        assert_eq!(ext.crl_urls.len(), 2);
    }

    #[test]
    fn a_malformed_extension_is_skipped_not_fatal() {
        for garbage in [b"".as_slice(), &[0xFF, 0xFF, 0xFF], &[0x30, 0x99], b"not der"] {
            let wrapped = der::tlv(tag::OCTET_STRING, garbage);
            let extn = Tlv::parse_all(&wrapped).unwrap();
            for oid in ["2.5.29.19", "2.5.29.15", "2.5.29.14", "2.5.29.35", "2.5.29.37", "1.3.6.1.5.5.7.1.1", "2.5.29.31"] {
                let mut ext = Extensions::default();
                ext.read(oid, &extn); // must not panic and must leave defaults
                assert!(ext.ocsp_urls.is_empty() && ext.crl_urls.is_empty() && !ext.is_ca);
            }
        }
    }
}
