//! Redaction (architecture §11.2, execution plan M8.5–M8.7). Layer L4.
//!
//! Content is marked for redaction with Redact annotations (§12.5.6.23); `pdfcraft-annot`
//! creates them. [`apply`] then removes everything under the marks, for good:
//! - text glyphs (the rest of each line keeps its position), inline images, and paths that the
//!   marks cover; images and vectors partly under a mark lose the covered part;
//! - content inside form XObjects (rewritten as new objects, so pages sharing them keep theirs);
//! - comments, links and form fields whose rectangle overlaps a mark;
//! - the marks themselves, replaced by boxes in their fill colour (with their overlay text)
//!   drawn into the page.
//!
//! A verification pass then re-reads every redacted page; if any glyph or inline image is still
//! under a region the whole operation fails (callers keep the previous document: fail-closed).
//! A second, independent check ([`verify`]) then sweeps the serialized output for the removed
//! text on every surface and produces a [`Proof`].
//!
//! Page resources are read through inheritance: `/Resources` is an inheritable page-tree entry
//! (ISO 32000-2, 7.8.3, Table 30), and `pdfcraft_model::pages` fills each page's dictionary with
//! the effective value — the page's own if it has one, else the nearest node above that does —
//! so a `Do` whose name resolves only through an ancestor is processed exactly like a page-own
//! one, and a name that resolves nowhere is left alone (as any unknown name is). Writes go the
//! other way: everything a run adds (cleared copies, rewritten forms) is written into the page's
//! own dictionary, which then carries a private copy of the effective resources, and retirement
//! (`tags::retire_forms`) removes superseded names both from the page's copy and from every node
//! above that owns one — while a node's entry stays as long as any page still draws that name,
//! so a sibling that inherits it keeps what it shows.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

pub mod codes;
mod image;
mod interp;
mod limits;
pub mod patterns;
pub mod sanitize;
mod tags;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_failclosed;
#[cfg(test)]
mod tests_interp;
#[cfg(test)]
mod tests_patterns;
#[cfg(test)]
mod tests_sanitize;
#[cfg(test)]
mod tests_sweep;
#[cfg(test)]
mod tests_verify;
pub mod verify;

pub use interp::Unsupported;
use interp::{Mode, Scope, process};
use limits::{Budget, MAX_PAGE_TOTAL};
pub use sanitize::{LayerPolicy, Sanitize};
pub use verify::{Proof, ProofOptions, Snapshot};

