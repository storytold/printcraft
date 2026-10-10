use std::sync::Arc;

use pdfcraft_cos::{SaveOptions, write_full};

use super::*;

fn reopen(doc: &Document) -> Document {
    let bytes = write_full(doc, &SaveOptions::default()).unwrap();
    hayro_syntax::Pdf::new(bytes.clone()).expect("parses");
    Document::open(Arc::new(bytes)).unwrap()
}

fn pages(doc: &Document) -> Vec<Dict> {
    let pages = doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap();
    let kids = doc.get(pages).as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap().clone();
    kids.iter().map(|k| doc.resolve(k).as_dict().cloned().unwrap()).collect()
}

fn media(d: &Dict) -> Vec<f64> {
    d.get(b"MediaBox").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect()
}

/// A tiny PNG: 4×2 RGBA with one transparent pixel, 144 dpi.
fn png_bytes(alpha: bool) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, 4, 2);
        enc.set_color(if alpha { png::ColorType::Rgba } else { png::ColorType::Rgb });
        enc.set_depth(png::BitDepth::Eight);
        enc.set_pixel_dims(Some(png::PixelDimensions { xppu: 5669, yppu: 5669, unit: png::Unit::Meter }));
        let mut w = enc.write_header().unwrap();
        let n = if alpha { 4 } else { 3 };
        let mut data = vec![200u8; 8 * n];
        if alpha {
            data[3] = 0;
        }
        w.write_image_data(&data).unwrap();
    }
    out
}

/// A minimal baseline JPEG header (SOI, JFIF at 300 dpi, SOF0 3×2 RGB, EOI). Only the headers
/// matter: the data is embedded as is.
fn jpeg_bytes() -> Vec<u8> {
    let mut v = vec![0xFF, 0xD8];
    v.extend_from_slice(&[0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 1, 1, 0x01, 0x2C, 0x01, 0x2C, 0, 0]);
    v.extend_from_slice(&[0xFF, 0xC0, 0, 17, 8, 0, 2, 0, 3, 3, 1, 0x11, 0, 2, 0x11, 1, 3, 0x11, 1]);
    v.extend_from_slice(&[0xFF, 0xD9]);
    v
}

#[test]
fn blank_documents() {
    let doc = reopen(&blank(612.0, 792.0, 3).unwrap());
    assert_eq!(pages(&doc).len(), 3);
    assert_eq!(media(&pages(&doc)[0]), [0.0, 0.0, 612.0, 792.0]);
    assert!(blank(1.0, 792.0, 1).is_err() && blank(612.0, 792.0, 0).is_err());
}

#[test]
fn images_become_pages_at_their_resolution() {
    let doc =
        from_images(&[("photo.png".into(), png_bytes(true)), ("scan.jpg".into(), jpeg_bytes()), ("flat.png".into(), png_bytes(false))]).unwrap();
    let doc = reopen(&doc);
    let p = pages(&doc);
    assert_eq!(p.len(), 3);
    // 4×2 px at 144 dpi → 2×1 pt; 3×2 px at 300 dpi → 0.72×0.48 pt.
    let m0 = media(&p[0]);
    assert!((m0[2] - 2.0).abs() < 0.01 && (m0[3] - 1.0).abs() < 0.01, "{m0:?}");
    let m1 = media(&p[1]);
    assert!((m1[2] - 0.72).abs() < 0.01, "{m1:?}");
    let img = |d: &Dict| {
        let x = doc.resolve(d.get(b"Resources").unwrap());
        let xo = doc.resolve(x.as_dict().unwrap().get(b"XObject").unwrap());
        doc.resolve(xo.as_dict().unwrap().get(b"Im0").unwrap()).as_dict().cloned().unwrap()
    };
    assert!(img(&p[0]).contains(b"SMask"), "transparency is kept");
    assert!(!img(&p[2]).contains(b"SMask"));
    assert_eq!(img(&p[1]).name(b"Filter"), Some(&b"DCTDecode"[..]));
    assert_eq!(img(&p[1]).name(b"ColorSpace"), Some(&b"DeviceRGB"[..]));
    let title = doc.resolve(doc.trailer().get(b"Info").unwrap());
    assert_eq!(title.as_dict().unwrap().get(b"Title").unwrap().as_string().unwrap().to_text(), "photo");
    assert!(matches!(from_images(&[("x.gif".into(), b"GIF89a".to_vec())]), Err(CreateError::Image(..))));
}

