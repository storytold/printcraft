//! optimization.rs — PDF Optimization domain models matching Datalogics APDFL architecture.
//! =========================================================================================
//!
//! Purpose:
//!     Defines PDFOptimizer, OptimizationParams, FlattenTransparencyParams, and
//!     CompressionType matching Datalogics APDFL PDF Optimization specifications.
//!
//! Layer:
//!     Domain Layer / PDF Optimization
//!
//! Key Types:
//!     - CompressionType: Stream compression types (Flate, DCT, JBIG2, None)
//!     - FlattenTransparencyParams: Parameters for flattening transparent artwork
//!     - OptimizationParams: Complete document compression and pruning options
//!     - PDFOptimizer: High-level optimizer task specification

use serde::{Deserialize, Serialize};

/// PDF Stream compression algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CompressionType {
    #[default]
    Flate,
    DCT,
    JBIG2,
    None,
}

/// Parameters for flattening transparency into opaque vector and raster elements.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlattenTransparencyParams {
    pub line_art_and_text_resolution_dpi: f64,
    pub raster_vector_balance: f64,
    pub clip_complex_regions: bool,
}

impl Default for FlattenTransparencyParams {
    fn default() -> Self {
        Self { line_art_and_text_resolution_dpi: 300.0, raster_vector_balance: 100.0, clip_complex_regions: true }
    }
}

/// Comprehensive optimization and compression parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptimizationParams {
    pub compress_streams: bool,
    pub remove_unreferenced_objects: bool,
    pub downsample_images: bool,
    pub target_image_dpi: Option<f64>,
    pub linearize: bool,
    pub flatten_transparency: Option<FlattenTransparencyParams>,
}

impl Default for OptimizationParams {
    fn default() -> Self {
        Self {
            compress_streams: true,
            remove_unreferenced_objects: true,
            downsample_images: false,
            target_image_dpi: None,
            linearize: false,
            flatten_transparency: None,
        }
    }
}

/// High-level optimizer definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PDFOptimizer {
    pub params: OptimizationParams,
}

impl PDFOptimizer {
    pub fn new(params: OptimizationParams) -> Self {
        Self { params }
    }
}
