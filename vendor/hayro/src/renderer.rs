use crate::{RenderCache, derive_settings};
use hayro_interpret::encode::EncodedShadingPattern;
use hayro_interpret::font::Glyph;
use hayro_interpret::pattern::Pattern;
use hayro_interpret::{
    BlendMode, CacheKey, ClipPath, Device, FillRule, GlyphDrawMode, ImageData, LumaData, MaskType,
    Paint, PathDrawMode, RgbData, SoftMask, StrokeProps,
};
use kurbo::{Affine, BezPath, Point, Rect, Shape, Vec2};
use pic_scale::{
    ImageSize, ImageStore, ImageStoreMut, PicScaleError, Resampling, ResamplingFunction, Scaler,
};
use std::collections::HashMap;
use std::mem::size_of;
use std::rc::Rc;
use std::sync::Arc;
use vello_cpu::color::palette::css::BLACK;
use vello_cpu::color::{AlphaColor, PremulRgba8, Srgb};
use vello_cpu::peniko::{Compose, Fill, ImageQuality, ImageSampler, Mix};
use vello_cpu::{
    Image, ImageSource, Mask, PaintType, Pixmap, RenderContext, RenderSettings, peniko,
};

/// PdfCraft patch: largest image drawn, in pixels (2^28 ≈ 268 MP, well above real page images).
const MAX_IMAGE_PIXELS: u64 = 1 << 28;

/// PdfCraft patch: source axes allowed into CatmullRom planning. Unlike the final pixmap,
/// sources need not fit u16: a 65536-pixel strip can safely shrink to a 16-pixel image.
/// This resource cap follows the existing CCITT column limit; the filter-work estimate below
/// additionally bounds tables/scratch before entering the opaque PicScale planner.
const MAX_RESAMPLING_SOURCE_SIDE: u32 = 1 << 20;
const MAX_RESAMPLING_PLAN_BYTES: usize = 128 << 20;

/// PdfCraft patch: source, intermediate and RGBA byte lengths, independent of backend sides.
fn source_byte_len(width: u32, height: u32, channels: usize, max_pixels: u64) -> Option<usize> {
    if width == 0 || height == 0 || !matches!(channels, 1 | 3 | 4) {
        return None;
    }
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    if pixels > max_pixels {
        return None;
    }
    let pixels = usize::try_from(pixels).ok()?;
    let rgba_len = pixels.checked_mul(4)?;
    if rgba_len > isize::MAX as usize {
        return None;
    }
    pixels.checked_mul(channels)
}

/// PdfCraft patch: only final pixmaps/padded glyphs need to fit the backend's u16 sides.
fn image_byte_len(width: u32, height: u32, channels: usize, max_pixels: u64) -> Option<usize> {
    if width > u32::from(u16::MAX) || height > u32::from(u16::MAX) {
        return None;
    }
    source_byte_len(width, height, channels, max_pixels)
}

/// PdfCraft patch: conservative CatmullRom planning work for one axis in PicScale 0.7.12.
/// Its four-tap kernel grows on downscaling. Include float/32-aligned i16 weights, temporary
/// float/f64/usize tap arrays and two copies of the bounds. This is a size estimate, not a
/// fallible reserve inside PicScale or a bound on total renderer memory.
fn filter_plan_bytes(source: u32, target: u32) -> Option<usize> {
    if source == target {
        return Some(0);
    }
    let (source, target) = (usize::try_from(source).ok()?, usize::try_from(target).ok()?);
    if target == 0 {
        return None;
    }
    let taps = source.checked_mul(4)?.div_ceil(target).max(4);
    let aligned_taps = taps.div_ceil(32).checked_mul(32)?;
    let float_weights = taps.checked_mul(target)?.checked_mul(size_of::<f32>())?;
    let fixed_weights = aligned_taps
        .checked_mul(target)?
        .checked_mul(size_of::<i16>())?;
    let tap_scratch = taps.checked_mul(size_of::<f32>() + size_of::<f64>() + size_of::<usize>())?;
    let bounds = target.checked_mul(4)?.checked_mul(size_of::<usize>())?;
    float_weights
        .checked_add(fixed_weights)?
        .checked_add(tap_scratch)?
        .checked_add(bounds)
}

/// PdfCraft patch: alpha-aware RGBA plans also need source-sized premultiplication scratch.
/// Bound the combined scratch before planning, including a conservative crossed intermediate
/// (or four rows for a short target) when both convolution axes change.
fn resampling_scratch_byte_len(
    source: (u32, u32),
    target: (u32, u32),
    channels: usize,
    max_pixels: u64,
) -> Option<usize> {
    if source == target {
        return Some(0);
    }
    let alpha = if channels == 4 {
        source_byte_len(source.0, source.1, channels, max_pixels)?
    } else {
        0
    };
    let filter = if source.0 != target.0 && source.1 != target.1 {
        source_byte_len(source.0, target.1.max(4), channels, max_pixels)?
    } else {
        0
    };
    let len = alpha.checked_add(filter)?;
    let limit = max_pixels.saturating_mul(4).min(isize::MAX as u64);
    if u64::try_from(len).ok()? > limit {
        return None;
    }
    Some(len)
}

/// PdfCraft patch: validate output, crossed intermediate and filter-work bounds before planning.
/// Source width × target height also bounds PicScale's smaller single-threaded four-row scratch.
/// Nearest-neighbour (`!interpolate`) has none of those: it copies source pixels into the target,
/// so only the source and target buffers are bounded.
fn resampling_byte_len(
    source: (u32, u32),
    target: (u32, u32),
    channels: usize,
    data_len: usize,
    max_pixels: u64,
    interpolate: bool,
) -> Option<usize> {
    if source_byte_len(source.0, source.1, channels, max_pixels)? != data_len {
        return None;
    }
    let target_len = image_byte_len(target.0, target.1, channels, max_pixels)?;
    if !interpolate {
        return Some(target_len);
    }
    resampling_scratch_byte_len(source, target, channels, max_pixels)?;
    source_byte_len(source.0, target.1, channels, max_pixels)?;
    if source.0 > MAX_RESAMPLING_SOURCE_SIDE || source.1 > MAX_RESAMPLING_SOURCE_SIDE {
        return None;
    }
    let plan_bytes = filter_plan_bytes(source.0, target.0)?
        .checked_add(filter_plan_bytes(source.1, target.1)?)?;
    if plan_bytes > MAX_RESAMPLING_PLAN_BYTES {
        return None;
    }
    Some(target_len)
}

/// PdfCraft patch: image dimensions selected before allocating resampling buffers.
/// Exported for PdfCraft's small, metadata-only regression tests. `None` rejects invalid
/// sources/scales or an over-budget target/intermediate; the caller may retain a valid source.
/// `interpolate` selects the Catmull-Rom limits; nearest-neighbour (`false`) skips them.
#[doc(hidden)]
pub fn image_resampling_size(
    width: u32,
    height: u32,
    channels: usize,
    data_len: usize,
    x_scale: f32,
    y_scale: f32,
    max_pixels: u64,
    interpolate: bool,
) -> Option<(u32, u32)> {
    if !x_scale.is_finite() || !y_scale.is_finite() || x_scale <= 0.0 || y_scale <= 0.0 {
        return None;
    }
    let target = if x_scale < 1.0 || y_scale < 1.0 {
        let side = |size: u32, scale: f32| {
            (size as f32 * scale)
                .ceil()
                .max(1.0)
                .min((u16::MAX / 2) as f32) as u32
        };
        (side(width, x_scale), side(height, y_scale))
    } else {
        (width, height)
    };
    resampling_byte_len(
        (width, height),
        target,
        channels,
        data_len,
        max_pixels,
        interpolate,
    )?;
    Some(target)
}

