//! Window chrome: tab strip (with the integrated macOS title bar), mode bar, right rail.

use egui::{Align, Align2, Color32, CornerRadius, Layout, Rect, Sense, Stroke, vec2};

use crate::canvas::{DocView, Fit, PageLayout};
use crate::theme::{self, ThemePreference, Tokens};
use crate::{Dialog, Mode, PdfCraftApp, PropsTab, RightPanel, icons, widgets};

/// Width of one window-control button on the title bar.
pub const CONTROL_W: f32 = 46.0;
/// Height of the title bar, and of the window-control buttons.
const TITLE_H: f32 = 38.0;
/// How far in from a frameless window's edge the pointer resizes it.
const RESIZE_BAND: f32 = 5.0;
/// How far along an edge from a corner the pointer resizes diagonally.
const RESIZE_CORNER: f32 = 14.0;

/// A frameless window (`PdfCraftApp::window_controls`): minimize, maximize/restore and close at the
/// top right, resizing from the edges and corners, and a hairline border. Drawn above everything,
/// so the controls stay reachable whatever covers the title bar.
pub fn window_frame(app: &PdfCraftApp, ctx: &egui::Context) {
    if !app.window_controls {
        return;
    }
    let (maximized, fullscreen) = ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.viewport().fullscreen.unwrap_or(false)));
    if fullscreen {
        return;
    }
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    let controls = egui::Rect::from_min_size(egui::pos2(screen.max.x - 3.0 * CONTROL_W, screen.min.y), egui::vec2(3.0 * CONTROL_W, TITLE_H));
    egui::Area::new(egui::Id::new("pc_window_controls")).order(egui::Order::Foreground).fixed_pos(controls.min).show(ctx, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
        ui.horizontal(|ui| {
            for kind in ["minimize", "maximize", "close"] {
                let (r, resp) = ui.allocate_exact_size(egui::vec2(CONTROL_W, TITLE_H), egui::Sense::click());
                let close = kind == "close";
                let hot = resp.hovered() || resp.is_pointer_button_down_on();
                let c = if close && hot {
                    ui.painter().rect_filled(r, 0.0, egui::Color32::from_rgb(0xC4, 0x2B, 0x1C));
                    egui::Color32::WHITE
                } else {
                    if hot {
                        ui.painter().rect_filled(r, 0.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
                    }
                    t.icon
                };
                let stroke = egui::Stroke::new(1.0, c);
                let m = r.center();
                let p = ui.painter();
                let tip = match kind {
                    "minimize" => {
                        p.line_segment([egui::pos2(m.x - 5.0, m.y), egui::pos2(m.x + 5.0, m.y)], stroke);
                        tl!("Minimize")
                    }
                    "maximize" if maximized => {
                        let front = egui::Rect::from_min_size(egui::pos2(m.x - 5.0, m.y - 3.0), egui::vec2(8.0, 8.0));
                        p.rect_stroke(front, 0.0, stroke, egui::StrokeKind::Inside);
                        p.line_segment([egui::pos2(m.x - 3.0, m.y - 5.0), egui::pos2(m.x + 5.0, m.y - 5.0)], stroke);
                        p.line_segment([egui::pos2(m.x + 5.0, m.y - 5.0), egui::pos2(m.x + 5.0, m.y + 3.0)], stroke);
                        tl!("Restore Down")
                    }
                    "maximize" => {
                        p.rect_stroke(egui::Rect::from_center_size(m, egui::vec2(10.0, 10.0)), 0.0, stroke, egui::StrokeKind::Inside);
                        tl!("Maximize")
                    }
                    _ => {
                        p.line_segment([egui::pos2(m.x - 5.0, m.y - 5.0), egui::pos2(m.x + 5.0, m.y + 5.0)], stroke);
                        p.line_segment([egui::pos2(m.x - 5.0, m.y + 5.0), egui::pos2(m.x + 5.0, m.y - 5.0)], stroke);
                        tl!("Close")
                    }
                };
                if resp.on_hover_text(tip).clicked() {
                    ctx.send_viewport_cmd(match kind {
                        "minimize" => egui::ViewportCommand::Minimized(true),
                        "maximize" => egui::ViewportCommand::Maximized(!maximized),
                        _ => egui::ViewportCommand::Close,
                    });
                }
            }
        });
    });
    if maximized {
        return;
    }
    ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("pc_window_border"))).rect_stroke(
        screen,
        0.0,
        egui::Stroke::new(1.0, t.border),
        egui::StrokeKind::Inside,
    );
    let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else { return };
    if !screen.contains(pos) {
        return;
    }
    let near = |d: f32, band: f32| d < band;
    let (dl, dr, dt, db) = (pos.x - screen.min.x, screen.max.x - pos.x, pos.y - screen.min.y, screen.max.y - pos.y);
    let (w, e, n, s) = (near(dl, RESIZE_BAND), near(dr, RESIZE_BAND), near(dt, RESIZE_BAND), near(db, RESIZE_BAND));
    let (cw, ce, cn, cs) = (near(dl, RESIZE_CORNER), near(dr, RESIZE_CORNER), near(dt, RESIZE_CORNER), near(db, RESIZE_CORNER));
    use egui::{CursorIcon, ResizeDirection};
    let hit = if (n && cw) || (w && cn) {
        Some((ResizeDirection::NorthWest, CursorIcon::ResizeNorthWest))
    } else if (n && ce) || (e && cn) {
        Some((ResizeDirection::NorthEast, CursorIcon::ResizeNorthEast))
    } else if (s && cw) || (w && cs) {
        Some((ResizeDirection::SouthWest, CursorIcon::ResizeSouthWest))
    } else if (s && ce) || (e && cs) {
        Some((ResizeDirection::SouthEast, CursorIcon::ResizeSouthEast))
    } else if n {
        Some((ResizeDirection::North, CursorIcon::ResizeNorth))
    } else if s {
        Some((ResizeDirection::South, CursorIcon::ResizeSouth))
    } else if w {
        Some((ResizeDirection::West, CursorIcon::ResizeWest))
    } else if e {
        Some((ResizeDirection::East, CursorIcon::ResizeEast))
    } else {
        None
    };
    if let Some((dir, icon)) = hit {
        ctx.set_cursor_icon(icon);
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
        }
    }
}

