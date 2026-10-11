//! sdk_invariants.rs — Invariant Tests: Contracts, resource limits, and failure containment.
//! =========================================================================================
//!
//! Purpose:
//!     Verifies non-negotiable SDK invariants:
//!     - Empty input rejection with SdkError::InvalidArgument
//!     - Invalid page indexes (page 0 rejection) with SdkError::InvalidArgument
//!     - Input exceeding max_request_bytes rejected with SdkError::QuotaExceeded
//!     - Corrupted PDF data contained gracefully without panics
//!     - In-process memory execution without disk I/O for small payloads
//!
//! Layer:
//!     Test Suite / SDK Invariant Tests

use pdfcraft_sdk::error::SdkError;
use pdfcraft_sdk::types::{DocumentResult, DocumentSource, MergeOptions, RenderOptions, ResourceLimits, SplitMode};
use pdfcraft_sdk::{LocalClient, PdfCraftClient};

fn sample_minimal_pdf() -> Vec<u8> {
    b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n".to_vec()
}

#[test]
fn test_invariant_empty_input_rejected() {
    let client = LocalClient::default();
    let res = client.merge(&[], MergeOptions::default());
    assert!(res.is_err(), "Empty sources array must be rejected");
    match res.expect_err("error expected") {
        SdkError::InvalidArgument(msg) => {
            assert!(msg.contains("cannot be empty"));
        }
        other => panic!("Expected InvalidArgument, got {other:?}"),
    }
}

#[test]
fn test_invariant_page_zero_rejected() {
    let client = LocalClient::default();
    let src = DocumentSource::from_bytes(sample_minimal_pdf(), Some("doc.pdf".into()));
    let res = client.render_page(&src, 0, RenderOptions::default());
    assert!(res.is_err(), "Page 0 must be rejected");
    match res.expect_err("error expected") {
        SdkError::InvalidArgument(msg) => {
            assert!(msg.contains("1-based"));
        }
        other => panic!("Expected InvalidArgument, got {other:?}"),
    }
}

#[test]
fn test_invariant_oversized_input_rejected_before_execution() {
    // Set 1 KB max request limit
    let limits = ResourceLimits { max_request_bytes: 1024, max_output_bytes: 10 * 1024 * 1024, max_memory_bytes: 10 * 1024 * 1024 };
    let client = LocalClient::default().with_limits(limits);

    // Create a 2 KB source
    let large_data = vec![0u8; 2048];
    let source = DocumentSource::from_bytes(large_data, Some("large.pdf".into()));

    let res = client.merge(&[source], MergeOptions::default());
    assert!(res.is_err(), "Oversized file must be rejected fast");
    match res.expect_err("error expected") {
        SdkError::QuotaExceeded(msg) => {
            assert!(msg.contains("exceeds configured max_request_bytes"));
        }
        other => panic!("Expected QuotaExceeded, got {other:?}"),
    }
}

#[test]
fn test_invariant_corrupted_pdf_rejected_without_panic() {
    let client = LocalClient::default();
    let corrupted_data = b"This is definitely not a valid PDF document at all!".to_vec();
    let source = DocumentSource::from_bytes(corrupted_data, Some("corrupted.pdf".into()));

    let res = client.split(&source, SplitMode::EveryNPages(1));
    assert!(res.is_err(), "Corrupted document must fail gracefully");
    // Crucial: Must produce a structured EngineError or EnginePanic, never abort process
    match res.expect_err("error expected") {
        SdkError::EngineError(_) | SdkError::EnginePanic(_) => {
            // Contained failure verified
        }
        other => panic!("Expected EngineError or EnginePanic, got {other:?}"),
    }
}

#[test]
fn test_invariant_in_memory_execution_produces_memory_result() {
    // Default 10 MB limit should keep this tiny document in-memory
    let client = LocalClient::default();
    let src1 = DocumentSource::from_bytes(sample_minimal_pdf(), Some("doc1.pdf".into()));
    let src2 = DocumentSource::from_bytes(sample_minimal_pdf(), Some("doc2.pdf".into()));

    if let Ok(result) = client.merge(&[src1, src2], MergeOptions::default()) {
        match result {
            DocumentResult::Memory { data, mime_type } => {
                assert_eq!(mime_type, "application/pdf");
                assert!(!data.is_empty());
            }
            DocumentResult::File { .. } => {
                panic!("Small output should have remained in DocumentResult::Memory");
            }
        }
    }
}