/// PdfCraft patch: allocation failures in image conversion/resampling keep the renderer alive.
fn image_buffer(len: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    out.try_reserve_exact(len).ok()?;
    Some(out)
}

/// PdfCraft patch (#624): the source index that PicScale's nearest-neighbour plan copies for
/// destination `index` of `dst` samples taken from `src`. It is the same fixed-point centre
/// sampling as `ResampleNearestPlan`, so these are the samples a nearest resize would copy.
fn nearest_source_index(src: u32, dst: u32, index: u32) -> usize {
    let step = (u64::from(src) << 32) / u64::from(dst);
    ((u64::from(index) * step + (step >> 1)) >> 32) as usize
}

/// PdfCraft patch (#624): nearest-resizes a colour image and its same-sized soft mask straight
/// into premultiplied RGBA of `target` size. Colour and mask come from the same source pixel, so
/// this equals a nearest resize of the expanded RGBA followed by premultiplying each pixel.
fn nearest_premultiplied_rgba(
    color: &ImageData,
    alpha: &[u8],
    source: (u32, u32),
    target: (u32, u32),
) -> Option<Vec<u8>> {
    let (samples, channels) = match color {
        ImageData::Luma(luma) => (luma.data.as_slice(), 1),
        ImageData::Rgb(rgb) => (rgb.data.as_slice(), 3),
    };
    let mut out = image_buffer(image_byte_len(target.0, target.1, 4, MAX_IMAGE_PIXELS)?)?;
    for ty in 0..target.1 {
        let sy = nearest_source_index(source.1, target.1, ty);
        for tx in 0..target.0 {
            let si = sy * source.0 as usize + nearest_source_index(source.0, target.0, tx);
            let [r, g, b] = match samples.get(si * channels..si * channels + channels)? {
                [v] => [*v; 3],
                [r, g, b] => [*r, *g, *b],
                _ => return None,
            };
            let a = *alpha.get(si)?;
            out.extend_from_slice(
                &AlphaColor::from_rgba8(r, g, b, a)
                    .premultiply()
                    .to_rgba8()
                    .to_u8_array(),
            );
        }
    }
    Some(out)
}

/// PdfCraft patch: most pieces one dashed stroke may be cut into. Stroke expansion keeps every
/// dash, so a pattern that is tiny next to its path asks for billions: a fuzzed `/D [[11] 0]` on
/// a line from x = 92234775807 allocated over 5 GB, and `[0.000001] 0 d` on a 20 pt line over
/// 10 GB. Measured in release builds: a stroke cut into 250,000 pieces renders in about 16 ms and
/// 75 MB at its peak (a million took 80 ms and 290 MB). Dotted and dashed lines in real documents
/// stay far below this.
const MAX_DASHES_PER_STROKE: f64 = 250_000.0;

/// PdfCraft patch: the dash pattern to stroke `path` with. Empty (a solid line) when the pattern
/// is invalid, or when it could cut the path into more than [`MAX_DASHES_PER_STROKE`] pieces.
/// ISO 32000-2 §8.4.3.6 requires nonnegative entries that are not all zero; a pattern whose
/// period isn't positive (`[-1 -1] 0 d`) made kurbo's search for the starting dash loop for ever.
fn usable_dash_pattern(path: &BezPath, dashes: &[f32]) -> Vec<f64> {
    if dashes.iter().any(|d| !d.is_finite() || *d < 0.0) {
        return Vec::new();
    }
    let sum: f64 = dashes.iter().map(|d| f64::from(*d)).sum();
    // kurbo repeats an odd-length pattern once, as SVG does, so a period covers it twice.
    let (period, per_period) = if dashes.len() % 2 == 1 {
        (2.0 * sum, dashes.len().saturating_mul(2))
    } else {
        (sum, dashes.len())
    };
    if !period.is_finite() || period <= 0.0 {
        return Vec::new();
    }
    let (length, restarts) = control_polygon_length(path);
    // kurbo restarts the pattern at every subpath and after every `ClosePath`, so each restart
    // can begin up to a whole period of pieces however short its segment is.
    let pieces = (length / period + restarts as f64) * per_period as f64;
    if !pieces.is_finite() || pieces > MAX_DASHES_PER_STROKE {
        return Vec::new();
    }
    dashes.iter().map(|d| f64::from(*d)).collect()
}

/// PdfCraft patch: the length of `path`'s control polygon, which a curve never exceeds, and how
/// many times kurbo restarts a dash pattern along it (each `MoveTo` and `ClosePath`).
fn control_polygon_length(path: &BezPath) -> (f64, usize) {
    let (mut length, mut restarts) = (0.0, 0usize);
    let (mut start, mut last) = (Point::ORIGIN, Point::ORIGIN);
    for el in path.elements() {
        match *el {
            kurbo::PathEl::MoveTo(p) => {
                (start, last) = (p, p);
                restarts = restarts.saturating_add(1);
            }
            kurbo::PathEl::LineTo(p) => {
                length += last.distance(p);
                last = p;
            }
            kurbo::PathEl::QuadTo(c, p) => {
                length += last.distance(c) + c.distance(p);
                last = p;
            }
            kurbo::PathEl::CurveTo(c1, c2, p) => {
                length += last.distance(c1) + c1.distance(c2) + c2.distance(p);
                last = p;
            }
            kurbo::PathEl::ClosePath => {
                length += last.distance(start);
                last = start;
                restarts = restarts.saturating_add(1);
            }
        }
    }
    (length, restarts)
}

/// PdfCraft patch: how far, in user space, drawing may place geometry outside a path's exact
/// outline. vello expands strokes to a tolerance of at most 0.25 user units (`TOL / max(|a|, |d|,
/// 1)` in vello_common's `flatten::stroke`), and kurbo approximates round joins and caps to 1e-3.
/// Fills are flattened into chords, which stay inside the outline's bounds.
const DRAW_SLACK: f64 = 0.5;

