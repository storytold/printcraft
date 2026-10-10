//! Edit a PDF ▸ Edit text & images: boxes around the paragraphs and images already on the page.
//! Click a paragraph to edit it in place (⌘Enter or clicking away applies and rewraps it to the
//! box, Esc cancels); drag it to move it, or drag the handle on its left or right edge to rewrap
//! it to a new width. Click an image to select it: drag to move, drag a corner to resize (keeping
//! its proportions), right-click for rotate, flip, replace, save and delete; Delete removes it.

use egui::{Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Stroke};
use pdfcraft_engine::Edit;
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageXform};

const ACCENT: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);

/// A paragraph being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct LineEditor {
    pub page: usize,
    pub block: usize,
    pub text: String,
    original: String,
    rect: Rect,
    /// The original PDF rectangle, in user space. It is converted again each frame so the editor
    /// stays attached while the page is zoomed, scrolled, or rotated.
    source_rect: [f32; 4],
    multiline: bool,
    /// How far the editor box may grow to the right (screen pixels): a single-line paragraph
    /// rewraps growing to the page's edge, a multi-line one keeps its width.
    max_width: f32,
    /// Current screen-space size. Recomputed from the current page transform before painting.
    size: f32,
    focus: bool,
    /// The Format text panel's values, and what the paragraph had (to send only changes).
    pub look: pdfcraft_engine::AddedText,
    look0: pdfcraft_engine::AddedText,
    /// Underline, line spacing (× size; 0 = the paragraph's own), character spacing (pt) and
    /// horizontal scale (%), and what they were.
    pub extras: Extras,
    extras0: Extras,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extras {
    pub underline: bool,
    pub line_spacing: f64,
    pub char_spacing: f64,
    pub scale: f64,
}

impl Default for Extras {
    fn default() -> Self {
        Extras { underline: false, line_spacing: 0.0, char_spacing: 0.0, scale: 100.0 }
    }
}

/// Underline, line spacing, character spacing and horizontal scale for the paragraph being
/// edited (under Format text). Returns `true` when something changed.
pub(crate) fn extras_panel(ui: &mut egui::Ui, e: &mut Extras) -> bool {
    let before = *e;
    ui.horizontal(|ui| {
        if crate::icons::button(ui, "underline", 26.0, e.underline, tl!("Underline")).clicked() {
            e.underline = !e.underline;
        }
        let zero = tl!("Line spacing").to_string();
        let label = |v: f64| if v == 0.0 { zero.clone() } else { format!("{v:.2}×") };
        egui::ComboBox::from_id_salt("line-spacing").selected_text(label(e.line_spacing)).width(110.0).show_ui(ui, |ui| {
            for v in [1.0, 1.15, 1.5, 2.0] {
                ui.selectable_value(&mut e.line_spacing, v, label(v));
            }
        });
    });
    ui.horizontal(|ui| {
        let l = ui.label(tl!("Character spacing"));
        ui.add(egui::DragValue::new(&mut e.char_spacing).range(-5.0..=50.0).speed(0.1).suffix(" pt")).labelled_by(l.id);
    });
    ui.horizontal(|ui| {
        let l = ui.label(tl!("Horizontal scale"));
        ui.add(egui::DragValue::new(&mut e.scale).range(10.0..=400.0).speed(1.0).suffix(" %")).labelled_by(l.id);
    });
    *e != before
}

impl LineEditor {
    pub(crate) fn has_changes(&self) -> bool {
        self.text != self.original || self.look != self.look0 || self.extras != self.extras0
    }

    /// How far the box may grow to the right, when the paragraph is a single line (a multi-line
    /// paragraph rewraps to its own width and the box doesn't grow).
    pub fn growth(&self) -> Option<f32> {
        (self.max_width > self.rect.width()).then_some(self.max_width)
    }

    /// Whether the edited text differs from the original (unsaved changes).
    pub fn has_unsaved_text(&self) -> bool {
        self.text != self.original
    }