#[test]
fn image_resolution_override_changes_size_without_resampling() {
    let images = [("photo.png".into(), png_bytes(true)), ("scan.jpg".into(), jpeg_bytes())];
    for (dpi, sizes) in [(72.0, [(4.0, 2.0), (3.0, 2.0)]), (300.0, [(0.96, 0.48), (0.72, 0.48)])] {
        let doc = reopen(&from_images_with_resolution(&images, ImageResolution::Dpi(dpi)).unwrap());
        for (page, size) in pages(&doc).iter().zip(sizes) {
            let m = media(page);
            assert!((m[2] - size.0).abs() < 0.001 && (m[3] - size.1).abs() < 0.001);
            let res = doc.resolve(page.get(b"Resources").unwrap());
            let xo = doc.resolve(res.as_dict().unwrap().get(b"XObject").unwrap());
            let img = doc.resolve(xo.as_dict().unwrap().get(b"Im0").unwrap());
            let Object::Stream(stream) = &*img else { panic!("expected an image stream") };
            if stream.dict.name(b"Filter") == Some(&b"DCTDecode"[..]) {
                assert_eq!(*stream.raw, jpeg_bytes());
            } else {
                assert_eq!(stream.dict.int(b"Width"), Some(4));
                assert_eq!(stream.dict.int(b"Height"), Some(2));
                assert!(stream.dict.contains(b"SMask"));
            }
        }
    }
    for dpi in [0.0, -1.0, f64::NAN, f64::INFINITY, 1201.0] {
        assert!(matches!(from_images_with_resolution(&images, ImageResolution::Dpi(dpi)), Err(CreateError::Invalid(_))));
    }
}

#[test]
fn text_is_wrapped_and_paginated() {
    let long: String = (0..200).map(|i| format!("Line {i} of a plain text file\n")).collect();
    let doc = reopen(&from_text("notes", &format!("{long}\u{c}After a form feed"), LETTER, 11.0).unwrap());
    let p = pages(&doc);
    assert!(p.len() >= 5, "{} pages", p.len());
    let content = |d: &Dict| {
        let c = doc.resolve(d.get(b"Contents").unwrap());
        let Object::Stream(s) = &*c else { panic!() };
        String::from_utf8_lossy(&s.decoded().unwrap()).into_owned()
    };
    assert!(content(&p[0]).contains("(Line 0 of a plain text file) Tj"));
    assert!(content(p.last().unwrap()).contains("(After a form feed) Tj"), "a form feed starts a page");
}

