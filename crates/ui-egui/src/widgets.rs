//! Small custom widgets built on the design tokens.

use egui::{Align2, Color32, CornerRadius, Rect, Response, Sense, Stroke, vec2};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, icons};

/// A mode-bar tab: text with an underline when active.
pub fn mode_tab(ui: &mut egui::Ui, label: &str, active: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = if active { theme::semibold(13.5) } else { theme::medium(13.5) };
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 22.0, 48.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), active, label));
    if resp.hovered() && !active {
        ui.painter().rect_filled(rect.shrink2(vec2(2.0, 9.0)), CornerRadius::same(6), t.hover);
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font, if active { t.text } else { t.text_muted });
    if active {
        let r = Rect::from_min_max(rect.left_bottom() + vec2(11.0, -3.0), rect.right_bottom() - vec2(11.0, 0.0));
        ui.painter().rect_filled(r, CornerRadius::same(1), t.text);
    }
    resp
}

/// Rounded pill button; `primary` fills with the accent.
pub fn pill_button(ui: &mut egui::Ui, label: &str, primary: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(12.5);
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 26.0, 28.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    let (fill, stroke, text) = if primary {
        (if resp.hovered() { t.accent_text } else { t.accent }, Stroke::NONE, Color32::WHITE)
    } else {
        (if resp.hovered() { t.hover } else { t.card }, Stroke::new(1.2, t.text_muted), t.text)
    };
    ui.painter().rect(rect, CornerRadius::same(14), fill, stroke, egui::StrokeKind::Inside);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font, text);
    resp
}

/// Icon + label, transparent until hovered.
pub fn ghost_button(ui: &mut egui::Ui, icon: &str, label: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(13.0);
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 38.0, 30.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(6), t.hover);
    }
    icons::paint(ui, Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(18.0, 18.0)), icon, 17.0, t.icon);
    ui.painter().text(rect.left_center() + vec2(30.0, 0.0), Align2::LEFT_CENTER, label, font, t.text);
    resp
}

/// A search-field lookalike that opens the command palette.
pub fn search_box(ui: &mut egui::Ui, placeholder: &str, width: f32) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), placeholder));
    let fill = if resp.hovered() { t.hover } else { t.field };
    ui.painter().rect(rect, CornerRadius::same(16), fill, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    icons::paint(ui, Rect::from_min_size(rect.min + vec2(10.0, 8.0), vec2(16.0, 16.0)), "search", 15.0, t.text_muted);
    let shortcut = crate::commands::command_shortcut_label(ui.ctx(), "view.palette");
    let shortcut = ui.painter().layout_no_wrap(shortcut, theme::regular(11.5), t.text_faint);
    let shortcut_pos = rect.right_center() - vec2(12.0 + shortcut.size().x, shortcut.size().y * 0.5);
    // Reserve the measured shortcut width in every language; long translations are elided
    // within the remaining space rather than painted underneath the shortcut.
    let text_right = (shortcut_pos.x - 10.0).max(rect.left());
    let text_left = (rect.left() + 34.0).min(text_right);
    let text_rect = Rect::from_min_max(egui::pos2(text_left, rect.top()), egui::pos2(text_right, rect.bottom()));
    let mut job = egui::text::LayoutJob::simple_singleline(placeholder.to_owned(), theme::regular(13.0), t.text_faint);
    job.wrap = egui::text::TextWrapping { max_width: text_rect.width(), max_rows: 1, break_anywhere: true, ..Default::default() };
    let text = ui.fonts_mut(|f| f.layout_job(job));
    let elided = text.elided;
    ui.painter().with_clip_rect(text_rect).galley(egui::pos2(text_left, rect.center().y - text.size().y * 0.5), text, t.text_faint);
    ui.painter().with_clip_rect(rect).galley(shortcut_pos, shortcut, t.text_faint);
    let resp = resp.on_hover_cursor(egui::CursorIcon::Text);
    if elided { resp.on_hover_text(placeholder) } else { resp }
}

pub fn menu_item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> Response {
    ui.add(egui::Button::new(label).shortcut_text(shortcut))
}

pub fn section_title(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(10.0);
    // Section titles across every panel go through here, so one translation point covers them.
    ui.label(egui::RichText::new(tl!(text).to_uppercase()).font(theme::semibold(10.5)).color(t.text_faint).extra_letter_spacing(0.6));
    ui.add_space(2.0);
}

