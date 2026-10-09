//! Clearing the pixels of an image under redaction regions. The image is decoded, every pixel
//! whose cell touches a region is set to zero (no ink for image masks), and the result is
//! written as a new Flate image; the original object is untouched (other pages may use it).
//! An `/SMask` or stencil `/Mask` image is cleared the same way (into a copy, made transparent
//! where it covers a region); a colour-key `/Mask`, `/Alternates` and the other entries that can
//! carry the original pixels (`/SMaskInData`, `/OPI`, `/Metadata`, `/Thumb`) are dropped from the
//! copy. Images whose codec PdfCraft can't decode (DCT, JPX, JBIG2, CCITT), or that have such
//! a mask, can't be cleared, and the caller removes the whole image instead; that is fail-closed.

use pdfcraft_content::{Matrix, overlaps};
use pdfcraft_cos::{Document, Object, Stream};

use crate::limits::{EDGE_EPS, MAX_IMAGE_BYTES, MAX_PIXELS};

/// Slack for producers that write a few bytes more than the image needs.
const SLACK: usize = 4096;

/// Which role a stream plays: colour samples, or a mask whose cleared samples must hide.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Colour,
    /// An image mask or explicit `/Mask`: a 1 sample is masked out.
    Stencil,
    /// An `/SMask`: 0 is transparent.
    Soft,
}

fn components(doc: &Document, s: &Stream) -> Option<usize> {
    let cs = doc.resolve(s.dict.get(b"ColorSpace")?);
    let cs = &*cs;
    let name = match cs {
        Object::Name(n) => n.as_slice(),
        Object::Array(a) => a.first()?.as_name()?,
        _ => return None,
    };
    Some(match name {
        b"DeviceGray" | b"G" | b"CalGray" | b"Indexed" | b"I" | b"Separation" => 1,
        b"DeviceN" => {
            // One component per colorant name.
            let n = doc.resolve(cs.as_array()?.get(1)?).as_array()?.len();
            if n == 0 || n > 32 {
                return None;
            }
            n
        }
        b"DeviceRGB" | b"RGB" | b"CalRGB" | b"Lab" => 3,
        b"DeviceCMYK" | b"CMYK" => 4,
        b"ICCBased" => {
            // /N lives in the ICC profile stream.
            match &*doc.resolve(cs.as_array()?.get(1)?) {
                Object::Stream(icc) => usize::try_from(icc.dict.int(b"N")?).ok().filter(|n| (1..=32).contains(n))?,
                _ => return None,
            }
        }
        _ => return None,
    })
}

/// `Ok(Some(copy))` with the covered pixels cleared, `Ok(None)` when no pixel is covered,
/// `Err(())` when the image can't be cleared (the caller removes it). The copy has no entry that
/// could bring the cleared pixels back; masks that carry pixels are cleared into copies of their
/// own, so shared mask objects stay as they are.
#[allow(clippy::result_unit_err)]
pub(crate) fn clear(doc: &mut Document, s: &Stream, ctm: &Matrix, rects: &[[f64; 4]]) -> Result<Option<Stream>, ()> {
    let kind = if matches!(s.dict.get(b"ImageMask"), Some(Object::Bool(true))) { Kind::Stencil } else { Kind::Colour };
    let main = clear_samples(doc, s, ctm, rects, kind).ok_or(())?;
    // The masks, resolved before anything is added to the document.
    let soft = mask_stream(doc, s, b"SMask");
    let hard = mask_stream(doc, s, b"Mask");
    let soft = match soft {
        Some(m) => Some(clear_samples(doc, &m, ctm, rects, Kind::Soft).ok_or(())?),
        None => None,
    };
    let hard = match hard {
        Some(m) => Some(clear_samples(doc, &m, ctm, rects, Kind::Stencil).ok_or(())?),
        None => None,
    };
    let changed = main.is_some() || soft.as_ref().is_some_and(Option::is_some) || hard.as_ref().is_some_and(Option::is_some);
    if !changed {
        return Ok(None);
    }
    let mut out = main.unwrap_or_else(|| s.clone());
    for key in [&b"SMaskInData"[..], b"Alternates", b"OPI", b"Metadata", b"Thumb"] {
        out.dict.remove(key);
    }
    // An unclearable mask (not a stream: a colour-key array, or damaged) goes; the others point
    // at their cleared copy, or keep the shared original when nothing of it was covered.
    for (key, cleared) in [(&b"SMask"[..], soft), (b"Mask", hard)] {
        match cleared {
            Some(Some(copy)) => out.dict.set(key.to_vec(), Object::Ref(doc.add(Object::Stream(copy)))),
            Some(None) => {}
            None => {
                out.dict.remove(key);
            }
        }
    }
    Ok(Some(out))
}

