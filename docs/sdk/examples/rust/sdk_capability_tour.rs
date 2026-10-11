//! sdk_capability_tour.rs: a 5-page showcase PDF built with the `pdfcraft-sdk` Rust API.
//! =====================================================================================
//!
//! Purpose:
//!     Builds a vector-art-heavy, interactive PDF from the SDK's own domain types
//!     (`Path`/`Segment` with Bezier curves, `Matrix`, `Color` incl. CMYK,
//!     `ExtendedGraphicState` + `BlendMode`, `TextRun`/`Font`, `Annotation`/`Action`,
//!     `Field`, `Bookmark`/`ViewDestination`, `Quad`), then drives the real engine through
//!     `LocalClient` (`render_page`, `split`, `merge`) and prints measured results onto page 5.
//!
//!       Page 1  Cover: layered Bezier "flower" (Screen blend + alpha) and a spirograph.
//!       Page 2  Vector lab: gears via `Matrix`, even-odd star, Bezier wave, Bezier donut chart.
//!       Page 3  Type & colour: font specimens, CMYK/RGB ramps, Multiply and Screen blending.
//!       Page 4  Review: link / highlight / ink / line annotations, AcroForm fields, outline.
//!       Page 5  Engine report: measured render/split/merge timings and SdkError taxonomy.
//!
//! The SDK ships domain *models* and an engine client, but no document writer, so this example
//! includes a small dependency-light PDF serializer (`pdf` module below) that consumes those
//! models. It is a reference implementation, not part of the published crate.
//!
//! Usage (from the repo root):
//!     cargo run -p pdfcraft-sdk --example sdk_capability_tour
//!     cargo run -p pdfcraft-sdk --example sdk_capability_tour -- \
//!         --output sample-docs/outputs/sdk_capability_tour_rs.pdf --render-previews
//!
//! Notes: text is ASCII (built-in Type 1 fonts). Previews are rendered by the PdfCraft engine
//! itself (`LocalClient::render_page`), not an external tool.

// Drawing helpers take (page, text, position, font, size, colour) positionally to keep call sites terse.
#![allow(clippy::too_many_arguments, clippy::type_complexity)]

use std::error::Error;
use std::f64::consts::PI;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use pdfcraft_sdk::{
    Action, Annotation, AnnotationSubtype, BlendMode, Bookmark, ButtonField, Color, DocumentResult, DocumentSource, ExtendedGraphicState, Field,
    FieldType, FitMode, Font, GoToAction, HighlightAnnotation, InkAnnotation, LineAnnotation, LineEnding, LinkAnnotation, LocalClient, Matrix,
    MergeOptions, Path, PdfCraftClient, Point, Quad, Rect, RenderOptions, ResourceLimits, Segment, SplitMode, TextRun, URIAction, ViewDestination,
};

type Res<T> = Result<T, Box<dyn Error>>;

// =================================================================================================
// Reference PDF serializer for SDK domain models
// =================================================================================================
mod pdf {
    use super::*;
    use std::fmt::Write as _;

    /// A paint operation: an SDK `Path` plus how to colour it.
    pub struct Paint {
        pub fill: Option<Color>,
        pub stroke: Option<Color>,
        pub width: f64,
        pub gs: ExtendedGraphicState,
    }

    impl Paint {
        pub fn fill(c: Color) -> Self {
            Self { fill: Some(c), stroke: None, width: 1.0, gs: ExtendedGraphicState::default() }
        }
        pub fn stroke(c: Color, width: f64) -> Self {
            Self { fill: None, stroke: Some(c), width, gs: ExtendedGraphicState::default() }
        }
        pub fn both(fill: Color, stroke: Color, width: f64) -> Self {
            Self { fill: Some(fill), stroke: Some(stroke), width, gs: ExtendedGraphicState::default() }
        }
        pub fn alpha(mut self, a: f64) -> Self {
            self.gs.alpha_fill = a;
            self.gs.alpha_stroke = a;
            self
        }
        pub fn blend(mut self, b: BlendMode) -> Self {
            self.gs.blend_mode = b;
            self
        }
    }

    pub enum Op {
        Path(Path, Paint),
        Text(TextRun),
        Image { id: usize, w: u32, h: u32, rgb: Vec<u8>, rect: Rect },
    }

    #[derive(Default)]
    pub struct PageData {
        pub media: Option<Rect>,
        pub ops: Vec<Op>,
        pub annots: Vec<Annotation>,
        pub fields: Vec<Field>,
    }

    #[derive(Default)]
    pub struct Doc {
        pub pages: Vec<PageData>,
        pub bookmarks: Vec<Bookmark>,
        pub title: String,
    }

    const FONTS: [&str; 6] = ["Helvetica", "Helvetica-Bold", "Times-Roman", "Times-Bold", "Courier", "Courier-Bold"];

    // ASCII 32..=126 advance widths of Helvetica (1/1000 em).
    const HELV: [u16; 95] = [
        278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278,
        584, 584, 584, 556, 1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944,
        667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500,
        278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
    ];

    /// Advance width of `text` in points for one of the standard fonts.
    pub fn text_width(text: &str, font: &str, size: f64) -> f64 {
        let mono = font.starts_with("Courier");
        let serif = font.starts_with("Times");
        let bold = font.contains("Bold");
        let units: f64 = text
            .chars()
            .map(|c| {
                if mono {
                    600.0
                } else {
                    let w = (c as usize).checked_sub(32).and_then(|i| HELV.get(i)).copied().unwrap_or(556) as f64;
                    w * if serif { 0.92 } else { 1.0 } * if bold { 1.06 } else { 1.0 }
                }
            })
            .sum();
        units * size / 1000.0
    }

