//! lib.rs — Unified Document Processing SDK for PdfCraft (Issue #872).
//! =========================================================================
//!
//! Purpose:
//!     Main entry point for pdfcraft-sdk. Provides the complete, unencumbered,
//!     memory-safe open-source domain model and client interfaces matching Datalogics APDFL.
//!
//! Layer:
//!     Core SDK Root
//!
//! Domains:
//!     - document: Document, Page, PageRange, PageLabel, Bookmark, ViewDestination
//!     - annotation: Action, GoToAction, URIAction, Annotation, HighlightAnnotation, Redaction
//!     - content: Element, Path, Segment, Content, Container, Group, Clip, Form
//!     - graphics: Point, Rect, Quad, Matrix, Color, ColorSpace, BlendMode, GraphicState
//!     - text: Font, Text, TextRun, TextState, Word, DocTextFinderMatch
//!     - image: Image, ImageData, DrawParams, PageImageParams, ImageSaveParams, ImageFormat
//!     - forms: Field, TextField, ButtonField, ChoiceField, SignatureField, AcroFormExportType
//!     - security: SignDoc, DigitalSignature, PermissionsFlags, EncryptionType
//!     - optimization: PDFOptimizer, OptimizationParams, FlattenTransparencyParams
//!     - lowlevel: PDFObject, PDFDict, PDFArray, PDFStream, PDFReference, NameTree, NumberTree
//!     - optionalcontent: OptionalContentGroup, OptionalContentContext, OptionalContentConfig
//!     - metadata: FileAttachment, FileSpecification, WatermarkParams
//!
//! Key Types & Functions Index:
//!     - PdfCraftClient: Unified document processing client trait (merge, split, render_page)
//!     - LocalClient: In-process client facade
//!     - AutomationEngineAdapter: Concrete engine adapter implementing OperationExecutor
//!     - SdkError: Strongly typed error enum
//!     - DocumentSource: Input abstraction (Path or Bytes)
//!     - DocumentResult: Output abstraction (Memory or File)

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod annotation;
pub mod content;
pub mod document;
pub mod engine_adapter;
pub mod error;
pub mod forms;
pub mod graphics;
pub mod image;
pub mod local;
pub mod lowlevel;
pub mod metadata;
pub mod optimization;
pub mod optionalcontent;
pub mod ports;
pub mod security;
pub mod text;
pub mod types;

pub use engine_adapter::AutomationEngineAdapter;
pub use error::SdkError;
pub use local::LocalClient;
pub use ports::OperationExecutor;
pub use types::{DocumentResult, DocumentSource, ImageFormat, MergeOptions, RenderOptions, ResourceLimits, SplitMode, SplitResult};

/// Re-exported canonical domain models matching Datalogics APDFL taxonomy.
pub use annotation::{
    Action, Annotation, AnnotationFlags, AnnotationSubtype, CircleAnnotation, FreeTextAnnotation, GoToAction, HighlightAnnotation, InkAnnotation,
    LaunchAction, LineAnnotation, LineEnding, LinkAnnotation, PolyLineAnnotation, PolygonAnnotation, Redaction, RedactionStatus, RemoteGoToAction,
    SquareAnnotation, URIAction, UnderlineAnnotation,
};
pub use content::{Clip, Container, Content, Element, Form, Group, Path, Segment};
pub use document::{Bookmark, Document, FitMode, Page, PageLabel, PageLabelStyle, PageRange, ViewDestination};
pub use forms::{AcroFormExportType, AcroFormImportType, ButtonField, ChoiceField, Field, FieldType, SignatureField, TextField};
pub use graphics::{BlendMode, Color, ColorSpace, ExtendedGraphicState, GraphicState, Matrix, Point, Quad, Rect};
pub use image::{DrawParams, ImageData, ImageSaveParams, PageImageParams};
pub use lowlevel::{NameTree, NumberTree, PDFObject, PDFReference};
pub use metadata::{FileAttachment, FileSpecification, WatermarkParams};
pub use optimization::{CompressionType, FlattenTransparencyParams, OptimizationParams, PDFOptimizer};
pub use security::{DigitalSignature, EncryptionType, PermissionsFlags, SignDoc};
pub use text::{DocTextFinderConfig, DocTextFinderMatch, Font, Style, StyleTransition, Text, TextRun, TextState, Word, WordFinderConfig};

/// Unified document processing client trait shared across in-process and remote transports.
pub trait PdfCraftClient {
    /// Merges multiple PDF files into a single output PDF.
    fn merge(&self, files: &[DocumentSource], options: MergeOptions) -> Result<DocumentResult, SdkError>;

    /// Splits a PDF file into multiple PDF documents based on the specified split mode.
    fn split(&self, file: &DocumentSource, mode: SplitMode) -> Result<SplitResult, SdkError>;

    /// Renders a single page of a PDF document to an image at the specified physical DPI.
    fn render_page(&self, file: &DocumentSource, page: usize, options: RenderOptions) -> Result<DocumentResult, SdkError>;
}
