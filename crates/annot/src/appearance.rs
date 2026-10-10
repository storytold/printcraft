//! Appearance streams (§12.5.5) for the comment types PdfCraft creates, execution plan M5.2.
//!
//! [`build`] draws a normal appearance (`/AP /N`) from the annotation dictionary alone, so the
//! same code serves new comments and restyled ones. Drawings are in page space with
//! `/BBox = /Rect` (identity matrix), except note icons, which are drawn in a 20 × 20 box.
//! It returns `None` for anything it cannot draw faithfully (cloudy borders, unknown line
//! endings, indirect geometry), so callers never replace an appearance with a worse one.
//!
//! The note icons are PdfCraft's own drawings (AGENTS.md §1). Text boxes use the standard
//! Helvetica font with WinAnsi encoding; line breaking uses [`text_width`], an approximation of
//! Helvetica's proportions by character class (no font program or metrics file is bundled).

use pdfcraft_cos::{Dict, Object, PdfString, Stream};
pub use pdfcraft_fonts::{helvetica_width as text_width, wrap};
use pdfcraft_fonts::{literal, win_ansi};

use crate::{NOTE_SIZE, Rgb, n};

/// Length of an arrowhead for a line of width `w`.
pub fn arrow_size(w: f64) -> f64 {
    6.0 + 3.0 * w.max(0.0)
}

/// Nominal radius of a cloudy border's bumps for a line of width `w` (intensity 1).
pub fn cloud_radius(w: f64) -> f64 {
    6.0 + 1.5 * w.max(0.0)
}

/// One of the ten `/LE` names (ISO 32000-2 Table 217). Unknown names are not drawn.
pub(crate) fn known_ending(kind: &[u8]) -> bool {
    std::str::from_utf8(kind).ok().and_then(crate::LineEnding::parse).is_some()
}

/// Draw a line ending (`/LE`) at `tip`, for a line arriving from `from`.
///
/// Returns false for an unknown style or a non-finite direction, so the caller keeps the
/// previous appearance instead of writing `NaN`. `None` and a zero-length direction draw nothing.
fn line_end(c: &mut String, kind: &[u8], tip: (f64, f64), from: (f64, f64), w: f64) -> bool {
    if kind == b"None" {
        return true;
    }
    if !known_ending(kind) {
        return false;
    }
    let (dx, dy) = (from.0 - tip.0, from.1 - tip.1);
    if !dx.is_finite() || !dy.is_finite() {
        return false;
    }
    let len = dx.hypot(dy);
    if !len.is_finite() {
        return false;
    }
    if len == 0.0 {
        return true;
    }
    let s = arrow_size(w);
    if !s.is_finite() {
        return false;
    }
    let (ix, iy) = (dx / len, dy / len);
    match kind {
        b"OpenArrow" | b"ClosedArrow" | b"ROpenArrow" | b"RClosedArrow" => {
            // Open and closed arrows keep their original geometry. A reverse arrow uses the same
            // head, pointing back along the line.
            let (ux, uy) = if kind.starts_with(b"R") { (-ix, -iy) } else { (ix, iy) };
            let (cos, sin) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
            let a = (tip.0 + s * (ux * cos - uy * sin), tip.1 + s * (ux * sin + uy * cos));
            let b = (tip.0 + s * (ux * cos + uy * sin), tip.1 + s * (-ux * sin + uy * cos));
            let op = if kind.ends_with(b"ClosedArrow") { "h B" } else { "S" };
            c.push_str(&format!("{} {} m {} {} l {} {} l {op}\n", n(a.0), n(a.1), n(tip.0), n(tip.1), n(b.0), n(b.1)));
        }
        b"Square" | b"Circle" | b"Diamond" => {
            let h = s / 2.0;
            if kind == b"Square" {
                c.push_str(&format!("{} {} {} {} re h B\n", n(tip.0 - h), n(tip.1 - h), n(s), n(s)));
            } else if kind == b"Circle" {
                c.push_str(&ellipse(tip.0 - h, tip.1 - h, tip.0 + h, tip.1 + h));
                c.push_str("B\n");
            } else {
                let (px, py) = (-iy, ix);
                let v = |along: f64, across: f64| (tip.0 + ix * along + px * across, tip.1 + iy * along + py * across);
                let (a, b, d, e) = (v(h, 0.0), v(0.0, h), v(-h, 0.0), v(0.0, -h));
                c.push_str(&format!("{} {} m {} {} l {} {} l {} {} l h B\n", n(a.0), n(a.1), n(b.0), n(b.1), n(d.0), n(d.1), n(e.0), n(e.1)));
            }
        }
        _ => {
            // Butt is perpendicular to the line. Slash is that stroke turned 45 degrees.
            let (px, py) = (-iy, ix);
            let (sx, sy) = if kind == b"Slash" {
                let q = std::f64::consts::FRAC_1_SQRT_2;
                (px * q - py * q, px * q + py * q)
            } else {
                (px, py)
            };
            let h = s / 2.0;
            c.push_str(&format!("{} {} m {} {} l S\n", n(tip.0 - sx * h), n(tip.1 - sy * h), n(tip.0 + sx * h), n(tip.1 + sy * h)));
        }
    }
    true
}

