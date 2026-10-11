//! India CCA trust roots for e-Aadhaar signature verification.
//!
//! The Controller of Certifying Authorities (CCA) roots that UIDAI e-Aadhaar
//! signer chains anchor in. Bundled so verification works offline with the same
//! trust the standalone e-Aadhaar verifier uses (pyHanko plus these roots), but
//! checked natively by the pdf module against this trust store.
//!
//! - Source: https://cca.gov.in/root_certificate.html (public root certificates).
//! - Copies compared against the eaadhaar-pdf-signature-verifier repository
//!   (CCAIndia2022.cer, CCAIndia2022SPL.cer), which ships the same files.
//! - CCA India 2022: self-signed, 2022-02-02 to 2042-02-02 (see fingerprint const).
//! - CCA India 2022 SPL: self-signed, 2022-09-20 to 2042-09-20 (see fingerprint const).
//! - Verification is local and offline: these bytes are the only trust input, no
//!   network fetch happens. Trust is by chain to these roots, never by subject name.
//! - Revocation still needs embedded evidence (/DSS): without it the report says
//!   revocation is unknown (see the verify module).

use crate::SignError;
use crate::pdf::TrustStore;
use crate::x509::Certificate;

/// Public CCA root source (India PKI).
pub const SOURCE_URL: &str = "https://cca.gov.in/root_certificate.html";

/// SHA-256 fingerprint (openssl x509 -fingerprint -sha256) of CCA India 2022.
pub const CCA_INDIA_2022_FINGERPRINT: &str = "9A:3F:D3:17:67:98:E8:42:DD:CB:12:C2:62:F1:1C:FA:CC:A7:0A:8B:84:C6:EA:6F:DA:30:84:2A:95:A9:4C:D8";

/// SHA-256 fingerprint of CCA India 2022 SPL.
pub const CCA_INDIA_2022_SPL_FINGERPRINT: &str = "B7:24:68:9B:79:B2:EF:94:21:EF:8F:5C:C7:33:EB:09:38:51:B1:70:EE:71:51:77:00:5A:09:F2:26:D8:C9:1A";

/// A CCA root certificate (PEM).
pub const CCA_INDIA_2022_PEM: &str = r#"-----BEGIN CERTIFICATE-----
MIIFNDCCAxygAwIBAgIQdiQz69smdlqFYM0KqC/hFzANBgkqhkiG9w0BAQsFADA6
MQswCQYDVQQGEwJJTjESMBAGA1UEChMJSW5kaWEgUEtJMRcwFQYDVQQDEw5DQ0Eg
SW5kaWEgMjAyMjAeFw0yMjAyMDIxMjA0MzdaFw00MjAyMDIxMjA0MzdaMDoxCzAJ
BgNVBAYTAklOMRIwEAYDVQQKEwlJbmRpYSBQS0kxFzAVBgNVBAMTDkNDQSBJbmRp
YSAyMDIyMIICIjANBgkqhkiG9w0BAQEFAAOCAg8AMIICCgKCAgEAv3EBudWC8HY0
oSwtJZCqpjQTGpEewl3EdDqUORV0qoFp78mdR/vuATXI83G7nF9RLvmNjgQgKr/b
Mx6gPO4Y57bMjAsgwEzleFclZka/sqc68iN5rS3huhrCX6MEINLyDOQ71MRA7GJC
aNL6E3j1438eTu011mlikeZYBdkhvfpAVjCw90w8wcWDmqx66Y561T/RiXyz2uEh
BBZAD43gV58eXStOeOTwAzEZYMrmp232GfmQKabYRfdIRus1avyuGea2nICEsRHE
8M2tdzwpGP7oIy2qHBFJJ+3AwmwQA4DjmDkJtCD+58awohQavRNhqjsGD+ZifG3V
R4i6WrKv8OWqZzcZj3g3Elr5+fRMlz1GSqkWPBw1Ev8KWTHazSUKF7OMxm3XzyXx
Qnw7fZF9GOVtx3adpfRPqYGgtbOP34EVkz4wsHvNMrvUrYcKymdOrnkTjlX26fIH
UJpKGYkLk9q0jhMNKs4Rn8lj4pJ7YF33/ND4bjpV0ex1EAQz0iZvT37OnxNiuAZ/
+4Djf075UuNX2ecWnadOrN1r8NAParZIwUoSUnWhU8TqAWWRqzFURHUZuOMQcA0g
eg4c9zqtBoUPgtQksbIAEsEXmDuRpwSIFjEkK11f5Eemfmfdg37KyIjQ67TRTmBA
+kT9Q5JIm/e7m1ILg/HKckgLUOCnAMsCAwEAAaM2MDQwDwYDVR0TAQH/BAUwAwEB
/zARBgNVHQ4ECgQITjtINlziX30wDgYDVR0PAQH/BAQDAgEGMA0GCSqGSIb3DQEB
CwUAA4ICAQCdbE8d1c1DysKtrtYlApYIXTlY3N2XHNQ6gKoaVWsKa1TJ/ovrT+FV
3bmQLet3aSoEG6pTe/vLZSg8WiF7cn7WuF4XlQS3yA2Uu8/cg/S4owqhQJp6K/Xg
6UoSBad9Kog1H8deOfV8Nmb8a89zB4Yf8/AepId+Lr/3I6O7iub+PUT2QBXnksa+
cf0yf+49GhyMCILZvctNSQd4Vxr9EgRvBARTrAgNQ9sEOJ6myOz4iTFR7T2pIFP8
Cp15e8jEVI1q4IuHu3XlwJNk9f5k3gbwrzoy9P5rP8voQU3u9wh62JZa9U63b+u/
Ur1tsKb5Lx0YUedtHvpIiIRurEPxumW0twjrx8TrAcXRrViSL7dsXAoYC0dXo154
EE8jBAzgIIur7tJizxgXDEn4i2pu8Yd615YML9ii5BooEJ2j6fQ0nzyPRmx1Egw2
Fjlgzzceai4TUOcaCKab86yyu5MZIp+BiPR840nw5MggbRgYH2nFRBA70toVm4VF
lbZs3reGmaICm4ST6R395OxYS1iYBm5kXm9tLb4pkIhUxrkgyuiwE+DsWceBjHAY
aXnCgUGKtiG9tfBMUw3fChoPb9L1yKdNof3zXDdTloMqEpO4BFrmjco8kt1v0LUQ
PhNZmQP4nqd4Hqx2384nPmWDXbQ+eePyxRteYGY0hJeDLVpyeYG8VQ==
-----END CERTIFICATE-----
"#;

