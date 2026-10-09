//! What the output sweep may and may not count as a survivor: binary and structural bytes are
//! not text, and a string that legitimately remains is counted across the whole document.

use pdfcraft_cos::{Dict, Object, Stream};

use super::*;
use crate::tests_verify::{FONT, REGION, attach, mark, open, pdf_bytes, prove_unapplied, run, stream, surface};
use crate::verify::{Surface, Verdict};

/// "Public" at the top and `secret` at x 10..65, y 100..110 of the one page, which is marked.
/// `extra` objects are numbered from 7.
fn one_page_with(secret: &str, extra: Vec<Vec<u8>>, media: &str) -> Document {
    let content = format!("BT /F1 10 Tf 10 200 Td (Public) Tj ET BT /F1 10 Tf 10 100 Td ({secret}) Tj ET");
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox {media} /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>").into_bytes(),
        stream("", content.as_bytes()),
        FONT.as_bytes().to_vec(),
        format!("[{}]", vec!["500"; 95].join(" ")).into_bytes(),
    ];
    objs.extend(extra);
    let mut doc = open(pdf_bytes(objs, ""));
    mark(&mut doc, &[REGION]);
    doc
}

/// Hang the objects numbered `nums` off the catalog so a save keeps them.
fn keep(doc: &mut Document, nums: &[u32]) {
    for &n in nums {
        attach(doc, Object::Ref(ObjRef::new(n, 0)));
    }
}

#[test]
fn a_short_number_does_not_match_a_stream_length() {
    // Another object is a 123-byte stream, so the file says `/Length 123` somewhere.
    let blob = stream("/Type /Extra", &[b'x'; 123]);
    let mut doc = one_page_with("123", vec![blob], "[0 0 300 300]");
    keep(&mut doc, &[7]);
    let (r, proof) = run(&mut doc);
    r.unwrap();
    assert!(proof.passed(), "{}", proof.to_text());
}

#[test]
fn a_short_number_does_not_match_the_page_size() {
    let mut doc = one_page_with("612", vec![], "[0 0 612 792]");
    let (r, proof) = run(&mut doc);
    r.unwrap();
    assert!(proof.passed(), "{}", proof.to_text());
}

#[test]
fn a_short_word_does_not_match_image_or_font_bytes() {
    // Pixel data and an embedded font program are binary: three bytes of them spelling the
    // redacted text prove nothing.
    let mut pixels = vec![0u8; 8];
    pixels.extend_from_slice(b"123");
    pixels.extend_from_slice(&[7u8; 5]);
    let image = stream("/Type /XObject /Subtype /Image /Width 4 /Height 4 /BitsPerComponent 8 /ColorSpace /DeviceGray", &pixels);
    let font = stream("/Length1 16", b"\x00\x01\x00\x00123\x00\x02\x03\x04\x05\x06\x07\x08\x09");
    let mut doc = one_page_with("123", vec![image, font], "[0 0 300 300]");
    keep(&mut doc, &[7, 8]);
    let (r, proof) = run(&mut doc);
    r.unwrap();
    assert!(proof.passed(), "{}", proof.to_text());
}

#[test]
fn a_long_string_in_an_image_stream_is_still_found_as_raw_bytes() {
    // Binary data is not exempt from long needles: a whole redacted sentence in it is residue.
    let pixels = b"\x00\x00SECRETVALUE\x00\x00";
    let image = stream("/Type /XObject /Subtype /Image /Width 4 /Height 4 /BitsPerComponent 8 /ColorSpace /DeviceGray", pixels);
    let mut doc = one_page_with("SECRETVALUE", vec![image], "[0 0 300 300]");
    keep(&mut doc, &[7]);
    let (r, proof) = run(&mut doc);
    assert!(matches!(r, Err(RedactError::Residue(n)) if n > 0), "{r:?}");
    assert!(!proof.passed());
}

/// Three pages: the secret in the marked region of page 1, and on page 3 either a copy of it or
/// something else.
fn three_pages(copy_on_three: bool) -> Document {
    let secret = "BT /F1 10 Tf 10 100 Td (SECRETVALUE) Tj ET";
    let other = if copy_on_three { secret } else { "BT /F1 10 Tf 10 200 Td (Public) Tj ET" };
    let page = |contents: u32| {
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents {contents} 0 R /Resources << /Font << /F1 9 0 R >> >> >>")
            .into_bytes()
    };
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".to_vec(),
        page(6),
        page(7),
        page(8),
        stream("", secret.as_bytes()),
        stream("", b"BT /F1 10 Tf 10 200 Td (Page two) Tj ET"),
        stream("", other.as_bytes()),
        FONT.replace("6 0 R", "10 0 R").into_bytes(),
        format!("[{}]", vec!["500"; 95].join(" ")).into_bytes(),
    ];
    let mut doc = open(pdf_bytes(objs, ""));
    mark(&mut doc, &[REGION]);
    doc
}

#[test]
fn a_copy_that_remains_elsewhere_does_not_hide_a_survivor_on_another_page() {
    // Nothing is applied: page 1 still shows the secret, and page 3 has a legitimate copy.
    // The allowance is one copy for the whole document, not one per page.
    let proof = prove_unapplied(&mut three_pages(true));
    assert_eq!(surface(&proof, Surface::ExtractedText).verdict, Verdict::Survivor, "{}", proof.to_text());
    // The decoded streams hold the same two copies and are counted, not skipped.
    assert_eq!(surface(&proof, Surface::DecodedStreams).verdict, Verdict::Survivor, "{}", proof.to_text());
}

#[test]
fn a_copy_that_remains_elsewhere_is_allowed_exactly_once() {
    let (r, proof) = run(&mut three_pages(true));
    r.unwrap();
    assert!(proof.passed(), "{}", proof.to_text());
    assert_eq!(surface(&proof, Surface::DecodedStreams).verdict, Verdict::Clean);
    assert_eq!(surface(&proof, Surface::RawBytes).verdict, Verdict::Clean);
}

#[test]
fn a_surplus_copy_is_a_survivor_on_the_decoded_streams() {
    // One copy legitimately remains; a second sits in a stream no page draws.
    let mut doc = three_pages(true);
    let orphan = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"BT /F1 10 Tf 10 50 Td (SECRETVALUE) Tj ET".to_vec())));
    attach(&mut doc, Object::Ref(orphan));
    let (_, proof) = run(&mut doc);
    assert!(!proof.passed());
    assert_eq!(surface(&proof, Surface::DecodedStreams).verdict, Verdict::Survivor, "{}", proof.to_text());
}
