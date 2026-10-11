//! domain_tests.rs — Unit tests verifying SDK Domain Models and Port Contracts.
//! ==============================================================================
//!
//! Purpose:
//!     Verifies DocumentSource (Path & Bytes), DocumentResult (Memory & File),
//!     MergeOptions, SplitMode, and OperationExecutor port delegation.
//!
//! Layer:
//!     Test Suite / SDK Domain Tests

use pdfcraft_sdk::error::SdkError;
use pdfcraft_sdk::ports::OperationExecutor;
use pdfcraft_sdk::types::{DocumentResult, DocumentSource, MergeOptions, RenderOptions, SplitMode, SplitResult};
use pdfcraft_sdk::{LocalClient, PdfCraftClient};
use std::path::PathBuf;
use std::sync::Arc;

/// Test fake executor for contract verification.
struct FakeExecutor;

impl OperationExecutor for FakeExecutor {
    fn execute_merge(&self, sources: &[DocumentSource], _options: &MergeOptions) -> Result<DocumentResult, SdkError> {
        if sources.is_empty() {
            return Err(SdkError::InvalidArgument("sources cannot be empty".into()));
        }
        Ok(DocumentResult::memory(b"%PDF-1.7 merged".to_vec(), "application/pdf"))
    }

    fn execute_split(&self, _source: &DocumentSource, _mode: &SplitMode) -> Result<SplitResult, SdkError> {
        Ok(SplitResult { files: vec![PathBuf::from("part_1.pdf"), PathBuf::from("part_2.pdf")] })
    }

    fn execute_render(&self, _source: &DocumentSource, page: usize, _options: &RenderOptions) -> Result<DocumentResult, SdkError> {
        if page == 0 {
            return Err(SdkError::InvalidArgument("page must be 1-based".into()));
        }
        Ok(DocumentResult::memory(b"PNG_BYTES".to_vec(), "image/png"))
    }
}

#[test]
fn test_document_source_constructors() {
    let from_str = DocumentSource::from("sample.pdf");
    assert_eq!(from_str, DocumentSource::Path(PathBuf::from("sample.pdf")));

    let bytes = vec![1, 2, 3];
    let from_bytes = DocumentSource::from(bytes.clone());
    assert_eq!(from_bytes, DocumentSource::Bytes { data: bytes, name: None });
    assert_eq!(from_bytes.byte_len(), Some(3));
}

#[test]
fn test_document_result_variants() {
    let mem = DocumentResult::memory(vec![1, 2, 3], "application/pdf");
    assert_eq!(mem.byte_len(), 3);
    assert_eq!(mem.mime_type(), "application/pdf");

    let file = DocumentResult::file(PathBuf::from("/tmp/doc.pdf"), "application/pdf", 1024);
    assert_eq!(file.byte_len(), 1024);
    assert_eq!(file.mime_type(), "application/pdf");
}

#[test]
fn test_sdk_error_http_status_mapping() {
    let err_arg = SdkError::InvalidArgument("bad".into());
    assert_eq!(err_arg.http_status(), 400);
    assert_eq!(err_arg.error_code(), "INVALID_ARGUMENT");

    let err_quota = SdkError::QuotaExceeded("limit".into());
    assert_eq!(err_quota.http_status(), 413);
    assert_eq!(err_quota.error_code(), "QUOTA_EXCEEDED");

    let err_panic = SdkError::EnginePanic("panic caught".into());
    assert_eq!(err_panic.http_status(), 500);
    assert_eq!(err_panic.error_code(), "ENGINE_PANIC_CONTAINED");
}

#[test]
fn test_local_client_delegates_to_executor() {
    let executor = Arc::new(FakeExecutor);
    let client = LocalClient::new(executor);

    // Merge validation
    let empty_sources: Vec<DocumentSource> = vec![];
    assert!(client.merge(&empty_sources, MergeOptions::default()).is_err());

    let sources = vec![DocumentSource::from("a.pdf"), DocumentSource::from("b.pdf")];
    let res = client.merge(&sources, MergeOptions::default()).expect("merge ok");
    assert_eq!(res.mime_type(), "application/pdf");

    // Render validation
    assert!(client.render_page(&DocumentSource::from("a.pdf"), 0, RenderOptions::default()).is_err());
    let render_res = client.render_page(&DocumentSource::from("a.pdf"), 1, RenderOptions::default()).expect("render ok");
    assert_eq!(render_res.mime_type(), "image/png");
}