/// Paint one line of `text` left-aligned and vertically centred on `pos`, cut with "…" so it is
/// at most `max_width` wide: a long file name or path stays inside its row (#425).
pub fn row_text(ui: &egui::Ui, pos: egui::Pos2, text: impl Into<String>, font: egui::FontId, color: Color32, max_width: f32) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.into(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(0.0));
    let galley = ui.painter().layout_job(job);
    let rect = Align2::LEFT_CENTER.anchor_size(pos, galley.size());
    ui.painter().galley(rect.min, galley, color);
}

/// Transient message at the bottom centre.
pub fn toast(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some((msg, start)) = app.toast.clone() else { return };
    let now = ctx.input(|i| i.time);
    let start = if start == 0.0 { now } else { start };
    app.toast = Some((msg.clone(), start));
    if now - start > 3.5 {
        app.toast = None;
        return;
    }
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    // Above the progress notice while a job runs.
    let card = if app.progress_notice.is_some() { ctx.data(|d| d.get_temp::<f32>(egui::Id::new(PROGRESS_HEIGHT))).unwrap_or(0.0) } else { 0.0 };
    let lift = if card > 0.0 { 28.0 + card + 10.0 } else { 28.0 };
    egui::Area::new(egui::Id::new("toast"))
        .order(egui::Order::Tooltip)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(screen.center_bottom() - vec2(0.0, lift))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(if t.dark() { Color32::from_rgb(0xEC, 0xEC, 0xEF) } else { Color32::from_rgb(0x2A, 0x2A, 0x2F) })
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(16, 10))
                .show(ui, |ui| {
                    // An Area remembers its previous content width. A short notice
                    // must not force the next validation message into a narrow column.
                    // Allow natural short labels; wrap longer ones within the viewport,
                    // including the frame's 32 points and 16-point outside margins.
                    ui.set_max_width((screen.width() - 64.0).clamp(1.0, 560.0));
                    ui.add(
                        egui::Label::new(egui::RichText::new(msg).color(if t.dark() { Color32::from_rgb(0x22, 0x22, 0x26) } else { Color32::WHITE }))
                            .wrap(),
                    );
                });
        });
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
}

/// A background job's progress, shown at the bottom centre until the job is done.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressNotice {
    /// What is happening ("Optimizing… image 3 of 40").
    pub label: String,
    /// From 0 to 1.
    pub fraction: f32,
    /// Show a Cancel button.
    pub cancellable: bool,
}

/// Where the progress notice keeps its height this frame, for placing the toast above it.
const PROGRESS_HEIGHT: &str = "progress-notice-height";

/// Draw [`PdfCraftApp::progress_notice`]: the label, a bar that eases towards the fraction with
/// a light sweeping across it, the percentage and Cancel. Returns true when Cancel was clicked.
pub fn progress_notice(app: &PdfCraftApp, ctx: &egui::Context) -> bool {
    let Some(p) = &app.progress_notice else { return false };
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    // The same inverted card as the toast.
    let (fill, text) = if t.dark() {
        (Color32::from_rgb(0xEC, 0xEC, 0xEF), Color32::from_rgb(0x22, 0x22, 0x26))
    } else {
        (Color32::from_rgb(0x2A, 0x2A, 0x2F), Color32::WHITE)
    };
    let fraction = if p.fraction.is_finite() { p.fraction.clamp(0.0, 1.0) } else { 0.0 };
    let shown = ctx.animate_value_with_time(egui::Id::new("progress-notice-bar"), fraction, 0.3);
    let now = ctx.input(|i| i.time);
    let mut cancel = false;
    let area = egui::Area::new(egui::Id::new("progress-notice"))
        .order(egui::Order::Tooltip)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(screen.center_bottom() - vec2(0.0, 28.0))
        .show(ctx, |ui| {
            egui::Frame::NONE.fill(fill).corner_radius(CornerRadius::same(10)).inner_margin(egui::Margin::symmetric(16, 12)).show(ui, |ui| {
                let width = 340.0_f32.min(screen.width() - 64.0).max(160.0);
                ui.set_width(width);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&p.label).color(text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(format!("{:.0}%", shown * 100.0)).font(theme::medium(12.0)).color(text.gamma_multiply(0.7)));
                    });
                });
                ui.add_space(6.0);
                let (track, resp) = ui.allocate_exact_size(vec2(width, 6.0), Sense::hover());
                // The label above names the job; the bar carries the percentage.
                resp.widget_info(|| {
                    let mut info = egui::WidgetInfo::new(egui::WidgetType::ProgressIndicator);
                    info.value = Some(f64::from(fraction) * 100.0);
                    info
                });
                let painter = ui.painter();
                painter.rect_filled(track, CornerRadius::same(3), text.gamma_multiply(0.18));
                let done = Rect::from_min_size(track.min, vec2(track.width() * shown, track.height()));
                if done.width() > 0.5 {
                    painter.rect_filled(done, CornerRadius::same(3), t.accent);
                    // A soft light sweeping along the filled part says the job is alive between
                    // updates.
                    let band = 48.0;
                    let x = done.left() - band + ((now * 160.0) % f64::from(done.width() + band)) as f32;
                    let clip = painter.with_clip_rect(done);
                    for (i, a) in [0.10_f32, 0.22, 0.10].into_iter().enumerate() {
                        let r = Rect::from_min_size(egui::pos2(x + i as f32 * band / 3.0, done.top()), vec2(band / 3.0, done.height()));
                        clip.rect_filled(r, CornerRadius::ZERO, Color32::WHITE.gamma_multiply(a));
                    }
                }
                if p.cancellable {
                    ui.add_space(4.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let b = egui::Button::new(egui::RichText::new(tl!("Cancel")).font(theme::medium(12.5)).color(text)).frame(false);
                        cancel = ui.add(b).clicked();
                    });
                }
            });
        });
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(PROGRESS_HEIGHT), area.response.rect.height()));
    ctx.request_repaint_after(std::time::Duration::from_millis(33));
    cancel
}

