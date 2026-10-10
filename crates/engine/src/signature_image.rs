//! Local image signatures, shared by Fill & Sign and its automation tool.

use std::io::{Cursor, Read};
use std::sync::Arc;

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};
use pdfcraft_render::PageInfo;

use crate::{Document, Edit, MarkFile, guard};

/// Read-only layers for interactive movement: the page without this signature, and its image.
/// Preparing them never changes the working bytes, dirty flag or undo history.
pub struct ImageSignaturePreview {
    pub image: SignatureImage,
    pub opacity: f32,
    /// The page `/Rotate` the picture is drawn turned back by (see [`pdfcraft_annot::picture_rotation`]).
    pub turn: i64,
    page: usize,
    bytes: Arc<Vec<u8>>,
    config: pdfcraft_render::RenderConfig,
}

impl ImageSignaturePreview {
    /// One background worker; the UI reuses its page texture throughout a gesture.
    pub fn renderer(&self) -> pdfcraft_render::RenderPool {
        pdfcraft_render::RenderPool::new(self.bytes.clone(), 1, self.config.clone())
    }

    /// Synchronous counterpart for automation, with the renderer's normal pixel limits.
    pub fn render_background(&self, scale: f32) -> Result<pdfcraft_render::RenderedPage, String> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err("preview scale must be finite and positive".into());
        }
        guard(|| {
            let mut renderer = pdfcraft_render::PageRenderer::new(self.bytes.clone(), self.config.clone());
            let out = renderer.render(pdfcraft_render::RenderRequest { page: self.page, scale, ..Default::default() });
            if let Some(e) = &out.error {
                return Err(e.clone());
            }
            Ok(out)
        })?
    }
}

impl Document {
    /// Prepare the actual embedded signature, including alpha, and an independent background.
    /// A cheap COS clone excludes only this annotation; other page content remains visible.
    pub fn image_signature_preview(&self, page: usize, index: usize) -> Result<Option<ImageSignaturePreview>, String> {
        guard(|| {
            let editor = self.editor.as_ref().ok_or("the document can't be read")?;
            let Some(picture) = pdfcraft_annot::signature_image(&editor.cos, page, index).map_err(|e| e.to_string())? else { return Ok(None) };
            let obj = editor.cos.get(picture);
            let pdfcraft_cos::Object::Stream(s) = &*obj else { return Err("the signature image is damaged".into()) };
            let (w, h) = (s.dict.int(b"Width").unwrap_or(0), s.dict.int(b"Height").unwrap_or(0));
            if w <= 0 || h <= 0 || w > 4096 || h > 4096 || w.saturating_mul(h) > MAX_PIXELS as i64 {
                return Err(SignatureImageError::Size.to_string());
            }
            let (_, bytes) = pdfcraft_create::image_file(&editor.cos, picture)?;
            let image = SignatureImage::from_bytes(&bytes).map_err(|e| e.to_string())?;
            let opacity =
                pdfcraft_annot::props(&editor.cos, page, index).map(|p| p.opacity).filter(|v| v.is_finite()).unwrap_or(1.0).clamp(0.0, 1.0) as f32;
            let turn = pdfcraft_annot::picture_rotation(&editor.cos, page, index).map_err(|e| e.to_string())?;
            let mut cos = editor.cos.clone();
            pdfcraft_annot::delete_annotation(&mut cos, page, index).map_err(|e| e.to_string())?;
            let bytes = Arc::new(pdfcraft_cos::write_incremental(&cos, &pdfcraft_cos::SaveOptions::default()).map_err(|e| e.to_string())?);
            Ok(Some(ImageSignaturePreview { image, opacity, turn, page, bytes, config: self.config.clone() }))
        })?
    }
}

/// The user-space box `width` x `height` points as displayed on a page turned by `rotation` (its
/// `/Rotate`), with its left edge at `at` (a user-space point) and vertically centred on it. On a
/// quarter-turned page it is `height` wide and `width` tall in user space. `None` for a non-finite
/// point.
pub fn upright_box(rotation: i64, at: [f64; 2], width: f64, height: f64) -> Option<[f64; 4]> {
    if !at.iter().chain(&[width, height]).all(|v| v.is_finite()) {
        return None;
    }
    // Right and up as displayed, as user-space vectors: the page's view matrix without its offset.
    let [a, b, c, d, ..] = pdfcraft_model::view_matrix_for(rotation, [0.0; 4]);
    let (right, up) = ([a * width, b * width], [c * height / 2.0, d * height / 2.0]);
    Some([
        at[0] + right[0].min(0.0) - up[0].abs(),
        at[1] + right[1].min(0.0) - up[1].abs(),
        at[0] + right[0].max(0.0) + up[0].abs(),
        at[1] + right[1].max(0.0) + up[1].abs(),
    ])
}

