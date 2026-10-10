//! Signing identities in the Windows Current User Personal ("My") certificate store.
//! Private keys remain in CNG; Windows may ask permission to use a key.
//!
//! Certificates that can't sign through PdfCraft (no usable private key, a key type it doesn't
//! sign with) are reported in [`Listing::unusable`] with the reason rather than left out
//! silently, so a missing identity can be explained (issue #179).

use std::sync::Arc;

use rustls_cng::cert::CertContext;
use rustls_cng::error::CngError;
use rustls_cng::key::{AlgorithmGroup, NCryptKey, SignaturePadding};
use rustls_cng::store::{CertStore, CertStoreType};

use crate::keys::{DigestAlg, ExternalKey, PrivateKey, PublicKey, StoreKey};
use crate::{Certificate, DigitalId, SignError, x509};

struct WindowsKey {
    context: CertContext,
    public: PublicKey,
}

impl ExternalKey for WindowsKey {
    fn sign(&self, alg: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        let digest = alg.digest(&[msg]);
        // CNG adds the DigestInfo for PKCS #1 v1.5 when given PKCS1 padding.
        let padding = if matches!(self.public, PublicKey::Rsa { .. }) { SignaturePadding::Pkcs1 } else { SignaturePadding::None };
        // A key handle acquired silently can never prompt: smart cards and tokens then refuse
        // to sign (NTE_SILENT_CONTEXT, 0x80090022) instead of asking for the PIN. Acquire the
        // signing handle without the silent flag so the provider can show its own dialog.
        let key = self.context.acquire_key(false).map_err(|e| {
            SignError::Crypto(format!(
                "the Windows certificate store key couldn't be opened for signing (is the smart card or token connected?): {}",
                describe(&e)
            ))
        })?;
        let raw = key.sign(&digest, padding).map_err(|e| {
            SignError::Crypto(format!("the Windows certificate store didn't sign (key use may have been cancelled): {}", describe(&e)))
        })?;
        self.public.store_signature_der(&raw)
    }
}

/// A Personal store certificate PdfCraft can't sign with, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unusable {
    /// The subject's common name (else organization, else the DN), or a placeholder when
    /// the certificate couldn't be read at all.
    pub subject: String,
    /// SHA-256 of the certificate as upper-case hex pairs (`Certificate::fingerprint`).
    pub fingerprint: String,
    /// Why it can't sign, as a clause that follows the name ("its private key is …").
    pub reason: String,
    /// The store holds no private key for it: a certificate someone else issued, filed under
    /// Personal. Not a problem with an identity, so the Sign dialog doesn't show these.
    pub no_private_key: bool,
}

/// The Personal store's signing identities and the certificates that can't sign.
#[derive(Default)]
pub struct Listing {
    pub ids: Vec<DigitalId>,
    pub unusable: Vec<Unusable>,
}