pub fn tab_strip(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let left = if cfg!(target_os = "macos") && app.integrated_titlebar { 80 } else { 8 };
    egui::Panel::top("tab_strip")
        .exact_size(38.0)
        .frame(egui::Frame::NONE.fill(t.titlebar).inner_margin(egui::Margin { left, right: 10, top: 0, bottom: 0 }))
        .show(ui, |ui| {
            let full = ui.max_rect();
            let drag = ui.interact(full, ui.id().with("titledrag"), Sense::click_and_drag());
            if drag.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if drag.double_clicked() {
                let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
            }
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if icons::button(ui, "house", 28.0, app.active.is_none() && !app.combine_showing(), tl!("Home")).clicked() {
                    app.active = None;
                    app.combine_tab.focused = false;
                }
                // Keep the two 28-point buttons, Discord's text/padding and three gaps outside the scrolling tabs.
                let controls_width = ui.fonts_mut(|f| f.layout_no_wrap("Discord".into(), theme::medium(13.0), t.text).size().x) + 106.0;
                // Open stays in sight after the tabs: its icon, text and padding, and the gaps around it.
                let open_width = ui.fonts_mut(|f| f.layout_no_wrap(tl!("Open").into(), theme::medium(13.0), t.text).size().x) + 38.0 + 12.0;
                let mut tabs_width = (ui.available_width() - controls_width - open_width).max(0.0);
                // Tabs shrink to share the strip (names end in "…") and scroll only once they reach
                // their narrowest; then arrows either side say so and step through them.
                let mut natural: Vec<f32> = Vec::with_capacity(app.views.len() + 1);
                for v in &app.views {
                    if let Some(doc) = app.session.get(v.id) {
                        natural.push(tab_natural_width(ui, &t, &doc.display_name()));
                    }
                }
                if app.combine_tab.open {
                    natural.push(tab_natural_width(ui, &t, tl!("Combine files")));
                }
                let gap = ui.spacing().item_spacing.x;
                let overflow = tabs_overflow(&natural, tabs_width, gap, TAB_MIN_WIDTH);
                if overflow {
                    tabs_width = (tabs_width - 2.0 * (TAB_ARROW + gap)).max(0.0);
                }
                let cap = tab_cap(&natural, tabs_width, gap, TAB_MIN_WIDTH);
                // Where the tabs were scrolled to last frame, and how far they can go.
                let scroll_id = ui.id().with("tab_scroll");
                let (offset, max_offset) = ui.data(|d| d.get_temp::<(f32, f32)>(scroll_id)).unwrap_or((0.0, 0.0));
                let mut scroll_to = None;
                if overflow && arrow(ui, "chevron-left", offset > 0.5, tl!("Scroll tabs left")) {
                    scroll_to = Some((offset - (TAB_MIN_WIDTH + gap)).max(0.0));
                }
                let state = (app.active, app.views.len(), app.combine_showing());
                let changed = ui.data_mut(|data| {
                    let id = ui.id().with("active_tab");
                    let changed = data.get_temp::<(Option<usize>, usize, bool)>(id) != Some(state);
                    data.insert_temp(id, state);
                    changed
                });
                let mut close = None;
                ui.scope(|ui| {
                    ui.style_mut().always_scroll_the_only_direction = true;
                    let mut area = egui::ScrollArea::horizontal().id_salt("document_tabs").max_width(tabs_width).auto_shrink([true, true]);
                    if let Some(x) = ui.data_mut(|d| d.remove_temp::<f32>(scroll_id.with("to"))) {
                        area = area.horizontal_scroll_offset(x);
                    }
                    let out = area.show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            for i in 0..app.views.len() {
                                let Some(doc) = app.session.get(app.views[i].id) else { continue };
                                let (name, dirty) = (doc.display_name(), doc.dirty);
                                // A tab showing the document's title names its file on hover.
                                let file = (name != doc.name).then_some(doc.name.as_str());
                                let response = tab(ui, &t, "file-text", &name, file, dirty, app.active == Some(i), &mut close, i, cap);
                                if changed && app.active == Some(i) && !app.combine_showing() {
                                    response.scroll_to_me(Some(Align::Center));
                                }
                                if response.clicked() {
                                    app.active = Some(i);
                                }
                            }
                            if let Some(i) = close {
                                app.request_close_tab(i);
                            }
                            if app.combine_tab.open {
                                // After the document tabs; its index can't clash with theirs.
                                let mut close = None;
                                let response =
                                    tab(ui, &t, "files", tl!("Combine files"), None, false, app.combine_showing(), &mut close, usize::MAX, cap);
                                if changed && app.combine_showing() {
                                    response.scroll_to_me(Some(Align::Center));
                                }
                                if response.clicked() {
                                    app.open_combine_tab();
                                }
                                if close.is_some() {
                                    app.close_combine_tab();
                                }
                            }
                        });
                    });
                    let max = (out.content_size.x - out.inner_rect.width()).max(0.0);
                    ui.data_mut(|d| d.insert_temp(scroll_id, (out.state.offset.x, max)));
                });
                if overflow && arrow(ui, "chevron-right", offset < max_offset - 0.5, tl!("Scroll tabs right")) {
                    scroll_to = Some((offset + TAB_MIN_WIDTH + gap).min(max_offset));
                }
                if let Some(x) = scroll_to {
                    ui.data_mut(|d| d.insert_temp(scroll_id.with("to"), x));
                    ui.ctx().request_repaint();
                }
                ui.add_space(4.0);
                let tip = crate::commands::command_tip(ui.ctx(), tl!("Open a PDF ({key})"), "file.open");
                if widgets::ghost_button(ui, "plus", tl!("Open")).on_hover_text(tip).clicked() {
                    app.open_dialog();
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if app.window_controls {
                        ui.add_space(3.0 * CONTROL_W);
                    }
                    let (icon, label) = match app.theme_preference {
                        ThemePreference::System => ("settings", tl!("Use system setting")),
                        ThemePreference::Light => ("sun", tl!("Light gray")),
                        ThemePreference::Dark => ("moon", tl!("Dark gray")),
                    };
                    let tip = format!("{}: {label}", tl!("Display theme"));
                    let response = icons::button(ui, icon, 28.0, false, &tip);
                    egui::Popup::menu(&response).show(|ui| theme_menu(app, ui));
                    if icons::button(ui, "circle-help", 28.0, false, tl!("Keyboard shortcuts")).clicked() {
                        app.dialog = Some(Dialog::Shortcuts);
                    }
                    // One click to the community, from anywhere in the app.
                    if widgets::ghost_button(ui, "messages-square", "Discord").on_hover_text(pdfcraft_engine::links::DISCORD).clicked() {
                        app.execute("help.discord");
                    }
                });
            });
        });
}