    pub fn wrap(text: &str, font: &str, size: f64, max_w: f64) -> Vec<String> {
        let mut lines = Vec::new();
        let mut cur = String::new();
        for word in text.split_whitespace() {
            let test = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if text_width(&test, font, size) <= max_w || cur.is_empty() {
                cur = test;
            } else {
                lines.push(std::mem::replace(&mut cur, word.to_string()));
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        lines
    }

    fn n(v: f64) -> String {
        if !v.is_finite() {
            return "0".into();
        }
        let s = format!("{v:.3}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        if s.is_empty() || s == "-" || s == "-0" { "0".into() } else { s.to_string() }
    }

    fn color_ops(c: &Color, stroke: bool) -> String {
        match c {
            Color::RGB(r, g, b) => format!("{} {} {} {}", n(*r), n(*g), n(*b), if stroke { "RG" } else { "rg" }),
            Color::CMYK(c, m, y, k) => {
                format!("{} {} {} {} {}", n(*c), n(*m), n(*y), n(*k), if stroke { "K" } else { "k" })
            }
            Color::Gray(g) => format!("{} {}", n(*g), if stroke { "G" } else { "g" }),
            Color::Named { components, .. } => {
                let g = components.first().copied().unwrap_or(0.0);
                format!("{} {}", n(g), if stroke { "G" } else { "g" })
            }
        }
    }

    fn blend_name(b: BlendMode) -> &'static str {
        match b {
            BlendMode::Normal => "Normal",
            BlendMode::Multiply => "Multiply",
            BlendMode::Screen => "Screen",
            BlendMode::Overlay => "Overlay",
            BlendMode::Darken => "Darken",
            BlendMode::Lighten => "Lighten",
            BlendMode::ColorDodge => "ColorDodge",
            BlendMode::ColorBurn => "ColorBurn",
            BlendMode::HardLight => "HardLight",
            BlendMode::SoftLight => "SoftLight",
            BlendMode::Difference => "Difference",
            BlendMode::Exclusion => "Exclusion",
        }
    }

    fn path_ops(out: &mut String, p: &Path) {
        for seg in &p.segments {
            let _ = match seg {
                Segment::MoveTo(a) => writeln!(out, "{} {} m", n(a.x), n(a.y)),
                Segment::LineTo(a) => writeln!(out, "{} {} l", n(a.x), n(a.y)),
                Segment::CurveTo { control_point1: a, control_point2: b, endpoint: c } => {
                    writeln!(out, "{} {} {} {} {} {} c", n(a.x), n(a.y), n(b.x), n(b.y), n(c.x), n(c.y))
                }
                Segment::CurveToV { control_point2: b, endpoint: c } => {
                    writeln!(out, "{} {} {} {} v", n(b.x), n(b.y), n(c.x), n(c.y))
                }
                Segment::CurveToY { control_point1: a, endpoint: c } => {
                    writeln!(out, "{} {} {} {} y", n(a.x), n(a.y), n(c.x), n(c.y))
                }
                Segment::ClosePath => writeln!(out, "h"),
                Segment::RectSegment(r) => writeln!(out, "{} {} {} {} re", n(r.x), n(r.y), n(r.width), n(r.height)),
            };
        }
    }

    fn pstr(s: &str) -> String {
        let mut o = String::from("(");
        for ch in s.chars() {
            match ch {
                '\\' | '(' | ')' => {
                    o.push('\\');
                    o.push(ch);
                }
                c if c.is_ascii() && !c.is_ascii_control() => o.push(c),
                _ => o.push('?'),
            }
        }
        o.push(')');
        o
    }

    fn rect_arr(r: &Rect) -> String {
        format!("[{} {} {} {}]", n(r.x), n(r.y), n(r.x + r.width), n(r.y + r.height))
    }

    fn quad_points(qs: &[Quad]) -> String {
        qs.iter()
            .flat_map(|q| [q.top_left, q.top_right, q.bottom_left, q.bottom_right])
            .map(|p| format!("{} {}", n(p.x), n(p.y)))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn rgb_of(c: &Color) -> String {
        match c {
            Color::RGB(r, g, b) => format!("{} {} {}", n(*r), n(*g), n(*b)),
            Color::Gray(g) => format!("{0} {0} {0}", n(*g)),
            Color::CMYK(c, m, y, k) => {
                format!("{} {} {}", n((1.0 - c) * (1.0 - k)), n((1.0 - m) * (1.0 - k)), n((1.0 - y) * (1.0 - k)))
            }
            Color::Named { .. } => "0 0 0".into(),
        }
    }

    impl Doc {
        pub fn add_page(&mut self, w: f64, h: f64) -> usize {
            self.pages.push(PageData { media: Some(Rect::new(0.0, 0.0, w, h)), ..Default::default() });
            self.pages.len()
        }

        fn pg(&mut self, page: usize) -> &mut PageData {
            let last = self.pages.len().saturating_sub(1);
            let idx = page.saturating_sub(1).min(last);
            &mut self.pages[idx]
        }

        pub fn path(&mut self, page: usize, mut path: Path, paint: Paint) {
            path.fill = paint.fill.is_some();
            path.stroke = paint.stroke.is_some();
            self.pg(page).ops.push(Op::Path(path, paint));
        }

        pub fn rect(&mut self, page: usize, r: Rect, fill: Option<Color>, stroke: Option<Color>, lw: f64) {
            let mut p = Path::new();
            p.segments.push(Segment::RectSegment(r));
            let paint = Paint { fill, stroke, width: lw, gs: ExtendedGraphicState::default() };
            self.path(page, p, paint);
        }

        pub fn text(&mut self, page: usize, s: &str, x: f64, y: f64, font: &str, size: f64, color: Color) {
            self.text_matrix(page, s, Matrix::translation(x, y), font, size, color);
        }

        pub fn text_matrix(&mut self, page: usize, s: &str, m: Matrix, font: &str, size: f64, color: Color) {
            let mut run = TextRun::new(s, Font::standard(font), size);
            run.matrix = m;
            run.color = color;
            self.pg(page).ops.push(Op::Text(run));
        }

        pub fn text_centered(&mut self, page: usize, s: &str, cx: f64, y: f64, font: &str, size: f64, color: Color) {
            let w = text_width(s, font, size);
            self.text(page, s, cx - w / 2.0, y, font, size, color);
        }

        /// Word-wrapped paragraph from the top of `r`; returns the number of lines placed.
        pub fn text_box(&mut self, page: usize, s: &str, r: Rect, font: &str, size: f64, color: Color, lead: f64) -> usize {
            let lines = wrap(s, font, size, r.width);
            let mut y = r.y + r.height - size;
            let mut placed = 0;
            for l in lines {
                if y < r.y {
                    break;
                }
                self.text(page, &l, r.x, y, font, size, color.clone());
                y -= size * lead;
                placed += 1;
            }
            placed
        }

        pub fn image(&mut self, page: usize, w: u32, h: u32, rgb: Vec<u8>, rect: Rect) {
            let id = self.pages.iter().flat_map(|p| p.ops.iter()).filter(|o| matches!(o, Op::Image { .. })).count();
            self.pg(page).ops.push(Op::Image { id, w, h, rgb, rect });
        }

        pub fn annotate(&mut self, page: usize, a: Annotation) {
            self.pg(page).annots.push(a);
        }

        pub fn field(&mut self, f: Field) {
            let page = f.page_number;
            self.pg(page).fields.push(f);
        }

        pub fn counts(&self) -> (usize, usize, usize, usize, usize) {
            let (mut paths, mut curves, mut texts, mut imgs, mut annots) = (0, 0, 0, 0, 0);
            for p in &self.pages {
                annots += p.annots.len() + p.fields.len();
                for o in &p.ops {
                    match o {
                        Op::Path(path, _) => {
                            paths += 1;
                            curves += path.segments.iter().filter(|s| matches!(s, Segment::CurveTo { .. })).count();
                        }
                        Op::Text(_) => texts += 1,
                        Op::Image { .. } => imgs += 1,
                    }
                }
            }
            let _ = imgs;
            (self.pages.len(), paths, curves, texts, annots)
        }

        pub fn to_bytes(&self) -> Res<Vec<u8>> {
            if self.pages.is_empty() {
                return Err("cannot serialize a document with no pages".into());
            }
            let mut objs: Vec<Vec<u8>> = Vec::new();
            let alloc = |objs: &mut Vec<Vec<u8>>| {
                objs.push(Vec::new());
                objs.len()
            };
            let stream = |dict: &str, data: &[u8]| -> Vec<u8> {
                let mut v = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
                v.extend_from_slice(data);
                v.extend_from_slice(b"\nendstream");
                v
            };

            let catalog = alloc(&mut objs);
            let pages_id = alloc(&mut objs);
            let info = alloc(&mut objs);
            let mut font_ids = Vec::new();
            for f in FONTS {
                let id = alloc(&mut objs);
                objs[id - 1] = format!("<< /Type /Font /Subtype /Type1 /BaseFont /{f} /Encoding /WinAnsiEncoding >>").into_bytes();
                font_ids.push(id);
            }
            let page_ids: Vec<usize> = self.pages.iter().map(|_| alloc(&mut objs)).collect();
            let mut field_ids = Vec::new();

            for (pi, page) in self.pages.iter().enumerate() {
                let mut content = String::new();
                let mut gstates: Vec<ExtendedGraphicState> = Vec::new();
                let mut xobjs = String::new();
                for op in &page.ops {
                    match op {
                        Op::Path(path, paint) => {
                            let _ = writeln!(content, "q");
                            if paint.gs != ExtendedGraphicState::default() {
                                let idx = gstates.iter().position(|g| *g == paint.gs).unwrap_or_else(|| {
                                    gstates.push(paint.gs.clone());
                                    gstates.len() - 1
                                });
                                let _ = writeln!(content, "/GS{idx} gs");
                            }
                            if let Some(c) = &paint.fill {
                                let _ = writeln!(content, "{}", color_ops(c, false));
                            }
                            if let Some(c) = &paint.stroke {
                                let _ = writeln!(content, "{}", color_ops(c, true));
                                let _ = writeln!(content, "{} w 1 J 1 j", n(paint.width));
                            }
                            path_ops(&mut content, path);
                            let paint_op = match (path.fill, path.stroke, path.even_odd) {
                                (true, true, false) => "B",
                                (true, true, true) => "B*",
                                (true, false, false) => "f",
                                (true, false, true) => "f*",
                                (false, true, _) => "S",
                                (false, false, _) => "n",
                            };
                            let _ = writeln!(content, "{paint_op}\nQ");
                        }
                        Op::Text(run) => {
                            let fname = if FONTS.contains(&run.font.base_font.as_str()) { run.font.base_font.as_str() } else { "Helvetica" };
                            let m = &run.matrix;
                            let _ = writeln!(
                                content,
                                "BT /{} {} Tf {} {} {} {} {} {} {} Tm {} Tj ET",
                                fname.replace('-', "_"),
                                n(run.font_size),
                                color_ops(&run.color, false),
                                n(m.a),
                                n(m.b),
                                n(m.c),
                                n(m.d),
                                n(m.tx),
                                n(m.ty),
                                pstr(&run.text)
                            );
                        }
                        Op::Image { id, w, h, rgb, rect } => {
                            let oid = alloc(&mut objs);
                            let data = pdfcraft_filters::encode_flate(rgb);
                            objs[oid - 1] = stream(
                                &format!(
                                    "/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode"
                                ),
                                &data,
                            );
                            let _ = write!(xobjs, "/Im{id} {oid} 0 R ");
                            let _ = writeln!(content, "q {} 0 0 {} {} {} cm /Im{id} Do Q", n(rect.width), n(rect.height), n(rect.x), n(rect.y));
                        }
                    }
                }
                let content_id = alloc(&mut objs);
                objs[content_id - 1] = stream("/Filter /FlateDecode", &pdfcraft_filters::encode_flate(content.as_bytes()));

                let mut annot_ids = Vec::new();
                for a in &page.annots {
                    let id = alloc(&mut objs);
                    let mut common = format!("/Type /Annot /Rect {} /F 4", rect_arr(&a.rect));
                    if let Some(c) = &a.contents {
                        let _ = write!(common, " /Contents {}", pstr(c));
                    }
                    if let Some(au) = &a.author {
                        let _ = write!(common, " /T {}", pstr(au));
                    }
                    if let Some(c) = &a.color {
                        let _ = write!(common, " /C [{}]", rgb_of(c));
                    }
                    let body = match &a.subtype {
                        AnnotationSubtype::Link(LinkAnnotation { action, destination }) => {
                            let act = action.as_ref().or(a.action.as_ref());
                            let target = match (act, destination) {
                                (Some(Action::URI(URIAction { uri, .. })), _) => format!("/A << /S /URI /URI {} >>", pstr(uri)),
                                (Some(Action::GoTo(GoToAction { destination: d })), _) | (None, Some(d)) => {
                                    let pid = page_ids.get(d.page_number.saturating_sub(1)).copied().unwrap_or(page_ids[0]);
                                    format!("/Dest [{pid} 0 R /Fit]")
                                }
                                _ => String::new(),
                            };
                            format!("<< {common} /Subtype /Link /Border [0 0 0] {target} >>")
                        }
                        AnnotationSubtype::Highlight(HighlightAnnotation { quads, color }) => {
                            let ap = alloc(&mut objs);
                            let r = &a.rect;
                            objs[ap - 1] = stream(
                                &format!(
                                    "/Type /XObject /Subtype /Form /BBox {} /Resources << /ExtGState << /G << /ca {} /BM /Multiply >> >> >>",
                                    rect_arr(r),
                                    n(a.opacity)
                                ),
                                format!("/G gs {} rg {} {} {} {} re f", rgb_of(color), n(r.x), n(r.y), n(r.width), n(r.height)).as_bytes(),
                            );
                            format!(
                                "<< {common} /Subtype /Highlight /QuadPoints [{}] /CA {} /AP << /N {ap} 0 R >> >>",
                                quad_points(quads),
                                n(a.opacity)
                            )
                        }
                        AnnotationSubtype::Ink(InkAnnotation { ink_list, stroke_width }) => {
                            let lists = ink_list
                                .iter()
                                .map(|l| format!("[{}]", l.iter().map(|p| format!("{} {}", n(p.x), n(p.y))).collect::<Vec<_>>().join(" ")))
                                .collect::<Vec<_>>()
                                .join(" ");
                            format!("<< {common} /Subtype /Ink /InkList [{lists}] /BS << /W {} >> >>", n(*stroke_width))
                        }
                        AnnotationSubtype::Line(LineAnnotation { start_point: s, end_point: e, start_ending, end_ending, line_width }) => {
                            let le = |l: &LineEnding| match l {
                                LineEnding::OpenArrow => "OpenArrow",
                                LineEnding::ClosedArrow => "ClosedArrow",
                                LineEnding::Circle => "Circle",
                                LineEnding::Diamond => "Diamond",
                                LineEnding::Square => "Square",
                                _ => "None",
                            };
                            format!(
                                "<< {common} /Subtype /Line /L [{} {} {} {}] /LE [/{} /{}] /BS << /W {} >> >>",
                                n(s.x),
                                n(s.y),
                                n(e.x),
                                n(e.y),
                                le(start_ending),
                                le(end_ending),
                                n(*line_width)
                            )
                        }
                        _ => format!("<< {common} /Subtype /Text >>"),
                    };
                    objs[id - 1] = body.into_bytes();
                    annot_ids.push(id);
                }

                for f in &page.fields {
                    let r = &f.rect;
                    let id = alloc(&mut objs);
                    let mk = "/MK << /BC [0.2 0.5 0.8] /BG [0.96 0.98 1] >>";
                    let name = pstr(&f.name);
                    let body = match &f.field_type {
                        FieldType::Button(b) => {
                            let (w, h) = (r.width, r.height);
                            let off_ap = alloc(&mut objs);
                            let on_ap = alloc(&mut objs);
                            let frame =
                                format!("0.96 0.98 1 rg 0 0 {} {} re f 0.2 0.5 0.8 RG 1.5 w 1 1 {} {} re S", n(w), n(h), n(w - 2.0), n(h - 2.0));
                            let bbox = format!("/Type /XObject /Subtype /Form /BBox [0 0 {} {}]", n(w), n(h));
                            objs[off_ap - 1] = stream(&bbox, frame.as_bytes());
                            objs[on_ap - 1] = stream(
                                &bbox,
                                format!("{frame} 0.1 0.4 0.75 RG 2.2 w 4 {} m {} 4 l {} {} l S", n(h * 0.5), n(w * 0.42), n(w - 4.0), n(h - 4.0))
                                    .as_bytes(),
                            );
                            let st = if b.checked { "Yes" } else { "Off" };
                            format!(
                                "<< /Type /Annot /Subtype /Widget /FT /Btn /T {name} /Rect {} /F 4 /V /{st} /AS /{st} /AP << /N << /Yes {on_ap} 0 R /Off {off_ap} 0 R >> >> {mk} >>",
                                rect_arr(r)
                            )
                        }
                        _ => {
                            let ap = alloc(&mut objs);
                            let (w, h) = (r.width, r.height);
                            let val = f.value.as_deref().unwrap_or("");
                            objs[ap - 1] = stream(
                                &format!("/Type /XObject /Subtype /Form /BBox [0 0 {} {}] /Resources << /Font << /Courier {} 0 R >> >>", n(w), n(h), font_ids[4]),
                                format!(
                                    "0.96 0.98 1 rg 0 0 {w} {h} re f 0.65 0.75 0.88 RG 1 w 0.5 0.5 {} {} re S BT /Courier 10 Tf 0.1 0.1 0.15 rg 6 {} Td {} Tj ET",
                                    n(w - 1.0), n(h - 1.0), n(h / 2.0 - 3.5), pstr(val), w = n(w), h = n(h)
                                ).as_bytes(),
                            );
                            format!(
                                "<< /Type /Annot /Subtype /Widget /FT /Tx /T {name} /V {} /Rect {} /F 4 /DA (/Courier 10 Tf 0.1 0.1 0.15 rg) /AP << /N {ap} 0 R >> {mk} >>",
                                pstr(val),
                                rect_arr(r)
                            )
                        }
                    };
                    objs[id - 1] = body.into_bytes();
                    annot_ids.push(id);
                    field_ids.push(id);
                }

                let gs_res: String = gstates
                    .iter()
                    .enumerate()
                    .map(|(i, g)| format!("/GS{i} << /CA {} /ca {} /BM /{} >> ", n(g.alpha_stroke), n(g.alpha_fill), blend_name(g.blend_mode)))
                    .collect();
                let fonts: String = FONTS.iter().zip(&font_ids).map(|(f, id)| format!("/{} {id} 0 R ", f.replace('-', "_"))).collect();
                let mb = page.media.unwrap_or(Rect::new(0.0, 0.0, 612.0, 792.0));
                let annots = if annot_ids.is_empty() {
                    String::new()
                } else {
                    format!("/Annots [{}]", annot_ids.iter().map(|i| format!("{i} 0 R")).collect::<Vec<_>>().join(" "))
                };
                objs[page_ids[pi] - 1] = format!(
                    "<< /Type /Page /Parent {pages_id} 0 R /MediaBox {} /Contents {content_id} 0 R /Resources << /Font << {fonts}>> {}{} >> {annots} >>",
                    rect_arr(&mb),
                    if xobjs.is_empty() { String::new() } else { format!("/XObject << {xobjs}>> ") },
                    if gs_res.is_empty() { String::new() } else { format!("/ExtGState << {gs_res}>>") },
                )
                .into_bytes();
            }

            // Outline from SDK Bookmark tree.
            let mut outline = String::new();
            if !self.bookmarks.is_empty() {
                let root = alloc(&mut objs);
                fn build(objs: &mut Vec<Vec<u8>>, items: &[Bookmark], parent: usize, pages: &[usize]) -> Vec<usize> {
                    let ids: Vec<usize> = items
                        .iter()
                        .map(|_| {
                            objs.push(Vec::new());
                            objs.len()
                        })
                        .collect();
                    for (i, b) in items.iter().enumerate() {
                        let kids = build(objs, &b.children, ids[i], pages);
                        let dest = b
                            .destination
                            .as_ref()
                            .and_then(|d| pages.get(d.page_number.saturating_sub(1)).map(|p| format!("/Dest [{p} 0 R /Fit]")))
                            .unwrap_or_default();
                        let mut s = format!("<< /Title {} /Parent {parent} 0 R {dest}", pstr(&b.title));
                        if i > 0 {
                            let _ = write!(s, " /Prev {} 0 R", ids[i - 1]);
                        }
                        if i + 1 < ids.len() {
                            let _ = write!(s, " /Next {} 0 R", ids[i + 1]);
                        }
                        if let (Some(f), Some(l)) = (kids.first(), kids.last()) {
                            let _ = write!(s, " /First {f} 0 R /Last {l} 0 R /Count {}", kids.len());
                        }
                        s.push_str(" >>");
                        objs[ids[i] - 1] = s.into_bytes();
                    }
                    ids
                }
                let top = build(&mut objs, &self.bookmarks, root, &page_ids);
                if let (Some(f), Some(l)) = (top.first(), top.last()) {
                    objs[root - 1] = format!("<< /Type /Outlines /First {f} 0 R /Last {l} 0 R /Count {} >>", top.len()).into_bytes();
                    outline = format!("/Outlines {root} 0 R /PageMode /UseOutlines");
                }
            }

            let acro = if field_ids.is_empty() {
                String::new()
            } else {
                format!(
                    "/AcroForm << /Fields [{}] /NeedAppearances true /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv {} 0 R /Courier {} 0 R >> >> >>",
                    field_ids.iter().map(|i| format!("{i} 0 R")).collect::<Vec<_>>().join(" "),
                    font_ids[0],
                    font_ids[4]
                )
            };
            objs[catalog - 1] = format!("<< /Type /Catalog /Pages {pages_id} 0 R {outline} {acro} >>").into_bytes();
            objs[pages_id - 1] = format!(
                "<< /Type /Pages /Count {} /Kids [{}] >>",
                page_ids.len(),
                page_ids.iter().map(|i| format!("{i} 0 R")).collect::<Vec<_>>().join(" ")
            )
            .into_bytes();
            objs[info - 1] = format!("<< /Title {} /Author (PdfCraft SDK) /Creator (pdfcraft-sdk Rust example) >>", pstr(&self.title)).into_bytes();

            let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
            let mut offsets = Vec::with_capacity(objs.len());
            for (i, body) in objs.iter().enumerate() {
                offsets.push(out.len());
                out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
                out.extend_from_slice(body);
                out.extend_from_slice(b"\nendobj\n");
            }
            let xref_pos = out.len();
            let mut xref = format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1);
            for o in offsets {
                let _ = writeln!(xref, "{o:010} 00000 n ");
            }
            out.extend_from_slice(xref.as_bytes());
            out.extend_from_slice(
                format!("trailer\n<< /Size {} /Root {catalog} 0 R /Info {info} 0 R >>\nstartxref\n{xref_pos}\n%%EOF\n", objs.len() + 1).as_bytes(),
            );
            Ok(out)
        }
    }
}

use pdf::{Doc, Paint, text_width};

// =================================================================================================
// Geometry helpers producing SDK `Path`s
// =================================================================================================
const KAPPA: f64 = 0.552_284_749_8;

fn pt(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

fn rgb(r: f64, g: f64, b: f64) -> Color {
    Color::RGB(r, g, b)
}

fn hsv(h: f64, s: f64, v: f64) -> Color {
    let h = h.rem_euclid(1.0);
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    let (r, g, b) = match (i as i64).rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    Color::RGB(r, g, b)
}

fn rot(deg: f64) -> Matrix {
    let (s, c) = deg.to_radians().sin_cos();
    Matrix::new(c, s, -s, c, 0.0, 0.0)
}

/// Rotation about an arbitrary centre, composed with `Matrix::multiply`.
fn rot_about(deg: f64, cx: f64, cy: f64) -> Matrix {
    Matrix::translation(-cx, -cy).multiply(&rot(deg)).multiply(&Matrix::translation(cx, cy))
}

fn map_path(p: &Path, m: &Matrix) -> Path {
    let t = |q: &Point| m.transform_point(*q);
    let mut out = Path::new();
    out.even_odd = p.even_odd;
    for s in &p.segments {
        out.segments.push(match s {
            Segment::MoveTo(a) => Segment::MoveTo(t(a)),
            Segment::LineTo(a) => Segment::LineTo(t(a)),
            Segment::CurveTo { control_point1, control_point2, endpoint } => {
                Segment::CurveTo { control_point1: t(control_point1), control_point2: t(control_point2), endpoint: t(endpoint) }
            }
            other => other.clone(),
        });
    }
    out
}

fn ellipse(cx: f64, cy: f64, rx: f64, ry: f64) -> Path {
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    let mut p = Path::new();
    p.move_to(pt(cx + rx, cy));
    p.curve_to(pt(cx + rx, cy + ky), pt(cx + kx, cy + ry), pt(cx, cy + ry));
    p.curve_to(pt(cx - kx, cy + ry), pt(cx - rx, cy + ky), pt(cx - rx, cy));
    p.curve_to(pt(cx - rx, cy - ky), pt(cx - kx, cy - ry), pt(cx, cy - ry));
    p.curve_to(pt(cx + kx, cy - ry), pt(cx + rx, cy - ky), pt(cx + rx, cy));
    p.close();
    p
}

fn circle(cx: f64, cy: f64, r: f64) -> Path {
    ellipse(cx, cy, r, r)
}

fn polygon(points: &[Point]) -> Path {
    let mut p = Path::new();
    for (i, q) in points.iter().enumerate() {
        if i == 0 { p.move_to(*q) } else { p.line_to(*q) }
    }
    p.close();
    p
}

/// Closed Catmull-Rom spline converted to cubic Beziers.
fn smooth_closed(pts: &[Point]) -> Path {
    let n = pts.len();
    let mut p = Path::new();
    if n < 3 {
        return p;
    }
    p.move_to(pts[0]);
    for i in 0..n {
        let (p0, p1, p2, p3) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n], pts[(i + 2) % n]);
        p.curve_to(pt(p1.x + (p2.x - p0.x) / 6.0, p1.y + (p2.y - p0.y) / 6.0), pt(p2.x - (p3.x - p1.x) / 6.0, p2.y - (p3.y - p1.y) / 6.0), p2);
    }
    p.close();
    p
}

/// Open Catmull-Rom spline (end points duplicated).
fn smooth_open(pts: &[Point]) -> Path {
    let n = pts.len();
    let mut p = Path::new();
    if n < 2 {
        return p;
    }
    p.move_to(pts[0]);
    for i in 0..n - 1 {
        let p0 = pts[i.saturating_sub(1)];
        let (p1, p2) = (pts[i], pts[i + 1]);
        let p3 = pts[(i + 2).min(n - 1)];
        p.curve_to(pt(p1.x + (p2.x - p0.x) / 6.0, p1.y + (p2.y - p0.y) / 6.0), pt(p2.x - (p3.x - p1.x) / 6.0, p2.y - (p3.y - p1.y) / 6.0), p2);
    }
    p
}

/// Circular arc as cubic Beziers (<= 90 degrees per segment); appended to `p` (assumes current point at start).
fn arc_to(p: &mut Path, cx: f64, cy: f64, r: f64, a0: f64, a1: f64) {
    let steps = ((a1 - a0).abs() / (PI / 2.0)).ceil().max(1.0) as usize;
    let da = (a1 - a0) / steps as f64;
    let k = 4.0 / 3.0 * (da / 4.0).tan();
    for i in 0..steps {
        let (s, e) = (a0 + da * i as f64, a0 + da * (i + 1) as f64);
        let (p1, p4) = (pt(cx + r * s.cos(), cy + r * s.sin()), pt(cx + r * e.cos(), cy + r * e.sin()));
        let c1 = pt(p1.x - k * r * s.sin(), p1.y + k * r * s.cos());
        let c2 = pt(p4.x + k * r * e.sin(), p4.y - k * r * e.cos());
        p.curve_to(c1, c2, p4);
    }
}

fn ring_slice(cx: f64, cy: f64, r_out: f64, r_in: f64, a0: f64, a1: f64) -> Path {
    let mut p = Path::new();
    p.move_to(pt(cx + r_out * a0.cos(), cy + r_out * a0.sin()));
    arc_to(&mut p, cx, cy, r_out, a0, a1);
    p.line_to(pt(cx + r_in * a1.cos(), cy + r_in * a1.sin()));
    arc_to(&mut p, cx, cy, r_in, a1, a0);
    p.close();
    p
}

fn gear(cx: f64, cy: f64, r_out: f64, r_in: f64, teeth: usize, phase: f64) -> Path {
    let mut pts = Vec::new();
    let step = 2.0 * PI / teeth as f64;
    for i in 0..teeth {
        let a = phase + step * i as f64;
        for (da, r) in [(0.0, r_in), (0.18, r_out), (0.42, r_out), (0.6, r_in)] {
            let ang = a + step * da;
            pts.push(pt(cx + r * ang.cos(), cy + r * ang.sin()));
        }
    }
    polygon(&pts)
}

fn star(cx: f64, cy: f64, r: f64) -> Path {
    let pts: Vec<Point> = (0..5)
        .map(|i| {
            let a = PI / 2.0 + i as f64 * 4.0 * PI / 5.0;
            pt(cx + r * a.cos(), cy + r * a.sin())
        })
        .collect();
    let mut p = polygon(&pts);
    p.even_odd = true;
    p
}

// =================================================================================================
// Raster helper
// =================================================================================================
fn gradient_strip(w: u32, h: u32, f: impl Fn(f64, f64) -> (f64, f64, f64)) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = f(x as f64 / w as f64, y as f64 / h as f64);
            v.extend([(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]);
        }
    }
    v
}

