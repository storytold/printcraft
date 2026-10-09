//! The non-text content under the regions: images, paths and shadings, and the placement of the
//! glyphs still shown, checked independently of the interpreter.

use std::collections::HashSet;

use pdfcraft_content::{Matrix, contains, overlaps, parse};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use super::extract::{bbox_of, matrix_of, page_content, res_dict};
use super::util::{Ctx, image_codec};
use crate::limits::{MAX_DEPTH, MAX_PIXELS};

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Finding {
    /// This many glyphs are placed inside the region.
    Text(usize),
    Image,
    Path,
    Shading,
    Missing,
    Unverifiable(&'static str),
}

#[derive(Clone)]
pub(super) struct Gs {
    pub(super) ctm: Matrix,
    /// Device-space boxes cut out of the clip (even-odd clips with a hole).
    pub(super) holes: Vec<[f64; 4]>,
}

pub(super) struct Geo<'a> {
    pub(super) doc: &'a Document,
    pub(super) ctx: &'a Ctx,
    pub(super) regions: &'a [[f64; 4]],
    pub(super) found: Vec<(usize, Finding)>,
    /// Pattern, soft-mask and Type3 resources already searched for images.
    pub(super) searched: HashSet<ObjRef>,
}

/// What still paints inside `regions` on a page of the output (the overlay boxes excepted):
/// images with pixels there, paths and shadings not clipped away from there.
pub(super) fn check_geometry(doc: &Document, page: &Dict, regions: &[[f64; 4]], ctx: &Ctx) -> Vec<(usize, Finding)> {
    let unverifiable = |why: &'static str| (0..regions.len()).map(|i| (i, Finding::Unverifiable(why))).collect();
    // The redaction boxes are painted on purpose.
    let Ok(data) = page_content(doc, page, ctx, true) else { return unverifiable("the page content can't be read in full") };
    let mut geo = Geo { doc, ctx, regions, found: Vec::new(), searched: HashSet::new() };
    let res = page.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    geo.run(&data, &res, Matrix::IDENTITY, Vec::new(), 0);
    geo.found.dedup();
    geo.found
}

