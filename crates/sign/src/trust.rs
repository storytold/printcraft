//! Optional trust sets, kept apart from the certificates the user trusts themselves.
//!
//! Nothing here is trusted unless the user switches it on (`TrustStore::builtin_roots`,
//! `TrustStore::lists`): which authorities a PDF reader vouches for is the user's decision, not
//! ours. Chain building ([`crate::x509::build_chain`]) still applies to them: a root only vouches
//! for what a chain of CA certificates under it leads to.
//!
//! - [`builtin_roots`]: roots of commercial CAs, embedded (`data/builtin-roots.der`, about 23 KB),
//!   each pinned in `data/builtin-roots.toml`.
//! - [`TrustList`]: a list loaded from a file at run time, like the EU Trusted Lists' qualified
//!   CA services (`cargo xtask trust-lists` makes the file). Nothing of the sort is embedded.
//!
//! Adobe's AATL is not used in any form: Adobe-authored data outside the closed list in AGENTS.md §1.1
//! needs the owner's explicit approval, and the EU Trusted Lists cover the same ground.

use std::sync::{Arc, OnceLock};

use crate::SignError;
use crate::der::Tlv;
use crate::x509::Certificate;

static ROOTS: &[u8] = include_bytes!("../data/builtin-roots.der");

/// The embedded roots (`data/builtin-roots.der`, certificates one after the other): the commercial
/// CAs behind most non-qualified PDF signatures. `data/builtin-roots.toml` lists each with its
/// download URL and pinned SHA-256; `cargo xtask trust-roots` rebuilds the file from it. Adding a
/// root is a trust decision for the project owner. Off unless `TrustStore::builtin_roots` is set.
pub fn builtin_roots() -> &'static [Certificate] {
    static PARSED: OnceLock<Vec<Certificate>> = OnceLock::new();
    PARSED.get_or_init(|| parse_concatenated(ROOTS))
}

/// Whether `c` is one of [`builtin_roots`].
pub fn is_builtin_root(c: &Certificate) -> bool {
    builtin_roots().iter().any(|t| t.raw == c.raw)
}

/// Certificates as DER one after the other. Entries that don't parse are skipped.
fn parse_concatenated(mut rest: &[u8]) -> Vec<Certificate> {
    let mut out = Vec::new();
    while !rest.is_empty() {
        let Ok((t, r)) = Tlv::parse(rest) else { break };
        if let Ok(c) = Certificate::parse(t.raw) {
            out.push(c);
        }
        rest = r;
    }
    out
}

/// The most a list file may hold: far above the roughly 1.5 MB of the EU qualified CAs.
pub const MAX_LIST_BYTES: usize = 32 << 20;

/// A named set of trusted CA certificates the user loaded from a file, e.g. "EU Trusted List".
#[derive(Clone, Debug)]
pub struct TrustList {
    pub name: String,
    pub certs: Arc<Vec<Certificate>>,
}

impl TrustList {
    /// A list from a file's bytes: certificates as DER one after the other (what
    /// `cargo xtask trust-lists` writes), or a PEM bundle. Certificates that don't parse are
    /// skipped; a file with none, or one over [`MAX_LIST_BYTES`], is an error.
    pub fn from_bytes(name: &str, bytes: &[u8]) -> Result<TrustList, SignError> {
        if bytes.len() > MAX_LIST_BYTES {
            return Err(SignError::Malformed(format!("trust list over {} MiB", MAX_LIST_BYTES >> 20)));
        }
        let mut certs = if bytes.first() == Some(&0x30) { parse_concatenated(bytes) } else { crate::x509::load_certificates(bytes)? };
        let mut seen = std::collections::HashSet::new();
        certs.retain(|c| seen.insert(c.raw.clone()));
        if certs.is_empty() {
            return Err(SignError::Malformed("no certificates in the trust list".into()));
        }
        Ok(TrustList { name: name.to_string(), certs: Arc::new(certs) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_roots_match_their_manifest() {
        let roots = builtin_roots();
        let manifest = include_str!("../data/builtin-roots.toml");
        // Every [[root]] pin in the manifest is one of the bundled certificates, and the other way round.
        let pins: Vec<&str> = manifest.lines().filter_map(|l| l.strip_prefix("sha256 = \"")).filter_map(|l| l.strip_suffix('"')).collect();
        assert_eq!(pins.len(), roots.len());
        for r in roots {
            let sum: String = crate::DigestAlg::Sha256.digest(&[&r.raw]).iter().map(|b| format!("{b:02x}")).collect();
            assert!(pins.contains(&sum.as_str()), "{} is not pinned", r.display_name());
            assert!(r.is_self_signed(), "{}: a root, its own key verifies it", r.display_name());
            assert!(r.may_issue(), "{}: a CA", r.display_name());
            assert!(is_builtin_root(r));
        }
        let names: Vec<String> = roots.iter().map(|r| r.display_name()).collect();
        for want in [
            "Entrust.net Certification Authority (2048)",
            "DigiCert Global Root G2",
            "DigiCert Trusted Root G4",
            "GlobalSign",
            "USERTrust RSA Certification Authority",
            "OISTE WISeKey Global Root GB CA",
        ] {
            assert!(names.iter().any(|n| n.starts_with(want)), "{want} missing from {names:?}");
        }
        // (GlobalSign's roots share a common name; the full subject tells them apart.)
        let mut unique: Vec<String> = roots.iter().map(|r| r.subject.display()).collect();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), roots.len(), "no root twice");
        // The SHA-256 fingerprint Entrust publishes for the root that started the list.
        let e = roots.iter().find(|r| r.display_name().starts_with("Entrust.net")).unwrap();
        assert_eq!(e.fingerprint(), "6D C4 71 72 E0 1C BC B0 BF 62 58 0D 89 5F E2 B8 AC 9A D4 F8 73 80 1E 0C 10 B9 C8 37 D2 1E B1 77");
        // And the one Mozilla's root set and a Documenso-signed PDF carry for WISeKey.
        let w = roots.iter().find(|r| r.display_name().starts_with("OISTE WISeKey")).unwrap();
        assert_eq!(w.fingerprint(), "6B 9C 08 E8 6E B0 F7 67 CF AD 65 CD 98 B6 21 49 E5 49 4A 67 F5 84 5E 7B D1 ED 01 9F 27 B8 6B D6");
    }

    #[test]
    fn a_list_reads_concatenated_der_and_rejects_garbage() {
        let roots = builtin_roots();
        let mut file = Vec::new();
        for r in &roots[..3] {
            file.extend_from_slice(&r.raw);
        }
        let list = TrustList::from_bytes("Test list", &file).unwrap();
        assert_eq!((list.name.as_str(), list.certs.len()), ("Test list", 3));
        // The same as a PEM bundle.
        let pem: String = roots[..3].iter().map(crate::x509::to_pem).collect();
        assert_eq!(TrustList::from_bytes("pem", pem.as_bytes()).unwrap().certs.len(), 3);
        assert!(TrustList::from_bytes("none", b"not certificates").is_err());
        assert!(TrustList::from_bytes("none", &[]).is_err());
        assert!(TrustList::from_bytes("big", &vec![0x30; MAX_LIST_BYTES + 1]).is_err());
        // A certificate listed twice, even apart, is one entry.
        let repeated = [roots[0].raw.clone(), roots[1].raw.clone(), roots[0].raw.clone()].concat();
        assert_eq!(TrustList::from_bytes("dups", &repeated).unwrap().certs.len(), 2);
    }
}
