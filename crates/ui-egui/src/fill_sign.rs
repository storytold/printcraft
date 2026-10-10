//! Fill & Sign (Acrobat's Fill & Sign tool, execution plan M5.7): type text onto the page, place
//! ✓ ✕ ● ─ marks and today's date (in Preferences ▸ Date format), and sign with a drawn
//! signature. Everything is an annotation (typewriter text, PdfCraft-drawn stamps, ink), so it
//! can be moved, deleted and undone like any comment.

use egui::{Color32, CornerRadius, Pos2, Sense, Stroke, pos2, vec2};
use pdfcraft_engine::{Edit, FillMark, NewAnnotation, Shape, SignatureImage, Style};
use pdfcraft_render::{DocInfo, PageInfo};

use crate::canvas::{DocView, PageXform};
use crate::theme::Tokens;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FillTool {
    Text,
    Check,
    Cross,
    Dot,
    Line,
    Date,
    Signature,
    Initials,
}

pub const FILL_TOOLS: [FillTool; 8] =
    [FillTool::Text, FillTool::Cross, FillTool::Check, FillTool::Dot, FillTool::Line, FillTool::Date, FillTool::Signature, FillTool::Initials];

/// A saved signature or initials: drawn strokes (normalised to the pad width, y up) or typed
/// text (drawn in the script font), or a locally imported image.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SavedSig {
    Drawn(Vec<Vec<[f32; 2]>>),
    Typed(String),
    Image(#[serde(with = "image_data")] SignatureImage),
}

