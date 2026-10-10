//! Digital signatures (execution plan M9; ISO 32000-2 §12.8; ETSI EN 319 142 PAdES).
//!
//! - [`der`], [`x509`], [`cms`], [`pkcs12`], [`keys`]: the cryptographic formats, on RustCrypto
//!   primitives (RSA private-key operations use aws-lc-rs on native targets, ADR-0009).
//! - [`pdf`]: signature fields in a document — listing and validating them, and signing
//!   (PAdES B-B, `ETSI.CAdES.detached`) with an incremental save.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod cms;
pub mod der;
pub mod dss;
mod ec512;
pub mod graphic;
#[cfg(target_os = "macos")]
pub mod keychain;
pub mod keys;
pub mod pdf;
pub mod pkcs12;
pub mod revocation;
mod rsa_pad;
pub mod timestamp;
#[cfg(target_os = "windows")]
pub mod windows;
pub mod x509;

pub use der::Time;
pub use graphic::Graphic;
pub use keys::{DigestAlg, PrivateKey, PublicKey};
pub use pdf::{
    Appearance, DigestCache, Modification, SignOptions, SignatureInfo, Status, TrustStore, list as signatures, sign, sign_with_timestamp,
    timestamp_document, validate,
};
pub use pkcs12::DigitalId;
pub use timestamp::{TimestampAuthority, TimestampQuery, TimestampToken};
pub use x509::{Certificate, Name};

#[derive(Debug, thiserror::Error)]
pub enum SignError {
    #[error("malformed data: {0}")]
    Malformed(String),
    #[error("not supported: {0}")]
    Unsupported(String),
    #[error("the password is incorrect")]
    WrongPassword,
    #[error("{0}")]
    Crypto(String),
    #[error("{0}")]
    Pdf(String),
    #[error(transparent)]
    Cos(#[from] pdfcraft_cos::CosError),
}
