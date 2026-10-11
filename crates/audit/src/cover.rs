//! Walk page content streams and collect the opaque filled rectangles they draw.
//!
//! This is the geometric half of the faux-redaction audit: a rectangle filled with an opaque
//! color (`re` + `f`) is the classic way a "redaction" is faked. Graphics state tracked here is
//! deliberately small: the CTM stack (`q`/`Q`/`cm`), the non-stroking color (`g`/`rg`/`k`) and
//! the non-stroking alpha from ExtGState (`gs` reading `/ca`). Form XObjects are followed
//! read-only with a depth cap. Anything fancier (arbitrary filled paths, shadings, patterns) is
//! out of scope and documented as such.
//!
//! Paint order: a page's content streams are one stream in pieces (ISO 32000-2 §7.8.2), so they
//! are parsed joined and walked once with a single graphics state — state set in an earlier
//! stream (a `cm`, a fill color) is still in force in the later ones. Each rectangle records the
//! position of the fill operator that painted it, as `(content-stream index, operator index in
//! the joined stream)`; rectangles inside a form XObject carry the `Do` operator's position.
//! Positions compare lexicographically in paint order.

use pdfcraft_content::{Matrix, Op, Pieces, parse};
use pdfcraft_cos::{Dict, Document, Object};

use crate::AuditError;

/// Maximum form-XObject nesting followed (cyclic references are cut by the cap).
const MAX_DEPTH: usize = 8;
/// Bound on total operators walked per page, so a hostile stream cannot spin forever.
/// Hitting it fails the audit: a truncated walk must never report "clean".
const MAX_OPS: usize = 2_000_000;

/// One opaque filled rectangle, in page user space.
#[derive(Clone, Debug, PartialEq)]
pub struct FilledRect {
    /// `[x0, y0, x1, y1]`, normalized so `x0 <= x1` and `y0 <= y1`.
    pub rect: [f64; 4],
    /// The fill color as sRGB-ish `[r, g, b]` (CMYK is converted approximately).
    pub rgb: [f64; 3],
    /// Paint position: `(content-stream index, operator index in the joined stream)`.
    /// Rectangles inside a form XObject carry the `Do` operator's position.
    pub pos: (usize, usize),
}

#[derive(Clone)]
struct Gs {
    ctm: Matrix,
    fill: [f64; 3],
    alpha: f64,
}

impl Default for Gs {
    fn default() -> Self {
        Gs { ctm: Matrix::IDENTITY, fill: [0.0, 0.0, 0.0], alpha: 1.0 }
    }
}

struct Walker<'a> {
    doc: &'a Document,
    pieces: &'a Pieces,
    out: Vec<FilledRect>,
    ops_seen: usize,
}

impl<'a> Walker<'a> {
    fn bump(&mut self) -> Result<(), AuditError> {
        self.ops_seen += 1;
        if self.ops_seen > MAX_OPS {
            return Err(AuditError::TooManyOps);
        }
        Ok(())
    }