thread_local! {
    /// PdfCraft patch: whether [`may_paint_canvas`] may skip paths on this thread. Turned off
    /// only by PdfCraft's regression test, which renders each tile with and without skipping.
    static SKIP_OFFSCREEN_PATHS: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// PdfCraft patch: exported for its regression test (see [`SKIP_OFFSCREEN_PATHS`]).
#[doc(hidden)]
pub fn set_skip_offscreen_paths(skip: bool) {
    SKIP_OFFSCREEN_PATHS.with(|s| s.set(skip));
}

/// PdfCraft patch: whether a path drawn with `transform` can paint a pixel of a `width` ×
/// `height` canvas. `reach` is how far its paint extends beyond the path, in user space: 0 for a
/// fill; for a stroke, half its width times the larger of its miter limit (kurbo draws a miter
/// only when its tip is within `limit × width / 2` of the vertex) and √2 (square caps). The box
/// of the path's points and control points (a curve stays inside their hull) grown by that and
/// [`DRAW_SLACK`], mapped to the device and grown by 2 pixels for antialiasing, contains
/// everything it paints: a path whose box misses the canvas is skipped without changing a pixel,
/// so a tile of a large page no longer strokes and fills every path on the page. Bounds that
/// aren't finite (an empty path) are drawn as before.
fn may_paint_canvas(
    path: &BezPath,
    transform: Affine,
    reach: f64,
    width: u16,
    height: u16,
) -> bool {
    if !SKIP_OFFSCREEN_PATHS.with(std::cell::Cell::get) {
        return true;
    }
    let mut points = Rect::new(
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    let mut add = |p: Point| points = points.union_pt(p);
    for el in path.elements() {
        match *el {
            kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) => add(p),
            kurbo::PathEl::QuadTo(c, p) => {
                add(c);
                add(p);
            }
            kurbo::PathEl::CurveTo(c1, c2, p) => {
                add(c1);
                add(c2);
                add(p);
            }
            kurbo::PathEl::ClosePath => {}
        }
    }
    let grow = reach + DRAW_SLACK;
    let device = transform
        .transform_rect_bbox(points.inflate(grow, grow))
        .inflate(2.0, 2.0);
    let finite = [device.x0, device.y0, device.x1, device.y1]
        .iter()
        .all(|v| v.is_finite());
    !finite
        || (device.x0 < f64::from(width)
            && device.x1 > 0.0
            && device.y0 < f64::from(height)
            && device.y1 > 0.0)
}

pub(crate) struct Renderer {
    pub(crate) ctx: RenderContext,
    pub(crate) inside_pattern: bool,
    pub(crate) soft_mask_cache: HashMap<u128, Mask>,
    pub(crate) outline_cache: Rc<std::cell::RefCell<HashMap<u128, Rc<BezPath>>>>,
    pub(crate) cur_mask: Option<Mask>,
    pub(crate) in_type3_glyph: bool,
    pub(crate) scaler: Scaler,
    // TODO: Remove this once vello_cpu bug with non-transparent images is fixed
    image_transparency_stack: Vec<bool>,
}

#[derive(Clone, Copy)]
enum ImagePixelFormat {
    Luma,
    Rgb,
    Rgba,
}

impl Renderer {
    pub(crate) fn new(
        width: u16,
        height: u16,
        settings: RenderSettings,
        cache: &RenderCache<'_>,
    ) -> Self {
        Self {
            ctx: RenderContext::new_with(width, height, settings),
            inside_pattern: false,
            soft_mask_cache: HashMap::default(),
            outline_cache: cache.outline_cache.clone(),
            cur_mask: None,
            in_type3_glyph: false,
            scaler: Scaler::new(ResamplingFunction::CatmullRom),
            image_transparency_stack: Vec::new(),
        }
    }

    /// PdfCraft patch: returns the stroke's reach for [`may_paint_canvas`].
    fn set_stroke_properties(
        &mut self,
        stroke_props: &StrokeProps,
        is_text: bool,
        path: &BezPath,
    ) -> f64 {
        let threshold = if is_text { 0.25 } else { 1.0 };

        // Best-effort attempt to ensure a line width of at least 1.0, as required by the PDF
        // specification. If we are stroking text, we reduce the threshold as it will otherwise
        // lead to very bold-looking text at low resolutions.
        let min_factor = max_factor(self.ctx.transform());
        let mut line_width = stroke_props.line_width.max(0.01);
        let transformed_width = line_width * min_factor;

        // Only enforce line width if not inside of pattern or type 3 glyph.
        if transformed_width < threshold && !self.inside_pattern && !self.in_type3_glyph {
            line_width /= transformed_width;
            line_width *= threshold;
        }

        // PdfCraft patch: a stroke far wider than the canvas looks the same as one a few canvases
        // wide, but its geometry grows with the width: a fuzzed `/LW 9223372036854775807`
        // allocated 10 GB in stroke expansion.
        let widest = 4.0 * (f32::from(self.ctx.width()) + f32::from(self.ctx.height()));
        if min_factor.is_finite() && min_factor > 0.0 && line_width * min_factor > widest {
            line_width = widest / min_factor;
        }

        let stroke = kurbo::Stroke {
            width: line_width as f64,
            join: stroke_props.line_join,
            miter_limit: stroke_props.miter_limit as f64,
            start_cap: stroke_props.line_cap,
            end_cap: stroke_props.line_cap,
            // PdfCraft patch: see `usable_dash_pattern`.
            dash_pattern: usable_dash_pattern(path, &stroke_props.dash_array).into(),
            dash_offset: if stroke_props.dash_offset.is_finite() {
                stroke_props.dash_offset as f64
            } else {
                0.0
            },
        };

        // A miter limit that isn't finite leaves miters unbounded: never skipped.
        let miter = stroke.miter_limit.abs();
        let reach = stroke.width / 2.0
            * if miter.is_finite() {
                miter.max(std::f64::consts::SQRT_2)
            } else {
                f64::INFINITY
            };
        self.ctx.set_stroke(stroke);
        reach
    }

    fn draw_image_with_alpha_mask(&mut self, image_data: ImageData, alpha_data: LumaData) {
        // PdfCraft patch: mismatched masks need a dummy RGB staging image. Validate and
        // reserve it before creating/pushing any mask, so failure skips the whole image.
        let Some(len) = source_byte_len(alpha_data.width, alpha_data.height, 3, MAX_IMAGE_PIXELS)
        else {
            log::warn!(
                "Image alpha-mask staging dimensions exceeded the buffer limit; image skipped"
            );
            return;
        };
        let Some(mut dummy_rgb) = image_buffer(len) else {
            log::warn!("Image alpha-mask staging buffer could not be allocated; image skipped");
            return;
        };
        dummy_rgb.resize(len, 0);
        let mask = {
            let transform = *self.ctx.transform()
                * Affine::scale_non_uniform(
                    image_data.width() as f64 / alpha_data.width as f64,
                    image_data.height() as f64 / alpha_data.height as f64,
                );
            let mut renderer = Self {
                ctx: RenderContext::new_with(
                    self.ctx.width(),
                    self.ctx.height(),
                    derive_settings(self.ctx.render_settings()),
                ),
                inside_pattern: false,
                soft_mask_cache: HashMap::default(),
                outline_cache: self.outline_cache.clone(),
                cur_mask: None,
                in_type3_glyph: false,
                scaler: self.scaler,
                image_transparency_stack: Vec::new(),
            };
            let mut mask_pix = Pixmap::new(self.ctx.width(), self.ctx.height());
            let rgb_data = ImageData::Rgb(RgbData {
                data: dummy_rgb,
                width: alpha_data.width,
                height: alpha_data.height,
                interpolate: alpha_data.interpolate,
                scale_factors: alpha_data.scale_factors,
            });
            renderer.ctx.set_transform(transform);
            // Note that there is a circle between `draw_image` and `draw_image_with_alpha_mask`,
            // but `draw_image_with_alpha_mask` is only called if the dimensions or interpolate
            // values between alpha_data and rgb_data don't match, which they do here.
            renderer.draw_image(rgb_data, Some(alpha_data));
            renderer.ctx.flush();
            let mut resources = vello_cpu::Resources::default();
            renderer.ctx.render_to_pixmap(&mut resources, &mut mask_pix);
            Mask::new_alpha(&mask_pix)
        };

        self.ctx.push_mask_layer(mask);
        self.image_transparency_stack.push(true);
        self.draw_image(image_data, None);
        self.image_transparency_stack.pop();
        self.ctx.pop_layer();
    }

    fn resize_image_data(
        &self,
        data: Vec<u8>,
        src_width: u32,
        src_height: u32,
        new_width: u32,
        new_height: u32,
        pixel_format: ImagePixelFormat,
        interpolate: bool,
    ) -> (Vec<u8>, u32, u32) {
        // PdfCraft patch (#624): `/Interpolate` governs the pre-minification pass too. Using
        // Catmull-Rom here for an image that did not request interpolation blends hard edges
        // before ImageQuality::Low gets a chance to sample it. Keep the pre-resize (and its
        // bounded memory/performance behaviour), but use nearest-neighbour when interpolation
        // is disabled. PDF's default for `/Interpolate` is false.
        let nearest = Scaler::new(ResamplingFunction::Nearest);
        let scaler = if interpolate { &self.scaler } else { &nearest };
        let resized = match pixel_format {
            ImagePixelFormat::Luma => self.resize_image_data_impl::<1>(
                scaler,
                interpolate,
                data,
                src_width,
                src_height,
                new_width,
                new_height,
                |scaler, source_size, target_size| {
                    scaler.plan_planar_resampling(source_size, target_size)
                },
            ),
            ImagePixelFormat::Rgb => self.resize_image_data_impl::<3>(
                scaler,
                interpolate,
                data,
                src_width,
                src_height,
                new_width,
                new_height,
                |scaler, source_size, target_size| {
                    scaler.plan_rgb_resampling(source_size, target_size)
                },
            ),
            ImagePixelFormat::Rgba => self.resize_image_data_impl::<4>(
                scaler,
                interpolate,
                data,
                src_width,
                src_height,
                new_width,
                new_height,
                |scaler, source_size, target_size| {
                    scaler.plan_rgba_resampling(source_size, target_size, true)
                },
            ),
        };
        match resized {
            Ok(out) => (out, new_width, new_height),
            Err(original) => {
                log::warn!(
                    "Image resampling failed or exceeded its buffer limit; falling back to source data"
                );
                (original, src_width, src_height)
            }
        }
    }

    fn resize_image_data_impl<const N: usize>(
        &self,
        scaler: &Scaler,
        interpolate: bool,
        data: Vec<u8>,
        src_width: u32,
        src_height: u32,
        new_width: u32,
        new_height: u32,
        plan: impl FnOnce(
            &Scaler,
            ImageSize,
            ImageSize,
        ) -> Result<Arc<Resampling<u8, N>>, PicScaleError>,
    ) -> Result<Vec<u8>, Vec<u8>> {
        // PdfCraft patch: validate before constructing a plan or allocating its output/scratch.
        // Returning the owned input preserves both its pixels and dimensions on any failure.
        let resized = (|| {
            let len = resampling_byte_len(
                (src_width, src_height),
                (new_width, new_height),
                N,
                data.len(),
                MAX_IMAGE_PIXELS,
                interpolate,
            )?;
            let source_size = ImageSize::new(src_width as usize, src_height as usize);
            let target_size = ImageSize::new(new_width as usize, new_height as usize);
            let src = ImageStore::<u8, N>::from_slice(&data, source_size.width, source_size.height)
                .ok()?;
            let plan = plan(scaler, source_size, target_size).ok()?;
            let scratch_len = plan.scratch_size();
            // Nearest-neighbour needs no scratch, so any scratch it reports is refused.
            let scratch_bound = if interpolate {
                resampling_scratch_byte_len(
                    (src_width, src_height),
                    (new_width, new_height),
                    N,
                    MAX_IMAGE_PIXELS,
                )?
            } else {
                0
            };
            if scratch_len > scratch_bound {
                return None;
            }
            let mut out = image_buffer(len)?;
            out.resize(len, 0);
            let mut dst =
                ImageStoreMut::<u8, N>::from_slice(&mut out, target_size.width, target_size.height)
                    .ok()?;
            let mut scratch = image_buffer(scratch_len)?;
            scratch.resize(scratch_len, 0);
            plan.resample_with_scratch(&src, &mut dst, &mut scratch)
                .ok()?;
            Some(out)
        })();
        match resized {
            Some(out) => Ok(out),
            None => Err(data),
        }
    }

    fn draw_image(&mut self, image_data: ImageData, alpha_data: Option<LumaData>) {
        let cur_transform = *self.ctx.transform();
        let (x_scale, y_scale) = {
            let (x, y) = x_y_advances(&cur_transform);
            (x.length() as f32, y.length() as f32)
        };
        // PdfCraft patch: do not send non-finite or collapsed geometry to the rasterizer.
        if !cur_transform.as_coeffs().iter().all(|v| v.is_finite())
            || !x_scale.is_finite()
            || !y_scale.is_finite()
            || x_scale <= 0.0
            || y_scale <= 0.0
        {
            log::warn!("Image has an invalid transform and was skipped");
            return;
        }
        let (source_width, source_height) = (image_data.width(), image_data.height());
        let channels = match &image_data {
            ImageData::Luma(_) => 1,
            ImageData::Rgb(_) => 3,
        };
        let data_len = match &image_data {
            ImageData::Luma(a) => a.data.len(),
            ImageData::Rgb(a) => a.data.len(),
        };
        if source_byte_len(source_width, source_height, channels, MAX_IMAGE_PIXELS)
            != Some(data_len)
        {
            log::warn!("Image has invalid or over-limit source dimensions/data and was skipped");
            return;
        }
        let interpolate = image_data.interpolate();
        if let Some(a) = &alpha_data {
            if source_byte_len(a.width, a.height, 1, MAX_IMAGE_PIXELS) != Some(a.data.len()) {
                log::warn!("Image has invalid or over-limit alpha dimensions/data and was skipped");
                return;
            }
            if a.width != source_width || a.height != source_height || a.interpolate != interpolate
            {
                if let Some(alpha) = alpha_data {
                    self.draw_image_with_alpha_mask(image_data, alpha);
                }
                return;
            }
        }
        let mut quality = if interpolate {
            ImageQuality::Medium
        } else {
            ImageQuality::Low
        };
        let has_alpha = alpha_data.is_some();
        let mut may_have_transparency = has_alpha || self.force_images_may_have_transparency();
        let requested_resize = x_scale < 1.0 || y_scale < 1.0;
        let (new_width, new_height) = match image_resampling_size(
            source_width,
            source_height,
            channels,
            data_len,
            x_scale,
            y_scale,
            MAX_IMAGE_PIXELS,
            interpolate,
        ) {
            Some(target) => target,
            None => {
                if image_byte_len(source_width, source_height, 4, MAX_IMAGE_PIXELS).is_none() {
                    log::warn!(
                        "Image resampling exceeded its buffer limit and the original resolution cannot fit the backend; image skipped"
                    );
                    return;
                }
                log::warn!(
                    "Image resampling exceeded its buffer limit; retaining the original resolution"
                );
                (source_width, source_height)
            }
        };
        // PdfCraft patch (#624): High is bicubic and would blend the nearest pre-resize of an image
        // that did not request interpolation, so only interpolated glyph images are promoted.
        if requested_resize && self.in_type3_glyph && interpolate {
            quality = ImageQuality::High;
        }
        let needs_resize = (new_width, new_height) != (source_width, source_height);

        // PdfCraft patch (#624): a minified image with a soft mask that is not interpolated is
        // nearest-sampled straight into premultiplied target RGBA. That skips the full-resolution
        // RGBA copy below and the premultiply pass after it.
        let direct = match &alpha_data {
            Some(alpha) if needs_resize && !interpolate => nearest_premultiplied_rgba(
                &image_data,
                &alpha.data,
                (source_width, source_height),
                (new_width, new_height),
            ),
            _ => None,
        };
        let premultiplied = direct.is_some();
        // Preserve the single-channel/RGB fast paths. With alpha, expand directly to RGBA;
        // every source and alpha length was validated before zipping or allocating.
        let (data, format) = match (direct, image_data, alpha_data) {
            (Some(rgba), _, _) => (rgba, ImagePixelFormat::Rgba),
            (None, ImageData::Luma(luma), None) => (luma.data, ImagePixelFormat::Luma),
            (None, ImageData::Rgb(rgb), None) => (rgb.data, ImagePixelFormat::Rgb),
            (None, image, Some(alpha)) => {
                let Some(len) = source_byte_len(source_width, source_height, 4, MAX_IMAGE_PIXELS)
                else {
                    return;
                };
                let Some(mut rgba) = image_buffer(len) else {
                    log::warn!(
                        "Image RGBA buffer could not be allocated and the image was skipped"
                    );
                    return;
                };
                match image {
                    ImageData::Luma(luma) => {
                        rgba.extend(
                            luma.data
                                .iter()
                                .zip(alpha.data)
                                .flat_map(|(g, a)| [*g, *g, *g, a]),
                        );
                    }
                    ImageData::Rgb(rgb) => {
                        rgba.extend(
                            rgb.data
                                .as_chunks::<3>()
                                .0
                                .iter()
                                .zip(alpha.data)
                                .flat_map(|([r, g, b], a)| [*r, *g, *b, a]),
                        );
                    }
                }
                (rgba, ImagePixelFormat::Rgba)
            }
        };
        let (data, mut img_width, mut img_height) = if premultiplied {
            (data, new_width, new_height)
        } else if needs_resize {
            self.resize_image_data(
                data,
                source_width,
                source_height,
                new_width,
                new_height,
                format,
                interpolate,
            )
        } else {
            (data, source_width, source_height)
        };
        if image_byte_len(img_width, img_height, 4, MAX_IMAGE_PIXELS).is_none() {
            log::warn!(
                "Image resampling failed and the original resolution cannot fit the backend; image skipped"
            );
            return;
        }
        let mut additional_transform = Affine::scale_non_uniform(
            source_width as f64 / img_width as f64,
            source_height as f64 / img_height as f64,
        );
        let mut rgba_data = if matches!(format, ImagePixelFormat::Rgba) {
            data
        } else {
            let Some(len) = image_byte_len(img_width, img_height, 4, MAX_IMAGE_PIXELS) else {
                return;
            };
            let Some(mut rgba) = image_buffer(len) else {
                log::warn!("Image RGBA buffer could not be allocated and the image was skipped");
                return;
            };
            match format {
                ImagePixelFormat::Luma => rgba.extend(data.iter().flat_map(|g| [*g, *g, *g, 255])),
                ImagePixelFormat::Rgb => rgba.extend(
                    data.as_chunks::<3>()
                        .0
                        .iter()
                        .flat_map(|[r, g, b]| [*r, *g, *b, 255]),
                ),
                ImagePixelFormat::Rgba => {} // Handled above.
            }
            rgba
        };
        if has_alpha && !premultiplied {
            let (chunks, _) = rgba_data.as_chunks_mut::<4>();
            for chunk in chunks {
                let [r, g, b, a] = *chunk;
                *chunk = AlphaColor::from_rgba8(r, g, b, a)
                    .premultiply()
                    .to_rgba8()
                    .to_u8_array();
            }
        }
        // Type 3 glyphs get a transparent two-pixel frame for interpolation. Check the padded
        // RGBA allocation and u16 dimensions too, before narrowing or growing either side.
        if self.in_type3_glyph {
            let (Some(width), Some(height)) = (img_width.checked_add(4), img_height.checked_add(4))
            else {
                return;
            };
            let Some(len) = image_byte_len(width, height, 4, MAX_IMAGE_PIXELS) else {
                log::warn!(
                    "Image glyph padding exceeded its buffer limit and the image was skipped"
                );
                return;
            };
            let Some(mut padded) = image_buffer(len) else {
                log::warn!("Image glyph padding could not be allocated and the image was skipped");
                return;
            };
            let row_len = width as usize * 4; // Checked RGBA length above bounds every row.
            padded.resize(row_len * 2, 0);
            for row in rgba_data.chunks_exact(img_width as usize * 4) {
                padded.extend([0; 8]);
                padded.extend_from_slice(row);
                padded.extend([0; 8]);
            }
            padded.resize(len, 0);
            (img_width, img_height) = (width, height);
            additional_transform *= Affine::translate((-2.0, -2.0));
            may_have_transparency = true;
            rgba_data = padded;
        }
        let pixmap = Pixmap::from_parts_with_opacity(
            bytemuck::cast_vec(rgba_data),
            img_width as u16,
            img_height as u16,
            may_have_transparency,
        );
        self.draw_pixmap(
            Arc::new(pixmap),
            quality,
            cur_transform * additional_transform,
        );
    }

    fn push_clip_path_inner(&mut self, clip_path: &BezPath, fill: FillRule) {
        let old_transform = *self.ctx.transform();

        self.ctx.set_fill_rule(convert_fill_rule(fill));
        self.ctx.set_transform(Affine::IDENTITY);
        self.ctx.push_clip_path(clip_path);

        self.ctx.set_transform(old_transform);
    }

    fn draw_pixmap(&mut self, pixmap: Arc<Pixmap>, quality: ImageQuality, transform: Affine) {
        let (width, height) = (pixmap.width(), pixmap.height());
        let image = Image {
            image: ImageSource::Pixmap(pixmap),
            sampler: ImageSampler {
                x_extend: peniko::Extend::Pad,
                y_extend: peniko::Extend::Pad,
                quality,
                alpha: 1.0,
            },
        };

        self.ctx.set_transform(transform);
        self.ctx.set_paint(image);
        self.ctx
            .fill_rect(&Rect::new(0.0, 0.0, width as f64, height as f64));
    }

    #[must_use]
    fn set_paint(&mut self, paint: &Paint<'_>, path: &BezPath, is_stroke: bool) -> Option<BezPath> {
        let mut paint_transform = Affine::IDENTITY;
        let mut clip_path = None;

        let paint: PaintType = match paint.clone() {
            Paint::Color(c) => {
                let c = c.to_rgba().to_rgba8();
                AlphaColor::from_rgba8(c[0], c[1], c[2], c[3]).into()
            }
            Paint::Pattern(p) => {
                let path_transform = self.ctx.transform();

                match *p {
                    Pattern::Shading(s) => {
                        clip_path = s.shading.clip_path.clone();
                        let mut bbox = (*path_transform * path.clone()).bounding_box();

                        if is_stroke {
                            // Try to account for stroke in bbox.
                            let (a1, a2) = x_y_advances(path_transform);
                            let factor = a1.length().max(a2.length()) * self.ctx.stroke().width;
                            bbox = bbox.inflate(factor, factor);
                        }

                        bbox = bbox.intersect(Rect::new(
                            0.0,
                            0.0,
                            self.ctx.width() as f64,
                            self.ctx.height() as f64,
                        ));

                        // PdfCraft patch: mesh shadings are sampled only inside `bbox` (and a
                        // pixel around it: texels at a fractional edge look one pixel further).
                        let encoded = s.encode_within(Some(bbox.inflate(1.0, 1.0)));
                        let (image, width, height, transform, may_have_transparency) =
                            render_shading_texture(bbox, &encoded);
                        let may_have_transparency =
                            may_have_transparency || self.force_images_may_have_transparency();
                        paint_transform = path_transform.inverse() * transform;

                        let pixmap = Pixmap::from_parts_with_opacity(
                            image,
                            width as u16,
                            height as u16,
                            may_have_transparency,
                        );

                        let image = Image {
                            image: ImageSource::Pixmap(Arc::new(pixmap)),
                            sampler: ImageSampler {
                                x_extend: peniko::Extend::Repeat,
                                y_extend: peniko::Extend::Repeat,
                                quality: ImageQuality::Medium,
                                alpha: 1.0,
                            },
                        };

                        PaintType::Image(image)
                    }
                    Pattern::Tiling(t) => {
                        const MAX_PIXMAP_SIZE: f32 = 3000.0;
                        // TODO: Raise this limit and perform downsampling if reached
                        // (see pdftc_100k_0138.pdf).
                        const MIN_PIXMAP_SIZE: f32 = 1.0;

                        let bbox = t.bbox;
                        let max_x_scale = MAX_PIXMAP_SIZE / bbox.width() as f32;
                        let min_x_scale = MIN_PIXMAP_SIZE / bbox.width() as f32;
                        let max_y_scale = MAX_PIXMAP_SIZE / bbox.height() as f32;
                        let min_y_scale = MIN_PIXMAP_SIZE / bbox.height() as f32;

                        let (mut xs, mut ys) = {
                            let (x, y) = x_y_advances(&(t.matrix));
                            (x.length() as f32, y.length() as f32)
                        };
                        xs = xs.max(min_x_scale).min(max_x_scale);
                        ys = ys.max(min_y_scale).min(max_y_scale);
                        // PdfCraft patch: the cell pixmap is `step × scale` pixels, which only the
                        // u16 cast below bounded (a fuzzed /XStep far beyond /BBox gave 65535² cells).
                        xs = tiling_cell_scale(xs, t.x_step, MAX_PIXMAP_SIZE);
                        ys = tiling_cell_scale(ys, t.y_step, MAX_PIXMAP_SIZE);

                        let x_step = xs * t.x_step;
                        let y_step = ys * t.y_step;

                        let scaled_width = bbox.width() as f32 * xs;
                        let scaled_height = bbox.height() as f32 * ys;
                        let pix_width = x_step.abs().round() as u16;
                        let pix_height = y_step.abs().round() as u16;

                        let mut renderer = Self {
                            ctx: RenderContext::new_with(
                                pix_width,
                                pix_height,
                                derive_settings(self.ctx.render_settings()),
                            ),
                            cur_mask: None,
                            inside_pattern: true,
                            soft_mask_cache: HashMap::default(),
                            outline_cache: self.outline_cache.clone(),
                            in_type3_glyph: false,
                            scaler: self.scaler,
                            image_transparency_stack: Vec::new(),
                        };
                        let mut initial_transform = Affine::scale_non_uniform(xs as f64, ys as f64)
                            * Affine::translate((-bbox.x0, -bbox.y0));
                        t.interpret(&mut renderer, initial_transform, is_stroke);
                        let mut pix = Pixmap::new(pix_width, pix_height);
                        renderer.ctx.flush();
                        let mut resources = vello_cpu::Resources::default();
                        renderer.ctx.render_to_pixmap(&mut resources, &mut pix);

                        // TODO: Fix these
                        if x_step < 0.0 {
                            initial_transform *=
                                Affine::new([-1.0, 0.0, 0.0, 1.0, scaled_width as f64, 0.0]);
                        }

                        if y_step < 0.0 {
                            initial_transform *=
                                Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, scaled_height as f64]);
                        }

                        paint_transform =
                            path_transform.inverse() * t.matrix * initial_transform.inverse();

                        let image = Image {
                            image: ImageSource::Pixmap(Arc::new(pix)),
                            sampler: ImageSampler {
                                x_extend: peniko::Extend::Repeat,
                                y_extend: peniko::Extend::Repeat,
                                quality: ImageQuality::Medium,
                                alpha: 1.0,
                            },
                        };

                        PaintType::Image(image)
                    }
                }
            }
        };

        self.ctx.set_paint_transform(paint_transform);
        self.ctx.set_paint(paint);

        clip_path
    }

    fn stroke_path(
        &mut self,
        path: &BezPath,
        transform: Affine,
        paint: &Paint<'_>,
        stroke_props: &StrokeProps,
        is_text: bool,
    ) {
        self.ctx.set_transform(transform);
        let reach = self.set_stroke_properties(stroke_props, is_text, path);
        // PdfCraft patch: see `may_paint_canvas`.
        if !may_paint_canvas(path, transform, reach, self.ctx.width(), self.ctx.height()) {
            return;
        }

        let clip_path = self.set_paint(paint, path, true);
        if let Some(clip_path) = clip_path.as_ref() {
            self.push_clip_path_inner(clip_path, FillRule::NonZero);
        }
        self.ctx.stroke_path(path);
        if clip_path.is_some() {
            self.ctx.pop_clip_path();
        }
    }

    fn fill_path(
        &mut self,
        path: &BezPath,
        transform: Affine,
        paint: &Paint<'_>,
        fill_rule: FillRule,
    ) {
        self.ctx.set_fill_rule(convert_fill_rule(fill_rule));
        self.ctx.set_transform(transform);
        // PdfCraft patch: see `may_paint_canvas`.
        if !may_paint_canvas(path, transform, 0.0, self.ctx.width(), self.ctx.height()) {
            return;
        }

        let clip_path = self.set_paint(paint, path, false);
        if let Some(clip_path) = clip_path.as_ref() {
            self.push_clip_path_inner(clip_path, fill_rule);
        }

        self.ctx.fill_path(path);

        if clip_path.is_some() {
            self.ctx.pop_clip_path();
        }
    }

    fn fill_glyph<'a>(
        &mut self,
        glyph: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        paint: &Paint<'a>,
    ) {
        match glyph {
            Glyph::Outline(o) => {
                let base_outline = self.cached_outline(o);

                self.fill_path(
                    base_outline.as_ref(),
                    transform * glyph_transform,
                    paint,
                    FillRule::NonZero,
                );
            }
            Glyph::Type3(s) => {
                self.in_type3_glyph = true;
                s.interpret(self, transform, glyph_transform, paint);
                self.in_type3_glyph = false;
            }
        }
    }

    fn stroke_glyph<'a>(
        &mut self,
        glyph: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        paint: &Paint<'a>,
        stroke_props: &StrokeProps,
    ) {
        match glyph {
            Glyph::Outline(o) => {
                let base_outline = self.cached_outline(o);

                self.stroke_path(
                    &(glyph_transform * base_outline.as_ref().clone()),
                    transform,
                    paint,
                    stroke_props,
                    true,
                );
            }
            Glyph::Type3(s) => {
                s.interpret(self, transform, glyph_transform, paint);
            }
        }
    }

    fn cached_outline(&self, glyph: &hayro_interpret::font::OutlineGlyph) -> Rc<BezPath> {
        let id = glyph.identifier().cache_key();

        if let Some(path) = self.outline_cache.borrow().get(&id) {
            return path.clone();
        }

        let path = Rc::new(glyph.outline());
        self.outline_cache.borrow_mut().insert(id, path.clone());
        path
    }

    fn force_images_may_have_transparency(&self) -> bool {
        self.cur_mask.is_some()
            || self
                .image_transparency_stack
                .last()
                .copied()
                .unwrap_or(false)
            || self.ctx.blend_mode() != peniko::BlendMode::default()
    }
}

