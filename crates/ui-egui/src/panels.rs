//! Left tool panel (All tools + tool sub-panels, generated from the engine catalogue) and the
//! right-hand panels (Comments, Bookmarks, Pages, Fields, Layers, Attachments).

use egui::{Align, Align2, Color32, CornerRadius, Layout, Rect, Sense, Stroke, pos2, vec2};
use pdfcraft_engine::catalog::{self, Availability, TOOL_GROUPS, ToolGroup};
use pdfcraft_render::{DocInfo, FieldKind, OutlineItem};

use crate::theme::{self, Tokens};
use crate::{LeftPanel, PdfCraftApp, RightPanel, icons, widgets};

const COLLAPSED_TOOLS: usize = 14;

fn hue(g: &ToolGroup) -> Color32 {
    Color32::from_rgb(g.hue[0], g.hue[1], g.hue[2])
}

pub fn left_panel(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::left("tool_panel")
        .resizable(false)
        .exact_size(272.0)
        .frame(
            egui::Frame::NONE
                .fill(t.panel)
                .inner_margin(egui::Margin { left: 14, right: 10, top: 12, bottom: 10 })
                .stroke(Stroke::new(1.0, t.divider)),
        )
        .show(ui, |ui| match app.left {
            LeftPanel::AllTools => all_tools(app, ui, &t),
            LeftPanel::Tool(id) => match catalog::group(id) {
                Some(g) => tool_detail(app, ui, &t, g),
                None => app.left = LeftPanel::AllTools,
            },
        });
}

fn panel_header(ui: &mut egui::Ui, t: &Tokens, title: &str, back: bool) -> (bool, bool) {
    let mut go_back = false;
    let mut close = false;
    ui.horizontal(|ui| {
        if back && icons::button(ui, "chevron-left", 26.0, false, tl!("Back to all tools")).clicked() {
            go_back = true;
        }
        ui.label(egui::RichText::new(tl!(title)).font(theme::semibold(15.5)).color(t.text));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icons::button(ui, "x", 26.0, false, tl!("Close panel")).clicked() {
                close = true;
            }
        });
    });
    ui.add_space(6.0);
    (go_back, close)
}

fn all_tools(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let (_, close) = panel_header(ui, t, "All tools", false);
    if close {
        app.left_open = false;
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let shown = if app.all_tools_expanded { TOOL_GROUPS.len() } else { COLLAPSED_TOOLS.min(TOOL_GROUPS.len()) };
        for g in &TOOL_GROUPS[..shown] {
            if tool_row(ui, t, g).clicked() {
                app.left = LeftPanel::Tool(g.id);
            }
        }
        ui.add_space(4.0);
        let more = if app.all_tools_expanded { "View less" } else { "View more" };
        if ui.add(egui::Label::new(egui::RichText::new(tl!(more)).color(t.accent_text).font(theme::medium(13.0))).sense(Sense::click())).clicked() {
            app.all_tools_expanded = !app.all_tools_expanded;
        }
    });
}

fn tool_row(ui: &mut egui::Ui, t: &Tokens, g: &ToolGroup) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tl!(g.label)));
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(6), t.hover);
    }
    icons::paint(ui, Rect::from_min_size(rect.min + vec2(6.0, 7.0), vec2(20.0, 20.0)), g.icon, 19.0, hue(g));
    ui.painter().text(rect.left_center() + vec2(36.0, 0.0), Align2::LEFT_CENTER, tl!(g.label), theme::regular(13.5), t.text);
    match (g.badge, g.availability) {
        (Some(b), _) => {
            let font = theme::semibold(9.5);
            let badge = tl!(b);
            let w = ui.fonts_mut(|f| f.layout_no_wrap(badge.to_string(), font.clone(), Color32::WHITE).size().x);
            let r = Rect::from_center_size(rect.right_center() - vec2(w / 2.0 + 10.0, 0.0), vec2(w + 10.0, 16.0));
            ui.painter().rect_filled(r, CornerRadius::same(4), t.badge_new);
            ui.painter().text(r.center(), Align2::CENTER_CENTER, badge, font, Color32::WHITE);
        }
        // Planned tools: a quiet milestone hint instead of a chip, so the list stays calm.
        (None, Availability::Planned(m)) if resp.hovered() => {
            ui.painter().text(
                rect.right_center() - vec2(10.0, 0.0),
                Align2::RIGHT_CENTER,
                format!("{} · {m}", tl!("Planned")),
                theme::medium(10.5),
                t.text_faint,
            );
        }
        _ => {}
    }
    let tip = match g.availability {
        Availability::Ready => tl!("Available").to_string(),
        Availability::Planned(m) => format!("{} {m} — {}", tl!("Planned for milestone"), tl!("open to see what it will include")),
        Availability::Provider => tl!("Optional: needs an AI provider you configure").into(),
    };
    resp.on_hover_text(tip)
}

fn tool_detail(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens, g: &'static ToolGroup) {
    let (back, close) = panel_header(ui, t, g.label, true);
    if back {
        app.left = LeftPanel::AllTools;
        app.mode = crate::Mode::AllTools;
    }
    if close {
        app.left_open = false;
    }
    if let Availability::Planned(m) = g.availability {
        egui::Frame::NONE.fill(t.accent_soft).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                egui::RichText::new(format!("{} {m}. {}", tl!("Coming in milestone"), tl!("Items marked Ready work today.")))
                    .color(t.text)
                    .font(theme::regular(12.0)),
            );
        });
    }
    if g.id == "measure" {
        crate::measure_ui::panel(app, ui, t);
        return;
    }
    // Add a stamp: the palette.
    if g.id == "stamp" {
        stamp_palette(app, ui, t);
        return;
    }
    // Prepare a form: Preview fills the form as a reader would; Edit goes back.
    if g.id == "form" {
        ui.horizontal(|ui| {
            let label = if app.form_preview { "Edit fields" } else { "Preview" };
            if widgets::pill_button(ui, tl!(label), app.form_preview).on_hover_text(tl!("Try the form as people filling it in will see it")).clicked()
            {
                app.form_preview = !app.form_preview;
                if app.form_preview {
                    app.quick_tool = crate::QuickTool::Select;
                }
            }
        });
        ui.add_space(6.0);
    }
    // Edit a PDF shows Format text at the top while text is selected or being added.
    if g.id == "edit" {
        if app.quick_tool == crate::QuickTool::EditText {
            let count = app.active_ids().map_or(0, |(i, _)| app.views[i].objects.count());
            if count > 0 {
                ui.label(crate::i18n::trn(crate::i18n::current(), count as u64, "{n} object selected", "{n} objects selected"));
                ui.label(tl!("Drag the selection to move it. Esc clears the selection."));
            } else {
                ui.label(tl!("Shift-click to select several objects. Drag empty space to select an area."));
            }
            ui.add_space(6.0);
        }
        format_section(app, ui, t);
    }
    let mut run = None;
    // Redact a PDF has Acrobat's footer: Clear all / Redact all.
    let footer = g.id == "redact";
    let marks = if footer { app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(0, |d| d.redaction_marks()) } else { 0 };
    let list_h = if footer { (ui.available_height() - 52.0).max(80.0) } else { ui.available_height() };
    egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(list_h).show(ui, |ui| {
        for s in g.sections {
            widgets::section_title(ui, tl!(s.title));
            for item in s.items {
                if g.id == "fill_sign" && (item.command.starts_with("sign.fill.signature") || item.command.starts_with("sign.fill.initials")) {
                    continue;
                }
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tl!(item.label)));
                let ready = item.availability == Availability::Ready;
                if resp.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::same(6), t.hover);
                }
                let fg = if ready { t.text } else { t.text_muted };
                icons::paint(
                    ui,
                    Rect::from_min_size(rect.min + vec2(6.0, 8.0), vec2(18.0, 18.0)),
                    item.icon,
                    17.0,
                    if ready { hue(g) } else { t.text_faint },
                );
                ui.painter().text(rect.left_center() + vec2(34.0, 0.0), Align2::LEFT_CENTER, tl!(item.label), theme::regular(13.0), fg);
                let (chip, fill, cfg) = match item.availability {
                    Availability::Ready => (tl!("Ready"), Color32::from_rgb(0xDD, 0xF3, 0xE4), Color32::from_rgb(0x1E, 0x7B, 0x43)),
                    Availability::Planned(m) => (m, t.pressed, t.text_muted),
                    Availability::Provider => ("AI", t.pressed, t.text_muted),
                };
                let font = theme::semibold(9.5);
                let w = ui.fonts_mut(|f| f.layout_no_wrap(tl!(chip).to_string(), font.clone(), cfg).size().x);
                let r = Rect::from_center_size(rect.right_center() - vec2(w / 2.0 + 10.0, 0.0), vec2(w + 10.0, 16.0));
                ui.painter().rect_filled(r, CornerRadius::same(4), fill);
                ui.painter().text(r.center(), Align2::CENTER_CENTER, tl!(chip), font, cfg);
                if resp.on_hover_text(item.command).clicked() {
                    run = Some(item.command);
                }
            }
        }
        if g.id == "fill_sign" {
            widgets::section_title(ui, tl!("Sign yourself"));
            if let Some(cmd) = crate::fill_sign::signature_entries(ui, app, t) {
                run = Some(cmd);
            }
        }
    });
    if footer {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if marks > 0 {
                ui.label(
                    egui::RichText::new(if marks == 1 {
                        crate::i18n::fmt(tl!("1 mark"), &[])
                    } else {
                        crate::i18n::fmt(tl!("{n} marks"), &[("n", &marks.to_string())])
                    })
                    .color(t.text_muted),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled_ui(marks > 0, |ui| widgets::pill_button(ui, tl!("Redact all"), true)).inner.clicked() {
                    run = Some("redact.apply");
                }
                if ui.add_enabled_ui(marks > 0, |ui| widgets::pill_button(ui, tl!("Clear all"), false)).inner.clicked() {
                    run = Some("redact.clear");
                }
            });
        });
    }
    if let Some(cmd) = run {
        app.run_command(cmd);
    }
}

