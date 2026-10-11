//! types.rs — Strongly typed request and response value objects for document operations.
//! =========================================================================================
//!
//! Purpose:
//!     Decouples the SDK from raw filesystem paths (`DocumentSource`) and
//!     prevents high-memory allocations via streamed/file-backed results (`DocumentResult`).
//!
//! Layer:
//!     Domain Layer / Data Transfer Objects
//!
//! Key Input Dependencies:
//!     - std::path::PathBuf (File paths)
//!     - serde (Serialization and deserialization)
//!
//! Usage Examples:
//!     ```rust
//!     use pdfcraft_sdk::types::{DocumentSource, MergeOptions};
//!     let src = DocumentSource::from_bytes(b"%PDF-1.4...".to_vec(), None);
//!     let opts = MergeOptions::default();
//!     ```
//!
//! Key Types & Functions Index:
//!     - DocumentSource: Input abstraction for PDF files (Path or Bytes)
//!     - DocumentResult: Output abstraction (Memory or File)
//!     - MergeOptions: Options configuring document merging
//!     - SplitMode: Modes for splitting PDF documents (EveryNPages, BeforePages, AtBookmarks)
//!     - SplitResult: Split output file paths list
//!     - RenderOptions: Options for rendering pages (dpi, image format)
//!     - ResourceLimits: Safety resource quota limits (max request bytes, max memory, max pages)

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Input abstraction decoupling operations from raw filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentSource {
    /// A path to a local PDF file.
    Path(PathBuf),
    /// In-memory PDF bytes with an optional virtual filename.
    Bytes { data: Vec<u8>, name: Option<String> },
}

impl DocumentSource {
    pub fn from_bytes(data: impl Into<Vec<u8>>, name: Option<String>) -> Self {
        Self::Bytes { data: data.into(), name }
    }

    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self::Path(path.into())
    }

    /// Returns the byte length if known without disk I/O.
    pub fn byte_len(&self) -> Option<usize> {
        match self {
            Self::Bytes { data, .. } => Some(data.len()),
            Self::Path(_) => None,
        }
    }
}

impl From<&str> for DocumentSource {
    fn from(s: &str) -> Self {
        Self::Path(PathBuf::from(s))
    }
}

impl From<PathBuf> for DocumentSource {
    fn from(p: PathBuf) -> Self {
        Self::Path(p)
    }
}

impl From<Vec<u8>> for DocumentSource {
    fn from(data: Vec<u8>) -> Self {
        Self::Bytes { data, name: None }
    }
}

/// Output abstraction preventing unbounded in-memory allocations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DocumentResult {
    /// In-memory result for smaller transformations.
    Memory { data: Vec<u8>, mime_type: String },
    /// Spilled or file-backed result for large documents.
    File { path: PathBuf, mime_type: String, size_bytes: u64 },
}

impl DocumentResult {
    pub fn memory(data: Vec<u8>, mime_type: impl Into<String>) -> Self {
        Self::Memory { data, mime_type: mime_type.into() }
    }

    pub fn file(path: PathBuf, mime_type: impl Into<String>, size_bytes: u64) -> Self {
        Self::File { path, mime_type: mime_type.into(), size_bytes }
    }

    pub fn mime_type(&self) -> &str {
        match self {
            Self::Memory { mime_type, .. } => mime_type,
            Self::File { mime_type, .. } => mime_type,
        }
    }

    pub fn byte_len(&self) -> u64 {
        match self {
            Self::Memory { data, .. } => data.len() as u64,
            Self::File { size_bytes, .. } => *size_bytes,
        }
    }
}

/// Result of a split operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitResult {
    pub files: Vec<PathBuf>,
}

/// Options configuring document merging.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MergeOptions {
    pub output_filename: Option<String>,
    pub pages: Option<Vec<Option<String>>>,
    pub passwords: Option<Vec<Option<String>>>,
}

/// Partitioning strategies for document splitting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SplitMode {
    EveryNPages(usize),
    BeforePages(Vec<usize>),
    AtBookmarks,
    MaxFileSizeMb(f64),
}

pub use crate::image::ImageFormat;

/// Rendering configuration strictly calculating physical DPI decoupled from monitor PPI (#739).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderOptions {
    pub dpi: f64,
    pub format: ImageFormat,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self { dpi: 150.0, format: ImageFormat::Png }
    }
}

/// Resource constraints configuring memory and processing budgets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub max_request_bytes: usize,
    pub max_output_bytes: usize,
    pub max_memory_bytes: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_request_bytes: 100 * 1024 * 1024, // 100 MB
            max_output_bytes: 500 * 1024 * 1024,  // 500 MB
            max_memory_bytes: 10 * 1024 * 1024,   // 10 MB memory buffer before spilling to disk
        }
    }
}
