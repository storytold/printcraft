//! A picture in a visible signature, where the signer's name would be large (Acrobat: Configure
//! Signature Appearance ▸ Draw or Image, and typed signatures). Drawn as Fill & Sign draws its
//! signatures: ink smoothed and stroked with round ends, typed outlines filled even-odd, images
//! with their alpha as a soft mask.

use std::sync::Arc;

use pdfcraft_annot::appearance::smooth_segments;
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::SignError;

/// Bounds the content stream a graphic from settings or an agent can produce. A typed name in
/// the script font stays under 262,144 points.
const MAX_POINTS: usize = 300_000;
/// Image limits: a side and the whole picture (the UI keeps signature images under 4 MP).
const MAX_SIDE: u32 = 8192;
const MAX_PIXELS: u64 = 1 << 24;

#[derive(Clone, PartialEq)]
pub enum Graphic {
    /// Drawn strokes, y up, in units where the drawing pad is 1 wide (as Fill & Sign saves them).
    Strokes(Vec<Vec<[f64; 2]>>),
    /// Closed outlines, y up, filled even-odd: a typed signature in the script font.
    Outlines(Vec<Vec<[f64; 2]>>),
    /// Straight (not premultiplied) RGBA pixels, rows from the top.
    Image { width: u32, height: u32, rgba: Arc<Vec<u8>> },
}

/// Sizes only: a graphic can hold megabytes of pixels or points.
impl std::fmt::Debug for Graphic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Graphic::Strokes(s) => write!(f, "Strokes({} strokes, {} points)", s.len(), s.iter().map(Vec::len).sum::<usize>()),
            Graphic::Outlines(c) => write!(f, "Outlines({} contours, {} points)", c.len(), c.iter().map(Vec::len).sum::<usize>()),
            Graphic::Image { width, height, rgba } => write!(f, "Image({width}x{height}, {} bytes)", rgba.len()),
        }
    }
}

fn bad(why: &str) -> SignError {
    SignError::Pdf(format!("the signature graphic {why}"))
}

/// The drawing's bounds `[x0, y0, x1, y1]` over `points`, refusing non-finite or unbounded ones.
fn bounds<'a>(points: impl Iterator<Item = &'a [f64; 2]>) -> Result<[f64; 4], SignError> {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in points {
        if !p.iter().all(|v| v.is_finite()) {
            return Err(bad("has a point that isn't a number"));
        }
        b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
    }
    if !b.iter().all(|v| v.is_finite()) {
        return Err(bad("is empty"));
    }
    if !((b[2] - b[0]).is_finite() && (b[3] - b[1]).is_finite()) {
        return Err(bad("is too large"));
    }
    Ok(b)
}

/// `(scale, x, y)` placing `b` as large as fits in `area` (`[x, y, w, h]`), centred.
fn fit(b: [f64; 4], area: [f64; 4]) -> (f64, f64, f64) {
    // A dot or a straight line has no extent on an axis; it is placed by the other one.
    let (bw, bh) = ((b[2] - b[0]).max(1e-9), (b[3] - b[1]).max(1e-9));
    let k = (area[2] / bw).min(area[3] / bh);
    (k, area[0] + (area[2] - (b[2] - b[0]) * k) / 2.0, area[1] + (area[3] - (b[3] - b[1]) * k) / 2.0)
}

impl Graphic {
    fn check(&self) -> Result<(), SignError> {
        let points = match self {
            Graphic::Strokes(s) => s.iter().map(Vec::len).sum::<usize>(),
            Graphic::Outlines(c) => {
                if !c.iter().any(|c| c.len() > 2) {
                    return Err(bad("has no outline that encloses anything"));
                }
                c.iter().map(Vec::len).sum()
            }
            Graphic::Image { width, height, rgba } => {
                if *width == 0 || *height == 0 || *width > MAX_SIDE || *height > MAX_SIDE || u64::from(*width) * u64::from(*height) > MAX_PIXELS {
                    return Err(bad(&format!("image is {width} x {height} pixels; at most {MAX_SIDE} per side and 16 megapixels")));
                }
                let expected = (*width as usize).checked_mul(*height as usize).and_then(|n| n.checked_mul(4));
                if expected != Some(rgba.len()) {
                    return Err(bad("image's pixels don't match its size"));
                }
                return Ok(());
            }
        };
        if points > MAX_POINTS {
            return Err(bad(&format!("has {points} points; at most {MAX_POINTS}")));
        }
        Ok(())
    }