// =================================================================================================
// Palette
// =================================================================================================
struct Pal;
impl Pal {
    fn night() -> Color {
        rgb(0.03, 0.06, 0.08)
    }
    fn ink() -> Color {
        rgb(0.06, 0.09, 0.12)
    }
    fn paper() -> Color {
        rgb(0.97, 0.975, 0.96)
    }
    fn mint() -> Color {
        rgb(0.13, 0.78, 0.62)
    }
    fn ember() -> Color {
        rgb(0.96, 0.45, 0.2)
    }
    fn gold() -> Color {
        rgb(0.98, 0.78, 0.25)
    }
    fn sky() -> Color {
        rgb(0.25, 0.6, 0.95)
    }
    fn rose() -> Color {
        rgb(0.92, 0.28, 0.45)
    }
    fn muted() -> Color {
        rgb(0.42, 0.47, 0.5)
    }
    fn rule() -> Color {
        rgb(0.85, 0.87, 0.86)
    }
}

const PW: f64 = 612.0;
const PH: f64 = 792.0;

fn background(d: &mut Doc, page: usize, c: Color) {
    d.rect(page, Rect::new(0.0, 0.0, PW, PH), Some(c), None, 0.0);
}

fn heading(d: &mut Doc, page: usize, kicker: &str, title: &str, dark: bool) {
    d.rect(page, Rect::new(48.0, 716.0, 36.0, 5.0), Some(Pal::ember()), None, 0.0);
    d.text(page, &kicker.to_uppercase(), 48.0, 730.0, "Helvetica-Bold", 9.0, if dark { Pal::gold() } else { Pal::ember() });
    d.text(page, title, 48.0, 684.0, "Helvetica-Bold", 30.0, if dark { rgb(1.0, 1.0, 1.0) } else { Pal::ink() });
}