#[test]
fn test_apdfl_action_and_goto_action_domain_model() {
    use pdfcraft_sdk::annotation::{Action, GoToAction, URIAction};
    use pdfcraft_sdk::document::{Bookmark, FitMode, ViewDestination};

    let dest = ViewDestination::new(3, FitMode::Fit);
    let goto = GoToAction::new(dest.clone());
    let action = Action::GoTo(goto);

    match &action {
        Action::GoTo(g) => {
            assert_eq!(g.destination.page_number, 3);
            assert_eq!(g.destination.fit_mode, FitMode::Fit);
        }
        _ => panic!("Expected GoToAction"),
    }

    let mut bookmark = Bookmark::new("Chapter 1").with_destination(dest);
    bookmark.add_child(Bookmark::new("Section 1.1"));
    assert_eq!(bookmark.children.len(), 1);
    assert_eq!(bookmark.children[0].title, "Section 1.1");

    let uri_act = Action::URI(URIAction::new("https://example.com"));
    match uri_act {
        Action::URI(u) => assert_eq!(u.uri, "https://example.com"),
        _ => panic!("Expected URIAction"),
    }
}

#[test]
fn test_apdfl_annotations_and_redaction() {
    use pdfcraft_sdk::annotation::{Annotation, AnnotationSubtype, HighlightAnnotation, LineAnnotation, LineEnding, Redaction, RedactionStatus};
    use pdfcraft_sdk::graphics::{Color, Point, Quad, Rect};

    let rect = Rect::new(50.0, 100.0, 200.0, 50.0);
    let quads = vec![Quad::from_rect(&rect)];
    let annot = Annotation::new(rect, AnnotationSubtype::Highlight(HighlightAnnotation { quads, color: Color::RGB(1.0, 1.0, 0.0) }));
    assert!(annot.flags.print);
    assert_eq!(annot.opacity, 1.0);

    let redaction = Redaction {
        quads: vec![Quad::from_rect(&rect)],
        overlay_text: Some("REDACTED".into()),
        fill_color: Color::BLACK,
        status: RedactionStatus::Marked,
    };
    assert_eq!(redaction.status, RedactionStatus::Marked);

    let line = LineAnnotation {
        start_point: Point::new(10.0, 10.0),
        end_point: Point::new(100.0, 100.0),
        start_ending: LineEnding::None,
        end_ending: LineEnding::ClosedArrow,
        line_width: 2.0,
    };
    assert_eq!(line.end_ending, LineEnding::ClosedArrow);
}

#[test]
fn test_apdfl_content_elements_and_graphics() {
    use pdfcraft_sdk::content::{Path, Segment};
    use pdfcraft_sdk::graphics::{Matrix, Point};

    let mut path = Path::new();
    path.move_to(Point::new(0.0, 0.0));
    path.line_to(Point::new(100.0, 0.0));
    path.curve_to(Point::new(150.0, 50.0), Point::new(150.0, 100.0), Point::new(100.0, 100.0));
    path.close();
    path.stroke = true;

    assert_eq!(path.segments.len(), 4);
    assert!(matches!(path.segments[3], Segment::ClosePath));

    let m1 = Matrix::translation(50.0, 50.0);
    let m2 = Matrix::scale(2.0, 2.0);
    let combined = m1.multiply(&m2);
    let pt = combined.transform_point(Point::new(10.0, 10.0));
    // PDF row-vector convention: `a.multiply(b)` applies `a` first, then `b`
    // (translate (10,10) by (50,50) -> (60,60), then scale x2 -> (120,120)).
    assert_eq!(pt, Point::new(120.0, 120.0));
}

#[test]
fn test_apdfl_forms_and_data_exchange() {
    use pdfcraft_sdk::forms::{AcroFormExportType, AcroFormImportType, Field, FieldType};
    use pdfcraft_sdk::graphics::Rect;

    let field = Field::new_text("applicant_name", 1, Rect::new(100.0, 200.0, 150.0, 20.0));
    assert_eq!(field.name, "applicant_name");
    assert_eq!(field.page_number, 1);
    assert!(matches!(field.field_type, FieldType::Text(_)));

    assert_eq!(AcroFormExportType::FDF as u8, 1);
    assert_eq!(AcroFormExportType::XFDF as u8, 2);
    assert_eq!(AcroFormImportType::XML as u8, 3);
}

#[test]
fn test_apdfl_security_optimization_lowlevel() {
    use pdfcraft_sdk::lowlevel::{NameTree, PDFObject};
    use pdfcraft_sdk::optimization::{OptimizationParams, PDFOptimizer};
    use pdfcraft_sdk::security::{PermissionsFlags, SignDoc};

    // Security
    let perms = PermissionsFlags::default();
    assert!(perms.allow_print);
    assert!(perms.allow_copy);

    let sign = SignDoc::new(vec![0x30, 0x82]);
    assert_eq!(sign.cert_p12_bytes.len(), 2);

    // Optimization
    let opt = PDFOptimizer::new(OptimizationParams::default());
    assert!(opt.params.compress_streams);
    assert!(opt.params.remove_unreferenced_objects);

    // Low level
    let mut tree = NameTree::new();
    tree.insert("Dest1", PDFObject::Integer(42));
    assert_eq!(tree.lookup("Dest1"), Some(&PDFObject::Integer(42)));
}
