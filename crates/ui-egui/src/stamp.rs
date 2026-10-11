//! Adobe-style validation stamp for verified signatures (viewer-only overlay).
//!
//! Reproduces the reference e-Aadhaar verifier's stamp (STAMP_CONFIG v4,
//! `stamp-config.default.json` + `app/static/stamp-render.js`): a 50x30pt form with
//! a green tick ribbon, the "Signature valid" title, and signer body lines carrying
//! the real signing date. Painted opaquely over the widget's baked appearance; the
//! PDF bytes are untouched. Shown only for signatures whose validation actually
//! justifies the valid state (see [`valid_stamp_for`]).

use egui::{Color32, Pos2, Rect, Shape, Stroke, pos2};

use pdfcraft_engine::{SignatureInfo, SignatureStatus};

/// Stamp form size in the reference layout (points).
pub const FORM_W: f32 = 50.0;
pub const FORM_H: f32 = 30.0;

/// Tick ribbon fill (reference `tick.fillColor`, Acrobat green).
pub const TICK_GREEN: Color32 = Color32::from_rgb(0x00, 0xA6, 0x51);
/// Tick shadow and outline (reference `tick.shadowColor` / `tick.borderColor`).
pub const TICK_INK: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);
/// Stamp text (reference n4/n2 black).
pub const STAMP_INK: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);

/// Title text (reference `n4.textValid`): note the lowercase "valid". The
/// "Signature Not Verified" wording is the reference *invalid* text and must never
/// appear on a valid stamp.
pub const TITLE_VALID: &str = "Signature valid";

/// Title metrics (reference `n4`): x, baseline measured down from the form top, size.
pub const TITLE_X: f32 = 6.0;
pub const TITLE_BASELINE: f32 = 6.8; // FORM_H - 23.2
pub const TITLE_SIZE: f32 = 5.15;

/// Body metrics (reference `n2`): x, top-down baselines, size.
pub const BODY_X: f32 = 7.1;
pub const BODY_SIZE: f32 = 2.4;
pub const BODY_BASELINES: [f32; 5] = [12.1, 14.2, 16.3, 18.4, 20.5];

/// Tick ribbon polygon in form points, y down from the form top, in draw order.
/// Derived from the reference `tick.points100` (polygon order [0,1,6,2,4,5,3])
/// through its mapping chain (`mapScale` 7, `mapOx`/`mapOy` 200/100, the icon
/// stream `0.1 ... cm`, the n1 placement at (5.4, 2.8) with scale 0.27 and flipY);
/// see `tick_point_derivation`.
pub const TICK_POLYGON: [[f32; 2]; 7] =
    [[17.895, 11.949], [16.303, 13.957], [20.809, 18.228], [21.584, 18.833], [30.353, 6.964], [27.802, 5.338], [21.319, 15.601]];
/// Tick shadow offset and outline width in form points (reference `shadowDx/Dy` 16/21
/// and `borderWidth` 8.5 through the same mapping).
pub const TICK_SHADOW: [f32; 2] = [0.432, 0.567];
pub const TICK_OUTLINE: f32 = 0.2295;

/// A validated signature's stamp content.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidStamp {
    pub title: String,
    /// Signer identity + details lines (at most [`BODY_BASELINES`] entries).
    pub lines: Vec<String>,
}

