//! Prepare a form: create, change and delete fields (execution plan M6.6).
//!
//! New fields follow Acrobat's conventions: names "Text1", "Check Box1", "Group1" (radio groups),
//! "Dropdown1", "List Box1", "Button1", "Signature1", "Date1"; 12 pt Helvetica (auto-size for
//! check boxes), a thin grey border and a white background, and appearance streams generated
//! immediately so every viewer shows the empty field.

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};
use pdfcraft_fonts::{helvetica_width, literal};

use crate::{Field, FieldKind, FormError, Widget, appearance, fields, flags, page_refs};

/// The kind of field to add.
#[derive(Clone, Debug, PartialEq)]
pub enum NewField {
    Text {
        multiline: bool,
    },
    /// A date field: a text field with Acrobat's date format action (`AFDate_FormatEx`).
    Date,
    CheckBox,
    /// One radio button in `group` (a new group is created when none of that name exists),
    /// selected by `export`.
    Radio {
        group: Option<String>,
        export: String,
    },
    Combo {
        options: Vec<String>,
        editable: bool,
    },
    List {
        options: Vec<String>,
        multi: bool,
    },
    Button {
        caption: String,
    },
    /// An image field: an icon-only push button that asks for a picture when clicked.
    Image,
    Signature,
}

impl NewField {
    fn base_name(&self) -> &'static str {
        match self {
            NewField::Text { .. } => "Text",
            NewField::Date => "Date",
            NewField::CheckBox => "Check Box",
            NewField::Radio { .. } => "Group",
            NewField::Combo { .. } => "Dropdown",
            NewField::List { .. } => "List Box",
            NewField::Button { .. } => "Button",
            NewField::Image => "Image",
            NewField::Signature => "Signature",
        }
    }
}

/// Field properties the Properties dialog edits (`None` leaves a value unchanged).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldProps {
    pub name: Option<String>,
    pub tooltip: Option<String>,
    pub read_only: Option<bool>,
    pub required: Option<bool>,
    pub multiline: Option<bool>,
    pub max_len: Option<Option<usize>>,
    /// Choice options (display texts; export values equal them).
    pub options: Option<Vec<String>>,
    /// Font size (0 = auto).
    pub font_size: Option<f64>,
    /// Position: move or resize one widget (its index in [`Field::widgets`]) to a new rect in
    /// user space.
    pub rect: Option<(usize, [f64; 4])>,
    /// Appearance tab (border, fill, line, text colour, font).
    pub look: Option<Look>,
    /// Change only these appearance properties. Unlike `look`, this preserves each widget's
    /// other colours, border settings and default appearance, including custom font resources.
    pub appearance: Option<LookPatch>,
    /// Format, Validate and Calculate tabs (written as Acrobat's AF scripts).
    pub format: Option<crate::af::Format>,
    pub validate: Option<crate::af::Validate>,
    pub calculate: Option<crate::af::Calculate>,
    /// Options-tab flags (`/Ff` bits from [`crate::flags`]) to set or clear.
    pub flags: Vec<(u32, bool)>,
    /// Alignment (`/Q`): 0 left, 1 centre, 2 right.
    pub quadding: Option<i64>,
    /// Default value (`/DV`): what Reset form restores.
    pub default_value: Option<Option<String>>,
    /// Lock or unlock the field (its widgets' Locked flag).
    pub locked: Option<bool>,
    /// Actions tab: every trigger's action (replacing the field's).
    pub actions: Option<Vec<(crate::Trigger, crate::FieldAction)>>,
    /// Options tab: the mark of a check box or radio button (every widget).
    pub check_style: Option<CheckStyle>,
    /// Position tab: rotate one widget (`/MK /R`) to 0, 90, 180 or 270 degrees counterclockwise.
    /// Changing between horizontal and vertical swaps that widget's width and height around its
    /// center, unless [`Self::rect`] is also set.
    pub rotation: Option<(usize, i64)>,
}

fn invalid<T>(m: impl Into<String>) -> Result<T, FormError> {
    Err(FormError::Invalid(m.into()))
}

fn rgb(c: [f64; 3]) -> Object {
    Object::Array(c.iter().map(|v| Object::Real(*v)).collect())
}

/// The AcroForm dictionary's reference, creating the form (with `/Helv` in `/DR`) if needed.
pub(crate) fn ensure_form(doc: &mut Document) -> Result<ObjRef, FormError> {
    let root = doc.root().ok_or(FormError::NoForm)?;
    let existing = doc.get(root).as_dict().and_then(|d| d.get(b"AcroForm").cloned());
    let r = match existing {
        Some(Object::Ref(r)) if doc.get(r).as_dict().is_some() => r,
        other => {
            let d = other.and_then(|o| o.as_dict().cloned()).unwrap_or_default();
            let r = doc.add(Object::Dict(d));
            doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Ref(r)))?;
            r
        }
    };
    let mut af = doc.get(r).as_dict().cloned().unwrap_or_default();
    if !af.contains(b"Fields") {
        af.set(b"Fields".to_vec(), Object::Array(Vec::new()));
    }
    if !af.contains(b"DA") {
        af.set(b"DA".to_vec(), PdfString::literal(b"/Helv 0 Tf 0 g".to_vec()));
    }
    let mut dr = af.get(b"DR").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    let mut fonts = dr.get(b"Font").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    // Acrobat's resource names for the standard fonts its Appearance tab offers.
    let mut added = false;
    for (name, base) in [("Helv", "Helvetica"), ("TiRo", "Times-Roman"), ("Cour", "Courier")] {
        if fonts.contains(name.as_bytes()) {
            continue;
        }
        let mut f = Dict::new();
        f.set(b"Type".to_vec(), Object::name("Font"));
        f.set(b"Subtype".to_vec(), Object::name("Type1"));
        f.set(b"BaseFont".to_vec(), Object::name(base));
        f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        fonts.set(name.as_bytes().to_vec(), Object::Dict(f));
        added = true;
    }
    if added {
        dr.set(b"Font".to_vec(), Object::Dict(fonts));
        af.set(b"DR".to_vec(), Object::Dict(dr));
    }
    doc.set(r, Object::Dict(af));
    Ok(r)
}

/// The Appearance tab: borders, colours and text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub border: Option<[f64; 3]>,
    pub fill: Option<[f64; 3]>,
    /// Line thickness in points (Acrobat: thin 1, medium 2, thick 3).
    pub width: f64,
    pub style: BorderStyle,
    pub text: [f64; 3],
    pub font: FieldFont,
}

/// A partial Appearance-tab edit. Outer `None` keeps a property; `Some(None)` removes a
/// border or fill colour. The patch is applied to each widget's own dictionaries.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LookPatch {
    pub border: Option<Option<[f64; 3]>>,
    pub fill: Option<Option<[f64; 3]>>,
    pub width: Option<f64>,
    pub style: Option<BorderStyle>,
    pub text: Option<[f64; 3]>,
    pub font: Option<FieldFont>,
}

impl LookPatch {
    fn validate(&self) -> Result<(), FormError> {
        for c in self.border.flatten().into_iter().chain(self.fill.flatten()).chain(self.text) {
            if !c.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) {
                return invalid("appearance colours must be finite RGB values between 0 and 1");
            }
        }
        if self.width.is_some_and(|v| !v.is_finite() || !(0.0..=12.0).contains(&v)) {
            return invalid("border width must be between 0 and 12 points");
        }
        Ok(())
    }
}

