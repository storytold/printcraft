//! security.rs — Digital Signatures & Security domain models matching Datalogics APDFL architecture.
//! ==============================================================================================
//!
//! Purpose:
//!     Defines SignDoc, DigitalSignature, UserPassword, OwnerPassword, PermissionsFlags,
//!     and EncryptionType matching Datalogics APDFL Security & Digital Signatures specifications.
//!
//! Layer:
//!     Domain Layer / Security & Digital Signatures
//!
//! Key Types:
//!     - EncryptionType: Standard PDF encryption handlers (AES-128, AES-256)
//!     - PermissionsFlags: Document usage permissions bitmask (print, copy, edit, annotate)
//!     - SignDoc: Cryptographic parameters for applying a digital signature
//!     - DigitalSignature: Signature dictionary metadata and validation result

use serde::{Deserialize, Serialize};

/// Standard PDF encryption algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EncryptionType {
    AES128,
    #[default]
    AES256,
}

/// PDF document permissions flags (P bitmask) controlling allowed user operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionsFlags {
    pub allow_print: bool,
    pub allow_high_quality_print: bool,
    pub allow_modify: bool,
    pub allow_copy: bool,
    pub allow_annotate: bool,
    pub allow_fill_forms: bool,
    pub allow_extract_access: bool,
    pub allow_assemble: bool,
}

impl Default for PermissionsFlags {
    fn default() -> Self {
        Self {
            allow_print: true,
            allow_high_quality_print: true,
            allow_modify: true,
            allow_copy: true,
            allow_annotate: true,
            allow_fill_forms: true,
            allow_extract_access: true,
            allow_assemble: true,
        }
    }
}

/// Cryptographic signing parameters for certifying or signing a PDF.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignDoc {
    pub cert_p12_bytes: Vec<u8>,
    pub password: Option<String>,
    pub signer_name: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact_info: Option<String>,
    pub timestamp_server_url: Option<String>,
}

impl SignDoc {
    pub fn new(cert_p12_bytes: Vec<u8>) -> Self {
        Self { cert_p12_bytes, password: None, signer_name: None, reason: None, location: None, contact_info: None, timestamp_server_url: None }
    }
}

/// Metadata and validation state of a digital signature in a PDF document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DigitalSignature {
    pub field_name: String,
    pub signer_name: Option<String>,
    pub signing_time: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub is_valid: bool,
    pub certificate_issuer: Option<String>,
}