impl<'a> Device<'a> for Renderer {
    fn draw_image(&mut self, image: hayro_interpret::Image<'a, '_>, mut transform: Affine) {
        self.ctx.set_paint_transform(Affine::IDENTITY);
        self.ctx.set_aliasing_threshold(Some(1));

        let target_width = (transform * Point::new(image.width() as f64, 0.0))
            .to_vec2()
            .length()
            .ceil() as u32;
        let target_height = (transform * Point::new(0.0, image.height() as f64))
            .to_vec2()
            .length()
            .ceil() as u32;

        match image {
            hayro_interpret::Image::Stencil(s) => {
                s.with_stencil(
                    |stencil, paint| {
                        transform *= Affine::scale_non_uniform(
                            stencil.scale_factors.0 as f64,
                            stencil.scale_factors.1 as f64,
                        );

                        match paint {
                            Paint::Color(c) => {
                                let color = c.to_rgba().to_rgba8();
                                let (rgb_bytes, alpha) = (
                                    stencil
                                        .data
                                        .iter()
                                        .flat_map(|_| [color[0], color[1], color[2]])
                                        .collect::<Vec<u8>>(),
                                    color[3],
                                );

                                let blend_mode = self.ctx.blend_mode();
                                let push_layer =
                                    alpha != 255 || blend_mode != peniko::BlendMode::default();
                                self.ctx.set_transform(transform);
                                if push_layer {
                                    self.ctx.push_layer(
                                        None,
                                        Some(blend_mode),
                                        Some(alpha as f32 / 255.0),
                                        None,
                                        None,
                                    );
                                }
                                let old_rule = *self.ctx.fill_rule();
                                self.ctx.set_fill_rule(Fill::NonZero);

                                let rgb_data = ImageData::Rgb(RgbData {
                                    data: rgb_bytes,
                                    width: stencil.width,
                                    height: stencil.height,
                                    interpolate: stencil.interpolate,
                                    scale_factors: stencil.scale_factors,
                                });
                                self.draw_image(rgb_data, Some(stencil));

                                if push_layer {
                                    self.ctx.pop_layer();
                                }

                                self.ctx.set_fill_rule(old_rule);
                            }
                            Paint::Pattern(_) => {
                                let (width, height) = (self.ctx.width(), self.ctx.height());
                                let stencil_rect = Rect::new(
                                    0.0,
                                    0.0,
                                    stencil.width as f64,
                                    stencil.height as f64,
                                );
                                let mask_pix = {
                                    let rgb_bytes = ImageData::Rgb(RgbData {
                                        data: vec![
                                            255;
                                            stencil.width as usize
                                                * stencil.height as usize
                                                * 3
                                        ],
                                        width: stencil.width,
                                        height: stencil.height,
                                        interpolate: stencil.interpolate,
                                        scale_factors: stencil.scale_factors,
                                    });
                                    let mut sub_renderer = Self {
                                        ctx: RenderContext::new_with(
                                            width,
                                            height,
                                            derive_settings(self.ctx.render_settings()),
                                        ),
                                        inside_pattern: false,
                                        soft_mask_cache: HashMap::default(),
                                        outline_cache: self.outline_cache.clone(),
                                        cur_mask: None,
                                        in_type3_glyph: false,
                                        scaler: self.scaler,
                                        image_transparency_stack: Vec::new(),
                                    };
                                    let mut sub_pix = Pixmap::new(width, height);
                                    sub_renderer.ctx.set_transform(transform);
                                    sub_renderer.draw_image(rgb_bytes, Some(stencil));
                                    sub_renderer.ctx.flush();
                                    let mut resources = vello_cpu::Resources::default();
                                    sub_renderer
                                        .ctx
                                        .render_to_pixmap(&mut resources, &mut sub_pix);
                                    sub_pix
                                };

                                self.ctx.push_layer(
                                    None,
                                    Some(self.ctx.blend_mode()),
                                    None,
                                    Some(Mask::new_luminance(&mask_pix)),
                                    None,
                                );
                                self.ctx.set_transform(transform);

                                let clip_path =
                                    self.set_paint(paint, &stencil_rect.to_path(0.1), true);
                                if let Some(clip_path) = clip_path.as_ref() {
                                    self.push_clip_path_inner(clip_path, FillRule::NonZero);
                                }
                                self.ctx.fill_rect(&stencil_rect);
                                if clip_path.is_some() {
                                    self.ctx.pop_clip_path();
                                }

                                self.ctx.pop_layer();
                            }
                        };
                    },
                    Some((target_width, target_height)),
                );
            }
            hayro_interpret::Image::Raster(r) => {
                r.with_rgba(
                    |image, alpha| {
                        let (sx, sy) = image.scale_factors();
                        transform *= Affine::scale_non_uniform(sx as f64, sy as f64);
                        self.ctx.set_transform(transform);
                        self.draw_image(image, alpha);
                    },
                    Some((target_width, target_height)),
                );
            }
        }

        self.ctx.set_aliasing_threshold(None);
    }