/// Rewrite only the selected operators in place: the last `Tf` (font and size) and the last
/// fill colour (`g`, `rg` or `k`), appending one when the string has none. Everything else (a
/// custom font resource, colour space or other graphics state) is kept, and repeated edits don't
/// grow the string.
fn patch_da(da: &str, size: Option<f64>, patch: &LookPatch) -> String {
    if patch.font.is_none() && size.is_none() && patch.text.is_none() {
        return da.to_string();
    }
    let mut toks: Vec<String> = da.split_whitespace().map(str::to_string).collect();
    let numeric = |s: &str| s.parse::<f64>().is_ok();
    if patch.font.is_some() || size.is_some() {
        let old = appearance::parse_da(da);
        let font = patch.font.map_or_else(|| old.font.clone(), |f| f.resource().to_string());
        // `parse_da` reads sizes up to 300 points; keep the same bound when writing.
        let size = appearance::fmt(size.unwrap_or(old.size).clamp(0.0, 300.0));
        let at = toks.iter().enumerate().rev().find(|(i, t)| t.as_str() == "Tf" && *i >= 2).map(|(i, _)| i);
        match at {
            Some(i) => {
                if let Some(slot) = toks.get_mut(i - 2) {
                    *slot = format!("/{font}");
                }
                if let Some(slot) = toks.get_mut(i - 1) {
                    *slot = size;
                }
            }
            None => toks.extend([format!("/{font}"), size, "Tf".into()]),
        }
    }
    if let Some(c) = patch.text {
        let rgb = [appearance::fmt(c[0]), appearance::fmt(c[1]), appearance::fmt(c[2]), "rg".into()];
        let at = toks.iter().enumerate().rev().find_map(|(i, t)| {
            let n = match t.as_str() {
                "g" => 1,
                "rg" => 3,
                "k" => 4,
                _ => return None,
            };
            let start = i.checked_sub(n)?;
            toks.get(start..i).is_some_and(|ops| ops.iter().all(|o| numeric(o))).then_some((start, i))
        });
        match at {
            Some((start, end)) => {
                toks.splice(start..=end, rgb);
            }
            None => toks.extend(rgb),
        }
    }
    toks.join(" ")
}

