//! Signature tools: list and validate signatures, create a self-signed digital ID, sign a
//! document, and manage trusted certificates. Rectangles are points from the top-left of the
//! displayed page.

use pdfcraft_engine::sign::verify::{CertValidity, Integrity, Revocation, TimeSource, Trust};
use pdfcraft_engine::sign::{self, Certificate, DigitalId, Modification, Name, PrivateKey};
use pdfcraft_engine::{SignOptions, SignatureInfo, SignatureReport, SignatureStatus, VerifyTrust};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, failed, write_atomic};

fn bad(m: impl Into<String>) -> ToolError {
    ToolError::InvalidArgs(m.into())
}

/// Seconds since 1970 → a DER time.
fn time(secs: i64) -> sign::Time {
    sign::Time::from_unix(secs)
}

pub(crate) fn cert_json(c: &Certificate) -> Value {
    json!({
        "name": c.display_name(),
        "subject": c.subject.display(),
        "issuer": c.issuer.display(),
        "email": c.subject.email(),
        "serial": c.serial_hex(),
        "valid_from": c.not_before.to_string(),
        "valid_to": c.not_after.to_string(),
        "key": c.public_key.describe(),
        "self_signed": c.is_self_signed(),
        "sha256": c.fingerprint(),
    })
}

impl Automation {
    fn sig_json(&self, a: &Args, s: &SignatureInfo) -> Result<Value> {
        let doc = self.doc(a)?;
        let rect = match (s.page, s.rect) {
            (Some(p), Some(r)) if s.visible => doc.info.pages.get(p).map(|pi| {
                let (a0, a1) = (pi.user_to_view(r[0] as f32, r[3] as f32), pi.user_to_view(r[2] as f32, r[1] as f32));
                json!([a0[0].min(a1[0]), a0[1].min(a1[1]), a0[0].max(a1[0]), a0[1].max(a1[1])])
            }),
            _ => None,
        };
        let (modification, changes) = match &s.modification {
            Modification::None => ("none", Vec::new()),
            Modification::Allowed(c) => ("allowed", c.clone()),
            Modification::Disallowed(c) => ("disallowed", c.clone()),
        };
        Ok(json!({
            "field": s.field,
            "signed": s.signed,
            "status": if !s.signed { "unsigned" } else { match s.status { SignatureStatus::Valid => "valid", SignatureStatus::Unknown => "unknown", SignatureStatus::Invalid => "invalid" } },
            "summary": s.summary(),
            "signer": s.signer,
            "certificate": s.certificate.as_ref().map(cert_json),
            "chain": s.chain.iter().skip(1).map(cert_json).collect::<Vec<_>>(),
            "date": s.date,
            "reason": s.reason,
            "location": s.location,
            "contact": s.contact,
            "certify": s.certify,
            "page": s.page.map(|p| p + 1),
            "rect": rect,
            "visible": s.visible,
            "revision": s.revision,
            "sub_filter": s.sub_filter,
            "algorithm": s.algorithm,
            "timestamp": s.timestamp,
            "timestamp_time": s.timestamp_time.map(|t| t.to_string()),
            "modification": modification,
            "changes": changes,
            "details": s.details,
        }))
    }

    pub(crate) fn sign_list(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let list: Vec<Value> = doc.signatures.iter().map(|s| self.sig_json(a, s)).collect::<Result<_>>()?;
        let signed = doc.signatures.iter().filter(|s| s.signed).count();
        let all_valid = signed > 0 && doc.signatures.iter().filter(|s| s.signed).all(|s| s.status == SignatureStatus::Valid);
        Ok(json!({ "count": list.len(), "signed": signed, "all_valid": all_valid, "signatures": list }))
    }

