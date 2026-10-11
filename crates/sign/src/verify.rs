//! Signature verification reports for e-Aadhaar and other signed PDFs.
//!
//! The pdf module already checks the cryptography; this module maps each
//! signature info record to a report that keeps the distinct verdicts apart
//! instead of collapsing them into one tick:
//!
//! - cryptographic integrity (intact / altered / unknown-unsupported),
//! - certificate trust (trusted / untrusted, and whether the anchor is a
//!   bundled CCA root),
//! - certificate validity at signing time (valid / not valid then),
//! - revocation from embedded evidence (good / revoked / unknown-offline),
//! - changes after signing (none / allowed / disallowed).
//!
//! Trust is by chain to the trust store, never by subject name. An e-Aadhaar
//! signer name alone trusts nothing; only the bundled CCA roots (or the user's
//! own trusted certificates) do. Everything runs locally and offline: revocation
//! is only what the document embeds (/DSS), and the report says so when there
//! is nothing to check.

use pdfcraft_cos::Document;

use crate::cca;
use crate::pdf::{Modification, SignatureInfo, Status, TrustStore, list};

/// Whether the signed bytes still match the signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Integrity {
    /// The digest and the signature value verify.
    Intact,
    /// The digest, the signature value, or the byte range does not verify, or
    /// later changes are disallowed.
    Altered { reason: String },
    /// PdfCraft cannot check this signature yet (unsupported algorithm or
    /// structure) or there is nothing signed to check.
    Unknown { reason: String },
    /// An empty signature field: nothing to verify.
    NotApplicable,
}

/// Whether the signer's chain reaches the trust store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    Trusted {
        /// Display name of the anchor certificate.
        anchor: String,
        /// The anchor is one of the bundled CCA roots.
        via_cca: bool,
    },
    Untrusted,
}

/// Whether the signer certificate was valid when the signature was made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CertValidity {
    Valid,
    /// The certificate was expired (or not yet valid) at the signing time.
    NotValidAtSigning {
        detail: String,
    },
    /// No signing time to judge against.
    NoSigningTime,
    /// The signature carries no signer certificate.
    NoCertificate,
}

/// Revocation from evidence embedded in the document (/DSS).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Revocation {
    /// Embedded evidence shows the certificate was not revoked.
    Good,
    Revoked {
        detail: String,
    },
    /// No usable embedded evidence: offline verification cannot tell.
    Unknown,
}

/// Where the reported signing time comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimeSource {
    /// A validated RFC 3161 timestamp from a trusted authority.
    TrustedTimestamp,
    /// The signer's own claimed time (/M or CMS): not independently validated.
    SignerClaim,
    /// No time recorded.
    None,
}

/// One signature field's verification report.
#[derive(Clone, Debug)]
pub struct SignatureReport {
    pub field: String,
    pub signed: bool,
    pub status: Status,
    pub integrity: Integrity,
    pub trust: Trust,
    pub cert_validity: CertValidity,
    pub revocation: Revocation,
    pub revocation_note: Option<String>,
    pub modification: Modification,
    pub time_source: TimeSource,
    pub signing_time: Option<String>,
    pub date_raw: Option<String>,
    pub signer: Option<String>,
    pub subject: Option<String>,
    pub issuer: Option<String>,
    pub cert_valid_from: Option<String>,
    pub cert_valid_to: Option<String>,
    pub serial: Option<String>,
    pub key: Option<String>,
    pub digest: Option<String>,
    pub algorithm: Option<String>,
    pub sub_filter: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub page: Option<usize>,
    pub visible: bool,
    pub revision: usize,
    pub signed_len: usize,
    pub doc_timestamp: bool,
    pub summary: String,
    pub details: Vec<String>,
    pub next_steps: Vec<String>,
}

/// A document's overall verification outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentSummary {
    pub fields: usize,
    pub signed: usize,
    pub valid: usize,
    pub invalid: usize,
    pub untrusted: usize,
    pub unsupported: usize,
    pub altered: usize,
    pub expired_or_invalid_cert: usize,
    pub revocation_unknown: usize,
    pub modified_disallowed: usize,
    /// One of: `no-signatures`, `invalid`, `unsupported`, `expired-or-invalid-cert`,
    /// `untrusted`, `valid`.
    pub overall: String,
    pub overall_text: String,
}

fn contains_any(details: &[String], needles: &[&str]) -> bool {
    details.iter().any(|d| {
        let lower = d.to_lowercase();
        needles.iter().any(|n| lower.contains(n))
    })
}

fn first_detail(details: &[String]) -> String {
    details.first().cloned().unwrap_or_else(|| "No details recorded.".to_string())
}

fn chain_trusts(info: &SignatureInfo, trust: &TrustStore) -> bool {
    info.chain.iter().any(|c| trust.certs.iter().any(|t| t.raw == c.raw))
}