fn patch_widget_look(doc: &mut Document, w: &Widget, patch: &LookPatch) -> Result<(), FormError> {
    let touches_mk = patch.border.is_some() || patch.fill.is_some();
    let touches_bs = patch.width.is_some() || patch.style.is_some();
    // A size- or font-only edit changes /DA, not the widget: leave it out of the save.
    if !touches_mk && !touches_bs {
        return Ok(());
    }
    let wd = doc.get(w.obj).as_dict().cloned().ok_or_else(|| FormError::Invalid("the widget is not a dictionary".into()))?;
    if touches_mk {
        let indirect = wd.get(b"MK").and_then(Object::as_ref);
        let mut mk = wd.get(b"MK").map(|m| doc.resolve(m)).and_then(|m| m.as_dict().cloned()).unwrap_or_default();
        for (key, change) in [(b"BC".as_slice(), patch.border), (b"BG".as_slice(), patch.fill)] {
            if let Some(change) = change {
                match change {
                    Some(c) => mk.set(key.to_vec(), rgb(c)),
                    None => {
                        mk.remove(key);
                    }
                }
            }
        }
        // A shared indirect /MK is updated where it lives, not copied into each widget.
        match indirect {
            Some(r) => doc.set(r, Object::Dict(mk)),
            None => doc.update_dict(w.obj, |d| d.set(b"MK".to_vec(), Object::Dict(mk)))?,
        }
    }
    if touches_bs {
        let indirect = wd.get(b"BS").and_then(Object::as_ref);
        let mut bs = wd.get(b"BS").map(|b| doc.resolve(b)).and_then(|b| b.as_dict().cloned()).unwrap_or_default();
        if let Some(width) = patch.width {
            bs.set(b"W".to_vec(), Object::Real(width));
        }
        if let Some(style) = patch.style {
            bs.set(b"S".to_vec(), Object::name(style.code()));
            if style == BorderStyle::Dashed && !bs.contains(b"D") {
                bs.set(b"D".to_vec(), Object::Array(vec![Object::Int(3)]));
            }
        }
        match indirect {
            Some(r) => doc.set(r, Object::Dict(bs)),
            None => doc.update_dict(w.obj, |d| d.set(b"BS".to_vec(), Object::Dict(bs)))?,
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorderStyle {
    Solid,
    Dashed,
    Beveled,
    Inset,
    Underline,
}

impl BorderStyle {
    pub const ALL: [BorderStyle; 5] = [BorderStyle::Solid, BorderStyle::Dashed, BorderStyle::Beveled, BorderStyle::Inset, BorderStyle::Underline];

    fn code(self) -> &'static str {
        match self {
            BorderStyle::Solid => "S",
            BorderStyle::Dashed => "D",
            BorderStyle::Beveled => "B",
            BorderStyle::Inset => "I",
            BorderStyle::Underline => "U",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BorderStyle::Solid => "Solid",
            BorderStyle::Dashed => "Dashed",
            BorderStyle::Beveled => "Beveled",
            BorderStyle::Inset => "Inset",
            BorderStyle::Underline => "Underline",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldFont {
    Helvetica,
    Times,
    Courier,
}

impl FieldFont {
    pub const ALL: [FieldFont; 3] = [FieldFont::Helvetica, FieldFont::Times, FieldFont::Courier];

    fn resource(self) -> &'static str {
        match self {
            FieldFont::Helvetica => "Helv",
            FieldFont::Times => "TiRo",
            FieldFont::Courier => "Cour",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FieldFont::Helvetica => "Helvetica",
            FieldFont::Times => "Times Roman",
            FieldFont::Courier => "Courier",
        }
    }
}

/// The mark a check box or radio button shows when on (Field Properties ▸ Options ▸ style).
/// Stored as `/MK /CA`, the ZapfDingbats character Acrobat uses for it; PdfCraft draws the
/// mark as paths, so no symbol font is needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckStyle {
    Check,
    Circle,
    Cross,
    Diamond,
    Square,
    Star,
}

impl CheckStyle {
    pub const ALL: [CheckStyle; 6] =
        [CheckStyle::Check, CheckStyle::Circle, CheckStyle::Cross, CheckStyle::Diamond, CheckStyle::Square, CheckStyle::Star];

    fn code(self) -> &'static str {
        match self {
            CheckStyle::Check => "4",
            CheckStyle::Circle => "l",
            CheckStyle::Cross => "8",
            CheckStyle::Diamond => "u",
            CheckStyle::Square => "n",
            CheckStyle::Star => "H",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CheckStyle::Check => "Check",
            CheckStyle::Circle => "Circle",
            CheckStyle::Cross => "Cross",
            CheckStyle::Diamond => "Diamond",
            CheckStyle::Square => "Square",
            CheckStyle::Star => "Star",
        }
    }

    /// A widget's style: its `/MK /CA`, else a check for check boxes and a circle for radios.
    pub(crate) fn of_widget(doc: &Document, wd: &Dict, kind: FieldKind) -> Self {
        let mk = wd.get(b"MK").and_then(|m| doc.resolve(m).as_dict().cloned()).unwrap_or_default();
        let ca = mk.get(b"CA").and_then(|c| doc.resolve(c).as_string().map(|s| s.to_text()));
        CheckStyle::ALL.into_iter().find(|s| ca.as_deref() == Some(s.code())).unwrap_or(if kind == FieldKind::Radio {
            CheckStyle::Circle
        } else {
            CheckStyle::Check
        })
    }
}

/// A check box's or radio button's style (from its first widget).
pub fn check_style(doc: &Document, f: &Field) -> CheckStyle {
    let wd = f.widgets.first().and_then(|w| doc.get(w.obj).as_dict().cloned()).unwrap_or_default();
    CheckStyle::of_widget(doc, &wd, f.kind)
}

fn rgb_of(doc: &Document, o: Option<&Object>) -> Option<[f64; 3]> {
    let v: Vec<f64> = doc.resolve(o?).as_array()?.iter().filter_map(Object::as_f64).collect();
    match v.len() {
        1 => Some([v[0]; 3]),
        3 => Some([v[0], v[1], v[2]]),
        4 => Some([(1.0 - v[0]) * (1.0 - v[3]), (1.0 - v[1]) * (1.0 - v[3]), (1.0 - v[2]) * (1.0 - v[3])]),
        _ => None,
    }
}

/// A field's current look (from its first widget and its default appearance).
pub fn look(doc: &Document, f: &Field) -> Look {
    let wd = f.widgets.first().and_then(|w| doc.get(w.obj).as_dict().cloned()).unwrap_or_default();
    let mk = wd.get(b"MK").and_then(|m| doc.resolve(m).as_dict().cloned()).unwrap_or_default();
    let bs = wd.get(b"BS").and_then(|b| doc.resolve(b).as_dict().cloned()).unwrap_or_default();
    let da = wd.get(b"DA").and_then(|o| doc.resolve(o).as_string().map(|s| s.to_text())).unwrap_or_else(|| f.da.clone());
    let parsed = appearance::parse_da(&da);
    let nums: Vec<f64> = parsed.color.split_whitespace().filter_map(|t| t.parse().ok()).collect();
    let text = match nums.len() {
        1 => [nums[0]; 3],
        3 => [nums[0], nums[1], nums[2]],
        4 => [(1.0 - nums[0]) * (1.0 - nums[3]), (1.0 - nums[1]) * (1.0 - nums[3]), (1.0 - nums[2]) * (1.0 - nums[3])],
        _ => [0.0; 3],
    };
    Look {
        border: rgb_of(doc, mk.get(b"BC")),
        fill: rgb_of(doc, mk.get(b"BG")),
        width: bs.get(b"W").and_then(|w| doc.resolve(w).as_f64()).unwrap_or(1.0),
        style: match bs.name(b"S") {
            Some(b"D") => BorderStyle::Dashed,
            Some(b"B") => BorderStyle::Beveled,
            Some(b"I") => BorderStyle::Inset,
            Some(b"U") => BorderStyle::Underline,
            _ => BorderStyle::Solid,
        },
        text,
        font: match parsed.font.as_str() {
            "TiRo" | "Times-Roman" => FieldFont::Times,
            "Cour" | "Courier" => FieldFont::Courier,
            _ => FieldFont::Helvetica,
        },
    }
}

/// A `/DA` string with the given font, size and colour.
fn da_string(font: FieldFont, size: f64, c: [f64; 3]) -> String {
    let f = crate::appearance::fmt;
    format!("/{} {} Tf {} {} {} rg", font.resource(), f(size.clamp(0.0, 100.0)), f(c[0]), f(c[1]), f(c[2]))
}

fn add_to_page(doc: &mut Document, page: ObjRef, widget: ObjRef) -> Result<(), FormError> {
    let existing = doc.get(page).as_dict().and_then(|d| d.get(b"Annots").cloned());
    match existing.as_ref().and_then(|o| o.as_ref()).filter(|r| doc.get(*r).as_array().is_some()) {
        Some(r) => {
            let mut a = doc.get(r).as_array().cloned().unwrap_or_default();
            a.push(Object::Ref(widget));
            doc.set(r, Object::Array(a));
        }
        None => {
            let mut a = existing.and_then(|o| o.as_array().cloned()).unwrap_or_default();
            a.push(Object::Ref(widget));
            doc.update_dict(page, |d| d.set(b"Annots".to_vec(), Object::Array(a)))?;
        }
    }
    Ok(())
}

fn add_to_fields(doc: &mut Document, af: ObjRef, field: ObjRef) -> Result<(), FormError> {
    let mut d = doc.get(af).as_dict().cloned().unwrap_or_default();
    let mut list = d.get(b"Fields").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
    list.push(Object::Ref(field));
    d.set(b"Fields".to_vec(), Object::Array(list));
    doc.set(af, Object::Dict(d));
    Ok(())
}

/// The first free "<base>N" name.
fn free_name(existing: &[Field], base: &str) -> String {
    (1..=u64::MAX)
        .map(|n| format!("{base}{n}"))
        .find(|c| !existing.iter().any(|f| f.name == *c || f.name.starts_with(&format!("{c}."))))
        .unwrap_or_else(|| base.to_string())
}

fn widget_dict(page: ObjRef, rect: [f64; 4]) -> Dict {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Annot"));
    d.set(b"Subtype".to_vec(), Object::name("Widget"));
    d.set(b"Rect".to_vec(), Object::Array(rect.iter().map(|v| Object::Real(*v)).collect()));
    d.set(b"P".to_vec(), Object::Ref(page));
    d.set(b"F".to_vec(), Object::Int(4));
    let mut mk = Dict::new();
    mk.set(b"BC".to_vec(), rgb([0.55, 0.55, 0.55]));
    mk.set(b"BG".to_vec(), rgb([1.0, 1.0, 1.0]));
    d.set(b"MK".to_vec(), Object::Dict(mk));
    let mut bs = Dict::new();
    bs.set(b"W".to_vec(), Object::Int(1));
    bs.set(b"S".to_vec(), Object::name("S"));
    d.set(b"BS".to_vec(), Object::Dict(bs));
    d
}

/// Add a field on `page` (0-based) in `rect` (user space). Returns its full name.
pub fn add_field(doc: &mut Document, page: usize, rect: [f64; 4], kind: &NewField, name: Option<&str>) -> Result<String, FormError> {
    let pages = page_refs(doc);
    let page_ref = *pages.get(page).ok_or_else(|| FormError::Invalid(format!("page {} does not exist", page + 1)))?;
    let rect = [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])];
    if !rect.iter().all(|v| v.is_finite()) || rect[2] - rect[0] < 4.0 || rect[3] - rect[1] < 4.0 {
        return invalid("the field is too small");
    }
    let existing = fields(doc);
    // A radio button names its group: a new group takes that name.
    let name = match (name, kind) {
        (None, NewField::Radio { group: Some(g), .. }) => Some(g.as_str()),
        _ => name,
    };
    let name = match name.map(str::trim) {
        Some(n) if !n.is_empty() => {
            if n.contains('.') {
                return invalid("field names can't contain a period");
            }
            n.to_string()
        }
        _ => free_name(&existing, kind.base_name()),
    };
    let radio_group = match kind {
        NewField::Radio { group, .. } => existing.iter().find(|f| f.kind == FieldKind::Radio && Some(&f.name) == group.as_ref()).cloned(),
        _ => None,
    };
    if radio_group.is_none() && existing.iter().any(|f| f.name == name) {
        return invalid(format!("a field named {name:?} already exists"));
    }
    let af = ensure_form(doc)?;
    let mut w = widget_dict(page_ref, rect);
    let field_name = match kind {
        NewField::Radio { group: Some(g), .. } if radio_group.is_some() => g.clone(),
        _ => name.clone(),
    };
    match kind {
        NewField::Text { multiline } => {
            w.set(b"FT".to_vec(), Object::name("Tx"));
            w.set(b"DA".to_vec(), PdfString::literal(b"/Helv 12 Tf 0 g".to_vec()));
            if *multiline {
                w.set(b"Ff".to_vec(), Object::Int(flags::MULTILINE as i64));
            }
        }
        NewField::Date => {
            w.set(b"FT".to_vec(), Object::name("Tx"));
            w.set(b"DA".to_vec(), PdfString::literal(b"/Helv 12 Tf 0 g".to_vec()));
            // Acrobat's format and keystroke actions for mm/dd/yyyy (run by its JavaScript engine).
            let js = |s: &str| {
                let mut a = Dict::new();
                a.set(b"S".to_vec(), Object::name("JavaScript"));
                a.set(b"JS".to_vec(), PdfString::literal(s.as_bytes().to_vec()));
                Object::Dict(a)
            };
            let mut aa = Dict::new();
            aa.set(b"F".to_vec(), js("AFDate_FormatEx(\"mm/dd/yyyy\");"));
            aa.set(b"K".to_vec(), js("AFDate_KeystrokeEx(\"mm/dd/yyyy\");"));
            w.set(b"AA".to_vec(), Object::Dict(aa));
        }
        NewField::CheckBox => {
            w.set(b"FT".to_vec(), Object::name("Btn"));
            w.set(b"V".to_vec(), Object::name("Off"));
            w.set(b"AS".to_vec(), Object::name("Off"));
            w.set(b"DA".to_vec(), PdfString::literal(b"/ZaDb 0 Tf 0 g".to_vec()));
        }
        NewField::Radio { export, .. } => {
            w.set(b"AS".to_vec(), Object::name("Off"));
            if export.trim().is_empty() || export == "Off" {
                return invalid("a radio button needs an export value other than Off");
            }
        }
        NewField::Combo { options, editable } | NewField::List { options, multi: editable } => {
            w.set(b"FT".to_vec(), Object::name("Ch"));
            w.set(b"DA".to_vec(), PdfString::literal(b"/Helv 12 Tf 0 g".to_vec()));
            let combo = matches!(kind, NewField::Combo { .. });
            let mut ff = 0;
            if combo {
                ff |= flags::COMBO;
                if *editable {
                    ff |= flags::EDIT;
                }
            } else if *editable {
                ff |= flags::MULTI_SELECT;
            }
            w.set(b"Ff".to_vec(), Object::Int(ff as i64));
            w.set(b"Opt".to_vec(), Object::Array(options.iter().map(|o| Object::String(PdfString::text(o))).collect()));
        }
        NewField::Button { caption } => {
            w.set(b"FT".to_vec(), Object::name("Btn"));
            w.set(b"Ff".to_vec(), Object::Int(flags::PUSH_BUTTON as i64));
            w.set(b"DA".to_vec(), PdfString::literal(b"/Helv 0 Tf 0 g".to_vec()));
            let mut mk = w.get(b"MK").and_then(|m| m.as_dict()).cloned().unwrap_or_default();
            mk.set(b"BG".to_vec(), rgb([0.86, 0.86, 0.86]));
            mk.set(b"CA".to_vec(), PdfString::text(caption));
            w.set(b"MK".to_vec(), Object::Dict(mk));
        }
        NewField::Image => {
            w.set(b"FT".to_vec(), Object::name("Btn"));
            w.set(b"Ff".to_vec(), Object::Int(flags::PUSH_BUTTON as i64));
            w.set(b"DA".to_vec(), PdfString::literal(b"/Helv 0 Tf 0 g".to_vec()));
            let mut mk = w.get(b"MK").and_then(|m| m.as_dict()).cloned().unwrap_or_default();
            // Icon only, no border or fill: the picture is the field.
            mk.remove(b"BC");
            mk.remove(b"BG");
            mk.set(b"TP".to_vec(), Object::Int(1));
            w.set(b"MK".to_vec(), Object::Dict(mk));
            let mut a = Dict::new();
            a.set(b"S".to_vec(), Object::name("JavaScript"));
            a.set(b"JS".to_vec(), PdfString::literal(b"event.target.buttonImportIcon();".to_vec()));
            w.set(b"A".to_vec(), Object::Dict(a));
        }
        NewField::Signature => {
            w.set(b"FT".to_vec(), Object::name("Sig"));
        }
    }
    let widget = doc.add(Object::Dict(w));
    match kind {
        NewField::Radio { export, .. } => {
            let group = match &radio_group {
                Some(g) => g.obj,
                None => {
                    let mut g = Dict::new();
                    g.set(b"FT".to_vec(), Object::name("Btn"));
                    g.set(b"Ff".to_vec(), Object::Int((flags::RADIO | flags::NO_TOGGLE_TO_OFF) as i64));
                    g.set(b"T".to_vec(), PdfString::text(&field_name));
                    g.set(b"V".to_vec(), Object::name("Off"));
                    g.set(b"DA".to_vec(), PdfString::literal(b"/ZaDb 0 Tf 0 g".to_vec()));
                    g.set(b"Kids".to_vec(), Object::Array(Vec::new()));
                    let g = doc.add(Object::Dict(g));
                    add_to_fields(doc, af, g)?;
                    g
                }
            };
            doc.update_dict(widget, |d| d.set(b"Parent".to_vec(), Object::Ref(group)))?;
            doc.update_dict(group, |d| {
                let mut kids = d.get(b"Kids").and_then(|k| k.as_array().cloned()).unwrap_or_default();
                kids.push(Object::Ref(widget));
                d.set(b"Kids".to_vec(), Object::Array(kids));
            })?;
            let w = Widget {
                obj: widget,
                page: Some(page),
                rect,
                on_state: Some(export.clone()),
                state: Some("Off".into()),
                tab: usize::MAX,
                locked: false,
                hidden: false,
                rotation: 0,
            };
            let ap = appearance::check_box_states(doc, &w, FieldKind::Radio, export);
            doc.update_dict(widget, |d| d.set(b"AP".to_vec(), Object::Dict(ap)))?;
        }
        _ => {
            doc.update_dict(widget, |d| d.set(b"T".to_vec(), PdfString::text(&name)))?;
            add_to_fields(doc, af, widget)?;
        }
    }
    add_to_page(doc, page_ref, widget)?;
    redraw_field(doc, &field_name)?;
    Ok(field_name)
}