    fn report_json(r: &SignatureReport) -> Value {
        let (integrity, integrity_reason) = match &r.integrity {
            Integrity::Intact => ("intact", None),
            Integrity::Altered { reason } => ("altered", Some(reason.clone())),
            Integrity::Unknown { reason } => ("unknown", Some(reason.clone())),
            Integrity::NotApplicable => ("not-applicable", None),
        };
        let (trust, anchor, via_cca) = match &r.trust {
            Trust::Trusted { anchor, via_cca } => ("trusted", Some(anchor.clone()), Some(*via_cca)),
            Trust::Untrusted => ("untrusted", None, None),
        };
        let (cert_state, cert_detail) = match &r.cert_validity {
            CertValidity::Valid => ("valid", None),
            CertValidity::NotValidAtSigning { detail } => ("not-valid-at-signing", Some(detail.clone())),
            CertValidity::NoSigningTime => ("no-signing-time", None),
            CertValidity::NoCertificate => ("no-certificate", None),
        };
        let (revocation, revocation_detail) = match &r.revocation {
            Revocation::Good => ("good", None),
            Revocation::Revoked { detail } => ("revoked", Some(detail.clone())),
            Revocation::Unknown => ("unknown", None),
        };
        let (modification, changes) = match &r.modification {
            Modification::None => ("none", Vec::new()),
            Modification::Allowed(c) => ("allowed", c.clone()),
            Modification::Disallowed(c) => ("disallowed", c.clone()),
        };
        let time_source = match &r.time_source {
            TimeSource::TrustedTimestamp => "trusted-timestamp",
            TimeSource::SignerClaim => "signer-claim",
            TimeSource::None => "none",
        };
        json!({
            "field": r.field,
            "signed": r.signed,
            "status": if !r.signed { "unsigned" } else { match r.status { SignatureStatus::Valid => "valid", SignatureStatus::Unknown => "unknown", SignatureStatus::Invalid => "invalid" } },
            "summary": r.summary,
            "integrity": integrity,
            "integrity_reason": integrity_reason,
            "trust": trust,
            "trust_anchor": anchor,
            "trust_via_cca": via_cca,
            "certificate_validity": cert_state,
            "certificate_validity_detail": cert_detail,
            "revocation": revocation,
            "revocation_detail": revocation_detail,
            "revocation_note": r.revocation_note,
            "modification": modification,
            "changes": changes,
            "time_source": time_source,
            "signing_time": r.signing_time,
            "signer": r.signer,
            "subject": r.subject,
            "issuer": r.issuer,
            "valid_from": r.cert_valid_from,
            "valid_to": r.cert_valid_to,
            "serial": r.serial,
            "key": r.key,
            "digest": r.digest,
            "algorithm": r.algorithm,
            "sub_filter": r.sub_filter,
            "reason": r.reason,
            "location": r.location,
            "page": r.page.map(|p| p + 1),
            "visible": r.visible,
            "revision": r.revision,
            "signed_len": r.signed_len,
            "doc_timestamp": r.doc_timestamp,
            "details": r.details,
            "next_steps": r.next_steps,
        })
    }

    pub(crate) fn sign_verify(&self, a: &Args) -> Result<Value> {
        let mode = match a.opt_str("trust")?.unwrap_or("both") {
            "both" => VerifyTrust::Both,
            "session" => VerifyTrust::Session,
            "cca" => VerifyTrust::Cca,
            other => return Err(bad(format!("unknown trust {other:?} (both, session, cca)"))),
        };
        let id = self.doc(a)?.id;
        let v = self.session.verify_signatures(id, mode).map_err(failed)?;
        let mode_name = match v.trust_mode {
            VerifyTrust::Session => "session",
            VerifyTrust::Cca => "cca",
            VerifyTrust::Both => "both",
        };
        Ok(json!({
            "overall": v.summary.overall,
            "overall_text": v.summary.overall_text,
            "fields": v.summary.fields,
            "signed": v.summary.signed,
            "valid": v.summary.valid,
            "invalid": v.summary.invalid,
            "untrusted": v.summary.untrusted,
            "unsupported": v.summary.unsupported,
            "altered": v.summary.altered,
            "expired_or_invalid_cert": v.summary.expired_or_invalid_cert,
            "revocation_unknown": v.summary.revocation_unknown,
            "modified_disallowed": v.summary.modified_disallowed,
            "trust": mode_name,
            "trust_roots": v.trust_roots,
            "cca_trusted": v.cca_trusted,
            "revocation_limitation": "Offline verification only checks revocation evidence embedded in the document (/DSS); fetching CRLs or OCSP responses is not performed.",
            "signatures": v.reports.iter().map(Self::report_json).collect::<Vec<_>>(),
        }))
    }