/// Both theme entry points use the same choices and command path.
fn theme_menu(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    for (preference, command, label) in [
        (ThemePreference::System, "view.theme.system", "Use system setting"),
        (ThemePreference::Light, "view.theme.light", "Light gray"),
        (ThemePreference::Dark, "view.theme.dark", "Dark gray"),
    ] {
        if ui.radio(app.theme_preference == preference, tl!(label)).clicked() {
            app.execute(command);
            ui.close();
        }
    }
}

/// A tab's chrome around its name: the icon on the left, the close button on the right.
const TAB_CHROME: f32 = 64.0;
/// The narrowest a tab shrinks to before the strip scrolls: room for most of a name.
const TAB_MIN_WIDTH: f32 = 180.0;
/// The arrows either side of tabs that don't fit.
const TAB_ARROW: f32 = 24.0;

/// Whether tabs of `natural` widths, `gap` apart, are wider than `budget` even at `min` each.
fn tabs_overflow(natural: &[f32], budget: f32, gap: f32, min: f32) -> bool {
    let narrowest: f32 = natural.iter().map(|w| w.min(min)).sum();
    narrowest + gap * natural.len().saturating_sub(1) as f32 > budget
}

/// A scroll arrow beside the tabs; dimmed (and inert) at its end. True when clicked.
fn arrow(ui: &mut egui::Ui, icon: &str, enabled: bool, tip: &str) -> bool {
    ui.add_enabled_ui(enabled, |ui| icons::button(ui, icon, TAB_ARROW, false, tip)).inner.clicked()
}