fn footer(d: &mut Doc, page: usize, dark: bool) {
    let c = if dark { rgb(0.55, 0.65, 0.68) } else { Pal::muted() };
    d.rect(page, Rect::new(48.0, 46.0, PW - 96.0, 0.8), Some(c.clone()), None, 0.0);
    d.text(page, "PdfCraft Rust SDK  |  generated by sdk_capability_tour.rs", 48.0, 30.0, "Helvetica", 8.0, c.clone());
    d.text(page, &format!("{page:02}"), PW - 64.0, 30.0, "Helvetica-Bold", 9.0, c);
}

// =================================================================================================
// Engine measurements (filled by a first pass, rendered on page 5 of the second pass)
// =================================================================================================
#[derive(Default, Clone)]
struct EngineReport {
    renders: Vec<(usize, u32, u32, usize, f64)>, // page, w, h, bytes, ms
    split_files: usize,
    split_ms: f64,
    merge_pages: usize,
    merge_ms: f64,
    errors: Vec<(String, String, u16)>, // what, code, http
    limits: Option<ResourceLimits>,
}

fn png_dims(bytes: &[u8]) -> (u32, u32) {
    let be = |i: usize| bytes.get(i..i + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0);
    (be(16), be(20))
}

fn measure_engine(pdf: &[u8], pages: usize, dpi: f64) -> Res<(EngineReport, Vec<Vec<u8>>)> {
    let client = LocalClient::default();
    let src = DocumentSource::from_bytes(pdf.to_vec(), Some("tour.pdf".into()));
    let mut rep = EngineReport { limits: Some(client.limits().clone()), ..Default::default() };
    let mut pngs = Vec::new();

    for page in 1..=pages {
        let t = Instant::now();
        let res = client.render_page(&src, page, RenderOptions { dpi, ..Default::default() })?;
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if let DocumentResult::Memory { data, .. } = res {
            let (w, h) = png_dims(&data);
            rep.renders.push((page, w, h, data.len(), ms));
            pngs.push(data);
        }
    }

    let t = Instant::now();
    let split = client.split(&src, SplitMode::EveryNPages(1))?;
    rep.split_ms = t.elapsed().as_secs_f64() * 1000.0;
    rep.split_files = split.files.len();

    let t = Instant::now();
    let merged = client.merge(&[src.clone(), src.clone()], MergeOptions::default())?;
    rep.merge_ms = t.elapsed().as_secs_f64() * 1000.0;
    if let DocumentResult::Memory { data, .. } = merged {
        let probe = DocumentSource::from_bytes(data, Some("merged.pdf".into()));
        rep.merge_pages = client.split(&probe, SplitMode::EveryNPages(1))?.files.len();
    }

    for (what, r) in [
        ("render_page(page 0)", client.render_page(&src, 0, RenderOptions::default())),
        ("render_page(page 99)", client.render_page(&src, 99, RenderOptions::default())),
        ("merge([])", client.merge(&[], MergeOptions::default())),
    ] {
        if let Err(e) = r {
            rep.errors.push((what.to_string(), e.error_code().to_string(), e.http_status()));
        }
    }
    Ok((rep, pngs))
}

