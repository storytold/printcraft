//! Regression tests for the signed-document guard and strict stream decoding.

use std::sync::Arc;

use pdfcraft_cos::{CosError, Dict, Document, SaveOptions, Stream, write_full, write_incremental};

/// Classic one-revision file from object bodies (object i+1 = bodies[i]).
fn classic(bodies: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in bodies.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", bodies.len() + 1).as_bytes());
    out
}

fn with_form(form: &str, extra: &[&str]) -> Vec<u8> {
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /AcroForm {form} >>"),
        "<< /Type /Pages /Kids [] /Count 0 >>".to_string(),
        "<< /Unused true >>".to_string(),
        "<< /Unused true >>".to_string(),
        "<< /Unused true >>".to_string(),
        "<< /Unused true >>".to_string(),
    ];
    objs.extend(extra.iter().map(|s| (*s).to_string()));
    classic(&objs)
}

const SIG: &str = "<< /Type /Sig /ByteRange [0 1 2 3] /Contents <00> >>";

#[test]
fn indirect_sigflags_and_field_type_still_count_as_signed() {
    // Object 7 is the flags value, 8 the field.
    let doc = Document::open(Arc::new(with_form("<< /SigFlags 7 0 R /Fields [] >>", &["3"]))).unwrap();
    assert!(doc.is_signed(), "/SigFlags given as a reference");
    let doc = Document::open(Arc::new(with_form("<< /SigFlags 7 0 R /Fields [] >>", &["0"]))).unwrap();
    assert!(!doc.is_signed(), "a resolved zero is not signed");
    // /FT as a reference, with the signature value on the field.
    let doc = Document::open(Arc::new(with_form("<< /Fields [7 0 R] >>", &["<< /FT 9 0 R /T (s) /V 8 0 R >>", SIG, "/Sig"]))).unwrap();
    assert!(doc.is_signed(), "/FT given as a reference");
    // A dangling or malformed flags value is unreadable, so it fails closed.
    let doc = Document::open(Arc::new(with_form("<< /SigFlags 99 0 R /Fields [] >>", &[]))).unwrap();
    assert!(doc.is_signed());
}

#[test]
fn a_reconstructed_signed_file_cannot_be_rewritten_through_write_incremental() {
    let mut bytes = with_form("<< /SigFlags 3 /Fields [] >>", &[]);
    // Break the cross-reference chain so the file is repaired by scanning.
    let at = bytes.windows(9).rposition(|w| w == b"startxref").unwrap();
    bytes.truncate(at);
    let mut doc = Document::open(Arc::new(bytes)).unwrap();
    assert!(doc.revisions().is_empty(), "the file was reconstructed");
    assert!(doc.is_signed());
    // Without a rewrite request the repair path stays unguarded (signing needs it).
    assert!(write_incremental(&doc, &SaveOptions::default()).is_ok());
    // A requested rewrite (redaction) must not slip through the empty revision chain.
    doc.require_full_save_with_new_id();
    assert_eq!(write_incremental(&doc, &SaveOptions::default()), Err(CosError::SignedDocument));
    assert_eq!(write_full(&doc, &SaveOptions::default()), Err(CosError::SignedDocument));
    let allowed = SaveOptions { allow_signed_rewrite: true, ..SaveOptions::default() };
    assert!(write_incremental(&doc, &allowed).is_ok());
}

