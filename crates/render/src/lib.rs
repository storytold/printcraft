//! pdfcraft-render — page rasterization and document inspection.
//!
//! **Bootstrap status (see ADR-0004):** rasterization goes straight through the `hayro` crate
//! (hayro-interpret + vello_cpu). Inspection uses the lazy `cos` reader with a read-only lopdf
//! object adapter and retains `lopdf` loading for compatibility repairs. The renderer and that
//! adapter are replaced by `model` and the DisplayList design in M1–M2. The public API is what the engine and UI rely on, so
//! the swap stays internal to this crate.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod inspect;
mod pixels;
mod raster;
mod structure;
pub mod text;

pub use inspect::{
    Annotation, Attachment, AttachmentSource, DestView, DocInfo, Field, FieldKind, FontInfo, Layer, LayerOp, Link, LinkTarget, OutlineItem, PageInfo,
    Xfa, attachment_data, inspect, pretty_date,
};
pub use pixels::Pixels;
pub use raster::{
    MAX_PIXELS, MAX_SIDE, PageRenderer, RenderConfig, RenderPool, RenderRequest, RenderStats, RenderWarning, RenderedPage, RequestKind, Retiring,
    Tile, device_pixels, effective_scale,
};
pub use text::{PageText, TextGlyph};

/// Errors surfaced to the user when a document cannot be opened.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("the file is not a readable PDF: {0}")]
    Invalid(String),
    #[error("the document is protected by a password")]
    NeedsPassword,
    #[error("the password is incorrect")]
    WrongPassword,
    #[error("{0}")]
    Unsupported(String),
}
