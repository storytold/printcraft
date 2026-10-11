//! Field appearances (§12.7.4.3 and §12.5.5), execution plan M6.2.
//!
//! Text and choice fields are drawn from their `/DA` (font, size, colour) and the widget's `/MK`
//! (background and border colours) and `/BS` (border width and style), inside a `/Tx BMC … EMC`
//! marked-content block as viewers expect. Supported: single-line, multiline (wrapped), comb,
//! password, quadding, auto font size (`0 Tf`), combo boxes and list boxes (selection shown).
//!
//! The `/DA` font is used when the form's `/DR` defines it as a simple font (text encoded in
//! WinAnsi) or as a composite font with a predefined Unicode CMap such as `UniJIS-UTF16-H`, as
//! Japanese forms use (text encoded in that CMap). Otherwise (other composite fonts, missing
//! resources) Helvetica is used, so text is always visible. Widths use the approximate metrics
//! of `pdfcraft-fonts` (Helvetica, or one em per full-width character in composite fonts).
//!
//! `/MK /R` (0, 90, 180, 270, counterclockwise) lays the content out in the upright box and sets
//! the appearance `/Matrix` so that box lands on the widget rectangle. Any other value is 0, and
//! rotation 0 leaves `/Matrix` off.

use pdfcraft_cos::{Dict, Document, Object, Stream};
use pdfcraft_fonts::{UnicodeCMap, cjk_width, helvetica_width, literal, win_ansi, wrap_with};

use crate::{Field, FieldKind, Widget, acroform, flags};