/// Saved signatures live in settings, so both source and normalized PNG are bounded.
pub const MAX_SIGNATURE_IMAGE_BYTES: usize = 4 << 20;
const MAX_PIXELS: u64 = 4 << 20;

#[derive(Debug, thiserror::Error)]
pub enum SignatureImageError {
    #[error("Choose a PNG or JPEG image.")]
    Format,
    #[error("Choose an image smaller than 4 MiB and 4 megapixels (at most 4096 pixels per side).")]
    Size,
    #[error("Couldn't read the signature image: {0}")]
    Read(#[from] std::io::Error),
    #[error("Couldn't decode the signature image: {0}")]
    Decode(#[from] image::ImageError),
}

/// A validated image, normalized to PNG without source paths or metadata. Alpha is preserved.
#[derive(Clone, Debug, PartialEq)]
pub struct SignatureImage {
    bytes: Arc<Vec<u8>>,
    rgba: Arc<Vec<u8>>,
    size: [usize; 2],
}

impl SignatureImage {
    /// Read at most the file limit plus one byte, even if the file grows while being read.
    pub fn read(reader: impl Read) -> Result<Self, SignatureImageError> {
        let mut bytes = Vec::new();
        reader.take(MAX_SIGNATURE_IMAGE_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SignatureImageError> {
        if bytes.len() > MAX_SIGNATURE_IMAGE_BYTES {
            return Err(SignatureImageError::Size);
        }
        let format = image::guess_format(bytes).map_err(|_| SignatureImageError::Format)?;
        if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
            return Err(SignatureImageError::Format);
        }
        let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
        let mut limits = Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(32 << 20);
        reader.limits(limits);
        let mut decoder = reader.into_decoder()?;
        let (w, h) = decoder.dimensions();
        if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
            return Err(SignatureImageError::Size);
        }
        let orientation = decoder.orientation()?;
        let mut image = DynamicImage::from_decoder(decoder)?;
        image.apply_orientation(orientation);
        let rgba = image.into_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(rgba.clone()).write_to(&mut png, ImageFormat::Png)?;
        let bytes = png.into_inner();
        if bytes.len() > MAX_SIGNATURE_IMAGE_BYTES {
            return Err(SignatureImageError::Size);
        }
        Ok(Self { bytes: Arc::new(bytes), rgba: Arc::new(rgba.into_raw()), size })
    }

    pub fn bytes(&self) -> &Arc<Vec<u8>> {
        &self.bytes
    }

    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    pub fn size(&self) -> [usize; 2] {
        self.size
    }

    /// This image as the graphic of a visible digital signature.
    pub fn graphic(&self) -> pdfcraft_sign::Graphic {
        // Sides are at most 4096 pixels; 0 would be refused by the signer, never drawn.
        let side = |v: usize| u32::try_from(v).unwrap_or(0);
        pdfcraft_sign::Graphic::Image { width: side(self.size[0]), height: side(self.size[1]), rgba: self.rgba.clone() }
    }

    /// Make near-white paper transparent, keeping darker ink.
    pub fn remove_white_background(&self, cutoff: u8, feather: u8) -> Result<Self, SignatureImageError> {
        let [w, h] = self.size;
        let mut rgba = image::RgbaImage::from_raw(w as u32, h as u32, self.rgba.as_ref().clone()).ok_or(SignatureImageError::Size)?;
        let high = f32::from(cutoff);
        let span = f32::from(feather.max(1));
        for pixel in rgba.pixels_mut() {
            let [r, g, b, a] = pixel.0;
            let whiteness = f32::from(r.min(g).min(b));
            let keep = ((high - whiteness) / span).clamp(0.0, 1.0);
            pixel.0[3] = (f32::from(a) * keep).round() as u8;
        }
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(rgba).write_to(&mut png, ImageFormat::Png)?;
        Self::from_bytes(&png.into_inner())
    }

    /// The user-space box for the image with its left edge at `at` (a user-space point on `page`),
    /// vertically centered, with the same 150 pt width cap as typed names. Left and centered are
    /// as displayed: on a page with `/Rotate` the box runs along the turned page's axes.
    pub fn rect(&self, page: &PageInfo, at: [f64; 2], initials: bool) -> Option<[f64; 4]> {
        if !at.iter().all(|v| v.is_finite()) {
            return None;
        }
        let [w, h] = self.size;
        let scale = (150.0 / w as f64).min(if initials { 24.0 } else { 32.0 } / h as f64);
        upright_box(i64::from(page.rotation), at, w as f64 * scale, h as f64 * scale)
    }

    pub fn edit(&self, page: usize, info: &PageInfo, at: [f64; 2], initials: bool, author: &str) -> Option<Edit> {
        let rect = self.rect(info, at, initials)?;
        let label = if initials { "Add initials" } else { "Add signature" };
        Some(Edit::Batch {
            label: label.into(),
            edits: vec![Edit::AddCustomStamp {
                page,
                rect,
                name: if initials { "Initials" } else { "Signature" }.into(),
                file: MarkFile { name: "signature.png".into(), bytes: self.bytes.clone(), page: 0 },
                author: author.into(),
            }],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 300 x 400 page shown turned `rotation` degrees clockwise.
    fn page(rotation: u16) -> PageInfo {
        let (width, height) = if rotation.is_multiple_of(180) { (300.0, 400.0) } else { (400.0, 300.0) };
        PageInfo { width, height, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation }
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(w, h, image::Rgba([40, 60, 80, 128]));
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn image_signatures_preserve_alpha_and_fit_the_signature_box() {
        let image = SignatureImage::from_bytes(&png(400, 40)).unwrap();
        assert_eq!(image.size(), [400, 40]);
        assert_eq!(&image.rgba()[..4], &[40, 60, 80, 128]);
        let Edit::Batch { edits, .. } = image.edit(0, &page(0), [20.0, 100.0], false, "Ada").unwrap() else { panic!() };
        let Edit::AddCustomStamp { rect, .. } = &edits[0] else { panic!() };
        assert_eq!(*rect, [20.0, 92.5, 170.0, 107.5]);
        assert!(image.edit(0, &page(0), [f64::NAN, 0.0], false, "").is_none());
        assert_eq!(SignatureImage::from_bytes(image.bytes()).unwrap(), image);
    }

    #[test]
    fn invalid_and_oversized_signature_images_are_rejected() {
        assert!(SignatureImage::from_bytes(b"not an image").is_err());
        assert!(SignatureImage::from_bytes(b"\x89PNG\r\n\x1a\ntruncated").is_err());
        assert!(SignatureImage::from_bytes(&vec![0; MAX_SIGNATURE_IMAGE_BYTES + 1]).is_err());
        assert!(SignatureImage::from_bytes(&png(4097, 1)).is_err());
        assert!(SignatureImage::from_bytes(&png(4096, 1025)).is_err());
        assert!(SignatureImage::read(std::io::repeat(0)).is_err());
        assert!(matches!(SignatureImage::from_bytes(b"GIF89a\x02\0\x02\0"), Err(SignatureImageError::Format)));
    }

    #[test]
    fn jpeg_signatures_are_normalized_to_png() {
        let mut jpeg = Cursor::new(Vec::new());
        image::RgbImage::from_pixel(60, 20, image::Rgb([20, 20, 20])).write_to(&mut jpeg, ImageFormat::Jpeg).unwrap();
        let image = SignatureImage::from_bytes(&jpeg.into_inner()).unwrap();
        assert_eq!(image.size(), [60, 20]);
        assert!(image.bytes().starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(image.rgba().as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    }

    #[test]
    fn jpeg_signature_orientation_is_applied_before_preview_and_embedding() {
        let mut jpeg = Cursor::new(Vec::new());
        image::RgbImage::from_pixel(60, 20, image::Rgb([20, 20, 20])).write_to(&mut jpeg, ImageFormat::Jpeg).unwrap();
        let jpeg = jpeg.into_inner();
        // Synthetic EXIF IFD with Orientation=6 (90 degrees clockwise).
        let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
        let mut rotated = jpeg[..2].to_vec();
        rotated.extend_from_slice(&[0xff, 0xe1]);
        rotated.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        rotated.extend_from_slice(exif);
        rotated.extend_from_slice(&jpeg[2..]);
        let image = SignatureImage::from_bytes(&rotated).unwrap();
        assert_eq!(image.size(), [20, 60]);
        assert_eq!(SignatureImage::from_bytes(image.bytes()).unwrap().size(), [20, 60]);
        assert!(!image.bytes().windows(4).any(|w| w == b"Exif"));
    }

    #[test]
    fn very_wide_image_signatures_keep_the_requested_rectangle() {
        let image = SignatureImage::from_bytes(&png(3000, 10)).unwrap();
        let mut session = crate::Session::new();
        let blank = session.create_blank(300.0, 400.0, 1).unwrap();
        let id = session.open_new("form.pdf", blank).unwrap();
        session.apply(id, image.edit(0, &page(0), [20.0, 100.0], false, "Ada").unwrap()).unwrap();
        let a = &session.get(id).unwrap().info.annotations[0];
        assert_eq!(a.rect, [20.0, 99.75, 170.0, 100.25]);
    }

    #[test]
    fn embedded_signature_preview_preserves_alpha_without_editing_the_document() {
        let image = SignatureImage::from_bytes(&png(120, 40)).unwrap();
        let mut session = crate::Session::new();
        let blank = session.create_blank(300.0, 400.0, 1).unwrap();
        let id = session.open_new("form.pdf", blank).unwrap();
        session.apply(id, image.edit(0, &page(0), [20.0, 100.0], false, "Ada").unwrap()).unwrap();
        let doc = session.get(id).unwrap();
        let bytes = doc.bytes.clone();
        let generation = doc.edit_generation();
        let preview = doc.image_signature_preview(0, 0).unwrap().unwrap();
        assert_eq!(preview.image.rgba(), image.rgba());
        let background = preview.render_background(1.0).unwrap();
        assert!(background.rgba.as_chunks::<4>().0.iter().all(|p| *p == [255, 255, 255, 255]));
        assert!(Arc::ptr_eq(&bytes, &doc.bytes));
        assert_eq!(doc.edit_generation(), generation);
        assert_eq!(doc.can_undo(), Some("Add signature"));
        assert!(doc.image_signature_preview(0, 1).is_err());
        assert!(preview.render_background(0.0).is_err());
    }

    #[test]
    fn oversized_embedded_signature_preview_is_rejected_before_decoding() {
        let image = SignatureImage::from_bytes(&png(120, 40)).unwrap();
        let mut session = crate::Session::new();
        let blank = session.create_blank(300.0, 400.0, 1).unwrap();
        let id = session.open_new("form.pdf", blank).unwrap();
        session.apply(id, image.edit(0, &page(0), [20.0, 100.0], false, "Ada").unwrap()).unwrap();
        let doc = session.doc_mut(id).unwrap();
        let cos = &mut doc.editor.as_mut().unwrap().cos;
        let picture = pdfcraft_annot::signature_image(cos, 0, 0).unwrap().unwrap();
        let pdfcraft_cos::Object::Stream(mut stream) = (*cos.get(picture)).clone() else { panic!() };
        stream.dict.set(b"Width".to_vec(), pdfcraft_cos::Object::Int(i64::MAX));
        cos.set(picture, pdfcraft_cos::Object::Stream(stream));
        let error = doc.image_signature_preview(0, 0).err().unwrap();
        assert!(error.contains("4 megapixels"), "{error}");
    }

    /// "Left edge at the point, centered vertically" is as displayed: on a turned page the box
    /// is another one in user space. The image is 64 x 32 pt; the displayed point is (60, 100).
    #[test]
    fn the_signature_box_is_laid_out_as_displayed_on_turned_pages() {
        let image = SignatureImage::from_bytes(&png(80, 40)).unwrap();
        for (rotation, at, want) in [
            (0, [60.0, 300.0], [60.0, 284.0, 124.0, 316.0]),
            (90, [100.0, 60.0], [84.0, 60.0, 116.0, 124.0]),
            (180, [240.0, 100.0], [176.0, 84.0, 240.0, 116.0]),
            (270, [200.0, 340.0], [184.0, 276.0, 216.0, 340.0]),
        ] {
            let rect = image.rect(&page(rotation), at, false).unwrap();
            assert!(rect.iter().zip(want).all(|(a, b)| (a - b).abs() < 0.01), "{rotation}: {rect:?}, want {want:?}");
        }
    }

    #[test]
    fn white_background_becomes_transparent_and_ink_is_kept() {
        let mut source = image::RgbaImage::new(4, 1);
        source.put_pixel(0, 0, image::Rgba([255, 255, 255, 255]));
        source.put_pixel(1, 0, image::Rgba([20, 20, 20, 255]));
        source.put_pixel(2, 0, image::Rgba([227, 227, 227, 255]));
        source.put_pixel(3, 0, image::Rgba([10, 10, 10, 100]));
        let mut encoded = Cursor::new(Vec::new());
        source.write_to(&mut encoded, ImageFormat::Png).unwrap();

        let image = SignatureImage::from_bytes(&encoded.into_inner()).unwrap();
        let cleaned = image.remove_white_background(245, 35).unwrap();
        let pixels = cleaned.rgba().as_chunks::<4>().0;

        assert_eq!(cleaned.size(), [4, 1]);
        assert_eq!(pixels[0][3], 0, "white paper must be transparent");
        assert_eq!(pixels[1], [20, 20, 20, 255], "dark ink must be unchanged");
        assert!(pixels[2][3] > 0 && pixels[2][3] < 255, "near-white must fade");
        assert_eq!(pixels[3][3], 100, "existing transparency must be kept");
    }
}