/// Regenerate every widget appearance of a field from its current value and settings.
pub fn redraw_field(doc: &mut Document, name: &str) -> Result<(), FormError> {
    let f = fields(doc).into_iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?;
    for w in &f.widgets {
        let ap = match f.kind {
            FieldKind::Text | FieldKind::Combo | FieldKind::List => {
                let s = appearance::field_appearance(doc, &f, w, &f.value);
                let r = doc.add(Object::Stream(s));
                let mut d = Dict::new();
                d.set(b"N".to_vec(), Object::Ref(r));
                d
            }
            FieldKind::CheckBox | FieldKind::Radio => {
                let on = w.on_state.clone().unwrap_or_else(|| "Yes".into());
                appearance::check_box_states(doc, w, f.kind, &on)
            }
            FieldKind::PushButton => {
                let s = button_appearance(doc, w);
                let r = doc.add(Object::Stream(s));
                let mut d = Dict::new();
                d.set(b"N".to_vec(), Object::Ref(r));
                d
            }
            FieldKind::Signature => {
                let s = empty_box(doc, w);
                let r = doc.add(Object::Stream(s));
                let mut d = Dict::new();
                d.set(b"N".to_vec(), Object::Ref(r));
                d
            }
        };
        let existing = crate::merged_appearance(doc, w.obj, &ap);
        doc.update_dict(w.obj, |d| d.set(b"AP".to_vec(), Object::Dict(existing)))?;
    }
    Ok(())
}

