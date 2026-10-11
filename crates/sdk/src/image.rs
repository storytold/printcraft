//! image.rs — Image Processing domain models matching Datalogics APDFL architecture.
//! ===================================================================================
//!
//! Purpose:
//!     Defines Image, ImageData, DrawParams, PageImageParams, and ImageSaveParams
//!     matching Datalogics APDFL Image Processing & Rasterization specifications.
//!
//! Layer:
//!     Domain Layer / Image Processing
//!
//! Key Types:
//!     - ImageFormat: Target encoding format (PNG, JPEG, WebP, TIFF)
//!     - DrawParams: Rendering options including resolution (DPI) and clip bounds
//!     - PageImageParams: Parameters governing raster image generation
//!     - ImageSaveParams: Compression and quality settings for saving images
//!     - ImageData: Raw byte container for pixel data
//!     - Image: Image resource metadata and content

use crate::graphics::{ColorSpace, Rect};
use serde::{Deserialize, Serialize};

/// Image raster file format encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
    Webp,
    Tiff,
}

impl ImageFormat {
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
            Self::Tiff => "image/tiff",
        }
    }
}

/// Rendering parameters for drawing a PDF page to a raster canvas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrawParams {
    pub dpi: f64,
    pub color_space: ColorSpace,
    pub clip_box: Option<Rect>,
    pub smooth_text: bool,
    pub smooth_images: bool,
}

impl Default for DrawParams {
    fn default() -> Self {
        Self { dpi: 150.0, color_space: ColorSpace::DeviceRGB, clip_box: None, smooth_text: true, smooth_images: true }
    }
}

/// Page-to-image raster conversion options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageImageParams {
    pub dpi: f64,
    pub image_format: ImageFormat,
    pub clip_box: Option<Rect>,
}

impl Default for PageImageParams {
    fn default() -> Self {
        Self { dpi: 150.0, image_format: ImageFormat::Png, clip_box: None }
    }
}

/// Save parameters for encoding raster image data to disk or memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageSaveParams {
    pub format: ImageFormat,
    pub quality: u8,
    pub compress: bool,
}

impl Default for ImageSaveParams {
    fn default() -> Self {
        Self { format: ImageFormat::Png, quality: 90, compress: true }
    }
}

/// Raw container for pixel or encoded image byte stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageData {
    pub data: Vec<u8>,
}

/// Image resource element within PDF content or extracted from a page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub bits_per_component: usize,
    pub color_space: ColorSpace,
    pub format: ImageFormat,
    pub data: Vec<u8>,
}

impl Image {
    pub fn new(width: usize, height: usize, format: ImageFormat, data: Vec<u8>) -> Self {
        Self { width, height, bits_per_component: 8, color_space: ColorSpace::DeviceRGB, format, data }
    }
}
