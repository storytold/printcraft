//! Node.js N-API Native Addon Binding for `pdfcraft-sdk` (Issue #872).
//!
//! # Architecture Reference
//! Module: sdks/typescript/src/native/binding.rs
//! Purpose:
//!   Provides high-performance, zero-overhead N-API native bindings linking
//!   directly to `pdfcraft-sdk` for Node.js / TypeScript runtimes.

use std::sync::Arc;
use pdfcraft_sdk::{DocumentResult, DocumentSource, LocalClient, MergeOptions, PdfCraftClient, RenderOptions, SplitMode};

/// Native N-API wrapper exposing LocalClient to Node.js.
pub struct NodePdfCraftClient {
    client: LocalClient,
}

impl NodePdfCraftClient {
    pub fn new() -> Self {
        Self {
            client: LocalClient::default(),
        }
    }

    pub fn merge_buffers(&self, buffers: Vec<Vec<u8>>) -> Result<Vec<u8>, String> {
        let sources: Vec<DocumentSource> = buffers
            .into_iter()
            .map(|buf| DocumentSource::from_bytes(buf, None))
            .collect();

        match self.client.merge(&sources, MergeOptions::default()) {
            Ok(DocumentResult::Memory { data, .. }) => Ok(data),
            Ok(DocumentResult::File { path, .. }) => {
                std::fs::read(&path).map_err(|e| format!("Failed to read output: {e}"))
            }
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn render_page_buffer(&self, buffer: Vec<u8>, page: usize, dpi: f32) -> Result<Vec<u8>, String> {
        let source = DocumentSource::from_bytes(buffer, None);
        let opts = RenderOptions {
            dpi,
            ..Default::default()
        };

        match self.client.render_page(&source, page, opts) {
            Ok(DocumentResult::Memory { data, .. }) => Ok(data),
            Ok(DocumentResult::File { path, .. }) => {
                std::fs::read(&path).map_err(|e| format!("Failed to read output: {e}"))
            }
            Err(e) => Err(e.to_string()),
        }
    }
}