    /// Check the graphic and draw it into `area` (`[x, y, w, h]` in the appearance's space):
    /// content operators, and the image XObject added to `doc` that they name `/Im1`.
    pub(crate) fn draw(&self, doc: &mut Document, area: [f64; 4]) -> Result<(String, Option<ObjRef>), SignError> {
        self.check()?;
        let n = crate::pdf::fmt;
        match self {
            Graphic::Strokes(strokes) => {
                let curves: Vec<Vec<[(f64, f64); 3]>> =
                    strokes.iter().map(|s| smooth_segments(&s.iter().map(|p| (p[0], p[1])).collect::<Vec<_>>())).collect();
                // Curves stay within their points and control points.
                let controls: Vec<[f64; 2]> = curves.iter().flatten().flat_map(|[a, b, _]| [[a.0, a.1], [b.0, b.1]]).collect();
                let b = bounds(strokes.iter().flatten().chain(&controls))?;
                // Fill & Sign draws 1.5 pt ink on a 150 pt wide pad: 1% of the pad's width.
                let probe = fit(b, area).0;
                let width = (probe * 0.01).clamp(0.3, 4.0);
                let inset = [area[0] + width / 2.0, area[1] + width / 2.0, (area[2] - width).max(0.0), (area[3] - width).max(0.0)];
                let (k, x, y) = fit(b, inset);
                let at = |p: (f64, f64)| format!("{} {}", n(x + (p.0 - b[0]) * k), n(y + (p.1 - b[1]) * k));
                let mut c = format!("q 0 G {} w 1 J 1 j\n", n(width));
                for (s, segments) in strokes.iter().zip(&curves) {
                    let Some(first) = s.first() else { continue };
                    let start = at((first[0], first[1]));
                    c.push_str(&format!("{start} m\n"));
                    if segments.is_empty() {
                        // A dot: a zero-length line, which round caps draw as a disc.
                        c.push_str(&format!("{start} l\n"));
                    }
                    for [p, q, e] in segments {
                        c.push_str(&format!("{} {} {} c\n", at(*p), at(*q), at(*e)));
                    }
                    c.push_str("S\n");
                }
                c.push_str("Q\n");
                Ok((c, None))
            }
            Graphic::Outlines(contours) => {
                let closed: Vec<&Vec<[f64; 2]>> = contours.iter().filter(|c| c.len() > 2).collect();
                let b = bounds(closed.iter().copied().flatten())?;
                let (k, x, y) = fit(b, area);
                let mut c = String::from("q 0 g\n");
                for contour in closed {
                    for (i, p) in contour.iter().enumerate() {
                        c.push_str(&format!("{} {} {}\n", n(x + (p[0] - b[0]) * k), n(y + (p[1] - b[1]) * k), if i == 0 { "m" } else { "l" }));
                    }
                    c.push_str("h\n");
                }
                c.push_str("f*\nQ\n");
                Ok((c, None))
            }
            Graphic::Image { width, height, rgba } => {
                let (w, h) = (f64::from(*width), f64::from(*height));
                let (k, x, y) = fit([0.0, 0.0, w, h], area);
                let image = image_xobject(doc, *width, *height, rgba);
                Ok((format!("q {} 0 0 {} {} {} cm /Im1 Do Q\n", n(w * k), n(h * k), n(x), n(y)), Some(image)))
            }
        }
    }
}

/// An RGB image XObject, with a soft mask when any pixel isn't opaque. `rgba` is checked.
fn image_xobject(doc: &mut Document, width: u32, height: u32, rgba: &[u8]) -> ObjRef {
    let dict = |space: &str| {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("XObject"));
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"Width".to_vec(), Object::Int(i64::from(width)));
        d.set(b"Height".to_vec(), Object::Int(i64::from(height)));
        d.set(b"ColorSpace".to_vec(), Object::name(space));
        d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        d
    };
    let (pixels, _) = rgba.as_chunks::<4>();
    let rgb: Vec<u8> = pixels.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut d = dict("DeviceRGB");
    if pixels.iter().any(|p| p[3] != 255) {
        let alpha: Vec<u8> = pixels.iter().map(|p| p[3]).collect();
        let mask = doc.add(Object::Stream(Stream::flate(dict("DeviceGray"), &alpha)));
        d.set(b"SMask".to_vec(), Object::Ref(mask));
    }
    doc.add(Object::Stream(Stream::flate(d, &rgb)))
}