fn image_of(doc: &Document, page: usize) -> Dict {
    let p = &pages(doc)[page];
    let res = doc.resolve(p.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
    match &*doc.resolve(xo.get(b"Im0").unwrap()) {
        Object::Stream(s) => s.dict.clone(),
        _ => panic!("not an image"),
    }
}

#[test]
fn bmp_gif_and_multi_page_tiff_images() {
    use image::{ImageEncoder, Rgba, RgbaImage};
    // BMP: 3×2 opaque colour.
    let mut bmp = Vec::new();
    image::codecs::bmp::BmpEncoder::new(&mut bmp).write_image(&[10, 20, 30].repeat(6), 3, 2, image::ExtendedColorType::Rgb8).unwrap();
    // GIF: 2×2 with a transparent pixel.
    let mut gif = Vec::new();
    {
        let mut img = RgbaImage::from_pixel(2, 2, Rgba([255, 0, 0, 255]));
        img.put_pixel(0, 0, Rgba([0, 0, 0, 0]));
        let mut enc = image::codecs::gif::GifEncoder::new(&mut gif);
        enc.encode(img.as_raw(), 2, 2, image::ExtendedColorType::Rgba8).unwrap();
    }
    // TIFF: two pages, an 8-bit gray one at 144 dpi and a 1-bit one.
    let mut tif = std::io::Cursor::new(Vec::new());
    {
        let mut enc = tiff::encoder::TiffEncoder::new(&mut tif).unwrap();
        let mut im = enc.new_image::<tiff::encoder::colortype::Gray8>(4, 2).unwrap();
        im.resolution(tiff::tags::ResolutionUnit::Inch, tiff::encoder::Rational { n: 144, d: 1 });
        im.write_data(&[128; 8]).unwrap();
        enc.write_image::<tiff::encoder::colortype::Gray8>(2, 2, &[0, 255, 255, 0]).unwrap();
    }
    let doc = from_images(&[("a.bmp".into(), bmp), ("b.gif".into(), gif), ("scan.tif".into(), tif.into_inner())]).unwrap();
    let doc = reopen(&doc);
    assert_eq!(pages(&doc).len(), 4, "one page per BMP and GIF, two for the TIFF");
    assert_eq!(media(&pages(&doc)[0]), [0.0, 0.0, 3.0, 2.0]);
    assert_eq!(image_of(&doc, 0).name(b"ColorSpace"), Some(&b"DeviceRGB"[..]));
    assert!(image_of(&doc, 1).contains(b"SMask"), "GIF transparency");
    assert_eq!(media(&pages(&doc)[2]), [0.0, 0.0, 2.0, 1.0], "4×2 px at 144 dpi");
    assert_eq!(image_of(&doc, 2).name(b"ColorSpace"), Some(&b"DeviceGray"[..]));
    assert!(matches!(from_images(&[("x.webp".into(), b"RIFF0000WEBP".to_vec())]), Err(CreateError::Image(..))));
}

/// #665 lifted the tiff crate's 256 MiB decode limit; a few bytes claiming a 60000x60000 page
/// (3.6 GB of gray pixels) must still be refused, not allocated.
#[test]
fn a_tiff_claiming_enormous_dimensions_is_refused() {
    let entries: [(u16, u16, u32); 9] = [
        (256, 4, 60_000), // ImageWidth
        (257, 4, 60_000), // ImageLength
        (258, 3, 8),      // BitsPerSample
        (259, 3, 1),      // Compression: none
        (262, 3, 1),      // BlackIsZero
        (273, 4, 122),    // StripOffsets: just past the IFD
        (277, 3, 1),      // SamplesPerPixel
        (278, 4, 60_000), // RowsPerStrip
        (279, 4, 1),      // StripByteCounts
    ];
    let mut tif = b"II*\0".to_vec();
    tif.extend_from_slice(&8u32.to_le_bytes());
    tif.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (tag, kind, value) in entries {
        tif.extend_from_slice(&tag.to_le_bytes());
        tif.extend_from_slice(&kind.to_le_bytes());
        tif.extend_from_slice(&1u32.to_le_bytes());
        tif.extend_from_slice(&value.to_le_bytes());
    }
    tif.extend_from_slice(&0u32.to_le_bytes());
    assert_eq!(tif.len(), 122);
    tif.push(0);
    assert!(matches!(from_images(&[("huge.tif".into(), tif)]), Err(CreateError::Image(..))));
}

#[test]
fn images_export_as_jpeg_unchanged_and_others_as_png() {
    let doc = reopen(
        &from_images(&[("photo.png".into(), png_bytes(true)), ("scan.jpg".into(), jpeg_bytes()), ("flat.png".into(), png_bytes(false))]).unwrap(),
    );
    let out = extract_images(&doc, &[0, 1, 2], 0);
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    let kinds: Vec<(usize, &str, u32, u32)> = out.images.iter().map(|i| (i.page, i.extension, i.width, i.height)).collect();
    assert_eq!(kinds, [(0, "png", 4, 2), (1, "jpg", 3, 2), (2, "png", 4, 2)]);
    assert_eq!(out.images[1].data, jpeg_bytes(), "JPEG data is written as is");
    // The PNG round-trips the pixels, with the soft mask as alpha.
    let dec = png::Decoder::new(std::io::Cursor::new(out.images[0].data.clone()));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgba);
    assert_eq!(&buf[..8], &[200, 200, 200, 0, 200, 200, 200, 200]);
    // Pages filter; small images can be left out.
    assert_eq!(extract_images(&doc, &[2], 0).images.len(), 1);
    assert!(extract_images(&doc, &[0, 1, 2], 3).images.iter().all(|i| i.width.min(i.height) >= 3));
}

#[test]
fn indexed_and_one_bit_images_decode() {
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    // Replace the page's image with a 4×1 indexed image (two colours) and add a 1-bit mask.
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let res = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
    let (_, r) = xo.iter().next().map(|(k, v)| (k.clone(), v.as_ref().unwrap())).unwrap();
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(4));
    d.set(b"Height".to_vec(), Object::Int(1));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(1));
    d.set(
        b"ColorSpace".to_vec(),
        Object::Array(vec![
            Object::name("Indexed"),
            Object::name("DeviceRGB"),
            Object::Int(1),
            Object::String(PdfString::literal(vec![255, 0, 0, 0, 0, 255])),
        ]),
    );
    doc.set(r, Object::Stream(Stream::from_raw(d, vec![0b0101_0000])));
    let out = extract_images(&doc, &[0], 0);
    let dec = png::Decoder::new(std::io::Cursor::new(out.images[0].data.clone()));
    let mut rd = dec.read_info().unwrap();
    let mut buf = vec![0; rd.output_buffer_size().unwrap()];
    rd.next_frame(&mut buf).unwrap();
    assert_eq!(&buf[..12], &[255, 0, 0, 0, 0, 255, 255, 0, 0, 0, 0, 255]);
}