/// Build the stamp for a signature, or `None` unless its validation actually
/// justifies the valid state. Never invents validity: anything but a verified
/// `Valid` verdict gets no stamp.
pub fn valid_stamp_for(sig: &SignatureInfo) -> Option<ValidStamp> {
    if !sig.signed || sig.status != SignatureStatus::Valid {
        return None;
    }
    let signer = sig.signer.clone().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "Unknown signer".to_string());
    let mut lines = vec![format!("Digitally signed by {signer}")];
    let second = sig
        .certificate
        .as_ref()
        .and_then(|c| c.issuer.organization().map(str::to_string))
        .or_else(|| sig.certificate.as_ref().and_then(|c| c.issuer.common_name().map(str::to_string)))
        .or_else(|| sig.reason.clone())
        .or_else(|| sig.location.clone());
    if let Some(second) = second.filter(|s| !s.trim().is_empty()) {
        lines.push(second);
    }
    if let Some(t) = sig.signing_time {
        lines.push(format!("Date: {:04}.{:02}.{:02} {:02}:{:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second));
    }
    lines.truncate(BODY_BASELINES.len());
    Some(ValidStamp { title: TITLE_VALID.to_string(), lines })
}

/// Triangulate a simple polygon (ear clipping) into index triples. Empty when
/// degenerate (fewer than 3 points or zero area). The reference tick ribbon is
/// concave, which egui's polygon fill does not support — hence explicit triangles.
pub(crate) fn triangulate_polygon(pts: &[[f64; 2]]) -> Vec<[usize; 3]> {
    const EPS: f64 = 1e-9;
    fn cross(o: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    }
    let n = pts.len();
    let mut area2 = 0.0;
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n.max(1)]);
        area2 += a[0] * b[1] - a[1] * b[0];
    }
    if n < 3 || area2.abs() < EPS {
        return Vec::new();
    }
    // Work in counter-clockwise order.
    let mut idx: Vec<usize> = (0..n).collect();
    if area2 < 0.0 {
        idx.reverse();
    }
    let mut tris = Vec::with_capacity(n.saturating_sub(2));
    while idx.len() > 3 {
        let m = idx.len();
        let mut cut = None;
        for k in 0..m {
            let (a, b, c) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
            let (pa, pb, pc) = (pts[a], pts[b], pts[c]);
            if cross(pa, pb, pc) <= EPS {
                continue; // reflex in CCW order: not an ear tip
            }
            let mut clear = true;
            for &o in &idx {
                if o == a || o == b || o == c {
                    continue;
                }
                let p = pts[o];
                let (d1, d2, d3) = (cross(pa, pb, p), cross(pb, pc, p), cross(pc, pa, p));
                if d1 > EPS && d2 > EPS && d3 > EPS {
                    clear = false;
                    break;
                }
            }
            if clear {
                cut = Some(k);
                break;
            }
        }
        let Some(k) = cut else { break }; // numerically stuck: keep what we have
        let m = idx.len();
        tris.push([idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]]);
        idx.remove(k);
    }
    if idx.len() == 3 {
        tris.push([idx[0], idx[1], idx[2]]);
    }
    tris
}

/// Screen-space stamp geometry: uniform scale fitting the 50x30 form into `cover`,
/// centered. Pure so the layout is unit-testable.
#[derive(Clone, Debug)]
pub(crate) struct StampLayout {
    pub scale: f32,
    pub origin: Pos2,
}

pub(crate) fn stamp_layout(cover: Rect) -> Option<StampLayout> {
    if cover.width() < 16.0 || cover.height() < 10.0 {
        return None; // degenerate on screen: cover only
    }
    let scale = (cover.width() / FORM_W).min(cover.height() / FORM_H);
    Some(StampLayout {
        scale,
        origin: pos2(cover.left() + (cover.width() - FORM_W * scale) / 2.0, cover.top() + (cover.height() - FORM_H * scale) / 2.0),
    })
}

fn form_point(origin: Pos2, scale: f32, x: f32, y: f32) -> Pos2 {
    pos2(origin.x + x * scale, origin.y + y * scale)
}