/// Add a stamp: Dynamic, Sign Here and Standard Business stamps; click one, then click on the
/// page to place it.
fn stamp_palette(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    use pdfcraft_engine::{StampGroup, StampKind};
    ui.label(egui::RichText::new(tl!("Choose a stamp, then click on the page to place it.")).small().color(t.text_faint));
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (group, title) in [
            (StampGroup::Dynamic, tl!("Dynamic")),
            (StampGroup::SignHere, tl!("Sign Here")),
            (StampGroup::StandardBusiness, tl!("Standard Business")),
        ] {
            widgets::section_title(ui, title);
            for kind in StampKind::ALL.into_iter().filter(|k| k.group() == group) {
                let active = app.quick_tool == crate::QuickTool::Stamp(kind);
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, kind.label()));
                if active {
                    ui.painter().rect_filled(rect, CornerRadius::same(6), t.accent_soft);
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::same(6), t.hover);
                }
                let [r, g, b] = kind.color().map(|v| (v * 255.0) as u8);
                let col = Color32::from_rgb(r, g, b);
                let chip = Rect::from_min_size(rect.min + vec2(8.0, 5.0), vec2((rect.width() - 16.0).min(200.0), 28.0));
                if group == StampGroup::SignHere {
                    let tip = chip.height() * 0.45;
                    let pts = vec![
                        chip.left_center(),
                        chip.left_top() + vec2(tip, 0.0),
                        chip.right_top(),
                        chip.right_bottom(),
                        chip.left_bottom() + vec2(tip, 0.0),
                    ];
                    ui.painter().add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
                    ui.painter().text(
                        chip.center() + vec2(tip / 2.0, 0.0),
                        Align2::CENTER_CENTER,
                        kind.label(),
                        theme::semibold(11.0),
                        Color32::WHITE,
                    );
                } else {
                    ui.painter().rect(chip, CornerRadius::same(5), col.gamma_multiply(0.1), Stroke::new(1.5, col), egui::StrokeKind::Inside);
                    let y = if group == StampGroup::Dynamic { chip.center().y - 4.0 } else { chip.center().y };
                    ui.painter().text(egui::pos2(chip.center().x, y), Align2::CENTER_CENTER, kind.label(), theme::semibold(11.0), col);
                    if group == StampGroup::Dynamic {
                        ui.painter().text(
                            egui::pos2(chip.center().x, chip.bottom() - 6.0),
                            Align2::CENTER_CENTER,
                            "By name at time, date",
                            theme::regular(7.5),
                            col,
                        );
                    }
                }
                if resp.clicked() {
                    app.quick_tool = crate::QuickTool::Stamp(kind);
                }
            }
        }
        crate::stamps_ui::palette_section(app, ui, t);
    });
}

/// Edit a PDF ▸ Format text: for the selected added text (one undoable change), or the style
/// new text gets.
fn format_section(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    // Editing a paragraph of existing text: its formatting, applied as it changes.
    if let Some((i, _)) = app.active_ids()
        && let Some(ed) = app.views[i].line_editor.clone()
    {
        let mut ed = ed;
        let mut changed = false;
        if let Some(look) = crate::content_ui::format_panel(ui, t, &ed.look) {
            ed.look = look;
            changed = true;
        }
        changed |= crate::edit_text_ui::extras_panel(ui, &mut ed.extras);
        if changed {
            let edit = pdfcraft_engine::Edit::EditTextBlock { page: ed.page, block: ed.block, text: ed.text.clone(), style: ed.style() };
            if app.apply_edit(edit) {
                ed.applied();
                if let Some(doc) = app.session.get(app.views[i].id)
                    && let Some(block) = doc.text_blocks(ed.page).get(ed.block)
                {
                    ed.refresh_source(block);
                }
            }
            app.views[i].line_editor = Some(ed);
        }
        ui.add_space(6.0);
        ui.separator();
        return;
    }
    let image = app.active_ids().and_then(|(i, id)| {
        let (page, index) = app.views[i].content.selected?;
        let doc = app.session.get(id)?;
        match &doc.added.iter().filter(|a| a.page == page).nth(index)?.content {
            pdfcraft_engine::AddedContent::Image(img) => Some((page, index, img.clone())),
            _ => None,
        }
    });
    if let Some((page, index, img)) = image {
        match crate::content_ui::image_panel(ui, t, &img) {
            Some(crate::content_ui::ImageAction::Update(content)) => {
                app.apply_edit(pdfcraft_engine::Edit::UpdateContent { page, index, content });
            }
            Some(crate::content_ui::ImageAction::Replace) => app.replace_image_dialog(page, index),
            None => {}
        }
        ui.add_space(6.0);
        ui.separator();
        return;
    }
    let selected = app.active_ids().and_then(|(i, id)| {
        let (page, index) = app.views[i].content.selected?;
        let doc = app.session.get(id)?;
        let a = doc.added.iter().filter(|a| a.page == page).nth(index)?;
        match &a.content {
            pdfcraft_engine::AddedContent::Text(text) => Some((page, index, text.clone())),
            _ => None,
        }
    });
    match selected {
        Some((page, index, text)) => {
            if let Some(style) = crate::content_ui::format_panel(ui, t, &text) {
                app.text_style = pdfcraft_engine::AddedText { text: String::new(), rect: [0.0; 4], ..style.clone() };
                app.apply_edit(pdfcraft_engine::Edit::UpdateContent { page, index, content: pdfcraft_engine::AddedContent::Text(style) });
            }
        }
        None if app.quick_tool == crate::QuickTool::AddText => {
            crate::content_ui::hint(ui, t);
            if let Some(style) = crate::content_ui::format_panel(ui, t, &app.text_style) {
                app.text_style = style;
            }
        }
        None => return,
    }
    ui.add_space(6.0);
    ui.separator();
}

