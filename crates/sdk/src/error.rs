//! error.rs — Strongly typed error models conforming to PdfCraft's "Never Crash" principles.
//! =========================================================================================
//!
//! Purpose:
//!     Provides structured, actionable errors without disclosing internal paths,
//!     passwords, or uncontained stack traces. Maps cleanly to RFC 7807 Problem Details.
//!
//! Layer:
//!     Domain Layer / Error Handling
//!
//! Key Input Dependencies:
//!     - thiserror::Error
//!
//! Usage Examples:
//!     ```rust
//!     use pdfcraft_sdk::error::SdkError;
//!     let err = SdkError::InvalidArgument("Empty input".into());
//!     assert_eq!(err.http_status(), 400);
//!     ```
//!
//! Key Types & Functions Index:
//!     - SdkError: High-level SDK error enum (Io, FileNotFound, PageOutOfBounds, InvalidArgument, QuotaExceeded, EngineError, EnginePanic, UnsupportedOperation)
//!     - SdkError::http_status: Maps error to RFC 7807 HTTP status code
//!     - SdkError::error_code: Returns machine-readable uppercase snake_case error identifier

use thiserror::Error;

/// High-level SDK error enum returning actionable diagnostics.
#[derive(Debug, Error)]
pub enum SdkError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("File not found: {path}")]
    FileNotFound { path: String },

    #[error("Page index {page} out of bounds for document (total pages: {total})")]
    PageOutOfBounds { page: usize, total: usize },

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Resource quota exceeded: {0}")]
    QuotaExceeded(String),

    #[error("Engine execution failure: {0}")]
    EngineError(String),

    #[error("Engine panic caught and contained: {0}")]
    EnginePanic(String),

    #[error("Operation is not supported: {0}")]
    UnsupportedOperation(String),
}

impl SdkError {
    /// Returns the standard RFC 7807 HTTP status code corresponding to this error.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::InvalidArgument(_) => 400,
            Self::Unauthorized(_) => 401,
            Self::PageOutOfBounds { .. } => 400,
            Self::FileNotFound { .. } => 404,
            Self::QuotaExceeded(_) => 413,
            Self::UnsupportedOperation(_) => 501,
            Self::Io(_) | Self::EngineError(_) | Self::EnginePanic(_) => 500,
        }
    }

    /// Returns a machine-readable error code.
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::InvalidArgument(_) => "INVALID_ARGUMENT",
            Self::Unauthorized(_) => "UNAUTHORIZED",
            Self::PageOutOfBounds { .. } => "PAGE_OUT_OF_BOUNDS",
            Self::FileNotFound { .. } => "FILE_NOT_FOUND",
            Self::QuotaExceeded(_) => "QUOTA_EXCEEDED",
            Self::UnsupportedOperation(_) => "UNSUPPORTED_OPERATION",
            Self::EnginePanic(_) => "ENGINE_PANIC_CONTAINED",
            Self::EngineError(_) => "ENGINE_ERROR",
            Self::Io(_) => "IO_ERROR",
        }
    }
}