mod image_data {
    use base64::Engine as _;
    use pdfcraft_engine::{SignatureImage, signature_image::MAX_SIGNATURE_IMAGE_BYTES};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(image: &SignatureImage, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(image.bytes().as_slice()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SignatureImage, D::Error> {
        let encoded = String::deserialize(d)?;
        if encoded.len() > MAX_SIGNATURE_IMAGE_BYTES.div_ceil(3) * 4 {
            return Err(serde::de::Error::custom("signature image exceeds 4 MiB"));
        }
        let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(serde::de::Error::custom)?;
        SignatureImage::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

/// The Create signature / initials dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SigDraft {
    pub strokes: Vec<Vec<[f32; 2]>>,
    pub text: String,
    /// The Draw tab (otherwise Type).
    pub drawing: bool,
    /// The Image tab. The file is held in the draft until Apply.
    pub image_mode: bool,
    pub image: Option<SignatureImage>,
    /// Creating initials (otherwise the signature).
    pub initials: bool,
    /// Replacing an existing saved value.
    pub editing: bool,
}

impl SigDraft {
    pub fn new(initials: bool, name: &str) -> Self {
        let text = if initials { name.split_whitespace().filter_map(|w| w.chars().next()).collect() } else { name.trim().to_string() };
        Self { text, initials, ..Default::default() }
    }

    /// Edit a copy of the saved value; Cancel must leave the original untouched.
    pub fn from_saved(initials: bool, saved: &SavedSig) -> Self {
        match saved {
            SavedSig::Drawn(strokes) => Self { strokes: strokes.clone(), drawing: true, initials, editing: true, ..Default::default() },
            SavedSig::Typed(text) => Self { text: text.clone(), initials, editing: true, ..Default::default() },
            SavedSig::Image(image) => Self { image: Some(image.clone()), image_mode: true, initials, editing: true, ..Default::default() },
        }
    }

    fn ready(&self) -> bool {
        if self.image_mode {
            self.image.is_some()
        } else if self.drawing {
            self.strokes.iter().any(|s| s.len() > 1)
        } else {
            !self.text.trim().is_empty()
                && self.text.chars().take(pdfcraft_engine::MAX_SIGNATURE_CHARS + 1).count() <= pdfcraft_engine::MAX_SIGNATURE_CHARS
        }
    }

    pub fn saved(&self) -> Option<SavedSig> {
        if self.image_mode {
            self.image.clone().map(SavedSig::Image)
        } else if self.drawing {
            Some(SavedSig::Drawn(self.strokes.clone()))
        } else {
            Some(SavedSig::Typed(self.text.trim().to_string()))
        }
    }
}

impl FillTool {
    pub fn command(self) -> &'static str {
        match self {
            FillTool::Text => "sign.fill.text",
            FillTool::Check => "sign.fill.check",
            FillTool::Cross => "sign.fill.cross",
            FillTool::Dot => "sign.fill.dot",
            FillTool::Line => "sign.fill.line",
            FillTool::Date => "sign.fill.date",
            FillTool::Signature => "sign.fill.signature",
            FillTool::Initials => "sign.fill.initials",
        }
    }

    pub fn from_command(id: &str) -> Option<Self> {
        FILL_TOOLS.into_iter().find(|t| t.command() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            FillTool::Text => "Add text",
            FillTool::Check => "Checkmark",
            FillTool::Cross => "Cross",
            FillTool::Dot => "Dot",
            FillTool::Line => "Line",
            FillTool::Date => "Date",
            FillTool::Signature => "Sign",
            FillTool::Initials => "Initials",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            FillTool::Text => "type",
            FillTool::Check => "check",
            FillTool::Cross => "x",
            FillTool::Dot => "circle-dot",
            FillTool::Line => "minus",
            FillTool::Date => "clock-3",
            FillTool::Signature | FillTool::Initials => "signature",
        }
    }

    fn mark(self) -> Option<FillMark> {
        match self {
            FillTool::Check => Some(FillMark::Check),
            FillTool::Cross => Some(FillMark::Cross),
            FillTool::Dot => Some(FillMark::Dot),
            FillTool::Line => Some(FillMark::Line),
            _ => None,
        }
    }
}

/// Text being typed onto a page: (page, top-left in user space, text, focus requested).
#[derive(Clone, Debug, PartialEq)]
pub struct TypeBox {
    pub page: usize,
    pub at: [f64; 2],
    pub text: String,
    pub focus: bool,
}

/// The size Fill & Sign uses for typed text (Acrobat's default is 10 pt).
pub const TEXT_SIZE: f64 = 10.0;

fn to_user(xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> [f64; 2] {
    let (vx, vy) = xf.screen_to_view(p);
    let u = info.pages[page].view_to_user(vx, vy);
    [u[0] as f64, u[1] as f64]
}

fn new(page: usize, shape: Shape, contents: String, author: &str) -> Edit {
    let style = Style::default_for(&shape);
    Edit::AddAnnotation(NewAnnotation { page, shape, style, contents, author: author.to_string() })
}

/// A typewriter annotation sized to its text.
pub fn typed(page: usize, at: [f64; 2], text: &str, author: &str) -> Edit {
    let rect = crate::comments::text_box_rect(at, text, TEXT_SIZE);
    new(page, Shape::Typewriter { rect, font_size: TEXT_SIZE }, text.to_string(), author)
}

/// Place a saved signature (strokes normalised to a 0–1 box, y up) with its left edge at `at`,
/// 150 pt wide. Left, centred and upright are as displayed on a page turned by `rotation` (its
/// `/Rotate`), so the strokes are turned back into user space.
pub fn signature_at(page: usize, at: [f64; 2], strokes: &[Vec<[f32; 2]>], rotation: i64, author: &str) -> Option<Edit> {
    let w = 150.0;
    let (min_y, max_y) = strokes.iter().flatten().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
    if !min_y.is_finite() {
        return None;
    }
    let h = f64::from(max_y - min_y).max(0.05) * w;
    // Displayed right and up as user-space unit vectors (the identity on an unturned page).
    let [a, b, c, d, ..] = pdfcraft_model::view_matrix_for(rotation, [0.0; 4]);
    let strokes: Vec<Vec<[f64; 2]>> = strokes
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.iter()
                .map(|p| {
                    let (dx, dy) = (f64::from(p[0]) * w, f64::from(p[1] - min_y) * w - h / 2.0);
                    [at[0] + a * dx + c * dy, at[1] + b * dx + d * dy]
                })
                .collect()
        })
        .collect();
    (!strokes.is_empty()).then(|| new(page, Shape::Signature { strokes }, String::new(), author))
}

/// Place typed text in the script font with its left edge at `at`, `height` points tall, upright
/// as displayed on a page turned by `rotation`.
pub fn typed_signature_at(page: usize, at: [f64; 2], text: &str, height: f64, rotation: i64, author: &str) -> Option<Edit> {
    pdfcraft_engine::typed_signature_shape(at, text, height, rotation).map(|shape| new(page, shape, String::new(), author))
}

/// Place a saved signature or initials, upright as `info`'s page is displayed.
pub fn place(page: usize, info: &PageInfo, at: [f64; 2], sig: &SavedSig, initials: bool, author: &str) -> Option<Edit> {
    let rotation = i64::from(info.rotation);
    match sig {
        SavedSig::Drawn(strokes) => signature_at(page, at, strokes, rotation, author),
        SavedSig::Image(image) => image.edit(page, info, at, initials, author),
        SavedSig::Typed(text) => {
            let [left, bottom, right, top] = pdfcraft_engine::script_outline(text).bounds();
            let height = if initials { 24.0_f64 } else { 32.0_f64 };
            // Keep long names within the same placement width as drawn signatures.
            let height = height.min(150.0 * (top - bottom).max(0.1) / (right - left).max(0.01));
            typed_signature_at(page, at, text, height, rotation, author)
        }
    }
}

/// The text in the script font as a picture (`w`×`h` px, black on transparent), for previews.
pub(crate) fn script_preview(text: &str, w: usize, h: usize) -> egui::ColorImage {
    let o = pdfcraft_engine::script_outline(text);
    let mut img = egui::ColorImage::filled([w, h], Color32::TRANSPARENT);
    let [left, bottom, right, top] = o.bounds();
    let span = (top - bottom).max(0.1);
    let width = (right - left).max(0.01);
    if o.contours.is_empty() {
        return img;
    }
    let k = ((h as f64 * 0.9) / span).min((w as f64 * 0.95) / width);
    let x0 = (w as f64 - width * k) / 2.0;
    // Device points (y down), then an even-odd scanline fill.
    let polys: Vec<Vec<(f64, f64)>> = o
        .contours
        .iter()
        .map(|c| c.iter().map(|p| (x0 + (p[0] - left) * k, h as f64 * 0.5 + (top + bottom) / 2.0 * k - p[1] * k)).collect())
        .collect();
    for y in 0..h {
        let sy = y as f64 + 0.5;
        let mut xs: Vec<f64> = Vec::new();
        for poly in &polys {
            for i in 0..poly.len() {
                let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
                if (a.1 <= sy) != (b.1 <= sy) {
                    xs.push(a.0 + (sy - a.1) / (b.1 - a.1) * (b.0 - a.0));
                }
            }
        }
        xs.sort_by(|a, b| a.total_cmp(b));
        for pair in xs.as_chunks::<2>().0 {
            let (from, to) = (pair[0].round().clamp(0.0, w as f64) as usize, pair[1].round().clamp(0.0, w as f64) as usize);
            for x in from..to {
                img[(x, y)] = Color32::BLACK;
            }
        }
    }
    img
}

/// Saved previews and add/remove controls, shared by the left panel and quick-tool picker.
pub(crate) fn signature_entries(ui: &mut egui::Ui, app: &mut crate::PdfCraftApp, t: &Tokens) -> Option<&'static str> {
    ui.set_width(ui.available_width().clamp(240.0, 248.0));
    let mut command = None;
    for (i, (saved, what, use_id, change_id, remove_id)) in [
        (app.signature.as_ref(), "signature", "sign.fill.signature", "sign.fill.signature.change", "sign.fill.signature.remove"),
        (app.initials.as_ref(), "initials", "sign.fill.initials", "sign.fill.initials.change", "sign.fill.initials.remove"),
    ]
    .into_iter()
    .enumerate()
    {
        let label = |template: &str| crate::i18n::fmt(tl!(template), &[("what", tl!(what))]);
        let Some(saved) = saved else {
            if crate::widgets::ghost_button(ui, "plus", &label("Add {what}")).clicked() {
                command = Some(use_id);
            }
            continue;
        };
        ui.horizontal(|ui| {
            let (rect, response) = ui.allocate_exact_size(vec2((ui.available_width() - 68.0).max(140.0), 52.0), Sense::click());
            let use_label = label("Use {what}");
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, use_label.clone()));
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, CornerRadius::same(4), Color32::WHITE);
            painter.rect_stroke(
                rect,
                CornerRadius::same(4),
                Stroke::new(1.0, if response.hovered() { t.accent } else { t.border }),
                egui::StrokeKind::Inside,
            );
            let preview_rect = rect.shrink(8.0);
            match saved {
                SavedSig::Typed(text) => {
                    let cache = &mut app.saved_signature_previews[i];
                    if cache.as_ref().is_none_or(|(s, _)| s != saved) {
                        *cache = Some((
                            saved.clone(),
                            ui.ctx().load_texture(format!("saved-{what}"), script_preview(text, 480, 104), egui::TextureOptions::LINEAR),
                        ));
                    }
                    if let Some((_, tex)) = cache {
                        let size = vec2(480.0, 104.0) * (preview_rect.width() / 480.0).min(preview_rect.height() / 104.0);
                        painter.image(
                            tex.id(),
                            egui::Rect::from_center_size(preview_rect.center(), size),
                            egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                }
                SavedSig::Image(image) => {
                    image_preview(ui, preview_rect, image, &mut app.saved_signature_previews[i]);
                }
                SavedSig::Drawn(strokes) => {
                    let bounds = strokes
                        .iter()
                        .flatten()
                        .filter(|p| p.iter().all(|v| v.is_finite()))
                        .fold(egui::Rect::NOTHING, |r, p| r.union(egui::Rect::from_min_max(pos2(p[0], p[1]), pos2(p[0], p[1]))));
                    if bounds.is_finite() {
                        let scale = (preview_rect.width() / bounds.width().max(0.01)).min(preview_rect.height() / bounds.height().max(0.01));
                        for stroke in strokes {
                            let pts = stroke
                                .iter()
                                .filter(|p| p.iter().all(|v| v.is_finite()))
                                .map(|p| preview_rect.center() + vec2(p[0] - bounds.center().x, bounds.center().y - p[1]) * scale)
                                .collect();
                            painter.add(egui::Shape::line(pts, Stroke::new(1.5, Color32::BLACK)));
                        }
                    }
                }
            }
            if response.on_hover_text(label("Place saved {what}")).clicked() {
                command = Some(use_id);
            }
            if crate::icons::button(ui, "pencil", 26.0, false, &label("Change {what}")).clicked() {
                command = Some(change_id);
            }
            if crate::icons::button(ui, "x", 26.0, false, &label("Remove saved {what}")).clicked() {
                command = Some(remove_id);
            }
        });
        ui.add_space(4.0);
    }
    command
}