fn frame_only(doc: &Document, w: &Widget) -> (String, crate::appearance::Placement) {
    let (page_w, page_h) = ((w.rect[2] - w.rect[0]).max(1.0), (w.rect[3] - w.rect[1]).max(1.0));
    let wobj = doc.get(w.obj);
    let wd = wobj.as_dict().cloned().unwrap_or_default();
    let place = crate::appearance::placement(crate::appearance::mk_rotation(doc, &wd), page_w, page_h);
    let (width, height) = (place.layout_w, place.layout_h);
    let mk = wd.get(b"MK").and_then(|m| m.as_dict()).cloned().unwrap_or_default();
    let col = |k: &[u8]| -> Option<String> {
        let v: Vec<f64> = mk.get(k)?.as_array()?.iter().filter_map(|x| x.as_f64()).collect();
        (v.len() == 3).then(|| format!("{} {} {}", crate::appearance::fmt(v[0]), crate::appearance::fmt(v[1]), crate::appearance::fmt(v[2])))
    };
    let mut c = String::new();
    if let Some(bg) = col(b"BG") {
        c.push_str(&format!("{bg} rg 0 0 {width:.3} {height:.3} re f\n"));
    }
    if let Some(bc) = col(b"BC") {
        c.push_str(&format!("{bc} RG 1 w 0.5 0.5 {:.3} {:.3} re S\n", width - 1.0, height - 1.0));
    }
    (c, place)
}

fn form_stream(place: crate::appearance::Placement, content: Vec<u8>, resources: Dict) -> Stream {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    crate::appearance::set_form_box(&mut d, place);
    d.set(b"Resources".to_vec(), Object::Dict(resources));
    Stream::flate(d, &content)
}

/// A push button: background, border, its icon (`/MK /I`, scaled to fit and centred) and its
/// centred caption (unless the layout is icon only, `/TP 1`).
fn button_appearance(doc: &Document, w: &Widget) -> Stream {
    let (mut c, place) = frame_only(doc, w);
    let (width, height) = (place.layout_w, place.layout_h);
    let wobj = doc.get(w.obj);
    let mk = wobj.as_dict().and_then(|d| d.get(b"MK")).map(|m| doc.resolve(m)).and_then(|m| m.as_dict().cloned()).unwrap_or_default();
    let caption = mk.get(b"CA").map(|c| doc.resolve(c)).and_then(|c| c.as_string().map(|s| s.to_text())).unwrap_or_default();
    let icon_only = mk.get(b"TP").and_then(|t| doc.resolve(t).as_int()) == Some(1);
    let mut content = std::mem::take(&mut c).into_bytes();
    let mut xobjects = Dict::new();
    if let Some(icon) = mk.get(b"I").and_then(Object::as_ref)
        && let Object::Stream(s) = &*doc.get(icon)
    {
        let bb: Vec<f64> = s.dict.get(b"BBox").and_then(|b| b.as_array().map(|a| a.iter().filter_map(Object::as_f64).collect())).unwrap_or_default();
        if let [x0, y0, x1, y1] = bb[..]
            && x1 > x0
            && y1 > y0
        {
            // Proportional, as large as fits (Acrobat's default icon placement), centred.
            let k = ((width - 2.0) / (x1 - x0)).min((height - 2.0) / (y1 - y0)).max(0.0);
            let (dx, dy) = ((width - (x1 - x0) * k) / 2.0 - x0 * k, (height - (y1 - y0) * k) / 2.0 - y0 * k);
            content.extend(format!("q {k:.6} 0 0 {k:.6} {dx:.3} {dy:.3} cm /Icon Do Q\n").bytes());
            xobjects.set(b"Icon".to_vec(), Object::Ref(icon));
        }
    }
    let mut encoder = crate::appearance::AppearanceEncoder::new();
    if !icon_only && !caption.is_empty() {
        let size = ((height - 4.0) * 0.6).clamp(4.0, 14.0);
        let tw = helvetica_width(&caption, size);
        content.extend(format!("BT /Helv {size:.2} Tf 0 g 1 0 0 1 {:.3} {:.3} Tm ", (width - tw) / 2.0, (height - size * 0.7) / 2.0).bytes());
        let encoded = encoder.encode(&caption);
        content.extend(literal(&encoded));
        content.extend_from_slice(b" Tj ET\n");
    }
    let font = crate::appearance::appearance_font("Helvetica", if encoder.diffs.is_empty() { None } else { Some(encoder.diffs) });
    let mut fonts = Dict::new();
    fonts.set(b"Helv".to_vec(), font);
    let mut res = Dict::new();
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    if !xobjects.is_empty() {
        res.set(b"XObject".to_vec(), Object::Dict(xobjects));
    }
    form_stream(place, content, res)
}

/// Give a push button (an image field) the picture `image` (an image XObject of `px` pixels):
/// it becomes the button's icon (`/MK /I`) and its appearance.
pub fn set_button_icon(doc: &mut Document, name: &str, image: ObjRef, px: (u32, u32)) -> Result<(), FormError> {
    let f = fields(doc).into_iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?;
    if f.kind != FieldKind::PushButton {
        return invalid(format!("{name} is not a button or image field"));
    }
    // The icon form draws the image in a box of its pixel size (keeping its aspect ratio).
    let (w, h) = (px.0.max(1) as f64, px.1.max(1) as f64);
    let mut xo = Dict::new();
    xo.set(b"Im0".to_vec(), Object::Ref(image));
    let mut res = Dict::new();
    res.set(b"XObject".to_vec(), Object::Dict(xo));
    let icon =
        doc.add(Object::Stream(form_stream(crate::appearance::placement(0, w, h), format!("q {w} 0 0 {h} 0 0 cm /Im0 Do Q").into_bytes(), res)));
    for wd in &f.widgets {
        let mut mk =
            doc.get(wd.obj).as_dict().and_then(|d| d.get(b"MK").map(|m| doc.resolve(m))).and_then(|m| m.as_dict().cloned()).unwrap_or_default();
        mk.set(b"I".to_vec(), Object::Ref(icon));
        if !mk.contains(b"TP") {
            mk.set(b"TP".to_vec(), Object::Int(1));
        }
        doc.update_dict(wd.obj, |d| d.set(b"MK".to_vec(), Object::Dict(mk)))?;
    }
    redraw_field(doc, name)
}

fn empty_box(doc: &Document, w: &Widget) -> Stream {
    let (c, place) = frame_only(doc, w);
    form_stream(place, c.into_bytes(), Dict::new())
}