    pub(crate) fn sign_id_create(&mut self, a: &Args) -> Result<Value> {
        let name = a.str("name")?.trim().to_string();
        if name.is_empty() {
            return Err(bad("name must not be empty"));
        }
        let password = a.str("password")?;
        if password.chars().count() < 6 {
            return Err(bad("the password must have at least 6 characters"));
        }
        let path = self.resolve(a.str("path")?, true)?;
        let key = match a.opt_str("key")?.unwrap_or("rsa2048") {
            "rsa2048" => PrivateKey::generate_rsa(2048),
            "rsa3072" => PrivateKey::generate_rsa(3072),
            "rsa4096" => PrivateKey::generate_rsa(4096),
            "p256" => PrivateKey::generate_p256(),
            other => return Err(bad(format!("unknown key {other:?} (rsa2048, rsa3072, rsa4096, p256)"))),
        }
        .map_err(failed)?;
        let dn = Name::build(
            &name,
            a.opt_str("unit")?.unwrap_or(""),
            a.opt_str("organization")?.unwrap_or(""),
            a.opt_str("email")?.unwrap_or(""),
            a.opt_str("country")?.unwrap_or(""),
        );
        let years = a.opt_int("years")?.unwrap_or(5).clamp(1, 50) as u32;
        let now = self.session.now_secs();
        let serial = sign::keys::DigestAlg::Sha256.digest(&[name.as_bytes(), &now.to_be_bytes()])[..8].to_vec();
        let cert = Certificate::self_signed(&dn, &key, time(now), years, &serial).map_err(failed)?;
        let id = DigitalId { key, certificate: cert, chain: Vec::new(), friendly_name: Some(name) };
        let p12 = sign::pkcs12::write(&id, password).map_err(failed)?;
        write_atomic(&path, &p12)?;
        Ok(json!({ "path": path.to_string_lossy(), "certificate": cert_json(&id.certificate) }))
    }

    fn open_id(&self, a: &Args) -> Result<DigitalId> {
        // A macOS Keychain identity: "keychain:<fingerprint>" or "keychain:<common name>".
        if let Some(r) = a.str("id")?.strip_prefix("keychain:") {
            #[cfg(target_os = "macos")]
            return sign::keychain::find(&format!("keychain:{r}")).map_err(failed);
            #[cfg(not(target_os = "macos"))]
            return Err(failed(format!("keychain:{r}: Keychain identities are only available on macOS")));
        }
        if let Some(r) = a.str("id")?.strip_prefix("windows:") {
            #[cfg(target_os = "windows")]
            return sign::windows::find(&format!("windows:{r}")).map_err(failed);
            #[cfg(not(target_os = "windows"))]
            return Err(failed(format!("windows:{r}: Windows certificate store identities are only available on Windows")));
        }
        let path = self.resolve(a.str("id")?, false)?;
        let bytes = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        sign::pkcs12::open(&bytes, a.opt_str("password")?.unwrap_or("")).map_err(|e| match e {
            sign::SignError::WrongPassword => bad("the digital ID password is incorrect"),
            e => failed(e),
        })
    }