/// Format a number for content streams.
fn n(v: f64) -> String {
    let s = format!("{:.3}", if v.abs() < 5e-4 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// Format a number for content streams (up to three decimals).
pub fn fmt(v: f64) -> String {
    n(v)
}

/// `/MK /R` as a quadrant, or 0 when it is missing or not a multiple of 90.
pub(crate) fn mk_rotation(doc: &Document, wd: &Dict) -> i64 {
    let Some(raw) = wd.get(b"MK") else { return 0 };
    let mk_obj = doc.resolve(raw);
    let Some(mk) = mk_obj.as_dict() else { return 0 };
    let Some(r_obj) = mk.get(b"R") else { return 0 };
    let resolved = doc.resolve(r_obj);
    let Some(r) = degrees(&resolved) else { return 0 };
    match r.rem_euclid(360) {
        q @ (0 | 90 | 180 | 270) => q,
        _ => 0,
    }
}

fn degrees(o: &Object) -> Option<i64> {
    if let Some(i) = o.as_int() {
        return Some(i);
    }
    let v = o.as_f64()?;
    (v.is_finite() && v.abs() <= 1.0e6).then_some(v.round() as i64)
}

/// Where an appearance is drawn: the upright layout box, and the `/Matrix` that turns it onto
/// the widget rectangle. Rotation 0 has no matrix.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Placement {
    pub layout_w: f64,
    pub layout_h: f64,
    pub matrix: Option<[f64; 6]>,
}

pub(crate) fn placement(rotation: i64, page_w: f64, page_h: f64) -> Placement {
    let (layout_w, layout_h, matrix) = match rotation {
        // (x, y) -> (page_w - y, x): text that ran to the right runs upward.
        90 => (page_h, page_w, Some([0.0, 1.0, -1.0, 0.0, page_w, 0.0])),
        180 => (page_w, page_h, Some([-1.0, 0.0, 0.0, -1.0, page_w, page_h])),
        // (x, y) -> (y, page_h - x): text that ran to the right runs downward.
        270 => (page_h, page_w, Some([0.0, -1.0, 1.0, 0.0, 0.0, page_h])),
        _ => (page_w, page_h, None),
    };
    Placement { layout_w, layout_h, matrix }
}

pub(crate) fn set_form_box(d: &mut Dict, place: Placement) {
    let box_ = [0.0, 0.0, place.layout_w, place.layout_h];
    d.set(b"BBox".to_vec(), Object::Array(box_.iter().map(|v| Object::Real(*v)).collect()));
    if let Some(m) = place.matrix {
        d.set(b"Matrix".to_vec(), Object::Array(m.iter().map(|v| Object::Real(*v)).collect()));
    }
}

/// A parsed default appearance string: font resource name, size (0 = auto) and colour operator.
#[derive(Clone, Debug, PartialEq)]
pub struct Da {
    pub font: String,
    pub size: f64,
    /// The colour operation as written (`0 g`, `1 0 0 rg`, `0 0 0 1 k`).
    pub color: String,
}

pub fn parse_da(da: &str) -> Da {
    let toks: Vec<&str> = da.split_whitespace().collect();
    let mut out = Da { font: "Helv".into(), size: 0.0, color: "0 g".into() };
    for (i, t) in toks.iter().enumerate() {
        match *t {
            "Tf" if i >= 2 => {
                out.font = toks[i - 2].trim_start_matches('/').to_string();
                out.size = toks[i - 1].parse::<f64>().ok().filter(|s| s.is_finite() && *s >= 0.0).unwrap_or(0.0).min(300.0);
            }
            "g" if i >= 1 && toks[i - 1].parse::<f64>().is_ok() => out.color = format!("{} g", toks[i - 1]),
            "rg" if i >= 3 && toks[i - 3..i].iter().all(|x| x.parse::<f64>().is_ok()) => out.color = format!("{} rg", toks[i - 3..i].join(" ")),
            "k" if i >= 4 && toks[i - 4..i].iter().all(|x| x.parse::<f64>().is_ok()) => out.color = format!("{} k", toks[i - 4..i].join(" ")),
            _ => {}
        }
    }
    out
}

/// How a field font's text is encoded and measured.
#[derive(Clone, Copy, Debug, PartialEq)]
enum FontText {
    WinAnsi,
    /// A composite font with a predefined Unicode CMap.
    Unicode(UnicodeCMap),
}

impl FontText {
    fn encode(self, s: &str) -> Vec<u8> {
        match self {
            Self::WinAnsi => win_ansi(s),
            Self::Unicode(cmap) => cmap.encode(s),
        }
    }

    fn width(self, s: &str, size: f64) -> f64 {
        match self {
            Self::WinAnsi => helvetica_width(s, size),
            Self::Unicode(_) => cjk_width(s, size),
        }
    }
}

/// The font resource for `name` from the form's `/DR`, if it is a font we can encode for.
fn dr_font(doc: &Document, name: &str) -> Option<(Object, FontText)> {
    let af = acroform(doc)?;
    let dr = doc.resolve(af.get(b"DR")?);
    let fonts = doc.resolve(dr.as_dict()?.get(b"Font")?);
    let entry = fonts.as_dict()?.get(name.as_bytes())?.clone();
    let font = doc.resolve(&entry);
    let fd = font.as_dict()?;
    match fd.name(b"Subtype") {
        Some(b"Type1" | b"TrueType" | b"MMType1") => {}
        // Japanese, Chinese and Korean forms: a CID font addressed by Unicode (a non-embedded
        // `/HeiseiMin-W3` with `/UniJIS-UCS2-H`, say). Embedded CMap streams and Identity-H need
        // the font's own mapping, so those still fall back to Helvetica.
        Some(b"Type0") => return Some((entry, FontText::Unicode(UnicodeCMap::from_name(fd.name(b"Encoding")?)?))),
        _ => return None,
    }
    // Symbolic fonts (ZapfDingbats, Symbol) can't show WinAnsi text.
    if matches!(fd.name(b"BaseFont"), Some(b"ZapfDingbats" | b"Symbol")) {
        return None;
    }
    Some((entry, FontText::WinAnsi))
}

pub(crate) fn appearance_font(base_font: &str, diffs: Option<Vec<Object>>) -> Object {
    let mut f = Dict::new();
    f.set(b"Type".to_vec(), Object::name("Font"));
    f.set(b"Subtype".to_vec(), Object::name("Type1"));
    f.set(b"BaseFont".to_vec(), Object::name(base_font));
    if let Some(diffs) = diffs {
        let mut enc = Dict::new();
        enc.set(b"Type".to_vec(), Object::name("Encoding"));
        enc.set(b"BaseEncoding".to_vec(), Object::name("WinAnsiEncoding"));
        enc.set(b"Differences".to_vec(), Object::Array(diffs));
        f.set(b"Encoding".to_vec(), Object::Dict(enc));
    } else {
        f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    }
    Object::Dict(f)
}

fn color_array(doc: &Document, mk: Option<&Dict>, key: &[u8]) -> Option<String> {
    let a = doc.resolve(mk?.get(key)?);
    let v: Vec<f64> = a.as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
    match v.as_slice() {
        [g] => Some(format!("{} g", n(*g))),
        [r, g, b] => Some(format!("{} {} {} rg", n(*r), n(*g), n(*b))),
        [c, m, y, k] => Some(format!("{} {} {} {} k", n(*c), n(*m), n(*y), n(*k))),
        _ => None,
    }
}

fn stroke_op(fill: &str) -> String {
    match fill.rsplit_once(' ') {
        Some((nums, "g")) => format!("{nums} G"),
        Some((nums, "rg")) => format!("{nums} RG"),
        Some((nums, "k")) => format!("{nums} K"),
        _ => "0 G".into(),
    }
}

/// Background and border from `/MK` and `/BS`; returns (content, border width).
fn frame(doc: &Document, wd: &Dict, w: f64, h: f64) -> (String, f64) {
    let mk = wd.get(b"MK").map(|m| doc.resolve(m)).and_then(|m| m.as_dict().cloned());
    let mut c = String::new();
    if let Some(bg) = color_array(doc, mk.as_ref(), b"BG") {
        c.push_str(&format!("{bg}\n0 0 {} {} re f\n", n(w), n(h)));
    }
    let bs = wd.get(b"BS").map(|b| doc.resolve(b)).and_then(|b| b.as_dict().cloned());
    let bw = bs.as_ref().and_then(|b| b.get(b"W")).and_then(|x| doc.resolve(x).as_f64()).unwrap_or(1.0).max(0.0);
    if let Some(bc) = color_array(doc, mk.as_ref(), b"BC")
        && bw > 0.0
    {
        let style = bs.as_ref().and_then(|b| b.name(b"S").map(<[u8]>::to_vec)).unwrap_or_default();
        if style == b"U" {
            // Underline: only the bottom edge.
            c.push_str(&format!("{}\n{} w\n0 {} m {} {} l S\n", stroke_op(&bc), n(bw), n(bw / 2.0), n(w), n(bw / 2.0)));
            return (c, bw);
        }
        let dashed = style == b"D";
        c.push_str(&format!(
            "{}\n{} w\n{}{} {} {} {} re S\n[] 0 d\n",
            stroke_op(&bc),
            n(bw),
            if dashed { "[3] 0 d\n" } else { "" },
            n(bw / 2.0),
            n(bw / 2.0),
            n(w - bw),
            n(h - bw)
        ));
        if style == b"B" || style == b"I" {
            // Beveled: a light top-left and a darker bottom-right inside the border; inset:
            // grey top-left and light grey bottom-right.
            let (tl, br) = if style == b"B" { ("1 g", "0.5 g") } else { ("0.5 g", "0.75 g") };
            let (x0, y0, x1, y1) = (bw, bw, w - bw, h - bw);
            let k = bw;
            c.push_str(&format!(
                "{tl}\n{} {} m {} {} l {} {} l {} {} l {} {} l {} {} l f\n{br}\n{} {} m {} {} l {} {} l {} {} l {} {} l {} {} l f\n",
                n(x0),
                n(y0),
                n(x0),
                n(y1),
                n(x1),
                n(y1),
                n(x1 - k),
                n(y1 - k),
                n(x0 + k),
                n(y1 - k),
                n(x0 + k),
                n(y0 + k),
                n(x1),
                n(y1),
                n(x1),
                n(y0),
                n(x0),
                n(y0),
                n(x0 + k),
                n(y0 + k),
                n(x1 - k),
                n(y0 + k),
                n(x1 - k),
                n(y1 - k)
            ));
            return (c, 2.0 * bw);
        }
        return (c, bw);
    }
    (c, 0.0)
}

/// The appearance of a text or choice field's widget showing `values`.
pub fn field_appearance(doc: &Document, f: &Field, w: &Widget, values: &[String]) -> Stream {
    field_appearance_as(doc, f, w, values, true)
}

/// [`field_appearance`], where `format` false means `values` are already what to show (a
/// custom Format script ran).
pub fn field_appearance_as(doc: &Document, f: &Field, w: &Widget, values: &[String], format: bool) -> Stream {
    // The Format event: what is shown, not what is stored.
    let formatted: Vec<String>;
    let values = if format
        && matches!(f.kind, crate::FieldKind::Text | crate::FieldKind::Combo)
        && values.len() == 1
        && f.actions.format != crate::af::Format::None
    {
        formatted = vec![crate::af::format_value(&f.actions.format, &values[0])];
        &formatted[..]
    } else {
        values
    };
    let wobj = doc.get(w.obj);
    let wd = wobj.as_dict().cloned().unwrap_or_default();
    let (page_w, page_h) = ((w.rect[2] - w.rect[0]).max(1.0), (w.rect[3] - w.rect[1]).max(1.0));
    let place = placement(mk_rotation(doc, &wd), page_w, page_h);
    let (width, height) = (place.layout_w, place.layout_h);
    let da = parse_da(wd.get(b"DA").and_then(|o| doc.resolve(o).as_string().map(|s| s.to_text())).as_deref().unwrap_or(&f.da));
    let (font_name, font_obj, enc) = match dr_font(doc, &da.font) {
        Some((o, enc)) => (da.font.clone(), o, enc),
        None => ("Helv".to_string(), helvetica(), FontText::WinAnsi),
    };
    let width_of = |text: &str, size: f64| enc.width(text, size);
    let (mut c, bw) = frame(doc, &wd, width, height);
    let pad = 2.0 + bw;
    let inner_w = (width - 2.0 * pad).max(1.0);
    let q = wd.get(b"Q").and_then(|o| doc.resolve(o).as_int()).unwrap_or(f.quadding);
    let mut body: Vec<u8> = Vec::new();
    let mut encoder = AppearanceEncoder::new();
    let mut show = |body: &mut Vec<u8>, x: f64, y: f64, text: &str| {
        body.extend(format!("1 0 0 1 {} {} Tm ", n(x), n(y)).bytes());
        body.extend(literal(&enc.encode(text)));
        body.extend_from_slice(b" Tj\n");
    };
    let x_for = |text: &str, size: f64| -> f64 {
        let tw = width_of(text, size);
        match q {
            1 => pad + (inner_w - tw) / 2.0,
            2 => width - pad - tw,
            _ => pad,
        }
    };
    let mut size = da.size;
    match f.kind {
        FieldKind::List => {
            // Every option, one per line from the top; selected ones highlighted.
            if size == 0.0 {
                size = 12.0;
            }
            let line = size * 1.15;
            let top = wd.get(b"TI").and_then(|o| doc.resolve(o).as_int()).unwrap_or(0).max(0) as usize;
            let mut y = height - pad;
            for (export, display) in f.options.iter().skip(top) {
                if y - line < 0.0 {
                    break;
                }
                if values.contains(export) {
                    c.push_str(&format!("0.6 0.75 0.86 rg\n{} {} {} {} re f\n", n(bw), n(y - line), n(width - 2.0 * bw), n(line)));
                }
                show(&mut body, pad, y - line + size * 0.25, display);
                y -= line;
            }
        }
        _ => {
            let text: String = if f.kind == FieldKind::Combo {
                values.iter().map(|v| f.options.iter().find(|(e, _)| e == v).map_or(v.clone(), |(_, d)| d.clone())).collect::<Vec<_>>().join(", ")
            } else {
                values.first().cloned().unwrap_or_default()
            };
            let text = if f.has(flags::PASSWORD) { "*".repeat(text.chars().count()) } else { text };
            let comb = f.has(flags::COMB) && !f.has(flags::MULTILINE) && !f.has(flags::PASSWORD) && f.max_len.is_some_and(|m| m > 0);
            if f.kind == FieldKind::Text && f.has(flags::MULTILINE) {
                if size == 0.0 {
                    // Auto size: the largest size (≤ 12) whose wrapped lines fit the height.
                    size = 12.0;
                    while size > 4.0 && wrap_with(&text, size, inner_w, width_of).len() as f64 * size * 1.15 > height - 2.0 * pad {
                        size -= 0.5;
                    }
                }
                let mut y = height - pad - size * 0.85;
                for line in wrap_with(&text, size, inner_w, width_of) {
                    if y < -size {
                        break;
                    }
                    show(&mut body, x_for(&line, size), y, &line);
                    y -= size * 1.15;
                }
            } else {
                if size == 0.0 {
                    size = ((height - 2.0 * pad) / 1.15).clamp(4.0, 12.0);
                    let tw = width_of(&text, size);
                    if tw > inner_w && !comb {
                        size = (size * inner_w / tw).max(4.0);
                    }
                }
                // Centre Helvetica's ascent (0.718) and descent (0.207) vertically.
                let y = (height - 0.925 * size) / 2.0 + 0.207 * size;
                if comb {
                    let cells = f.max_len.unwrap_or(1).max(1);
                    let cell = width / cells as f64;
                    for (i, ch) in text.chars().take(cells).enumerate() {
                        let s = ch.to_string();
                        show(&mut body, cell * i as f64 + (cell - width_of(&s, size)) / 2.0, y, &s);
                    }
                } else {
                    show(&mut body, x_for(&text, size), y, &text);
                }
            }
        }
    }
    let (font_name, font_obj) = if encoder.diffs.is_empty() {
        match dr_font(doc, Some(&wd), &da.font) {
            Some(o) => (da.font.clone(), o),
            None => ("Helv".to_string(), appearance_font("Helvetica", None)),
        }
    } else {
        let base_font = match da.font.to_ascii_lowercase().as_str() {
            s if s.contains("tiro") || s.contains("times") => "Times-Roman",
            s if s.contains("cour") || s.contains("mono") => "Courier",
            _ => "Helvetica",
        };
        let font_name = if da.font.is_empty() { "Helv".to_string() } else { da.font.clone() };
        (font_name, appearance_font(base_font, Some(encoder.diffs)))
    };
    let mut content = c.into_bytes();
    content.extend(
        format!(
            "/Tx BMC\nq\n{} {} {} {} re W n\nBT\n/{} {} Tf\n{}\n",
            n(bw),
            n(bw),
            n(width - 2.0 * bw),
            n(height - 2.0 * bw),
            font_name,
            n(size),
            da.color
        )
        .bytes(),
    );
    content.extend(body);
    content.extend_from_slice(b"ET\nQ\nEMC\n");
    let mut fonts = Dict::new();
    fonts.set(font_name.into_bytes(), font_obj);
    let mut res = Dict::new();
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    set_form_box(&mut d, place);
    d.set(b"Resources".to_vec(), Object::Dict(res));
    Stream::flate(d, &content)
}

/// On/Off appearances for a check box or radio button that has none (a check mark or a dot,
/// drawn as paths, so no symbol font is needed).
/// The streams are added to `doc` as indirect objects (streams can't be direct objects).
pub fn check_box_states(doc: &mut Document, w: &Widget, kind: FieldKind, on_name: &str) -> Dict {
    let wobj = doc.get(w.obj);
    let wd = wobj.as_dict().cloned().unwrap_or_default();
    let (page_w, page_h) = ((w.rect[2] - w.rect[0]).max(1.0), (w.rect[3] - w.rect[1]).max(1.0));
    let place = placement(mk_rotation(doc, &wd), page_w, page_h);
    let (width, height) = (place.layout_w, place.layout_h);
    let (frame_c, _) = frame(doc, &wd, width, height);
    let style = crate::author::CheckStyle::of_widget(doc, &wd, kind);
    let mut form = |content: String| -> Object {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("XObject"));
        d.set(b"Subtype".to_vec(), Object::name("Form"));
        set_form_box(&mut d, place);
        Object::Ref(doc.add(Object::Stream(Stream::flate(d, content.as_bytes()))))
    };
    let s = width.min(height);
    let mark = check_mark(style, width / 2.0, height / 2.0, s);
    let mut nd = Dict::new();
    nd.set(crate::name_bytes(on_name), form(format!("{frame_c}{mark}")));
    nd.set(b"Off".to_vec(), form(frame_c));
    let mut ap = Dict::new();
    ap.set(b"N".to_vec(), Object::Dict(nd));
    ap
}