    /// The formatting the panel changed.
    pub fn style(&self) -> pdfcraft_engine::BlockStyle {
        let (l, o) = (&self.look, &self.look0);
        pdfcraft_engine::BlockStyle {
            family: (l.family != o.family || l.bold != o.bold || l.italic != o.italic).then_some((l.family, l.bold, l.italic)),
            size: (l.size != o.size).then_some(l.size),
            color: (l.color != o.color).then_some(l.color),
            align: (l.align != o.align).then_some(l.align),
            underline: (self.extras.underline != self.extras0.underline).then_some(self.extras.underline),
            line_spacing: (self.extras.line_spacing != self.extras0.line_spacing && self.extras.line_spacing > 0.0)
                .then_some(self.extras.line_spacing),
            char_spacing: (self.extras.char_spacing != self.extras0.char_spacing).then_some(self.extras.char_spacing),
            scale: (self.extras.scale != self.extras0.scale).then_some(self.extras.scale),
            ..Default::default()
        }
    }

    /// After applying formatting: the paragraph now has it.
    pub fn applied(&mut self) {
        self.look0 = self.look.clone();
        self.extras0 = self.extras;
        self.original = self.text.clone();
        self.focus = true;
    }

    /// Adopt the rewritten paragraph's current geometry before the next overlay frame.
    pub(crate) fn refresh_source(&mut self, block: &pdfcraft_engine::TextBlock) {
        self.source_rect = block.rect.map(|v| v as f32);
        self.multiline = block.lines.len() > 1;
    }
}

/// The look shown for a paragraph: the family and weight guessed from its PDF font name.
fn source_look(base_font: &str, size: f64, color: [f64; 3], detected_bold: bool, detected_italic: bool) -> pdfcraft_engine::AddedText {
    use pdfcraft_engine::FontFamily as F;
    let name = base_font.to_ascii_lowercase();
    let family = if ["courier", "mono", "consolas", "menlo", "monaco", "lucida console"].iter().any(|s| name.contains(s)) {
        F::Courier
    } else if !name.contains("sans") && ["times", "serif", "roman", "cambria", "georgia", "palatino", "garamond"].iter().any(|s| name.contains(s)) {
        F::Times
    } else {
        F::Helvetica
    };
    pdfcraft_engine::AddedText {
        family,
        bold: detected_bold || ["bold", "black", "heavy", "semibold", "demi"].iter().any(|s| name.contains(s)),
        italic: detected_italic || ["italic", "oblique", "slanted"].iter().any(|s| name.contains(s)),
        size: (size * 10.0).round() / 10.0,
        color,
        ..Default::default()
    }
}

fn look_of(b: &pdfcraft_engine::TextBlock) -> pdfcraft_engine::AddedText {
    source_look(&b.base_font, b.size, b.color, b.bold, b.italic)
}

fn editor_font(look: &pdfcraft_engine::AddedText, size: f32) -> FontId {
    let family = match (look.family, look.bold) {
        (pdfcraft_engine::FontFamily::Courier, _) => FontFamily::Monospace,
        (_, true) => FontFamily::Name("semibold".into()),
        (_, false) => FontFamily::Proportional,
    };
    FontId::new(size, family)
}

fn color32(color: [f64; 3]) -> Color32 {
    Color32::from_rgb(
        (color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/// The dark box behind text too pale to read on white.
const DARK_EDITOR_FILL: Color32 = Color32::from_gray(0x26);

/// WCAG relative luminance of an sRGB colour.
fn luminance(c: Color32) -> f32 {
    let channel = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
}

/// WCAG contrast ratio between two colours (1 to 21).
fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The inline editor's background for text drawn in `text`: white, or a dark box when the text
/// reads better on that (white or pale text, which would vanish on white).
pub(crate) fn editor_fill(text: Color32) -> Color32 {
    if contrast(text, Color32::WHITE) >= contrast(text, DARK_EDITOR_FILL) { Color32::WHITE } else { DARK_EDITOR_FILL }
}

/// A selected page image, and what the pointer is doing to it.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageSelection {
    pub page: usize,
    pub index: usize,
    /// Dragging: the start point and, for a corner, the opposite corner (screen).
    drag: Option<(Pos2, Option<Pos2>)>,
}

/// What a drag on a paragraph box takes hold of.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grip {
    Move,
    /// An edge (screen left or right), dragged to rewrap the paragraph.
    Left,
    Right,
}

/// A paragraph box being dragged: moved, or (from a handle on its left or right edge) resized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockDrag {
    page: usize,
    block: usize,
    /// Where the drag started (screen).
    start: Pos2,
    grip: Grip,
}