// ───────────────────────────────────────────────────────────────────────────── right panels

pub(crate) enum Nav {
    Page(usize),
    /// A bookmark's destination: its page and where on it.
    Dest(usize, pdfcraft_render::DestView),
    Flash(usize, [f32; 4]),
}

pub fn right_panel(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let Some((index, id)) = app.active_ids() else { return };
    let Some(panel) = app.right else { return };
    let mut nav: Option<Nav> = None;
    let mut close = false;
    let mut toggle_layer: Option<(usize, bool)> = None;
    let mut attachment_action: Option<(usize, bool)> = None; // (index, open instead of save)
    let mut bm_action: Option<BmAction> = None;
    let mut bm_expand: Option<usize> = None;
    let mut panel_edit: Option<pdfcraft_engine::Edit> = None;
    let mut field_properties = None;
    let mut panel_command: Option<&'static str> = None;
    let mut sig_action: Option<crate::sign_ui::PanelAction> = None;
    let mut a11y_action: Option<crate::a11y_ui::PanelAction> = None;
    let mut compare_action: Option<crate::compare_ui::PanelAction> = None;
    let mut bm_rename = app.bookmark_rename.clone();
    let bm_editable = app.session.get(id).is_some_and(|d| d.allows_assembly() && d.read_only_reason.is_none());
    // Prepare a form is open: the Fields panel orders tabs.
    let preparing = app.is_preparing();
    {
        let Some(doc) = app.session.get(id) else { return };
        let info = &doc.info;
        let view = &mut app.views[index];
        let prefs = &app.comment_prefs;
        let sig_expanded = &mut app.sig_expanded;
        let a11y = &mut app.a11y;
        let compare = &app.compare;
        let comment_allowed = doc.allows_annotation();
        // A dialog or the palette owns the keyboard: the panel leaves Escape to it.
        let modal = app.dialog.is_some() || app.palette_open;
        egui::Panel::right("right_panel")
            .resizable(true)
            .default_size(330.0)
            .size_range(260.0..=520.0)
            .frame(
                egui::Frame::NONE
                    .fill(t.panel)
                    .inner_margin(egui::Margin { left: 14, right: 12, top: 12, bottom: 8 })
                    .stroke(Stroke::new(1.0, t.divider)),
            )
            .show(ui, |ui| {
                let (title, count) = match panel {
                    RightPanel::Comments => ("Comments", Some(crate::comments_panel::count(info))),
                    RightPanel::Bookmarks => ("Bookmarks", None),
                    RightPanel::Pages => ("Pages", Some(info.pages.len())),
                    RightPanel::Fields => ("Fields", Some(info.fields.len())),
                    RightPanel::Layers => ("Layers", Some(info.layers.len())),
                    RightPanel::Attachments => ("Attachments", Some(info.attachments.len())),
                    RightPanel::Signatures => ("Signatures", Some(doc.signatures.iter().filter(|s| s.signed).count())),
                    RightPanel::Accessibility => ("Accessibility Checker", None),
                    RightPanel::Search => ("Search", None),
                    RightPanel::Compare => ("Compare", compare.as_ref().filter(|c| c.new == id).map(|c| c.result.changes.len())),
                };
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(tl!(title)).font(theme::semibold(15.5)));
                    if let Some(c) = count {
                        ui.label(egui::RichText::new(c.to_string()).font(theme::medium(13.0)).color(t.text_faint));
                    }
                    if panel == RightPanel::Pages && view.selected.len() > 1 {
                        let n = view.selected.len().to_string();
                        let picked = crate::i18n::fmt(tl!("{n} pages selected"), &[("n", &n)]);
                        ui.label(egui::RichText::new(picked).font(theme::medium(12.0)).color(t.accent_text));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icons::button(ui, "x", 26.0, false, tl!("Close")).clicked() {
                            close = true;
                        }
                        // Right to left: close, "…", filter, search (Acrobat's order left to right).
                        if panel == RightPanel::Comments {
                            panel_command = crate::comments_panel::header_controls(ui, info, view, doc.comments_hidden());
                        }
                        if panel == RightPanel::Comments
                            && icons::button(ui, "search", 26.0, view.comments.search.is_some(), tl!("Search comments")).clicked()
                        {
                            view.comments.search = match view.comments.search {
                                Some(_) => None,
                                None => Some(String::new()),
                            };
                            view.comments.search_focus = true;
                        }
                        if panel == RightPanel::Bookmarks && (bm_editable || !info.outline.is_empty()) {
                            let more = icons::button(ui, "ellipsis", 26.0, false, tl!("Bookmark options"));
                            egui::Popup::menu(&more).show(|ui| {
                                ui.set_min_width(200.0);
                                if bm_editable && ui.button(tl!("New bookmarks from structure")).clicked() {
                                    bm_action = Some(BmAction::FromStructure);
                                    ui.close();
                                }
                                if !info.outline.is_empty() {
                                    for (levels, label) in [
                                        (usize::MAX, tl!("Expand all bookmarks")),
                                        (1, tl!("Expand top-level bookmarks")),
                                        (0, tl!("Collapse all bookmarks")),
                                    ] {
                                        if ui.button(label).clicked() {
                                            bm_expand = Some(levels);
                                            ui.close();
                                        }
                                    }
                                }
                            });
                        }
                        let new_tip = crate::commands::command_tip(ui.ctx(), tl!("New bookmark ({key})"), "bookmark.add");
                        if panel == RightPanel::Bookmarks && bm_editable && icons::button(ui, "bookmark-plus", 26.0, false, &new_tip).clicked() {
                            bm_action = Some(BmAction::New);
                        }
                    });
                });
                ui.add_space(6.0);
                let search_id = egui::Id::new(("bookmark_search", id.0));
                let mut bookmark_query = ui.data(|d| d.get_temp::<String>(search_id)).unwrap_or_default();
                if panel == RightPanel::Bookmarks && !info.outline.is_empty() {
                    ui.horizontal(|ui| {
                        const CLEAR: f32 = 26.0;
                        let label = ui.label(tl!("Search"));
                        let previous = bookmark_query.clone();
                        // The panel keeps whatever width its content used, so this row must fit
                        // exactly: any overflow would widen the panel again on every repaint.
                        let width = ui.available_width() - CLEAR - ui.spacing().item_spacing.x;
                        let response = ui.add(egui::TextEdit::singleline(&mut bookmark_query).desired_width(width)).labelled_by(label.id);
                        response.widget_info(|| {
                            let mut info = egui::WidgetInfo::text_edit(ui.is_enabled(), &previous, &bookmark_query, "");
                            info.label = Some(tl!("Search").to_string());
                            info
                        });
                        if icons::button(ui, "x", CLEAR, false, tl!("Clear")).clicked() {
                            bookmark_query.clear();
                        }
                    });
                    ui.data_mut(|d| d.insert_temp(search_id, bookmark_query.clone()));
                    ui.add_space(6.0);
                }
                let bookmark_query = pdfcraft_render::text::fold_case(bookmark_query.trim());
                let mut bookmark_matches =
                    if panel == RightPanel::Bookmarks { outline_matches(&info.outline, &bookmark_query) } else { std::collections::HashSet::new() };
                // A new bookmark must stay nameable even when its initial title doesn't match.
                if let Some((path, _)) = &bm_rename {
                    let mut ancestor = path.clone();
                    while !ancestor.is_empty() {
                        bookmark_matches.insert(ancestor.clone());
                        ancestor.pop();
                    }
                }
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match panel {
                    RightPanel::Comments => {
                        if let Some(e) = crate::comments_panel::show(ui, &t, info, view, prefs, comment_allowed, &mut nav) {
                            panel_edit = Some(e);
                        }
                    }
                    RightPanel::Bookmarks => {
                        if info.outline.is_empty() {
                            empty(ui, &t, "bookmark", "This document has no bookmarks.");
                        }
                        let mut ctx = OutlineCtx {
                            nav: &mut nav,
                            action: &mut bm_action,
                            rename: &mut bm_rename,
                            editable: bm_editable,
                            current: view.current,
                            expand: bm_expand,
                            matches: &bookmark_matches,
                            searching: !bookmark_query.is_empty(),
                        };
                        if ctx.searching && bookmark_matches.is_empty() && !info.outline.is_empty() {
                            ui.label(tl!("No matches."));
                        }
                        for (i, item) in info.outline.iter().enumerate() {
                            outline_item(ui, &t, info, item, &[i], info.outline.len(), &mut ctx);
                        }
                        if info.outline_more {
                            ui.label(tl!("Some bookmarks are not shown here: the panel lists a limited number, nested a limited depth."));
                        }
                    }
                    RightPanel::Pages => pages(ui, &t, info, view, modal, bm_editable, &mut nav),
                    RightPanel::Fields => field_properties = fields(ui, &t, info, &doc.form, preparing, &mut view.prepare, &mut nav, &mut panel_edit),
                    RightPanel::Layers => {
                        if info.layers.is_empty() {
                            empty(ui, &t, "layers", "This document has no layers.");
                        }
                        for (li, l) in info.layers.iter().enumerate() {
                            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, l.visible, &l.name));
                            if resp.hovered() {
                                ui.painter().rect_filled(rect, CornerRadius::same(6), t.hover);
                            }
                            icons::paint(
                                ui,
                                Rect::from_min_size(rect.min + vec2(4.0, 7.0), vec2(18.0, 18.0)),
                                if l.visible { "eye" } else { "eye-off" },
                                16.0,
                                t.icon,
                            );
                            let fg = if l.visible { t.text } else { t.text_faint };
                            ui.painter().text(rect.left_center() + vec2(32.0, 0.0), Align2::LEFT_CENTER, &l.name, theme::regular(13.0), fg);
                            if resp.on_hover_text(if l.visible { tl!("Hide layer") } else { tl!("Show layer") }).clicked() {
                                toggle_layer = Some((li, !l.visible));
                            }
                        }
                    }
                    RightPanel::Signatures => {
                        sig_action = crate::sign_ui::panel(ui, &t, &doc.signatures, sig_expanded);
                    }
                    RightPanel::Accessibility => {
                        a11y_action = crate::a11y_ui::panel(ui, &t, a11y, id);
                    }
                    RightPanel::Search => crate::search_ui::panel(ui, &t, view, info.pages.len()),
                    RightPanel::Compare => compare_action = crate::compare_ui::panel(ui, &t, compare, id),
                    RightPanel::Attachments => {
                        if info.attachments.is_empty() {
                            empty(ui, &t, "paperclip", "This document has no attachments.");
                        }
                        for (ai, a) in info.attachments.iter().enumerate() {
                            egui::Frame::NONE.inner_margin(egui::Margin::symmetric(4, 6)).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.add(icons::image("paperclip", 16.0, t.icon));
                                    ui.vertical(|ui| {
                                        ui.set_width(ui.available_width() - 70.0);
                                        ui.add(egui::Label::new(egui::RichText::new(&a.name).font(theme::medium(13.0))).truncate());
                                        let mut meta = a.size.map(human_size).unwrap_or_default();
                                        if let pdfcraft_render::AttachmentSource::Annotation { page, .. } = a.source {
                                            let on_page = crate::i18n::fmt(
                                                tl!("on page {label}"),
                                                &[("label", info.pages.get(page).map(|p| p.label.as_str()).unwrap_or("?"))],
                                            );
                                            meta = format!("{meta}  ·  {on_page}");
                                        }
                                        if let Some(d) = &a.description {
                                            meta = format!("{meta}  ·  {d}");
                                        }
                                        ui.add(egui::Label::new(egui::RichText::new(meta).color(t.text_faint).small()).truncate());
                                    });
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if icons::button(ui, "file-down", 28.0, false, tl!("Save attachment…")).clicked() {
                                            attachment_action = Some((ai, false));
                                        }
                                        if a.name.to_lowercase().ends_with(".pdf")
                                            && icons::button(ui, "file-input", 28.0, false, tl!("Open in a new tab")).clicked()
                                        {
                                            attachment_action = Some((ai, true));
                                        }
                                    });
                                });
                            });
                        }
                    }
                });
            });
    }
    if close {
        app.choose_right_panel(None);
    }
    if let Some(e) = panel_edit {
        app.apply_edit(e);
    }
    if let Some((name, widget)) = field_properties {
        app.open_field_props(&name, widget);
    }
    if let Some(c) = panel_command {
        app.run_command(c);
    }
    if let Some(a) = a11y_action {
        app.a11y_action(index, a);
    }
    if let Some(a) = compare_action {
        app.compare_action(index, a);
    }
    match sig_action {
        Some(crate::sign_ui::PanelAction::Validate) => app.run_command("sign.validate"),
        Some(crate::sign_ui::PanelAction::GoTo(p)) => app.views[index].go_to_page(p),
        Some(crate::sign_ui::PanelAction::Trust(c)) => app.trust_certificate(*c),
        Some(crate::sign_ui::PanelAction::ViewCertificate(chain)) => {
            app.cert_viewer = Some(crate::sign_ui::CertViewer { chain, selected: 0, tab: crate::sign_ui::CertTab::Summary });
            app.dialog = Some(crate::Dialog::CertificateViewer);
        }
        Some(crate::sign_ui::PanelAction::ViewSigned(len)) => app.view_signed_version(len),
        Some(crate::sign_ui::PanelAction::Sign(field)) => {
            let page = app.views[index].current;
            app.start_signing(page, None, Some(field), None);
        }
        Some(crate::sign_ui::PanelAction::ExportCertificate(c)) => {
            let pem = crate::sign_ui::certificate_pem(&c);
            app.write_files(&[(format!("{}.cer", c.display_name()), std::sync::Arc::new(pem.into_bytes()))], "Export certificate");
        }
        None => {}
    }
    if let Some((p, i)) = app.views.get_mut(index).and_then(|v| v.comments.default_request.take()) {
        app.make_comment_default(p, i);
    }
    if let Some((p, i)) = app.views.get_mut(index).and_then(|v| v.comments.props_request.take()) {
        app.open_comment_props(p, i);
    }
    app.bookmark_rename = bm_rename;
    if let Some(a) = bm_action {
        app.bookmark_action(a);
    }
    if let Some((ai, open)) = attachment_action {
        app.attachment_action(id, ai, open);
    }
    if let Some((layer, visible)) = toggle_layer
        && app.session.set_layer_visible(id, layer, visible)
    {
        app.views[index].invalidate_content();
    }
    match nav {
        Some(Nav::Page(p)) => app.views[index].go_to_page(p),
        Some(Nav::Dest(p, dest)) => match app.session.get(id) {
            Some(doc) => app.views[index].go_to_dest(p, dest, &doc.info),
            None => app.views[index].go_to_page(p),
        },
        Some(Nav::Flash(p, r)) => {
            let v = &mut app.views[index];
            v.go_to_page(p);
            v.flash = Some((p, r, 0.0));
        }
        None => {}
    }
}