// =================================================================================================
// Pages
// =================================================================================================
fn page_cover(d: &mut Doc, counts: (usize, usize, usize, usize, usize)) {
    let p = d.add_page(PW, PH);
    background(d, p, Pal::night());
    d.rect(p, Rect::new(0.0, 0.0, 14.0, PH), Some(Pal::mint()), None, 0.0);

    d.text(p, "PDFCRAFT  /  RUST SDK", 48.0, 736.0, "Helvetica-Bold", 10.0, Pal::gold());
    d.text(p, "Curves, composed", 48.0, 672.0, "Helvetica-Bold", 40.0, rgb(1.0, 1.0, 1.0));
    d.text(p, "in safe Rust.", 48.0, 626.0, "Helvetica-Bold", 40.0, Pal::mint());

    // Layered Bezier "flower": 24 rotated ellipses, additive Screen blending.
    let (cx, cy) = (306.0, 410.0);
    for i in 0..24 {
        let deg = i as f64 * 15.0;
        let base = ellipse(cx, cy + 60.0, 62.0, 138.0);
        let path = map_path(&base, &rot_about(deg, cx, cy));
        let c = hsv(0.42 + 0.42 * (i as f64 / 24.0), 0.72, 1.0);
        d.path(p, path, Paint::both(c.clone(), c, 0.7).alpha(0.16).blend(BlendMode::Screen));
    }
    // Hypotrochoid spirograph in the middle, built from a smooth Bezier spline.
    let pts: Vec<Point> = (0..210)
        .map(|i| {
            let t = i as f64 / 210.0 * 2.0 * PI * 7.0;
            let (big_r, small_r, dist) = (60.0, 8.57, 40.0);
            let k = (big_r - small_r) / small_r;
            pt(cx + (big_r - small_r) * t.cos() + dist * (k * t).cos(), cy + (big_r - small_r) * t.sin() - dist * (k * t).sin())
        })
        .collect();
    d.path(p, smooth_closed(&pts), Paint::stroke(Pal::gold(), 0.8).alpha(0.95));
    d.path(p, circle(cx, cy, 5.0), Paint::fill(rgb(1.0, 1.0, 1.0)));

    // Raster gradient band (image XObject, Flate-compressed through pdfcraft-filters).
    let (w, h) = (516u32, 14u32);
    let rgb_data = gradient_strip(w, h, |x, _| {
        let Color::RGB(r, g, b) = hsv(0.42 + 0.42 * x, 0.7, 0.95) else { return (0.0, 0.0, 0.0) };
        (r, g, b)
    });
    d.image(p, w, h, rgb_data, Rect::new(48.0, 218.0, 516.0, 8.0));

    let (pages, paths, curves, texts, annots) = counts;
    let tiles = [
        (pages.to_string(), "pages", Pal::mint()),
        (paths.to_string(), "vector paths", Pal::ember()),
        (curves.to_string(), "Bezier segments", Pal::sky()),
        (format!("{texts}/{annots}"), "text runs / annots", Pal::gold()),
    ];
    for (i, (v, label, c)) in tiles.iter().enumerate() {
        let x = 48.0 + i as f64 * 132.0;
        d.rect(p, Rect::new(x, 110.0, 120.0, 84.0), Some(rgb(0.07, 0.11, 0.14)), Some(c.clone()), 1.3);
        d.rect(p, Rect::new(x, 188.0, 120.0, 6.0), Some(c.clone()), None, 0.0);
        d.text(p, v, x + 14.0, 142.0, "Helvetica-Bold", 26.0, rgb(1.0, 1.0, 1.0));
        d.text(p, label, x + 14.0, 122.0, "Helvetica", 9.5, rgb(0.7, 0.78, 0.8));
    }
    d.text(p, "Counts are measured from the live document model before it is serialized.", 48.0, 88.0, "Helvetica", 8.5, rgb(0.5, 0.6, 0.62));
    footer(d, p, true);
}