/// A tab name, at most 28 characters (longer names end in "…").
fn tab_name(name: &str) -> String {
    if name.chars().count() > 28 { format!("{}…", name.chars().take(27).collect::<String>()) } else { name.to_string() }
}

/// The width a tab takes when nothing has to shrink.
fn tab_natural_width(ui: &egui::Ui, t: &Tokens, name: &str) -> f32 {
    let label = crate::bidi::visual(&tab_name(name)).into_owned();
    ui.fonts_mut(|f| f.layout_no_wrap(label, theme::regular(13.0), t.text).size().x) + TAB_CHROME
}

/// The widest any tab may be so that tabs of `natural` widths, `gap` apart, fit in `budget`:
/// narrow tabs keep their width and the rest share what is left equally (as browsers do).
/// `None` when every tab fits as it is; never below `min` (then the strip scrolls).
fn tab_cap(natural: &[f32], budget: f32, gap: f32, min: f32) -> Option<f32> {
    let n = natural.len();
    if n == 0 {
        return None;
    }
    let mut left = budget - gap * (n - 1) as f32;
    if natural.iter().sum::<f32>() <= left {
        return None;
    }
    let mut sorted = natural.to_vec();
    sorted.sort_by(f32::total_cmp);
    for (k, w) in sorted.iter().enumerate() {
        let share = left / (n - k) as f32;
        if *w > share {
            return Some(share.max(min));
        }
        left -= w;
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn tab(
    ui: &mut egui::Ui,
    t: &Tokens,
    icon: &str,
    name: &str,
    file: Option<&str>,
    dirty: bool,
    active: bool,
    close: &mut Option<usize>,
    index: usize,
    cap: Option<f32>,
) -> egui::Response {
    let font = theme::regular(13.0);
    let mut label = tab_name(name);
    // A shrunk tab shows as much of its name as fits (the tooltip has all of it).
    if let Some(cap) = cap {
        let room = (cap - TAB_CHROME).max(0.0);
        label = ui.fonts_mut(|f| {
            crate::panels::ellipsized_prefix(&label, |s| f.layout_no_wrap(crate::bidi::visual(s).into_owned(), font.clone(), t.text).size().x <= room)
        });
    }
    // Painted text only: the accessibility name below keeps the logical order.
    let label = crate::bidi::visual(&label).into_owned();
    let text_w = ui.fonts_mut(|f| f.layout_no_wrap(label.clone(), font.clone(), t.text).size().x);
    let width = cap.map_or(text_w + TAB_CHROME, |cap| (text_w + TAB_CHROME).min(cap));
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
    let a11y = if dirty { format!("{name} (edited)") } else { name.to_string() };
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, &a11y));
    let bg = if active {
        t.chrome
    } else if resp.hovered() {
        t.hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius { nw: 7, ne: 7, sw: 0, se: 0 }, bg);
    icons::paint(ui, Rect::from_min_size(rect.min + vec2(6.0, 7.0), vec2(16.0, 16.0)), icon, 15.0, if active { t.accent } else { t.text_muted });
    ui.painter().text(rect.min + vec2(28.0, rect.height() / 2.0), Align2::LEFT_CENTER, label, font, if active { t.text } else { t.text_muted });
    let x_rect = Rect::from_center_size(rect.right_center() - vec2(16.0, 0.0), vec2(20.0, 20.0));
    let x = ui.interact(x_rect, ui.id().with(("tabclose", index)), Sense::click());
    if x.hovered() {
        ui.painter().rect_filled(x_rect, CornerRadius::same(4), t.pressed);
    }
    // Unsaved changes: a dot where the close button sits, until the tab is hovered.
    if dirty && !resp.hovered() && !x.hovered() {
        ui.painter().circle_filled(x_rect.center(), 4.0, if active { t.text } else { t.text_muted });
    } else if active || resp.hovered() || x.hovered() {
        icons::paint(ui, x_rect, "x", 13.0, t.text_muted);
    }
    if x.clicked() {
        *close = Some(index);
    }
    let shown = match file {
        Some(file) => std::borrow::Cow::Owned(format!("{}\n{}", crate::bidi::visual(name), crate::bidi::visual(file))),
        None => crate::bidi::visual(name),
    };
    resp.on_hover_text(if dirty { crate::i18n::fmt(tl!("{name} — unsaved changes"), &[("name", shown.as_ref())]) } else { shown.into_owned() })
}