    fn push_clip_path(&mut self, clip_path: &ClipPath) {
        self.push_clip_path_inner(&clip_path.path, clip_path.fill);
    }

    fn push_transparency_group(
        &mut self,
        opacity: f32,
        mask: Option<SoftMask<'_>>,
        blend_mode: BlendMode,
    ) {
        let settings = *self.ctx.render_settings();
        let force_images_may_have_transparency = mask.is_some()
            || blend_mode != BlendMode::Normal
            || self
                .image_transparency_stack
                .last()
                .copied()
                .unwrap_or(false);
        self.ctx.push_layer(
            None,
            Some(convert_blend_mode(blend_mode)),
            Some(opacity),
            // TODO: Deduplicate
            mask.map(|m| {
                let width = self.ctx.width();
                let height = self.ctx.height();

                self.soft_mask_cache
                    .entry(m.cache_key())
                    .or_insert_with(|| draw_soft_mask(&m, settings, width, height))
                    .clone()
            }),
            None,
        );
        self.image_transparency_stack
            .push(force_images_may_have_transparency);
    }

    fn pop_clip_path(&mut self) {
        self.ctx.pop_clip_path();
    }

    fn pop_transparency_group(&mut self) {
        self.image_transparency_stack.pop();
        self.ctx.pop_layer();
    }