/// Preview and clicks with a Fill & Sign tool on one page.
#[allow(clippy::too_many_arguments)]
pub(crate) fn page_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: &PageXform,
    page: usize,
    info: &DocInfo,
    tool: FillTool,
    view: &mut DocView,
    signature: Option<&SavedSig>,
    initials: Option<&SavedSig>,
    preview: &mut Option<(SavedSig, egui::TextureHandle)>,
    author: &str,
    date_text: &Result<String, String>,
) -> Option<FillAction> {
    let pointer = ui.input(|i| i.pointer.hover_pos())?;
    if !resp.contains_pointer() || !xf.rect.contains(pointer) {
        return None;
    }
    let p = info.pages.get(page)?;
    let at = to_user(xf, info, page, pointer);
    let saved = match tool {
        FillTool::Signature => signature,
        FillTool::Initials => initials,
        _ => None,
    };
    if let Some(SavedSig::Image(image)) = saved {
        if let Some(rect) = image.rect(p, at, tool == FillTool::Initials) {
            let tex = image_texture(ui, image, preview);
            xf.paint_user_image(ui.painter(), tex, p, rect, i64::from(p.rotation), Color32::WHITE);
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        }
    } else {
        ui.ctx().set_cursor_icon(if tool == FillTool::Text { egui::CursorIcon::Text } else { egui::CursorIcon::Crosshair });
    }
    if !resp.clicked() {
        return None;
    }
    match tool {
        FillTool::Text => {
            // Clicking away from an open box commits it: the page sees this click before the box
            // sees its lost focus, so replacing the box first would drop what was typed.
            let open = view.fill_text.replace(TypeBox { page, at: [at[0], at[1] + TEXT_SIZE * 0.6], text: String::new(), focus: true });
            open.filter(|tb| !tb.text.trim().is_empty()).map(|tb| FillAction::Edit(Box::new(typed(tb.page, tb.at, tb.text.trim(), author))))
        }
        FillTool::Date => Some(match date_text {
            Ok(text) => FillAction::Edit(Box::new(typed(page, [at[0], at[1] + TEXT_SIZE * 0.6], text, author))),
            Err(why) => FillAction::Refused(why.clone()),
        }),
        FillTool::Signature => match signature {
            Some(s) => place(page, p, at, s, false, author).map(|e| FillAction::Signature(Box::new(e))),
            None => Some(FillAction::CreateSignature),
        },
        FillTool::Initials => match initials {
            Some(s) => place(page, p, at, s, true, author).map(|e| FillAction::Signature(Box::new(e))),
            None => Some(FillAction::CreateInitials),
        },
        mark => {
            let mark = mark.mark()?;
            let (w, h) = if mark == FillMark::Line { (36.0, 4.0) } else { (12.0, 12.0) };
            let rect = [at[0] - w / 2.0, at[1] - h / 2.0, at[0] + w / 2.0, at[1] + h / 2.0];
            Some(FillAction::Edit(Box::new(new(page, Shape::Mark { rect, mark }, String::new(), author))))
        }
    }
}