/// A CCA root certificate (PEM).
pub const CCA_INDIA_2022_SPL_PEM: &str = r#"-----BEGIN CERTIFICATE-----
MIIFPDCCAySgAwIBAgIQYoKBxu6+xz94CH5f9Y9J9DANBgkqhkiG9w0BAQsFADA+
MQswCQYDVQQGEwJJTjESMBAGA1UEChMJSW5kaWEgUEtJMRswGQYDVQQDExJDQ0Eg
SW5kaWEgMjAyMiBTUEwwHhcNMjIwOTIwMDkxODE5WhcNNDIwOTIwMDkxODE5WjA+
MQswCQYDVQQGEwJJTjESMBAGA1UEChMJSW5kaWEgUEtJMRswGQYDVQQDExJDQ0Eg
SW5kaWEgMjAyMiBTUEwwggIiMA0GCSqGSIb3DQEBAQUAA4ICDwAwggIKAoICAQDM
A++VJxEXN+coznBAEf0dz+8DBj9SpEQGoehjxoDnD+WYEBAXnap2lh5yE7/wlpHU
Q7hJ54JsqLBZQGkM0pk35bkvvcf2wAGSdK0KRilNeFDmVdduqAUJUlmNeL0ufIuf
sSBEusOWKK6VvQHxiZ0qoyoeqV+CpnDm8I2IE0WBSbaotXtGWSyBLNDCEH3lRA9G
QDOZ5Utc8soR2YVwbQSicGoyAC1PiPah7LY6nZCbBjc63r22dE+Cm1TGeqEb0ZUd
hHl051WqEGXXmtQNfytdNP+VdKU1nrYEcQ5BeGMaWA5bHkO2ERldI5F55pX06hUx
wjL39H6JVX5/0I+bBiPsZiCpea2gKozNBg+MBWLqyBUzFRVhLfdCPB6tm9EKHgrl
P6rsWrjgZ5FcREmerCYs7HIYeemiI0UO4X8rqpobT3rL7mWALB2plyFaBdiUWW1r
87eZQDEeYWavOhNITYNc31FG3Kn8DANF3IhNeYmUDzG7/M1XyGycqBmXG7tVVXez
O1GbgfM0SYlkVFRDOD6u9Tf0lR1MfjrJpMB+JdIJlimMM5G6LGia7+3Nlll8SJ0D
hW49FdKlLB92iXuOok4iwPzyFKg8Eax/9iN9TxMadYxbpKkePL9DpL/ymRvpGDkP
aurqUP77smtB8Jlf7bx8SB9DWVjVqsgvemN+DZfNawIDAQABozYwNDAPBgNVHRMB
Af8EBTADAQH/MBEGA1UdDgQKBAhIEoydvDOh6jAOBgNVHQ8BAf8EBAMCAQYwDQYJ
KoZIhvcNAQELBQADggIBADYv91JbnwU+Ih5gNzZSJY0yJkYk4tbBBsCZDivmZknC
TbM8B9j/hEZcfXZCTbP6GCGkrxVx8aDl7E1s5DGmdO7x5R+dxrLD2B4+ORhDetlM
Yd22mopVnzqY5UdaQ8u16JNEp/m75UqT5NvcgtE+/s1Hr+3lhWKKvN7u9PxDSIoM
T/I0/sje9fWwjeX29nzqHTw7hYTLCIeQv8c9+wGBFAAFArUq0MaI6jxIav10DKeH
ptycUHwMNyzP9hh7G4nHo+lEIT0/jWIrjv53+aVFLdAvBKQ2jyAKd/OhuQFCue3z
MVMoTQ7Zl9HZk72CkBecaZhHnEzqjpOzueCweK0h27IvwED7scudpOPnY7ml6fEF
dpHXZXcVz8TZbqirkHTLepfAOUBcyuadAKkOR+m5uU1UKkESexaY2yVEdWcVAeCM
6p6FXlFeRMEebMbppkPeSFUPMRA9mhsUTOk5sbnGN2TVkSfUpcUMoQgz2bSqvGyR
7sAZ9Un457swjwEEEnzijdMvd9YDj1wRq+tsq38zq5IaE50VMMXeBoVz2/2Bn8Mr
p8kN7XIkrCui8wjclnG9BB1SbwjXPhE+7zE6GKh+oIRZua2RkamTpjwe+2oSihGV
FeD7pSMh/Pgyxg1lxSVDKvbBdxNrBpot2OQN35+DkAU2exJMtUYAvzdPVx9d2elo