pub fn mode_bar(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::top("mode_bar")
        .exact_size(48.0)
        .frame(egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::symmetric(10, 0)).stroke(Stroke::new(1.0, t.divider)))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                main_menu(app, ui);
                ui.add_space(6.0);
                ui.painter().vline(ui.cursor().left(), ui.max_rect().y_range().shrink(12.0), Stroke::new(1.0, t.divider));
                ui.add_space(10.0);
                for (mode, label) in
                    [(Mode::AllTools, "All tools"), (Mode::Read, "Read"), (Mode::Edit, "Edit"), (Mode::Convert, "Convert"), (Mode::Sign, "E-Sign")]
                {
                    if widgets::mode_tab(ui, tl!(label), app.mode == mode).clicked() {
                        app.select_mode(mode);
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let has_doc = app.active.is_some();
                    ui.add_enabled_ui(has_doc, |ui| {
                        let tip = crate::commands::command_tip(ui.ctx(), tl!("Print ({key})"), "print.dialog");
                        if icons::button(ui, "printer", 32.0, false, &tip).clicked() {
                            app.run_command("print.dialog");
                        }
                        if icons::button(ui, "save", 32.0, false, tl!("Save (M4)")).clicked() {
                            app.run_command("file.save");
                        }
                        let tip = crate::commands::command_tip(ui.ctx(), tl!("Document properties ({key})"), "file.properties");
                        if icons::button(ui, "info", 32.0, false, &tip).clicked() {
                            app.dialog = Some(Dialog::Properties(PropsTab::Description));
                        }
                    });
                    ui.add_space(8.0);
                    if widgets::search_box(ui, tl!("Find tools and commands"), 260.0).clicked() {
                        app.palette_open = true;
                    }
                });
            });
        });
}