/// What a Fill & Sign click asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum FillAction {
    Edit(Box<Edit>),
    /// Place once, then select the new signature or initials for adjustment.
    Signature(Box<Edit>),
    /// No signature yet: open the signature pad.
    CreateSignature,
    /// No initials yet.
    CreateInitials,
    /// Nothing placed, and why (today's date can't be written into the PDF yet).
    Refused(String),
}

/// The in-place editor for typed text. Returns the edit once committed.
pub(crate) fn type_box(ctx: &egui::Context, view: &mut DocView, info: &DocInfo, author: &str) -> Option<Edit> {
    let tb = view.fill_text.clone()?;
    let xf = view.page_xform(tb.page)?;
    let v = info.pages.get(tb.page)?.user_to_view(tb.at[0] as f32, tb.at[1] as f32);
    let pos = xf.norm_to_screen(v[0] / xf.pw, v[1] / xf.ph);
    let zoom = xf.rect.width() / xf.pw.max(1.0);
    let mut commit = false;
    let mut cancel = false;
    egui::Area::new(egui::Id::new(("fill-text", view.id.0))).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        let Some(t) = view.fill_text.as_mut() else { return };
        let width = ((t.text.chars().count().max(8) as f32) * TEXT_SIZE as f32 * 0.6 * zoom).clamp(60.0, 600.0);
        // The area's max rect is last frame's size, which caps the edit's width; widen it so the
        // box grows as text is typed.
        ui.set_min_width(width);
        let r = ui.add(
            egui::TextEdit::singleline(&mut t.text)
                .font(egui::FontId::proportional((TEXT_SIZE as f32 * zoom).max(8.0)))
                .desired_width(width)
                .background_color(Color32::from_rgba_unmultiplied(255, 255, 255, 230))
                .text_color(Color32::BLACK)
                .hint_text(tl!("Type text"))
                .id_salt("fill-text-edit"),
        );
        if t.focus {
            r.request_focus();
            t.focus = false;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        } else if r.lost_focus() {
            commit = true;
        }
    });
    if cancel {
        view.fill_text = None;
        return None;
    }
    if commit {
        let tb = view.fill_text.take()?;
        if !tb.text.trim().is_empty() {
            return Some(typed(tb.page, tb.at, tb.text.trim(), author));
        }
    }
    None
}