/// The cubic Bézier segments (first control point, second control point, end) of a smooth curve
/// through every point of `pts`, from the first: a Catmull-Rom spline, so the curve leaves each
/// point parallel to the chord between its neighbours (an end uses itself as the missing
/// neighbour). Ink is drawn so, as Acrobat draws it, rather than as straight segments. The curve
/// stays within its points and these control points.
pub fn smooth_segments(pts: &[(f64, f64)]) -> Vec<[(f64, f64); 3]> {
    let Some(&last) = pts.last() else { return Vec::new() };
    let at = |i: usize| pts.get(i).copied().unwrap_or(last);
    (0..pts.len().saturating_sub(1))
        .map(|i| {
            let (p0, p1, p2, p3) = (at(i.saturating_sub(1)), at(i), at(i + 1), at(i + 2));
            [(p1.0 + (p2.0 - p0.0) / 6.0, p1.1 + (p2.1 - p0.1) / 6.0), (p2.0 - (p3.0 - p1.0) / 6.0, p2.1 - (p3.1 - p1.1) / 6.0), p2]
        })
        .collect()
}

/// `c` operators for [`smooth_segments`] (the path already starts at the first point).
fn smooth_curve(pts: &[(f64, f64)]) -> String {
    smooth_segments(pts).iter().map(|[a, b, e]| format!("{} {} {} {} {} {} c\n", n(a.0), n(a.1), n(b.0), n(b.1), n(e.0), n(e.1))).collect()
}

/// A closed cloudy outline through `pts` (§12.5.4 `/BE /S /C`): each edge becomes a row of
/// half-circle bumps of about `r`, bulging outwards.
pub fn cloud_path(pts: &[(f64, f64)], r: f64) -> String {
    let area: f64 = pts.iter().zip(pts.iter().cycle().skip(1)).map(|(a, b)| a.0 * b.1 - b.0 * a.1).sum();
    // Outward normal of an edge: to the right of travel for counter-clockwise outlines.
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let mut c = String::new();
    for (i, (a, b)) in pts.iter().zip(pts.iter().cycle().skip(1)).enumerate() {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = dx.hypot(dy);
        if i == 0 {
            c.push_str(&format!("{} {} m\n", n(a.0), n(a.1)));
        }
        if len < 1e-6 {
            continue;
        }
        let k = ((len / (2.0 * r)).round() as usize).clamp(1, 10_000);
        let step = len / k as f64;
        let (ux, uy) = (dx / len, dy / len);
        let (nx, ny) = (sign * uy, -sign * ux);
        let h = 4.0 / 3.0 * step / 2.0;
        for j in 0..k {
            let p0 = (a.0 + ux * step * j as f64, a.1 + uy * step * j as f64);
            let p1 = (a.0 + ux * step * (j + 1) as f64, a.1 + uy * step * (j + 1) as f64);
            c.push_str(&format!("{} {} {} {} {} {} c\n", n(p0.0 + nx * h), n(p0.1 + ny * h), n(p1.0 + nx * h), n(p1.1 + ny * h), n(p1.0), n(p1.1)));
        }
    }
    c.push_str("h\n");
    c
}

fn nums(d: &Dict, key: &[u8]) -> Option<Vec<f64>> {
    d.get(key)?.as_array()?.iter().map(|o| o.as_f64()).collect()
}

/// `/C`-style colour arrays: none (transparent), gray, RGB or CMYK.
fn color(d: &Dict, key: &[u8]) -> Option<Option<Rgb>> {
    let c = match d.get(key) {
        None => return Some(None),
        Some(o) => o.as_array()?.iter().map(|o| o.as_f64()).collect::<Option<Vec<f64>>>()?,
    };
    Some(match c.as_slice() {
        [] => None,
        [g] => Some([*g; 3]),
        [r, g, b] => Some([*r, *g, *b]),
        [c, m, y, k] => Some([(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)]),
        _ => return None,
    })
}

fn interior_color(d: &Dict, fallback: Option<Rgb>) -> Option<Rgb> {
    color(d, b"IC").flatten().or(fallback)
}

fn border_width(d: &Dict) -> f64 {
    if let Some(w) = d.get(b"BS").and_then(|b| b.as_dict()).and_then(|b| b.get(b"W")).and_then(|w| w.as_f64()) {
        return w.max(0.0);
    }
    match nums(d, b"Border") {
        Some(b) if b.len() >= 3 => b[2].max(0.0),
        _ => 1.0,
    }
}

/// Dash pattern operator for a dashed border, if any.
fn dash(d: &Dict) -> String {
    let Some(bs) = d.get(b"BS").and_then(|b| b.as_dict()) else { return String::new() };
    if bs.name(b"S") != Some(b"D") {
        return String::new();
    }
    let arr = nums(bs, b"D").filter(|a| !a.is_empty() && a.iter().all(|x| *x >= 0.0)).unwrap_or_else(|| vec![3.0]);
    format!("[{}] 0 d\n", arr.iter().map(|x| n(*x)).collect::<Vec<_>>().join(" "))
}

fn rg(c: Rgb) -> String {
    format!("{} {} {} rg\n", n(c[0]), n(c[1]), n(c[2]))
}

fn rg_stroke(c: Rgb) -> String {
    format!("{} {} {} RG\n", n(c[0]), n(c[1]), n(c[2]))
}

/// `/DA` of a text box: (text colour, font size). Defaults to black 12 pt.
pub fn parse_da(d: &Dict) -> (Rgb, f64) {
    let da = d.get(b"DA").and_then(|o| o.as_string()).map(|s| String::from_utf8_lossy(&s.bytes).into_owned()).unwrap_or_default();
    let toks: Vec<&str> = da.split_whitespace().collect();
    let mut col = [0.0; 3];
    let mut size = 12.0;
    let num = |i: usize| toks.get(i).and_then(|t| t.parse::<f64>().ok());
    for (i, t) in toks.iter().enumerate() {
        match *t {
            "rg" if i >= 3 => {
                if let (Some(r), Some(g), Some(b)) = (num(i - 3), num(i - 2), num(i - 1)) {
                    col = [r, g, b];
                }
            }
            "g" if i >= 1 => {
                if let Some(g) = num(i - 1) {
                    col = [g; 3];
                }
            }
            "Tf" if i >= 1 => {
                if let Some(s) = num(i - 1).filter(|s| *s > 0.0) {
                    size = s;
                }
            }
            _ => {}
        }
    }
    (col.map(|x| x.clamp(0.0, 1.0)), size.min(400.0))
}