/// Swap width and height, keeping the center. `None` when that is not a usable widget.
fn swapped_around_center(r: [f64; 4]) -> Option<[f64; 4]> {
    if !r.iter().all(|v| v.is_finite()) {
        return None;
    }
    let (x0, y0, x1, y1) = (r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3]));
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (nw, nh) = (y1 - y0, x1 - x0);
    let out = [cx - nw / 2.0, cy - nh / 2.0, cx + nw / 2.0, cy + nh / 2.0];
    (out.iter().all(|v| v.is_finite()) && out[2] - out[0] >= 4.0 && out[3] - out[1] >= 4.0).then_some(out)
}

/// Change a field's properties (General and Options tabs) and redraw it.
pub fn set_props(doc: &mut Document, name: &str, props: &FieldProps) -> Result<String, FormError> {
    if props.font_size.is_some_and(|size| !size.is_finite() || size < 0.0) {
        return invalid("font size must be a finite, non-negative number");
    }
    if let Some(patch) = &props.appearance {
        if props.look.is_some() {
            return invalid("give an appearance patch or a complete look, not both");
        }
        patch.validate()?;
    }
    let all = fields(doc);
    let f = all.iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?.clone();
    // Validated before anything is written, so a refused rotation leaves the document unchanged.
    // `Some(Some(rect))`: the widget's axis changes and it gets the swapped rectangle.
    let mut rotated_rect: Option<Option<[f64; 4]>> = None;
    if let Some((wi, rot)) = props.rotation {
        if !matches!(rot, 0 | 90 | 180 | 270) {
            return invalid("rotation must be 0, 90, 180 or 270 degrees");
        }
        let Some(w) = f.widgets.get(wi) else {
            return invalid(format!("{name} has no widget {}", wi.saturating_add(1)));
        };
        let axis_change = (w.rotation % 180 == 0) != (rot % 180 == 0);
        rotated_rect = Some(None);
        if axis_change && props.rect.is_none() {
            let Some(rect) = swapped_around_center(w.rect) else { return invalid("the field is too small") };
            rotated_rect = Some(Some(rect));
        }
    }
    // A locked field only takes unlocking (Acrobat greys its properties out).
    if f.locked() && props.locked != Some(false) && *props != (FieldProps { locked: props.locked, ..FieldProps::default() }) {
        return invalid(format!("{name} is locked: unlock it first"));
    }
    if let Some(lock) = props.locked {
        for w in &f.widgets {
            let flags = doc.get(w.obj).as_dict().and_then(|d| d.get(b"F").and_then(|v| doc.resolve(v).as_int())).unwrap_or(0);
            let v = if lock { flags | 128 } else { flags & !128 };
            doc.update_dict(w.obj, |d| d.set(b"F".to_vec(), Object::Int(v)))?;
        }
        if *props == (FieldProps { locked: props.locked, ..FieldProps::default() }) {
            return Ok(name.to_string());
        }
    }
    if let Some(acts) = &props.actions {
        crate::set_field_actions(doc, name, acts)?;
    }
    let mut new_name = name.to_string();
    if let Some(n) = props.name.as_deref().map(str::trim).filter(|n| *n != f.name) {
        if n.is_empty() || n.contains('.') {
            return invalid("field names can't be empty or contain a period");
        }
        if all.iter().any(|x| x.name == n) {
            return invalid(format!("a field named {n:?} already exists"));
        }
        // Rename the terminal part (the field's own /T).
        let prefix = f.name.rsplit_once('.').map(|(p, _)| format!("{p}.")).unwrap_or_default();
        new_name = format!("{prefix}{n}");
        doc.update_dict(f.obj, |d| d.set(b"T".to_vec(), PdfString::text(n)))?;
    }
    // The new default appearance when the size or the look changes.
    let size = props.font_size.unwrap_or_else(|| crate::appearance::parse_da(&f.da).size);
    let new_da = match &props.look {
        Some(l) => da_string(l.font, size, l.text),
        None => patch_da(&f.da, props.font_size, &props.appearance.unwrap_or_default()),
    };
    if props.look.is_some() || props.appearance.is_some_and(|p| p.font.is_some()) {
        ensure_form(doc)?;
    }
    let mut ff = f.flags;
    let mut set_flag = |flag: u32, on: Option<bool>| {
        if let Some(on) = on {
            if on {
                ff |= flag;
            } else {
                ff &= !flag;
            }
        }
    };
    set_flag(flags::READ_ONLY, props.read_only);
    set_flag(flags::REQUIRED, props.required);
    if f.kind == FieldKind::Text {
        set_flag(flags::MULTILINE, props.multiline);
    }
    for (flag, on) in &props.flags {
        set_flag(*flag, Some(*on));
    }
    // A comb spreads a fixed number of characters across the field: it needs a limit and
    // excludes multi-line, password, file selection and scrolling (ISO 32000-2 Table 229).
    if f.kind == FieldKind::Text && ff & flags::COMB != 0 {
        let limit = match props.max_len {
            Some(m) => m,
            None => f.max_len,
        };
        if limit.is_none_or(|m| m == 0) {
            return invalid("a comb of characters needs a character limit");
        }
        ff &= !(flags::MULTILINE | flags::PASSWORD | flags::FILE_SELECT);
    }
    if let Some(q) = props.quadding
        && !(0..=2).contains(&q)
    {
        return invalid("alignment is 0 (left), 1 (centre) or 2 (right)");
    }
    if props.options.as_ref().is_some_and(|o| o.is_empty()) && matches!(f.kind, FieldKind::Combo | FieldKind::List) {
        return invalid("a list needs at least one option");
    }
    if props.check_style.is_some() && !matches!(f.kind, FieldKind::CheckBox | FieldKind::Radio) {
        return invalid("only check boxes and radio buttons have a check style");
    }
    doc.update_dict(f.obj, |d| {
        d.set(b"Ff".to_vec(), Object::Int(ff as i64));
        if let Some(t) = &props.tooltip {
            if t.is_empty() {
                d.remove(b"TU");
            } else {
                d.set(b"TU".to_vec(), PdfString::text(t));
            }
        }
        if let Some(m) = props.max_len {
            match m {
                Some(m) => d.set(b"MaxLen".to_vec(), Object::Int(m as i64)),
                None => {
                    d.remove(b"MaxLen");
                }
            }
        }
        if let Some(opts) = &props.options {
            d.set(b"Opt".to_vec(), Object::Array(opts.iter().map(|o| Object::String(PdfString::text(o))).collect()));
        }
        // "Sort items": the list itself is kept in order (export values stay with their items).
        if ff & flags::SORT != 0 && matches!(f.kind, FieldKind::Combo | FieldKind::List) {
            let mut items: Vec<(String, String)> = match &props.options {
                Some(o) => o.iter().map(|x| (x.clone(), x.clone())).collect(),
                None => f.options.clone(),
            };
            items.sort_by_key(|(_, display)| display.to_lowercase());
            let opt = items
                .iter()
                .map(|(e, disp)| {
                    if e == disp {
                        Object::String(PdfString::text(disp))
                    } else {
                        Object::Array(vec![Object::String(PdfString::text(e)), Object::String(PdfString::text(disp))])
                    }
                })
                .collect();
            d.set(b"Opt".to_vec(), Object::Array(opt));
        }
        if let Some(q) = props.quadding {
            d.set(b"Q".to_vec(), Object::Int(q));
        }
        match &props.default_value {
            // Buttons name their default state; text and choices hold a string.
            Some(Some(v)) if matches!(f.kind, FieldKind::CheckBox | FieldKind::Radio) => d.set(b"DV".to_vec(), Object::Name(crate::name_bytes(v))),
            Some(Some(v)) => d.set(b"DV".to_vec(), PdfString::text(v)),
            Some(None) => {
                d.remove(b"DV");
            }
            None => {}
        }
        if props.font_size.is_some() || props.look.is_some() || props.appearance.is_some_and(|p| p.font.is_some() || p.text.is_some()) {
            d.set(b"DA".to_vec(), PdfString::literal(new_da.clone().into_bytes()));
        }
    })?;
    if props.appearance.is_some() || (props.font_size.is_some() && props.look.is_none()) {
        let patch = &props.appearance.unwrap_or_default();
        for w in &f.widgets {
            patch_widget_look(doc, w, patch)?;
            // A widget's own /DA overrides its field's /DA. Patch that override separately
            // rather than replacing its font or colour with the first widget's defaults.
            if w.obj != f.obj && (props.font_size.is_some() || patch.font.is_some() || patch.text.is_some()) {
                let da = doc
                    .get(w.obj)
                    .as_dict()
                    .and_then(|d| d.get(b"DA").cloned())
                    .map(|v| doc.resolve(&v))
                    .and_then(|v| v.as_string().map(|s| s.to_text()));
                if let Some(da) = da {
                    let da = patch_da(&da, props.font_size, patch);
                    doc.update_dict(w.obj, |d| d.set(b"DA".to_vec(), PdfString::literal(da.into_bytes())))?;
                }
            }
        }
    }
    if let Some(l) = &props.look {
        let arr = |c: [f64; 3]| Object::Array(c.iter().map(|v| Object::Real(v.clamp(0.0, 1.0))).collect());
        for w in &f.widgets {
            doc.update_dict(w.obj, |d| {
                let mut mk = d.get(b"MK").and_then(|m| m.as_dict().cloned()).unwrap_or_default();
                match l.border {
                    Some(c) => mk.set(b"BC".to_vec(), arr(c)),
                    None => {
                        mk.remove(b"BC");
                    }
                }
                match l.fill {
                    Some(c) => mk.set(b"BG".to_vec(), arr(c)),
                    None => {
                        mk.remove(b"BG");
                    }
                }
                d.set(b"MK".to_vec(), Object::Dict(mk));
                let mut bs = Dict::new();
                bs.set(b"W".to_vec(), Object::Real(l.width.clamp(0.0, 12.0)));
                bs.set(b"S".to_vec(), Object::name(l.style.code()));
                if l.style == BorderStyle::Dashed {
                    bs.set(b"D".to_vec(), Object::Array(vec![Object::Int(3)]));
                }
                d.set(b"BS".to_vec(), Object::Dict(bs));
            })?;
        }
    }
    if let Some(style) = props.check_style {
        for w in &f.widgets {
            // Resolved first: an indirect /MK keeps its colours.
            let mut mk = doc.get(w.obj).as_dict().and_then(|d| d.get(b"MK").map(|m| doc.resolve(m).as_dict().cloned())).flatten().unwrap_or_default();
            mk.set(b"CA".to_vec(), PdfString::text(style.code()));
            doc.update_dict(w.obj, |d| d.set(b"MK".to_vec(), Object::Dict(mk)))?;
        }
    }
    if let Some((wi, r)) = props.rect {
        let w = f.widgets.get(wi).ok_or_else(|| FormError::Invalid(format!("{name} has no widget {}", wi + 1)))?;
        let r = [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])];
        if !r.iter().all(|v| v.is_finite()) || r[2] - r[0] < 4.0 || r[3] - r[1] < 4.0 {
            return invalid("the field is too small");
        }
        doc.update_dict(w.obj, |d| d.set(b"Rect".to_vec(), Object::Array(r.iter().map(|v| Object::Real(*v)).collect())))?;
    }
    if props.format.is_some() || props.validate.is_some() || props.calculate.is_some() {
        if !matches!(f.kind, FieldKind::Text | FieldKind::Combo) {
            return invalid("only text fields and drop-down lists have format, validate and calculate scripts");
        }
        let mut aa = doc
            .get(f.obj)
            .as_dict()
            .and_then(|d| d.get(b"AA").cloned())
            .map(|a| doc.resolve(&a))
            .and_then(|a| a.as_dict().cloned())
            .unwrap_or_default();
        let action = |js: &str| -> Object {
            let mut a = Dict::new();
            a.set(b"S".to_vec(), Object::name("JavaScript"));
            a.set(b"JS".to_vec(), PdfString::literal(js.as_bytes().to_vec()));
            Object::Dict(a)
        };
        if let Some(fm) = &props.format {
            aa.remove(b"F");
            aa.remove(b"K");
            if let Some((fjs, kjs)) = crate::af::format_js(fm) {
                if !fjs.is_empty() {
                    aa.set(b"F".to_vec(), action(&fjs));
                }
                aa.set(b"K".to_vec(), action(&kjs));
            }
        }
        if let Some(v) = &props.validate {
            aa.remove(b"V");
            if let Some(js) = crate::af::validate_js(v) {
                aa.set(b"V".to_vec(), action(&js));
            }
        }
        if let Some(c) = &props.calculate {
            aa.remove(b"C");
            if let Some(js) = crate::af::calculate_js(c) {
                aa.set(b"C".to_vec(), action(&js));
            }
            // The calculation order lists calculated fields.
            let af_ref = ensure_form(doc)?;
            let mut co: Vec<Object> = doc
                .get(af_ref)
                .as_dict()
                .and_then(|d| d.get(b"CO").cloned())
                .map(|c| doc.resolve(&c))
                .and_then(|c| c.as_array().cloned())
                .unwrap_or_default();
            co.retain(|o| o.as_ref() != Some(f.obj));
            if *c != crate::af::Calculate::None {
                co.push(Object::Ref(f.obj));
            }
            doc.update_dict(af_ref, |d| {
                if co.is_empty() {
                    d.remove(b"CO");
                } else {
                    d.set(b"CO".to_vec(), Object::Array(co));
                }
            })?;
        }
        doc.update_dict(f.obj, |d| {
            if aa.is_empty() {
                d.remove(b"AA");
            } else {
                d.set(b"AA".to_vec(), Object::Dict(aa));
            }
        })?;
    }
    // A complete look replaces widget /DA overrides. Partial changes were patched above
    // against each override, including size-only changes that keep its font and colour.
    if props.look.is_some() {
        for w in &f.widgets {
            if w.obj != f.obj {
                doc.update_dict(w.obj, |d| {
                    if d.contains(b"DA") {
                        d.set(b"DA".to_vec(), PdfString::literal(new_da.clone().into_bytes()));
                    }
                })?;
            }
        }
    }
    if let (Some((wi, rot)), Some(swapped)) = (props.rotation, rotated_rect)
        && let Some(w) = f.widgets.get(wi)
    {
        if let Some(rect) = swapped {
            doc.update_dict(w.obj, |d| d.set(b"Rect".to_vec(), Object::Array(rect.iter().map(|v| Object::Real(*v)).collect())))?;
        }
        let mut mk = doc.get(w.obj).as_dict().and_then(|d| d.get(b"MK").map(|m| doc.resolve(m).as_dict().cloned())).flatten().unwrap_or_default();
        if rot == 0 {
            mk.remove(b"R");
        } else {
            mk.set(b"R".to_vec(), Object::Int(rot));
        }
        doc.update_dict(w.obj, |d| {
            if mk.is_empty() {
                d.remove(b"MK");
            } else {
                d.set(b"MK".to_vec(), Object::Dict(mk));
            }
        })?;
    }
    redraw_field(doc, &new_name)?;
    if props.calculate.is_some() {
        crate::recalculate(doc)?;
    }
    Ok(new_name)
}