fn main_menu(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let resp = widgets::ghost_button(ui, "panel-left", tl!("Menu"));
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(230.0);
        ui.menu_button(tl!("File"), |ui| crate::commands::registry_menu(app, ui, "File"));
        ui.menu_button(tl!("Edit"), |ui| crate::commands::registry_menu(app, ui, "Edit"));
        ui.menu_button(tl!("Pages"), |ui| crate::commands::registry_menu(app, ui, "Pages"));
        ui.menu_button(tl!("View"), |ui| {
            if let Some(i) = app.active {
                let v = &mut app.views[i];
                let ctx = ui.ctx().clone();
                let key = |s| crate::commands::shortcut_label(&ctx, s);
                use crate::commands::{ACTUAL_SIZE, FIT_WIDTH, NEXT_VIEW, PAGE_LEVEL, PREV_VIEW, ROTATE_CCW, ROTATE_CW, ZOOM_IN, ZOOM_OUT};
                ui.label(egui::RichText::new(tl!("Zoom")).color(t.text_faint).small());
                if widgets::menu_item(ui, tl!("Actual size"), &key(ACTUAL_SIZE)).clicked() {
                    v.set_zoom(1.0);
                }
                if widgets::menu_item(ui, tl!("Zoom to page level"), &key(PAGE_LEVEL)).clicked() {
                    v.fit = Fit::Page;
                }
                if widgets::menu_item(ui, tl!("Fit to width"), &key(FIT_WIDTH)).clicked() {
                    v.fit = Fit::Width;
                }
                if widgets::menu_item(ui, tl!("Fit to height"), "").clicked() {
                    v.fit = Fit::Height;
                    v.goto = Some((v.current, 0.0));
                }
                if widgets::menu_item(ui, tl!("Fit visible"), &crate::commands::command_shortcut_label(&ctx, "view.fit_visible")).clicked() {
                    ui.close();
                    app.execute("view.fit_visible");
                    return;
                }
                if widgets::menu_item(ui, tl!("Zoom in"), &key(ZOOM_IN)).clicked() {
                    v.zoom_step(true);
                }
                if widgets::menu_item(ui, tl!("Zoom out"), &key(ZOOM_OUT)).clicked() {
                    v.zoom_step(false);
                }
                if widgets::menu_item(ui, tl!("Rotate view clockwise"), &key(ROTATE_CW)).clicked() {
                    v.rotate_view(true);
                }
                if widgets::menu_item(ui, tl!("Rotate view counterclockwise"), &key(ROTATE_CCW)).clicked() {
                    v.rotate_view(false);
                }
                ui.separator();
                ui.label(egui::RichText::new(tl!("Page navigation")).color(t.text_faint).small());
                if ui.add_enabled(!v.back.is_empty(), egui::Button::new(tl!("Previous view")).shortcut_text(key(PREV_VIEW))).clicked() {
                    v.view_history(false);
                }
                if ui.add_enabled(!v.forward.is_empty(), egui::Button::new(tl!("Next view")).shortcut_text(key(NEXT_VIEW))).clicked() {
                    v.view_history(true);
                }
                ui.separator();
                if let Some(id) = page_display_menu(ui, &app.views[i]) {
                    app.execute(id);
                }
                ui.separator();
            }
            crate::commands::registry_menu(app, ui, "View");
            ui.menu_button(tl!("Display theme"), |ui| theme_menu(app, ui));
            ui.menu_button(tl!("Side panels"), |ui| {
                for (p, label) in [
                    (RightPanel::Comments, tl!("Comments")),
                    (RightPanel::Bookmarks, tl!("Bookmarks")),
                    (RightPanel::Pages, tl!("Pages")),
                    (RightPanel::Fields, tl!("Fields")),
                    (RightPanel::Layers, tl!("Layers")),
                    (RightPanel::Attachments, tl!("Attachments")),
                    (RightPanel::Signatures, tl!("Signatures")),
                    (RightPanel::Accessibility, tl!("Accessibility Checker")),
                    (RightPanel::Search, tl!("Search")),
                    (RightPanel::Compare, tl!("Compare")),
                ] {
                    if ui.radio(app.right == Some(p), tl!(label)).clicked() {
                        app.choose_right_panel(Some(p));
                    }
                }
            });
        });
        ui.menu_button(tl!("Help"), |ui| crate::commands::registry_menu(app, ui, "Help"));
    });
}