fn empty(ui: &mut egui::Ui, t: &Tokens, icon: &str, text: &str) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.add(icons::image(icon, 36.0, t.text_faint));
        ui.add_space(8.0);
        // Every empty-panel message goes through here.
        ui.label(egui::RichText::new(tl!(text)).color(t.text_muted));
    });
}

/// What the user asked to do with a bookmark (applied after the panel is drawn).
#[derive(Clone, Debug, PartialEq)]
pub enum BmAction {
    /// New "Untitled" bookmark for the current page, after the selected one (⌘B).
    New,
    StartRename(Vec<usize>),
    Rename(Vec<usize>, String),
    SetToCurrentPage(Vec<usize>),
    Delete(Vec<usize>),
    MoveUp(Vec<usize>),
    MoveDown(Vec<usize>),
    /// Make it the last child of the bookmark above it.
    Indent(Vec<usize>),
    /// Move it out to follow its parent.
    Outdent(Vec<usize>),
    /// Bookmarks from the tagged headings, under a new first "Untitled" bookmark.
    FromStructure,
}

/// Matching titles plus their ancestors, keeping document paths rather than filtered indexes.
/// `query` is folded with [`pdfcraft_render::text::fold_case`], as titles are here.
fn outline_matches(items: &[OutlineItem], query: &str) -> std::collections::HashSet<Vec<usize>> {
    let mut matches = std::collections::HashSet::new();
    if query.is_empty() {
        return matches;
    }
    let mut pending: Vec<_> = items.iter().enumerate().map(|(i, item)| (vec![i], item)).collect();
    while let Some((path, item)) = pending.pop() {
        if pdfcraft_render::text::fold_case(&item.title).contains(query) {
            let mut ancestor = path.clone();
            while !ancestor.is_empty() {
                matches.insert(ancestor.clone());
                ancestor.pop();
            }
        }
        for (i, child) in item.children.iter().enumerate() {
            let mut child_path = path.clone();
            child_path.push(i);
            pending.push((child_path, child));
        }
    }
    matches
}