#[test]
fn jpeg_2000_images_are_embedded_as_is() {
    // A JP2 file: signature, ftyp, jp2h (ihdr 30 × 20, 3 components; resc 5906 px/m ≈ 150 dpi),
    // and a (stub) codestream.
    let bx = |ty: &[u8], payload: &[u8]| {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(ty);
        v.extend_from_slice(payload);
        v
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&20u32.to_be_bytes());
    ihdr.extend_from_slice(&30u32.to_be_bytes());
    ihdr.extend_from_slice(&[0, 3, 7, 7, 0, 0]);
    let mut resc = Vec::new();
    for _ in 0..2 {
        resc.extend_from_slice(&5906u16.to_be_bytes());
        resc.extend_from_slice(&1u16.to_be_bytes());
    }
    resc.extend_from_slice(&[0, 0]);
    let jp2h = [bx(b"ihdr", &ihdr), bx(b"res ", &bx(b"resc", &resc))].concat();
    let file = [JP2_SIGNATURE.to_vec(), bx(b"ftyp", b"jp2 \0\0\0\0jp2 "), bx(b"jp2h", &jp2h), bx(b"jp2c", &[0xFF, 0x4F, 0xFF, 0x51])].concat();
    let doc = reopen(&from_images(&[("photo.jp2".into(), file.clone())]).unwrap());
    let p = &pages(&doc)[0];
    let m = media(p);
    assert!((m[2] - 30.0 * 72.0 / 150.0).abs() < 0.2 && (m[3] - 20.0 * 72.0 / 150.0).abs() < 0.2, "{m:?}");
    let out = extract_images(&doc, &[0], 0);
    assert!(out.images.is_empty() && out.skipped.len() == 1, "JPX can't be exported yet: {:?}", out.skipped);
    // A raw codestream: the size comes from SIZ.
    let mut siz = vec![0xFF, 0x4F, 0xFF, 0x51, 0, 41, 0, 0];
    for v in [64u32, 48, 0, 0] {
        siz.extend_from_slice(&v.to_be_bytes());
    }
    let doc = reopen(&from_images(&[("raw.j2k".into(), siz)]).unwrap());
    assert_eq!(media(&pages(&doc)[0])[2], 64.0);
}

#[test]
fn text_files_are_decoded_by_byte_order_mark_then_utf8_then_code_page() {
    let utf16 = |text: &str, big: bool| -> Vec<u8> {
        let mut v = if big { vec![0xFE, 0xFF] } else { vec![0xFF, 0xFE] };
        for u in text.encode_utf16() {
            v.extend_from_slice(&if big { u.to_be_bytes() } else { u.to_le_bytes() });
        }
        v
    };
    assert_eq!(decode_text("Kış İstanbul".as_bytes()), "Kış İstanbul");
    assert_eq!(decode_text(b"\xEF\xBB\xBFMerhaba"), "Merhaba", "a UTF-8 byte order mark is dropped");
    assert_eq!(decode_text(&utf16("Kış İstanbul\r\nŞubat", false)), "Kış İstanbul\r\nŞubat");
    assert_eq!(decode_text(&utf16("Grüße 日本", true)), "Grüße 日本");
    // Not UTF-8: Windows-1254 for Turkish, 1252 for other Western text (Icelandic included).
    assert_eq!(decode_text(b"K\xFD\xFE \xDDstanbul \xF0\xFC\xE7"), "Kış İstanbul ğüç");
    assert_eq!(decode_text(b"\xDE\xF3r\xF0ur \xE1 \xEDslandi"), "Þórður á íslandi");
    assert_eq!(decode_text(b"caf\xE9 \x80 5"), "café € 5");
    // Odd and broken input never fails.
    assert_eq!(decode_text(&[0xFF, 0xFE, b'A']), "\u{FFFD}");
    assert_eq!(decode_text(b""), "");
}