fn chain_anchor(info: &SignatureInfo, trust: &TrustStore) -> Option<(String, bool)> {
    for c in &info.chain {
        if trust.certs.iter().any(|t| t.raw == c.raw) {
            let via_cca = cca::is_cca_root(c);
            return Some((c.display_name(), via_cca));
        }
    }
    None
}

/// Map one validated signature to its report.
pub fn report_for(info: &SignatureInfo, trust: &TrustStore) -> SignatureReport {
    let trusted = info.signed && chain_trusts(info, trust);
    let trust = if trusted {
        let (anchor, via_cca) = chain_anchor(info, trust).unwrap_or_else(|| ("a trusted certificate".to_string(), false));
        Trust::Trusted { anchor, via_cca }
    } else {
        Trust::Untrusted
    };

    let at = info.timestamp_time.or(info.signing_time);
    let cert_validity = match (&info.certificate, at) {
        (None, _) => CertValidity::NoCertificate,
        (Some(_), None) => CertValidity::NoSigningTime,
        (Some(cert), Some(t)) if cert.valid_at(t) => CertValidity::Valid,
        (Some(cert), Some(t)) => CertValidity::NotValidAtSigning {
            detail: format!(
                "The signer's certificate was valid from {} to {}, but the signing time used for checks is {t}.",
                cert.not_before, cert.not_after
            ),
        },
    };

    let unsupported = contains_any(&info.details, &["can't check", "cannot check", "not supported", "unsupported"]);
    let altered_words = contains_any(
        &info.details,
        &["altered or corrupted", "signature value does not match", "does not permit", "does not match the file", "does not exclude exactly"],
    );
    let integrity = if !info.signed {
        Integrity::NotApplicable
    } else if matches!(info.modification, Modification::Disallowed(_)) || (info.status == Status::Invalid && altered_words) {
        Integrity::Altered { reason: first_detail(&info.details) }
    } else {
        match info.status {
            Status::Valid => Integrity::Intact,
            Status::Invalid => Integrity::Altered { reason: first_detail(&info.details) },
            Status::Unknown if unsupported => Integrity::Unknown { reason: first_detail(&info.details) },
            Status::Unknown => Integrity::Intact,
        }
    };

    let (revocation, revocation_note) = if !info.signed {
        (Revocation::Unknown, None)
    } else if contains_any(&info.details, &["has been revoked"]) {
        (Revocation::Revoked { detail: first_detail(&info.details) }, None)
    } else if contains_any(&info.details, &["has not been revoked"]) {
        (Revocation::Good, None)
    } else {
        (
            Revocation::Unknown,
            Some(
                "Revocation status unknown: offline verification only checks revocation evidence embedded in the document (/DSS); this signature carries none."
                    .to_string(),
            ),
        )
    };

    let time_source = if info.timestamp_time.is_some() {
        TimeSource::TrustedTimestamp
    } else if info.signing_time.is_some() {
        TimeSource::SignerClaim
    } else {
        TimeSource::None
    };

    let mut next_steps = Vec::new();
    if !info.signed {
        next_steps.push("This is an empty signature field. Signing it certifies the document.".to_string());
    }
    match &integrity {
        Integrity::Altered { .. } => {
            next_steps.push(
                "Do not rely on this document: its signed content changed or the signature does not verify. Obtain a fresh copy from the issuer."
                    .to_string(),
            );
        }
        Integrity::Unknown { .. } => {
            next_steps.push(
                "PdfCraft cannot check this signature's algorithm or structure yet. Verify it with the issuer's recommended validator.".to_string(),
            );
        }
        Integrity::Intact | Integrity::NotApplicable => {}
    }
    if info.signed {
        if matches!(trust, Trust::Untrusted) {
            next_steps.push("The signature is intact but the signer is not trusted. For e-Aadhaar, verify with the bundled CCA India roots; otherwise add the signer's root to trusted certificates and validate again.".to_string());
        }
        if matches!(cert_validity, CertValidity::NotValidAtSigning { .. }) {
            next_steps.push("The signer certificate was not valid at the signing time. Ask the issuer for a freshly signed document if you need a currently valid chain.".to_string());
        }
        if matches!(revocation, Revocation::Unknown) {
            next_steps.push(
                "Revocation could not be checked offline. For high assurance, check the issuer's OCSP/CRL distribution points online.".to_string(),
            );
        }
        if matches!(info.modification, Modification::Allowed(_)) {
            next_steps.push("Later changes are permitted (form fill, comments, further signatures). Use View signed version to inspect what was originally signed.".to_string());
        }
    }

    let summary = if !info.signed {
        "Unsigned signature field.".to_string()
    } else {
        match (&integrity, &trust, &cert_validity) {
            (Integrity::Altered { .. }, _, _) => "Signature is INVALID: the document changed or the signature does not verify.".to_string(),
            (Integrity::Unknown { .. }, _, _) => "Verification is INCOMPLETE: PdfCraft cannot check this signature yet.".to_string(),
            (Integrity::Intact, Trust::Trusted { .. }, CertValidity::Valid) => "Signature is valid and the signer is trusted.".to_string(),
            (Integrity::Intact, Trust::Trusted { .. }, _) => {
                "Signature is intact and the signer is trusted, but the certificate was not valid at the signing time.".to_string()
            }
            (Integrity::Intact, Trust::Untrusted, _) => "Signature is intact but the signer is NOT trusted.".to_string(),
            _ => info.summary().to_string(),
        }
    };

    let cert = info.certificate.as_ref();
    SignatureReport {
        field: info.field.clone(),
        signed: info.signed,
        status: info.status,
        integrity,
        trust,
        cert_validity,
        revocation,
        revocation_note,
        modification: info.modification.clone(),
        time_source,
        signing_time: at.map(|t| t.to_string()),
        date_raw: info.date.clone(),
        signer: info.signer.clone(),
        subject: cert.map(|c| c.subject.display()),
        issuer: cert.map(|c| c.issuer.display()),
        cert_valid_from: cert.map(|c| c.not_before.to_string()),
        cert_valid_to: cert.map(|c| c.not_after.to_string()),
        serial: cert.map(|c| c.serial_hex()),
        key: cert.map(|c| c.public_key.describe()),
        digest: info.digest.map(|d| d.name().to_string()),
        algorithm: info.algorithm.clone(),
        sub_filter: info.sub_filter.clone(),
        reason: info.reason.clone(),
        location: info.location.clone(),
        page: info.page,
        visible: info.visible,
        revision: info.revision,
        signed_len: info.signed_len,
        doc_timestamp: info.doc_timestamp,
        summary,
        details: info.details.clone(),
        next_steps,
    }
}