    pub(crate) fn sign_document(&mut self, a: &Args) -> Result<Value> {
        let id = self.open_id(a)?;
        let doc = self.doc(a)?;
        let doc_id = doc.id;
        let field = a.opt_str("field")?.map(str::to_string);
        let page = match a.opt_int("page")? {
            Some(p) if p >= 1 && (p as usize) <= doc.info.pages.len() => p as usize - 1,
            Some(p) => return Err(bad(format!("page {p} does not exist"))),
            None => 0,
        };
        let rect = match a.nums::<4>("rect")? {
            Some(r) => {
                let p = &doc.info.pages[page];
                let (u0, u1) = (p.view_to_user(r[0] as f32, r[1] as f32), p.view_to_user(r[2] as f32, r[3] as f32));
                let u = [u0[0].min(u1[0]) as f64, u0[1].min(u1[1]) as f64, u0[0].max(u1[0]) as f64, u0[1].max(u1[1]) as f64];
                if u[2] - u[0] < 4.0 || u[3] - u[1] < 4.0 {
                    return Err(bad("the signature rectangle is too small"));
                }
                Some(u)
            }
            None => None,
        };
        let certify = match a.get("certify") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(match s.as_str() {
                "no_changes" => 1,
                "form_fill" => 2,
                "comments" => 3,
                other => return Err(bad(format!("unknown certify level {other:?} (no_changes, form_fill, comments)"))),
            }),
            Some(v) => Some(v.as_u64().filter(|n| (1..=3).contains(n)).ok_or_else(|| bad("certify must be 1, 2 or 3"))? as u8),
        };
        let out = self.resolve(a.str("out")?, true)?;
        let opts = SignOptions {
            field,
            page,
            rect,
            reason: a.opt_str("reason")?.map(str::to_string),
            location: a.opt_str("location")?.map(str::to_string),
            contact: a.opt_str("contact")?.map(str::to_string),
            certify,
            ..SignOptions::default()
        };
        let signed = self.session.sign(doc_id, &id, opts).map_err(failed)?;
        write_atomic(&out, &signed)?;
        let path = out.to_string_lossy().into_owned();
        self.session.mark_signed(doc_id, signed.clone(), Some(path.clone())).map_err(failed)?;
        let doc = self.doc(a)?;
        let newest = doc.signatures.iter().filter(|s| s.signed).max_by_key(|s| s.revision).cloned();
        Ok(json!({ "path": path, "bytes": signed.len(), "signature": newest.map(|s| self.sig_json(a, &s)).transpose()? }))
    }

    pub(crate) fn sign_trust(&mut self, a: &Args) -> Result<Value> {
        let mut certs: Vec<Certificate> =
            if a.opt_bool("clear")?.unwrap_or(false) { Vec::new() } else { self.session.trusted_certificates().to_vec() };
        let paths: Vec<String> = match a.get("paths") {
            None => Vec::new(),
            Some(v) => v
                .as_array()
                .ok_or_else(|| bad("paths must be an array"))?
                .iter()
                .map(|p| p.as_str().map(str::to_string).ok_or_else(|| bad("paths must be strings")))
                .collect::<Result<_>>()?,
        };
        for p in paths {
            let path = self.resolve(&p, false)?;
            let bytes = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
            let lower = p.to_ascii_lowercase();
            let found = if lower.ends_with(".p12") || lower.ends_with(".pfx") {
                let id = sign::pkcs12::open(&bytes, a.opt_str("password")?.unwrap_or("")).map_err(failed)?;
                std::iter::once(id.certificate).chain(id.chain).collect()
            } else {
                sign::x509::load_certificates(&bytes).map_err(failed)?
            };
            for c in found {
                if !certs.iter().any(|t| t.raw == c.raw) {
                    certs.push(c);
                }
            }
        }
        self.session.set_trusted_certificates(certs);
        Ok(json!({ "trusted": self.session.trusted_certificates().iter().map(cert_json).collect::<Vec<_>>() }))
    }
}