#[test]
fn strict_decoding_exposes_damage_the_tolerant_decoder_hides() {
    let data: Vec<u8> = (0..20_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    let good = Stream::flate(Dict::new(), &data);
    assert_eq!(good.decoded_strict_within(1 << 20).unwrap(), data);
    assert_eq!(good.decoded_within(1 << 20).unwrap(), data);

    // Cut the compressed data short: the tolerant decoder returns the prefix without complaint.
    let mut cut = good.clone();
    let keep = cut.raw.len() / 2;
    cut.raw = Arc::new(cut.raw.get(..keep).unwrap_or_default().to_vec()).into();
    let prefix = cut.decoded_within(1 << 20).unwrap();
    assert!(prefix.len() < data.len(), "the lenient API keeps hiding the damage");
    assert!(matches!(cut.decoded_strict_within(1 << 20), Err(CosError::Filter(_))));

    // Strict decoding is bounded too.
    let bomb = Stream::flate(Dict::new(), &vec![0u8; 4 << 20]);
    assert!(matches!(bomb.decoded_strict_within(1 << 20), Err(CosError::Filter(_))));
    assert_eq!(bomb.decoded_strict_within(8 << 20).map(|v| v.len()), Ok(4 << 20));
    // An unfiltered stream has nothing to be damaged, but the output limit still binds.
    let plain = Stream::from_raw(Dict::new(), b"abc".to_vec());
    assert!(matches!(plain.decoded_strict_within(2), Err(CosError::Filter(_))));
    assert_eq!(plain.decoded_strict_within(3).unwrap(), b"abc");
}

#[test]
fn an_unreadable_form_blocks_only_a_requested_redaction_rewrite() {
    // /Fields looping back on itself: unreadable, so `is_signed` fails closed, but nothing says
    // the document is signed.
    let bytes = with_form("<< /Fields [7 0 R] >>", &["<< /FT /Tx /T (a) /Kids [7 0 R] >>"]);
    let mut doc = Document::open(Arc::new(bytes)).unwrap();
    assert!(doc.is_signed() && !doc.is_signed_definitely());
    // Save As, Optimize, Combine, Split: an ordinary full rewrite goes through.
    assert!(write_full(&doc, &SaveOptions::default()).is_ok());
    // The rewrite redaction asks for does not.
    doc.require_full_save_with_new_id();
    assert_eq!(write_full(&doc, &SaveOptions::default()), Err(CosError::SignedDocument));
    // A document that is signed for certain is refused either way.
    let signed = Document::open(Arc::new(with_form("<< /SigFlags 3 /Fields [] >>", &[]))).unwrap();
    assert!(signed.is_signed_definitely());
    assert_eq!(write_full(&signed, &SaveOptions::default()), Err(CosError::SignedDocument));
}

fn with_perms(perms: &str, form: &str) -> Vec<u8> {
    classic(&[format!("<< /Type /Catalog /Pages 2 0 R /Perms {perms} /AcroForm {form} >>"), "<< /Type /Pages /Kids [] /Count 0 >>".to_string()])
}

#[test]
fn a_ur3_only_document_is_not_blocked_on_ordinary_rewrites() {
    let mut doc = Document::open(Arc::new(with_perms("<< /UR3 << /Type /Sig >> >>", "<< /Fields [] >>"))).unwrap();
    assert!(!doc.is_signed_definitely());
    assert!(write_full(&doc, &SaveOptions::default()).is_ok(), "Save As / Optimize / Combine / Split");
    // Redaction stays strict: the usage-rights signature would not survive the removal.
    assert!(doc.is_signed());
    doc.require_full_save_with_new_id();
    assert_eq!(write_full(&doc, &SaveOptions::default()), Err(CosError::SignedDocument));
}

#[test]
fn certification_and_real_signatures_still_block_ordinary_rewrites() {
    let certified = Document::open(Arc::new(with_perms("<< /DocMDP 5 0 R >>", "<< /Fields [] >>"))).unwrap();
    assert_eq!(write_full(&certified, &SaveOptions::default()), Err(CosError::SignedDocument));
    let by_range = Document::open(Arc::new(with_form("<< /Fields [7 0 R] >>", &["<< /FT /Tx /T (a) /V << /ByteRange [0 1 2 3] >> >>"]))).unwrap();
    assert_eq!(write_full(&by_range, &SaveOptions::default()), Err(CosError::SignedDocument));
    let signed = Document::open(Arc::new(with_form("<< /Fields [7 0 R] >>", &[&format!("<< /FT /Sig /T (s) /V {SIG} >>")]))).unwrap();
    assert_eq!(write_full(&signed, &SaveOptions::default()), Err(CosError::SignedDocument));
}

#[test]
fn ur3_with_a_malformed_form_allows_ordinary_and_refuses_redaction() {
    let mut doc = Document::open(Arc::new(with_perms("<< /UR3 << >> >>", "<< /Fields [7 0 R] >>"))).unwrap();
    assert!(write_full(&doc, &SaveOptions::default()).is_ok());
    doc.require_full_save_with_new_id();
    assert_eq!(write_full(&doc, &SaveOptions::default()), Err(CosError::SignedDocument));
}