pub type Rgb = [f64; 3];

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum RedactError {
    #[error("there are no redaction marks to apply")]
    NothingToApply,
    #[error("page {0} has content that can't be read, so it can't be redacted safely")]
    Unreadable(usize),
    #[error("page {page} can't be redacted safely: {reason}, so nothing was changed")]
    Unsupported { page: usize, reason: Unsupported },
    #[error("a redaction mark on page {0} has no readable area (neither /QuadPoints nor /Rect), so it can't be applied")]
    UnreadableMark(usize),
    #[error("the document is digitally signed; redacting it rewrites the file and invalidates every signature, so nothing was changed")]
    Signed,
    #[error(
        "the document has XFA form data, which redaction can't reach, so nothing was changed; sanitizing removes the XFA data, redaction alone does not"
    )]
    Xfa,
    #[error("a stream the redaction created can't be read back, so the result can't be proven and nothing was changed")]
    OverlayUnreadable,
    #[error("redaction could not be verified: {0} item(s) were still found under the marks, so nothing was changed")]
    Residue(usize),
    #[error(
        "redaction could not be proven: {unswept} surface(s) could not be swept, {unverifiable} region(s) can't be verified and {failures} check(s) failed, so nothing was changed"
    )]
    ProofIncomplete { unswept: usize, unverifiable: usize, failures: usize },
    #[error("the structure tree is too large or too deeply nested to be cleaned completely, so nothing was changed")]
    StructureTooLarge,
    #[error(transparent)]
    Edit(#[from] pdfcraft_edit::EditError),
    #[error(transparent)]
    Form(#[from] pdfcraft_forms::FormError),
    #[error(transparent)]
    Cos(#[from] pdfcraft_cos::CosError),
}

/// A redaction mark on a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    /// 0-based page index.
    pub page: usize,
    pub obj: ObjRef,
    /// The marked areas in user space (one per quadrilateral).
    pub rects: Vec<[f64; 4]>,
    /// The box colour once applied (`/IC`; none = no box).
    pub fill: Option<Rgb>,
    pub overlay: String,
    /// How the overlay text is drawn (`/DA`, `/Q`, `/Repeat`).
    pub look: pdfcraft_annot::OverlayLook,
}

/// The largest overlay text size (points) a `/DA` may ask for.
const MAX_OVERLAY_SIZE: f64 = 1000.0;

/// A finite number from a `/DA` token (`nan` and `inf` parse as floats but are not numbers here).
fn da_num(t: &str) -> Option<f64> {
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// The overlay look of a Redact annotation: font, size and colour from `/DA`, alignment from
/// `/Q`, repetition from `/Repeat`.
fn overlay_look(doc: &Document, d: &Dict) -> pdfcraft_annot::OverlayLook {
    let mut look = pdfcraft_annot::OverlayLook::default();
    if let Some(da) = d.get(b"DA").and_then(|o| doc.resolve(o).as_string().map(|s| String::from_utf8_lossy(&s.bytes).into_owned())) {
        let t: Vec<&str> = da.split_whitespace().collect();
        for (i, w) in t.iter().enumerate() {
            let arg = |back: usize| i.checked_sub(back).and_then(|k| t.get(k)).and_then(|x| da_num(x));
            match *w {
                "rg" => {
                    if let (Some(r), Some(g), Some(b)) = (arg(3), arg(2), arg(1)) {
                        look.color = [r, g, b];
                    }
                }
                "g" => {
                    if let Some(g) = arg(1) {
                        look.color = [g; 3];
                    }
                }
                "Tf" => {
                    if let Some(name) = i.checked_sub(2).and_then(|k| t.get(k)) {
                        look.font = pdfcraft_annot::OverlayFont::from_resource(name.trim_start_matches('/'));
                    }
                    look.size = arg(1).unwrap_or(0.0).clamp(0.0, MAX_OVERLAY_SIZE);
                }
                _ => {}
            }
        }
    }
    look.align = d.int(b"Q").unwrap_or(1).clamp(0, 2) as u8;
    look.repeat = matches!(d.get(b"Repeat"), Some(Object::Bool(true)));
    look
}

/// What [`apply`] removed.
#[derive(Clone, Default, PartialEq)]
pub struct Report {
    pub marks: usize,
    pub pages: usize,
    pub glyphs: usize,
    pub images_removed: usize,
    pub images_cleared: usize,
    pub paths_removed: usize,
    pub paths_clipped: usize,
    pub forms_rewritten: usize,
    pub forms_removed: usize,
    pub annotations: usize,
    pub fields: usize,
    /// Structure elements that lost alternate/actual text or emptied marked content.
    pub tags: usize,
    /// What the sanitize pass removed, per category (see [`ApplyOptions`]).
    pub sanitized: Vec<(sanitize::Hidden, usize)>,
    /// The names of the layers that were held hidden or switched off.
    pub layers: Vec<String>,
}

/// Layer names are text the user wrote, so `{:?}` (logs, panics, test output) shows how many
/// layers there were and never their names.
impl std::fmt::Debug for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Report")
            .field("marks", &self.marks)
            .field("pages", &self.pages)
            .field("glyphs", &self.glyphs)
            .field("images_removed", &self.images_removed)
            .field("images_cleared", &self.images_cleared)
            .field("paths_removed", &self.paths_removed)
            .field("paths_clipped", &self.paths_clipped)
            .field("forms_rewritten", &self.forms_rewritten)
            .field("forms_removed", &self.forms_removed)
            .field("annotations", &self.annotations)
            .field("fields", &self.fields)
            .field("tags", &self.tags)
            .field("sanitized", &self.sanitized)
            .field("layers", &format_args!("{} layer(s)", self.layers.len()))
            .finish()
    }
}

/// How [`apply_with`] runs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApplyOptions {
    /// What to sanitize in the same operation (default: [`Sanitize::Recommended`]).
    pub sanitize: Sanitize,
    /// What the sanitize pass does with layers.
    pub layers: LayerPolicy,
    /// Redact a digitally signed document anyway. Off by default: redaction rewrites the whole
    /// file, which invalidates every signature, so a signed document is refused with
    /// [`RedactError::Signed`] before anything is touched.
    pub allow_signed: bool,
}

fn rect_of(doc: &Document, o: Option<&Object>) -> Option<[f64; 4]> {
    let v: Vec<f64> = doc.resolve(o?).as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
    (v.len() == 4 && v.iter().all(|x| x.is_finite())).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
}

fn color(doc: &Document, d: &Dict, key: &[u8]) -> Option<Rgb> {
    let v: Vec<f64> = doc.resolve(d.get(key)?).as_array()?.iter().filter_map(Object::as_f64).collect();
    match v.len() {
        1 => Some([v[0]; 3]),
        3 => Some([v[0], v[1], v[2]]),
        4 => Some([(1.0 - v[0]) * (1.0 - v[3]), (1.0 - v[1]) * (1.0 - v[3]), (1.0 - v[2]) * (1.0 - v[3])]),
        _ => None,
    }
}

pub(crate) fn annots_of(doc: &Document, page: &Dict) -> Vec<Object> {
    page.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default()
}

