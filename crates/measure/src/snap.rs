//! Bounded path extraction and snapping in PDF user space, including nested Form XObjects.
use crate::{Point, Result, check_points, distance, invalid};
use pdfcraft_content::{Matrix, parse};
use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use serde::Serialize;
use std::collections::HashSet;

const MAX_SEGMENTS: usize = 20_000;
/// Endpoints and midpoints together, across the whole page.
const MAX_TARGETS: usize = 2 * MAX_SEGMENTS;
const MAX_BYTES: usize = 16 * 1024 * 1024;
#[derive(Clone, Debug, Default)]
pub struct Geometry {
    pub segments: Vec<[Point; 2]>,
    pub endpoints: Vec<Point>,
    pub midpoints: Vec<Point>,
    /// A limit stopped extraction early (segments, targets, bytes, streams, nesting).
    pub truncated: bool,
    /// Content streams that couldn't be decoded and were skipped.
    pub unreadable: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapKind {
    Endpoint,
    Midpoint,
    Intersection,
    Path,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Snap {
    pub point: Point,
    pub kind: SnapKind,
    pub distance: f64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapOptions {
    pub endpoints: bool,
    pub midpoints: bool,
    pub intersections: bool,
    pub paths: bool,
}
impl Default for SnapOptions {
    fn default() -> Self {
        Self { endpoints: true, midpoints: true, intersections: true, paths: true }
    }
}
fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}
fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
pub fn intersection(a: Point, b: Point, c: Point, d: Point) -> Option<Point> {
    let r = sub(b, a);
    let s = sub(d, c);
    let denominator = cross(r, s);
    if !denominator.is_finite() || denominator.abs() < 1e-12 {
        return None;
    }
    let t = cross(sub(c, a), s) / denominator;
    let u = cross(sub(c, a), r) / denominator;
    if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) { Some([a[0] + t * r[0], a[1] + t * r[1]]) } else { None }
}
fn nearest(p: Point, a: Point, b: Point) -> Point {
    let v = sub(b, a);
    let length = dot(v, v);
    if length < 1e-18 {
        return a;
    }
    let t = (dot(sub(p, a), v) / length).clamp(0.0, 1.0);
    [a[0] + v[0] * t, a[1] + v[1] * t]
}
/// Save a subpath's implicit closing edge for fill operations. An unpainted or
/// stroke-only open path must not gain that edge.
fn remember_close(closures: &mut Vec<[Point; 2]>, current: Option<Point>, start: Option<Point>) -> bool {
    if let (Some(a), Some(b)) = (current, start)
        && distance(a, b) > 1e-9
    {
        if closures.len() >= MAX_SEGMENTS {
            return false;
        }
        closures.push([a, b]);
    }
    true
}

impl Geometry {
    /// Dense intersection searches are capped independently from path extraction.
    pub fn intersection_limited(&self, at: Point, tolerance: f64) -> bool {
        self.segments.iter().filter(|[a, b]| distance(at, nearest(at, *a, *b)) <= tolerance).take(257).count() > 256
    }
    pub fn snap(&self, at: Point, tolerance: f64, options: SnapOptions) -> Result<Option<Snap>> {
        check_points(&[at])?;
        if !tolerance.is_finite() || !(0.0..=10_000.0).contains(&tolerance) {
            return Err(invalid("snap tolerance must be between 0 and 10000 user units"));
        }
        let mut best: Option<Snap> = None;
        let consider = |best: &mut Option<Snap>, p: Point, kind: SnapKind| {
            let distance = distance(at, p);
            // Discrete targets take precedence over projections at the same distance.
            if distance <= tolerance && best.is_none_or(|b| distance < b.distance - 1e-9) {
                *best = Some(Snap { point: p, kind, distance });
            }
        };
        if options.endpoints {
            for &p in &self.endpoints {
                consider(&mut best, p, SnapKind::Endpoint);
            }
        }
        if options.midpoints {
            for &p in &self.midpoints {
                consider(&mut best, p, SnapKind::Midpoint);
            }
        }
        // Only segments in the pointer's neighbourhood participate in intersection tests.
        // Cap the local quadratic work even on a malicious densely overlapping drawing.
        let local: Vec<_> = self.segments.iter().filter(|[a, b]| distance(at, nearest(at, *a, *b)) <= tolerance).take(256).collect();
        if options.intersections {
            for (i, s) in local.iter().enumerate() {
                for t in local.iter().skip(i + 1) {
                    if let Some(p) = intersection(s[0], s[1], t[0], t[1]) {
                        consider(&mut best, p, SnapKind::Intersection);
                    }
                }
            }
        }
        if options.paths && best.is_none() {
            for [a, b] in &self.segments {
                consider(&mut best, nearest(at, *a, *b), SnapKind::Path);
            }
        }
        Ok(best)
    }
    fn targets(&self) -> usize {
        self.endpoints.len().saturating_add(self.midpoints.len())
    }
    /// Whether the segment was accepted (finite, non-degenerate and within the cap).
    fn segment(&mut self, a: Point, b: Point) -> bool {
        if self.segments.len() >= MAX_SEGMENTS {
            self.truncated = true;
            return false;
        }
        if check_points(&[a, b]).is_ok() && distance(a, b) > 1e-9 {
            self.segments.push([a, b]);
            return true;
        }
        false
    }
    /// Snap targets are recorded only for accepted geometry, and are capped.
    fn target(&mut self, ends: [Point; 2], mid: Point) {
        if self.targets() >= MAX_TARGETS {
            self.truncated = true;
            return;
        }
        self.endpoints.extend(ends);
        self.midpoints.push(mid);
    }
    fn edge(&mut self, a: Point, b: Point) {
        if self.segment(a, b) {
            self.target([a, b], [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
        }
    }
    fn curve(&mut self, points: [Point; 4], depth: usize) {
        let [a, b, c, d] = points;
        if self.segments.len() >= MAX_SEGMENTS {
            self.truncated = true;
            return;
        }
        if depth >= 12 || distance(b, nearest(b, a, d)).max(distance(c, nearest(c, a, d))) <= 0.2 {
            self.segment(a, d);
            return;
        }
        let avg = |a: Point, b: Point| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let (ab, bc, cd) = (avg(a, b), avg(b, c), avg(c, d));
        let (abc, bcd) = (avg(ab, bc), avg(bc, cd));
        let mid = avg(abc, bcd);
        self.curve([a, ab, abc, mid], depth + 1);
        self.curve([mid, bcd, cd, d], depth + 1);
    }
}
/// Rotate a proposed vertex onto the nearest 45-degree ray from the last vertex.
pub fn constrain(last: Point, point: Point) -> Point {
    let v = sub(point, last);
    let length = v[0].hypot(v[1]);
    let angle = (v[1].atan2(v[0]) / std::f64::consts::FRAC_PI_4).round() * std::f64::consts::FRAC_PI_4;
    [last[0] + length * angle.cos(), last[1] + length * angle.sin()]
}
struct Walker<'a> {
    doc: &'a Document,
    geometry: Geometry,
    seen: HashSet<ObjRef>,
    bytes: usize,
    streams: usize,
}
impl Walker<'_> {
    fn walk(&mut self, object: &Object, resources: &Dict, initial: Matrix, depth: usize) -> Result<()> {
        if depth >= 32 || self.streams >= 4096 || self.geometry.truncated {
            self.geometry.truncated = true;
            return Ok(());
        }
        let o = self.doc.resolve(object);
        if let Some(arr) = o.as_array() {
            // Contents arrays form one stream; q/Q and paths may span stream boundaries.
            let mut joined = Vec::new();
            for part in arr {
                let s = self.doc.resolve(part);
                if let Object::Stream(s) = s.as_ref() {
                    let remaining = MAX_BYTES.saturating_sub(self.bytes).saturating_sub(joined.len());
                    if remaining == 0 {
                        self.geometry.truncated = true;
                        break;
                    }
                    // An undecodable part is skipped; the rest of the page still snaps.
                    match s.decoded_within(remaining) {
                        Ok(data) => {
                            joined.extend(data);
                            joined.push(b'\n');
                        }
                        Err(_) => self.geometry.unreadable = self.geometry.unreadable.saturating_add(1),
                    }
                }
            }
            return self.ops(&joined, resources, initial, depth);
        }
        let Object::Stream(stream) = o.as_ref() else { return Ok(()) };
        let remaining = MAX_BYTES.saturating_sub(self.bytes);
        if remaining == 0 {
            self.geometry.truncated = true;
            return Ok(());
        }
        match stream.decoded_within(remaining) {
            Ok(data) => self.ops(&data, resources, initial, depth),
            Err(_) => {
                self.geometry.unreadable = self.geometry.unreadable.saturating_add(1);
                Ok(())
            }
        }
    }
    fn ops(&mut self, data: &[u8], resources: &Dict, initial: Matrix, depth: usize) -> Result<()> {
        self.bytes = self.bytes.saturating_add(data.len());
        self.streams = self.streams.saturating_add(1);
        let mut matrix = initial;
        let mut stack = Vec::new();
        let mut start = None;
        let mut current = None;
        let mut closures = Vec::new();
        let mut path = Geometry::default();
        let point = |m: Matrix, x: f64, y: f64| {
            let (x, y) = m.apply(x, y);
            [x, y]
        };
        for op in parse(data).ops {
            if self.geometry.segments.len().saturating_add(path.segments.len()) >= MAX_SEGMENTS
                || self.geometry.targets().saturating_add(path.targets()) >= MAX_TARGETS
            {
                self.geometry.truncated = true;
                break;
            }
            match String::from_utf8_lossy(&op.op).as_ref() {
                "q" => {
                    if stack.len() < 256 {
                        stack.push(matrix);
                    } else {
                        // Later transforms can't be tracked reliably; keep what was found.
                        self.geometry.truncated = true;
                        break;
                    }
                }
                "Q" => {
                    matrix = stack.pop().unwrap_or(initial);
                }
                "cm" => {
                    if let Some(m) = Matrix::from_operands(&op.operands) {
                        matrix = m.then(&matrix);
                    }
                }
                "m" => {
                    if let Some([x, y]) = op.nums::<2>() {
                        if !remember_close(&mut closures, current, start) {
                            self.geometry.truncated = true;
                            break;
                        }
                        let p = point(matrix, x, y);
                        current = Some(p);
                        start = Some(p);
                    }
                }
                "l" => {
                    if let (Some(a), Some([x, y])) = (current, op.nums::<2>()) {
                        let b = point(matrix, x, y);
                        path.edge(a, b);
                        current = Some(b);
                    }
                }
                "c" | "v" | "y" => {
                    if let Some(a) = current {
                        let curve = match String::from_utf8_lossy(&op.op).as_ref() {
                            "c" => op
                                .nums::<6>()
                                .map(|[x1, y1, x2, y2, x3, y3]| [a, point(matrix, x1, y1), point(matrix, x2, y2), point(matrix, x3, y3)]),
                            "v" => op.nums::<4>().map(|[x2, y2, x3, y3]| [a, a, point(matrix, x2, y2), point(matrix, x3, y3)]),
                            _ => op.nums::<4>().map(|[x1, y1, x3, y3]| {
                                let d = point(matrix, x3, y3);
                                [a, point(matrix, x1, y1), d, d]
                            }),
                        };
                        if let Some([a, b, c, d]) = curve
                            && check_points(&[a, b, c, d]).is_ok()
                        {
                            let before = path.segments.len();
                            path.curve([a, b, c, d], 0);
                            if path.segments.len() > before {
                                path.target([a, d], [(a[0] + 3.0 * b[0] + 3.0 * c[0] + d[0]) / 8.0, (a[1] + 3.0 * b[1] + 3.0 * c[1] + d[1]) / 8.0]);
                            }
                            current = Some(d);
                        }
                    }
                }
                "h" => {
                    if let (Some(a), Some(b)) = (current, start) {
                        path.edge(a, b);
                        current = Some(b);
                    }
                }
                "re" => {
                    if let Some([x, y, w, h]) = op.nums::<4>() {
                        if !remember_close(&mut closures, current, start) {
                            self.geometry.truncated = true;
                            break;
                        }
                        let a = point(matrix, x, y);
                        let b = point(matrix, x + w, y);
                        let c = point(matrix, x + w, y + h);
                        let d = point(matrix, x, y + h);
                        for (f, t) in [(a, b), (b, c), (c, d), (d, a)] {
                            path.edge(f, t);
                        }
                        current = Some(a);
                        start = Some(a);
                    }
                }
                "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" => {
                    let paint = String::from_utf8_lossy(&op.op);
                    if matches!(paint.as_ref(), "f" | "F" | "f*" | "B" | "B*" | "b" | "b*") {
                        if !remember_close(&mut closures, current, start) {
                            self.geometry.truncated = true;
                            break;
                        }
                        // Filling implicitly closes every open subpath, not just the last one.
                        // Stroke-only S leaves its open subpaths unchanged.
                        for [a, b] in closures.drain(..) {
                            if self.geometry.segments.len().saturating_add(path.segments.len()) >= MAX_SEGMENTS
                                || self.geometry.targets().saturating_add(path.targets()) >= MAX_TARGETS
                            {
                                path.truncated = true;
                                break;
                            }
                            path.edge(a, b);
                        }
                    } else if paint == "s"
                        && let (Some(a), Some(b)) = (current, start)
                    {
                        path.edge(a, b);
                    }
                    self.geometry.segments.append(&mut path.segments);
                    self.geometry.endpoints.append(&mut path.endpoints);
                    self.geometry.midpoints.append(&mut path.midpoints);
                    self.geometry.truncated |= path.truncated;
                    closures.clear();
                    current = None;
                    start = None;
                }
                "n" => {
                    self.geometry.truncated |= path.truncated;
                    path = Geometry::default();
                    closures.clear();
                    current = None;
                    start = None;
                }
                "Do" => {
                    if let Some(name) = op.name(0) {
                        let xobjects = resources.get(b"XObject").map(|o| self.doc.resolve(o));
                        if let Some(o) = xobjects.as_ref().and_then(|o| o.as_dict()).and_then(|d| d.get(name)) {
                            let r = o.as_ref();
                            if r.is_some_and(|r| self.seen.contains(&r)) {
                                continue;
                            }
                            let form = self.doc.resolve(o);
                            if let Object::Stream(s) = form.as_ref()
                                && s.dict.name(b"Subtype") == Some(b"Form")
                            {
                                let m = s
                                    .dict
                                    .get(b"Matrix")
                                    .map(|m| self.doc.resolve(m))
                                    .and_then(|o| o.as_array().and_then(|a| Matrix::from_operands(a)))
                                    .unwrap_or_default()
                                    .then(&matrix);
                                let own = s.dict.get(b"Resources").map(|o| self.doc.resolve(o));
                                let res = own.as_ref().and_then(|o| o.as_dict()).unwrap_or(resources);
                                if let Some(r) = r {
                                    self.seen.insert(r);
                                }
                                self.walk(o, res, m, depth + 1)?;
                                if let Some(r) = r {
                                    self.seen.remove(&r);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}
pub fn geometry(doc: &Document, page_index: usize) -> Result<Geometry> {
    let p = crate::page(doc, page_index)?;
    let resources = p.dict.get(b"Resources").map(|o| doc.resolve(o));
    let empty = Dict::new();
    let resources = resources.as_ref().and_then(|o| o.as_dict()).unwrap_or(&empty);
    let mut walker = Walker { doc, geometry: Geometry::default(), seen: HashSet::new(), bytes: 0, streams: 0 };
    // Extraction is lenient: limits and unreadable streams are reported in the geometry.
    if let Some(contents) = p.dict.get(b"Contents") {
        walker.walk(contents, resources, Matrix::IDENTITY, 0)?;
    }
    Ok(walker.geometry)
}