    fn set_soft_mask(&mut self, mask: Option<SoftMask<'_>>) {
        let settings = *self.ctx.render_settings();
        self.cur_mask = mask.map(|m| {
            let width = self.ctx.width();
            let height = self.ctx.height();

            self.soft_mask_cache
                .entry(m.cache_key())
                .or_insert_with(|| draw_soft_mask(&m, settings, width, height))
                .clone()
        });
        if let Some(mask) = self.cur_mask.clone() {
            self.ctx.set_mask(mask);
        } else {
            self.ctx.reset_mask();
        }
    }

    fn draw_path(
        &mut self,
        path: &BezPath,
        transform: Affine,
        paint: &Paint<'_>,
        draw_mode: &PathDrawMode,
    ) {
        match draw_mode {
            PathDrawMode::Fill(f) => {
                Self::fill_path(self, path, transform, paint, *f);
            }
            PathDrawMode::Stroke(s) => {
                Self::stroke_path(self, path, transform, paint, s, false);
            }
        }
    }

    fn draw_rect(
        &mut self,
        rect: &Rect,
        transform: Affine,
        paint: &Paint<'_>,
        draw_mode: &PathDrawMode,
    ) {
        let path = rect.to_path(0.1);
        match draw_mode {
            PathDrawMode::Fill(fill_rule) => {
                self.ctx.set_fill_rule(convert_fill_rule(*fill_rule));
                self.ctx.set_transform(transform);
                // PdfCraft patch: see `may_paint_canvas`.
                if !may_paint_canvas(&path, transform, 0.0, self.ctx.width(), self.ctx.height()) {
                    return;
                }

                let clip_path = self.set_paint(paint, &path, false);
                if let Some(clip_path) = clip_path.as_ref() {
                    self.push_clip_path_inner(clip_path, *fill_rule);
                }

                self.ctx.fill_rect(rect);

                if clip_path.is_some() {
                    self.ctx.pop_clip_path();
                }
            }
            PathDrawMode::Stroke(s) => {
                Self::stroke_path(self, &path, transform, paint, s, false);
            }
        }
    }