fn ext_gstate(opacity: f64, multiply: bool) -> Option<Dict> {
    if opacity >= 1.0 && !multiply {
        return None;
    }
    let mut gs = Dict::new();
    gs.set(b"Type".to_vec(), Object::name("ExtGState"));
    if multiply {
        gs.set(b"BM".to_vec(), Object::name("Multiply"));
    }
    gs.set(b"CA".to_vec(), Object::Real(opacity));
    gs.set(b"ca".to_vec(), Object::Real(opacity));
    let mut res = Dict::new();
    res.set(b"GS0".to_vec(), Object::Dict(gs));
    Some(res)
}

fn form(bbox: [f64; 4], content: &[u8], resources: Dict) -> Stream {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"FormType".to_vec(), Object::Int(1));
    d.set(b"BBox".to_vec(), Object::Array(bbox.iter().map(|x| Object::Real(*x)).collect()));
    d.set(b"Resources".to_vec(), Object::Dict(resources));
    Stream::flate(d, content)
}

/// The page `/Rotate` an image signature's picture is turned back by (`/PCPictureRotate`, written
/// when the signature is added to a turned page): 0, 90, 180 or 270.
pub(crate) fn picture_turn(d: &Dict) -> i64 {
    d.get(b"PCPictureRotate").and_then(Object::as_int).unwrap_or(0).rem_euclid(360) / 90 * 90
}

