//! annotation.rs — Annotations & Actions domain models matching Datalogics APDFL architecture.
//! =============================================================================================
//!
//! Purpose:
//!     Defines interactive PDF annotations and action types (GoToAction, URIAction,
//!     HighlightAnnotation, FreeTextAnnotation, InkAnnotation, Redaction, etc.) matching
//!     Datalogics APDFL Annotations & Actions hierarchy.
//!
//! Layer:
//!     Domain Layer / Annotations & Actions
//!
//! Key Types:
//!     - Action: Polymorphic base action enum
//!     - GoToAction: Hyperlink jumping to a view destination in the same document
//!     - URIAction: Web hyperlink action
//!     - LaunchAction: Application or external document launch action
//!     - RemoteGoToAction: Navigation to a destination in an external PDF
//!     - Annotation: Base annotation model with common properties
//!     - HighlightAnnotation / UnderlineAnnotation: Text markup annotations
//!     - FreeTextAnnotation: Embedded text box annotation
//!     - LineAnnotation / CircleAnnotation / SquareAnnotation: Shape annotations
//!     - InkAnnotation: Freehand drawing strokes
//!     - LinkAnnotation: Interactive link zone
//!     - Redaction: Legal redaction box with overlay text and quads

use crate::document::ViewDestination;
use crate::graphics::{Color, Point, Quad, Rect};
use serde::{Deserialize, Serialize};

/// Polymorphic Action types that can be triggered by bookmarks, links, or document events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// A go-to action jump to a view destination within the same document.
    GoTo(GoToAction),
    /// A web URI hyperlink action.
    URI(URIAction),
    /// A launch action targeting an external file or application.
    Launch(LaunchAction),
    /// A remote go-to action navigating to a destination in an external PDF document.
    RemoteGoTo(RemoteGoToAction),
}

/// Jumps to a page and view inside the same document (PDF `/S /GoTo`), typically attached to a
/// bookmark or link annotation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoToAction {
    pub destination: ViewDestination,
}

impl GoToAction {
    pub fn new(destination: ViewDestination) -> Self {
        Self { destination }
    }
}

/// A URI action causes a web link to be opened in a browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct URIAction {
    pub uri: String,
    pub is_map: bool,
}

impl URIAction {
    pub fn new(uri: impl Into<String>) -> Self {
        Self { uri: uri.into(), is_map: false }
    }
}

/// Opens an external file or application (PDF `/S /Launch`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchAction {
    pub file_path: String,
    pub new_window: bool,
}

impl LaunchAction {
    pub fn new(file_path: impl Into<String>) -> Self {
        Self { file_path: file_path.into(), new_window: true }
    }
}

/// A remote go-to action jumps to a destination in a separate PDF document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteGoToAction {
    pub file_path: String,
    pub destination: ViewDestination,
    pub new_window: bool,
}

impl RemoteGoToAction {
    pub fn new(file_path: impl Into<String>, destination: ViewDestination) -> Self {
        Self { file_path: file_path.into(), destination, new_window: true }
    }
}

/// Line ending decorative styles for line and polyline annotations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LineEnding {
    #[default]
    None,
    Square,
    Circle,
    Diamond,
    OpenArrow,
    ClosedArrow,
    Butt,
    ROpenArrow,
    RClosedArrow,
    Slash,
}

/// High-level text markup highlight annotation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HighlightAnnotation {
    pub quads: Vec<Quad>,
    pub color: Color,
}

/// Underline text markup annotation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnderlineAnnotation {
    pub quads: Vec<Quad>,
    pub color: Color,
}

/// FreeText annotation for placing text boxes directly onto the page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FreeTextAnnotation {
    pub text: String,
    pub font_name: String,
    pub font_size: f64,
    pub text_color: Color,
}

/// Line annotation connecting two points on a page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineAnnotation {
    pub start_point: Point,
    pub end_point: Point,
    pub start_ending: LineEnding,
    pub end_ending: LineEnding,
    pub line_width: f64,
}

/// Circle (ellipse) shape annotation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CircleAnnotation {
    pub fill_color: Option<Color>,
    pub border_width: f64,
}

/// Square (rectangle) shape annotation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SquareAnnotation {
    pub fill_color: Option<Color>,
    pub border_width: f64,
}

/// Polygon annotation enclosed by multiple vertices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolygonAnnotation {
    pub vertices: Vec<Point>,
    pub fill_color: Option<Color>,
}

/// PolyLine annotation composed of connected line segments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolyLineAnnotation {
    pub vertices: Vec<Point>,
    pub line_width: f64,
}

/// Freehand ink drawing annotation composed of pen stroke gesture paths.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InkAnnotation {
    pub ink_list: Vec<Vec<Point>>,
    pub stroke_width: f64,
}

/// Interactive link annotation overlaying an area of a page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkAnnotation {
    pub action: Option<Action>,
    pub destination: Option<ViewDestination>,
}

/// Lifecycle state for redactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RedactionStatus {
    #[default]
    Marked,
    Applied,
}

/// Redaction annotation designating sensitive page areas for permanent removal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Redaction {
    pub quads: Vec<Quad>,
    pub overlay_text: Option<String>,
    pub fill_color: Color,
    pub status: RedactionStatus,
}

/// Annotation Subtype specific payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AnnotationSubtype {
    Highlight(HighlightAnnotation),
    Underline(UnderlineAnnotation),
    FreeText(FreeTextAnnotation),
    Line(LineAnnotation),
    Circle(CircleAnnotation),
    Square(SquareAnnotation),
    Polygon(PolygonAnnotation),
    PolyLine(PolyLineAnnotation),
    Ink(InkAnnotation),
    Link(LinkAnnotation),
    Redaction(Redaction),
}

/// Standard PDF Annotation flags (F bitmask).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AnnotationFlags {
    pub invisible: bool,
    pub hidden: bool,
    pub print: bool,
    pub no_zoom: bool,
    pub no_rotate: bool,
    pub no_view: bool,
    pub read_only: bool,
    pub locked: bool,
}

/// Base annotation model containing common geometry, appearance, and subtype data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub rect: Rect,
    pub contents: Option<String>,
    pub author: Option<String>,
    pub color: Option<Color>,
    pub opacity: f64,
    pub flags: AnnotationFlags,
    pub action: Option<Action>,
    pub subtype: AnnotationSubtype,
}

impl Annotation {
    pub fn new(rect: Rect, subtype: AnnotationSubtype) -> Self {
        Self {
            rect,
            contents: None,
            author: None,
            color: None,
            opacity: 1.0,
            flags: AnnotationFlags { print: true, ..Default::default() },
            action: None,
            subtype,
        }
    }
}