/// The `windows:<SHA-256 of the certificate DER>` reference kept for an identity.
pub fn reference(certificate: &Certificate) -> String {
    let digest = DigestAlg::Sha256.digest(&[&certificate.raw]);
    format!("windows:{}", digest.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// Find an identity by its reference or subject common name. Its `chain` holds the issuers
/// Windows finds for the certificate, so a signature embeds them as it does for a file's.
pub fn find(reference_or_name: &str) -> Result<DigitalId, SignError> {
    let (context, mut id) = entries()?
        .ids
        .into_iter()
        .find(|(_, id)| {
            reference(&id.certificate) == reference_or_name
                || id.certificate.subject.common_name() == Some(reference_or_name.strip_prefix("windows:").unwrap_or(reference_or_name))
        })
        .ok_or_else(|| SignError::Crypto(format!("no Windows certificate store identity {reference_or_name}")))?;
    id.chain = issuers(&context, &id.certificate);
    Ok(id)
}

/// The certificates Windows chains `certificate` to, without the root. Best effort: an
/// identity signs without them too.
fn issuers(context: &CertContext, certificate: &Certificate) -> Vec<Certificate> {
    let Ok(chain) = context.as_chain_der() else { return Vec::new() };
    chain.iter().filter(|der| **der != certificate.raw).filter_map(|der| Certificate::parse(der).ok()).collect()
}

/// List usable RSA, P-256 and P-384 CNG identities in Current User > Personal
/// (the `ids` of [`list`]).
pub fn identities() -> Result<Vec<DigitalId>, SignError> {
    Ok(entries()?.ids.into_iter().map(|(_, id)| id).collect())
}

/// List Current User > Personal: the RSA, P-256 and P-384 identities whose private key CNG can
/// open, and every other certificate with the reason it can't sign. A key that needs a prompt
/// to open counts as usable; the prompt appears at signing.
pub fn list() -> Result<Listing, SignError> {
    let Entries { ids, unusable } = entries()?;
    Ok(Listing { ids: ids.into_iter().map(|(_, id)| id).collect(), unusable })
}

/// Each usable identity with the store's certificate context it came from, and the rest.
struct Entries {
    ids: Vec<(CertContext, DigitalId)>,
    unusable: Vec<Unusable>,
}

fn entries() -> Result<Entries, SignError> {
    let store = CertStore::open(CertStoreType::CurrentUser, "My")
        .map_err(|e| SignError::Crypto(format!("the Windows Current User Personal store couldn't be opened: {}", describe(&e))))?;
    let contexts =
        store.find_all().map_err(|e| SignError::Crypto(format!("the Windows certificate store couldn't be searched: {}", describe(&e))))?;
    let mut ids = Vec::new();
    let mut unusable = Vec::new();
    for context in contexts {
        let der = context.as_der();
        let fingerprint = DigestAlg::Sha256.digest(&[der]).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ");
        let mut skip = |subject: String, reason: String, no_private_key: bool| {
            unusable.push(Unusable { subject, fingerprint: fingerprint.clone(), reason, no_private_key });
        };
        let certificate = match Certificate::parse(der) {
            Ok(c) => c,
            Err(e) => {
                let subject = x509::subject_of(der).map(|n| n.display_name()).unwrap_or_else(|| "(unreadable certificate)".to_string());
                skip(subject, format!("the certificate can't be read: {e}"), false);
                continue;
            }
        };
        // P-521, brainpool and Ed25519 certificates parse (to verify with) but aren't signed with.
        let Some(want) = certificate.public_key.store_signing_key() else {
            let what = certificate.public_key.describe();
            skip(certificate.display_name(), format!("its {what} key can't sign through PdfCraft yet (RSA, P-256 and P-384 only)"), false);
            continue;
        };
        // Enumeration must not open permission or PIN dialogs. Signing may prompt later.
        match context.acquire_key(true) {
            Ok(key) => {
                if let Err(reason) = matches_key(&key, want) {
                    skip(certificate.display_name(), reason, false);
                    continue;
                }
                // The silent handle was only for the checks above; `sign` acquires its own.
                drop(key);
            }
            // The key opens only with a prompt (strong private key protection, a PIN): usable.
            Err(CngError::WindowsError(NTE_SILENT_CONTEXT | NTE_UI_REQUIRED)) => {}
            Err(e) => {
                let no_private_key = e == CngError::WindowsError(CRYPT_E_NO_KEY_PROPERTY);
                let reason = if no_private_key {
                    "it has no private key in the store".to_string()
                } else {
                    format!("its private key can't be opened: {}", describe(&e))
                };
                skip(certificate.display_name(), reason, no_private_key);
                continue;
            }
        }
        let public = certificate.public_key.clone();
        let key = PrivateKey::external(public.clone(), Arc::new(WindowsKey { context: context.clone(), public }));
        let friendly_name = Some(certificate.display_name());
        ids.push((context, DigitalId { key, certificate, chain: Vec::new(), friendly_name }));
    }
    Ok(Entries { ids, unusable })
}

/// Whether the store's key is what the certificate needs (RSA; ECDSA on its curve).
fn matches_key(key: &NCryptKey, want: StoreKey) -> Result<(), String> {
    let group = key.algorithm_group().map_err(|e| format!("its private key's algorithm couldn't be read: {}", describe(&e)))?;
    let name = |g: &AlgorithmGroup| match g {
        AlgorithmGroup::Rsa => "an RSA".to_string(),
        AlgorithmGroup::Ecdsa => "an ECDSA".to_string(),
        other => format!("a {other:?}"),
    };
    let expected = match want {
        StoreKey::Rsa => AlgorithmGroup::Rsa,
        StoreKey::Ecdsa(_) => AlgorithmGroup::Ecdsa,
    };
    if group != expected {
        return Err(format!("its certificate holds {} key but the store's private key is {} key", name(&expected), name(&group)));
    }
    if let StoreKey::Ecdsa(bits) = want {
        let actual = key.bits().map_err(|e| format!("its private key's size couldn't be read: {}", describe(&e)))?;
        if actual != bits {
            return Err(format!("its certificate holds a P-{bits} key but the store's private key has {actual} bits"));
        }
    }
    Ok(())
}

// Error codes (winerror.h) that explain why a key can't be opened.
const NTE_NO_KEY: u32 = 0x8009_000D;
const NTE_PERM: u32 = 0x8009_0010;
const NTE_NOT_FOUND: u32 = 0x8009_0011;
const NTE_BAD_PROVIDER: u32 = 0x8009_0013;
const NTE_BAD_PROV_TYPE: u32 = 0x8009_0014;
const NTE_BAD_KEYSET: u32 = 0x8009_0016;
const NTE_PROV_DLL_NOT_FOUND: u32 = 0x8009_001E;
const NTE_SILENT_CONTEXT: u32 = 0x8009_0022;
const NTE_NOT_SUPPORTED: u32 = 0x8009_0029;
const NTE_UI_REQUIRED: u32 = 0x8009_002E;
const NTE_DEVICE_NOT_READY: u32 = 0x8009_0030;
const NTE_DEVICE_NOT_FOUND: u32 = 0x8009_0035;
const NTE_USER_CANCELLED: u32 = 0x8009_0036;
const CRYPT_E_NO_KEY_PROPERTY: u32 = 0x8009_200B;

/// A CNG error with the Windows code and, for the usual ones, what it means.
fn describe(e: &CngError) -> String {
    let CngError::WindowsError(code) = e else { return e.to_string() };
    let meaning = match *code {
        NTE_NO_KEY | NTE_BAD_KEYSET | NTE_NOT_FOUND => "the key is missing from its key store (deleted, or imported under another Windows account)",
        NTE_PERM => "access to the key was denied",
        NTE_BAD_PROVIDER | NTE_BAD_PROV_TYPE | NTE_PROV_DLL_NOT_FOUND => {
            "its key provider isn't available to CNG (a legacy CryptoAPI-only provider, or its driver isn't installed)"
        }
        NTE_NOT_SUPPORTED => "its key provider doesn't support the operation",
        NTE_DEVICE_NOT_READY | NTE_DEVICE_NOT_FOUND => "the smart card or token isn't available",
        NTE_USER_CANCELLED => "the request was cancelled",
        NTE_SILENT_CONTEXT | NTE_UI_REQUIRED => "the key needs a prompt to open",
        CRYPT_E_NO_KEY_PROPERTY => "the certificate has no private key",
        0x8010_0000..=0x8010_FFFF => "the smart card isn't available",
        _ => return format!("Windows error 0x{code:08X}"),
    };
    format!("{meaning} (Windows error 0x{code:08X})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_are_explained_with_their_code() {
        assert_eq!(
            describe(&CngError::WindowsError(NTE_BAD_KEYSET)),
            "the key is missing from its key store (deleted, or imported under another Windows account) (Windows error 0x80090016)"
        );
        assert_eq!(describe(&CngError::WindowsError(0x8010_000C)), "the smart card isn't available (Windows error 0x8010000C)");
        assert_eq!(describe(&CngError::WindowsError(0x57)), "Windows error 0x00000057");
        assert_eq!(describe(&CngError::InvalidHashLength), "Invalid hash length");
    }
}