/// Draw the normal appearance of an annotation, or `None` if this subtype/variant isn't supported.
pub fn build(d: &Dict) -> Option<Stream> {
    // Imported measurement appearances may include leaders, captions and formatting that
    // this builder cannot reproduce. Preserve them rather than silently losing detail.
    if d.contains(b"Measure") && !d.contains(b"PCMeasureValue") {
        return None;
    }
    let subtype = d.name(b"Subtype")?.to_vec();
    let rect = nums(d, b"Rect").filter(|r| r.len() == 4)?;
    let rect = [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])];
    // Cloudy borders (§12.5.4) are drawn for polygons only.
    let cloudy = d.get(b"BE").and_then(|b| b.as_dict()).is_some_and(|b| b.name(b"S") == Some(b"C"));
    if cloudy && subtype != b"Polygon" {
        return None;
    }
    let opacity = d.get(b"CA").and_then(|o| o.as_f64()).unwrap_or(1.0).clamp(0.0, 1.0);
    let stroke = color(d, b"C")?;
    let w = border_width(d);
    let mut res = Dict::new();
    let mut c = String::new();
    let markup = matches!(subtype.as_slice(), b"Highlight" | b"Underline" | b"StrikeOut" | b"Squiggly");
    if let Some(gs) = ext_gstate(opacity, subtype == b"Highlight") {
        res.set(b"ExtGState".to_vec(), Object::Dict(gs));
        c.push_str("/GS0 gs\n");
    }
    match subtype.as_slice() {
        b"Text" => {
            let col = stroke.unwrap_or([1.0, 0.82, 0.0]);
            let icon = d.name(b"Name").map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_else(|| "Note".into());
            c.push_str(&note_icon(&icon, col));
            return Some(form([0.0, 0.0, NOTE_SIZE, NOTE_SIZE], c.as_bytes(), res));
        }
        b"FileAttachment" => {
            let col = stroke.unwrap_or([0.0, 0.47, 0.84]);
            let icon = d.name(b"Name").map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_else(|| "PushPin".into());
            c.push_str(&attach_icon(&icon, col));
            return Some(form([0.0, 0.0, NOTE_SIZE, NOTE_SIZE], c.as_bytes(), res));
        }
        _ if markup => {
            let q = nums(d, b"QuadPoints").filter(|q| !q.is_empty() && q.len() % 8 == 0)?;
            let col = stroke?;
            for quad in q.as_chunks::<8>().0.iter() {
                let p = |i: usize| (quad[2 * i], quad[2 * i + 1]);
                let (p1, p2, p3, p4) = (p(0), p(1), p(2), p(3));
                let h = (p1.0 - p3.0).hypot(p1.1 - p3.1);
                // A point a fraction `t` of the way from the bottom edge to the top edge.
                let at = |bottom: (f64, f64), top: (f64, f64), t: f64| (bottom.0 + (top.0 - bottom.0) * t, bottom.1 + (top.1 - bottom.1) * t);
                match subtype.as_slice() {
                    b"Highlight" => {
                        c.push_str(&rg(col));
                        c.push_str(&format!(
                            "{} {} m {} {} l {} {} l {} {} l h f\n",
                            n(p1.0),
                            n(p1.1),
                            n(p2.0),
                            n(p2.1),
                            n(p4.0),
                            n(p4.1),
                            n(p3.0),
                            n(p3.1)
                        ));
                    }
                    b"Underline" | b"StrikeOut" => {
                        let lw = (h * 0.07).clamp(0.5, 3.0);
                        let t = if subtype == b"Underline" { 0.08 } else { 0.42 };
                        let (a, b) = (at(p3, p1, t), at(p4, p2, t));
                        c.push_str(&format!("{}{} w\n{} {} m {} {} l S\n", rg_stroke(col), n(lw), n(a.0), n(a.1), n(b.0), n(b.1)));
                    }
                    _ => {
                        // Squiggly: a zigzag along the bottom of the quad.
                        let lw = (h * 0.05).clamp(0.5, 2.0);
                        let amp = (h * 0.06).max(0.75);
                        let len = (p4.0 - p3.0).hypot(p4.1 - p3.1);
                        let step = (h / 4.0).max(1.5);
                        let steps = ((len / step).ceil() as usize).clamp(1, 10_000);
                        c.push_str(&format!("{}{} w 1 j\n", rg_stroke(col), n(lw)));
                        for i in 0..=steps {
                            let t = i as f64 / steps as f64;
                            let base = (p3.0 + (p4.0 - p3.0) * t, p3.1 + (p4.1 - p3.1) * t);
                            let up = if i % 2 == 0 { amp * 2.0 } else { 0.0 };
                            let (ux, uy) = if h > 0.0 { ((p1.0 - p3.0) / h, (p1.1 - p3.1) / h) } else { (0.0, 1.0) };
                            let (x, y) = (base.0 + ux * up, base.1 + uy * up);
                            c.push_str(&format!("{} {} {}\n", n(x), n(y), if i == 0 { "m" } else { "l" }));
                        }
                        c.push_str("S\n");
                    }
                }
            }
        }
        b"Redact" => {
            // While marked: the outline of each area (the fill comes when applied).
            let q = nums(d, b"QuadPoints").filter(|q| !q.is_empty() && q.len() % 8 == 0)?;
            let col = stroke.unwrap_or([0.89, 0.13, 0.13]);
            c.push_str(&format!("{}1 w\n", rg_stroke(col)));
            for quad in q.as_chunks::<8>().0.iter() {
                c.push_str(&format!(
                    "{} {} m {} {} l {} {} l {} {} l h S\n",
                    n(quad[0]),
                    n(quad[1]),
                    n(quad[2]),
                    n(quad[3]),
                    n(quad[6]),
                    n(quad[7]),
                    n(quad[4]),
                    n(quad[5])
                ));
            }
        }
        b"Square" | b"Circle" => {
            let fill = interior_color(d, None);
            if stroke.is_none() && fill.is_none() {
                return Some(form(rect, c.as_bytes(), res));
            }
            let inset = w / 2.0;
            let [x0, y0, x1, y1] = [rect[0] + inset, rect[1] + inset, rect[2] - inset, rect[3] - inset];
            if let Some(f) = fill {
                c.push_str(&rg(f));
            }
            if let Some(s) = stroke {
                c.push_str(&format!("{}{} w\n{}", rg_stroke(s), n(w), dash(d)));
            }
            if subtype == b"Square" {
                c.push_str(&format!("{} {} {} {} re\n", n(x0), n(y0), n(x1 - x0), n(y1 - y0)));
            } else {
                c.push_str(&ellipse(x0, y0, x1, y1));
            }
            c.push_str(match (fill.is_some(), stroke.is_some() && w > 0.0) {
                (true, true) => "B\n",
                (true, false) => "f\n",
                (false, true) => "S\n",
                (false, false) => "n\n",
            });
        }
        b"Line" => {
            let l = nums(d, b"L").filter(|l| l.len() == 4 && l.iter().all(|x| x.is_finite()))?;
            let col = stroke?;
            let ends: Vec<Vec<u8>> = match d.get(b"LE") {
                None => vec![b"None".to_vec(), b"None".to_vec()],
                Some(o) => o.as_array()?.iter().map(|e| e.as_name().map(<[u8]>::to_vec)).collect::<Option<_>>()?,
            };
            if ends.len() != 2 || ends.iter().any(|e| !known_ending(e)) {
                return None;
            }
            let fill = interior_color(d, Some(col)).unwrap_or(col);
            c.push_str(&format!("{}{}{} w 1 J 1 j\n{}", rg_stroke(col), rg(fill), n(w), dash(d)));
            c.push_str(&format!("{} {} m {} {} l S\n[] 0 d\n", n(l[0]), n(l[1]), n(l[2]), n(l[3])));
            for (end, (tip, from)) in ends.iter().zip([((l[0], l[1]), (l[2], l[3])), ((l[2], l[3]), (l[0], l[1]))]) {
                if !line_end(&mut c, end, tip, from, w) {
                    return None;
                }
            }
        }
        b"Polygon" | b"PolyLine" => {
            let v = nums(d, b"Vertices").filter(|v| v.len() >= 4 && v.len() % 2 == 0 && v.iter().all(|x| x.is_finite()))?;
            let pts: Vec<(f64, f64)> = v.as_chunks::<2>().0.iter().map(|p| (p[0], p[1])).collect();
            let col = stroke;
            let closed = subtype == b"Polygon";
            let fill = if closed { interior_color(d, None) } else { None };
            let ends = if subtype == b"PolyLine" {
                match d.get(b"LE") {
                    None => None,
                    Some(o) => {
                        let names: Vec<Vec<u8>> =
                            o.as_array().filter(|a| a.len() == 2)?.iter().map(|e| e.as_name().map(<[u8]>::to_vec)).collect::<Option<_>>()?;
                        if names.iter().any(|e| !known_ending(e)) {
                            return None;
                        }
                        Some(names)
                    }
                }
            } else {
                None
            };
            if let Some(f) = fill {
                c.push_str(&rg(f));
            }
            if let Some(s) = col {
                c.push_str(&format!("{}{} w 1 J 1 j\n{}", rg_stroke(s), n(w), dash(d)));
            }
            if cloudy {
                let intensity = d.get(b"BE").and_then(|b| b.as_dict()).and_then(|b| b.get(b"I")).and_then(|i| i.as_f64()).unwrap_or(1.0);
                c.push_str(&cloud_path(&pts, cloud_radius(w) * intensity.clamp(0.5, 2.0)));
            } else {
                for (i, p) in pts.iter().enumerate() {
                    c.push_str(&format!("{} {} {}\n", n(p.0), n(p.1), if i == 0 { "m" } else { "l" }));
                }
                if closed {
                    c.push_str("h\n");
                }
            }
            c.push_str(match (fill.is_some(), col.is_some() && w > 0.0) {
                (true, true) => "B\n",
                (true, false) => "f\n",
                (false, true) => "S\n",
                (false, false) => "n\n",
            });
            if let Some(ends) = ends {
                let col = col?;
                let (Some(start), Some(end)) = (ends.first(), ends.get(1)) else { return None };
                let (Some(&tip0), Some(&from0)) = (pts.first(), pts.get(1)) else { return None };
                let (Some(&tip1), Some(&from1)) = (pts.last(), pts.get(pts.len().saturating_sub(2))) else { return None };
                let fill = interior_color(d, Some(col)).unwrap_or(col);
                c.push_str(&format!("[] 0 d\n{}", rg(fill)));
                if !line_end(&mut c, start, tip0, from0, w) || !line_end(&mut c, end, tip1, from1, w) {
                    return None;
                }
            }
        }
        b"Caret" => {
            // A filled caret: two curved flanks meeting at the top centre.
            let col = stroke.unwrap_or([0.0, 0.47, 0.84]);
            let [x0, y0, x1, y1] = rect;
            let (cx, h) = ((x0 + x1) / 2.0, y1 - y0);
            c.push_str(&format!(
                "{}{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c h f\n",
                rg(col),
                n(x0),
                n(y0),
                n(cx - (x1 - x0) * 0.1),
                n(y0 + h * 0.2),
                n(cx),
                n(y1 - h * 0.25),
                n(cx),
                n(y1),
                n(cx),
                n(y1 - h * 0.25),
                n(cx + (x1 - x0) * 0.1),
                n(y0 + h * 0.2),
                n(x1),
                n(y0)
            ));
        }
        b"Ink" => {
            let col = stroke?;
            let list = d.get(b"InkList")?.as_array()?;
            c.push_str(&format!("{}{} w 1 J 1 j\n", rg_stroke(col), n(w)));
            for s in list {
                let pts: Vec<f64> = s.as_array()?.iter().map(|o| o.as_f64()).collect::<Option<_>>()?;
                let pts: Vec<(f64, f64)> = pts.as_chunks::<2>().0.iter().map(|p| (p[0], p[1])).collect();
                let Some(first) = pts.first() else { continue };
                c.push_str(&format!("{} {} m\n", n(first.0), n(first.1)));
                match pts.as_slice() {
                    [_] => c.push_str(&format!("{} {} l\n", n(first.0 + 0.01), n(first.1))),
                    [_, end] => c.push_str(&format!("{} {} l\n", n(end.0), n(end.1))),
                    _ => c.push_str(&smooth_curve(&pts)),
                }
                c.push_str("S\n");
            }
        }
        b"Stamp" => {
            // A typed signature: filled outlines normalised to the rectangle.
            if let Some(outline) = d.get(b"PCOutline").and_then(|o| o.as_array()) {
                let [x0, y0, x1, y1] = rect;
                let (w, h) = (x1 - x0, y1 - y0);
                c.push_str(&rg(stroke.unwrap_or([0.0; 3])));
                for contour in outline {
                    let v: Vec<f64> = contour.as_array().map(|a| a.iter().filter_map(Object::as_f64).collect()).unwrap_or_default();
                    for (i, p) in v.as_chunks::<2>().0.iter().enumerate() {
                        c.push_str(&format!("{} {} {}\n", n(x0 + p[0] * w), n(y0 + p[1] * h), if i == 0 { "m" } else { "l" }));
                    }
                    if v.len() >= 6 {
                        c.push_str("h\n");
                    }
                }
                c.push_str("f*\n");
                return Some(form(rect, c.as_bytes(), res));
            }
            // A custom stamp: its picture (an image, or a form mapped to /PCPictureSize) fills
            // the rectangle. An image signature added to a turned page (`/PCPictureRotate`) gets
            // the standard counter-rotation as its form `/Matrix`: viewers fit the turned
            // bounding box to `/Rect` (ISO 32000-2 §12.5.5), so it reads upright as displayed.
            if let Some(pic) = d.get(b"PCPicture").and_then(Object::as_ref) {
                let [x0, y0, x1, y1] = rect;
                let (w, h) = (x1 - x0, y1 - y0);
                let image = matches!(d.get(b"PCPictureImage"), Some(Object::Bool(true)));
                let place = if image {
                    format!("{} 0 0 {} {} {} cm", n(w), n(h), n(x0), n(y0))
                } else {
                    let size = nums(d, b"PCPictureSize").filter(|s| s.len() == 2 && s[0] > 0.0 && s[1] > 0.0)?;
                    format!("{} 0 0 {} {} {} cm", n(w / size[0]), n(h / size[1]), n(x0), n(y0))
                };
                c.push_str(&format!("q {place} /Pic Do Q\n"));
                let mut xo = Dict::new();
                xo.set(b"Pic".to_vec(), Object::Ref(pic));
                res.set(b"XObject".to_vec(), Object::Dict(xo));
                let mut stream = form(rect, c.as_bytes(), res);
                let turn = picture_turn(d);
                if image && turn != 0 {
                    // The page's view matrix without its offset: display axes in user space.
                    let m = pdfcraft_model::view_matrix_for(turn, [0.0; 4]);
                    let matrix = [m[0], m[1], m[2], m[3], 0.0, 0.0];
                    stream.dict.set(b"Matrix".to_vec(), Object::Array(matrix.iter().map(|x| Object::Real(*x)).collect()));
                }
                return Some(stream);
            }
            // Only PdfCraft's own Fill & Sign marks are drawn here.
            let name = d.name(b"Name")?;
            let col = stroke.unwrap_or([0.0; 3]);
            let [x0, y0, x1, y1] = rect;
            let (w, h) = (x1 - x0, y1 - y0);
            let lw = w.min(h) * 0.12;
            match name {
                b"PCCheck" => c.push_str(&format!(
                    "{}{} w 1 J 1 j\n{} {} m {} {} l {} {} l S\n",
                    rg_stroke(col),
                    n(lw),
                    n(x0 + w * 0.15),
                    n(y0 + h * 0.5),
                    n(x0 + w * 0.4),
                    n(y0 + h * 0.2),
                    n(x0 + w * 0.88),
                    n(y0 + h * 0.85)
                )),
                b"PCCross" => c.push_str(&format!(
                    "{}{} w 1 J\n{} {} m {} {} l {} {} m {} {} l S\n",
                    rg_stroke(col),
                    n(lw),
                    n(x0 + w * 0.18),
                    n(y0 + h * 0.18),
                    n(x1 - w * 0.18),
                    n(y1 - h * 0.18),
                    n(x0 + w * 0.18),
                    n(y1 - h * 0.18),
                    n(x1 - w * 0.18),
                    n(y0 + h * 0.18)
                )),
                b"PCDot" => {
                    let r = w.min(h) * 0.3;
                    c.push_str(&rg(col));
                    c.push_str(&ellipse(x0 + w / 2.0 - r, y0 + h / 2.0 - r, x0 + w / 2.0 + r, y0 + h / 2.0 + r));
                    c.push_str("f\n");
                }
                b"PCLine" => c.push_str(&format!(
                    "{}{} w 1 J\n{} {} m {} {} l S\n",
                    rg_stroke(col),
                    n(h.clamp(0.5, 2.0)),
                    n(x0),
                    n(y0 + h / 2.0),
                    n(x1),
                    n(y0 + h / 2.0)
                )),
                other => {
                    // Only stamps PdfCraft made: others keep their own artwork.
                    if !matches!(d.get(b"PCStamp"), Some(Object::Bool(true))) {
                        return None;
                    }
                    let kind = crate::StampKind::from_name(other)?;
                    let by = d.get(b"PCByLine").and_then(|o| o.as_string()).map(PdfString::to_text);
                    return Some(stamp(kind, rect, stroke.unwrap_or(kind.color()), by.as_deref(), opacity, res));
                }
            }
        }
        b"FreeText" => {
            let (text_color, size) = parse_da(d);
            let bg = stroke;
            let bw = if d.contains(b"BS") || d.contains(b"Border") { border_width(d) } else { 0.0 };
            // A callout: the leader line (arrowhead at its first point), and the text box inside
            // `/Rect` by `/RD`.
            let full = rect;
            let mut rect = rect;
            if d.contains(b"CL") {
                let cl = nums(d, b"CL").filter(|l| l.len() == 4 || l.len() == 6)?;
                let rd = nums(d, b"RD").filter(|r| r.len() == 4 && r.iter().all(|x| *x >= 0.0))?;
                rect = [full[0] + rd[0], full[1] + rd[1], full[2] - rd[2], full[3] - rd[3]];
                if rect[2] - rect[0] < 1.0 || rect[3] - rect[1] < 1.0 {
                    return None;
                }
                let end = match d.get(b"LE") {
                    None => b"None".to_vec(),
                    Some(o) => o.as_name()?.to_vec(),
                };
                if !known_ending(&end) {
                    return None;
                }
                let lw = bw.max(0.5);
                let pts: Vec<(f64, f64)> = cl.as_chunks::<2>().0.iter().map(|p| (p[0], p[1])).collect();
                if pts.len() < 2 || pts.iter().any(|p| !p.0.is_finite() || !p.1.is_finite()) {
                    return None;
                }
                c.push_str(&format!("{}{}{} w 1 J 1 j\n", rg_stroke(text_color), rg(bg.unwrap_or([1.0; 3])), n(lw)));
                for (i, p) in pts.iter().enumerate() {
                    c.push_str(&format!("{} {} {}\n", n(p.0), n(p.1), if i == 0 { "m" } else { "l" }));
                }
                c.push_str("S\n");
                if !line_end(&mut c, &end, pts[0], pts[1], lw) {
                    return None;
                }
            }
            if let Some(bg) = bg {
                c.push_str(&format!("{}{} {} {} {} re f\n", rg(bg), n(rect[0]), n(rect[1]), n(rect[2] - rect[0]), n(rect[3] - rect[1])));
            }
            if bw > 0.0 {
                let h = bw / 2.0;
                c.push_str(&format!(
                    "{}{} w\n{}{} {} {} {} re S\n[] 0 d\n",
                    rg_stroke(text_color),
                    n(bw),
                    dash(d),
                    n(rect[0] + h),
                    n(rect[1] + h),
                    n(rect[2] - rect[0] - bw),
                    n(rect[3] - rect[1] - bw)
                ));
            }
            let text = d.get(b"Contents").and_then(|o| o.as_string()).map(PdfString::to_text).unwrap_or_default();
            let pad = 2.0 + bw;
            let width = (rect[2] - rect[0] - 2.0 * pad).max(1.0);
            let q = d.int(b"Q").unwrap_or(0);
            c.push_str(&format!(
                "{} {} {} {} re W n\nBT\n/Helv {} Tf\n{}",
                n(rect[0]),
                n(rect[1]),
                n(rect[2] - rect[0]),
                n(rect[3] - rect[1]),
                n(size),
                rg(text_color)
            ));
            let mut out = c.into_bytes();
            let mut y = rect[3] - pad - size * 0.9;
            for line in wrap(&text, size, width) {
                if y < rect[1] - size {
                    break;
                }
                let lw = text_width(&line, size);
                let x = match q {
                    1 => rect[0] + pad + (width - lw) / 2.0,
                    2 => rect[2] - pad - lw,
                    _ => rect[0] + pad,
                };
                out.extend(format!("1 0 0 1 {} {} Tm ", n(x), n(y)).bytes());
                out.extend(literal(&win_ansi(&line)));
                out.extend_from_slice(b" Tj\n");
                y -= size * 1.2;
            }
            out.extend_from_slice(b"ET\n");
            let mut font = Dict::new();
            font.set(b"Type".to_vec(), Object::name("Font"));
            font.set(b"Subtype".to_vec(), Object::name("Type1"));
            font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
            font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
            let mut fonts = Dict::new();
            fonts.set(b"Helv".to_vec(), Object::Dict(font));
            res.set(b"Font".to_vec(), Object::Dict(fonts));
            return Some(form(full, &out, res));
        }
        _ => return None,
    }
    // PdfCraft measurement captions are kept separate from the comment's free-form text.
    // A restyle regenerates the path and its value together.
    let mut out = c.into_bytes();
    if let Some(value) = d.get(b"PCMeasureValue").and_then(Object::as_string) {
        let value = value.to_text();
        if value.chars().count() <= 256 && matches!(subtype.as_slice(), b"Line" | b"PolyLine" | b"Polygon") {
            let size = 10.0;
            let x = (rect[0] + rect[2] - text_width(&value, size)) * 0.5;
            let y = rect[3] - 12.0;
            let col = stroke.unwrap_or([0.0, 0.47, 0.84]);
            out.extend(format!("{}BT /Helv {} Tf {} {} Td ", rg(col), n(size), n(x), n(y)).bytes());
            // WinAnsi bytes (e.g. 0xB2 for "²") go into the stream as-is, not through UTF-8.
            out.extend(literal(&win_ansi(&value)));
            out.extend_from_slice(b" Tj ET\n");
            let mut font = Dict::new();
            font.set(b"Type".to_vec(), Object::name("Font"));
            font.set(b"Subtype".to_vec(), Object::name("Type1"));
            font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
            font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
            let mut fonts = Dict::new();
            fonts.set(b"Helv".to_vec(), Object::Dict(font));
            res.set(b"Font".to_vec(), Object::Dict(fonts));
        }
    }
    Some(form(rect, &out, res))
}

