//! graphics.rs — Geometric, color, and graphics state models matching Datalogics APDFL.
//! =========================================================================================
//!
//! Purpose:
//!     Defines 2D affine transforms, points, rectangles, quads, color representations,
//!     and graphics state structures conforming to ISO 32000-2 / APDFL conventions.
//!
//! Layer:
//!     Domain Layer / Graphics & Geometry
//!
//! Key Types:
//!     - Point: 2D coordinate (x, y)
//!     - Rect: Bounding rectangle [x, y, width, height]
//!     - Quad: Four vertices defining a polygon/text highlight region
//!     - Matrix: 3x2 Affine transform matrix [a, b, c, d, tx, ty]
//!     - Color: RGB, CMYK, Gray, or Named color value
//!     - ColorSpace: Standard PDF color spaces
//!     - BlendMode: Standard blend modes for transparency rendering
//!     - GraphicState: Drawing parameters for path stroke/fill
//!     - ExtendedGraphicState: Transparency and soft-mask parameters

use serde::{Deserialize, Serialize};

/// 2D geometric coordinate point in PDF default user space (points, 1/72 inch).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// Bounding rectangle in PDF user space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { x, y, width, height }
    }

    pub fn left(&self) -> f64 {
        self.x
    }

    pub fn bottom(&self) -> f64 {
        self.y
    }

    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    pub fn top(&self) -> f64 {
        self.y + self.height
    }

    pub fn to_array(&self) -> [f64; 4] {
        [self.x, self.y, self.x + self.width, self.y + self.height]
    }
}

/// Quadrilateral defined by 4 vertices, used for oriented text highlights and quads.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Quad {
    pub top_left: Point,
    pub top_right: Point,
    pub bottom_left: Point,
    pub bottom_right: Point,
}

impl Quad {
    pub fn new(top_left: Point, top_right: Point, bottom_left: Point, bottom_right: Point) -> Self {
        Self { top_left, top_right, bottom_left, bottom_right }
    }

    pub fn from_rect(rect: &Rect) -> Self {
        Self {
            top_left: Point::new(rect.x, rect.y + rect.height),
            top_right: Point::new(rect.x + rect.width, rect.y + rect.height),
            bottom_left: Point::new(rect.x, rect.y),
            bottom_right: Point::new(rect.x + rect.width, rect.y),
        }
    }
}

/// 3x2 Affine transform matrix `[a, b, c, d, tx, ty]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub tx: f64,
    pub ty: f64,
}

impl Default for Matrix {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Matrix {
    pub const IDENTITY: Self = Self { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 };

    pub fn new(a: f64, b: f64, c: f64, d: f64, tx: f64, ty: f64) -> Self {
        Self { a, b, c, d, tx, ty }
    }

    pub fn translation(tx: f64, ty: f64) -> Self {
        Self { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx, ty }
    }

    pub fn scale(sx: f64, sy: f64) -> Self {
        Self { a: sx, b: 0.0, c: 0.0, d: sy, tx: 0.0, ty: 0.0 }
    }

    pub fn multiply(&self, rhs: &Self) -> Self {
        Self {
            a: self.a * rhs.a + self.b * rhs.c,
            b: self.a * rhs.b + self.b * rhs.d,
            c: self.c * rhs.a + self.d * rhs.c,
            d: self.c * rhs.b + self.d * rhs.d,
            tx: self.tx * rhs.a + self.ty * rhs.c + rhs.tx,
            ty: self.tx * rhs.b + self.ty * rhs.d + rhs.ty,
        }
    }

    pub fn transform_point(&self, p: Point) -> Point {
        Point { x: p.x * self.a + p.y * self.c + self.tx, y: p.x * self.b + p.y * self.d + self.ty }
    }
}

/// Standard color representation matching PDF color models.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Color {
    RGB(f64, f64, f64),
    CMYK(f64, f64, f64, f64),
    Gray(f64),
    Named { name: String, components: Vec<f64> },
}

impl Default for Color {
    fn default() -> Self {
        Self::RGB(0.0, 0.0, 0.0)
    }
}

impl Color {
    pub const BLACK: Self = Self::RGB(0.0, 0.0, 0.0);
    pub const WHITE: Self = Self::RGB(1.0, 1.0, 1.0);
    pub const RED: Self = Self::RGB(1.0, 0.0, 0.0);
    pub const GREEN: Self = Self::RGB(0.0, 1.0, 0.0);
    pub const BLUE: Self = Self::RGB(0.0, 0.0, 1.0);
}

/// Standard PDF color spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ColorSpace {
    #[default]
    DeviceRGB,
    DeviceCMYK,
    DeviceGray,
    CalGray,
    CalRGB,
    Lab,
    ICCBased,
    Separation,
    Indexed,
}

/// Standard PDF Blend Modes for transparency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
}

/// Current graphics state parameters for rendering and drawing elements.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphicState {
    pub stroke_color: Color,
    pub fill_color: Color,
    pub line_width: f64,
    pub matrix: Matrix,
    pub blend_mode: BlendMode,
}

impl Default for GraphicState {
    fn default() -> Self {
        Self { stroke_color: Color::BLACK, fill_color: Color::BLACK, line_width: 1.0, matrix: Matrix::IDENTITY, blend_mode: BlendMode::Normal }
    }
}

/// Extended graphics state dictionary parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtendedGraphicState {
    pub alpha_stroke: f64,
    pub alpha_fill: f64,
    pub blend_mode: BlendMode,
}

impl Default for ExtendedGraphicState {
    fn default() -> Self {
        Self { alpha_stroke: 1.0, alpha_fill: 1.0, blend_mode: BlendMode::Normal }
    }
}