    fn draw_glyph(
        &mut self,
        glyph: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        paint: &Paint<'a>,
        draw_mode: &GlyphDrawMode,
    ) {
        match draw_mode {
            GlyphDrawMode::Fill => {
                Self::fill_glyph(self, glyph, transform, glyph_transform, paint);
            }
            GlyphDrawMode::Stroke(s) => {
                Self::stroke_glyph(self, glyph, transform, glyph_transform, paint, s);
            }
            GlyphDrawMode::Invisible => {
                // Don't render invisible text for visual output
            }
        }
    }

    fn set_blend_mode(&mut self, blend_mode: BlendMode) {
        self.ctx.set_blend_mode(convert_blend_mode(blend_mode));
    }
}

// TODO: Deduplicate with hayro-svg?
fn render_shading_texture(
    path_bbox: Rect,
    shading_pattern: &EncodedShadingPattern,
) -> (Vec<PremulRgba8>, u32, u32, Affine, bool) {
    let base_width = (path_bbox.width() as f32).max(1.0);
    let base_height = (path_bbox.height() as f32).max(1.0);

    let width = (base_width).ceil() as u32;
    let height = (base_height).ceil() as u32;

    let (x_advance, y_advance) = x_y_advances(&shading_pattern.base_transform);

    let mut buf = vec![PremulRgba8::from_u32(0); width as usize * height as usize];
    let mut start_point = shading_pattern.base_transform
        * Affine::translate((0.5, 0.5))
        * Point::new(path_bbox.x0, path_bbox.y0);
    let mut may_have_transparency = false;

    for row in buf.chunks_exact_mut(width as usize) {
        let mut point = start_point;

        for pixel in row {
            let sample = shading_pattern.sample(point);
            *pixel = AlphaColor::<Srgb>::new(sample).premultiply().to_rgba8();
            may_have_transparency |= pixel.a != 255;

            point += x_advance;
        }

        start_point += y_advance;
    }

    (
        buf,
        width,
        height,
        Affine::translate((path_bbox.x0, path_bbox.y0)),
        may_have_transparency,
    )
}