impl Geo<'_> {
    fn touching(&self, b: [f64; 4]) -> Vec<usize> {
        (0..self.regions.len()).filter(|&i| self.regions.get(i).is_some_and(|r| overlaps(*r, b, 0.0))).collect()
    }

    /// Does something painting over `within` reach region `i` through the clip? Only the part of
    /// the region the painting could touch has to be cut out.
    fn unclipped(&self, i: usize, within: [f64; 4], gs: &Gs) -> bool {
        let Some(r) = self.regions.get(i) else { return false };
        let reach = [r[0].max(within[0]), r[1].max(within[1]), r[2].min(within[2]), r[3].min(within[3])];
        !gs.holes.iter().any(|h| contains(*h, reach, 0.01))
    }

    fn run(&mut self, data: &[u8], res: &Dict, ctm: Matrix, holes: Vec<[f64; 4]>, depth: usize) {
        if depth > MAX_DEPTH {
            for i in 0..self.regions.len() {
                self.found.push((i, Finding::Unverifiable("forms are nested too deep to check")));
            }
            return;
        }
        let xobjects = res_dict(self.doc, res, b"XObject");
        self.hidden_images(res, depth);
        let mut gs = Gs { ctm, holes };
        let mut stack: Vec<Gs> = Vec::new();
        // The current path in device space: its subpaths, and the clip operator it carries.
        let mut sub: Vec<Vec<(f64, f64)>> = Vec::new();
        let mut clip: Option<bool> = None;
        for op in parse(data).ops {
            let o = op.op.as_slice();
            if matches!(o, b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re" | b"W" | b"W*") {
                extend_path(&op, &gs.ctm, &mut sub, &mut clip);
                continue;
            }
            if matches!(o, b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n") {
                self.paint_path(o, &mut gs, &sub, clip);
                sub.clear();
                clip = None;
                continue;
            }
            sub.clear();
            clip = None;
            match o {
                b"q" => stack.push(gs.clone()),
                b"Q" => {
                    if let Some(g) = stack.pop() {
                        gs = g;
                    }
                }
                b"cm" => {
                    if let Some(m) = op.nums::<6>() {
                        gs.ctm = Matrix(m).then(&gs.ctm);
                    }
                }
                b"sh" => {
                    for i in 0..self.regions.len() {
                        if self.unclipped(i, [f64::MIN, f64::MIN, f64::MAX, f64::MAX], &gs) {
                            self.found.push((i, Finding::Shading));
                        }
                    }
                }
                b"BI" => {
                    for i in self.touching(gs.ctm.bbox([0.0, 0.0, 1.0, 1.0])) {
                        self.found.push((i, Finding::Image));
                    }
                }
                b"Do" => {
                    let Some(entry) = op.name(0).and_then(|n| xobjects.get(n)) else { continue };
                    let obj = self.doc.resolve(entry);
                    let Object::Stream(s) = &*obj else { continue };
                    match s.dict.name(b"Subtype") {
                        Some(b"Image") => {
                            for i in self.touching(gs.ctm.bbox([0.0, 0.0, 1.0, 1.0])) {
                                let Some(region) = self.regions.get(i) else { continue };
                                match image_cleared(self.doc, self.ctx, s, &gs.ctm, *region) {
                                    Ok(true) => {}
                                    Ok(false) => self.found.push((i, Finding::Image)),
                                    Err(()) => self.found.push((i, Finding::Unverifiable("an image under the region can't be inspected"))),
                                }
                            }
                        }
                        Some(b"Form") => self.form(s, &gs, res, depth),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// A path-painting operator `o` ends the current path: report what it paints over a region,
    /// and let an even-odd clip add its holes to `gs`.
    fn paint_path(&mut self, o: &[u8], gs: &mut Gs, sub: &[Vec<(f64, f64)>], clip: Option<bool>) {
        let pts: Vec<(f64, f64)> = sub.iter().flatten().copied().collect();
        let bbox = pts.iter().fold(None, |b: Option<[f64; 4]>, &(u, v)| {
            Some(match b {
                None => [u, v, u, v],
                Some(b) => [b[0].min(u), b[1].min(v), b[2].max(u), b[3].max(v)],
            })
        });
        if o != b"n"
            && let Some(b) = bbox
        {
            for i in self.touching(b) {
                if self.unclipped(i, b, gs) {
                    self.found.push((i, Finding::Path));
                }
            }
        }
        // An even-odd clip of two or more boxes lets through what only one of them covers:
        // what both cover (the overlap with the first) is cut out of the clip. This is
        // how redaction clips regions out of what it leaves.
        if clip == Some(true)
            && let Some(outer) = sub.first().map(|s| bounds(s))
        {
            for s in sub.iter().skip(1) {
                let b = bounds(s);
                let hole = [outer[0].max(b[0]), outer[1].max(b[1]), outer[2].min(b[2]), outer[3].min(b[3])];
                if hole[0] < hole[2] && hole[1] < hole[3] {
                    gs.holes.push(hole);
                }
            }
        }
    }

    fn unverifiable(&mut self, why: &'static str) {
        for i in 0..self.regions.len() {
            self.found.push((i, Finding::Unverifiable(why)));
        }
    }

    fn form(&mut self, s: &Stream, gs: &Gs, parent: &Dict, depth: usize) {
        let (Ok(m), Ok(bbox)) = (matrix_of(self.doc, &s.dict), bbox_of(self.doc, &s.dict)) else {
            self.unverifiable("a form under the region has a malformed /Matrix or /BBox");
            return;
        };
        let fctm = m.then(&gs.ctm);
        if let Some(bb) = bbox
            && self.touching(fctm.bbox(bb)).is_empty()
        {
            return;
        }
        let Ok(data) = self.ctx.decode(s) else {
            self.unverifiable("a form under the region can't be decoded in full");
            return;
        };
        let own = s.dict.get(b"Resources").and_then(|r| self.doc.resolve(r).as_dict().cloned());
        self.run(&data, own.as_ref().unwrap_or(parent), fctm, gs.holes.clone(), depth + 1);
    }

    /// Images drawn from tiling patterns, soft-mask groups and Type3 glyph procedures can't be
    /// placed or cleared by this check: if `res` reaches any, every region is unverifiable.
    fn hidden_images(&mut self, res: &Dict, depth: usize) {
        let doc = self.doc;
        let mut streams: Vec<(Option<ObjRef>, Stream)> = Vec::new();
        let take = |o: &Object, streams: &mut Vec<(Option<ObjRef>, Stream)>| {
            if let Object::Stream(s) = &*doc.resolve(o) {
                streams.push((o.as_ref(), s.clone()));
            }
        };
        for (_, p) in res_dict(doc, res, b"Pattern").iter() {
            take(p, &mut streams);
        }
        for (_, g) in res_dict(doc, res, b"ExtGState").iter() {
            if let Some(Object::Dict(sm)) = doc.resolve(g).as_dict().and_then(|g| g.get(b"SMask")).map(|m| (*doc.resolve(m)).clone()).as_ref()
                && let Some(group) = sm.get(b"G")
            {
                take(group, &mut streams);
            }
        }
        for (_, f) in res_dict(doc, res, b"Font").iter() {
            let Some(font) = doc.resolve(f).as_dict().cloned() else { continue };
            if font.name(b"Subtype") != Some(b"Type3") {
                continue;
            }
            if let Some(procs) = font.get(b"CharProcs").and_then(|c| doc.resolve(c).as_dict().cloned()) {
                for (_, p) in procs.iter() {
                    take(p, &mut streams);
                }
            }
        }
        for (id, s) in streams {
            if let Some(r) = id
                && !self.searched.insert(r)
            {
                continue;
            }
            match self.has_image(&s, res, depth + 1) {
                Ok(false) => {}
                Ok(true) => self.unverifiable("an image inside a pattern, soft mask or Type3 glyph can't be placed or cleared"),
                Err(()) => self.unverifiable("a pattern, soft mask or Type3 glyph can't be read in full"),
            }
        }
    }

    /// Does the content of `s` (forms it draws included) draw an image?
    fn has_image(&mut self, s: &Stream, parent: &Dict, depth: usize) -> Result<bool, ()> {
        if depth > MAX_DEPTH {
            return Err(());
        }
        let data = self.ctx.decode(s)?;
        let own = s.dict.get(b"Resources").and_then(|r| self.doc.resolve(r).as_dict().cloned());
        let res = own.as_ref().unwrap_or(parent);
        let xobjects = res_dict(self.doc, res, b"XObject");
        for op in parse(&data).ops {
            match op.op.as_slice() {
                b"BI" => return Ok(true),
                b"Do" => {
                    let Some(entry) = op.name(0).and_then(|n| xobjects.get(n)) else { continue };
                    let Object::Stream(x) = &*self.doc.resolve(entry) else { continue };
                    match x.dict.name(b"Subtype") {
                        Some(b"Image") => return Ok(true),
                        Some(b"Form") => {
                            if let Some(r) = entry.as_ref()
                                && !self.searched.insert(r)
                            {
                                continue;
                            }
                            if self.has_image(x, res, depth + 1)? {
                                return Ok(true);
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Ok(false)
    }
}

/// Extend the current path (`sub`: its subpaths in device space; `clip`: the clip operator it
/// carries) by one construction operator.
fn extend_path(op: &pdfcraft_content::Op, ctm: &Matrix, sub: &mut Vec<Vec<(f64, f64)>>, clip: &mut Option<bool>) {
    let o = op.op.as_slice();
    let nums: Vec<f64> = op.operands.iter().filter_map(Object::as_f64).collect();
    let at = |x: f64, y: f64| ctm.apply(x, y);
    match o {
        b"m" => {
            if let [x, y] = nums[..] {
                sub.push(vec![at(x, y)]);
            }
        }
        b"l" | b"c" | b"v" | b"y" => {
            if let Some(cur) = sub.last_mut() {
                cur.extend(nums.as_chunks::<2>().0.iter().map(|p| at(p[0], p[1])));
            }
        }
        b"re" => {
            if let [x, y, w, h] = nums[..] {
                sub.push(vec![at(x, y), at(x + w, y), at(x + w, y + h), at(x, y + h)]);
            }
        }
        b"W" => *clip = Some(false),
        b"W*" => *clip = Some(true),
        _ => {}
    }
}

pub(super) fn bounds(pts: &[(f64, f64)]) -> [f64; 4] {
    pts.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, &(x, y)| [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)])
}

/// Are the samples of image `s` whose centres fall in `region` all clear (zero; for image masks,
/// the sample that does not paint)? `Err` when the image can't be inspected.
#[allow(clippy::result_unit_err)]
pub(super) fn image_cleared(doc: &Document, ctx: &Ctx, s: &Stream, ctm: &Matrix, region: [f64; 4]) -> Result<bool, ()> {
    if image_codec(&s.dict) {
        // Still encoded pixels: nothing was cleared.
        return Ok(false);
    }
    let (w, h) = (s.dict.int(b"Width").ok_or(())?, s.dict.int(b"Height").ok_or(())?);
    if w <= 0 || h <= 0 || (w as u64).saturating_mul(h as u64) > MAX_PIXELS {
        return Err(());
    }
    let (w, h) = (usize::try_from(w).map_err(|_| ())?, usize::try_from(h).map_err(|_| ())?);
    let mask = matches!(s.dict.get(b"ImageMask"), Some(Object::Bool(true)));
    let (ncomp, bpc) = if mask {
        (1, 1)
    } else {
        let cs = doc.resolve(s.dict.get(b"ColorSpace").ok_or(())?);
        let name = match &*cs {
            Object::Name(n) => n.clone(),
            Object::Array(a) => a.first().and_then(Object::as_name).ok_or(())?.to_vec(),
            _ => return Err(()),
        };
        let n = match name.as_slice() {
            b"DeviceGray" | b"G" | b"CalGray" | b"Indexed" | b"I" | b"Separation" => 1,
            b"DeviceRGB" | b"RGB" | b"CalRGB" | b"Lab" => 3,
            b"DeviceCMYK" | b"CMYK" => 4,
            // One component per colorant name.
            b"DeviceN" => doc.resolve(cs.as_array().and_then(|a| a.get(1)).ok_or(())?).as_array().ok_or(())?.len(),
            b"ICCBased" => match &*doc.resolve(cs.as_array().and_then(|a| a.get(1)).ok_or(())?) {
                Object::Stream(icc) => usize::try_from(icc.dict.int(b"N").ok_or(())?).map_err(|_| ())?,
                _ => return Err(()),
            },
            _ => return Err(()),
        };
        (n, usize::try_from(s.dict.int(b"BitsPerComponent").ok_or(())?).map_err(|_| ())?)
    };
    if !matches!(bpc, 1 | 2 | 4 | 8 | 16) || ncomp == 0 || ncomp > 32 {
        return Err(());
    }
    let data = ctx.decode(s)?;
    let bits = ncomp.checked_mul(bpc).ok_or(())?;
    let row = w.checked_mul(bits).ok_or(())?.div_ceil(8);
    if data.len() < row.checked_mul(h).ok_or(())? {
        return Err(());
    }
    let inverted = s.dict.get(b"Decode").and_then(Object::as_array).and_then(|d| d.first()).and_then(Object::as_f64) == Some(1.0);
    let want = u8::from(mask && !inverted);
    for j in 0..h {
        let v = 1.0 - (j as f64 + 0.5) / h as f64;
        // Rows that don't reach the region hold nothing to check.
        let ((ax, ay), (bx, by)) = (ctm.apply(0.0, v), ctm.apply(1.0, v));
        if !overlaps(region, [ax.min(bx), ay.min(by) - 0.01, ax.max(bx), ay.max(by) + 0.01], 0.0) {
            continue;
        }
        for i in 0..w {
            let (x, y) = ctm.apply((i as f64 + 0.5) / w as f64, v);
            if x < region[0] || x > region[2] || y < region[1] || y > region[3] {
                continue;
            }
            let start = j.checked_mul(row).and_then(|v| v.checked_mul(8)).and_then(|v| v.checked_add(i.checked_mul(bits)?)).ok_or(())?;
            for bit in start..start.checked_add(bits).ok_or(())? {
                let byte = data.get(bit / 8).ok_or(())?;
                if (byte >> (7 - bit % 8)) & 1 != want {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}