struct OutlineCtx<'a> {
    nav: &'a mut Option<Nav>,
    action: &'a mut Option<BmAction>,
    rename: &'a mut Option<(Vec<usize>, String)>,
    editable: bool,
    current: usize,
    /// Expand all (`Some(usize::MAX)`), collapse all (`Some(0)`) or expand to a depth, this frame.
    expand: Option<usize>,
    matches: &'a std::collections::HashSet<Vec<usize>>,
    searching: bool,
}

/// The destination page label gets its own right-aligned column in a bookmark row; drawing
/// tools put whole section titles in /PageLabels, so the column is bounded (#124).
const LABEL_COLUMN_MAX: f32 = 72.0;
/// More characters than ever fit [`LABEL_COLUMN_MAX`] at the label size.
const LABEL_MEASURE_CHARS: usize = 64;

/// The longest head of `text` that `fits`, plus an ellipsis when the whole string doesn't.
/// Whole characters are dropped from the end, so the string is only ever cut at a char
/// boundary, and the loop always ends (at the ellipsis alone). A label is document text: only
/// its first [`LABEL_MEASURE_CHARS`] characters are measured, so a huge one can't make each
/// frame lay out thousands of candidates.
pub(crate) fn ellipsized_prefix(text: &str, mut fits: impl FnMut(&str) -> bool) -> String {
    let mut s: String = text.chars().take(LABEL_MEASURE_CHARS).collect();
    if s.len() == text.len() && fits(text) {
        return text.to_owned();
    }
    loop {
        s.pop();
        let candidate = format!("{s}\u{2026}");
        if s.is_empty() || fits(&candidate) {
            return candidate;
        }
    }
}

fn outline_item(ui: &mut egui::Ui, t: &Tokens, info: &DocInfo, item: &OutlineItem, path: &[usize], siblings: usize, cx: &mut OutlineCtx<'_>) {
    if cx.searching && !cx.matches.contains(path) {
        return;
    }
    let depth = path.len() - 1;
    let indent = depth as f32 * 16.0;
    let id = ui.id().with(("outline", path));
    if let Some(levels) = cx.expand {
        ui.data_mut(|d| d.insert_temp(id, depth < levels));
    }
    let mut open = cx.searching || ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(item.open || depth == 0);
    let x_text = indent + 20.0;
    if let Some((rpath, text)) = cx.rename.as_mut()
        && rpath.as_slice() == path
    {
        // Inline rename: Enter commits, Escape cancels.
        let mut commit = None;
        let mut cancel = false;
        ui.horizontal(|ui| {
            ui.add_space(x_text);
            let edit = egui::TextEdit::singleline(text).desired_width(ui.available_width() - 8.0).id(id.with("rename"));
            let resp = ui.add(edit);
            if !resp.has_focus() && !resp.lost_focus() {
                resp.request_focus();
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            } else if resp.lost_focus() {
                commit = Some(text.trim().to_string());
            }
        });
        if cancel {
            *cx.rename = None;
        } else if let Some(title) = commit {
            *cx.rename = None;
            if !title.is_empty() && title != item.title {
                *cx.action = Some(BmAction::Rename(path.to_vec(), title));
            }
        }
    } else {
        // The page label sits right-aligned in its own bounded column, and the title wraps to
        // what is really left of it — a long label painted over a long title was the overlap
        // of #124.
        let label_font = theme::regular(11.0);
        let label_galley = item.page.and_then(|p| info.pages.get(p)).map(|page| {
            ui.fonts_mut(|f| {
                let text =
                    ellipsized_prefix(&page.label, |s| f.layout_no_wrap(s.to_owned(), label_font.clone(), t.text_faint).size().x <= LABEL_COLUMN_MAX);
                f.layout_no_wrap(text, label_font, t.text_faint)
            })
        });
        let label_w = label_galley.as_ref().map_or(0.0, |g| g.size().x);
        let font = if depth == 0 { theme::medium(13.0) } else { theme::regular(13.0) };
        let wrap_w = (ui.available_width() - indent - 20.0 - label_w - 12.0).max(60.0);
        let galley = ui.fonts_mut(|f| f.layout(item.title.clone(), font, t.text, wrap_w));
        let h = (galley.size().y + 12.0).max(28.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &item.title));
        if resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(6), t.hover);
        }
        let x0 = rect.left() + indent;
        if !item.children.is_empty() {
            let tri = Rect::from_min_size(pos2(x0, rect.top() + 6.0), vec2(16.0, 16.0));
            icons::paint(ui, tri, if open { "chevron-down" } else { "chevron-right" }, 14.0, t.text_muted);
            if !cx.searching && ui.interact(tri, id.with("tri"), Sense::click()).clicked() {
                open = !open;
                ui.data_mut(|d| d.insert_temp(id, open));
            }
        }
        ui.painter().galley(pos2(x0 + 20.0, rect.top() + 6.0), galley, t.text);
        if let Some(g) = label_galley {
            // Right-aligned, vertically centred at the first line, inside the column the title
            // wrapped around.
            let pos = pos2(rect.right() - 6.0 - g.size().x, rect.top() + 14.0 - g.size().y / 2.0);
            ui.painter().galley(pos, g, t.text_faint);
        }
        if resp.clicked()
            && let Some(p) = item.page
        {
            *cx.nav = Some(Nav::Dest(p, item.view));
        }
        if resp.double_clicked() && cx.editable {
            *cx.action = Some(BmAction::StartRename(path.to_vec()));
        }
        if cx.editable
            && let Some(&i) = path.last()
        {
            let current = cx.current;
            resp.context_menu(|ui| {
                let mut pick = |ui: &mut egui::Ui, label: &str, enabled: bool, a: BmAction| {
                    if ui.add_enabled(enabled, egui::Button::new(tl!(label))).clicked() {
                        *cx.action = Some(a);
                        ui.close();
                    }
                };
                pick(ui, tl!("Rename"), true, BmAction::StartRename(path.to_vec()));
                let set_page = crate::i18n::fmt(tl!("Set to current page ({n})"), &[("n", &(current + 1).to_string())]);
                pick(ui, &set_page, true, BmAction::SetToCurrentPage(path.to_vec()));
                ui.separator();
                pick(ui, tl!("Move up"), i > 0, BmAction::MoveUp(path.to_vec()));
                pick(ui, tl!("Move down"), i + 1 < siblings, BmAction::MoveDown(path.to_vec()));
                pick(ui, tl!("Indent"), i > 0, BmAction::Indent(path.to_vec()));
                pick(ui, tl!("Outdent"), depth > 0, BmAction::Outdent(path.to_vec()));
                ui.separator();
                pick(ui, tl!("Delete"), true, BmAction::Delete(path.to_vec()));
            });
        }
    }
    if open {
        for (ci, c) in item.children.iter().enumerate() {
            let mut p = path.to_vec();
            p.push(ci);
            outline_item(ui, t, info, c, &p, item.children.len(), cx);
        }
    }
}