pub fn right_rail(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let Some((index, id)) = app.active_ids() else { return };
    let Some(doc) = app.session.get(id) else { return };
    let has_signatures = doc.is_signed();
    let has_check = app.a11y.report.as_ref().is_some_and(|(d, _)| *d == id);
    let (has_comments, has_outline, has_fields, has_layers, has_files) = (
        !doc.info.annotations.is_empty(),
        !doc.info.outline.is_empty(),
        !doc.info.fields.is_empty(),
        !doc.info.layers.is_empty(),
        !doc.info.attachments.is_empty(),
    );
    let page_count = doc.info.pages.len();
    let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
    egui::Panel::right("rail")
        .resizable(false)
        .exact_size(48.0)
        .frame(egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::symmetric(6, 8)).stroke(Stroke::new(1.0, t.divider)))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            let mut rail_button = |ui: &mut egui::Ui, panel: RightPanel, icon: &str, tip: &str, has: bool| {
                let selected = app.right == Some(panel);
                let r = icons::button(ui, icon, 34.0, selected, tl!(tip));
                if has && !selected {
                    let c = r.rect.right_top() + vec2(-8.0, 8.0);
                    ui.painter().circle_filled(c, 3.0, t.accent);
                }
                if r.clicked() {
                    app.choose_right_panel(if selected { None } else { Some(panel) });
                }
            };
            rail_button(ui, RightPanel::Comments, "message-square-text", "Comments", has_comments);
            rail_button(ui, RightPanel::Bookmarks, "bookmark", "Bookmarks", has_outline);
            rail_button(ui, RightPanel::Pages, "files", "Page thumbnails", false);
            rail_button(ui, RightPanel::Fields, "text-cursor-input", "Form fields", has_fields);
            rail_button(ui, RightPanel::Layers, "layers", "Layers", has_layers);
            rail_button(ui, RightPanel::Attachments, "paperclip", "Attachments", has_files);
            rail_button(ui, RightPanel::Signatures, "signature", "Signatures", has_signatures);
            if has_check {
                rail_button(ui, RightPanel::Accessibility, "accessibility", "Accessibility Checker", false);
            }

            // Page navigation cluster at the bottom (as in Acrobat's rail).
            let view = &mut app.views[index];
            let mut page_display = None;
            ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                let tip = crate::commands::key_tip(ui.ctx(), tl!("Zoom out ({key})"), crate::commands::ZOOM_OUT);
                if icons::button(ui, "zoom-out", 32.0, false, &tip).clicked() {
                    view.zoom_step(false);
                }
                let tip = crate::commands::key_tip(ui.ctx(), tl!("Zoom in ({key})"), crate::commands::ZOOM_IN);
                if icons::button(ui, "zoom-in", 32.0, false, &tip).clicked() {
                    view.zoom_step(true);
                }
                // Between rotate and zoom, as in Acrobat. Its command runs once `view` is free.
                let tip = crate::i18n::fmt(tl!("Page display: {layout}"), &[("layout", tl!(view.layout.label()))]);
                let resp = icons::button(ui, view.layout.icon(), 32.0, false, &tip);
                page_display = egui::Popup::menu(&resp)
                    .show(|ui| {
                        ui.set_min_width(220.0);
                        rail_view_menu(ui, view)
                    })
                    .and_then(|r| r.inner);
                let tip = crate::commands::key_tip(ui.ctx(), tl!("Rotate view clockwise ({key})"), crate::commands::ROTATE_CW);
                if icons::button(ui, "rotate-cw", 32.0, false, &tip).clicked() {
                    view.rotate_view(true);
                }
                // Not `columns-2` while fitting the page: that is the two-page view's icon.
                let fit_icon = if view.fit == Fit::Width { "maximize" } else { "maximize-2" };
                if icons::button(ui, fit_icon, 32.0, false, tl!("Toggle fit page / fit width")).clicked() {
                    view.fit = if view.fit == Fit::Width { Fit::Page } else { Fit::Width };
                    view.goto = Some((view.current, 0.0));
                }
                ui.label(egui::RichText::new(format!("{:.0}%", view.zoom * 100.0)).font(theme::regular(10.5)).color(t.text_faint));
                ui.add_space(6.0);
                if icons::button(ui, "chevron-down", 30.0, false, tl!("Next page")).clicked() {
                    view.step_page(true);
                }
                if icons::button(ui, "chevron-up", 30.0, false, tl!("Previous page")).clicked() {
                    view.step_page(false);
                }
                ui.label(egui::RichText::new(page_count.to_string()).font(theme::regular(11.0)).color(t.text_muted));
                let edit = egui::TextEdit::singleline(&mut view.page_input)
                    .id(egui::Id::new("page-input"))
                    .desired_width(34.0)
                    .horizontal_align(Align::Center)
                    .font(theme::medium(12.0))
                    .margin(vec2(2.0, 4.0));
                let r = egui::Frame::NONE
                    .fill(t.field)
                    .stroke(Stroke::new(1.0, t.border))
                    .corner_radius(CornerRadius::same(5))
                    .show(ui, |ui| ui.add(edit))
                    .inner
                    .on_hover_text(tl!("Current page — type a page number or label (such as iv) and press Enter"));
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    // A page label first (logical page numbers, as Acrobat), then a number.
                    let typed = view.page_input.clone();
                    if !view.go_to_typed(&typed, &labels) {
                        view.page_input = (view.current + 1).to_string();
                    }
                }
            });
            if let Some(id) = page_display {
                app.execute(id);
            }
        });
}