-----END CERTIFICATE-----
"#;

/// The bundled CCA roots as certificates (in PEM order).
pub fn certificates() -> Result<Vec<Certificate>, SignError> {
    let mut out = crate::x509::load_certificates(CCA_INDIA_2022_PEM.as_bytes())?;
    out.extend(crate::x509::load_certificates(CCA_INDIA_2022_SPL_PEM.as_bytes())?);
    Ok(out)
}

/// A [] of the bundled CCA roots.
pub fn trust_store() -> Result<TrustStore, SignError> {
    Ok(TrustStore { certs: certificates()? })
}

/// Whether  is one of the bundled CCA roots (byte comparison).
pub fn is_cca_root(cert: &Certificate) -> bool {
    certificates().map(|roots| roots.iter().any(|r| r.raw == cert.raw)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_holds_two_self_signed_ca_roots() {
        let roots = certificates().unwrap();
        assert_eq!(roots.len(), 2);
        for r in &roots {
            assert!(r.is_self_signed(), "{}", r.subject.display());
            assert!(r.may_issue(), "{}", r.subject.display());
        }
        let names: Vec<String> = roots.iter().map(|r| r.subject.display()).collect();
        assert!(names.iter().any(|n| n.contains("CCA India 2022")), "{names:?}");
        assert!(names.iter().any(|n| n.contains("SPL")), "{names:?}");
    }

    #[test]
    fn fingerprints_match_the_published_roots() {
        use crate::keys::DigestAlg;
        let roots = certificates().unwrap();
        let fps: Vec<String> =
            roots.iter().map(|r| DigestAlg::Sha256.digest(&[&r.raw]).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")).collect();
        assert!(fps.contains(&CCA_INDIA_2022_FINGERPRINT.to_string()), "{fps:?}");
        assert!(fps.contains(&CCA_INDIA_2022_SPL_FINGERPRINT.to_string()), "{fps:?}");
        assert_eq!(trust_store().unwrap().certs.len(), 2);
        assert!(roots.iter().all(is_cca_root));
    }
}