/// The on-state mark for `style` (#94), centred on (`cx`, `cy`) in a box whose smaller side is
/// `s`, drawn in black.
fn check_mark(style: crate::author::CheckStyle, cx: f64, cy: f64, s: f64) -> String {
    use crate::author::CheckStyle;
    // A filled polygon through `pts`.
    let polygon = |pts: &[(f64, f64)]| {
        let mut c = String::from("0 g\n");
        for (i, (x, y)) in pts.iter().enumerate() {
            c.push_str(&format!("{} {} {} ", n(*x), n(*y), if i == 0 { "m" } else { "l" }));
        }
        c.push_str("h f\n");
        c
    };
    let line = n((s * 0.1).max(1.0));
    match style {
        CheckStyle::Check => format!(
            "0 G\n{line} w 1 J 1 j\n{} {} m {} {} l {} {} l S\n",
            n(cx - s * 0.28),
            n(cy),
            n(cx - s * 0.08),
            n(cy - s * 0.22),
            n(cx + s * 0.3),
            n(cy + s * 0.25)
        ),
        CheckStyle::Cross => {
            let d = s * 0.25;
            format!(
                "0 G\n{line} w 1 J\n{} {} m {} {} l S\n{} {} m {} {} l S\n",
                n(cx - d),
                n(cy - d),
                n(cx + d),
                n(cy + d),
                n(cx - d),
                n(cy + d),
                n(cx + d),
                n(cy - d)
            )
        }
        CheckStyle::Square => {
            let d = s * 0.22;
            format!("0 g\n{} {} {} {} re f\n", n(cx - d), n(cy - d), n(2.0 * d), n(2.0 * d))
        }
        CheckStyle::Diamond => {
            let d = s * 0.3;
            polygon(&[(cx, cy + d), (cx + d, cy), (cx, cy - d), (cx - d, cy)])
        }
        CheckStyle::Star => {
            // Five points: outer and inner radii alternate, the first point straight up.
            let (outer, inner) = (s * 0.32, s * 0.32 * 0.382);
            let pts: Vec<(f64, f64)> = (0..10)
                .map(|i| {
                    let a = std::f64::consts::FRAC_PI_2 + i as f64 * std::f64::consts::PI / 5.0;
                    let r = if i % 2 == 0 { outer } else { inner };
                    (cx + r * a.cos(), cy + r * a.sin())
                })
                .collect();
            polygon(&pts)
        }
        CheckStyle::Circle => {
            let r = s * 0.25;
            let k = 0.5523 * r;
            format!(
                "0 g\n{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c f\n",
                n(cx + r),
                n(cy),
                n(cx + r),
                n(cy + k),
                n(cx + k),
                n(cy + r),
                n(cx),
                n(cy + r),
                n(cx - k),
                n(cy + r),
                n(cx - r),
                n(cy + k),
                n(cx - r),
                n(cy),
                n(cx - r),
                n(cy - k),
                n(cx - k),
                n(cy - r),
                n(cx),
                n(cy - r),
                n(cx + k),
                n(cy - r),
                n(cx + r),
                n(cy - k),
                n(cx + r),
                n(cy)
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_appearance_strings_parse() {
        assert_eq!(parse_da("/Helv 0 Tf 0 g"), Da { font: "Helv".into(), size: 0.0, color: "0 g".into() });
        assert_eq!(parse_da("0.1 0.2 0.3 rg /F1 11 Tf"), Da { font: "F1".into(), size: 11.0, color: "0.1 0.2 0.3 rg".into() });
        assert_eq!(parse_da("/Cour 9 Tf 0 0 0 1 k").color, "0 0 0 1 k");
        assert_eq!(parse_da("garbage").font, "Helv");
        assert_eq!(stroke_op("1 0 0 rg"), "1 0 0 RG");
    }
}