/// The page thumbnails. A click goes to the page; ⌘/Ctrl-click and ⇧-click pick several pages
/// (the selection page commands and Print act on) without moving the document. Right-click
/// offers Extract, Cut, Copy and Paste on the selection, or on the page clicked; dragging
/// moves the selection, or the page grabbed, to the gap it is dropped on.
fn pages(ui: &mut egui::Ui, t: &Tokens, info: &DocInfo, view: &mut crate::DocView, modal: bool, editable: bool, nav: &mut Option<Nav>) {
    // Escape drops the selection while the pointer is over the panel and nothing else wants the key.
    if !modal
        && !view.selected.is_empty()
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.rect_contains_pointer(ui.clip_rect())
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        view.clear_page_selection(Some(view.current));
    }
    // Delete (or Backspace) deletes the selection, or the current page, likewise.
    if !modal
        && editable
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.rect_contains_pointer(ui.clip_rect())
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete) || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace))
    {
        delete_pages(view, info.pages.len());
    }
    let w = (ui.available_width() - 40.0).min(150.0);
    let count = info.pages.len();
    let mut rows: Vec<Rect> = Vec::with_capacity(info.pages.len());
    let mut dropped = false;
    for (i, p) in info.pages.iter().enumerate() {
        ui.vertical_centered(|ui| {
            let h = w * p.height / p.width.max(1.0);
            let sense = if editable { Sense::click_and_drag() } else { Sense::click() };
            let (rect, resp) = ui.allocate_exact_size(vec2(w + 16.0, h + 16.0), sense);
            rows.push(rect);
            if rect.intersects(ui.clip_rect().expand(200.0)) {
                view.need_thumbnail(i, rect.intersects(ui.clip_rect()));
            }
            // Only the primary button drags pages (the middle button scrolls, the secondary one opens the menu).
            if resp.drag_started_by(egui::PointerButton::Primary) {
                if !view.target_pages().contains(&i) {
                    view.click_page(i, egui::Modifiers::NONE, true);
                }
                view.panel_drag = Some(view.target_pages());
            }
            if resp.drag_stopped() {
                dropped = true;
            }
            let info = crate::i18n::fmt(tl!("Page {label}"), &[("label", &p.label)]);
            let picked = view.selected.contains(&i);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, picked, info.clone()));
            // The current page keeps its heavier border; picked pages share its fill.
            let selected = i == view.current;
            if selected || picked {
                ui.painter().rect_filled(rect, CornerRadius::same(8), t.accent_soft);
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(8), t.hover);
            }
            if picked {
                ui.painter().rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
            }
            let pr = rect.shrink(8.0);
            ui.painter().rect_filled(pr, CornerRadius::ZERO, Color32::WHITE);
            if let Some(tex) = view.thumb(i) {
                ui.painter().image(tex.id(), pr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            ui.painter().rect_stroke(
                pr,
                CornerRadius::ZERO,
                Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { t.accent } else { t.border }),
                egui::StrokeKind::Outside,
            );
            ui.label(egui::RichText::new(&p.label).font(theme::medium(12.0)).color(if selected || picked { t.accent_text } else { t.text_muted }));
            if resp.clicked() {
                let m = ui.input(|i| i.modifiers);
                if m.shift || m.command {
                    view.click_page(i, m, true);
                } else {
                    view.clear_page_selection(Some(i));
                    *nav = Some(Nav::Page(i));
                }
            }
            resp.context_menu(|ui| {
                // A page outside the selection becomes the selection, as in the organize grid.
                if !view.target_pages().contains(&i) {
                    view.click_page(i, egui::Modifiers::NONE, true);
                }
                if ui.button(tl!("Extract pages")).clicked() {
                    view.pending_action = Some(crate::canvas::ViewAction::Extract);
                    ui.close();
                }
                ui.separator();
                if ui.add_enabled(editable, egui::Button::new(tl!("Cut"))).clicked() {
                    view.pending_action = Some(crate::canvas::ViewAction::CopyPages { cut: true });
                    ui.close();
                }
                if ui.button(tl!("Copy")).clicked() {
                    view.pending_action = Some(crate::canvas::ViewAction::CopyPages { cut: false });
                    ui.close();
                }
                if ui.add_enabled(editable, egui::Button::new(tl!("Paste after"))).clicked() {
                    view.pending_action = Some(crate::canvas::ViewAction::PastePages);
                    ui.close();
                }
                ui.separator();
                let some_left = view.target_pages().len() < count;
                // Never every page: a document keeps at least one.
                if ui.add_enabled(editable && some_left, egui::Button::new(tl!("Delete pages"))).clicked() {
                    delete_pages(view, count);
                    ui.close();
                }
            });
        });
        ui.add_space(4.0);
    }
    page_drag(ui, t, view, &rows, dropped);
}