/// Paint the valid stamp: opaque cover, tick shadow + ribbon + outline, title, body
/// lines — in the reference draw order (icon behind, text on top). No outer border:
/// the reference stamp has none.
pub(crate) fn paint_valid_stamp(ui: &egui::Ui, painter: &egui::Painter, cover: Rect, stamp: &ValidStamp) {
    painter.rect_filled(cover, egui::CornerRadius::same(2), Color32::WHITE);
    let Some(l) = stamp_layout(cover) else { return };
    let painter = painter.with_clip_rect(cover);
    let tick: Vec<Pos2> = TICK_POLYGON.iter().map(|[x, y]| form_point(l.origin, l.scale, *x, *y)).collect();
    let shadow: Vec<Pos2> = tick.iter().map(|p| pos2(p.x + TICK_SHADOW[0] * l.scale, p.y + TICK_SHADOW[1] * l.scale)).collect();
    let tris = triangulate_polygon(&TICK_POLYGON.map(|[x, y]| [f64::from(x), f64::from(y)]));
    let at = |i: usize| -> Pos2 { tick[i] };
    let sat = |i: usize| -> Pos2 { shadow[i] };
    for [a, b, c] in &tris {
        painter.add(Shape::convex_polygon(vec![sat(*a), sat(*b), sat(*c)], TICK_INK, Stroke::NONE));
    }
    for [a, b, c] in &tris {
        painter.add(Shape::convex_polygon(vec![at(*a), at(*b), at(*c)], TICK_GREEN, Stroke::NONE));
    }
    painter.add(Shape::Path(egui::epaint::PathShape {
        points: tick,
        closed: true,
        fill: Color32::TRANSPARENT,
        stroke: Stroke::new((TICK_OUTLINE * l.scale).max(0.75), TICK_INK).into(),
    }));
    let _ = ui;
    let title = painter.layout_no_wrap(stamp.title.clone(), crate::theme::regular(TITLE_SIZE * l.scale), STAMP_INK);
    painter.galley(form_point(l.origin, l.scale, TITLE_X, TITLE_BASELINE - TITLE_SIZE), title, STAMP_INK);
    for (i, line) in stamp.lines.iter().enumerate() {
        let Some(y) = BODY_BASELINES.get(i) else { break };
        let g = painter.layout_no_wrap(line.clone(), crate::theme::regular(BODY_SIZE * l.scale), STAMP_INK);
        painter.galley(form_point(l.origin, l.scale, BODY_X, y - BODY_SIZE), g, STAMP_INK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The form-space tick polygon re-derived from the reference
    /// `tick.points100` through its documented mapping chain
    /// (`mapScale`/`mapOx`/`mapOy`, the icon stream `0.1 ... cm`, n1 placement at
    /// (5.4, 2.8) scale 0.27 with flipY). Guards the baked constants above.
    #[test]
    fn tick_point_derivation() {
        const PTS_100: [[f64; 2]; 7] = [[24.68, 47.88], [16.26, 58.5], [44.2, 84.3], [42.8, 67.2], [90.6, 21.5], [77.1, 12.9], [40.1, 81.1]];
        const ORDER: [usize; 7] = [0, 1, 6, 2, 4, 5, 3];
        let form = |p: [f64; 2]| -> [f32; 2] {
            let (ix, iy) = (p[0] * 7.0 + 200.0, p[1] * 7.0 + 100.0);
            let (sx, sy) = (0.1 * ix + 9.0, 0.1 * iy);
            [(5.4 + 0.27 * sx) as f32, (0.2 + 0.27 * sy) as f32]
        };
        for (k, p) in ORDER.iter().map(|&i| form(PTS_100[i])).enumerate() {
            assert!((p[0] - TICK_POLYGON[k][0]).abs() < 0.002, "{k}: {p:?}");
            assert!((p[1] - TICK_POLYGON[k][1]).abs() < 0.002, "{k}: {p:?}");
        }
    }

    #[test]
    fn tick_matches_reference_config() {
        assert_eq!(TICK_POLYGON.len(), 7);
        let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for [x, y] in TICK_POLYGON {
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
        }
        // Left-of-title ribbon inside the 50x30 form.
        assert!((x0 - 16.303).abs() < 0.01 && (x1 - 30.353).abs() < 0.01);
        assert!((y0 - 5.338).abs() < 0.01 && (y1 - 18.833).abs() < 0.01);
        assert_eq!((TICK_GREEN, TICK_INK), (Color32::from_rgb(0x00, 0xA6, 0x51), Color32::BLACK));
        assert!((TICK_SHADOW[0] - 0.432).abs() < 0.001 && (TICK_SHADOW[1] - 0.567).abs() < 0.001);
        assert!((TICK_OUTLINE - 0.2295).abs() < 0.0005);
        // Wording: the valid text with lowercase "valid"; the invalid wording must
        // never appear on a valid stamp.
        assert_eq!(TITLE_VALID, "Signature valid");
        assert_eq!((TITLE_X, TITLE_BASELINE, TITLE_SIZE), (6.0, 6.8, 5.15));
        assert_eq!((BODY_X, BODY_SIZE), (7.1, 2.4));
        assert_eq!(BODY_BASELINES, [12.1, 14.2, 16.3, 18.4, 20.5]);
        assert_eq!((FORM_W, FORM_H), (50.0, 30.0));
    }

    #[test]
    fn triangulation_covers_tick_exactly() {
        let pts: Vec<[f64; 2]> = TICK_POLYGON.map(|[x, y]| [f64::from(x), f64::from(y)]).to_vec();
        let tris = triangulate_polygon(&pts);
        assert_eq!(tris.len(), pts.len() - 2, "a 7-gon yields 5 triangles");
        let area = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / 2.0;
        let mut shoelace = 0.0;
        for i in 0..pts.len() {
            shoelace += pts[i][0] * pts[(i + 1) % pts.len()][1] - pts[(i + 1) % pts.len()][0] * pts[i][1];
        }
        let poly = shoelace.abs() / 2.0;
        let sum: f64 = tris.iter().map(|[a, b, c]| area(pts[*a], pts[*b], pts[*c])).sum();
        assert!((poly - sum).abs() < 1e-6, "{poly} vs {sum}");
        for [a, b, c] in &tris {
            assert!(*a < pts.len() && *b < pts.len() && *c < pts.len());
            assert!(area(pts[*a], pts[*b], pts[*c]) > 1e-9, "no degenerate triangle");
            assert!(*a != *b && *b != *c && *a != *c);
        }
    }

    #[test]
    fn triangulation_handles_squares_and_winding() {
        let ccw = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        assert_eq!(triangulate_polygon(&ccw).len(), 2);
        let cw: Vec<[f64; 2]> = ccw.iter().rev().cloned().collect();
        assert_eq!(triangulate_polygon(&cw).len(), 2);
        assert!(triangulate_polygon(&[]).is_empty());
        assert!(triangulate_polygon(&[[0.0, 0.0], [1.0, 1.0]]).is_empty());
        assert!(triangulate_polygon(&[[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]]).is_empty(), "zero area");
    }

    #[test]
    fn stamp_layout_fits_form_into_cover() {
        let cover = Rect::from_min_max(pos2(10.0, 20.0), pos2(210.0, 73.0));
        let l = stamp_layout(cover).expect("a layout for a normal widget");
        assert!((l.scale - (200.0f32 / 50.0).min(53.0 / 30.0)).abs() < 0.001, "{l:?}");
        // 50x30 content centered in the 200x53 cover.
        assert!((l.origin.x - (10.0 + (200.0 - 50.0 * l.scale) / 2.0)).abs() < 0.01);
        assert!((l.origin.y - (20.0 + (53.0 - 30.0 * l.scale) / 2.0)).abs() < 0.01);
        // Title and all body baselines land inside the cover.
        assert!(l.origin.y + TITLE_BASELINE * l.scale < cover.bottom());
        for y in BODY_BASELINES {
            assert!(l.origin.y + y * l.scale < cover.bottom());
        }
        assert!(stamp_layout(Rect::from_min_max(pos2(0.0, 0.0), pos2(5.0, 5.0))).is_none());
    }
}
