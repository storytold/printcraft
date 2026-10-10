//! Image resampling and recompression.

use std::collections::{HashMap, HashSet};

use pdfcraft_content::Matrix;
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::{Compression, ImageSettings, OptimizeError, Report, Settings, Stage};

/// Images with more pixels than this are left alone (memory).
const MAX_PIXELS: u64 = 80_000_000;

/// The resources of a page, inherited from the page tree if the page has none.
fn page_resources(doc: &Document, page: ObjRef) -> Option<Dict> {
    let mut node = Some(page);
    for _ in 0..64 {
        let d = doc.get(node?).as_dict()?.clone();
        if let Some(r) = d.get(b"Resources") {
            return doc.resolve(r).as_dict().cloned();
        }
        node = d.reference(b"Parent");
    }
    None
}

fn content_bytes(doc: &Document, contents: &Object) -> Vec<u8> {
    match &*doc.resolve(contents) {
        Object::Stream(s) => s.decoded().unwrap_or_default(),
        Object::Array(a) => a
            .iter()
            .flat_map(|c| match &*doc.resolve(c) {
                Object::Stream(s) => {
                    let mut v = s.decoded().unwrap_or_default();
                    v.push(b'\n');
                    v
                }
                _ => Vec::new(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Walk one content stream, recording each image's displayed resolution (pixels per inch, the
/// smaller of the two axes) and descending into form XObjects.
fn walk(doc: &Document, data: &[u8], resources: &Dict, ctm: Matrix, depth: usize, seen: &mut HashSet<ObjRef>, out: &mut HashMap<ObjRef, f64>) {
    if depth > 12 {
        return;
    }
    let xobjects = resources.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default();
    let mut stack = vec![ctm];
    for op in pdfcraft_content::parse(data).ops {
        let top = stack.last().copied().unwrap_or(ctm);
        match op.op.as_slice() {
            b"q" => stack.push(top),
            b"Q" if stack.len() > 1 => {
                stack.pop();
            }
            b"cm" => {
                if let Some(m) = Matrix::from_operands(&op.operands)
                    && let Some(t) = stack.last_mut()
                {
                    *t = m.then(&top);
                }
            }
            b"Do" => {
                let Some(name) = op.name(0) else { continue };
                let Some(r) = xobjects.get(name).and_then(Object::as_ref) else { continue };
                let obj = doc.get(r);
                let Object::Stream(s) = &*obj else { continue };
                match s.dict.name(b"Subtype") {
                    Some(b"Image") => {
                        let (w, h) = (s.dict.int(b"Width").unwrap_or(0) as f64, s.dict.int(b"Height").unwrap_or(0) as f64);
                        let [a, b, c, d, ..] = top.0;
                        let (sx, sy) = ((a * a + b * b).sqrt(), (c * c + d * d).sqrt());
                        if w > 0.0 && h > 0.0 && sx > 1e-6 && sy > 1e-6 {
                            let ppi = (w / (sx / 72.0)).min(h / (sy / 72.0));
                            let e = out.entry(r).or_insert(f64::INFINITY);
                            *e = e.min(ppi);
                        }
                    }
                    Some(b"Form") => {
                        if !seen.insert(r) && depth > 4 {
                            continue;
                        }
                        let m = s.dict.get(b"Matrix").map(|m| doc.resolve(m)).and_then(|m| m.as_array().map(|a| Matrix::from_operands(a))).flatten();
                        let res =
                            s.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_else(|| resources.clone());
                        let inner = m.unwrap_or(Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])).then(&top);
                        walk(doc, &s.decoded().unwrap_or_default(), &res, inner, depth + 1, seen, out);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// The effective resolution (ppi) of every image drawn by `pages`: the smallest over its uses.
pub fn effective_resolutions(doc: &Document, pages: &[ObjRef]) -> HashMap<ObjRef, f64> {
    let mut out = HashMap::new();
    let mut seen = HashSet::new();
    for p in pages {
        let Some(d) = doc.get(*p).as_dict().cloned() else { continue };
        let Some(contents) = d.get(b"Contents") else { continue };
        let res = page_resources(doc, *p).unwrap_or_default();
        walk(doc, &content_bytes(doc, contents), &res, Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]), 0, &mut seen, &mut out);
    }
    out
}

/// Colour components of a colour space PdfCraft resamples: gray (1) or RGB (3).
fn components(doc: &Document, cs: &Object) -> Option<usize> {
    let cs = doc.resolve(cs);
    match &*cs {
        Object::Name(n) => match n.as_slice() {
            b"DeviceGray" | b"G" => Some(1),
            b"DeviceRGB" | b"RGB" => Some(3),
            _ => None,
        },
        Object::Array(a) => match a.first()?.as_name()? {
            b"ICCBased" => match &*doc.resolve(a.get(1)?) {
                Object::Stream(icc) => match icc.dict.int(b"N")? {
                    1 => Some(1),
                    3 => Some(3),
                    _ => None,
                },
                _ => None,
            },
            b"CalGray" => Some(1),
            b"CalRGB" => Some(3),
            _ => None,
        },
        _ => None,
    }
}

fn is_dct(s: &Stream) -> Option<bool> {
    match s.dict.get(b"Filter") {
        None => Some(false),
        Some(Object::Name(n)) => match n.as_slice() {
            b"DCTDecode" | b"DCT" => Some(true),
            b"FlateDecode" | b"Fl" | b"LZWDecode" | b"LZW" | b"RunLengthDecode" | b"RL" | b"ASCII85Decode" | b"A85" | b"ASCIIHexDecode" | b"AHx" => {
                Some(false)
            }
            _ => None,
        },
        Some(Object::Array(a)) => {
            let names: Vec<&[u8]> = a.iter().filter_map(Object::as_name).collect();
            if names.len() != a.len() {
                return None;
            }
            match names.as_slice() {
                [b"DCTDecode" | b"DCT"] => Some(true),
                n if n.iter().all(|f| {
                    matches!(
                        *f,
                        b"FlateDecode"
                            | b"Fl"
                            | b"LZWDecode"
                            | b"LZW"
                            | b"RunLengthDecode"
                            | b"RL"
                            | b"ASCII85Decode"
                            | b"A85"
                            | b"ASCIIHexDecode"
                            | b"AHx"
                    )
                }) =>
                {
                    Some(false)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// 8-bit samples of an image (`n` components), decoded.
fn pixels(s: &Stream, n: usize) -> Option<Vec<u8>> {
    let (w, h) = (s.dict.int(b"Width")? as usize, s.dict.int(b"Height")? as usize);
    if is_dct(s)? {
        let img = image::load_from_memory_with_format(&s.raw, image::ImageFormat::Jpeg).ok()?;
        // Adobe CMYK/YCCK JPEGs aren't handled (the colour space check excludes them).
        let v = if n == 1 { img.into_luma8().into_raw() } else { img.into_rgb8().into_raw() };
        (v.len() == w * h * n).then_some(v)
    } else {
        let v = s.decoded().ok()?;
        (v.len() >= w * h * n).then(|| v[..w * h * n].to_vec())
    }
}

fn resample(px: &[u8], n: usize, w: u32, h: u32, nw: u32, nh: u32) -> Option<Vec<u8>> {
    use image::imageops::{FilterType, resize};
    Some(match n {
        1 => resize(&image::GrayImage::from_raw(w, h, px.to_vec())?, nw, nh, FilterType::CatmullRom).into_raw(),
        _ => resize(&image::RgbImage::from_raw(w, h, px.to_vec())?, nw, nh, FilterType::CatmullRom).into_raw(),
    })
}

fn jpeg(px: &[u8], n: usize, w: u32, h: u32, quality: u8) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
    let ty = if n == 1 { image::ExtendedColorType::L8 } else { image::ExtendedColorType::Rgb8 };
    enc.encode(px, w, h, ty).ok()?;
    Some(out)
}

/// Whether `decode` is the default decode array for `n` components ([0 1] each), which changes
/// nothing. Scanners write it on page images (#490).
fn is_identity_decode(doc: &Document, decode: &Object, n: usize) -> bool {
    let decode = doc.resolve(decode);
    let Some(a) = decode.as_array() else { return false };
    a.len() == 2 * n && a.chunks(2).all(|p| matches!(p, [lo, hi] if doc.resolve(lo).as_f64() == Some(0.0) && doc.resolve(hi).as_f64() == Some(1.0)))
}

/// A new image stream from 8-bit samples: the original dictionary with the new size and filter.
/// A decode array is dropped: only images without one, or with the default one, get here.
fn image_stream(old: &Dict, px: &[u8], n: usize, w: u32, h: u32, compression: Compression, was_dct: bool) -> Option<Stream> {
    let mut d = old.clone();
    for k in [&b"Filter"[..], b"DecodeParms", b"Decode", b"Length", b"DL"] {
        d.remove(k);
    }
    d.set(b"Width".to_vec(), Object::Int(w as i64));
    d.set(b"Height".to_vec(), Object::Int(h as i64));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    let use_jpeg = match compression {
        Compression::Jpeg(_) => true,
        Compression::Flate => false,
        Compression::Retain => was_dct,
    };
    if use_jpeg {
        let q = match compression {
            Compression::Jpeg(q) => q,
            _ => 80,
        };
        let data = jpeg(px, n, w, h, q)?;
        d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
        Some(Stream::from_raw(d, data))
    } else {
        Some(Stream::flate(d, px))
    }
}

/// One image: resample and/or recompress per `settings`; `ppi` is its effective resolution.
/// Returns the new image and its new soft mask, if the result is smaller.
fn process(doc: &Document, s: &Stream, ppi: f64, settings: &ImageSettings, n: usize) -> Option<(Stream, Option<(ObjRef, Stream)>)> {
    let d = &s.dict;
    if d.get(b"Decode").is_some_and(|a| !is_identity_decode(doc, a, n))
        || d.contains(b"Mask")
        || d.name(b"ImageMask").is_some()
        || matches!(d.get(b"ImageMask"), Some(Object::Bool(true)))
    {
        return None;
    }
    if d.int(b"BitsPerComponent") != Some(8) {
        return None;
    }
    let was_dct = is_dct(s)?;
    let (w, h) = (d.int(b"Width")? as u32, d.int(b"Height")? as u32);
    if w == 0 || h == 0 || (w as u64) * (h as u64) > MAX_PIXELS {
        return None;
    }
    let resize = settings.downsample && ppi.is_finite() && ppi > settings.above_ppi && settings.target_ppi > 0.0;
    let recompress = matches!(settings.compression, Compression::Jpeg(_)) || (matches!(settings.compression, Compression::Flate) && was_dct);
    if !resize && !recompress {
        return None;
    }
    // Tiny images (icons, rules) don't benefit and JPEG would blur them.
    if !resize && (w as u64) * (h as u64) < 64 * 64 {
        return None;
    }
    let (nw, nh) = if resize {
        let k = settings.target_ppi / ppi;
        (((w as f64 * k).round() as u32).max(1), ((h as f64 * k).round() as u32).max(1))
    } else {
        (w, h)
    };
    let px = pixels(s, n)?;
    let px = if (nw, nh) != (w, h) { resample(&px, n, w, h, nw, nh)? } else { px };
    let new = image_stream(d, &px, n, nw, nh, settings.compression, was_dct)?;
    // A soft mask must keep the image's dimensions: resample it with the image (lossless).
    let mut smask = None;
    if let Some(mr) = d.reference(b"SMask") {
        let mobj = doc.get(mr);
        let Object::Stream(m) = &*mobj else { return None };
        if (nw, nh) != (w, h) {
            if m.dict.int(b"BitsPerComponent") != Some(8) || m.dict.contains(b"Matte") || m.dict.contains(b"Decode") {
                return None;
            }
            let (mw, mh) = (m.dict.int(b"Width")? as u32, m.dict.int(b"Height")? as u32);
            let mpx = pixels(m, 1)?;
            let mpx = resample(&mpx, 1, mw, mh, nw, nh)?;
            smask = Some((mr, image_stream(&m.dict, &mpx, 1, nw, nh, Compression::Flate, false)?));
        }
    }
    let before = s.raw.len()
        + smask.as_ref().map_or(0, |(r, _)| match &*doc.get(*r) {
            Object::Stream(m) => m.raw.len(),
            _ => 0,
        });
    let after = new.raw.len() + smask.as_ref().map_or(0, |(_, m)| m.raw.len());
    (after < before).then_some((new, smask))
}

pub(crate) fn run(
    doc: &mut Document,
    pages: &[ObjRef],
    settings: &Settings,
    report: &mut Report,
    progress: &mut dyn FnMut(Stage) -> bool,
) -> Result<(), OptimizeError> {
    let ppi = effective_resolutions(doc, pages);
    // Soft masks are processed with their image, not on their own.
    let masks: HashSet<ObjRef> = ppi
        .keys()
        .filter_map(|r| match &*doc.get(*r) {
            Object::Stream(s) => s.dict.reference(b"SMask"),
            _ => None,
        })
        .collect();
    let mut refs: Vec<(ObjRef, f64)> = ppi.into_iter().filter(|(r, _)| !masks.contains(r)).collect();
    refs.sort_by_key(|(r, _)| r.num);
    let total = refs.len();
    for (done, (r, ppi)) in refs.into_iter().enumerate() {
        if !progress(Stage::Images { done, total }) {
            return Err(OptimizeError::Cancelled);
        }
        let obj = doc.get(r);
        let Object::Stream(s) = &*obj else { continue };
        report.images += 1;
        let Some(n) = s.dict.get(b"ColorSpace").and_then(|cs| components(doc, cs)) else { continue };
        let st = if n == 1 { &settings.gray } else { &settings.color };
        let Some((new, smask)) = process(doc, s, ppi, st, n) else { continue };
        let resized = new.dict.int(b"Width") != s.dict.int(b"Width") || new.dict.int(b"Height") != s.dict.int(b"Height");
        report.image_bytes_before += s.raw.len();
        report.image_bytes_after += new.raw.len();
        if resized {
            report.images_resampled += 1;
        } else {
            report.images_recompressed += 1;
        }
        doc.set(r, Object::Stream(new));
        if let Some((mr, m)) = smask {
            doc.set(mr, Object::Stream(m));
        }
    }
    if !progress(Stage::Images { done: total, total }) {
        return Err(OptimizeError::Cancelled);
    }
    Ok(())
}