fn draw_soft_mask(mask: &SoftMask<'_>, settings: RenderSettings, width: u16, height: u16) -> Mask {
    let mut renderer = Renderer {
        ctx: RenderContext::new_with(width, height, derive_settings(&settings)),
        inside_pattern: false,
        cur_mask: None,
        soft_mask_cache: HashMap::default(),
        outline_cache: Rc::new(std::cell::RefCell::new(HashMap::new())),
        in_type3_glyph: false,
        scaler: Scaler::new(ResamplingFunction::CatmullRom),
        image_transparency_stack: Vec::new(),
    };

    let bg_color = mask.background_color().to_rgba();
    let apply_bg = bg_color.to_rgba8() != BLACK.to_rgba8().to_u8_array();

    if apply_bg {
        renderer
            .ctx
            .set_paint(AlphaColor::<Srgb>::new(bg_color.components()));
        renderer
            .ctx
            .fill_rect(&Rect::new(0.0, 0.0, width as f64, height as f64));
        renderer.ctx.push_layer(None, None, None, None, None);
    }

    mask.interpret(&mut renderer);

    if apply_bg {
        renderer.ctx.pop_layer();
    }

    let mut pix = Pixmap::new(width, height);
    renderer.ctx.flush();
    let mut resources = vello_cpu::Resources::default();
    renderer.ctx.render_to_pixmap(&mut resources, &mut pix);

    let mut rendered_mask = match mask.mask_type() {
        MaskType::Luminosity => Mask::new_luminance(&pix),
        MaskType::Alpha => Mask::new_alpha(&pix),
    };

    if let Some(transfer_function) = mask.transfer_function() {
        let mut map = Vec::new();

        for y in 0..rendered_mask.height() {
            for x in 0..rendered_mask.width() {
                map.push(
                    (transfer_function.apply(rendered_mask.sample(x, y) as f32 / 255.0) * 255.0
                        + 0.5) as u8,
                );
            }
        }

        rendered_mask = Mask::from_parts(map, rendered_mask.width(), rendered_mask.height());
    }

    rendered_mask
}

pub(crate) fn max_factor(transform: &Affine) -> f32 {
    let scale_skew_transform = {
        let c = transform.as_coeffs();
        Affine::new([c[0], c[1], c[2], c[3], 0.0, 0.0])
    };

    let x_advance = scale_skew_transform * Point::new(1.0, 0.0);
    let y_advance = scale_skew_transform * Point::new(0.0, 1.0);

    x_advance
        .to_vec2()
        .length()
        .max(y_advance.to_vec2().length()) as f32
}

pub(crate) fn x_y_advances(transform: &Affine) -> (Vec2, Vec2) {
    let scale_skew_transform = {
        let c = transform.as_coeffs();
        Affine::new([c[0], c[1], c[2], c[3], 0.0, 0.0])
    };

    let x_advance = scale_skew_transform * Point::new(1.0, 0.0);
    let y_advance = scale_skew_transform * Point::new(0.0, 1.0);

    (
        Vec2::new(x_advance.x, x_advance.y),
        Vec2::new(y_advance.x, y_advance.y),
    )
}

fn convert_fill_rule(fill_rule: FillRule) -> Fill {
    match fill_rule {
        FillRule::NonZero => Fill::NonZero,
        FillRule::EvenOdd => Fill::EvenOdd,
    }
}

fn convert_blend_mode(blend_mode: BlendMode) -> peniko::BlendMode {
    let mix = match blend_mode {
        BlendMode::Normal => Mix::Normal,
        BlendMode::Multiply => Mix::Multiply,
        BlendMode::Screen => Mix::Screen,
        BlendMode::Overlay => Mix::Overlay,
        BlendMode::Darken => Mix::Darken,
        BlendMode::Lighten => Mix::Lighten,
        BlendMode::ColorDodge => Mix::ColorDodge,
        BlendMode::ColorBurn => Mix::ColorBurn,
        BlendMode::HardLight => Mix::HardLight,
        BlendMode::SoftLight => Mix::SoftLight,
        BlendMode::Difference => Mix::Difference,
        BlendMode::Exclusion => Mix::Exclusion,
        BlendMode::Hue => Mix::Hue,
        BlendMode::Saturation => Mix::Saturation,
        BlendMode::Color => Mix::Color,
        BlendMode::Luminosity => Mix::Luminosity,
    };

    peniko::BlendMode::new(mix, Compose::SrcOver)
}

/// PdfCraft patch: the scale for a tiling cell, lowered so that the cell pixmap (`|step| ×
/// scale` pixels a side) stays within `max_pixels`. Geometry is unchanged (the pattern transform
/// is derived from the scale); an over-large step only renders at lower resolution.
pub fn tiling_cell_scale(scale: f32, step: f32, max_pixels: f32) -> f32 {
    let step = step.abs();
    if step.is_finite() && step > 0.0 && scale * step > max_pixels {
        max_pixels / step
    } else {
        scale
    }
}