/// Delete a field: its widgets leave their pages and the field leaves the form.
pub fn delete_field(doc: &mut Document, name: &str) -> Result<(), FormError> {
    let all = fields(doc);
    let f = all.iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?.clone();
    let widgets: std::collections::HashSet<ObjRef> = f.widgets.iter().map(|w| w.obj).collect();
    for p in page_refs(doc) {
        let Some(a) = doc.get(p).as_dict().and_then(|d| d.get(b"Annots").cloned()) else { continue };
        let list = doc.resolve(&a).as_array().cloned().unwrap_or_default();
        let kept: Vec<Object> = list.iter().filter(|o| !o.as_ref().is_some_and(|r| widgets.contains(&r))).cloned().collect();
        if kept.len() != list.len() {
            match a.as_ref() {
                Some(r) if doc.get(r).as_array().is_some() => doc.set(r, Object::Array(kept)),
                _ => doc.update_dict(p, |d| d.set(b"Annots".to_vec(), Object::Array(kept)))?,
            }
        }
    }
    // Out of the field tree: from its parent's /Kids, or from /Fields.
    let parent = doc.get(f.obj).as_dict().and_then(|d| d.reference(b"Parent"));
    let remove_from = |doc: &mut Document, holder: ObjRef, key: &[u8]| -> Result<(), FormError> {
        let mut d = doc.get(holder).as_dict().cloned().unwrap_or_default();
        let list = d.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
        let kept: Vec<Object> = list.into_iter().filter(|o| o.as_ref() != Some(f.obj)).collect();
        d.set(key.to_vec(), Object::Array(kept));
        doc.set(holder, Object::Dict(d));
        Ok(())
    };
    match parent {
        Some(p) => remove_from(doc, p, b"Kids")?,
        None => {
            let root = doc.root().ok_or(FormError::NoForm)?;
            match doc.get(root).as_dict().and_then(|d| d.get(b"AcroForm").cloned()) {
                Some(Object::Ref(af)) => remove_from(doc, af, b"Fields")?,
                // A form dictionary held directly in the catalog.
                Some(Object::Dict(mut af)) => {
                    let list = af.get(b"Fields").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
                    let kept: Vec<Object> = list.into_iter().filter(|o| o.as_ref() != Some(f.obj)).collect();
                    af.set(b"Fields".to_vec(), Object::Array(kept));
                    doc.update_dict(root, |d| d.set(b"AcroForm".to_vec(), Object::Dict(af)))?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Widget-only keys: they stay with each widget when a merged field is split.
const WIDGET_KEYS: [&[u8]; 12] = [b"Type", b"Subtype", b"Rect", b"P", b"AP", b"AS", b"MK", b"F", b"BS", b"Border", b"H", b"StructParent"];

/// Duplicate a field onto other pages (Acrobat: right-click a field ▸ Duplicate): each page
/// gets a widget at the same place, belonging to the same field, so they share one value.
/// A field that is its own widget is first split into a field and a widget kid. Returns how
/// many widgets were added (pages that already have one are skipped).
pub fn duplicate_field(doc: &mut Document, name: &str, pages: &[usize]) -> Result<usize, FormError> {
    let f = fields(doc).into_iter().find(|f| f.name == name).ok_or_else(|| FormError::NoSuchField(name.into()))?;
    let model = f.widgets.first().cloned().ok_or_else(|| FormError::Invalid(format!("{name} has no widget")))?;
    let refs = page_refs(doc);
    for p in pages {
        if *p >= refs.len() {
            return invalid(format!("page {} does not exist", p + 1));
        }
    }
    let have: std::collections::HashSet<usize> = f.widgets.iter().filter_map(|w| w.page).collect();
    let targets: Vec<usize> = pages.iter().copied().filter(|p| !have.contains(p)).collect();
    if targets.is_empty() {
        return Ok(0);
    }
    // A merged field/widget becomes a field with one widget kid.
    let mut model_ref = model.obj;
    if model.obj == f.obj {
        let d = doc.get(f.obj).as_dict().cloned().unwrap_or_default();
        let mut kid = Dict::new();
        for k in WIDGET_KEYS {
            if let Some(v) = d.get(k) {
                kid.set(k.to_vec(), v.clone());
            }
        }
        kid.set(b"Parent".to_vec(), Object::Ref(f.obj));
        let kr = doc.add(Object::Dict(kid));
        doc.update_dict(f.obj, |d| {
            for k in WIDGET_KEYS {
                d.remove(k);
            }
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(kr)]));
        })?;
        // The page lists the widget now, not the field.
        if let Some(p) = model.page {
            let pr = refs[p];
            let annots = doc.get(pr).as_dict().and_then(|d| d.get(b"Annots").cloned());
            let swap =
                |a: Vec<Object>| -> Vec<Object> { a.into_iter().map(|o| if o.as_ref() == Some(f.obj) { Object::Ref(kr) } else { o }).collect() };
            match annots {
                Some(Object::Ref(ar)) => {
                    let a = doc.get(ar).as_array().cloned().unwrap_or_default();
                    doc.set(ar, Object::Array(swap(a)));
                }
                Some(Object::Array(a)) => doc.update_dict(pr, |d| d.set(b"Annots".to_vec(), Object::Array(swap(a))))?,
                _ => {}
            }
        }
        model_ref = kr;
    }
    let template = doc.get(model_ref).as_dict().cloned().unwrap_or_default();
    let mut added = Vec::new();
    for p in &targets {
        let pr = refs[*p];
        let mut w = template.clone();
        w.set(b"P".to_vec(), Object::Ref(pr));
        w.set(b"Parent".to_vec(), Object::Ref(f.obj));
        w.remove(b"StructParent");
        let wr = doc.add(Object::Dict(w));
        let annots = doc.get(pr).as_dict().and_then(|d| d.get(b"Annots").cloned());
        match annots {
            Some(Object::Ref(ar)) => {
                let mut a = doc.get(ar).as_array().cloned().unwrap_or_default();
                a.push(Object::Ref(wr));
                doc.set(ar, Object::Array(a));
            }
            other => {
                let mut a = other.and_then(|o| o.as_array().cloned()).unwrap_or_default();
                a.push(Object::Ref(wr));
                doc.update_dict(pr, |d| d.set(b"Annots".to_vec(), Object::Array(a)))?;
            }
        }
        added.push(wr);
    }
    doc.update_dict(f.obj, |d| {
        let mut kids = d.get(b"Kids").and_then(|k| k.as_array().cloned()).unwrap_or_default();
        kids.extend(added.iter().map(|r| Object::Ref(*r)));
        d.set(b"Kids".to_vec(), Object::Array(kids));
    })?;
    redraw_field(doc, name)?;
    Ok(added.len())
}
