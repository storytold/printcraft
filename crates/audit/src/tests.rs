//! Tests build their own tiny PDFs in code (contributor-original fixtures): no binary files,
//! no Adobe material, exact xref offsets computed by the builder below.

use std::sync::Arc;

use pdfcraft_cos::Document;

use crate::{FindingKind, audit_redactions};

/// Minimal PDF assembler: objects are numbered 1..=n in order.
struct Builder {
    objects: Vec<Vec<u8>>,
}

impl Builder {
    fn new() -> Self {
        Builder { objects: Vec::new() }
    }

    fn add(&mut self, body: &str) -> usize {
        self.objects.push(body.as_bytes().to_vec());
        self.objects.len()
    }

    fn add_stream(&mut self, data: &[u8]) -> usize {
        self.add_stream_with(data, "")
    }

    fn add_stream_with(&mut self, data: &[u8], dict_extra: &str) -> usize {
        let mut body = format!("<< /Length {} {dict_extra} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        self.objects.push(body);
        self.objects.len()
    }

    fn build(self) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, obj) in self.objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(obj);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref_at = out.len();
        let n = self.objects.len() + 1;
        out.extend_from_slice(format!("xref\n0 {n}\n").as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(b"trailer\n<< ");
        out.extend_from_slice(format!("/Size {n} /Root 1 0 R").as_bytes());
        out.extend_from_slice(b" >>\nstartxref\n");
        out.extend_from_slice(format!("{xref_at}\n").as_bytes());
        out.extend_from_slice(b"%%EOF\n");
        out
    }
}

const FONT: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";

/// One page with `content` as its stream; `page_extra` is appended inside the page dict.
fn one_page(content: &[u8], page_extra: &str) -> Vec<u8> {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    let stream_num = 4;
    b.add(&format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {stream_num} 0 R /Resources << /Font << /F1 5 0 R >> >> {page_extra} >>"
    ));
    b.add_stream(content);
    b.add(FONT);
    b.build()
}

/// One page with two content streams (`/Contents [4 0 R 5 0 R]`).
fn two_streams(s1: &[u8], s2: &[u8]) -> Vec<u8> {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents [4 0 R 5 0 R] /Resources << /Font << /F1 6 0 R >> >> >>");
    b.add_stream(s1);
    b.add_stream(s2);
    b.add(FONT);
    b.build()
}

/// One page whose content stream claims `/FlateDecode` but holds garbage: decoding must fail.
fn bad_filter_page() -> Vec<u8> {
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>");
    b.add_stream_with(b"this is not deflated data", "/Filter /FlateDecode");
    b.add(FONT);
    b.build()
}

fn open(bytes: Vec<u8>) -> Document {
    Document::open(Arc::new(bytes)).expect("test pdf must open")
}

#[test]
fn faux_redaction_is_found() {
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0 g\n100 688 220 32 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    let f = &findings[0];
    assert_eq!(f.page, 0);
    assert_eq!(f.kind, FindingKind::CoveredText);
    assert!(f.covered_text.contains("SECRET"), "covered_text was {:?}", f.covered_text);
}

#[test]
fn clean_document_has_no_findings() {
    let content = b"BT /F1 24 Tf 100 700 Td (nothing hidden here) Tj ET\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn black_box_without_text_is_not_a_finding() {
    // A filled rectangle with no text under it is decoration, not a faux redaction.
    let content = b"BT /F1 24 Tf 100 100 Td (far away) Tj ET\n0 g\n400 600 100 40 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn translucent_cover_is_not_flagged() {
    // A rectangle drawn with a translucent ExtGState does not hide the text.
    let content = b"/GS1 gs\nBT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0 g\n100 688 220 32 re f\n";
    let page_extra = "/Resources << /Font << /F1 5 0 R >> /ExtGState << /GS1 6 0 R >> >>";
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add(&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R {page_extra} >>"));
    b.add_stream(content);
    b.add(FONT);
    b.add("<< /Type /ExtGState /ca 0.5 >>");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn unapplied_redact_mark_is_found() {
    let content = b"BT /F1 24 Tf 100 700 Td (DO NOT SHARE) Tj ET\n";
    let page_extra = "/Annots [6 0 R]";
    let mut b = Builder::new();
    b.add("<< /Type /Catalog /Pages 2 0 R >>");
    b.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.add(&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> {page_extra} >>"));
    b.add_stream(content);
    b.add(FONT);
    b.add("<< /Type /Annot /Subtype /Redact /Rect [95 685 305 725] /C [1 0 0] >>");
    let findings = audit_redactions(&open(b.build())).expect("audit must run");
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].kind, FindingKind::UnappliedMark);
    assert!(findings[0].covered_text.contains("DO NOT SHARE"));
}

#[test]
fn background_before_text_is_not_flagged() {
    // An opaque dark rectangle painted *before* the text is a background, not a cover.
    let content = b"0 g\n100 688 220 32 re f\nBT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn light_shading_after_text_is_not_flagged() {
    // Light table-style shading painted after the text leaves it readable: not a cover.
    let content = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n0.9 g\n100 688 220 32 re f\n";
    let findings = audit_redactions(&open(one_page(content, ""))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn graphics_state_carries_across_streams() {
    // The first stream sets a white fill after the text; the second stream's rectangle over
    // the text is therefore white, not the default black: no finding. Without the carry fix
    // the rectangle would default to black and flag.
    let s1 = b"BT /F1 24 Tf 100 700 Td (SECRET DATA 123) Tj ET\n1 g\n";
    let s2 = b"100 688 220 32 re f\n";
    let findings = audit_redactions(&open(two_streams(s1, s2))).expect("audit must run");
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn audit_fails_closed_on_undecodable_stream() {
    // A content stream that cannot be decoded is an audit error, never an empty finding list.
    let err = audit_redactions(&open(bad_filter_page())).expect_err("audit must fail closed");
    let msg = err.to_string();
    assert!(msg.contains("decoded"), "unexpected error: {msg}");
}
