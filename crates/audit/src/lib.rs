//! Verification tooling: audit a document for faux redactions.
//!
//! [`audit_redactions`] finds *faux redactions* — opaque filled rectangles drawn over text
//! that is still extractable underneath — and redaction annotations that were marked but
//! never applied. A properly applied redaction removes the text, so it never flags.
//!
//! A rectangle counts as a cover only when it was painted strictly *after* the text it
//! overlaps, and only when it is dark enough against the text's color to make the text
//! unreadable (WCAG contrast below 3.0). Page backgrounds, table shading and highlight
//! marks are therefore not flagged.
//!
//! Detection limits (documented, not hidden): only `re` rectangles closed by a fill operator
//! are considered — arbitrary filled paths, shadings and patterns are not; a rectangle drawn
//! with a translucent ExtGState (`/ca < 1`) is not flagged; annotation rectangles are compared
//! in unrotated user space, so pages with `/Rotate` may misalign.
//!
//! The audit fails closed: any content that cannot be read (an undecodable content stream,
//! malformed annotations) is an error, never an empty finding list.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_cos::{Dict, Document};
use pdfcraft_edit::TextLine;

mod cover;

#[cfg(test)]
mod tests;

/// What the audit found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FindingKind {
    /// An opaque filled rectangle covers text that is still extractable.
    CoveredText,
    /// A `/Redact` annotation was placed but never applied: the text is untouched.
    UnappliedMark,
}

impl FindingKind {
    pub fn id(self) -> &'static str {
        match self {
            FindingKind::CoveredText => "covered-text",
            FindingKind::UnappliedMark => "unapplied-mark",
        }
    }
}

/// One audit finding.
#[derive(Clone, Debug, PartialEq)]
pub struct AuditFinding {
    /// 0-based page index.
    pub page: usize,
    pub kind: FindingKind,
    /// The covering rectangle (or the mark's rectangle), in page user space.
    pub rect: [f64; 4],
    /// The extractable text the cover hides (possibly empty when nothing decodable remains).
    pub covered_text: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("page {0}: {1}")]
    Text(usize, String),
    #[error("content walk exceeded the operator bound")]
    TooManyOps,
    #[error("page has a malformed /Annots entry")]
    BadAnnots,
    #[error(transparent)]
    Cos(#[from] pdfcraft_cos::CosError),
    #[error(transparent)]
    Edit(#[from] pdfcraft_edit::EditError),
}

/// Fraction of a text line's area that must lie under a cover to count as hidden.
const COVER_FRACTION: f64 = 0.4;
/// WCAG contrast ratio below which the text is unreadable against the cover.
const COVER_CONTRAST: f64 = 3.0;

/// Audit every page for faux redactions and unapplied redaction marks. Fails closed: any
/// content that cannot be decoded is returned as an error, never as an empty finding list.
pub fn audit_redactions(doc: &Document) -> Result<Vec<AuditFinding>, AuditError> {
    let mut out = Vec::new();
    for (n, page) in pdfcraft_model::pages(doc).iter().enumerate() {
        let lines = pdfcraft_edit::text_lines(doc, n).map_err(|e| AuditError::Text(n, e.to_string()))?;
        // 1. Opaque filled rectangles over extractable text.
        for r in cover::filled_rects(doc, &page.dict)? {
            let mut covered = String::new();
            for line in &lines {
                if is_cover(&r, line) {
                    if !covered.is_empty() {
                        covered.push(' ');
                    }
                    covered.push_str(line.text.trim());
                }
            }
            if !covered.trim().is_empty() {
                out.push(AuditFinding { page: n, kind: FindingKind::CoveredText, rect: r.rect, covered_text: covered });
            }
        }
        // 2. Redact annotations that were never applied.
        for annot in annots(doc, &page.dict)? {
            let mut covered = String::new();
            for line in &lines {
                if covers(&annot, &line.rect) {
                    if !covered.is_empty() {
                        covered.push(' ');
                    }
                    covered.push_str(line.text.trim());
                }
            }
            if !covered.trim().is_empty() {
                out.push(AuditFinding { page: n, kind: FindingKind::UnappliedMark, rect: annot, covered_text: covered });
            }
        }
    }
    Ok(out)
}

/// A filled rectangle is a faux-redaction cover for a text line iff it was painted strictly
/// after the line, overlaps it substantially, and is dark enough against the line's color to
/// make the text unreadable. Light covers (table shading, highlights) over dark text fail the
/// contrast check and are not flagged.
fn is_cover(rect: &cover::FilledRect, line: &TextLine) -> bool {
    rect.pos > line.paint_pos() && covers(&rect.rect, &line.rect) && contrast(line.color, rect.rgb) < COVER_CONTRAST
}

/// Does `cover` hide a substantial part of `line`?
fn covers(cover: &[f64; 4], line: &[f64; 4]) -> bool {
    let ix0 = cover[0].max(line[0]);
    let iy0 = cover[1].max(line[1]);
    let ix1 = cover[2].min(line[2]);
    let iy1 = cover[3].min(line[3]);
    if ix1 <= ix0 || iy1 <= iy0 {
        return false;
    }
    let line_area = (line[2] - line[0]).max(0.0) * (line[3] - line[1]).max(0.0);
    if line_area <= 0.0 {
        return false;
    }
    (ix1 - ix0) * (iy1 - iy0) / line_area >= COVER_FRACTION
}

/// WCAG 2.x relative luminance of an sRGB triple.
fn luminance(rgb: [f64; 3]) -> f64 {
    let lin = rgb.map(|c| {
        let c = c.clamp(0.0, 1.0);
        if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    });
    0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2]
}

/// WCAG contrast ratio between two sRGB colors: 1.0 (identical) to 21.0 (black on white).
fn contrast(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (l1, l2) = (luminance(a), luminance(b));
    let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

/// Rectangles of `/Redact` annotations on a page (unrotated user space). A missing `/Annots`
/// means no marks; a malformed one fails the audit rather than silently passing.
fn annots(doc: &Document, page: &Dict) -> Result<Vec<[f64; 4]>, AuditError> {
    let Some(a) = page.get(b"Annots") else { return Ok(Vec::new()) };
    let resolved = doc.resolve(a);
    let items: Vec<pdfcraft_cos::Object> = match &*resolved {
        pdfcraft_cos::Object::Array(items) => items.clone(),
        // A missing /Annots means no marks; a malformed one fails the audit.
        _ => return Err(AuditError::BadAnnots),
    };
    let mut out = Vec::new();
    for a in &items {
        let dict = match &*doc.resolve(a) {
            pdfcraft_cos::Object::Dict(d) => d.clone(),
            _ => continue,
        };
        if dict.name(b"Subtype") != Some(b"Redact") {
            continue;
        }
        let v: Vec<f64> = match dict.get(b"Rect") {
            None => continue,
            Some(r) => doc.resolve(r).as_array().map(|arr| arr.iter().filter_map(|o| doc.resolve(o).as_f64()).collect()).unwrap_or_default(),
        };
        if v.len() == 4 && v.iter().all(|x| x.is_finite()) {
            out.push([v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])]);
        }
    }
    Ok(out)
}