/// A rubber stamp: a rounded frame (a pointed tag for sign-here stamps) with the label in bold
/// capitals, and the dynamic stamps' "By … at …" line.
fn stamp(kind: crate::StampKind, rect: [f64; 4], col: Rgb, by: Option<&str>, opacity: f64, mut res: Dict) -> Stream {
    let [x0, y0, x1, y1] = rect;
    let h = y1 - y0;
    let mut c = String::new();
    if opacity < 1.0 {
        c.push_str("/GS0 gs\n");
    }
    let lw = (h * 0.07).clamp(1.0, 3.0);
    let inset = lw / 2.0 + 0.5;
    if kind.group() == crate::StampGroup::SignHere {
        // A tag pointing left, filled, with white text.
        let tip = h * 0.45;
        c.push_str(&format!(
            "{}{}{} w 1 j\n{} {} m {} {} l {} {} l {} {} l {} {} l h B\n",
            rg(col),
            rg_stroke(col),
            n(lw),
            n(x0 + inset),
            n(y0 + h / 2.0),
            n(x0 + tip),
            n(y1 - inset),
            n(x1 - inset),
            n(y1 - inset),
            n(x1 - inset),
            n(y0 + inset),
            n(x0 + tip),
            n(y0 + inset)
        ));
    } else {
        // A rounded frame with a light tint.
        let r = (h * 0.2).min(8.0);
        let k = 0.552_284_75 * r;
        let (a0, b0, a1, b1) = (x0 + inset, y0 + inset, x1 - inset, y1 - inset);
        let tint = col.map(|v| 1.0 - (1.0 - v) * 0.12);
        c.push_str(&format!(
            "{}{}{} w\n{} {} m {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c h B\n",
            rg(tint),
            rg_stroke(col),
            n(lw),
            n(a0 + r),
            n(b0),
            n(a1 - r),
            n(b0),
            n(a1 - r + k),
            n(b0),
            n(a1),
            n(b0 + r - k),
            n(a1),
            n(b0 + r),
            n(a1),
            n(b1 - r),
            n(a1),
            n(b1 - r + k),
            n(a1 - r + k),
            n(b1),
            n(a1 - r),
            n(b1),
            n(a0 + r),
            n(b1),
            n(a0 + r - k),
            n(b1),
            n(a0),
            n(b1 - r + k),
            n(a0),
            n(b1 - r),
            n(a0),
            n(b0 + r),
            n(a0),
            n(b0 + r - k),
            n(a0 + r - k),
            n(b0),
            n(a0 + r),
            n(b0)
        ));
    }
    let label = kind.label();
    let (title_h, by_h) = if by.is_some() { (h * 0.5, h * 0.22) } else { (h * 0.52, 0.0) };
    let text_area = if kind.group() == crate::StampGroup::SignHere { (x0 + h * 0.45, x1 - lw * 2.0) } else { (x0 + lw * 2.0, x1 - lw * 2.0) };
    let mut size = title_h;
    let tw = text_width(label, size) * 1.12;
    if tw > text_area.1 - text_area.0 - 4.0 {
        size *= (text_area.1 - text_area.0 - 4.0) / tw;
    }
    let tw = text_width(label, size) * 1.12;
    let text_col = if kind.group() == crate::StampGroup::SignHere { [1.0, 1.0, 1.0] } else { col };
    let ty = if by.is_some() { y0 + h * 0.48 } else { y0 + (h - size * 0.72) / 2.0 };
    let mut out = c.into_bytes();
    out.extend(
        format!("BT\n{}/HelvB {} Tf\n1 0 0 1 {} {} Tm ", rg(text_col), n(size), n(text_area.0 + (text_area.1 - text_area.0 - tw) / 2.0), n(ty))
            .bytes(),
    );
    out.extend(literal(&win_ansi(label)));
    out.extend_from_slice(b" Tj\n");
    if let Some(b) = by {
        let mut s = by_h;
        let bw = text_width(b, s);
        if bw > text_area.1 - text_area.0 - 4.0 {
            s *= (text_area.1 - text_area.0 - 4.0) / bw;
        }
        let bw = text_width(b, s);
        out.extend(
            format!("/Helv {} Tf\n1 0 0 1 {} {} Tm ", n(s), n(text_area.0 + (text_area.1 - text_area.0 - bw) / 2.0), n(y0 + h * 0.18)).bytes(),
        );
        out.extend(literal(&win_ansi(b)));
        out.extend_from_slice(b" Tj\n");
    }
    out.extend_from_slice(b"ET\n");
    let font = |base: &str| {
        let mut f = Dict::new();
        f.set(b"Type".to_vec(), Object::name("Font"));
        f.set(b"Subtype".to_vec(), Object::name("Type1"));
        f.set(b"BaseFont".to_vec(), Object::name(base));
        f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        Object::Dict(f)
    };
    let mut fonts = Dict::new();
    fonts.set(b"HelvB".to_vec(), font("Helvetica-Bold"));
    fonts.set(b"Helv".to_vec(), font("Helvetica"));
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    form(rect, &out, res)
}