/// Verify every signature field against `trust`.
pub fn verify_doc(doc: &Document, bytes: &[u8], trust: &TrustStore) -> Vec<SignatureReport> {
    list(doc, bytes, trust).iter().map(|s| report_for(s, trust)).collect()
}

/// Verify against the user's trust plus the bundled CCA roots. Returns the
/// reports and the combined store used.
pub fn verify_doc_with_cca(doc: &Document, bytes: &[u8], user_trust: &TrustStore) -> (Vec<SignatureReport>, TrustStore) {
    let mut certs = user_trust.certs.clone();
    if let Ok(roots) = cca::certificates() {
        for r in roots {
            if !certs.iter().any(|c| c.raw == r.raw) {
                certs.push(r);
            }
        }
    }
    let combined = TrustStore { certs };
    (verify_doc(doc, bytes, &combined), combined)
}

/// Summarize per-signature reports into one document verdict.
pub fn summarize(reports: &[SignatureReport]) -> DocumentSummary {
    let signed = reports.iter().filter(|r| r.signed).count();
    let mut s = DocumentSummary {
        fields: reports.len(),
        signed,
        valid: 0,
        invalid: 0,
        untrusted: 0,
        unsupported: 0,
        altered: 0,
        expired_or_invalid_cert: 0,
        revocation_unknown: 0,
        modified_disallowed: 0,
        overall: String::new(),
        overall_text: String::new(),
    };
    for r in reports.iter().filter(|r| r.signed) {
        match r.status {
            Status::Valid => s.valid += 1,
            Status::Invalid => s.invalid += 1,
            Status::Unknown => {}
        }
        if matches!(r.integrity, Integrity::Altered { .. }) {
            s.altered += 1;
        }
        if matches!(r.integrity, Integrity::Unknown { .. }) {
            s.unsupported += 1;
        }
        if matches!(r.trust, Trust::Untrusted) {
            s.untrusted += 1;
        }
        if matches!(r.cert_validity, CertValidity::NotValidAtSigning { .. } | CertValidity::NoCertificate) {
            s.expired_or_invalid_cert += 1;
        }
        if matches!(r.revocation, Revocation::Unknown) {
            s.revocation_unknown += 1;
        }
        if matches!(r.modification, Modification::Disallowed(_)) {
            s.modified_disallowed += 1;
        }
    }
    let (overall, overall_text) = if signed == 0 {
        ("no-signatures", "This document has no digital signatures to verify.")
    } else if s.invalid > 0 || s.altered > 0 {
        ("invalid", "At least one signature is invalid: the document changed or the signature does not verify.")
    } else if s.unsupported > 0 {
        ("unsupported", "Verification is incomplete: at least one signature uses an algorithm or structure PdfCraft cannot check yet.")
    } else if s.expired_or_invalid_cert > 0 {
        ("expired-or-invalid-cert", "At least one signer certificate was not valid at the signing time.")
    } else if s.untrusted > 0 {
        ("untrusted", "Signatures are intact but at least one signer is not trusted.")
    } else {
        ("valid", "All signatures are valid and trusted.")
    };
    s.overall = overall.to_string();
    s.overall_text = overall_text.to_string();
    s
}
