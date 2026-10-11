//! document.rs — Document Layer domain models matching Datalogics APDFL architecture.
//! =====================================================================================
//!
//! Purpose:
//!     Encapsulates Document, Page, PageRange, PageLabel, Bookmark, and ViewDestination
//!     abstractions matching Datalogics APDFL Document Layer specifications.
//!
//! Layer:
//!     Domain Layer / Document
//!
//! Key Types:
//!     - Document: Document metadata, page count, and structural properties
//!     - Page: Physical page box geometries and orientation
//!     - Bookmark: Interactive outline tree node with title and destination/action
//!     - ViewDestination: Target view zoom and coordinates for bookmarks/links
//!     - FitMode: Destination zoom fit styles (XYZ, Fit, FitH, FitV, etc.)
//!     - PageRange: Numerical subsets of pages
//!     - PageLabel: Visual page numbering labeling schemes

use crate::graphics::Rect;
use serde::{Deserialize, Serialize};

/// Fit modes for document view destinations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FitMode {
    #[default]
    XYZ,
    Fit,
    FitH,
    FitV,
    FitR,
    FitB,
    FitBH,
    FitBV,
}

/// A target destination within a PDF document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ViewDestination {
    pub page_number: usize,
    pub fit_mode: FitMode,
    pub zoom: Option<f64>,
    pub rect: Option<Rect>,
}

impl ViewDestination {
    pub fn new(page_number: usize, fit_mode: FitMode) -> Self {
        Self { page_number, fit_mode, zoom: None, rect: None }
    }

    pub fn xyz(page_number: usize, zoom: Option<f64>) -> Self {
        Self { page_number, fit_mode: FitMode::XYZ, zoom, rect: None }
    }
}

/// Visual outline bookmark node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    pub title: String,
    pub destination: Option<ViewDestination>,
    pub children: Vec<Bookmark>,
    pub is_open: bool,
    pub bold: bool,
    pub italic: bool,
}

impl Bookmark {
    pub fn new(title: impl Into<String>) -> Self {
        Self { title: title.into(), destination: None, children: Vec::new(), is_open: true, bold: false, italic: false }
    }

    pub fn with_destination(mut self, dest: ViewDestination) -> Self {
        self.destination = Some(dest);
        self
    }

    pub fn add_child(&mut self, child: Bookmark) {
        self.children.push(child);
    }
}

/// Page label styles for visual page numbering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PageLabelStyle {
    #[default]
    Decimal,
    UpperRoman,
    LowerRoman,
    UpperAlpha,
    LowerAlpha,
    None,
}

/// Page numbering labeling scheme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PageLabel {
    pub prefix: String,
    pub style: PageLabelStyle,
    pub start_index: usize,
}

/// Page range specification for batch/subset operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRange {
    pub start_page: usize,
    pub end_page: usize,
    pub even_only: bool,
    pub odd_only: bool,
}

impl PageRange {
    pub fn all(num_pages: usize) -> Self {
        Self { start_page: 1, end_page: num_pages, even_only: false, odd_only: false }
    }

    pub fn range(start_page: usize, end_page: usize) -> Self {
        Self { start_page, end_page, even_only: false, odd_only: false }
    }
}

/// Represents a single page within a PDF document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub page_number: usize,
    pub media_box: Rect,
    pub crop_box: Rect,
    pub rotation_degrees: u16,
}

impl Page {
    pub fn new(page_number: usize, media_box: Rect) -> Self {
        Self { page_number, crop_box: media_box, media_box, rotation_degrees: 0 }
    }

    pub fn width(&self) -> f64 {
        self.crop_box.width
    }

    pub fn height(&self) -> f64 {
        self.crop_box.height
    }
}

/// Document Layer root model representing an opened or newly created PDF.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub num_pages: usize,
    pub version: String,
    pub is_encrypted: bool,
    pub linearized: bool,
    pub bookmarks: Vec<Bookmark>,
}

impl Document {
    pub fn new(num_pages: usize) -> Self {
        Self { num_pages, version: "1.7".into(), is_encrypted: false, linearized: false, bookmarks: Vec::new() }
    }
}