/// How far an edge grip reaches either side of a box's edge (screen pixels).
const EDGE_REACH: f32 = 6.0;

/// The rewrap handles drawn on a paragraph box's left and right edges (screen).
fn width_handles(b: Rect) -> [Rect; 2] {
    let h = (b.height() * 0.5).clamp(14.0, 28.0);
    [b.left(), b.right()].map(|x| Rect::from_center_size(Pos2::new(x, b.center().y), egui::vec2(7.0, h)))
}

/// The edge of `b` that `p` grabs: anywhere along its height, within reach of the line. A narrow
/// box keeps its middle for moving.
fn edge_at(b: Rect, p: Pos2) -> Option<Grip> {
    if p.y < b.top() - EDGE_REACH || p.y > b.bottom() + EDGE_REACH {
        return None;
    }
    let inside = EDGE_REACH.min(b.width() / 4.0);
    let (dl, dr) = (p.x - b.left(), b.right() - p.x);
    match (dl >= -EDGE_REACH && dl <= inside, dr >= -EDGE_REACH && dr <= inside) {
        (true, true) if dl <= dr => Some(Grip::Left),
        (_, true) => Some(Grip::Right),
        (true, false) => Some(Grip::Left),
        _ => None,
    }
}

/// The topmost box under `p` and what it grabs there; edges only on an upright page.
fn grab_at(boxes: &[Rect], p: Pos2, upright: bool) -> Option<(usize, Grip)> {
    boxes.iter().enumerate().rev().find_map(|(i, b)| match upright.then(|| edge_at(*b, p)).flatten() {
        Some(g) => Some((i, g)),
        None => b.contains(p).then_some((i, Grip::Move)),
    })
}

/// What a right-click on a selected image asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ImageAction {
    Replace(usize, usize),
    Save(usize, usize),
}

fn user_box(xf: &PageXform, info: &DocInfo, page: usize, r: Rect) -> [f64; 4] {
    let p = &info.pages[page];
    let (a, b) = (xf.screen_to_view(r.min), xf.screen_to_view(r.max));
    let (u, v) = (p.view_to_user(a.0, a.1), p.view_to_user(b.0, b.1));
    [u[0].min(v[0]) as f64, u[1].min(v[1]) as f64, u[0].max(v[0]) as f64, u[1].max(v[1]) as f64]
}