fn page_vector(d: &mut Doc) {
    let p = d.add_page(PW, PH);
    background(d, p, Pal::paper());
    heading(d, p, "02  /  Vector Lab", "Paths & Matrices", false);

    let card = |d: &mut Doc, x: f64, y: f64, title: &str, sub: &str| {
        d.rect(p, Rect::new(x, y, 164.0, 196.0), Some(rgb(1.0, 1.0, 1.0)), Some(Pal::rule()), 0.8);
        d.text(p, title, x + 12.0, y + 172.0, "Helvetica-Bold", 11.0, Pal::ink());
        d.text(p, sub, x + 12.0, y + 159.0, "Courier", 7.5, Pal::muted());
    };

    // 1. Gears: one Path, rotated copies via Matrix.
    card(d, 48.0, 440.0, "Interlocking gears", "Matrix::multiply");
    let g1 = gear(110.0, 500.0, 40.0, 31.0, 12, 0.0);
    let g2 = map_path(&gear(0.0, 0.0, 28.0, 20.0, 9, 0.0), &rot(20.0).multiply(&Matrix::translation(177.0, 463.0)));
    for (g, c) in [(g1, Pal::ember()), (g2, Pal::sky())] {
        d.path(p, g, Paint::both(c.clone(), Pal::ink(), 1.2).alpha(0.9));
    }
    d.path(p, circle(110.0, 500.0, 9.0), Paint::fill(rgb(1.0, 1.0, 1.0)));
    d.path(p, circle(177.0, 463.0, 7.0), Paint::fill(rgb(1.0, 1.0, 1.0)));

    // 2. Even-odd star.
    card(d, 224.0, 440.0, "Even-odd star", "Path.even_odd = true");
    d.path(p, star(306.0, 520.0, 60.0), Paint::both(Pal::gold(), Pal::ink(), 1.5));
    d.path(p, circle(306.0, 520.0, 5.0), Paint::fill(Pal::ink()));

    // 3. Smooth wave: Catmull-Rom spline -> cubic Beziers, filled below.
    card(d, 400.0, 440.0, "Bezier wave", "smooth_open -> CurveTo");
    let pts: Vec<Point> =
        (0..=9).map(|i| pt(412.0 + i as f64 * 15.0, 495.0 + 28.0 * (i as f64 * 0.9).sin() + 6.0 * (i as f64 * 2.3).cos())).collect();
    let mut area = smooth_open(&pts);
    area.line_to(pt(547.0, 462.0));
    area.line_to(pt(412.0, 462.0));
    area.close();
    d.path(p, area, Paint::fill(Pal::mint()).alpha(0.35));
    d.path(p, smooth_open(&pts), Paint::stroke(Pal::mint(), 2.4));
    for q in &pts {
        d.path(p, circle(q.x, q.y, 2.6), Paint::both(rgb(1.0, 1.0, 1.0), Pal::mint(), 1.2));
    }

    // 4. Donut chart from arc Beziers.
    card(d, 48.0, 230.0, "Donut chart", "ring_slice: arcs as CurveTo");
    let shares = [("Parse", 34.0, Pal::sky()), ("Render", 29.0, Pal::mint()), ("Edit", 22.0, Pal::ember()), ("Save", 15.0, Pal::gold())];
    let total: f64 = shares.iter().map(|s| s.1).sum();
    let mut a = PI / 2.0;
    for (i, (name, v, c)) in shares.iter().enumerate() {
        let sweep = -2.0 * PI * v / total;
        d.path(p, ring_slice(110.0, 316.0, 42.0, 24.0, a, a + sweep + 0.03), Paint::fill(c.clone()));
        a += sweep;
        let y = 258.0 - i as f64 * 0.0;
        let x = 62.0 + (i as f64 % 2.0) * 72.0;
        d.rect(p, Rect::new(x, y - (i / 2) as f64 * 13.0, 8.0, 8.0), Some(c.clone()), None, 0.0);
        d.text(p, &format!("{name} {v:.0}%"), x + 12.0, y - (i / 2) as f64 * 13.0, "Helvetica", 8.5, Pal::ink());
    }

    // 5. Gauge: arcs and a needle rotated with Matrix.
    card(d, 224.0, 230.0, "Gauge", "arc_to + rot_about");
    let (gx, gy) = (306.0, 290.0);
    for i in 0..3 {
        let (a0, a1) = (PI - i as f64 * PI / 3.0, PI - (i + 1) as f64 * PI / 3.0);
        d.path(p, ring_slice(gx, gy, 52.0, 38.0, a0, a1 + 0.02), Paint::fill([Pal::mint(), Pal::gold(), Pal::rose()][i].clone()));
    }
    let needle = polygon(&[pt(gx, gy + 3.0), pt(gx + 46.0, gy), pt(gx, gy - 3.0)]);
    d.path(p, map_path(&needle, &rot_about(145.0, gx, gy)), Paint::fill(Pal::ink()));
    d.path(p, circle(gx, gy, 7.0), Paint::fill(Pal::ink()));
    d.text_centered(p, "73 %", gx, 252.0, "Helvetica-Bold", 14.0, Pal::ink());

    // 6. Rose curve with rotated text on a baseline.
    card(d, 400.0, 230.0, "Rose curve r = cos(5t)", "Matrix text + closed spline");
    let rose_pts: Vec<Point> = (0..120)
        .map(|i| {
            let t = i as f64 / 120.0 * 2.0 * PI;
            let r = 50.0 * (5.0 * t).cos();
            pt(482.0 + r * t.cos(), 316.0 + r * t.sin())
        })
        .collect();
    d.path(p, smooth_closed(&rose_pts), Paint::both(Pal::rose(), Pal::ink(), 1.0).alpha(0.55));
    d.text_matrix(p, "rotated 90 deg", rot(90.0).multiply(&Matrix::translation(558.0, 240.0)), "Helvetica", 8.0, Pal::muted());

    d.text_box(
        p,
        "Every shape on this page is a pdfcraft_sdk::Path: MoveTo, LineTo and CurveTo segments, a fill/stroke flag, \
         and an even-odd rule. The serializer maps those segments one-to-one onto PDF path operators (m, l, c, h, re), \
         so what you build in Rust is what the engine renders.",
        Rect::new(48.0, 96.0, 516.0, 100.0),
        "Times-Roman",
        11.0,
        Pal::ink(),
        1.4,
    );
    footer(d, p, false);
}

fn page_type_colour(d: &mut Doc) {
    let p = d.add_page(PW, PH);
    background(d, p, Pal::paper());
    heading(d, p, "03  /  Type & Colour", "Type, CMYK, Blend", false);

    // Font specimens
    for (i, f) in ["Helvetica", "Times-Roman", "Courier", "Helvetica-Bold", "Times-Bold", "Courier-Bold"].iter().enumerate() {
        let x = 48.0 + (i % 3) as f64 * 176.0;
        let y = 622.0 - (i / 3) as f64 * 62.0;
        d.rect(p, Rect::new(x, y - 36.0, 164.0, 52.0), Some(rgb(1.0, 1.0, 1.0)), Some(Pal::rule()), 0.8);
        d.text(p, "Aa Gg 42", x + 12.0, y - 10.0, f, 22.0, Pal::ink());
        d.text(p, f, x + 12.0, y - 29.0, "Helvetica", 7.5, Pal::muted());
    }

    // Style runs of one logical line, advanced with measured widths.
    d.text(p, "Mixed runs, measured advances", 48.0, 474.0, "Helvetica-Bold", 12.0, Pal::mint());
    let runs: [(&str, &str, Color); 5] = [
        ("Safe ", "Helvetica-Bold", Pal::ember()),
        ("by ", "Times-Roman", Pal::ink()),
        ("construction", "Courier-Bold", Pal::sky()),
        (", fast ", "Times-Bold", Pal::ink()),
        ("by design.", "Helvetica-Bold", Pal::mint()),
    ];
    let mut x = 48.0;
    for (s, f, c) in runs {
        d.text(p, s, x, 446.0, f, 22.0, c);
        x += text_width(s, f, 22.0);
    }

    // CMYK ramps (k/K operators) next to RGB hue ramp.
    d.text(p, "Device colour spaces: CMYK ramps (top) and RGB hues (bottom)", 48.0, 408.0, "Helvetica-Bold", 12.0, Pal::mint());
    let ramps: [(&str, fn(f64) -> Color); 4] = [
        ("C", |t| Color::CMYK(t, 0.0, 0.0, 0.0)),
        ("M", |t| Color::CMYK(0.0, t, 0.0, 0.0)),
        ("Y", |t| Color::CMYK(0.0, 0.0, t, 0.0)),
        ("K", |t| Color::CMYK(0.0, 0.0, 0.0, t)),
    ];
    for (r, (label, f)) in ramps.iter().enumerate() {
        let y = 372.0 - r as f64 * 24.0;
        d.text(p, label, 48.0, y + 5.0, "Helvetica-Bold", 10.0, Pal::ink());
        for i in 0..20 {
            d.rect(p, Rect::new(64.0 + i as f64 * 25.0, y, 24.0, 20.0), Some(f(i as f64 / 19.0)), None, 0.0);
        }
    }
    for i in 0..20 {
        d.rect(p, Rect::new(64.0 + i as f64 * 25.0, 262.0, 24.0, 20.0), Some(hsv(i as f64 / 20.0, 0.8, 0.95)), None, 0.0);
    }
    d.text(p, "RGB", 48.0, 267.0, "Helvetica-Bold", 10.0, Pal::ink());

    // Blend modes: subtractive (Multiply) on white, additive (Screen) on dark.
    d.text(p, "BlendMode::Multiply", 48.0, 226.0, "Helvetica-Bold", 11.0, Pal::ink());
    d.text(p, "BlendMode::Screen", 324.0, 226.0, "Helvetica-Bold", 11.0, Pal::ink());
    d.rect(p, Rect::new(48.0, 80.0, 240.0, 132.0), Some(rgb(1.0, 1.0, 1.0)), Some(Pal::rule()), 0.8);
    d.rect(p, Rect::new(324.0, 80.0, 240.0, 132.0), Some(Pal::night()), None, 0.0);
    let venn = |d: &mut Doc, cx: f64, cy: f64, cols: [Color; 3], mode: BlendMode| {
        for (c, (dx, dy)) in cols.into_iter().zip([(-24.0, 14.0), (24.0, 14.0), (0.0, -24.0)]) {
            d.path(p, circle(cx + dx, cy + dy, 40.0), Paint::fill(c).blend(mode));
        }
    };
    venn(d, 168.0, 140.0, [Color::CMYK(1.0, 0.0, 0.0, 0.0), Color::CMYK(0.0, 1.0, 0.0, 0.0), Color::CMYK(0.0, 0.0, 1.0, 0.0)], BlendMode::Multiply);
    venn(d, 444.0, 140.0, [rgb(1.0, 0.1, 0.1), rgb(0.1, 1.0, 0.2), rgb(0.15, 0.3, 1.0)], BlendMode::Screen);
    footer(d, p, false);
}