/// Queue deleting the selection (or the current page) of a document of `n` pages, unless that
/// would leave no page. The selection goes; the current page stays on the page it showed, or on
/// the page that follows the deleted ones.
fn delete_pages(view: &mut crate::DocView, n: usize) {
    let mut pages = view.target_pages();
    pages.sort_unstable();
    pages.dedup();
    if pages.is_empty() || pages.len() >= n {
        return;
    }
    // Deleted pages before it shift it up; if it was deleted itself, the page after the deleted
    // run lands where it was (or the last page, when the deletion ran to the end).
    let before = pages.iter().filter(|p| **p < view.current).count();
    view.current = view.current.saturating_sub(before).min(n.saturating_sub(pages.len() + 1));
    view.clear_page_selection(None);
    view.pending_edit = Some(pdfcraft_engine::Edit::DeletePages { pages });
}

/// The gap (0 = before the first page, n = after the last) the pointer points at in the Pages
/// panel, whose thumbnails are `rows`, top to bottom.
fn panel_gap(rows: &[Rect], y: f32) -> Option<usize> {
    let (i, r) = rows.iter().enumerate().min_by(|(_, a), (_, b)| (a.center().y - y).abs().total_cmp(&(b.center().y - y).abs()))?;
    Some(if y < r.center().y { i } else { i + 1 })
}

/// Where page `p` goes when `moving` (sorted) move to `to` (counted without them).
fn moved_index(p: usize, moving: &[usize], to: usize) -> usize {
    match moving.iter().position(|m| *m == p) {
        Some(k) => to + k,
        None => {
            let rest = p.saturating_sub(moving.iter().filter(|m| **m < p).count());
            if rest >= to { rest + moving.len() } else { rest }
        }
    }
}

/// While thumbnails are dragged: a bar on the gap they would go to, the page count by the
/// pointer, and scrolling near the panel's edges. On release, queue the move.
fn page_drag(ui: &mut egui::Ui, t: &Tokens, view: &mut crate::DocView, rows: &[Rect], dropped: bool) {
    let Some(mut moving) = view.panel_drag.clone() else { return };
    // A drag that ended while the panel was closed (or elsewhere) leaves nothing to drop.
    if !dropped && !ui.input(|i| i.pointer.primary_down()) {
        view.panel_drag = None;
        return;
    }
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let gap = pointer.and_then(|p| panel_gap(rows, p.y));
    if let (Some(gap), Some(first), Some(last)) = (gap, rows.first(), rows.last()) {
        let y = rows.get(gap).map_or(last.bottom() + 20.0, |r| r.top() - 3.0);
        ui.painter().line_segment([pos2(first.left(), y), pos2(first.right(), y)], Stroke::new(3.0, t.accent));
    }
    if let Some(p) = pointer {
        let n = moving.len();
        let what = if n == 1 { tl!("1 page").to_string() } else { crate::i18n::fmt(tl!("{n} pages"), &[("n", &n.to_string())]) };
        ui.painter().text(p + vec2(14.0, 14.0), Align2::LEFT_TOP, what, theme::medium(12.0), t.accent_text);
        let clip = ui.clip_rect();
        let edge = 40.0;
        if p.y < clip.top() + edge {
            ui.scroll_with_delta(vec2(0.0, 10.0));
        } else if p.y > clip.bottom() - edge {
            ui.scroll_with_delta(vec2(0.0, -10.0));
        }
        ui.ctx().request_repaint();
    }
    if !dropped {
        return;
    }
    view.panel_drag = None;
    let Some(gap) = gap else { return };
    moving.sort_unstable();
    moving.dedup();
    let Some(&first) = moving.first() else { return };
    // `to` counts positions without the moving pages.
    let to = gap.saturating_sub(moving.iter().filter(|x| **x < gap).count());
    let contiguous = moving.windows(2).all(|w| w[1] == w[0] + 1);
    if contiguous && to == first {
        return;
    }
    view.current = moved_index(view.current, &moving, to);
    view.selected = (to..to + moving.len()).collect();
    view.pending_edit = Some(pdfcraft_engine::Edit::MovePages { pages: moving, to });
}

/// The Fields panel: fields by page, in tab order. While preparing a form, each field can move
/// earlier or later in its page's tab order (Acrobat: Order Tabs Manually).
#[allow(clippy::too_many_arguments)]
fn fields(
    ui: &mut egui::Ui,
    t: &Tokens,
    info: &DocInfo,
    form: &[pdfcraft_engine::FormField],
    preparing: bool,
    selection: &mut crate::prepare::PrepareView,
    nav: &mut Option<Nav>,
    edit: &mut Option<pdfcraft_engine::Edit>,
) -> Option<(String, usize)> {
    let mut properties = None;
    if info.fields.is_empty() {
        empty(ui, t, "text-cursor-input", "This document has no form fields.");
        return None;
    }
    let rank = |name: &str| form.iter().find(|f| f.name == name).and_then(|f| f.widgets.iter().map(|w| w.tab).min()).unwrap_or(usize::MAX);
    let mut ordered: Vec<_> = info.fields.iter().collect();
    ordered.sort_by_key(|f| (f.page, rank(&f.name)));
    if preparing {
        ui.label(egui::RichText::new(tl!("Tab order: move a field with its arrows.")).small().color(t.text_muted));
        ui.label(egui::RichText::new(tl!("Shift- or Ctrl/Command-click to select several fields.")).small().color(t.text_muted));
        let names = selection.names();
        // Keep the field rows in place when the first click selects a field, so the
        // second click of a double-click still lands on the same row.
        ui.horizontal(|ui| {
            ui.label(crate::i18n::fmt(tl!("{n} selected"), &[("n", &names.len().to_string())]));
            if ui.add_enabled(!names.is_empty(), egui::Button::new(tl!("Properties…"))).clicked() {
                properties = selection.selected.clone();
            }
        });
    }
    let mut pages: Vec<Option<usize>> = info.fields.iter().map(|f| f.page).collect();
    pages.sort();
    pages.dedup();
    for p in pages {
        let label =
            p.map(|p| crate::i18n::fmt(tl!("Page {label}"), &[("label", &info.pages[p].label)])).unwrap_or_else(|| tl!("Unplaced").to_string());
        ui.add_space(4.0);
        ui.label(egui::RichText::new(label).font(theme::semibold(12.5)).color(t.text_muted));
        for f in ordered.iter().copied().filter(|f| f.page == p) {
            let key = form
                .iter()
                .find(|field| field.name == f.name)
                .and_then(|field| field.widgets.iter().position(|w| w.page == f.page).map(|wi| (field.name.clone(), wi)));
            let selected = key.as_ref().is_some_and(|key| selection.selected.as_ref() == Some(key) || selection.also.contains(key));
            let icon = match f.kind {
                FieldKind::Text => "text-cursor-input",
                FieldKind::CheckBox => "check-circle-2",
                FieldKind::Radio => "circle",
                FieldKind::PushButton => "square",
                FieldKind::Combo => "chevron-down",
                FieldKind::List => "list",
                FieldKind::Signature => "signature",
                FieldKind::Unknown => "square",
            };
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, &f.name));
            if selected || resp.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(6), if selected { t.pressed } else { t.hover });
            }
            icons::paint(ui, Rect::from_min_size(rect.min + vec2(8.0, 7.0), vec2(16.0, 16.0)), icon, 15.0, Color32::from_rgb(0x8E, 0x4E, 0xE6));
            ui.painter().text(rect.left_center() + vec2(32.0, 0.0), Align2::LEFT_CENTER, &f.name, theme::regular(13.0), t.text);
            if let Some(v) = &f.value {
                let v: String = if v.chars().count() > 18 { format!("{}…", v.chars().take(17).collect::<String>()) } else { v.clone() };
                ui.painter().text(rect.right_center() - vec2(8.0, 0.0), Align2::RIGHT_CENTER, v, theme::regular(11.5), t.text_faint);
            }
            let kind = format!("{:?}", f.kind);
            let mut tip = crate::i18n::fmt(tl!("{kind} field"), &[("kind", tl!(&kind))]);
            if let Some(tt) = &f.tooltip {
                tip.push_str(&format!(" — {tt}"));
            }
            if f.has_actions {
                tip.push_str(&format!("\n{}", tl!("Has JavaScript actions (run in M6)")));
            }
            if preparing && f.page.is_some() {
                let up = Rect::from_center_size(rect.right_center() - vec2(44.0, 0.0), vec2(22.0, 22.0));
                let down = Rect::from_center_size(rect.right_center() - vec2(20.0, 0.0), vec2(22.0, 22.0));
                for (r, icon, earlier, tip) in
                    [(up, "chevron-up", true, tl!("Earlier in tab order")), (down, "chevron-down", false, tl!("Later in tab order"))]
                {
                    let b = ui.interact(r, ui.id().with((&f.name, earlier)), Sense::click());
                    b.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{tip}: {}", f.name)));
                    if b.hovered() {
                        ui.painter().rect_filled(r, CornerRadius::same(4), t.pressed);
                    }
                    icons::paint(ui, r.shrink(3.0), icon, 15.0, t.icon);
                    if b.on_hover_text(tip).clicked() {
                        *edit = Some(pdfcraft_engine::Edit::MoveInTabOrder { name: f.name.clone(), earlier });
                    }
                }
            }
            if preparing && resp.double_clicked() && !ui.input(|i| i.modifiers.shift || i.modifiers.command) {
                if let Some(key) = key {
                    if !selected {
                        selection.select(key.clone(), false);
                    }
                    properties = Some(key);
                }
            } else if preparing
                && resp.clicked()
                && let Some(key) = key
            {
                selection.select(key, ui.input(|i| i.modifiers.shift || i.modifiers.command));
            }
            if resp.on_hover_text(tip).clicked()
                && let (Some(p), Some(r)) = (f.page, f.rect)
            {
                *nav = Some(Nav::Flash(p, r));
            }
        }
    }
    properties
}