/// The areas of a Redact annotation: one rectangle per quadrilateral of `/QuadPoints`. A list
/// that is malformed (not an array, not all numbers, not a multiple of eight) can't be trusted
/// to say which quadrilaterals were meant, so the annotation's `/Rect` is marked as well (and
/// the whole quadrilaterals of a list that is merely cut short are kept): the result is then a
/// larger area, never a shifted or smaller one. `None` = nothing readable.
fn mark_areas(doc: &Document, d: &Dict) -> Option<Vec<[f64; 4]>> {
    let (mut numeric, mut quads) = (true, Vec::new());
    match d.get(b"QuadPoints").map(|q| doc.resolve(q)).as_deref() {
        None | Some(Object::Null) => {}
        Some(Object::Array(a)) => {
            for x in a {
                match doc.resolve(x).as_f64().filter(|v| v.is_finite()) {
                    Some(v) => quads.push(v),
                    None => numeric = false,
                }
            }
        }
        Some(_) => numeric = false,
    }
    let well_formed = numeric && quads.len() % 8 == 0;
    // Entries that were skipped would shift every later quadrilateral: none of them is trusted.
    let mut rects: Vec<[f64; 4]> = if numeric {
        quads
            .as_chunks::<8>()
            .0
            .iter()
            .map(|q| {
                let xs = [q[0], q[2], q[4], q[6]];
                let ys = [q[1], q[3], q[5], q[7]];
                [
                    xs.iter().copied().fold(f64::MAX, f64::min),
                    ys.iter().copied().fold(f64::MAX, f64::min),
                    xs.iter().copied().fold(f64::MIN, f64::max),
                    ys.iter().copied().fold(f64::MIN, f64::max),
                ]
            })
            .collect()
    } else {
        Vec::new()
    };
    if (rects.is_empty() || !well_formed)
        && let Some(r) = rect_of(doc, d.get(b"Rect"))
    {
        rects.push(r);
    }
    (!rects.is_empty()).then_some(rects)
}

/// Every redaction mark in the document, page by page.
pub fn marks(doc: &Document) -> Vec<Mark> {
    marks_checked(doc).0
}

/// [`marks`], and the pages (1-based) of Redact annotations that have no readable area and so
/// can't be applied.
pub(crate) fn marks_checked(doc: &Document) -> (Vec<Mark>, Vec<usize>) {
    let (mut out, mut unreadable) = (Vec::new(), Vec::new());
    for (pi, p) in pdfcraft_model::pages(doc).iter().enumerate() {
        for a in annots_of(doc, &p.dict) {
            let Some(r) = a.as_ref() else { continue };
            let obj = doc.get(r);
            let Some(d) = obj.as_dict() else { continue };
            if d.name(b"Subtype") != Some(b"Redact") {
                continue;
            }
            let Some(rects) = mark_areas(doc, d) else {
                unreadable.push(pi + 1);
                continue;
            };
            let overlay = d.get(b"OverlayText").and_then(|t| doc.resolve(t).as_string().map(|s| s.to_text())).unwrap_or_default();
            out.push(Mark { page: pi, obj: r, rects, fill: color(doc, d, b"IC"), overlay, look: overlay_look(doc, d) });
        }
    }
    (out, unreadable)
}

/// A page's `/Contents` entries as listed (a single stream or an array). A reference that can't
/// be loaded is an error: a failed parse must not read as "no content" (a reference to an object
/// that simply doesn't exist is the spec's null, and is empty).
fn contents_list(doc: &Document, page: &Dict, index: usize) -> Result<Vec<Object>, RedactError> {
    let unreadable = || RedactError::Unreadable(index + 1);
    let Some(c) = page.get(b"Contents") else { return Ok(Vec::new()) };
    match &*load(doc, c).ok_or_else(unreadable)? {
        Object::Array(a) => Ok(a.clone()),
        _ => Ok(vec![c.clone()]),
    }
}

/// `o`, or the object it refers to; `None` when that can't be loaded.
fn load(doc: &Document, o: &Object) -> Option<std::sync::Arc<Object>> {
    match o {
        Object::Ref(r) => doc.try_get(r.num).ok(),
        other => Some(std::sync::Arc::new(other.clone())),
    }
}

/// A page's content streams: (the references or inline objects as listed, their decoded data).
/// Decoding is strict and bounded: a stream that is damaged (the part after the damage would go
/// unexamined), too long, or one more than the page's budget allows is `Unreadable`.
pub(crate) fn page_streams(doc: &Document, page: &Dict, index: usize) -> Result<(Vec<Object>, Vec<Vec<u8>>), RedactError> {
    let list = contents_list(doc, page, index)?;
    let mut budget = Budget::new(MAX_PAGE_TOTAL);
    let mut data = Vec::new();
    for o in &list {
        match &*load(doc, o).ok_or(RedactError::Unreadable(index + 1))? {
            Object::Stream(s) => data.push(budget.decode(s).map_err(|_| RedactError::Unreadable(index + 1))?),
            Object::Null => data.push(Vec::new()),
            _ => return Err(RedactError::Unreadable(index + 1)),
        }
    }
    Ok((list, data))
}

/// Most repeated copies of an overlay text one area gets, per line and in all (the area comes
/// from the annotation and may be absurdly large; the text is decoration, the box is not).
const MAX_REPEAT_PER_LINE: usize = 200;
const MAX_REPEAT_CELLS: usize = 4096;
/// Most characters of an overlay text that are drawn.
const MAX_OVERLAY_CHARS: usize = 512;

/// `r` cut down to `bounds` (the page), `None` when nothing is left.
fn within(r: [f64; 4], bounds: [f64; 4]) -> Option<[f64; 4]> {
    let c = [r[0].max(bounds[0]), r[1].max(bounds[1]), r[2].min(bounds[2]), r[3].min(bounds[3])];
    (c[2] > c[0] && c[3] > c[1]).then_some(c)
}