    /// Walk one operator slice with the given graphics state. `at_do` is the `Do` position to
    /// stamp on rectangles found inside a form XObject, or `None` at page level (where each
    /// fill operator's own position is used).
    fn walk_ops(
        &mut self,
        ops: &[Op],
        gs: &mut Gs,
        stack: &mut Vec<Gs>,
        resources: &Dict,
        depth: usize,
        at_do: Option<(usize, usize)>,
    ) -> Result<(), AuditError> {
        // A pending `re` rectangle, in the user space active when it was built.
        let mut pending: Option<[f64; 4]> = None;
        for (i, op) in ops.iter().enumerate() {
            self.bump()?;
            // Paint position of this operator: its own `(piece, joined index)` at page level,
            // the enclosing `Do`'s position inside a form XObject.
            let pos = match at_do {
                Some(p) => p,
                None => (self.pieces.pieces_of(op).0, i),
            };
            match op.op.as_slice() {
                b"q" => stack.push(gs.clone()),
                b"Q" => {
                    if let Some(g) = stack.pop() {
                        *gs = g;
                    }
                    pending = None;
                }
                b"cm" => {
                    if let Some(m) = Matrix::from_operands(&op.operands)
                        && m.0.iter().all(|v| v.is_finite())
                    {
                        gs.ctm = m.then(&gs.ctm);
                    }
                    pending = None;
                }
                b"g" => {
                    if let Some(v) = op.num(0).filter(|v| v.is_finite()) {
                        let v = v.clamp(0.0, 1.0);
                        gs.fill = [v, v, v];
                    }
                }
                b"rg" => {
                    if let Some([r, g, b]) = op.nums::<3>()
                        && [r, g, b].iter().all(|v| v.is_finite())
                    {
                        gs.fill = [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)];
                    }
                }
                b"k" => {
                    if let Some([c, m, y, k]) = op.nums::<4>()
                        && [c, m, y, k].iter().all(|v| v.is_finite())
                    {
                        // Naive CMYK -> RGB; good enough to tell black from white.
                        let (c, m, y, k) = (c.clamp(0.0, 1.0), m.clamp(0.0, 1.0), y.clamp(0.0, 1.0), k.clamp(0.0, 1.0));
                        gs.fill = [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)];
                    }
                }
                b"gs" => {
                    if let Some(name) = op.name(0) {
                        gs.alpha = ext_alpha(self.doc, resources, name);
                    }
                }
                b"re" => {
                    pending = op.nums::<4>().and_then(|[x, y, w, h]| {
                        if [x, y, w, h].iter().all(|v| v.is_finite()) && w != 0.0 && h != 0.0 {
                            let r = gs.ctm.bbox([x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h)]);
                            (r[2] > r[0] && r[3] > r[1]).then_some(r)
                        } else {
                            None
                        }
                    });
                }
                b"f" | b"F" | b"f*" | b"B" | b"B*" => {
                    if let Some(r) = pending.take()
                        && gs.alpha >= 1.0
                    {
                        self.out.push(FilledRect { rect: r, rgb: gs.fill, pos });
                    }
                    // Any other path construction clears the pending rectangle.
                }
                b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"S" | b"s" | b"n" | b"W" | b"W*" => {
                    pending = None;
                }
                b"Do" => {
                    if depth < MAX_DEPTH
                        && let Some(name) = op.name(0)
                    {
                        self.walk_xobject(name, gs, resources, depth, pos)?;
                    }
                    pending = None;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Follow a form XObject read-only, composing its matrix onto the current CTM. State
    /// changes inside never leak out; every rectangle found is stamped with the `Do`
    /// position `pos`.
    fn walk_xobject(&mut self, name: &[u8], gs: &Gs, resources: &Dict, depth: usize, pos: (usize, usize)) -> Result<(), AuditError> {
        let xobjects = resources.get(b"XObject").map(|o| self.doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
        let Some(entry) = xobjects.get(name) else { return Ok(()) };
        let resolved = self.doc.resolve(entry);
        let Object::Stream(stream) = &*resolved else { return Ok(()) };
        let dict = &stream.dict;
        if dict.name(b"Subtype") != Some(b"Form") {
            return Ok(());
        }
        let matrix = dict
            .get(b"Matrix")
            .map(|o| self.doc.resolve(o))
            .and_then(|o| o.as_array().map(|a| a.to_vec()))
            .and_then(|a| {
                let v: Vec<f64> = a.iter().filter_map(|o| self.doc.resolve(o).as_f64()).collect();
                (v.len() == 6 && v.iter().all(|x| x.is_finite())).then(|| Matrix([v[0], v[1], v[2], v[3], v[4], v[5]]))
            })
            .unwrap_or(Matrix::IDENTITY);
        let sub_resources = dict.get(b"Resources").map(|o| self.doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
        // A form stream that cannot be decoded fails the audit: it might hide a cover.
        let data = stream.decoded_strict()?;
        let ops = parse(&data).ops;
        // The XObject matrix maps form space into the user space active at `Do`.
        let mut sub_gs = gs.clone();
        sub_gs.ctm = matrix.then(&gs.ctm);
        let mut sub_stack = Vec::new();
        self.walk_ops(&ops, &mut sub_gs, &mut sub_stack, &sub_resources, depth + 1, Some(pos))
    }
}

/// The non-stroking alpha (`/ca`) of a named ExtGState, defaulting to opaque.
fn ext_alpha(doc: &Document, resources: &Dict, name: &[u8]) -> f64 {
    let alpha = resources
        .get(b"ExtGState")
        .map(|o| doc.resolve(o))
        .and_then(|o| o.as_dict().cloned())
        .and_then(|d| d.get(name).map(|o| doc.resolve(o)))
        .and_then(|o| o.as_dict().cloned())
        .and_then(|d| d.get(b"ca").map(|o| doc.resolve(o)).and_then(|o| o.as_f64()));
    alpha.filter(|a| a.is_finite()).map(|a| a.clamp(0.0, 1.0)).unwrap_or(1.0)
}

/// Every opaque filled rectangle drawn by a page's content (and its form XObjects), with paint
/// positions. A page's content streams are walked once, joined, with one graphics state, so
/// state set in an earlier stream is still in force in the later ones. Any content stream that
/// cannot be decoded is an audit failure, never an empty result.
pub fn filled_rects(doc: &Document, page: &Dict) -> Result<Vec<FilledRect>, AuditError> {
    let resources = page.get(b"Resources").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    let contents: Vec<Object> = match page.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        },
    };
    let mut streams = Vec::with_capacity(contents.len());
    for c in &contents {
        if let Object::Stream(s) = &*doc.resolve(c) {
            streams.push(s.decoded_strict()?);
        }
    }
    let pieces = Pieces::join(&streams);
    let parsed = pieces.parse();
    let mut w = Walker { doc, pieces: &pieces, out: Vec::new(), ops_seen: 0 };
    let mut gs = Gs::default();
    let mut stack = Vec::new();
    w.walk_ops(&parsed.ops, &mut gs, &mut stack, &resources, 0, None)?;
    Ok(w.out)
}
