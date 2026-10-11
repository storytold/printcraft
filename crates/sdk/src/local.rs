//! local.rs — In-process engine driver executing operations through an injected OperationExecutor.
//! ==================================================================================================
//!
//! Purpose:
//!     Provides direct in-process document processing linking directly to the Rust engine
//!     through the abstract OperationExecutor port (open-source alternative to Adobe PDFL).
//!
//! Layer:
//!     Client Layer / Facade
//!
//! Key Input Dependencies:
//!     - crate::ports::OperationExecutor
//!     - crate::engine_adapter::AutomationEngineAdapter
//!     - crate::types::{DocumentResult, DocumentSource, MergeOptions, RenderOptions, ResourceLimits, SplitMode, SplitResult}
//!
//! Usage Examples:
//!     ```rust
//!     use pdfcraft_sdk::{LocalClient, PdfCraftClient, DocumentSource, MergeOptions};
//!     let client = LocalClient::default();
//!     let doc = DocumentSource::from_bytes(b"%PDF-1.4...".to_vec(), None);
//!     let result = client.merge(&[doc], MergeOptions::default());
//!     ```
//!
//! Key Types & Functions Index:
//!     - LocalClient: In-process client implementing PdfCraftClient trait
//!     - LocalClient::default: Constructs client backed by AutomationEngineAdapter
//!     - LocalClient::with_executor: Constructs client with injected custom executor
//!     - LocalClient::merge: Merges multiple PDF sources
//!     - LocalClient::split: Splits PDF into separate files
//!     - LocalClient::render_page: Renders page at physical DPI

use crate::PdfCraftClient;
use crate::error::SdkError;
use crate::ports::OperationExecutor;
use crate::types::{DocumentResult, DocumentSource, MergeOptions, RenderOptions, ResourceLimits, SplitMode, SplitResult};
use std::sync::Arc;

/// Local in-process client for running document transformations without an HTTP server.
pub struct LocalClient {
    executor: Arc<dyn OperationExecutor>,
    limits: ResourceLimits,
}

impl LocalClient {
    /// Creates a new `LocalClient` with the provided engine executor implementation and custom limits.
    pub fn new(executor: Arc<dyn OperationExecutor>) -> Self {
        Self { executor, limits: ResourceLimits::default() }
    }

    /// Configures resource constraints on this client.
    pub fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn limits(&self) -> &ResourceLimits {
        &self.limits
    }
}

impl Default for LocalClient {
    fn default() -> Self {
        let limits = ResourceLimits::default();
        Self { executor: Arc::new(crate::engine_adapter::AutomationEngineAdapter::new(limits.max_memory_bytes)), limits }
    }
}

impl PdfCraftClient for LocalClient {
    fn merge(&self, files: &[DocumentSource], options: MergeOptions) -> Result<DocumentResult, SdkError> {
        if files.is_empty() {
            return Err(SdkError::InvalidArgument("files list cannot be empty".into()));
        }
        for file in files {
            if let Some(len) = file.byte_len()
                && len > self.limits.max_request_bytes
            {
                return Err(SdkError::QuotaExceeded(format!(
                    "File size ({len} bytes) exceeds configured max_request_bytes ({} bytes)",
                    self.limits.max_request_bytes
                )));
            }
        }
        self.executor.execute_merge(files, &options)
    }

    fn split(&self, file: &DocumentSource, mode: SplitMode) -> Result<SplitResult, SdkError> {
        if let Some(len) = file.byte_len()
            && len > self.limits.max_request_bytes
        {
            return Err(SdkError::QuotaExceeded(format!(
                "File size ({len} bytes) exceeds configured max_request_bytes ({} bytes)",
                self.limits.max_request_bytes
            )));
        }
        self.executor.execute_split(file, &mode)
    }

    fn render_page(&self, file: &DocumentSource, page: usize, options: RenderOptions) -> Result<DocumentResult, SdkError> {
        if page == 0 {
            return Err(SdkError::InvalidArgument("Page number must be 1-based (greater than 0)".into()));
        }
        self.executor.execute_render(file, page, &options)
    }
}