/// The boxes and overlay text drawn for the applied marks, limited to the page (`bounds`).
fn overlay_content(marks: &[&Mark], bounds: [f64; 4]) -> Vec<u8> {
    let n = |v: f64| {
        let s = format!("{:.3}", v);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    let mut c: Vec<u8> = Vec::new();
    for m in marks {
        let Some(fill) = m.fill else { continue };
        c.extend(format!("q {} {} {} rg\n", n(fill[0]), n(fill[1]), n(fill[2])).bytes());
        for r in m.rects.iter().filter_map(|r| within(*r, bounds)) {
            c.extend(format!("{} {} {} {} re f\n", n(r[0]), n(r[1]), n(r[2] - r[0]), n(r[3] - r[1])).bytes());
        }
        c.extend_from_slice(b"Q\n");
        if m.overlay.is_empty() {
            continue;
        }
        // The overlay text: its font, colour and alignment; auto-sized to fit unless a size is
        // set; repeated to fill the area when asked.
        // (Cut to a sane length: it is repeated across the area.)
        let overlay: String = m.overlay.chars().take(MAX_OVERLAY_CHARS).collect();
        let look = &m.look;
        let (res, width): (&str, fn(&str, f64) -> f64) = match look.font {
            pdfcraft_annot::OverlayFont::Helvetica => ("PCHelv", pdfcraft_fonts::helvetica_width),
            // Approximations of the standard metrics (no font program is bundled).
            pdfcraft_annot::OverlayFont::Times => ("PCTimes", |s, size| pdfcraft_fonts::helvetica_width(s, size) * 0.9),
            pdfcraft_annot::OverlayFont::Courier => ("PCCour", |s, size| s.chars().count() as f64 * size * 0.6),
        };
        let [cr, cg, cb] = look.color.map(|v| v.clamp(0.0, 1.0));
        for r in m.rects.iter().filter_map(|r| within(*r, bounds)) {
            let (w, h) = (r[2] - r[0], r[3] - r[1]);
            let mut size = if look.size > 0.0 { look.size } else { (h * 0.7).min(12.0) };
            let tw = width(&overlay, size);
            if look.size <= 0.0 && tw > w - 2.0 && tw > 0.0 {
                size *= (w - 2.0).max(0.0) / tw;
            }
            if size < 2.0 {
                continue;
            }
            // One line, or as many repeated lines as fit (each line the text repeated across).
            let lines: Vec<String> = if look.repeat {
                let unit = width(&format!("{overlay} "), size).max(0.01);
                let per_line = (((w - 2.0) / unit).floor().max(1.0) as usize).min(MAX_REPEAT_PER_LINE);
                let count = ((h / (size * 1.2)).floor() as usize).clamp(1, MAX_REPEAT_CELLS / per_line);
                vec![vec![overlay.as_str(); per_line].join(" "); count]
            } else {
                vec![overlay.clone()]
            };
            let block = lines.len() as f64 * size * 1.2;
            let mut y = r[1] + (h + block) / 2.0 - size * 0.95;
            c.extend(
                format!("q {} {} {} {} re W n BT {} {} {} rg /{res} {} Tf ", n(r[0]), n(r[1]), n(w), n(h), n(cr), n(cg), n(cb), n(size)).bytes(),
            );
            for line in &lines {
                let lw = width(line, size);
                let x = match look.align {
                    0 => r[0] + 1.0,
                    2 => r[2] - 1.0 - lw,
                    _ => r[0] + (w - lw) / 2.0,
                };
                c.extend(format!("1 0 0 1 {} {} Tm ", n(x), n(y)).bytes());
                c.extend_from_slice(&pdfcraft_fonts::literal(&pdfcraft_fonts::win_ansi(line)));
                c.extend_from_slice(b" Tj ");
                y -= size * 1.2;
            }
            c.extend_from_slice(b"ET Q\n");
        }
    }
    c
}

/// Apply every redaction mark (or only those on `pages`, 0-based), without sanitizing
/// ([`apply_with`] does both as one operation). Irreversible for the saved file; callers keep the
/// previous document for undo.
pub fn apply(doc: &mut Document, pages: Option<&[usize]>) -> Result<Report, RedactError> {
    apply_with(doc, pages, &ApplyOptions { sanitize: Sanitize::None, ..ApplyOptions::default() })
}

/// Apply the redaction marks, then sanitize what `opts` asks for, as one operation. When it
/// fails `doc` is left exactly as it was (the work is done on a copy, which replaces `doc` only
/// on success).
pub fn apply_with(doc: &mut Document, pages: Option<&[usize]>, opts: &ApplyOptions) -> Result<Report, RedactError> {
    run(doc, pages, opts, &ProofOptions::default()).map(|(report, _)| report)
}

/// [`apply_with`], and the [`Proof`] that what was removed is gone from the saved file. It
/// fails with [`RedactError::Residue`] when the proof finds a survivor, and with
/// [`RedactError::ProofIncomplete`] when a surface could not be swept or a region can't be
/// verified (so a returned proof always [passes](Proof::passed)). To inspect a proof that does
/// not pass, take a [`Snapshot`] before applying and [`prove`](Snapshot::prove) afterwards.
pub fn apply_with_proof(
    doc: &mut Document,
    pages: Option<&[usize]>,
    opts: &ApplyOptions,
    proof_opts: &ProofOptions,
) -> Result<(Report, Proof), RedactError> {
    run(doc, pages, opts, proof_opts)
}

/// [`apply_with_proof`], also returning the [`Snapshot`] of what was removed. The proof covers
/// one serialization of the redacted document; a caller that writes the file itself can sweep
/// the bytes it is about to write with [`Snapshot::prove_saved`] before replacing anything.
pub fn apply_with_snapshot(
    doc: &mut Document,
    pages: Option<&[usize]>,
    opts: &ApplyOptions,
    proof_opts: &ProofOptions,
) -> Result<(Report, Proof, Snapshot), RedactError> {
    let done = attempt(doc, pages, opts, proof_opts);
    let (report, proof) = done.result?;
    // A successful attempt always keeps its snapshot; failing closed costs nothing if not.
    let snapshot = done.snapshot.ok_or(RedactError::ProofIncomplete { unswept: 0, unverifiable: 0, failures: 1 })?;
    *doc = done.document;
    Ok((report, proof, snapshot))
}

fn run(doc: &mut Document, pages: Option<&[usize]>, opts: &ApplyOptions, proof_opts: &ProofOptions) -> Result<(Report, Proof), RedactError> {
    let done = attempt(doc, pages, opts, proof_opts);
    match done.result {
        Ok(ok) => {
            *doc = done.document;
            Ok(ok)
        }
        Err(e) => Err(e),
    }
}

/// What [`attempt`] produced.
#[derive(Debug)]
pub(crate) struct Attempt {
    /// The report and proof, or why the redaction failed.
    pub(crate) result: Result<(Report, Proof), RedactError>,
    /// The copy the attempt worked on: redacted when it succeeded, otherwise as far as the
    /// failure let it get. For diagnosis only: a failed attempt's copy must never be saved.
    pub(crate) document: Document,
    /// The proof that did not pass, when the attempt failed at the proof stage
    /// ([`RedactError::Residue`], [`RedactError::ProofIncomplete`]); it says what was found.
    /// Read by the tests only.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) failed_proof: Option<Proof>,
    /// What the redaction removed, recorded for [`Snapshot::prove_saved`]; set on success.
    pub(crate) snapshot: Option<Snapshot>,
}

/// [`apply_with_proof`] on a copy of `doc`, which is never changed: the result carries the copy,
/// and the proof of a failed attempt, for the tests that want to see why it failed. A signed
/// document is refused ([`RedactError::Signed`]) unless [`ApplyOptions::allow_signed`] is set.
pub(crate) fn attempt(doc: &Document, pages: Option<&[usize]>, opts: &ApplyOptions, proof_opts: &ProofOptions) -> Attempt {
    let mut work = doc.clone();
    // Redaction rewrites the whole file, which invalidates every signature: refuse before
    // anything is touched, unless the caller accepted that.
    if !opts.allow_signed && doc.is_signed() {
        return Attempt { result: Err(RedactError::Signed), document: work, failed_proof: None, snapshot: None };
    }
    let mut failed_proof = None;
    let mut snapshot = None;
    let result = run_on(&mut work, pages, opts, proof_opts, &mut failed_proof, &mut snapshot);
    Attempt { result, document: work, failed_proof, snapshot }
}

/// The text of a rich-text string (`/RC`): the markup removed, the character entities decoded.
fn strip_markup(rich: &str) -> String {
    let (mut out, mut in_tag) = (String::new(), false);
    for ch in rich.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// The strings of a value (a text string, or an array of them, as a choice field's value is).
fn value_strings(doc: &Document, o: &Object, depth: usize, out: &mut Vec<String>) {
    match &*doc.resolve(o) {
        Object::String(s) => out.push(s.to_text()),
        Object::Array(a) if depth < 4 => a.iter().for_each(|x| value_strings(doc, x, depth + 1, out)),
        _ => {}
    }
}

/// The text a removed annotation held besides what it drew: `/Contents`, the text of `/RC`, and,
/// for a widget, the field's value and default (`/V`, `/DV`, inherited through `/Parent`). The
/// proof looks for these, since they leave the document by another route than page glyphs.
fn annotation_strings(doc: &Document, d: &Dict, widget: bool) -> Vec<String> {
    let mut out = Vec::new();
    for key in [&b"Contents"[..], b"RC"] {
        if let Some(o) = d.get(key) {
            match &*doc.resolve(o) {
                Object::String(s) if key == b"RC" => out.push(strip_markup(&s.to_text())),
                // A rich-text stream (decoded strictly; a damaged one just isn't searched for).
                Object::Stream(s) => out.extend(s.decoded_strict_within(limits::MAX_STREAM).ok().map(|b| strip_markup(&String::from_utf8_lossy(&b)))),
                _ => value_strings(doc, o, 0, &mut out),
            }
        }
    }
    if widget {
        let mut node = d.clone();
        // Bounded: a loop in `/Parent` can't hang this.
        for _ in 0..16 {
            for key in [&b"V"[..], b"DV"] {
                if let Some(o) = node.get(key) {
                    value_strings(doc, o, 0, &mut out);
                }
            }
            match node.get(b"Parent").and_then(|p| doc.resolve(p).as_dict().cloned()) {
                Some(parent) => node = parent,
                None => break,
            }
        }
    }
    out
}

/// What annotations and fields removed from the document, to be dealt with after the pages.
#[derive(Default)]
struct Gone {
    /// Text that left through annotations and fields (looked for by the proof).
    strings: Vec<String>,
    /// Names of the fields with a widget under a mark (all their widgets go).
    fields: Vec<String>,
    widgets: Vec<ObjRef>,
}

fn run_on(
    doc: &mut Document,
    pages: Option<&[usize]>,
    opts: &ApplyOptions,
    proof_opts: &ProofOptions,
    failed_proof: &mut Option<Proof>,
    kept: &mut Option<Snapshot>,
) -> Result<(Report, Proof), RedactError> {
    // XFA form data holds values redaction can't reach.
    if doc.has_xfa() && !opts.sanitize.removes_xfa() {
        return Err(RedactError::Xfa);
    }
    // Annotations held directly in `/Annots` get an object of their own, so they can be named.
    sanitize::promote_direct_annots(doc)?;
    let (all_marks, unreadable) = marks_checked(doc);
    // A mark that can't be read can't be applied, and applying the rest would look complete.
    if let Some(page) = unreadable.iter().copied().find(|p| pages.is_none_or(|sel| sel.contains(&p.saturating_sub(1)))) {
        return Err(RedactError::UnreadableMark(page));
    }
    let chosen: Vec<&Mark> = all_marks.iter().filter(|m| pages.is_none_or(|p| p.contains(&m.page))).collect();
    if chosen.is_empty() {
        return Err(RedactError::NothingToApply);
    }
    // What is about to go, recorded before it does (the proof looks for exactly that).
    let mut snapshot = Snapshot::capture_marks(doc, &chosen)?;
    let mut report = Report { marks: chosen.len(), ..Report::default() };
    let mut by_page: Vec<usize> = chosen.iter().map(|m| m.page).collect();
    by_page.sort_unstable();
    by_page.dedup();
    report.pages = by_page.len();
    let mut gone = Gone::default();
    let widget_owner: Vec<(ObjRef, String)> =
        pdfcraft_forms::fields(doc).into_iter().flat_map(|f| f.widgets.iter().map(|w| (w.obj, f.name.clone())).collect::<Vec<_>>()).collect();

    let page_list = pdfcraft_model::pages(doc);
    for &pi in &by_page {
        let page = page_list.get(pi).cloned().ok_or(RedactError::Unreadable(pi + 1))?;
        let page_marks: Vec<&Mark> = chosen.iter().copied().filter(|m| m.page == pi).collect();
        let rects: Vec<[f64; 4]> = page_marks.iter().flat_map(|m| m.rects.iter().copied()).collect();
        // 1. Content.
        redact_content(doc, &page, pi, &rects, &mut report)?;
        // 2. Annotations: the marks, and whatever lies under them (with their pop-ups).
        remove_annotations(doc, &page, &page_marks, &rects, &widget_owner, &mut gone, &mut report)?;
    }

    // 3. Form fields with a widget under a mark (all their widgets go).
    for name in &gone.fields {
        if pdfcraft_forms::fields(doc).iter().any(|f| &f.name == name) {
            pdfcraft_forms::delete_field(doc, name)?;
            report.fields += 1;
        }
    }

    // 4. The boxes. The streams this run drew them with are exempt from the checks below, and
    // only these: a stream that merely claims to be one isn't.
    let overlay_refs = draw_boxes(doc, &by_page, &chosen)?;

    // The thumbnails and private data of redacted pages show or quote what was removed, whatever
    // the sanitize options say.
    sanitize::drop_page_leaks(doc, &by_page)?;

    // 5. Sanitize what the removed content may have left around.
    let outcome = sanitize::run(doc, &opts.sanitize, opts.layers, &sanitize::Marked { pages: &by_page, widgets: &gone.widgets })?;
    report.sanitized = outcome.counts;
    report.layers = outcome.layers;

    // 6. Verify: nothing readable may remain under a region.
    let residue = verify_pages(doc, &by_page, &chosen, &overlay_refs)?;
    if residue > 0 {
        return Err(RedactError::Residue(residue));
    }
    // The previous revision still holds the removed content: the next save must rewrite (and the
    // rewritten file is a different file, so it gets a new identifier).
    doc.require_full_save_with_new_id();
    // 7. Prove it: sweep the output (sanitized, so surviving Info/XMP text counts) for
    // everything that was removed, on every surface. Gaps fail like survivors do.
    snapshot.add_removed_strings(&gone.strings);
    snapshot.set_overlay_streams(doc, &overlay_refs)?;
    let proof = snapshot.prove(doc, &ProofOptions { allow_signed: opts.allow_signed || proof_opts.allow_signed, ..*proof_opts });
    let error = if proof.survivors() > 0 {
        Some(RedactError::Residue(proof.survivors()))
    } else if !proof.passed() {
        Some(RedactError::ProofIncomplete { unswept: proof.unswept(), unverifiable: proof.unverifiable(), failures: proof.failures.len() })
    } else {
        None
    };
    if let Some(error) = error {
        *failed_proof = Some(proof);
        return Err(error);
    }
    *kept = Some(snapshot);
    Ok((report, proof))
}

/// Cut what lies under `rects` out of the content of `page` (and its forms), rewriting the
/// streams into new objects, and clean the tags that pointed at what went.
fn redact_content(doc: &mut Document, page: &pdfcraft_model::Page, pi: usize, rects: &[[f64; 4]], report: &mut Report) -> Result<(), RedactError> {
    let (list, data) = page_streams(doc, &page.dict, pi)?;
    let resources = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    let out = {
        let mut scope = Scope::new(rects, Mode::Apply, report);
        process(doc, &mut scope, &data, &resources, pdfcraft_content::Matrix::IDENTITY)
    };
    if let Some(reason) = out.failure {
        return Err(RedactError::Unsupported { page: pi + 1, reason });
    }
    let mut new_list = list.clone();
    let mut changed = false;
    for (i, new) in out.streams.into_iter().enumerate() {
        let Some(bytes) = new else { continue };
        let (Some(original), Some(slot)) = (list.get(i), new_list.get_mut(i)) else { continue };
        let mut dict = match &*doc.resolve(original) {
            Object::Stream(s) => s.dict.clone(),
            _ => Dict::new(),
        };
        dict.remove(b"Length");
        // Edit ▸ Add content keeps an added item's source text in `/PCAdded`; once its glyphs
        // are redacted, keeping it would leave the redacted text in the file. The item stays
        // as plain drawn content but is no longer editable.
        dict.remove(b"PCAdded");
        // A new object: the original may be shared with other pages.
        *slot = Object::Ref(doc.add(Object::Stream(Stream::flate(dict, &bytes))));
        changed = true;
    }
    if !(changed || !out.xobjects.is_empty() || !out.properties.is_empty()) {
        return Ok(());
    }
    let mut res = resources;
    if !out.xobjects.is_empty() {
        let mut xo = res.get(b"XObject").and_then(|x| doc.resolve(x).as_dict().cloned()).unwrap_or_default();
        for (n, r) in &out.xobjects {
            xo.set(n.clone(), Object::Ref(*r));
        }
        res.set(b"XObject".to_vec(), Object::Dict(xo));
    }
    // Property lists that held the alternate text of removed content, now without it.
    if !out.properties.is_empty() {
        let mut props = res.get(b"Properties").and_then(|x| doc.resolve(x).as_dict().cloned()).unwrap_or_default();
        for (n, list) in &out.properties {
            props.set(n.clone(), Object::Dict(list.clone()));
        }
        res.set(b"Properties".to_vec(), Object::Dict(props));
    }
    doc.update_dict(page.obj, |d| {
        d.set(b"Contents".to_vec(), Object::Array(new_list));
        d.set(b"Resources".to_vec(), Object::Dict(res));
    })?;
    // Tags must not keep what the page no longer shows.
    let after = doc.get(page.obj).as_dict().cloned().unwrap_or_default();
    let (_, new_data) = page_streams(doc, &after, pi)?;
    let (before, now) = (data.join(&b'\n'), new_data.join(&b'\n'));
    let (changed_ids, empty_ids) = tags::touched(&before, &now);
    let mut gone = tags::retired_names(&before, &now);
    // Names rewritten forms retired against resources they inherited from this page: the
    // originals leave the page's resources — and the tree nodes above — with them.
    gone.extend(out.inherited_gone.iter().cloned());
    gone.sort_unstable();
    gone.dedup();
    let rewritten = tags::retire_forms(doc, page.obj, &gone)?;
    report.tags += tags::clean(doc, page.obj, &changed_ids, &empty_ids, &rewritten)?;
    Ok(())
}

/// Remove the page's marks and the annotations whose rectangle or appearance overlaps one (with
/// their replies and pop-ups), noting what they said and which fields they belonged to.
fn remove_annotations(
    doc: &mut Document,
    page: &pdfcraft_model::Page,
    page_marks: &[&Mark],
    rects: &[[f64; 4]],
    widget_owner: &[(ObjRef, String)],
    gone: &mut Gone,
    report: &mut Report,
) -> Result<(), RedactError> {
    let annots = annots_of(doc, &doc.get(page.obj).as_dict().cloned().unwrap_or_default());
    let mut removed: Vec<ObjRef> = Vec::new();
    for a in &annots {
        let Some(r) = a.as_ref() else { continue };
        let obj = doc.get(r);
        let Some(d) = obj.as_dict() else { continue };
        let subtype = d.name(b"Subtype").unwrap_or(b"");
        if subtype == b"Redact" {
            if page_marks.iter().any(|m| m.obj == r) {
                removed.push(r);
            }
            continue;
        }
        if subtype == b"Popup" {
            continue;
        }
        // What the annotation may draw: its rectangle and what its appearance covers.
        if sanitize::coverage(doc, d).iter().any(|b| rects.iter().any(|x| pdfcraft_content::overlaps(*x, *b, 0.0))) {
            removed.push(r);
            if subtype == b"Widget" {
                gone.widgets.push(r);
                if let Some((_, name)) = widget_owner.iter().find(|(w, _)| *w == r)
                    && !gone.fields.contains(name)
                {
                    gone.fields.push(name.clone());
                }
            } else {
                report.annotations += 1;
            }
        }
    }
    // Replies to removed annotations (and their pop-ups) go too.
    report.annotations += sanitize::remove_replies(doc, &annots, &mut removed);
    // What the removed annotations said (a mark's own text is not removed content).
    for r in &removed {
        let obj = doc.get(*r);
        if let Some(d) = obj.as_dict().filter(|d| d.name(b"Subtype") != Some(b"Redact")) {
            gone.strings.extend(annotation_strings(doc, d, d.name(b"Subtype") == Some(b"Widget")));
        }
    }
    report.tags += tags::drop_objects(doc, &removed)?;
    let kept: Vec<Object> = annots
        .into_iter()
        .filter(|a| {
            let Some(r) = a.as_ref() else { return true };
            if removed.contains(&r) {
                return false;
            }
            // Pop-ups of removed annotations go too.
            let obj = doc.get(r);
            !obj.as_dict().and_then(|d| d.get(b"Parent")).and_then(Object::as_ref).is_some_and(|p| removed.contains(&p))
        })
        .collect();
    doc.update_dict(page.obj, |d| {
        if kept.is_empty() {
            d.remove(b"Annots");
        } else {
            d.set(b"Annots".to_vec(), Object::Array(kept));
        }
    })?;
    Ok(())
}

/// Draw the boxes (and overlay text) of the marks into their pages, and return the streams
/// that were added for it.
fn draw_boxes(doc: &mut Document, by_page: &[usize], chosen: &[&Mark]) -> Result<Vec<ObjRef>, RedactError> {
    let mut overlay_refs: Vec<ObjRef> = Vec::new();
    let page_list = pdfcraft_model::pages(doc);
    for &pi in by_page {
        let page_marks: Vec<&Mark> = chosen.iter().copied().filter(|m| m.page == pi).collect();
        let current = page_list.get(pi).ok_or(RedactError::Unreadable(pi + 1))?;
        let content = overlay_content(&page_marks, current.crop(doc));
        if !content.is_empty() {
            let before = contents_list(doc, &current.dict, pi)?;
            pdfcraft_edit::stamp(doc, pi, "Redaction", content)?;
            // The overlay stream is the one `stamp` appended: a reference that wasn't there.
            let after = doc.get(current.obj).as_dict().cloned().unwrap_or_default();
            if let Some(Object::Ref(r)) = contents_list(doc, &after, pi)?.last()
                && !before.iter().any(|o| o.as_ref() == Some(*r))
            {
                overlay_refs.push(*r);
            }
        }
    }
    Ok(overlay_refs)
}

/// Re-read each redacted page with the verifying interpreter: how many glyphs or inline images
/// are still under a region (the overlay streams this run drew are left out).
fn verify_pages(doc: &mut Document, by_page: &[usize], chosen: &[&Mark], overlay_refs: &[ObjRef]) -> Result<usize, RedactError> {
    let mut residue = 0;
    let page_list = pdfcraft_model::pages(doc);
    for &pi in by_page {
        let page = page_list.get(pi).cloned().ok_or(RedactError::Unreadable(pi + 1))?;
        let rects: Vec<[f64; 4]> = chosen.iter().filter(|m| m.page == pi).flat_map(|m| m.rects.iter().copied()).collect();
        let (list, data) = page_streams(doc, &page.dict, pi)?;
        // The overlay streams this run drew have text of their own: the overlay label is allowed.
        let original: Vec<Vec<u8>> =
            list.iter().zip(data).filter(|(o, _)| !o.as_ref().is_some_and(|r| overlay_refs.contains(&r))).map(|(_, d)| d).collect();
        let resources = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
        let mut scratch = Report::default();
        let mut scope = Scope::new(&rects, Mode::Verify, &mut scratch);
        let out = process(doc, &mut scope, &original, &resources, pdfcraft_content::Matrix::IDENTITY);
        if let Some(reason) = out.failure {
            return Err(RedactError::Unsupported { page: pi + 1, reason });
        }
        residue += out.residue;
    }
    Ok(residue)
}

/// Remove redaction marks without applying them (`None` = all).
pub fn clear_marks(doc: &mut Document, pages: Option<&[usize]>) -> Result<usize, RedactError> {
    sanitize::promote_direct_annots(doc)?;
    let all = marks(doc);
    let doomed: Vec<ObjRef> = all.iter().filter(|m| pages.is_none_or(|p| p.contains(&m.page))).map(|m| m.obj).collect();
    if doomed.is_empty() {
        return Err(RedactError::NothingToApply);
    }
    for p in pdfcraft_model::pages(doc) {
        let annots = annots_of(doc, &p.dict);
        let kept: Vec<Object> = annots
            .iter()
            .filter(|a| {
                let Some(r) = a.as_ref() else { return true };
                let parent = doc.get(r).as_dict().and_then(|d| d.get(b"Parent")).and_then(Object::as_ref);
                !doomed.contains(&r) && !parent.is_some_and(|p| doomed.contains(&p))
            })
            .cloned()
            .collect();
        if kept.len() != annots.len() {
            doc.update_dict(p.obj, |d| d.set(b"Annots".to_vec(), Object::Array(kept)))?;
        }
    }
    Ok(doomed.len())
}