/// The stream behind `/SMask` or `/Mask` (`None` for anything else, including a colour-key array).
fn mask_stream(doc: &Document, s: &Stream, key: &[u8]) -> Option<Stream> {
    match &*doc.resolve(s.dict.get(key)?) {
        Object::Stream(m) => Some(m.clone()),
        _ => None,
    }
}

fn clear_samples(doc: &Document, s: &Stream, ctm: &Matrix, rects: &[[f64; 4]], kind: Kind) -> Option<Option<Stream>> {
    let supported = |n: &[u8]| {
        matches!(
            n,
            b"FlateDecode" | b"Fl" | b"LZWDecode" | b"LZW" | b"ASCII85Decode" | b"A85" | b"ASCIIHexDecode" | b"AHx" | b"RunLengthDecode" | b"RL"
        )
    };
    let ok = match s.dict.get(b"Filter") {
        None => true,
        Some(Object::Name(n)) => supported(n),
        Some(Object::Array(a)) => a.iter().all(|f| f.as_name().is_some_and(supported)),
        _ => false,
    };
    if !ok {
        return None;
    }
    let (w, h) = (u64::try_from(s.dict.int(b"Width")?).ok()?, u64::try_from(s.dict.int(b"Height")?).ok()?);
    if w == 0 || h == 0 || w.checked_mul(h)? > MAX_PIXELS {
        return None;
    }
    let mask = kind == Kind::Stencil || matches!(s.dict.get(b"ImageMask"), Some(Object::Bool(true)));
    let (ncomp, bpc) = if mask { (1, 1) } else { (components(doc, s)? as u64, u64::try_from(s.dict.int(b"BitsPerComponent")?).ok()?) };
    if !matches!(bpc, 1 | 2 | 4 | 8 | 16) {
        return None;
    }
    let row = w.checked_mul(ncomp)?.checked_mul(bpc)?.div_ceil(8);
    let size = row.checked_mul(h)?;
    if size > MAX_IMAGE_BYTES {
        return None;
    }
    let (w, h, row, size) = (usize::try_from(w).ok()?, usize::try_from(h).ok()?, usize::try_from(row).ok()?, usize::try_from(size).ok()?);
    let mut data = s.decoded_within(size.checked_add(SLACK)?).ok()?;
    if data.len() < size {
        return None;
    }
    data.truncate(size);
    // Which sample value hides a cleared pixel: a stencil's 1 (a 0 paints unless /Decode is
    // [1 0]), an soft mask's transparent end (0 unless /Decode is [1 0]), no ink / black
    // otherwise. Index 0 of an Indexed palette and L = a = b = 0 of a Lab image are such fills:
    // fixed values that say nothing about the original pixel.
    let inverted = s.dict.get(b"Decode").and_then(Object::as_array).and_then(|d| d.first()).and_then(Object::as_f64) == Some(1.0);
    let ones = match kind {
        Kind::Stencil => !inverted,
        Kind::Soft => inverted,
        Kind::Colour => mask && !inverted,
    };
    let bits_per_pixel = usize::try_from(ncomp * bpc).ok()?;
    let mut cleared = 0usize;
    for j in 0..h {
        // Image space: (0,0) is the bottom-left of the unit square; row 0 is the top.
        let (v0, v1) = (1.0 - (j + 1) as f64 / h as f64, 1.0 - j as f64 / h as f64);
        let row_box = ctm.bbox([0.0, v0, 1.0, v1]);
        if !rects.iter().any(|r| overlaps(*r, row_box, EDGE_EPS)) {
            continue;
        }
        for i in 0..w {
            // The pixel's cell: any overlap with a region clears the pixel.
            let (u0, u1) = (i as f64 / w as f64, (i + 1) as f64 / w as f64);
            let cell = ctm.bbox([u0, v0, u1, v1]);
            if !rects.iter().any(|r| overlaps(*r, cell, EDGE_EPS)) {
                continue;
            }
            cleared += 1;
            // Checked: on a 32-bit target (wasm) the bit offset of a large image can overflow.
            let start = j.checked_mul(row)?.checked_mul(8)?.checked_add(i.checked_mul(bits_per_pixel)?)?;
            for bit in start..start.checked_add(bits_per_pixel)? {
                let (byte, shift) = (bit / 8, 7 - bit % 8);
                if let Some(b) = data.get_mut(byte) {
                    if ones {
                        *b |= 1 << shift;
                    } else {
                        *b &= !(1 << shift);
                    }
                }
            }
        }
    }
    if cleared == 0 {
        return Some(None);
    }
    let mut dict = s.dict.clone();
    dict.remove(b"Length");
    dict.remove(b"DecodeParms");
    Some(Some(Stream::flate(dict, &data)))
}