fn page_review(d: &mut Doc) -> Vec<Bookmark> {
    let p = d.add_page(PW, PH);
    background(d, p, Pal::paper());
    heading(d, p, "04  /  Annotations & Forms", "Review & Interact", false);

    // Log panel
    let log = [
        "INFO  engine boot, 0 plugins",
        "WARN  font fallback for U+2603",
        "INFO  page 3 rendered in 11 ms",
        "ERROR xref entry 42 repaired",
        "INFO  incremental save complete",
        "WARN  image downsampled to 150 dpi",
        "ERROR signature digest mismatch",
    ];
    let panel = Rect::new(48.0, 468.0, 330.0, 190.0);
    d.rect(p, panel, Some(rgb(0.07, 0.1, 0.13)), None, 0.0);
    let (mut errors, mut warns) = (0, 0);
    for (i, line) in log.iter().enumerate() {
        let y = panel.y + panel.height - 24.0 - i as f64 * 22.0;
        d.text(p, line, panel.x + 12.0, y, "Courier", 10.0, rgb(0.8, 0.88, 0.92));
        // Locate the leading level keyword (DocTextFinderMatch-style) and highlight it with a real annotation.
        for (kw, c) in [("ERROR", Pal::rose()), ("WARN", Pal::gold())] {
            if line.starts_with(kw) {
                if kw == "ERROR" {
                    errors += 1
                } else {
                    warns += 1
                }
                let w = text_width(kw, "Courier", 10.0);
                let bbox = Rect::new(panel.x + 12.0 - 1.0, y - 2.0, w + 2.0, 13.0);
                let mut a = Annotation::new(
                    bbox,
                    AnnotationSubtype::Highlight(HighlightAnnotation { quads: vec![Quad::from_rect(&bbox)], color: c.clone() }),
                );
                a.opacity = 0.85;
                a.contents = Some(format!("{kw} line found by text search"));
                a.author = Some("PdfCraft".into());
                d.annotate(p, a);
            }
        }
    }
    // Line annotation with arrow pointing at the failing line.
    let tip = pt(panel.x + panel.width - 8.0, panel.y + 17.0);
    let tail = pt(panel.x + panel.width + 44.0, panel.y - 18.0);
    d.path(
        p,
        {
            let mut l = Path::new();
            l.move_to(tail);
            l.line_to(tip);
            l
        },
        Paint::stroke(Pal::rose(), 1.6),
    );
    d.path(p, polygon(&[tip, pt(tip.x + 9.0, tip.y - 2.0), pt(tip.x + 3.0, tip.y - 9.0)]), Paint::fill(Pal::rose()));
    let line_rect = Rect::new(tip.x.min(tail.x), tip.y.min(tail.y), (tip.x - tail.x).abs(), (tip.y - tail.y).abs());
    d.annotate(
        p,
        Annotation::new(
            line_rect,
            AnnotationSubtype::Line(LineAnnotation {
                start_point: tail,
                end_point: tip,
                start_ending: LineEnding::None,
                end_ending: LineEnding::OpenArrow,
                line_width: 1.6,
            }),
        ),
    );
    d.text(p, "digest mismatch", tail.x - 66.0, tail.y - 12.0, "Helvetica-Bold", 8.0, Pal::rose());

    d.text(p, "Highlight annotations", 400.0, 636.0, "Helvetica-Bold", 12.0, Pal::mint());
    for (i, (label, n, c)) in [("ERROR", errors, Pal::rose()), ("WARN", warns, Pal::gold())].into_iter().enumerate() {
        let y = 570.0 - i as f64 * 62.0;
        d.rect(p, Rect::new(400.0, y, 164.0, 52.0), Some(rgb(1.0, 1.0, 1.0)), Some(c.clone()), 1.5);
        d.text(p, &n.to_string(), 414.0, y + 14.0, "Helvetica-Bold", 26.0, c);
        d.text(p, &format!("{label} markups"), 448.0, y + 20.0, "Helvetica", 9.5, Pal::ink());
    }

    // Link annotations: URI + GoTo (SDK Action enum)
    let links: [(&str, &str, Action, Color); 3] = [
        ("Project repo", "URI action", Action::URI(URIAction::new("https://github.com/richfrem/pdfcraft")), Pal::sky()),
        ("Back to cover", "GoTo action", Action::GoTo(GoToAction::new(ViewDestination::new(1, FitMode::Fit))), Pal::mint()),
        ("Engine report", "GoTo action", Action::GoTo(GoToAction::new(ViewDestination::new(5, FitMode::Fit))), Pal::ember()),
    ];
    for (i, (t, sub, act, c)) in links.into_iter().enumerate() {
        let r = Rect::new(48.0 + i as f64 * 176.0, 376.0, 164.0, 38.0);
        d.rect(p, r, Some(rgb(1.0, 1.0, 1.0)), Some(c.clone()), 1.5);
        d.text(p, t, r.x + 10.0, r.y + 22.0, "Helvetica-Bold", 10.5, c);
        d.text(p, sub, r.x + 10.0, r.y + 9.0, "Helvetica", 8.0, Pal::muted());
        d.annotate(p, Annotation::new(r, AnnotationSubtype::Link(LinkAnnotation { action: Some(act), destination: None })));
    }

    // Form card with AcroForm fields from SDK `Field`
    d.rect(p, Rect::new(48.0, 96.0, 516.0, 262.0), Some(rgb(1.0, 1.0, 1.0)), Some(Pal::rule()), 0.8);
    d.rect(p, Rect::new(48.0, 352.0, 516.0, 6.0), Some(Pal::sky()), None, 0.0);
    d.text(p, "Interactive AcroForm and an ink signature", 62.0, 330.0, "Helvetica-Bold", 12.0, Pal::sky());
    let lab = |d: &mut Doc, t: &str, x: f64, y: f64| d.text(p, t, x, y, "Helvetica-Bold", 9.5, rgb(0.2, 0.25, 0.3));
    lab(d, "Reviewer", 62.0, 304.0);
    let mut f = Field::new_text("reviewer", p, Rect::new(62.0, 274.0, 230.0, 26.0));
    f.value = Some("Rin Chen".into());
    d.field(f);
    lab(d, "Date", 308.0, 304.0);
    let mut f = Field::new_text("date", p, Rect::new(308.0, 274.0, 120.0, 26.0));
    f.value = Some("2026-10-10".into());
    d.field(f);
    lab(d, "Notes", 62.0, 252.0);
    let mut f = Field::new_text("notes", p, Rect::new(62.0, 222.0, 366.0, 26.0));
    f.value = Some("Ship after signature check is fixed.".into());
    d.field(f);
    for (i, (name, label, checked)) in [("approved", "Release approved", true), ("legal", "Needs legal review", false)].into_iter().enumerate() {
        let y = 304.0 - i as f64 * 52.0;
        lab(d, label, 460.0, y);
        let mut f = Field::new_text(name, p, Rect::new(460.0, y - 30.0, 22.0, 22.0));
        f.field_type = FieldType::Button(ButtonField { is_checkbox: true, checked, export_value: "Yes".into(), ..Default::default() });
        d.field(f);
    }
    // Ink annotation: a signature scribble (SDK InkAnnotation) drawn both as content and as an annotation.
    lab(d, "Signature", 62.0, 192.0);
    d.rect(p, Rect::new(62.0, 112.0, 230.0, 74.0), Some(rgb(0.98, 0.99, 1.0)), Some(Pal::rule()), 0.8);
    let stroke: Vec<Point> = (0..=60)
        .map(|i| {
            let t = i as f64 / 60.0;
            pt(76.0 + t * 200.0, 150.0 + 22.0 * (t * 17.0).sin() * (1.0 - t) + 8.0 * (t * 5.0).cos())
        })
        .collect();
    d.path(p, smooth_open(&stroke), Paint::stroke(rgb(0.1, 0.2, 0.55), 1.8));
    d.annotate(
        p,
        Annotation::new(Rect::new(70.0, 118.0, 214.0, 62.0), AnnotationSubtype::Ink(InkAnnotation { ink_list: vec![stroke], stroke_width: 1.8 })),
    );
    d.text(p, "Widgets are registered in /AcroForm; the check boxes carry on/off appearance streams.", 62.0, 102.0, "Helvetica", 8.0, Pal::muted());
    footer(d, p, false);

    let mut root = Bookmark::new("PdfCraft Rust SDK tour").with_destination(ViewDestination::new(1, FitMode::Fit));
    for (t, pg) in [("Paths & Matrices", 2), ("Type, CMYK, Blend", 3), ("Review & Interact", 4), ("Engine report", 5)] {
        root.add_child(Bookmark::new(t).with_destination(ViewDestination::new(pg, FitMode::Fit)));
    }
    vec![root]
}