#[test]
fn source_kind_tells_pdfs_images_and_text_apart() {
    assert_eq!(source_kind("a.bin", b"%PDF-1.7\n"), Some(SourceKind::Pdf));
    assert_eq!(source_kind("a.txt", b"junk before\n%PDF-1.4"), Some(SourceKind::Pdf), "a header after leading junk");
    assert_eq!(source_kind("a", &png_bytes(false)), Some(SourceKind::Image));
    assert_eq!(source_kind("a.txt", &jpeg_bytes()), Some(SourceKind::Image), "bytes win over the name");
    assert_eq!(source_kind("notes.TXT", b"hello"), Some(SourceKind::Text));
    assert_eq!(source_kind("notes.text", b""), Some(SourceKind::Text));
    // Text that merely starts like a BMP stays text; a truncated BMP header is not an image.
    assert_eq!(source_kind("b.txt", b"BMW drivers"), Some(SourceKind::Text));
    assert_eq!(source_kind("b.bmp", b"BM"), None);
    assert_eq!(source_kind("a.docx", b"PK\x03\x04"), None);
    assert_eq!(source_kind("", b""), None);
}

/// A synthetic ICC profile: a 128-byte header (size, device class, colour space, `acsp`) and
/// some payload. Only the header is read.
fn icc_profile(space: &[u8; 4]) -> Vec<u8> {
    let mut p = vec![0u8; 300];
    p[..4].copy_from_slice(&300u32.to_be_bytes());
    p[12..16].copy_from_slice(b"prtr");
    p[16..20].copy_from_slice(space);
    p[36..40].copy_from_slice(b"acsp");
    for (i, b) in p.iter_mut().enumerate().skip(128) {
        *b = i as u8;
    }
    p
}

/// A minimal CMYK JPEG header (SOI, Adobe APP14, `profile` in two APP2 ICC_PROFILE chunks,
/// SOF0 3×2 with four components, EOI).
fn cmyk_jpeg_bytes(profile: &[u8]) -> Vec<u8> {
    let mut v = vec![0xFF, 0xD8];
    v.extend_from_slice(&[0xFF, 0xEE, 0, 14, b'A', b'd', b'o', b'b', b'e', 0, 100, 0, 0, 0, 0, 2]);
    let (a, b) = profile.split_at(profile.len() / 2);
    // Written out of order: the sequence numbers decide.
    for (seq, part) in [(2u8, b), (1, a)] {
        v.extend_from_slice(&[0xFF, 0xE2]);
        v.extend_from_slice(&(2 + 14 + part.len() as u16).to_be_bytes());
        v.extend_from_slice(b"ICC_PROFILE\0");
        v.extend_from_slice(&[seq, 2]);
        v.extend_from_slice(part);
    }
    v.extend_from_slice(&[0xFF, 0xC0, 0, 20, 8, 0, 2, 0, 3, 4, 1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0, 4, 0x11, 0]);
    v.extend_from_slice(&[0xFF, 0xD9]);
    v
}

/// The ICC profile behind an image's colour space: (`/N`, `/Alternate`, profile bytes, object).
fn icc_of(doc: &Document, image: &Dict) -> Option<(i64, Vec<u8>, Vec<u8>, Object)> {
    let cs = image.get(b"ColorSpace")?.as_array()?.clone();
    assert_eq!(cs.first().and_then(Object::as_name), Some(&b"ICCBased"[..]));
    let r = cs.get(1)?.clone();
    let Object::Stream(s) = &*doc.resolve(&r) else { panic!("the ICC profile is not a stream") };
    Some((s.dict.int(b"N")?, s.dict.name(b"Alternate")?.to_vec(), s.decoded().unwrap(), r))
}

/// Move the synthetic fixture's frame before its application segments.
fn jpeg_frame_first(mut bytes: Vec<u8>) -> Vec<u8> {
    let at = bytes.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    let len = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
    let frame: Vec<_> = bytes.drain(at..at + 2 + len).collect();
    bytes.splice(2..2, frame);
    bytes
}

#[test]
fn jpeg_fill_bytes_and_metadata_after_frame_are_preserved() {
    let profile = icc_profile(b"CMYK");
    let mut bytes = jpeg_frame_first(cmyk_jpeg_bytes(&profile));
    // Fill before SOF and APP14, including multiple consecutive fill bytes.
    bytes.splice(24..24, [0xFF, 0xFF]);
    bytes.insert(2, 0xFF);
    let doc = reopen(&from_images(&[("filled.jpg".into(), bytes.clone())]).unwrap());
    let image = image_of(&doc, 0);
    let (n, alt, data, _) = icc_of(&doc, &image).unwrap();
    assert_eq!((n, alt.as_slice(), data), (4, &b"DeviceCMYK"[..], profile));
    assert!(image.contains(b"Decode"), "APP14 after SOF still inverts CMYK");
    assert_eq!(extract_images(&doc, &[0], 0).images[0].data, bytes);

    let embedded = jpeg("density.jpg", &jpeg_frame_first(jpeg_bytes())).unwrap();
    assert_eq!(embedded.px, (3, 2));
    assert_eq!(embedded.dpi, (300.0, 300.0), "JFIF after SOF is read");

    // A mismatched profile found after SOF still falls back leniently, as in #729.
    let bytes = jpeg_frame_first(cmyk_jpeg_bytes(&icc_profile(b"RGB ")));
    let doc = reopen(&from_images(&[("mismatch.jpg".into(), bytes)]).unwrap());
    assert_eq!(image_of(&doc, 0).name(b"ColorSpace"), Some(&b"DeviceCMYK"[..]));
}

