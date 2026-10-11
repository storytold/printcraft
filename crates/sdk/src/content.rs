//! content.rs — Content Elements domain models matching Datalogics APDFL architecture.
//! =======================================================================================
//!
//! Purpose:
//!     Defines Content, Element, Container, Group, Clip, Form, Path, and Segment
//!     abstractions matching Datalogics APDFL Content Elements hierarchy.
//!
//! Layer:
//!     Domain Layer / Content Elements
//!
//! Key Types:
//!     - Segment: Vector path segments (MoveTo, LineTo, CurveTo, ClosePath, RectSegment)
//!     - Path: Vector geometry path with stroke and fill flags
//!     - Clip: Clipping boundary enclosing elements
//!     - Container / Group: Structured grouping of graphic elements
//!     - Element: Polymorphic graphic element enum
//!     - Content: High-level page content stream representation

use crate::graphics::{Matrix, Point, Rect};
use serde::{Deserialize, Serialize};

/// Path drawing and segment operators.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Segment {
    MoveTo(Point),
    LineTo(Point),
    CurveTo { control_point1: Point, control_point2: Point, endpoint: Point },
    CurveToV { control_point2: Point, endpoint: Point },
    CurveToY { control_point1: Point, endpoint: Point },
    ClosePath,
    RectSegment(Rect),
}

/// A vector graphics path containing segments and rendering flags.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Path {
    pub segments: Vec<Segment>,
    pub fill: bool,
    pub stroke: bool,
    pub even_odd: bool,
}

impl Path {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn move_to(&mut self, p: Point) {
        self.segments.push(Segment::MoveTo(p));
    }

    pub fn line_to(&mut self, p: Point) {
        self.segments.push(Segment::LineTo(p));
    }

    pub fn curve_to(&mut self, cp1: Point, cp2: Point, endpoint: Point) {
        self.segments.push(Segment::CurveTo { control_point1: cp1, control_point2: cp2, endpoint });
    }

    pub fn close(&mut self) {
        self.segments.push(Segment::ClosePath);
    }
}

/// A clip element restricting the visible drawing region for its child elements.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub path: Path,
    pub elements: Vec<Element>,
}

/// A container or layer element grouping children elements.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Container {
    pub elements: Vec<Element>,
}

/// A named group of content elements.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub name: Option<String>,
    pub elements: Vec<Element>,
}

/// A reusable Form XObject element with its own coordinate matrix and bounding box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form {
    pub matrix: Matrix,
    pub bbox: Rect,
    pub content: Box<Content>,
}

/// High-level element representation within page content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Element {
    Path(Path),
    Clip(Clip),
    Container(Container),
    Group(Group),
    Form(Form),
}

/// High-level representation of a page or XObject content stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Content {
    pub elements: Vec<Element>,
}

impl Content {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_element(&mut self, element: Element) {
        self.elements.push(element);
    }
}