fn page_engine(d: &mut Doc, rep: &EngineReport) {
    let p = d.add_page(PW, PH);
    background(d, p, Pal::night());
    d.rect(p, Rect::new(48.0, 716.0, 36.0, 5.0), Some(Pal::ember()), None, 0.0);
    d.text(p, "05  /  LOCALCLIENT", 48.0, 730.0, "Helvetica-Bold", 9.0, Pal::gold());
    d.text(p, "Engine Report", 48.0, 684.0, "Helvetica-Bold", 30.0, rgb(1.0, 1.0, 1.0));
    d.text(
        p,
        "Measured by driving this very document through the SDK's LocalClient (previous pass of the same build).",
        48.0,
        660.0,
        "Helvetica",
        9.5,
        rgb(0.6, 0.72, 0.74),
    );

    // Render timings bar chart
    d.text(p, "render_page(), ms per page", 48.0, 626.0, "Helvetica-Bold", 12.0, Pal::mint());
    d.rect(p, Rect::new(48.0, 470.0, 516.0, 142.0), Some(rgb(0.07, 0.11, 0.14)), None, 0.0);
    let max_ms = rep.renders.iter().map(|r| r.4).fold(1.0_f64, f64::max);
    for (i, (page, w, h, bytes, ms)) in rep.renders.iter().enumerate() {
        let x = 70.0 + i as f64 * 96.0;
        let bh = ms / max_ms * 90.0;
        d.rect(p, Rect::new(x, 492.0, 60.0, bh.max(2.0)), Some(hsv(0.42 + i as f64 * 0.09, 0.7, 0.95)), None, 0.0);
        d.text_centered(p, &format!("{ms:.0} ms"), x + 30.0, 496.0 + bh, "Helvetica-Bold", 8.5, rgb(1.0, 1.0, 1.0));
        d.text_centered(p, &format!("page {page}"), x + 30.0, 478.0, "Helvetica", 8.0, rgb(0.7, 0.78, 0.8));
        let _ = (w, h, bytes);
    }

    // Results table
    let mut rows: Vec<(String, String)> = Vec::new();
    for (page, w, h, bytes, _) in &rep.renders {
        rows.push((format!("render_page({page})"), format!("{w} x {h} px PNG, {:.1} KiB", *bytes as f64 / 1024.0)));
    }
    rows.push(("split(EveryNPages(1))".into(), format!("{} files in {:.0} ms", rep.split_files, rep.split_ms)));
    rows.push(("merge([doc, doc])".into(), format!("{} pages in {:.0} ms", rep.merge_pages, rep.merge_ms)));
    let rh = 19.0;
    for (i, (a, b)) in rows.iter().enumerate() {
        let y = 440.0 - i as f64 * rh;
        d.rect(p, Rect::new(48.0, y - 5.0, 516.0, rh), Some(if i % 2 == 0 { rgb(0.07, 0.11, 0.14) } else { rgb(0.05, 0.09, 0.11) }), None, 0.0);
        d.text(p, a, 58.0, y, "Courier", 9.5, Pal::mint());
        d.text(p, b, 250.0, y, "Helvetica", 9.5, rgb(0.88, 0.93, 0.94));
    }

    // Error taxonomy
    let ey = 440.0 - rows.len() as f64 * rh - 24.0;
    d.text(p, "SdkError taxonomy (RFC 7807 status mapping)", 48.0, ey, "Helvetica-Bold", 12.0, Pal::ember());
    for (i, (what, code, http)) in rep.errors.iter().enumerate() {
        let y = ey - 22.0 - i as f64 * 22.0;
        d.rect(p, Rect::new(48.0, y - 6.0, 516.0, 19.0), Some(rgb(0.1, 0.07, 0.07)), None, 0.0);
        d.text(p, what, 58.0, y, "Courier", 9.5, rgb(0.95, 0.8, 0.75));
        d.text(p, code, 236.0, y, "Courier-Bold", 9.5, Pal::gold());
        d.text(p, &format!("HTTP {http}"), 500.0, y, "Helvetica-Bold", 9.5, rgb(1.0, 1.0, 1.0));
    }
    if let Some(l) = &rep.limits {
        let y = ey - 22.0 - rep.errors.len() as f64 * 22.0 - 22.0;
        d.text(
            p,
            &format!(
                "ResourceLimits::default(): request {} MiB | output {} MiB | spill-to-disk above {} MiB",
                l.max_request_bytes / (1024 * 1024),
                l.max_output_bytes / (1024 * 1024),
                l.max_memory_bytes / (1024 * 1024)
            ),
            48.0,
            y,
            "Helvetica",
            9.0,
            rgb(0.6, 0.72, 0.74),
        );
    }
    footer(d, p, true);
}

fn build(rep: &EngineReport, counts: (usize, usize, usize, usize, usize)) -> Doc {
    let mut d = Doc { title: "PdfCraft Rust SDK Capability Tour".into(), ..Default::default() };
    page_cover(&mut d, counts);
    page_vector(&mut d);
    page_type_colour(&mut d);
    let bookmarks = page_review(&mut d);
    page_engine(&mut d, rep);
    d.bookmarks = bookmarks;
    d
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let output = flag("--output").map(PathBuf::from).unwrap_or_else(|| repo_root().join("sample-docs/outputs/sdk_capability_tour_rs.pdf"));

    // Pass 1: build with an empty report, then measure the engine on that document.
    let pass1 = build(&EngineReport::default(), (0, 0, 0, 0, 0));
    let counts = pass1.counts();
    let pass1_bytes = pass1.to_bytes()?;
    let (report, _) = measure_engine(&pass1_bytes, pass1.pages.len(), 110.0)?;

    // Pass 2: same layout, real numbers.
    let doc = build(&report, counts);
    let bytes = doc.to_bytes()?;
    if let Some(dir) = output.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&output, &bytes)?;
    println!("Saved {} ({:.1} KiB, {} pages)", output.display(), bytes.len() as f64 / 1024.0, doc.pages.len());

    if args.iter().any(|a| a == "--render-previews") {
        let client = LocalClient::default();
        let src = DocumentSource::from_bytes(bytes, Some("tour.pdf".into()));
        let dir = output.parent().map(|p| p.join("previews")).unwrap_or_else(|| PathBuf::from("previews"));
        fs::create_dir_all(&dir)?;
        let stem = output.file_stem().and_then(|s| s.to_str()).unwrap_or("tour");
        for page in 1..=doc.pages.len() {
            if let DocumentResult::Memory { data, .. } = client.render_page(&src, page, RenderOptions { dpi: 110.0, ..Default::default() })? {
                let path = dir.join(format!("{stem}_page_{page}.png"));
                fs::write(&path, data)?;
                println!("  preview: {}", path.display());
            }
        }
    }
    Ok(())
}
