//! metadata.rs — Metadata & Attachments domain models matching Datalogics APDFL architecture.
//! =========================================================================================
//!
//! Purpose:
//!     Defines FileAttachment, FileSpecification, WatermarkParams, and WatermarkTextParams
//!     matching Datalogics APDFL Metadata & Attachments specifications.
//!
//! Layer:
//!     Domain Layer / Metadata & Attachments
//!
//! Key Types:
//!     - FileAttachment: Embedded file payload within PDF catalog
//!     - FileSpecification: Reference to external or embedded file asset
//!     - WatermarkParams: Graphical and textual watermark overlay settings

use crate::graphics::Color;
use serde::{Deserialize, Serialize};

/// Embedded file attachment embedded into a PDF document portfolio or catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAttachment {
    pub file_name: String,
    pub description: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub data: Vec<u8>,
}

impl FileAttachment {
    pub fn new(file_name: impl Into<String>, data: Vec<u8>) -> Self {
        let size_bytes = data.len() as u64;
        Self { file_name: file_name.into(), description: None, mime_type: None, size_bytes, data }
    }
}

/// File specification referencing an external or embedded resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSpecification {
    pub path: String,
    pub is_embedded: bool,
}

/// Parameters for stamping text watermarks onto document pages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatermarkParams {
    pub text: String,
    pub opacity: f64,
    pub rotation_degrees: f64,
    pub font_size: f64,
    pub color: Color,
    pub on_top: bool,
}

impl Default for WatermarkParams {
    fn default() -> Self {
        Self { text: "CONFIDENTIAL".into(), opacity: 0.5, rotation_degrees: 45.0, font_size: 48.0, color: Color::RGB(0.5, 0.5, 0.5), on_top: true }
    }
}