#[test]
fn jpeg_segment_scan_stops_at_sos_or_eoi() {
    let profile = icc_profile(b"CMYK");
    for marker in [0xDA, 0xD9] {
        let mut bytes = jpeg_frame_first(cmyk_jpeg_bytes(&profile));
        bytes.truncate(bytes.len() - 2);
        // Neither marker needs a segment length to stop metadata scanning. The trailing
        // invalid APP2 would fail if entropy data or bytes after EOI were walked.
        bytes.extend_from_slice(&[0xFF, 0xFF, marker, 0xFF, 0xE2, 0, 1]);
        let embedded = jpeg("boundary.jpg", &bytes).unwrap();
        assert_eq!(embedded.icc, Some(profile.clone()));
        assert_eq!(embedded.px, (3, 2));
    }
}

#[test]
fn jpeg_segment_scan_rejects_truncated_or_invalid_lengths() {
    for tail in [&[0xFF][..], &[0xFF, 0xFF], &[0xFF, 0xE2], &[0xFF, 0xE2, 0], &[0xFF, 0xE2, 0, 0], &[0xFF, 0xE2, 0, 1], &[0xFF, 0xE2, 0, 3]] {
        let mut bytes = jpeg_bytes();
        bytes.truncate(bytes.len() - 2);
        bytes.extend_from_slice(tail);
        assert!(jpeg("truncated.jpg", &bytes).is_err(), "tail: {tail:?}");
    }
}

#[test]
fn embedded_icc_profiles_tag_the_image_colour_space() {
    // A CMYK JPEG keeps its profile as /ICCBased (N 4), its data byte for byte and its Decode.
    let cmyk = icc_profile(b"CMYK");
    let jpeg = cmyk_jpeg_bytes(&cmyk);
    let rgb = icc_profile(b"RGB ");
    let png = {
        let mut out = Vec::new();
        let mut info = png::Info::with_size(2, 1);
        info.color_type = png::ColorType::Rgb;
        info.bit_depth = png::BitDepth::Eight;
        info.icc_profile = Some(rgb.clone().into());
        let mut w = png::Encoder::with_info(&mut out, info).unwrap().write_header().unwrap();
        w.write_image_data(&[10, 20, 30, 40, 50, 60]).unwrap();
        w.finish().unwrap();
        out
    };
    let doc = reopen(&from_images(&[("a.jpg".into(), jpeg.clone()), ("b.jpg".into(), jpeg.clone()), ("c.png".into(), png)]).unwrap());
    let (n, alt, data, first) = icc_of(&doc, &image_of(&doc, 0)).expect("the CMYK JPEG is ICC-tagged");
    assert_eq!((n, alt.as_slice(), data), (4, &b"DeviceCMYK"[..], cmyk.clone()));
    assert!(image_of(&doc, 0).contains(b"Decode"), "Adobe CMYK stays inverted");
    let p = &pages(&doc)[0];
    let res = doc.resolve(p.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
    let Object::Stream(s) = &*doc.resolve(xo.get(b"Im0").unwrap()) else { panic!() };
    assert_eq!(*s.raw, jpeg, "the JPEG data is embedded as is");
    let (_, _, _, second) = icc_of(&doc, &image_of(&doc, 1)).unwrap();
    assert_eq!(first, second, "images with the same profile share one profile object");
    // A PNG iCCP profile tags an RGB image (N 3).
    let (n, alt, data, _) = icc_of(&doc, &image_of(&doc, 2)).expect("the PNG is ICC-tagged");
    assert_eq!((n, alt.as_slice(), data), (3, &b"DeviceRGB"[..], rgb.clone()));
    // The colour space is still exported as CMYK / RGB.
    let out = extract_images(&doc, &[0, 1, 2], 0);
    assert_eq!(out.images.len(), 3, "{:?}", out.skipped);

    // A profile for another colour space, a damaged one or a missing chunk: the device space.
    let mut no_acsp = cmyk.clone();
    no_acsp[36] = b'x';
    let missing_chunk = {
        let mut v = cmyk_jpeg_bytes(&cmyk);
        // Drop the first APP2 segment (sequence 2 of 2).
        let at = v.windows(2).position(|w| w == [0xFF, 0xE2]).unwrap();
        let len = u16::from_be_bytes([v[at + 2], v[at + 3]]) as usize;
        v.drain(at..at + 2 + len);
        v
    };
    for (what, bytes) in [("an RGB profile", cmyk_jpeg_bytes(&rgb)), ("no acsp", cmyk_jpeg_bytes(&no_acsp)), ("a missing chunk", missing_chunk)] {
        let doc = reopen(&from_images(&[("x.jpg".into(), bytes)]).unwrap());
        assert_eq!(image_of(&doc, 0).name(b"ColorSpace"), Some(&b"DeviceCMYK"[..]), "{what}");
    }

    // image_xobject (stamps, signatures, inserted images) tags its image the same way.
    let mut doc = Document::new_empty();
    let (r, _) = image_xobject(&mut doc, "a.jpg", &jpeg).unwrap();
    let Object::Stream(s) = &*doc.get(r) else { panic!() };
    assert_eq!(icc_of(&doc, &s.dict).map(|(n, ..)| n), Some(4));
}

/// Replaces page 0's image with the stream `image` builds (it can add objects, e.g. a mask).
fn with_image(doc: &mut Document, image: impl FnOnce(&mut Document) -> Stream) {
    let page = pdfcraft_model::pages(doc)[0].clone();
    let res = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
    let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
    let r = xo.iter().next().map(|(_, v)| v.as_ref().unwrap()).unwrap();
    let s = image(doc);
    doc.set(r, Object::Stream(s));
}

fn image_dict(w: i64, h: i64, space: &str, bpc: i64) -> Dict {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(w));
    d.set(b"Height".to_vec(), Object::Int(h));
    d.set(b"ColorSpace".to_vec(), Object::name(space));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(bpc));
    d
}