/// The ArtCraft wordmark (Storyteller's brand, docs/brand/; not open source), sized to `height`.
pub fn artcraft_logo(ui: &mut egui::Ui, height: f32) -> Response {
    let dark = ui.visuals().dark_mode;
    let (uri, bytes): (&str, &'static [u8]) = if dark {
        ("bytes://artcraft-logo-white.svg", include_bytes!("../../../docs/brand/artcraft-logo-white.svg"))
    } else {
        ("bytes://artcraft-logo.svg", include_bytes!("../../../docs/brand/artcraft-logo.svg"))
    };
    ui.add(egui::Image::from_bytes(uri, bytes).max_height(height).alt_text("ArtCraft"))
}

/// The ArtCraft mark (brand blue, works on light and dark), `size` points square.
pub fn artcraft_mark(ui: &mut egui::Ui, size: f32) -> Response {
    ui.add(
        egui::Image::from_bytes("bytes://artcraft-mark.svg", include_bytes!("../../../docs/brand/artcraft-mark.svg"))
            .fit_to_exact_size(vec2(size, size))
            .alt_text("ArtCraft"),
    )
}

/// Buttons for every community link (`pdfcraft_engine::links`), Discord first and prominent.
/// Returns the registry command of the one clicked.
pub fn community_links(ui: &mut egui::Ui) -> Option<&'static str> {
    let mut clicked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for (i, l) in pdfcraft_engine::links::LINKS.iter().enumerate() {
            let resp = if i == 0 { icon_pill(ui, l.icon, tl!("Join our Discord"), true) } else { icon_pill(ui, l.icon, tl!(l.label), false) };
            if resp.on_hover_text(l.url).clicked() {
                clicked = Some(l.command);
            }
        }
    });
    clicked
}

/// [`icon_pill`] with a ▾ part at its end that opens a menu of related choices. Returns the
/// main button's response and the ▾'s (give it to `egui::Popup::menu`).
pub fn split_pill(ui: &mut egui::Ui, icon: &str, label: &str, more: &str) -> (Response, Response) {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(12.5);
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, _) = ui.allocate_exact_size(vec2(w + 46.0 + 28.0, 30.0), Sense::hover());
    let (main_rect, more_rect) = rect.split_left_right_at_x(rect.right() - 28.0);
    let main = ui.interact(main_rect, ui.id().with(("split-pill", label)), Sense::click());
    let arrow = ui.interact(more_rect, ui.id().with(("split-pill-more", label)), Sense::click());
    main.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    arrow.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), more));
    let radius = CornerRadius::same(15);
    ui.painter().rect(rect, radius, t.card, Stroke::new(1.2, t.text_muted), egui::StrokeKind::Inside);
    let left = CornerRadius { nw: 15, sw: 15, ne: 0, se: 0 };
    let right = CornerRadius { nw: 0, sw: 0, ne: 15, se: 15 };
    for (r, resp, corners) in [(main_rect, &main, left), (more_rect, &arrow, right)] {
        if resp.hovered() {
            ui.painter().rect_filled(r.shrink(1.2), corners, t.hover);
        }
    }
    ui.painter().vline(more_rect.left(), rect.y_range().shrink(7.0), Stroke::new(1.0, t.text_muted));
    crate::icons::paint(ui, Rect::from_min_size(rect.min + vec2(12.0, 7.0), vec2(16.0, 16.0)), icon, 15.0, t.text);
    ui.painter().text(rect.left_center() + vec2(34.0, 0.0), Align2::LEFT_CENTER, label, font, t.text);
    crate::icons::paint(ui, Rect::from_center_size(more_rect.center(), vec2(14.0, 14.0)), "chevron-down", 13.0, t.text);
    (main.on_hover_text(label), arrow.on_hover_text(more))
}