/// The signature pad: type, draw or import an image. Returns Apply, Cancel and Browse.
pub(crate) fn signature_pad(
    ui: &mut egui::Ui,
    t: &Tokens,
    d: &mut SigDraft,
    preview: &mut Option<(SavedSig, egui::TextureHandle)>,
) -> (bool, bool, bool) {
    let what = if d.initials { tl!("initials") } else { tl!("signature") };
    let title = if d.editing { tl!("Change {what}") } else { tl!("Create {what}") };
    ui.label(egui::RichText::new(crate::i18n::fmt(title, &[("what", what)])).font(crate::theme::semibold(18.0)));
    ui.horizontal(|ui| {
        if crate::widgets::pill_button(ui, tl_ctx!("signature pad", "Type"), !d.drawing && !d.image_mode).clicked() {
            d.drawing = false;
            d.image_mode = false;
        }
        if crate::widgets::pill_button(ui, tl!("Draw"), d.drawing && !d.image_mode).clicked() {
            d.drawing = true;
            d.image_mode = false;
        }
        if crate::widgets::pill_button(ui, tl!("Image"), d.image_mode).clicked() {
            d.image_mode = true;
        }
    });
    ui.add_space(6.0);
    if d.image_mode {
        ui.label(egui::RichText::new(tl!("Choose a PNG or JPEG image (up to 4 MiB). Transparency is preserved.")).color(t.text_muted));
        let browse = crate::widgets::ghost_button(ui, "folder", tl!("Browse…")).clicked();
        let (rect, _) = ui.allocate_exact_size(vec2(460.0, 150.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(6), Color32::WHITE);
        ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        if let Some(image) = &d.image {
            image_preview(ui, rect.shrink(8.0), image, preview);
        }
        let (apply, cancel) = pad_buttons(ui, d);
        return (apply, cancel, browse);
    }
    if !d.drawing {
        let l = ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Type your {what}."), &[("what", what)])).color(t.text_muted));
        ui.add(
            egui::TextEdit::singleline(&mut d.text)
                .char_limit(pdfcraft_engine::MAX_SIGNATURE_CHARS)
                .desired_width(460.0)
                .hint_text(tl!(if d.initials { "Initials" } else { "Your name" })),
        )
        .labelled_by(l.id);
        let (rect, _) = ui.allocate_exact_size(vec2(460.0, 150.0), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(6), Color32::WHITE);
        painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        let key = SavedSig::Typed(d.text.clone());
        if preview.as_ref().is_none_or(|(s, _)| *s != key) {
            let img = script_preview(&d.text, 920, 300);
            *preview = Some((key, ui.ctx().load_texture("typed-signature", img, egui::TextureOptions::LINEAR)));
        }
        if let Some((_, tex)) = preview {
            painter.image(tex.id(), rect.shrink(4.0), egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        let (apply, cancel) = pad_buttons(ui, d);
        return (apply, cancel, false);
    }
    ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Draw your {what} below."), &[("what", what)])).color(t.text_muted));
    let strokes = &mut d.strokes;
    let (rect, resp) = ui.allocate_exact_size(vec2(460.0, 150.0), Sense::drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(6), Color32::WHITE);
    painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    painter.hline(rect.x_range().shrink(24.0), rect.bottom() - 34.0, Stroke::new(1.0, t.divider));
    // Normalised: x 0–1 across the pad, y up, in units of the pad's width.
    let norm = |p: Pos2| [(p.x - rect.left()) / rect.width(), (rect.bottom() - p.y) / rect.width()];
    if resp.drag_started() {
        strokes.push(Vec::new());
    }
    if resp.dragged()
        && let (Some(p), Some(s)) = (resp.interact_pointer_pos(), strokes.last_mut())
    {
        let n = norm(p.clamp(rect.min, rect.max));
        if s.last().is_none_or(|l| (l[0] - n[0]).abs() + (l[1] - n[1]).abs() > 0.002) {
            s.push(n);
        }
    }
    for s in strokes.iter() {
        let pts: Vec<Pos2> = s.iter().map(|p| pos2(rect.left() + p[0] * rect.width(), rect.bottom() - p[1] * rect.width())).collect();
        painter.add(egui::Shape::line(pts, Stroke::new(2.0, Color32::BLACK)));
    }
    let (apply, cancel) = pad_buttons(ui, d);
    (apply, cancel, false)
}

fn image_texture(ui: &egui::Ui, image: &SignatureImage, cache: &mut Option<(SavedSig, egui::TextureHandle)>) -> egui::TextureId {
    let key = SavedSig::Image(image.clone());
    if cache.as_ref().is_some_and(|(s, _)| *s != key) {
        *cache = None;
    }
    let (_, tex) = cache.get_or_insert_with(|| {
        let pixels = egui::ColorImage::from_rgba_unmultiplied(image.size(), image.rgba());
        (key, ui.ctx().load_texture("signature-image", pixels, egui::TextureOptions::LINEAR))
    });
    tex.id()
}

fn image_preview(ui: &egui::Ui, rect: egui::Rect, image: &SignatureImage, cache: &mut Option<(SavedSig, egui::TextureHandle)>) {
    let tex = image_texture(ui, image, cache);
    let [w, h] = image.size();
    let size = vec2(w as f32, h as f32);
    let size = size * (rect.width() / size.x).min(rect.height() / size.y);
    ui.painter().image(
        tex,
        egui::Rect::from_center_size(rect.center(), size),
        egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
}

fn pad_buttons(ui: &mut egui::Ui, d: &mut SigDraft) -> (bool, bool) {
    ui.add_space(10.0);
    let (mut apply, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        if ui.button(tl!("Clear")).clicked() {
            d.strokes.clear();
            d.text.clear();
            d.image = None;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let ready = d.ready();
            if ui.add_enabled_ui(ready, |ui| crate::widgets::pill_button(ui, tl!("Apply"), true)).inner.clicked() {
                apply = true;
            }
            if crate::widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
            }
        });
    });
    (apply, cancel)
}

#[cfg(target_arch = "wasm32")]
#[derive(Clone, Default)]
pub(crate) struct ImageInbox {
    busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    picked: std::sync::Arc<std::sync::Mutex<Option<PickedImage>>>,
}

#[cfg(target_arch = "wasm32")]
struct PickedImage {
    epoch: u64,
    initials: bool,
    bytes: Option<Result<Vec<u8>, String>>,
}

impl crate::PdfCraftApp {
    /// Browse asynchronously, answering only into the dialog that asked for the image.
    pub(crate) fn pick_signature_image(&mut self) {
        let epoch = self.dialog_epoch();
        let initials = self.signature_draft.initials;
        let dialog = rfd::AsyncFileDialog::new().add_filter(tl!("Image"), &["png", "jpg", "jpeg"]);
        #[cfg(not(target_arch = "wasm32"))]
        self.ask_one(crate::pickers::Ask::File(dialog), None, move |app, path| {
            if app.dialog_epoch() != epoch || app.dialog != Some(crate::Dialog::Signature) || app.signature_draft.initials != initials {
                return;
            }
            let result =
                std::fs::File::open(path).map_err(pdfcraft_engine::signature_image::SignatureImageError::from).and_then(SignatureImage::read);
            app.use_signature_image(result.map_err(|e| e.to_string()));
        });
        #[cfg(target_arch = "wasm32")]
        {
            use std::sync::atomic::Ordering;
            if self.signature_images.busy.swap(true, Ordering::SeqCst) {
                self.notify_tr("Another file dialog is still open. Finish with it first.");
                return;
            }
            let inbox = self.signature_images.clone();
            let ctx = self.ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let bytes = if let Some(file) = dialog.pick_file().await {
                    if file.inner().size() > pdfcraft_engine::signature_image::MAX_SIGNATURE_IMAGE_BYTES as f64 {
                        Some(Err(pdfcraft_engine::signature_image::SignatureImageError::Size.to_string()))
                    } else {
                        // `pick_file` returns a readable handle. Read its Blob directly so a
                        // rejected browser read is an error (rfd's read helper unwraps it).
                        Some(match wasm_bindgen_futures::JsFuture::from(file.inner().array_buffer()).await {
                            Ok(buffer) => {
                                let bytes = js_sys::Uint8Array::new(&buffer);
                                if bytes.length() as usize > pdfcraft_engine::signature_image::MAX_SIGNATURE_IMAGE_BYTES {
                                    Err(pdfcraft_engine::signature_image::SignatureImageError::Size.to_string())
                                } else {
                                    Ok(bytes.to_vec())
                                }
                            }
                            Err(error) => Err(format!("{error:?}")),
                        })
                    }
                } else {
                    None
                };
                *inbox.picked.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(PickedImage { epoch, initials, bytes });
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            });
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn process_signature_images(&mut self) {
        let picked = self.signature_images.picked.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        let Some(picked) = picked else { return };
        self.signature_images.busy.store(false, std::sync::atomic::Ordering::SeqCst);
        if self.dialog_epoch() != picked.epoch || self.dialog != Some(crate::Dialog::Signature) || self.signature_draft.initials != picked.initials {
            return;
        }
        let Some(bytes) = picked.bytes else { return };
        let result = bytes
            .and_then(|bytes| pdfcraft_engine::guard(|| SignatureImage::from_bytes(&bytes)).map_err(|e| e.to_string())?.map_err(|e| e.to_string()));
        self.use_signature_image(result);
    }

    fn use_signature_image(&mut self, result: Result<SignatureImage, String>) {
        let cleaned = result.and_then(|image| image.remove_white_background(245, 35).map_err(|e| e.to_string()));
        match cleaned {
            Ok(image) => {
                self.signature_draft.image = Some(image);
                self.toast = None;
            }
            Err(e) => self.notify_fmt("Couldn't import the signature image: {e}", &[("e", &e)]),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn long_signature_previews_have_clear_margins() {
        for text in ["Alexandria Catherine Elizabeth Montgomery-Wellington", "Jg Jg Jg Jg Jg Jg Jg Jg Jg Jg Jg"] {
            for [w, h] in [[920, 300], [480, 104]] {
                let image = super::script_preview(text, w, h);
                assert!(image.pixels.iter().any(|p| p.a() > 0));
                for x in 0..w {
                    assert_eq!(image[(x, 0)].a(), 0);
                    assert_eq!(image[(x, h - 1)].a(), 0);
                }
                for y in 0..h {
                    assert_eq!(image[(0, y)].a(), 0);
                    assert_eq!(image[(w - 1, y)].a(), 0);
                }
            }
        }
    }
}
