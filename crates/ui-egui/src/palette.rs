//! ⌘K command palette: every registered command (with its shortcut) and every tool in the
//! catalogue, with a fuzzy-ish substring match.

use egui::{Align2, CornerRadius, Rect, Sense, Stroke, vec2};
use pdfcraft_engine::catalog::{Availability, TOOL_GROUPS};

use crate::theme::{self, Tokens};
use crate::{LeftPanel, PdfCraftApp, icons};

struct Hit {
    /// The tool panel to open (tools and catalogue items); `None` for plain commands.
    group: Option<&'static str>,
    label: String,
    detail: String,
    icon: &'static str,
    command: Option<&'static str>,
    ready: bool,
}

fn score(hay: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let h = hay.to_lowercase();
    if let Some(p) = h.find(needle) {
        return Some(p);
    }
    // A right-to-left language's labels are in display order (their words run the other way)
    // while the query is in typing order, so several words match one by one.
    if crate::i18n::current().rtl() && needle.contains(char::is_whitespace) {
        let first = needle.split_whitespace().map(|word| h.find(word)).try_fold(usize::MAX, |first, p| p.map(|p| first.min(p)));
        if let Some(first) = first {
            return Some(first);
        }
    }
    // Subsequence match as a fallback.
    let mut it = h.chars();
    needle.chars().all(|c| it.any(|x| x == c)).then_some(100)
}

pub fn show(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let palette_rect_id = egui::Id::new("palette_rect");
    if !app.palette_open {
        return;
    }
    let t = Tokens::get(ctx);
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.palette_open = false;
        return;
    }
    let q = app.palette_query.trim().to_lowercase();
    let mut hits: Vec<(usize, Hit)> = Vec::new();
    let active = app.active_ids().map(|(_, id)| id);
    for spec in pdfcraft_engine::commands::COMMANDS {
        let label = pdfcraft_engine::commands::current_label(spec, &app.session, active);
        let translated = crate::i18n::command_label(&label);
        if let Some(s) = score(&translated, &q).or_else(|| score(&label, &q)).or_else(|| score(spec.id, &q).map(|s| s + 50)) {
            hits.push((
                s,
                Hit {
                    group: None,
                    label: translated,
                    detail: spec
                        .shortcut
                        .map(|k| crate::commands::shortcut_label(ctx, k))
                        .unwrap_or_else(|| tl!(spec.menu.unwrap_or("Command")).to_string()),
                    icon: spec.icon,
                    command: Some(spec.id),
                    ready: app.command_enabled(spec),
                },
            ));
        }
    }
    for g in TOOL_GROUPS {
        if let Some(s) = score(tl!(g.label), &q).or_else(|| score(g.label, &q)) {
            hits.push((
                s,
                Hit {
                    group: Some(g.id),
                    label: tl!(g.label).to_string(),
                    detail: tl!("Tool").into(),
                    icon: g.icon,
                    command: None,
                    ready: g.availability == Availability::Ready,
                },
            ));
        }
        for sec in g.sections {
            for i in sec.items {
                if pdfcraft_engine::commands::command(i.command).is_some() {
                    continue; // listed above as a command
                }
                if let Some(s) = score(tl!(i.label), &q).or_else(|| score(i.label, &q)).or_else(|| score(i.command, &q).map(|s| s + 50)) {
                    hits.push((
                        s + 1,
                        Hit {
                            group: Some(g.id),
                            label: tl!(i.label).to_string(),
                            detail: tl!(g.label).into(),
                            icon: i.icon,
                            command: Some(i.command),
                            ready: i.availability == Availability::Ready,
                        },
                    ));
                }
            }
        }
    }
    hits.sort_by_key(|(s, h)| (*s, !h.ready));
    hits.truncate(12);

    let screen = ctx.content_rect();

    let previous_rect = ctx.data(|data| data.get_temp::<Rect>(palette_rect_id));

    let outside_click = ctx.input(|input| {
        input.pointer.button_pressed(egui::PointerButton::Primary)
            && input.pointer.interact_pos().is_some_and(|pos| previous_rect.is_some_and(|rect| !rect.contains(pos)))
    });

    if outside_click {
        app.palette_open = false;
        app.palette_query.clear();
        return;
    }
    let mut chosen: Option<(Option<&'static str>, Option<&'static str>)> = None;
    let area_response = egui::Area::new(egui::Id::new("palette"))
        .order(egui::Order::Foreground)
        .pivot(Align2::CENTER_TOP)
        .fixed_pos(egui::pos2(screen.center().x, screen.top() + 96.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).corner_radius(CornerRadius::same(12)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                ui.set_width(560.0);
                ui.horizontal(|ui| {
                    ui.add(icons::image("search", 18.0, t.text_muted));
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut app.palette_query)
                            .hint_text(tl!("Search tools and commands…"))
                            .frame(egui::Frame::NONE)
                            .font(theme::regular(15.0))
                            .desired_width(f32::INFINITY),
                    );
                    // Enter runs the top hit. The field keeps focus (requested every frame), so
                    // check while it is focused as well as when focus is lost.
                    if (r.has_focus() || r.lost_focus())
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && let Some((_, h)) = hits.first()
                    {
                        chosen = Some((h.command, h.group));
                    }
                    r.request_focus();
                });
                ui.separator();
                for (_, h) in &hits {
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(6), t.hover);
                    }
                    icons::paint(ui, Rect::from_min_size(rect.min + vec2(8.0, 9.0), vec2(18.0, 18.0)), h.icon, 17.0, t.icon);
                    // Display is translated; matching above already considered both languages.
                    let shown = crate::i18n::command_label(&h.label);
                    let shown_detail = tl!(&h.detail).to_string();
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, h.ready, &shown));
                    let fg = if h.ready { t.text } else { t.text_faint };
                    ui.painter().text(rect.left_center() + vec2(36.0, 0.0), Align2::LEFT_CENTER, shown, theme::regular(13.5), fg);
                    ui.painter().text(rect.right_center() - vec2(10.0, 0.0), Align2::RIGHT_CENTER, shown_detail, theme::regular(12.0), t.text_faint);
                    if resp.clicked() {
                        chosen = Some((h.command, h.group));
                    }
                }
                if hits.is_empty() {
                    ui.label(egui::RichText::new(tl!("No matching tools")).color(t.text_muted));
                }
                ui.add_space(2.0);
                let _ = Stroke::NONE;
            });
        });

    ctx.data_mut(|data| {
        data.insert_temp(palette_rect_id, area_response.response.rect);
    });

    if let Some((command, group)) = chosen {
        app.palette_open = false;
        app.palette_query.clear();
        if let Some(g) = group {
            app.left_open = true;
            app.left = LeftPanel::Tool(g);
        }
        if let Some(c) = command {
            app.run_command(c);
        }
    }
}