/// A pill button with an icon (primary = filled accent).
pub fn icon_pill(ui: &mut egui::Ui, icon: &str, label: &str, primary: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(12.5);
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 46.0, 30.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    let (fill, stroke, text) = if primary {
        (if resp.hovered() { t.accent_text } else { t.accent }, Stroke::NONE, Color32::WHITE)
    } else {
        (if resp.hovered() { t.hover } else { t.card }, Stroke::new(1.2, t.text_muted), t.text)
    };
    ui.painter().rect(rect, CornerRadius::same(15), fill, stroke, egui::StrokeKind::Inside);
    crate::icons::paint(ui, Rect::from_min_size(rect.min + vec2(12.0, 7.0), vec2(16.0, 16.0)), icon, 15.0, text);
    ui.painter().text(rect.left_center() + vec2(34.0, 0.0), Align2::LEFT_CENTER, label, font, text);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Pos2;

    #[test]
    fn search_placeholder_never_overlaps_the_shortcut() {
        let previous = crate::i18n::current();
        for (code, expected) in [("en", "Ctrl+K"), ("de", "Strg+K")] {
            crate::i18n::set_current(crate::i18n::Lang::from_code(code).unwrap());
            for width in [260.0, 160.0, 90.0] {
                let ctx = egui::Context::default();
                ctx.set_os(egui::os::OperatingSystem::Nix);
                ctx.set_fonts(theme::font_definitions());
                let placeholder = "Werkzeuge und Befehle suchen";
                let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    search_box(ui, placeholder, width);
                });
                let text_shapes: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| if let egui::Shape::Text(text) = &shape.shape { Some((shape.clip_rect, text)) } else { None })
                    .collect();
                let (clip, hint) = text_shapes.iter().find(|(_, text)| text.galley.job.text == placeholder).unwrap();
                let (_, shortcut) = text_shapes.iter().find(|(_, text)| text.galley.job.text.contains('K')).unwrap();
                assert_eq!(shortcut.galley.job.text, expected);
                assert!(clip.right() <= shortcut.pos.x - 9.0, "hint must leave a gap before the shortcut at width {width}");
                assert!(hint.galley.elided, "the long original German hint must be elided at width {width}");
                assert_eq!(hint.galley.rows.len(), 1);
                output.drop_without_applying_deltas();
            }
        }
        crate::i18n::set_current(previous);
    }

    fn toast_rect(app: &mut PdfCraftApp, ctx: &egui::Context, width: f32, now: &mut f64) -> Rect {
        // Let the Area settle after a notice/viewport change, without advancing to
        // expiry. No native rendering or renderer worker is needed for this layout.
        for _ in 0..3 {
            *now += 0.016;
            let output = ctx.run_ui(
                egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 900.0))), time: Some(*now), ..Default::default() },
                |ui| toast(app, ui.ctx()),
            );
            output.drop_without_applying_deltas();
        }
        ctx.memory(|memory| memory.area_rect(egui::Id::new("toast")).unwrap())
    }

    #[test]
    fn toast_width_adapts_to_long_notices_and_narrow_viewports() {
        let ctx = egui::Context::default();
        let mut app = PdfCraftApp::new();
        let mut now = 1.0;
        app.notify("Saved");
        let short = toast_rect(&mut app, &ctx, 1280.0, &mut now);
        assert!(short.width() < 160.0, "short notices retain their natural width: {short:?}");
        app.notify("Fill in name failed: The value entered is not valid for the field [ name ]");
        let long = toast_rect(&mut app, &ctx, 1280.0, &mut now);
        assert!(long.width() > 300.0, "a previous short Area must not squeeze validation feedback: {long:?}");
        assert!(long.height() < 85.0, "ordinary feedback must not become a seven-line column: {long:?}");
        for width in [320.0, 180.0] {
            let rect = toast_rect(&mut app, &ctx, width, &mut now);
            assert!(rect.width() <= width - 32.0 + 1.0, "frame and viewport margins must fit: {rect:?}");
            assert!(rect.left() >= 15.0 && rect.right() <= width - 15.0, "notice remains horizontally on screen: {rect:?}");
        }
        app.notify("Saved");
        let again = toast_rect(&mut app, &ctx, 1280.0, &mut now);
        assert!((again.width() - short.width()).abs() < 1.0, "long notices must not impose a permanent minimum width");
    }
}