/// View ▸ Page display and the rail's Page display menu: the layouts and the cover page.
/// Returns the command a click asks for, for the caller to run.
fn page_display_menu(ui: &mut egui::Ui, view: &DocView) -> Option<&'static str> {
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(tl!("Page display")).color(t.text_faint).small());
    let mut picked = None;
    for l in PageLayout::ORDER {
        if ui.radio(view.layout == l, tl!(l.label())).clicked() {
            picked = Some(l.command());
        }
    }
    let mut cover = view.cover;
    if ui.add_enabled(view.cover_applies(), egui::Checkbox::new(&mut cover, tl!("Show cover page in two-page view"))).changed() {
        picked = Some("view.layout.cover");
    }
    if picked.is_some() {
        ui.close();
    }
    picked
}

/// The rail's Page display menu: Acrobat's view choices around the page display. Zoom changes
/// apply at once; the command a click asks for is returned for the caller to run.
fn rail_view_menu(ui: &mut egui::Ui, view: &mut DocView) -> Option<&'static str> {
    let mut picked = None;
    for (id, label) in [("view.fit_width_scrolling", "Fit to width scrolling"), ("view.fit_one_page", "Fit one full page")] {
        if ui.button(tl!(label)).clicked() {
            picked = Some(id);
        }
    }
    ui.separator();
    if ui.button(tl!("Actual size")).clicked() {
        view.set_zoom(1.0);
        ui.close();
    }
    if ui.button(tl!("Zoom to page level")).clicked() {
        view.set_fit(Fit::Page);
        ui.close();
    }
    if ui.button(tl!("Fit visible")).clicked() {
        picked = Some("view.fit_visible");
    }
    ui.separator();
    picked = page_display_menu(ui, view).or(picked);
    ui.separator();
    for (id, label) in [("view.read_mode", "Read mode"), ("view.full_screen", "Full screen mode")] {
        if ui.button(tl!(label)).clicked() {
            picked = Some(id);
        }
    }
    if picked.is_some() {
        ui.close();
    }
    picked
}

#[cfg(test)]
mod tab_widths {
    use super::tab_cap;

    /// Tabs that fit keep their own widths.
    #[test]
    fn tabs_that_fit_do_not_shrink() {
        assert_eq!(tab_cap(&[200.0, 150.0], 400.0, 4.0, 110.0), None);
        assert_eq!(tab_cap(&[], 0.0, 4.0, 110.0), None);
    }

    /// Too wide: narrow tabs keep their width, the wide ones share the rest equally, and
    /// together they fill the strip exactly.
    #[test]
    fn wide_tabs_share_what_narrow_ones_leave() {
        let natural = [120.0, 300.0, 300.0];
        let cap = tab_cap(&natural, 500.0, 4.0, 110.0).expect("must shrink");
        let used: f32 = natural.iter().map(|w| w.min(cap)).sum::<f32>() + 2.0 * 4.0;
        assert!((used - 500.0).abs() < 0.01, "{cap} uses {used}");
        assert!((cap - 186.0).abs() < 0.01, "{cap}");
    }

    /// Arrows appear only when the tabs can't fit even at their narrowest.
    #[test]
    fn arrows_only_when_the_narrowest_tabs_overflow() {
        use super::tabs_overflow;
        assert!(!tabs_overflow(&[250.0; 3], 600.0, 4.0, 180.0), "three shrunk tabs fit");
        assert!(tabs_overflow(&[250.0; 4], 600.0, 4.0, 180.0), "four don't, even at 180");
        assert!(!tabs_overflow(&[100.0; 5], 600.0, 4.0, 180.0), "narrow tabs fit as they are");
        assert!(!tabs_overflow(&[], 0.0, 4.0, 180.0));
    }

    /// Tabs never get narrower than the minimum: past it, the strip scrolls instead.
    #[test]
    fn tabs_stop_at_their_minimum() {
        assert_eq!(tab_cap(&[250.0; 12], 600.0, 4.0, 110.0), Some(110.0));
        assert_eq!(tab_cap(&[250.0; 3], 0.0, 4.0, 110.0), Some(110.0));
    }
}