/// An ellipse inscribed in a rectangle, as four Bézier arcs.
fn ellipse(x0: f64, y0: f64, x1: f64, y1: f64) -> String {
    let k = 0.552_284_75;
    let (cx, cy, rx, ry) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, (x1 - x0) / 2.0, (y1 - y0) / 2.0);
    let (ox, oy) = (rx * k, ry * k);
    let mut s = format!("{} {} m\n", n(cx + rx), n(cy));
    for [a, b, c, d, e, f] in [
        [cx + rx, cy + oy, cx + ox, cy + ry, cx, cy + ry],
        [cx - ox, cy + ry, cx - rx, cy + oy, cx - rx, cy],
        [cx - rx, cy - oy, cx - ox, cy - ry, cx, cy - ry],
        [cx + ox, cy - ry, cx + rx, cy - oy, cx + rx, cy],
    ] {
        s.push_str(&format!("{} {} {} {} {} {} c\n", n(a), n(b), n(c), n(d), n(e), n(f)));
    }
    s.push_str("h\n");
    s
}

/// PdfCraft's note icons, drawn in a 20 × 20 box: a speech bubble for `/Comment`, a page
/// with a folded corner for everything else, both filled with the note colour.
/// File attachment icons in a 20 × 20 box: PdfCraft's own drawings.
fn attach_icon(name: &str, col: Rgb) -> String {
    let mut s = format!("{}{}1.2 w 1 j 1 J\n", rg(col), rg_stroke(col));
    match name {
        "Paperclip" => s.push_str("8 4 m 8 15 l 8 18 13 18 13 15 c 13 6 l 13 3 10 3 10 6 c 10 14 l S\n"),
        "Graph" => s.push_str("3 3 m 3 17 l 3 3 m 17 3 l S\n5 3 3 6 re 9 3 3 10 re 13 3 3 13 re f\n"),
        "Tag" => s.push_str("3 10 m 8 15 l 17 15 l 17 5 l 8 5 l h S\n7.3 10 m 7.3 10.7 6.7 11.3 6 11.3 c 5.3 11.3 4.7 10.7 4.7 10 c 4.7 9.3 5.3 8.7 6 8.7 c 6.7 8.7 7.3 9.3 7.3 10 c f\n"),
        _ => {
            // Push pin: a round head on a short needle.
            s.push_str("10 2 m 10 9 l S\n6 9 m 14 9 l 14 11 l 12 12 l 12 16 l 13.5 17 l 13.5 18.5 l 6.5 18.5 l 6.5 17 l 8 16 l 8 12 l 6 11 l h f\n");
        }
    }
    s
}

fn note_icon(name: &str, col: Rgb) -> String {
    let mut s = format!("{}0.25 0.25 0.25 RG 0.8 w 1 j 1 J\n", rg(col));
    if name == "Comment" {
        s.push_str("3 18.5 m 17 18.5 l 18.5 18.5 18.5 17 18.5 17 c 18.5 8 l 18.5 6.5 17 6.5 17 6.5 c 9.5 6.5 l 5 2 l 5.5 6.5 l 3 6.5 l 1.5 6.5 1.5 8 1.5 8 c 1.5 17 l 1.5 18.5 3 18.5 3 18.5 c h B\n");
        s.push_str("4.5 15 m 15.5 15 l 4.5 12.5 m 15.5 12.5 l 4.5 10 m 11.5 10 l S\n");
    } else {
        s.push_str("3 19 m 13 19 l 17 15 l 17 1 l 3 1 l h B\n13 19 m 13 15 l 17 15 l S\n");
        s.push_str("5.5 12 m 14.5 12 l 5.5 9 m 14.5 9 l 5.5 6 m 11.5 6 l S\n");
    }
    s
}