/// The exported file's extension and its pixels as RGBA.
fn exported_rgba(doc: &Document) -> (&'static str, Vec<u8>) {
    let out = extract_images(doc, &[0], 0);
    assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    let img = &out.images[0];
    if img.extension == "jpg" {
        return ("jpg", image::load_from_memory(&img.data).unwrap().into_rgba8().into_raw());
    }
    let dec = png::Decoder::new(std::io::Cursor::new(img.data.clone()));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.as_chunks::<3>().0.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        other => panic!("unexpected {other:?}"),
    };
    ("png", rgba)
}

fn alphas(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>().0.iter().map(|p| p[3]).collect()
}

/// A 4×2 grey JPEG (mid grey).
fn real_jpeg() -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95).encode(&[128u8; 8], 4, 2, image::ExtendedColorType::L8).unwrap();
    out
}

#[test]
fn jpeg_images_with_a_soft_mask_export_as_png_with_alpha() {
    // Issue #645: a cut-out photo (JPEG with an /SMask) lost its transparent background.
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    with_image(&mut doc, |doc| {
        let mask = Stream::from_raw(image_dict(4, 2, "DeviceGray", 8), vec![0, 255, 0, 255, 255, 0, 255, 0]);
        let m = doc.add(Object::Stream(mask));
        let mut d = image_dict(4, 2, "DeviceGray", 8);
        d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
        d.set(b"SMask".to_vec(), Object::Ref(m));
        Stream::from_raw(d, real_jpeg())
    });
    let (ext, rgba) = exported_rgba(&doc);
    assert_eq!(ext, "png", "a JPEG with transparency is written as a PNG");
    assert_eq!(alphas(&rgba), [0, 255, 0, 255, 255, 0, 255, 0]);
    assert!(rgba.as_chunks::<4>().0.iter().all(|p| p[0].abs_diff(128) <= 4 && p[0] == p[1] && p[1] == p[2]), "{rgba:?}");
    // Without a mask the JPEG is still written as is.
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    with_image(&mut doc, |_| {
        let mut d = image_dict(4, 2, "DeviceGray", 8);
        d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
        Stream::from_raw(d, real_jpeg())
    });
    let out = extract_images(&doc, &[0], 0);
    assert_eq!((out.images[0].extension, &out.images[0].data), ("jpg", &real_jpeg()));
}