pub fn human_size(n: usize) -> String {
    match n {
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} KB", n as f64 / (1u64 << 10) as f64),
        n => format!("{n} bytes"),
    }
}

#[cfg(test)]
mod page_moves {
    use super::{moved_index, panel_gap};
    use egui::{Rect, pos2};

    /// Above a thumbnail's middle is the gap before it; below is the gap after it.
    #[test]
    fn the_gap_follows_the_nearest_thumbnail() {
        let rows: Vec<Rect> = (0..3).map(|i| Rect::from_min_max(pos2(0.0, i as f32 * 100.0), pos2(50.0, i as f32 * 100.0 + 90.0))).collect();
        assert_eq!(panel_gap(&rows, -20.0), Some(0));
        assert_eq!(panel_gap(&rows, 30.0), Some(0));
        assert_eq!(panel_gap(&rows, 60.0), Some(1));
        assert_eq!(panel_gap(&rows, 260.0), Some(3));
        assert_eq!(panel_gap(&rows, 900.0), Some(3));
        assert_eq!(panel_gap(&[], 10.0), None);
    }

    /// The current page follows a move, whether it moved or others moved around it.
    #[test]
    fn pages_keep_their_identity_across_a_move() {
        // [0 1 2 3 4], move [1 2] to the end: [0 3 4 1 2].
        let order: Vec<usize> = (0..5).map(|p| moved_index(p, &[1, 2], 3)).collect();
        assert_eq!(order, [0, 3, 4, 1, 2]);
        // Move [3] to the front: [3 0 1 2 4].
        let order: Vec<usize> = (0..5).map(|p| moved_index(p, &[3], 0)).collect();
        assert_eq!(order, [1, 2, 3, 0, 4]);
    }
}

#[cfg(test)]
mod label_column {
    use super::ellipsized_prefix;

    /// A short page label (the usual "3", "iv", "1-1") is shown unchanged.
    #[test]
    fn short_labels_are_not_touched() {
        for label in ["3", "iv", "1-1", "A-201"] {
            assert_eq!(ellipsized_prefix(label, |s| s.len() <= 10), label);
        }
    }

    /// A label wider than its column (#124: whole section titles in /PageLabels) keeps only
    /// the head that fits, with an ellipsis, and that head is a prefix of the real label.
    #[test]
    fn long_labels_are_cut_to_their_column_with_an_ellipsis() {
        let label = "Floor Plans (001 Floor Plans): GROUND FLOOR PLAN";
        let shown = ellipsized_prefix(label, |s| s.len() <= 20);
        assert!(shown.ends_with('\u{2026}'), "{shown:?}");
        assert!(shown.len() <= 20, "{shown:?}");
        assert!(label.starts_with(shown.trim_end_matches('\u{2026}')), "{shown:?}");
    }

    /// CJK labels (three bytes per character) are cut on character boundaries too.
    #[test]
    fn multibyte_labels_are_cut_on_char_boundaries() {
        let label = "図面（一階平面図）：配置図";
        let shown = ellipsized_prefix(label, |s| s.len() <= 12);
        assert!(shown.ends_with('\u{2026}'), "{shown:?}");
        assert!(shown.len() <= 12, "{shown:?}");
        assert!(label.starts_with(shown.trim_end_matches('\u{2026}')), "{shown:?}");
    }

    /// The pathological floor: nothing fits, the loop still ends with the lone ellipsis.
    #[test]
    fn the_loop_ends_when_nothing_fits() {
        assert_eq!(ellipsized_prefix("Floor Plans", |_| false), "\u{2026}");
    }

    /// A huge label from a hostile file is measured a bounded number of times per frame.
    #[test]
    fn a_huge_label_is_measured_a_bounded_number_of_times() {
        let label = "x".repeat(1_000_000);
        let mut calls = 0;
        let shown = ellipsized_prefix(&label, |s| {
            calls += 1;
            s.len() <= 12
        });
        assert!(calls <= super::LABEL_MEASURE_CHARS + 1, "{calls} measurements");
        assert!(shown.ends_with('\u{2026}') && label.starts_with(shown.trim_end_matches('\u{2026}')), "{shown:?}");
    }
}

#[cfg(test)]
mod bookmark_filter {
    use super::outline_matches;
    use pdfcraft_render::OutlineItem;

    fn item(title: &str, children: Vec<OutlineItem>) -> OutlineItem {
        OutlineItem { title: title.into(), page: Some(0), view: pdfcraft_render::DestView::Top, children, open: true }
    }

    /// Turkish dotted and dotless i match in any case, as in Find (#892).
    #[test]
    fn titles_match_in_any_case_with_turkish_i() {
        let outline = [item("İSTANBUL ŞUBESİ", vec![item("Kış dönemi", vec![])]), item("Ankara", vec![])];
        let fold = pdfcraft_render::text::fold_case;
        assert_eq!(outline_matches(&outline, &fold("istanbul")).len(), 1);
        assert_eq!(outline_matches(&outline, &fold("şubesi")).len(), 1);
        // A child match keeps its parent: [0] and [0, 0].
        assert_eq!(outline_matches(&outline, &fold("KIŞ")).len(), 2);
        assert!(outline_matches(&outline, &fold("izmir")).is_empty());
    }
}