/// Images on a page: select, move, resize, right-click. Returns `true` when the pointer was used.
#[allow(clippy::too_many_arguments)]
pub(crate) fn image_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: &PageXform,
    page: usize,
    info: &DocInfo,
    images: &[pdfcraft_engine::PageImage],
    view: &mut DocView,
    action: &mut Option<ImageAction>,
) -> bool {
    let boxes: Vec<Rect> = images.iter().map(|im| xf.user_rect(info, page, im.rect.map(|v| v as f32))).collect();
    let painter = ui.painter();
    for b in &boxes {
        painter.rect_stroke(*b, CornerRadius::ZERO, Stroke::new(0.75, ACCENT.gamma_multiply(0.35)), egui::StrokeKind::Outside);
    }
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let selected = view.image_selection.as_ref().filter(|s| s.page == page).map(|s| s.index).filter(|i| *i < boxes.len());
    for (i, b) in boxes.iter().enumerate() {
        let selected_box = selected == Some(i);
        let hovered = ui.input(|inp| inp.pointer.hover_pos()).is_some_and(|p| b.contains(p));
        let stroke = if selected_box {
            Stroke::new(1.5, ACCENT)
        } else if hovered {
            Stroke::new(1.5, ACCENT.gamma_multiply(0.7))
        } else {
            Stroke::new(0.75, ACCENT.gamma_multiply(0.35))
        };
        if selected_box {
            painter.rect_filled(*b, CornerRadius::ZERO, ACCENT.gamma_multiply(0.04));
        }
        painter.rect_stroke(*b, CornerRadius::ZERO, stroke, egui::StrokeKind::Outside);
    }
    // The selected image: frame, corner handles, dragging.
    if let Some(i) = selected {
        let b = boxes[i];
        painter.rect_stroke(b, CornerRadius::ZERO, Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
        let corners = [b.left_top(), b.right_top(), b.left_bottom(), b.right_bottom()];
        for c in corners {
            painter.rect(
                Rect::from_center_size(c, egui::vec2(8.0, 8.0)),
                CornerRadius::ZERO,
                Color32::WHITE,
                Stroke::new(1.0, ACCENT),
                egui::StrokeKind::Middle,
            );
        }
        let origin = ui.input(|i| i.pointer.press_origin());
        if resp.drag_started()
            && let Some(o) = origin
        {
            let corner = corners.iter().position(|c| c.distance(o) < 8.0);
            if corner.is_some() || b.contains(o) {
                let opposite = corner.map(|k| corners[3 - k]);
                if let Some(s) = view.image_selection.as_mut() {
                    s.drag = Some((o, opposite));
                }
            }
        }
        if let Some((start, opposite)) = view.image_selection.as_ref().and_then(|s| s.drag)
            && let Some(p) = pointer
        {
            let preview = match opposite {
                // Resize from the opposite corner, keeping the aspect ratio.
                Some(fixed) => {
                    let (w0, h0) = (b.width().max(1.0), b.height().max(1.0));
                    let k = ((p.x - fixed.x).abs() / w0).max((p.y - fixed.y).abs() / h0).max(0.05);
                    let (w, h) = (w0 * k, h0 * k);
                    let x = if p.x < fixed.x { fixed.x - w } else { fixed.x };
                    let y = if p.y < fixed.y { fixed.y - h } else { fixed.y };
                    Rect::from_min_size(Pos2::new(x, y), egui::vec2(w, h))
                }
                None => b.translate(p - start),
            };
            painter.rect_stroke(preview, CornerRadius::ZERO, Stroke::new(1.0, ACCENT), egui::StrokeKind::Middle);
            if resp.drag_stopped() {
                if let Some(s) = view.image_selection.as_mut() {
                    s.drag = None;
                }
                if preview != b {
                    view.pending_edit =
                        Some(Edit::EditPageImage { page, index: i, change: pdfcraft_engine::ImageEdit::Move(user_box(xf, info, page, preview)) });
                }
            }
            return true;
        }
        if ui.input(|inp| inp.key_pressed(egui::Key::Delete) || inp.key_pressed(egui::Key::Backspace)) && !ui.ctx().egui_wants_keyboard_input() {
            view.image_selection = None;
            view.pending_edit = Some(Edit::EditPageImage { page, index: i, change: pdfcraft_engine::ImageEdit::Delete });
            return true;
        }
    }
    let Some(p) = pointer.filter(|p| xf.rect.contains(*p)) else { return false };
    let Some(hit) = boxes.iter().rposition(|b| b.contains(p)) else { return false };
    if selected != Some(hit) {
        painter.rect_stroke(boxes[hit], CornerRadius::ZERO, Stroke::new(1.5, ACCENT.gamma_multiply(0.7)), egui::StrokeKind::Outside);
    }
    ui.ctx().set_cursor_icon(if selected == Some(hit) { egui::CursorIcon::Move } else { egui::CursorIcon::PointingHand });
    if resp.clicked() || resp.secondary_clicked() {
        view.image_selection = Some(ImageSelection { page, index: hit, drag: None });
    }
    if selected == Some(hit) {
        resp.context_menu(|ui| {
            use pdfcraft_engine::ImageEdit as E;
            let items: [(&str, Option<E>); 4] = [
                (tl!("Rotate Clockwise"), Some(E::Rotate(1))),
                (tl!("Rotate Counterclockwise"), Some(E::Rotate(3))),
                (tl!("Flip Horizontal"), Some(E::Flip { horizontal: true })),
                (tl!("Flip Vertical"), Some(E::Flip { horizontal: false })),
            ];
            for (label, change) in items {
                if ui.button(label).clicked() {
                    view.pending_edit = change.map(|c| Edit::EditPageImage { page, index: hit, change: c });
                    ui.close();
                }
            }
            let raster = images.get(hit).is_some_and(|image| !image.is_form);
            if ui.add_enabled(raster, egui::Button::new(tl!("Replace Image…"))).clicked() {
                *action = Some(ImageAction::Replace(page, hit));
                ui.close();
            }
            if ui.add_enabled(raster, egui::Button::new(tl!("Save Image As…"))).clicked() {
                *action = Some(ImageAction::Save(page, hit));
                ui.close();
            }
            ui.separator();
            if ui.button(tl!("Delete")).clicked() {
                view.image_selection = None;
                view.pending_edit = Some(Edit::EditPageImage { page, index: hit, change: E::Delete });
                ui.close();
            }
        });
    }
    true
}

/// Lines and their screen boxes for a page; hover outlines, click opens the editor. Returns
/// `true` when the pointer was used.
pub(crate) fn page_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: &PageXform,
    page: usize,
    info: &DocInfo,
    lines: &[pdfcraft_engine::TextBlock],
    view: &mut DocView,
) -> bool {
    let boxes: Vec<Rect> = lines.iter().map(|l| xf.user_rect(info, page, l.rect.map(|v| v as f32)).expand(2.0)).collect();
    let painter = ui.painter();
    let active = view.line_editor.as_ref().filter(|e| e.page == page).map(|e| e.block);
    for (i, b) in boxes.iter().enumerate() {
        if active == Some(i) {
            painter.rect_filled(b.expand(1.0), CornerRadius::same(2), ACCENT.gamma_multiply(0.08));
            painter.rect_stroke(*b, CornerRadius::same(2), Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
        } else {
            painter.rect_stroke(*b, CornerRadius::same(2), Stroke::new(0.75, ACCENT.gamma_multiply(0.35)), egui::StrokeKind::Outside);
        }
    }
    // The width handle needs the page upright (or upside down): on a quarter-turned page the
    // screen's horizontal is the paragraph's vertical.
    let upright = info.pages.get(page).is_some_and(|p| p.rotation % 180 == 0);
    let draw_handles = |b: Rect| {
        for h in width_handles(b) {
            painter.rect(h, CornerRadius::same(2), Color32::WHITE, Stroke::new(1.0, ACCENT), egui::StrokeKind::Middle);
        }
    };
    // A drag in progress: the box follows the pointer (or the grabbed edge does); releasing applies it.
    if let Some(d) = view.block_drag.filter(|d| d.page == page)
        && let (Some(b), Some(l)) = (boxes.get(d.block).copied(), lines.get(d.block))
    {
        let p = ui.input(|i| i.pointer.interact_pos()).unwrap_or(d.start);
        let dx = p.x - d.start.x;
        let preview = match d.grip {
            Grip::Move => b.translate(p - d.start),
            Grip::Left => Rect::from_min_max(Pos2::new((b.left() + dx).min(b.right() - 12.0), b.min.y), b.max),
            Grip::Right => Rect::from_min_max(b.min, Pos2::new((b.right() + dx).max(b.left() + 12.0), b.max.y)),
        };
        painter.rect_stroke(preview, CornerRadius::same(2), Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
        let resizing = d.grip != Grip::Move;
        if resizing {
            draw_handles(preview);
        }
        ui.ctx().set_cursor_icon(if resizing { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::Grabbing });
        if resp.drag_stopped() || !ui.input(|i| i.pointer.any_down()) {
            view.block_drag = None;
            let (from, to) = (user_box(xf, info, page, b), user_box(xf, info, page, preview));
            let style = if resizing {
                // The new width in user space: the box's change, added to the paragraph's own. A
                // moved left side (in user space; on an upside-down page that's the screen's right
                // edge) moves the paragraph with it.
                let width = (l.rect[2] - l.rect[0]) + (to[2] - to[0]) - (from[2] - from[0]);
                let shift = to[0] - from[0];
                pdfcraft_engine::BlockStyle { width: Some(width), offset: (shift != 0.0).then_some([shift, 0.0]), ..Default::default() }
            } else {
                pdfcraft_engine::BlockStyle { offset: Some([to[0] - from[0], to[1] - from[1]]), ..Default::default() }
            };
            if preview != b {
                view.pending_edit = Some(Edit::EditTextBlock { page, block: d.block, text: l.text.clone(), style });
            }
        }
        return true;
    }
    // Dragging a box (not while a paragraph is open for typing) moves it; from an edge, resizes it.
    // What it grabs comes from where the button went down, not where the pointer is now: egui
    // only calls it a drag once the pointer has moved a few pixels (or been held a moment), and a
    // quick flick has left the edge by then.
    if resp.drag_started()
        && active.is_none()
        && let Some(o) = ui.input(|i| i.pointer.press_origin())
        && let Some((block, grip)) = grab_at(&boxes, o, upright)
    {
        view.block_drag = Some(BlockDrag { page, block, start: o, grip });
        return true;
    }
    let Some(p) = ui.input(|i| i.pointer.hover_pos()).filter(|p| xf.rect.contains(*p)) else { return false };
    let Some((hit, grip)) = grab_at(&boxes, p, upright) else { return false };
    if active != Some(hit) {
        painter.rect_stroke(boxes[hit], CornerRadius::same(2), Stroke::new(1.5, ACCENT), egui::StrokeKind::Outside);
    }
    let on_edge = active.is_none() && grip != Grip::Move;
    if upright && active.is_none() {
        draw_handles(boxes[hit]);
    }
    ui.ctx().set_cursor_icon(if on_edge { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::Text });
    if resp.clicked() {
        let l = &lines[hit];
        // Screen pixels per point, from the box's width.
        let scale = (boxes[hit].width() - 4.0) / ((l.rect[2] - l.rect[0]).max(1.0) as f32);
        // How far the editor box may grow: a single-line paragraph's rewrite grows to the
        // page's right edge, a multi-line paragraph rewraps to its own width.
        let right = xf.rect.right().min(view.viewport_rect().right()) - 6.0;
        let max_width = if l.lines.len() == 1 { (right - boxes[hit].left()).max(boxes[hit].width()) } else { boxes[hit].width() };
        view.line_editor = Some(LineEditor {
            page,
            block: hit,
            text: l.text.clone(),
            original: l.text.clone(),
            rect: boxes[hit],
            source_rect: l.rect.map(|v| v as f32),
            multiline: l.lines.len() > 1,
            max_width,
            size: (l.size as f32 * scale).clamp(8.0, 72.0),
            focus: true,
            look: look_of(l),
            look0: look_of(l),
            extras: Extras::default(),
            extras0: Extras::default(),
        });
    }
    true
}

/// The inline editor; returns the edit once the text is applied.
pub(crate) fn overlay(ctx: &egui::Context, view: &mut DocView, info: &DocInfo) -> Option<Edit> {
    // Clicks outside the document (the Format text panel) keep the paragraph open.
    let outside = ctx.input(|i| i.pointer.latest_pos()).is_some_and(|p| !view.viewport_rect().contains(p));
    let page = view.line_editor.as_ref()?.page;
    let xf = view.page_xform(page)?;
    let viewport_right = view.viewport_rect().right();
    let hold = view.objects.hold_editor;
    let ed = view.line_editor.as_mut()?;
    if hold {
        ed.focus = true;
    }
    // Reproject the source box every frame. The page may have been zoomed, scrolled or rotated
    // while the format panel was open.
    ed.rect = xf.user_rect(info, ed.page, ed.source_rect).expand(2.0);
    let right = xf.rect.right().min(viewport_right) - 6.0;
    ed.max_width = if ed.multiline { ed.rect.width() } else { (right - ed.rect.left()).max(ed.rect.width()) };
    let scale = (ed.rect.width() / (ed.source_rect[2] - ed.source_rect[0]).abs().max(1.0)).max(0.01);
    ed.size = (ed.look.size as f32 * scale).clamp(8.0, 72.0);
    let font = editor_font(&ed.look, ed.size);
    let text_color = color32(ed.look.color);
    let mut done = None;
    egui::Area::new(egui::Id::new("edit-text-line")).order(egui::Order::Foreground).fixed_pos(Pos2::new(ed.rect.left(), ed.rect.top())).show(
        ctx,
        |ui| {
            // The box follows the text as you type: as wide as the longest drafted line needs
            // (up to what the rewrite allows), so new content grows the box instead of wrapping
            // inside the old one.
            let mut width = ed.rect.width().max(120.0);
            if ed.max_width > width {
                let needed = ui.fonts_mut(|f| {
                    ed.text.lines().map(|l| f.layout_no_wrap(l.to_owned(), font.clone(), text_color).size().x).fold(0.0_f32, f32::max)
                }) + 8.0;
                width = width.max(needed).min(ed.max_width);
            }
            // White, unless the text is too pale to read on white (white text from a coloured
            // banner, #913): then a dark box, with a caret that shows on it.
            let fill = editor_fill(text_color);
            egui::Frame::NONE.fill(fill).stroke(Stroke::new(1.5, ACCENT)).inner_margin(egui::Margin::symmetric(2, 0)).show(ui, |ui| {
                ui.visuals_mut().text_cursor.stroke.color = if fill == Color32::WHITE { Color32::BLACK } else { Color32::WHITE };
                let rows = ed.text.lines().count().max(1);
                let r = ui.add(
                    egui::TextEdit::multiline(&mut ed.text)
                        .id(egui::Id::new("edit-text-line-input"))
                        .font(font.clone())
                        .text_color(text_color)
                        .frame(egui::Frame::NONE)
                        .desired_width(width)
                        .desired_rows(rows),
                );
                if ed.focus {
                    r.request_focus();
                    ed.focus = false;
                }
                let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                let apply = ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
                if esc {
                    done = Some(false);
                } else if apply || (r.lost_focus() && !outside && !hold) {
                    done = Some(true);
                }
            });
        },
    );
    match done {
        Some(apply) => {
            let ed = view.line_editor.take()?;
            let style = ed.style();
            (apply && (ed.text != ed.original || style != pdfcraft_engine::BlockStyle::default())).then_some(Edit::EditTextBlock {
                page: ed.page,
                block: ed.block,
                text: ed.text,
                style,
            })
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pale_text_is_edited_on_a_dark_box() {
        // #913: white text from a coloured banner was typed onto a white box and vanished.
        for pale in [Color32::WHITE, Color32::from_rgb(0xFF, 0xFF, 0x00), Color32::from_gray(0xE0), Color32::from_rgb(0xFF, 0xA5, 0x00)] {
            assert_eq!(editor_fill(pale), DARK_EDITOR_FILL, "{pale:?}");
            assert!(contrast(pale, editor_fill(pale)) >= 4.5, "{pale:?} is readable");
        }
        // Ordinary dark or saturated text keeps the white box.
        for dark in [Color32::BLACK, Color32::from_rgb(0x1F, 0x2A, 0x6B), Color32::from_rgb(0xC0, 0x00, 0x00), Color32::from_gray(0x60)] {
            assert_eq!(editor_fill(dark), Color32::WHITE, "{dark:?}");
        }
    }
}