#[test]
fn soft_masks_of_another_size_and_stencil_and_colour_key_masks_become_alpha() {
    // A 2×1 soft mask on a 4×2 image is scaled to the image's size.
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    with_image(&mut doc, |doc| {
        let m = doc.add(Object::Stream(Stream::from_raw(image_dict(2, 1, "DeviceGray", 8), vec![0, 255])));
        let mut d = image_dict(4, 2, "DeviceRGB", 8);
        d.set(b"SMask".to_vec(), Object::Ref(m));
        Stream::from_raw(d, vec![10; 24])
    });
    let a = alphas(&exported_rgba(&doc).1);
    assert!(a[0] < 64 && a[3] > 192 && a[4] < 64 && a[7] > 192, "{a:?}");

    // A stencil /Mask: its 1 samples hide the image.
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    with_image(&mut doc, |doc| {
        let mut md = Dict::new();
        md.set(b"Type".to_vec(), Object::name("XObject"));
        md.set(b"Subtype".to_vec(), Object::name("Image"));
        md.set(b"Width".to_vec(), Object::Int(4));
        md.set(b"Height".to_vec(), Object::Int(2));
        md.set(b"ImageMask".to_vec(), Object::Bool(true));
        let m = doc.add(Object::Stream(Stream::from_raw(md, vec![0b1100_0000, 0b0011_0000])));
        let mut d = image_dict(4, 2, "DeviceRGB", 8);
        d.set(b"Mask".to_vec(), Object::Ref(m));
        Stream::from_raw(d, vec![10; 24])
    });
    assert_eq!(alphas(&exported_rgba(&doc).1), [0, 0, 255, 255, 255, 255, 0, 0]);

    // A colour-key /Mask: pure white pixels are transparent.
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    with_image(&mut doc, |_| {
        let mut d = image_dict(2, 1, "DeviceRGB", 8);
        d.set(b"Mask".to_vec(), Object::Array([250, 255, 250, 255, 250, 255].map(Object::Int).to_vec()));
        Stream::from_raw(d, vec![255, 255, 255, 255, 0, 0])
    });
    assert_eq!(exported_rgba(&doc).1, [255, 255, 255, 0, 255, 0, 0, 255]);

    // A soft mask with /Matte: the colours are un-blended from the matte colour.
    let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
    with_image(&mut doc, |doc| {
        let mut md = image_dict(2, 1, "DeviceGray", 8);
        md.set(b"Matte".to_vec(), Object::Array(vec![Object::Int(1)]));
        let m = doc.add(Object::Stream(Stream::from_raw(md, vec![255, 128])));
        let mut d = image_dict(2, 1, "DeviceGray", 8);
        d.set(b"SMask".to_vec(), Object::Ref(m));
        // Black at half alpha, pre-blended with white: 255 + (0 − 255) × 128 / 255 ≈ 127.
        Stream::from_raw(d, vec![0, 127])
    });
    let rgba = exported_rgba(&doc).1;
    assert_eq!(rgba[..4], [0, 0, 0, 255]);
    assert!(rgba[4] <= 2 && rgba[7] == 128, "{rgba:?}");
}

#[test]
fn damaged_masks_and_sizes_never_panic() {
    for (w, h, mw, mh, data) in [(4, 2, -3, 1, vec![]), (4, 2, i64::MAX, 2, vec![0; 8]), (-4, 2, 4, 2, vec![0; 8]), (4, 2, 4, 2, vec![])] {
        let mut doc = from_images(&[("flat.png".into(), png_bytes(false))]).unwrap();
        with_image(&mut doc, |doc| {
            let m = doc.add(Object::Stream(Stream::from_raw(image_dict(mw, mh, "DeviceGray", 8), data.clone())));
            let mut d = image_dict(w, h, "DeviceRGB", 8);
            d.set(b"SMask".to_vec(), Object::Ref(m));
            d.set(b"Mask".to_vec(), Object::Array(vec![Object::Real(f64::NAN), Object::Int(-5)]));
            Stream::from_raw(d, vec![10; 24])
        });
        let page = pdfcraft_model::pages(&doc)[0].clone();
        let res = doc.resolve(page.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap();
        let xo = doc.resolve(res.get(b"XObject").unwrap()).as_dict().cloned().unwrap();
        let r = xo.iter().next().map(|(_, v)| v.as_ref().unwrap()).unwrap();
        let _ = extract_images(&doc, &[0], 0);
        let _ = image_file(&doc, r);
    }
}
