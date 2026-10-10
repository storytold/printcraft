//! Modal dialogs: Document Properties, Keyboard Shortcuts, About (with the Contributors and Models credits).

use egui::{Align, Layout};

use crate::theme::{self, Tokens};
use pdfcraft_engine::Edit;

use crate::{CloseRequest, Dialog, PdfCraftApp, PropsTab, panels::human_size, widgets};

const INFO_KEYS: [&str; 4] = ["Title", "Author", "Subject", "Keywords"];

pub fn show(app: &mut PdfCraftApp, ctx: &egui::Context) {
    password(app, ctx);
    save_prompt(app, ctx);
    link_prompt(app, ctx);
    crate::updates::dialog(app, ctx);
    let Some(dialog) = app.dialog else {
        app.props_draft = None;
        app.view_draft = None;
        return;
    };
    // Seed the editable Description fields from the document when the dialog opens.
    if let (Dialog::Properties(_), Some((_, id))) = (dialog, app.active_ids())
        && app.props_draft.as_ref().is_none_or(|(d, _)| *d != id)
        && let Some(doc) = app.session.get(id)
    {
        app.props_draft = Some((id, INFO_KEYS.map(|k| doc.info_value(k).unwrap_or_default())));
        app.view_draft = Some((id, doc.initial_view()));
    }
    let mut apply = false;
    let mut split_now: Option<crate::SplitPlan> = None;
    let mut split_ready: Option<crate::SplitPlan> = None;
    let mut extract_now = false;
    let mut rotate_now = false;
    let mut link_now: Option<Edit> = None;
    let mut recover: Option<bool> = None;
    let mut number_now: Option<Edit> = None;
    let mut apply_number = false;
    let mut link_command: Option<&'static str> = None;
    let mut protect_now = false;
    let mut boxes_now = false;
    let mut marks_now = false;
    let mut export_now = false;
    let mut props_now = false;
    let mut field_props_now = false;
    let mut bulk_field_props_now = false;
    let mut redact_now: Option<Dialog> = None;
    let mut import_words = false;
    let mut print_go = false;
    let mut revert_now = false;
    let mut summarize_now = false;
    let mut optimize_now = false;
    let mut duplicate_now: Option<Edit> = None;
    let mut replace_now = false;
    let mut open_revision: Option<usize> = None;
    let mut a11y_now = false;
    let mut ocr_now = false;
    let mut compare_now = false;
    let mut images_now = false;
    let mut stamp_now = false;
    let mut alt_now = false;
    let t = Tokens::get(ctx);
    let mut close = false;
    let mut next = dialog;
    let modal = egui::Modal::new(egui::Id::new("dialog")).show(ctx, |ui| {
        ui.set_width(match dialog {
            Dialog::Properties(_) => 640.0,
            Dialog::Print => 820.0,
            Dialog::FieldProps | Dialog::BulkFieldProps => 600.0,
            Dialog::About => 780.0,
            _ => 520.0,
        });
        // Dialog controls are outlined (radio buttons, check boxes, combo boxes and number fields
        // would otherwise blend into the dialog, whose fill matches the theme's field colour).
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.bg_stroke = egui::Stroke::new(1.0, t.border);
        w.inactive.weak_bg_fill = t.field;
        // Slider rails and check-box interiors use the plain fill.
        w.inactive.bg_fill = t.hover;
        w.hovered.bg_stroke = egui::Stroke::new(1.0, t.text_muted);
        match dialog {
            Dialog::CreateImages => {
                let (go, cancel) = crate::create_ui::image_import_body(ui, app);
                images_now = go;
                close = go || cancel;
                return;
            }
            Dialog::Properties(tab) => {
                ui.label(egui::RichText::new(tl!("Document Properties")).font(theme::semibold(18.0)));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    for (tb, label) in [
                        (PropsTab::Description, tl!("Description")),
                        (PropsTab::InitialView, tl!("Initial View")),
                        (PropsTab::Security, tl!("Security")),
                        (PropsTab::Fonts, tl!("Fonts")),
                        (PropsTab::Advanced, tl!("Advanced")),
                    ] {
                        if widgets::mode_tab(ui, label, tab == tb).clicked() {
                            next = Dialog::Properties(tb);
                        }
                    }
                });
                ui.separator();
                let Some((_, id)) = app.active_ids() else { return };
                let Some(doc) = app.session.get(id) else { return };
                let i = &doc.info;
                let row = |ui: &mut egui::Ui, k: &str, v: String| {
                    // Row labels are UI chrome; values are document data and stay as they are.
                    ui.label(egui::RichText::new(tl!(k)).color(t.text_muted));
                    ui.label(if v.is_empty() { egui::RichText::new("—").color(t.text_faint) } else { egui::RichText::new(v) });
                    ui.end_row();
                };
                egui::ScrollArea::vertical().max_height(460.0).auto_shrink([false, true]).show(ui, |ui| {
                    egui::Grid::new("props").num_columns(2).spacing([18.0, 8.0]).min_col_width(140.0).show(ui, |ui| match tab {
                        PropsTab::Description => {
                            row(ui, "File", crate::bidi::visual(&doc.name).into_owned());
                            match app.props_draft.as_mut() {
                                Some((_, draft)) if doc.allows_modification() => {
                                    for (k, v) in INFO_KEYS.iter().zip(draft.iter_mut()) {
                                        let l = ui.label(egui::RichText::new(tl!(k)).color(t.text_muted));
                                        ui.add(
                                            egui::TextEdit::singleline(v)
                                                .desired_width(420.0)
                                                .background_color(t.field)
                                                .margin(egui::Margin::symmetric(6, 4))
                                                .id_salt(("info", *k)),
                                        )
                                        .labelled_by(l.id);
                                        ui.end_row();
                                    }
                                }
                                _ => {
                                    row(ui, "Title", i.title.clone().unwrap_or_default());
                                    row(ui, "Author", i.author.clone().unwrap_or_default());
                                    row(ui, "Subject", i.subject.clone().unwrap_or_default());
                                    row(ui, "Keywords", i.keywords.clone().unwrap_or_default());
                                }
                            }
                            row(ui, "Application", i.creator.clone().unwrap_or_default());
                            row(ui, "PDF producer", i.producer.clone().unwrap_or_default());
                        }
                        PropsTab::InitialView => {
                            use pdfcraft_engine::{InitialLayout as L, Magnification as M, Navigation as N};
                            let editable = doc.allows_modification();
                            let pages = i.pages.len();
                            let Some((_, v)) = app.view_draft.as_mut() else { return };
                            ui.label(egui::RichText::new(tl!("Layout and Magnification")).font(theme::semibold(12.5)));
                            ui.end_row();
                            ui.label(tl!("Navigation tab"));
                            ui.add_enabled_ui(editable, |ui| {
                                egui::ComboBox::from_id_salt("iv-nav")
                                    .selected_text(tl!(format!("{:?}", v.navigation)
                                        .replace("PageOnly", "Page Only")
                                        .replace("Pages", "Pages Panel and Page")
                                        .as_str()))
                                    .show_ui(ui, |ui| {
                                        for (n, l) in [
                                            (N::PageOnly, tl!("Page Only")),
                                            (N::Bookmarks, tl!("Bookmarks Panel and Page")),
                                            (N::Pages, tl!("Pages Panel and Page")),
                                            (N::Attachments, tl!("Attachments Panel and Page")),
                                            (N::Layers, tl!("Layers Panel and Page")),
                                        ] {
                                            ui.selectable_value(&mut v.navigation, n, l);
                                        }
                                    });
                            });
                            ui.end_row();
                            ui.label(tl!("Page layout"));
                            ui.add_enabled_ui(editable, |ui| {
                                let names = [
                                    (L::Default, tl!("Default")),
                                    (L::SinglePage, tl!("Single Page")),
                                    (L::SinglePageContinuous, tl!("Single Page Continuous")),
                                    (L::TwoUp, tl!("Two-Up (Facing)")),
                                    (L::TwoUpContinuous, tl!("Two-Up Continuous (Facing)")),
                                    (L::TwoUpCoverPage, tl!("Two-Up (Cover Page)")),
                                    (L::TwoUpContinuousCoverPage, tl!("Two-Up Continuous (Cover Page)")),
                                ];
                                let shown = names.iter().find(|(l, _)| *l == v.layout).map_or(tl!("Default"), |(_, n)| n);
                                egui::ComboBox::from_id_salt("iv-layout").selected_text(shown).show_ui(ui, |ui| {
                                    for (l, n) in names {
                                        ui.selectable_value(&mut v.layout, l, n);
                                    }
                                });
                            });
                            ui.end_row();
                            ui.label(tl!("Magnification"));
                            ui.add_enabled_ui(editable, |ui| {
                                ui.horizontal(|ui| {
                                    let names = [
                                        (M::Default, tl!("Default")),
                                        (M::ActualSize, tl!("Actual Size")),
                                        (M::FitPage, tl!("Fit Page")),
                                        (M::FitWidth, tl!("Fit Width")),
                                        (M::FitHeight, tl!("Fit Height")),
                                        (M::FitVisible, tl!("Fit Visible")),
                                    ];
                                    let shown = match v.magnification {
                                        M::Percent(p) => format!("{p:.0}%"),
                                        m => names.iter().find(|(x, _)| *x == m).map_or(tl!("Default").to_string(), |(_, n)| n.to_string()),
                                    };
                                    egui::ComboBox::from_id_salt("iv-mag").selected_text(shown).show_ui(ui, |ui| {
                                        for (m, n) in names {
                                            ui.selectable_value(&mut v.magnification, m, n);
                                        }
                                        for p in [50.0, 75.0, 125.0, 150.0, 200.0] {
                                            ui.selectable_value(&mut v.magnification, M::Percent(p), format!("{p:.0}%"));
                                        }
                                    });
                                });
                            });
                            ui.end_row();
                            ui.label(tl!("Open to page"));
                            ui.add_enabled_ui(editable, |ui| {
                                let mut p = v.page + 1;
                                if ui.add(egui::DragValue::new(&mut p).range(1..=pages.max(1))).changed() {
                                    v.page = p - 1;
                                }
                                ui.label(crate::i18n::fmt(tl!("of {pages}"), &[("pages", &pages.to_string())]));
                            });
                            ui.end_row();
                            ui.label(egui::RichText::new(tl!("Window Options")).font(theme::semibold(12.5)));
                            ui.end_row();
                            ui.label("");
                            ui.add_enabled_ui(editable, |ui| {
                                ui.vertical(|ui| {
                                    ui.checkbox(&mut v.fit_window, tl!("Resize window to initial page"));
                                    ui.checkbox(&mut v.center_window, tl!("Center window on screen"));
                                    ui.checkbox(&mut v.full_screen, tl!("Open in Full Screen mode"));
                                    ui.horizontal(|ui| {
                                        ui.label(tl!("Show:"));
                                        ui.radio_value(&mut v.display_title, false, tl!("File Name"));
                                        ui.radio_value(&mut v.display_title, true, tl!("Document Title"));
                                    });
                                });
                            });
                            ui.end_row();
                            ui.label(egui::RichText::new(tl!("User Interface Options")).font(theme::semibold(12.5)));
                            ui.end_row();
                            ui.label("");
                            ui.add_enabled_ui(editable, |ui| {
                                ui.vertical(|ui| {
                                    ui.checkbox(&mut v.hide_menubar, tl!("Hide menu bar"));
                                    ui.checkbox(&mut v.hide_toolbar, tl!("Hide toolbars"));
                                    ui.checkbox(&mut v.hide_window_ui, tl!("Hide window controls"));
                                });
                            });
                            ui.end_row();
                        }
                        PropsTab::Security => match doc.security_summary() {
                            None => {
                                row(ui, "Security method", tl!("No security").to_string());
                                row(ui, "Restrictions", tl!("None — everything is allowed").to_string());
                                if doc.allows_security_change() && ui.button(tl!("Protect using password…")).clicked() {
                                    link_command = Some("protect.password");
                                }
                            }
                            Some(sec) => {
                                row(ui, "Security method", tl!("Password security").to_string());
                                row(ui, "Encryption", sec.method.clone());
                                row(
                                    ui,
                                    "Opened with",
                                    if sec.pending {
                                        tl!("— (protection is applied when you save)").to_string()
                                    } else if sec.owner {
                                        tl!("Owner password (no restrictions apply)").to_string()
                                    } else {
                                        tl!("User password").to_string()
                                    },
                                );
                                if doc.allows_security_change() {
                                    ui.horizontal(|ui| {
                                        if ui.button(tl!("Change settings…")).clicked() {
                                            link_command = Some("protect.password");
                                        }
                                        if ui.button(tl!("Remove security")).clicked() {
                                            link_command = Some("protect.remove");
                                        }
                                    });
                                }
                                let p = sec.permissions;
                                let yes = |b: bool| tl!(if b { "Allowed" } else { "Not allowed" }).to_string();
                                row(
                                    ui,
                                    "Printing",
                                    if !p.print() {
                                        tl!("Not allowed").to_string()
                                    } else if p.print_high_quality() {
                                        tl!("High resolution").to_string()
                                    } else {
                                        tl!("Low resolution").to_string()
                                    },
                                );
                                row(ui, "Changing the document", yes(p.modify()));
                                row(ui, "Document assembly", yes(p.assemble()));
                                row(ui, "Content copying", yes(p.copy()));
                                row(ui, "Content copying for accessibility", yes(p.extract_for_accessibility()));
                                row(ui, "Commenting", yes(p.annotate()));
                                row(ui, "Filling of form fields", yes(p.fill_forms()));
                            }
                        },
                        PropsTab::Fonts => {
                            if i.fonts.is_empty() {
                                row(ui, "Fonts", tl!("No fonts are referenced by the pages.").to_string());
                            }
                            for f in &i.fonts {
                                let mut detail = f.kind.clone();
                                if let Some(e) = &f.encoding {
                                    detail.push_str(&format!(" · {e}"));
                                }
                                detail.push_str(if f.subset {
                                    " · Embedded subset"
                                } else if f.embedded {
                                    " · Embedded"
                                } else {
                                    " · Not embedded (substituted)"
                                });
                                row(ui, &f.name, detail);
                            }
                        }
                        PropsTab::Advanced => {
                            row(ui, "PDF version", i.pdf_version.clone());
                            row(ui, "Location", crate::bidi::visual(doc.path.as_deref().unwrap_or_default()).into_owned());
                            row(ui, "File size", format!("{} ({} bytes)", human_size(i.file_size), i.file_size));
                            let p = &i.pages[0];
                            row(ui, "Page size", format!("{:.2} × {:.2} in", p.width / 72.0, p.height / 72.0));
                            row(ui, "Number of pages", i.pages.len().to_string());
                            row(ui, "Tagged PDF", yes(i.tagged));
                            row(ui, "Form fields", i.fields.len().to_string());
                            row(ui, "Comments", i.annotations.len().to_string());
                            row(ui, "Layers", i.layers.len().to_string());
                            row(ui, "Attachments", i.attachments.len().to_string());
                            row(ui, "JavaScript", yes(i.has_javascript));
                            // Each incremental update is a revision; earlier ones open as their own document.
                            let ends = doc.revision_ends();
                            if ends.len() > 1 {
                                ui.label(egui::RichText::new(tl!("Revisions")).color(t.text_muted));
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(ends.len().to_string());
                                    for n in (1..ends.len()).rev().take(12) {
                                        if ui
                                            .small_button(crate::i18n::fmt(tl!("View revision {n}"), &[("n", &n.to_string())]))
                                            .on_hover_text(tl!("Open the file as it was saved then"))
                                            .clicked()
                                        {
                                            open_revision = Some(n);
                                        }
                                    }
                                });
                                ui.end_row();
                            }
                            // Reading Options: binding and language.
                            if let Some((_, v)) = app.view_draft.as_mut() {
                                let editable = doc.allows_modification();
                                ui.label(egui::RichText::new(tl!("Binding")).color(t.text_muted));
                                ui.add_enabled_ui(editable, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.radio_value(&mut v.right_to_left, false, tl!("Left Edge"));
                                        ui.radio_value(&mut v.right_to_left, true, tl!("Right Edge"));
                                    });
                                });
                                ui.end_row();
                                let l = ui.label(egui::RichText::new(tl!("Language")).color(t.text_muted));
                                let mut lang = v.language.clone().unwrap_or_default();
                                if ui
                                    .add_enabled(editable, egui::TextEdit::singleline(&mut lang).hint_text(tl!("e.g. en-US")).desired_width(160.0))
                                    .labelled_by(l.id)
                                    .changed()
                                {
                                    v.language = (!lang.trim().is_empty()).then(|| lang.trim().to_string());
                                }
                                ui.end_row();
                            }
                            // What was repaired while reading a damaged file (fidelity: never silent).
                            let repairs = doc.repair_log();
                            row(ui, "Repairs", if repairs.is_empty() { tl!("None").to_string() } else { repairs.len().to_string() });
                            if !repairs.is_empty() {
                                ui.label("");
                                egui::CollapsingHeader::new(tl!("Repair log")).show(ui, |ui| {
                                    for r in &repairs {
                                        ui.add(egui::Label::new(egui::RichText::new(r).small()).wrap());
                                    }
                                });
                                ui.end_row();
                            }
                        }
                    })
                });
            }
            Dialog::Split => {
                ui.label(egui::RichText::new(tl!("Split document")).font(theme::semibold(18.0)));
                ui.add_space(8.0);
                let Some((vi, id)) = app.active_ids() else { return };
                let n = app.session.get(id).map(|d| d.info.pages.len()).unwrap_or(0);
                let selected: Vec<usize> = app.views[vi].selected.iter().copied().filter(|p| *p > 0).collect();
                let marks = app.session.bookmark_splits(id);
                let draft = &mut app.split_draft;
                use crate::SplitMode as M;
                if selected.is_empty() && draft.mode == M::Selection {
                    draft.mode = M::Pages;
                }
                ui.radio_value(&mut draft.mode, M::Pages, tl!("Number of pages"));
                ui.add_enabled_ui(draft.mode == M::Pages, |ui| {
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        ui.label(tl!("Pages per file"));
                        ui.add(egui::DragValue::new(&mut draft.every).range(1..=n.max(1)));
                    });
                });
                ui.radio_value(&mut draft.mode, M::Size, tl!("File size"));
                ui.add_enabled_ui(draft.mode == M::Size, |ui| {
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        ui.label(tl!("At most"));
                        ui.add(egui::DragValue::new(&mut draft.size_mb).range(0.05..=2000.0).speed(0.1).suffix(" MB"));
                    });
                });
                ui.add_enabled_ui(!marks.is_empty(), |ui| {
                    ui.radio_value(
                        &mut draft.mode,
                        M::Bookmarks,
                        crate::i18n::fmt(tl!("Top-level bookmarks ({n})"), &[("n", &marks.len().to_string())]),
                    )
                });
                ui.add_enabled_ui(!selected.is_empty(), |ui| {
                    ui.radio_value(&mut draft.mode, M::Selection, tl!("Before each selected page (select pages in Organize)"))
                });
                let plan = match draft.mode {
                    M::Pages => crate::SplitPlan::By(pdfcraft_engine::SplitBy::PageCount(draft.every)),
                    M::Selection => crate::SplitPlan::By(pdfcraft_engine::SplitBy::Before(selected)),
                    M::Size => crate::SplitPlan::Size((draft.size_mb * 1_048_576.0) as usize),
                    M::Bookmarks => crate::SplitPlan::Bookmarks,
                };
                let files = match &plan {
                    crate::SplitPlan::By(by) => Some(pdfcraft_engine::split_ranges(n, by).len()),
                    crate::SplitPlan::Bookmarks => {
                        Some(pdfcraft_engine::split_ranges(n, &pdfcraft_engine::SplitBy::Before(marks.iter().map(|m| m.0).collect())).len())
                    }
                    crate::SplitPlan::Size(_) => None,
                };
                ui.add_space(8.0);
                let text = match files {
                    Some(f) => {
                        if f == 1 {
                            crate::i18n::fmt(tl!("Creates 1 file from {n} pages."), &[("n", &n.to_string())])
                        } else {
                            crate::i18n::fmt(tl!("Creates {f} files from {n} pages."), &[("f", &f.to_string()), ("n", &n.to_string())])
                        }
                    }
                    None => crate::i18n::fmt(tl!("Each file holds as many of the {n} pages as fit."), &[("n", &n.to_string())]),
                };
                ui.label(egui::RichText::new(text).color(t.text_muted));
                if files.is_none_or(|f| f > 1) {
                    split_ready = Some(plan);
                }
            }
            Dialog::ReplacePages => {
                let count = app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|d| d.info.pages.len()).unwrap_or(1).max(1);
                let Some(d) = app.replace_draft.as_mut() else {
                    close = true;
                    return;
                };
                ui.label(egui::RichText::new(tl!("Replace Pages")).font(theme::semibold(18.0)));
                ui.add_space(8.0);
                d.to = d.to.clamp(1, count);
                d.from = d.from.clamp(1, d.to);
                let n = d.to - d.from + 1;
                ui.horizontal(|ui| {
                    ui.label(tl!("Original: replace pages"));
                    ui.add(egui::DragValue::new(&mut d.from).range(1..=count));
                    ui.label(tl!("to"));
                    ui.add(egui::DragValue::new(&mut d.to).range(1..=count));
                    ui.label(egui::RichText::new(crate::i18n::fmt(tl!("of {name}"), &[("name", &count.to_string())])).color(t.text_muted));
                });
                let max_start = d.src_pages.saturating_sub(n) + 1;
                d.src_from = d.src_from.clamp(1, max_start.max(1));
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::fmt(tl!("Replacement: pages of {name}"), &[("name", &d.name)]));
                    ui.add(egui::DragValue::new(&mut d.src_from).range(1..=max_start.max(1)));
                    ui.label(crate::i18n::fmt(tl!("to {n}"), &[("n", &(d.src_from + n - 1).to_string())]));
                    ui.label(egui::RichText::new(crate::i18n::fmt(tl!("of {name}"), &[("name", &d.src_pages.to_string())])).color(t.text_muted));
                });
                let fits = n <= d.src_pages;
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(if fits {
                        tl!("Only the page content changes: links, comments, form fields and bookmarks on the original pages stay.")
                    } else {
                        tl!("The replacement file doesn't have that many pages.")
                    })
                    .small()
                    .color(t.text_faint),
                );
                ui.add_space(10.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add_enabled_ui(fits, |ui| widgets::pill_button(ui, tl!("OK"), true)).inner.clicked() {
                        replace_now = true;
                        close = true;
                    }
                    if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                        close = true;
                    }
                });
                return;
            }
            Dialog::RedactPages => {
                let n = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len());
                let (ok, cancel) = crate::redact_ui::pages_body(ui, &mut app.redact_pages_draft, n, &t);
                if ok {
                    redact_now = Some(dialog);
                }
                close = ok || cancel;
                return;
            }
            Dialog::RedactSearch => {
                let (go, cancel, import) = crate::redact_ui::search_body(ui, &mut app.redact_search, &t);
                if go {
                    redact_now = Some(dialog);
                }
                import_words = import;
                close = cancel;
                return;
            }
            Dialog::RedactProps => {
                let mut prefs = app.redact_prefs.clone();
                let (ok, cancel) = crate::redact_ui::props_body(ui, &mut prefs, &t);
                app.redact_prefs = prefs;
                close = ok || cancel;
                return;
            }
            Dialog::RedactApply => {
                let marks = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(0, |d| d.redaction_marks());
                let (ok, cancel) = crate::redact_ui::apply_body(ui, marks, &t);
                if ok {
                    redact_now = Some(dialog);
                }
                close = ok || cancel;
                return;
            }
            Dialog::LinkProps => {
                let pages = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len());
                let Some(d) = app.link_draft.as_mut() else {
                    close = true;
                    return;
                };
                let (ok, cancel) = crate::link_ui::body(ui, d, pages, &t);
                if ok {
                    link_now = Some(crate::link_ui::edit_for(d));
                }
                close = ok || cancel;
                return;
            }
            Dialog::Extract => {
                let count = app.active_ids().map_or(0, |(i, _)| app.views[i].target_pages().len());
                ui.label(egui::RichText::new(tl!("Extract pages")).font(theme::semibold(18.0)));
                ui.add_space(8.0);
                let text = if count == 1 {
                    tl!("1 page selected.").to_string()
                } else {
                    crate::i18n::fmt(tl!("{n} pages selected."), &[("n", &count.to_string())])
                };
                ui.label(text);
                ui.add_space(4.0);
                // The name the pages are saved under (#737); separate files add " (page N)".
                ui.horizontal(|ui| {
                    let l = ui.label(tl!("File name"));
                    ui.add(egui::TextEdit::singleline(&mut app.extract_draft.name).desired_width(240.0)).labelled_by(l.id);
                });
                ui.checkbox(&mut app.extract_draft.delete, tl!("Delete pages after extracting"));
                ui.checkbox(&mut app.extract_draft.separate, tl!("Extract pages as separate files"));
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::pill_button(ui, tl!("Extract"), true).clicked() {
                        extract_now = true;
                        close = true;
                    }
                    if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                        close = true;
                    }
                });
                return;
            }
            Dialog::RotatePages => {
                use pdfcraft_engine::{PageOrientation as O, PageParity as P};
                let n = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len());
                let d = &mut app.rotate_draft;
                ui.label(egui::RichText::new(tl!("Rotate Pages")).font(theme::semibold(18.0)));
                ui.add_space(8.0);
                egui::Grid::new("rotate-pages").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    ui.label(tl!("Direction:"));
                    egui::ComboBox::from_id_salt("rotate-dir")
                        .selected_text(match d.degrees {
                            270 => tl!("Counterclockwise 90 degrees"),
                            180 => tl!("180 degrees"),
                            _ => tl!("Clockwise 90 degrees"),
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut d.degrees, 90, tl!("Clockwise 90 degrees"));
                            ui.selectable_value(&mut d.degrees, 270, tl!("Counterclockwise 90 degrees"));
                            ui.selectable_value(&mut d.degrees, 180, tl!("180 degrees"));
                        });
                    ui.end_row();
                    ui.label(tl!("Pages:"));
                    ui.vertical(|ui| {
                        ui.radio_value(&mut d.which, 0, tl!("All"));
                        ui.radio_value(&mut d.which, 1, tl!("Selection"));
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut d.which, 2, tl!("From"));
                            ui.add_enabled(d.which == 2, egui::DragValue::new(&mut d.from).range(1..=n));
                            ui.label(tl!("to"));
                            ui.add_enabled(d.which == 2, egui::DragValue::new(&mut d.to).range(1..=n));
                            ui.label(crate::i18n::fmt(tl!("of {name}"), &[("name", &n.to_string())]));
                        });
                    });
                    ui.end_row();
                    ui.label(tl!("Rotate:"));
                    egui::ComboBox::from_id_salt("rotate-parity")
                        .selected_text(match d.parity {
                            P::Both => tl!("Even and Odd Pages"),
                            P::Even => tl!("Even Pages Only"),
                            P::Odd => tl!("Odd Pages Only"),
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut d.parity, P::Both, tl!("Even and Odd Pages"));
                            ui.selectable_value(&mut d.parity, P::Even, tl!("Even Pages Only"));
                            ui.selectable_value(&mut d.parity, P::Odd, tl!("Odd Pages Only"));
                        });
                    ui.end_row();
                    ui.label("");
                    egui::ComboBox::from_id_salt("rotate-orient")
                        .selected_text(match d.orientation {
                            O::Both => tl!("Landscape and Portrait Pages"),
                            O::Landscape => tl!("Landscape Pages"),
                            O::Portrait => tl!("Portrait Pages"),
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut d.orientation, O::Both, tl!("Landscape and Portrait Pages"));
                            ui.selectable_value(&mut d.orientation, O::Landscape, tl!("Landscape Pages"));
                            ui.selectable_value(&mut d.orientation, O::Portrait, tl!("Portrait Pages"));
                        });
                    ui.end_row();
                });
                d.to = d.to.max(d.from);
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::pill_button(ui, tl!("OK"), true).clicked() {
                        rotate_now = true;
                        close = true;
                    }
                    if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                        close = true;
                    }
                });
                return;
            }
            Dialog::DuplicateField => {
                let n = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len());
                let Some(d) = app.duplicate_draft.as_mut() else {
                    close = true;
                    return;
                };
                ui.label(egui::RichText::new(tl!("Duplicate Field")).font(theme::semibold(18.0)));
                ui.add_space(8.0);
                ui.label(crate::i18n::fmt(tl!("Duplicate \"{name}\" onto:"), &[("name", &d.name)]));
                ui.radio_value(&mut d.all, true, tl!("All pages"));
                ui.horizontal(|ui| {
                    ui.radio_value(&mut d.all, false, tl!("From"));
                    ui.add_enabled(!d.all, egui::DragValue::new(&mut d.from).range(1..=n));
                    ui.label(tl!("to"));
                    ui.add_enabled(!d.all, egui::DragValue::new(&mut d.to).range(1..=n));
                    ui.label(crate::i18n::fmt(tl!("of {name}"), &[("name", &n.to_string())]));
                });
                d.to = d.to.max(d.from);
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::pill_button(ui, tl!("OK"), true).clicked() {
                        let pages: Vec<usize> = if d.all { (0..n).collect() } else { (d.from - 1..d.to.min(n)).collect() };
                        duplicate_now = Some(Edit::DuplicateField { name: d.name.clone(), pages });
                        close = true;
                    }
                    if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                        close = true;
                    }
                });
                return;
            }
            Dialog::Optimize => {
                let (ok, cancel) = crate::optimize_ui::body(ui, &mut app.optimize_draft, &t);
                optimize_now = ok;
                close = ok || cancel;
                if std::mem::take(&mut app.optimize_draft.audit) {
                    app.space_audit = app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|d| d.audit_space()).unwrap_or_default();
                    next = Dialog::AuditSpace;
                }
                return;
            }
            Dialog::CertificateViewer => {
                ui.set_width(700.0);
                let trusted = app.session.trusted_certificates().to_vec();
                let Some(v) = app.cert_viewer.as_mut() else {
                    close = true;
                    return;
                };
                let (done, action) = crate::sign_ui::cert_viewer(ui, v, &trusted, &t);
                close = done;
                match action {
                    Some(crate::sign_ui::CertAction::Trust(c)) => app.trust_certificate(*c),
                    Some(crate::sign_ui::CertAction::Export(c)) => {
                        let pem = crate::sign_ui::certificate_pem(&c);
                        app.write_files(&[(format!("{}.cer", c.display_name()), std::sync::Arc::new(pem.into_bytes()))], "Export certificate");
                    }
                    None => {}
                }
                return;
            }
            Dialog::AuditSpace => {
                if crate::optimize_ui::audit_body(ui, &app.space_audit, &t) {
                    next = Dialog::Optimize;
                }
                return;
            }
            Dialog::Sign => {
                if crate::sign_ui::dialog(ui, app, &t) {
                    app.sign_draft = None;
                    close = true;
                }
                return;
            }
            Dialog::SummarizeComments => {
                ui.label(egui::RichText::new(tl!("Summarize Options")).font(theme::semibold(18.0)));
                ui.add_space(8.0);
                ui.label(tl!("Choose a layout:"));
                let _ = ui.radio(true, tl!("Comments only"));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(tl!("Sort comments by:"));
                    egui::ComboBox::from_id_salt("summary-sort").selected_text(tl!(app.summary_sort.name())).show_ui(ui, |ui| {
                        for s in pdfcraft_engine::SummarySort::ALL {
                            ui.selectable_value(&mut app.summary_sort, s, tl!(s.name()));
                        }
                    });
                });
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::pill_button(ui, tl!("Create PDF Comment Summary"), true).clicked() {
                        summarize_now = true;
                        close = true;
                    }
                    if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                        close = true;
                    }
                });
                return;
            }
            Dialog::Revert => {
                let name = app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|d| d.name.clone()).unwrap_or_default();
                ui.label(egui::RichText::new(tl!("Revert")).font(theme::semibold(18.0)));
                ui.add_space(8.0);
                ui.label(crate::i18n::fmt(
                    tl!("Revert to the previously saved version of “{name}”? Changes since then can't be undone afterwards."),
                    &[("name", &name)],
                ));
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::pill_button(ui, tl!("Revert"), true).clicked() {
                        revert_now = true;
                        close = true;
                    }
                    if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                        close = true;
                    }
                });
                return;
            }
            Dialog::Print => {
                let Some((i, id)) = app.active_ids() else {
                    close = true;
                    return;
                };
                let Some(doc) = app.session.get(id) else { return };
                let sizes: Vec<(f64, f64)> = doc.info.pages.iter().map(|p| (p.width as f64, p.height as f64)).collect();
                let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
                let rasters = crate::print_ui::preview_rasters(
                    &app.print_draft,
                    &sizes,
                    &labels,
                    ui.ctx().pixels_per_point(),
                    ui.ctx().input(|i| i.max_texture_side) as f32,
                );
                let view = &mut app.views[i];
                view.queue_print_previews(&rasters);
                let (go, cancel) = crate::print_ui::body(ui, &mut app.print_draft, &t, &sizes, &labels, &mut |p| {
                    view.need_thumbnail(p, true);
                    view.page_preview(p)
                });
                print_go = go;
                close = go || cancel;
                return;
            }
            Dialog::RemoveHidden => {
                let (ok, cancel) = crate::redact_ui::hidden_body(ui, &mut app.hidden_draft, &t);
                if ok {
                    redact_now = Some(dialog);
                }
                close = ok || cancel;
                return;
            }
            Dialog::Sanitize => {
                let (ok, cancel) = crate::redact_ui::sanitize_body(ui, &t);
                if ok {
                    redact_now = Some(dialog);
                }
                close = ok || cancel;
                return;
            }
            Dialog::FieldProps => {
                let Some(d) = app.field_props.as_mut() else {
                    close = true;
                    return;
                };
                let (apply, cancel) = crate::prepare::body(ui, d, &t);
                field_props_now = apply;
                close = apply || cancel;
                return;
            }
            Dialog::BulkFieldProps => {
                let Some(d) = app.bulk_field_props.as_mut() else {
                    close = true;
                    return;
                };
                let (apply, cancel) = crate::bulk_fields::body(ui, d, &t);
                bulk_field_props_now = apply;
                close = cancel;
                return;
            }
            Dialog::CommentProps => {
                let (apply, cancel) = crate::comment_props::body(ui, app, &t);
                props_now = apply;
                close = apply || cancel;
                return;
            }
            Dialog::Signature => {
                let (apply, cancel, browse) = crate::fill_sign::signature_pad(ui, &t, &mut app.signature_draft, &mut app.signature_preview);
                if browse {
                    app.pick_signature_image();
                }
                if apply {
                    let d = std::mem::take(&mut app.signature_draft);
                    let tool = if d.initials {
                        app.initials = d.saved();
                        crate::fill_sign::FillTool::Initials
                    } else {
                        app.signature = d.saved();
                        crate::fill_sign::FillTool::Signature
                    };
                    app.quick_tool = crate::QuickTool::Fill(tool);
                    app.toast = None;
                }
                close = apply || cancel;
                return;
            }
            Dialog::AltText => {
                let (save, cancel) = crate::a11y_ui::alt_body(ui, app, &t);
                alt_now = save;
                close = save || cancel;
                return;
            }
            Dialog::CreateStamp => {
                let (save, cancel) = crate::stamps_ui::create_body(ui, app, &t);
                stamp_now = save;
                close = save || cancel;
                return;
            }
            Dialog::PdfA => {
                close = crate::standards_ui::body(ui, app, &t);
                return;
            }
            Dialog::ActionWizard => {
                ui.set_width(620.0);
                close = crate::actions_ui::body(ui, app, &t);
                return;
            }
            Dialog::CompareFiles => {
                let (go, cancel) = crate::compare_ui::body(ui, app, &t);
                compare_now = go;
                close = go || cancel;
                return;
            }
            Dialog::JsConsole => {
                ui.set_width(640.0);
                close = crate::js_ui::console_body(ui, app, &t);
                return;
            }
            Dialog::DocumentJs => {
                ui.set_width(640.0);
                close = crate::js_ui::document_js_body(ui, app, &t);
                return;
            }
            Dialog::Preferences => {
                close = crate::js_ui::preferences_body(ui, app, &t);
                return;
            }
            Dialog::RecognizeText => {
                let (go, cancel) = crate::ocr_ui::body(ui, app, &t);
                ocr_now = go;
                close = go || cancel;
                return;
            }
            Dialog::AccessibilityOptions => {
                let (start, cancel) = crate::a11y_ui::options_body(ui, app, &t);
                a11y_now = start;
                close = start || cancel;
                return;
            }
            Dialog::Export(kind) => {
                let (apply, cancel) = crate::export_ui::body(ui, app, &t, kind);
                export_now = apply;
                close = apply || cancel;
                return;
            }
            Dialog::Marks(kind) => {
                ui.set_width(720.0);
                let (apply, cancel) = crate::marks_ui::body(ui, app, &t, kind);
                marks_now = apply;
                close = apply || cancel;
                return;
            }
            Dialog::PageBoxes => {
                let (apply, cancel) = crate::pageboxes::body(ui, app, &t);
                boxes_now = apply;
                close = apply || cancel;
                return;
            }
            Dialog::Protect => {
                let (apply, cancel) = crate::protect::body(ui, app, &t);
                protect_now = apply;
                close = apply || cancel;
                return;
            }
            Dialog::NumberPages => {
                use pdfcraft_engine::LabelStyle as L;
                ui.label(egui::RichText::new(tl!("Number pages")).font(theme::semibold(18.0)));
                ui.add_space(8.0);
                let Some((_, id)) = app.active_ids() else { return };
                let n = app.session.get(id).map(|d| d.info.pages.len()).unwrap_or(1).max(1);
                let d = &mut app.number_draft;
                d.to = d.to.clamp(1, n);
                d.from = d.from.clamp(1, d.to);
                // Editable values get a visible border (the dialog and field fills are alike).
                let boxed = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui) -> egui::Response| {
                    egui::Frame::new()
                        .stroke(egui::Stroke::new(1.0, t.border))
                        .corner_radius(egui::CornerRadius::same(5))
                        .inner_margin(egui::Margin::symmetric(4, 1))
                        .show(ui, |ui| add(ui))
                        .inner
                };
                egui::Grid::new("number_pages").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    ui.label(tl!("Pages"));
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut d.from).range(1..=n));
                        ui.label(tl!("to"));
                        ui.add(egui::DragValue::new(&mut d.to).range(1..=n));
                        ui.label(egui::RichText::new(crate::i18n::fmt(tl!("of {name}"), &[("name", &n.to_string())])).color(t.text_muted));
                    });
                    ui.end_row();
                    ui.label(tl!("Style"));
                    let styles = [
                        (L::Decimal, "1, 2, 3"),
                        (L::LowerRoman, "i, ii, iii"),
                        (L::UpperRoman, "I, II, III"),
                        (L::LowerAlpha, "a, b, c"),
                        (L::UpperAlpha, "A, B, C"),
                        (L::None, tl!("None (prefix only)")),
                    ];
                    let current = styles.iter().find(|(s, _)| *s == d.style).map_or("1, 2, 3", |(_, l)| *l);
                    egui::ComboBox::from_id_salt("label_style").selected_text(current).show_ui(ui, |ui| {
                        for (s, l) in styles {
                            ui.selectable_value(&mut d.style, s, l);
                        }
                    });
                    ui.end_row();
                    let l = ui.label(tl!("Prefix"));
                    boxed(ui, &mut |ui| ui.add(egui::TextEdit::singleline(&mut d.prefix).desired_width(160.0).frame(egui::Frame::NONE)))
                        .labelled_by(l.id);
                    ui.end_row();
                    ui.label(tl!("Start"));
                    ui.add(egui::DragValue::new(&mut d.start).range(1..=99_999));
                    ui.end_row();
                });
                d.to = d.to.max(d.from);
                let label = |k: u32| format!("{}{}", d.prefix, d.style.format(k));
                ui.add_space(8.0);
                let preview = if d.from == d.to {
                    label(d.start)
                } else {
                    format!("{}, {} … {}", label(d.start), label(d.start + 1), label(d.start + (d.to - d.from) as u32))
                };
                ui.label(
                    egui::RichText::new(crate::i18n::fmt(tl!("Labels: {preview}. Later pages keep their labels."), &[("preview", &preview)]))
                        .color(t.text_muted),
                );
                number_now = Some(Edit::NumberPages { from: d.from - 1, to: d.to - 1, style: d.style, prefix: d.prefix.clone(), first: d.start });
            }
            Dialog::Recovery => {
                ui.horizontal(|ui| {
                    ui.add(crate::icons::image("clock-3", 22.0, t.accent));
                    ui.label(egui::RichText::new(tl!("Recover unsaved documents?")).font(theme::semibold(18.0)));
                });
                ui.add_space(6.0);
                ui.label(tl!("PdfCraft didn't shut down normally. These documents had changes that were autosaved:"));
                ui.add_space(8.0);
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                egui::Grid::new("recoverable").num_columns(2).spacing([18.0, 6.0]).show(ui, |ui| {
                    for m in &app.recoverable {
                        ui.label(egui::RichText::new(&m.name).font(theme::medium(13.0)));
                        let mins = now.saturating_sub(m.saved_at) / 60;
                        let when = match mins {
                            0 => tl!("just now").to_string(),
                            1..=59 => crate::i18n::fmt(tl!("{n} min ago"), &[("n", &mins.to_string())]),
                            _ => crate::i18n::fmt(tl!("{n} h ago"), &[("n", &(mins / 60).to_string())]),
                        };
                        let lock = if m.encrypted { tl!(" · password-protected") } else { "" };
                        ui.label(egui::RichText::new(format!("{when}{lock}")).color(t.text_muted));
                        ui.end_row();
                    }
                });
            }
            Dialog::Shortcuts => {
                ui.label(egui::RichText::new(tl!("Keyboard shortcuts")).font(theme::semibold(18.0)));
                ui.add_space(8.0);
                use crate::commands::{
                    ACTUAL_SIZE, COPY, FIND_NEXT, FIND_PREV, FIT_WIDTH, PAGE_LEVEL, PAGE_NEXT, PAGE_PREV, ROTATE_CCW, ROTATE_CW, SELECT_ALL, ZOOM_IN,
                    ZOOM_OUT,
                };
                // Registered commands first (always in sync with the real bindings), then the
                // keys the document view handles itself.
                let mut rows: Vec<(String, String)> = pdfcraft_engine::commands::COMMANDS
                    .iter()
                    .filter_map(|c| {
                        c.shortcut.map(|k| (crate::commands::shortcut_label(ui.ctx(), k), tl!(c.label).trim_end_matches('…').to_string()))
                    })
                    .collect();
                let key = |s: pdfcraft_engine::commands::Shortcut| crate::commands::shortcut_label(ui.ctx(), s);
                let pair = |a, b| format!("{} / {}", key(a), key(b));
                // Key names, translated where a catalog has them (not scanned as UI literals).
                let named = |k: &str| tl!(k).to_string();
                let scroll = crate::i18n::fmt(
                    tl!("Zoom in / out (also pinch or {key}-scroll)"),
                    &[("key", crate::commands::command_modifier_label(ui.ctx()))],
                );
                for (k, v) in [
                    (pair(FIND_NEXT, FIND_PREV), tl!("Next / previous match").to_string()),
                    (key(COPY), tl!("Copy selected text").to_string()),
                    (named("Double-click"), tl!("Select a word").to_string()),
                    (crate::i18n::key_name("Esc").to_string(), tl!("Clear selection / close find").to_string()),
                    (key(ACTUAL_SIZE), tl!("Actual size").to_string()),
                    (key(PAGE_LEVEL), tl!("Zoom to page level").to_string()),
                    (key(FIT_WIDTH), tl!("Fit to width").to_string()),
                    (pair(ZOOM_IN, ZOOM_OUT), scroll),
                    (pair(ROTATE_CW, ROTATE_CCW), tl!("Rotate view").to_string()),
                    (format!("{} / {}", crate::i18n::key_name("Home"), crate::i18n::key_name("End")), tl!("First / last page").to_string()),
                    (format!("← / →, {}", pair(PAGE_PREV, PAGE_NEXT)), tl!("Previous / next page").to_string()),
                    (named("V"), tl!("Select (V)").to_string()),
                    (named("H / Space (hold)"), tl!("Hand (H)").to_string()),
                    (crate::i18n::key_name("Delete").to_string(), tl!("Delete selected pages (Organize)").to_string()),
                    (key(SELECT_ALL), tl!("Select all pages (Organize)").to_string()),
                ] {
                    rows.push((k, v));
                }
                egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| {
                    egui::Grid::new("keys").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                        for (k, v) in &rows {
                            ui.label(egui::RichText::new(k).font(egui::FontId::monospace(12.5)));
                            ui.label(v);
                            ui.end_row();
                        }
                    });
                });
            }
            Dialog::About => {
                // Tabs About · Contributors · Models (craftrules standards/contributors.md).
                let tab_id = egui::Id::new("about_tab");
                let mut tab = ui.data_mut(|d| d.get_temp::<u8>(tab_id)).unwrap_or(0);
                ui.horizontal(|ui| {
                    for (i, label) in ["About", "Contributors", "Models"].into_iter().enumerate() {
                        let i = i as u8;
                        if widgets::mode_tab(ui, tl!(label), tab == i).clicked() {
                            tab = i;
                        }
                    }
                });
                ui.data_mut(|d| d.insert_temp(tab_id, tab));
                ui.separator();
                match tab {
                    1 => crate::credits::contributors_ui(ui),
                    2 => crate::credits::models_ui(ui),
                    _ => {
                        ui.horizontal(|ui| {
                            widgets::artcraft_mark(ui, 40.0);
                            ui.vertical(|ui| {
                                ui.label(egui::RichText::new("PdfCraft").font(theme::semibold(20.0)));
                                ui.label(crate::i18n::fmt(tl!("Version {v}"), &[("v", env!("CARGO_PKG_VERSION"))]));
                            });
                        });
                        ui.add_space(6.0);
                        ui.label(tl!("A clean-room, open-source PDF application written in Rust. MIT OR Apache-2.0."));
                        ui.label(
                            egui::RichText::new(
                                "Rendering: hayro (bootstrap) · UI: egui · Icons: Lucide (ISC) · Fonts: Inter, JetBrains Mono, Dancing Script (OFL)",
                            )
                            .color(t.text_muted)
                            .small(),
                        );
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(tl!("Part of")).color(t.text_muted));
                            widgets::artcraft_logo(ui, 16.0);
                        });
                        ui.add_space(6.0);
                        if let Some(cmd) = widgets::community_links(ui) {
                            link_command = Some(cmd);
                        }
                    }
                }
            }
        }
        ui.add_space(12.0);
        let changed = draft_changes(app).is_some_and(|c| !c.is_empty());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if dialog == Dialog::Recovery {
                if widgets::pill_button(ui, tl!("Recover"), true).clicked() {
                    recover = Some(true);
                    close = true;
                }
                if widgets::pill_button(ui, tl!("Discard"), false).clicked() {
                    recover = Some(false);
                    close = true;
                }
            } else if dialog == Dialog::NumberPages {
                if widgets::pill_button(ui, tl!("OK"), true).clicked() {
                    apply_number = true;
                    close = true;
                }
                if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                    close = true;
                }
            } else if dialog == Dialog::Split {
                if ui.add_enabled_ui(split_ready.is_some(), |ui| widgets::pill_button(ui, tl!("Split"), true)).inner.clicked() {
                    split_now = split_ready.clone();
                    close = true;
                }
                if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                    close = true;
                }
            } else if changed {
                if widgets::pill_button(ui, tl!("OK"), true).clicked() {
                    apply = true;
                    close = true;
                }
                if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                    close = true;
                }
            } else if widgets::pill_button(ui, tl!("Close"), true).clicked() {
                close = true;
            }
        });
    });
    if let Some(yes) = recover {
        let keys: Vec<String> = app.recoverable.iter().map(|m| m.key.clone()).collect();
        if yes {
            app.recover(&keys);
        } else {
            app.discard_recovered(&keys);
        }
    }
    // The pages to replace belong to the document the dialog was opened on (#167).
    if replace_now
        && let Some(d) = app.replace_draft.take()
        && app.still_pick_target(d.target)
    {
        let n = d.to - d.from + 1;
        app.apply_edit(Edit::ReplacePages {
            pages: (d.from - 1..d.to).collect(),
            name: d.name.clone(),
            bytes: d.bytes.clone(),
            src_pages: (d.src_from - 1..d.src_from - 1 + n).collect(),
        });
    }
    // A print or save that fails keeps the dialog open, with the reason in a notice.
    if print_go && !app.print_now() {
        close = false;
    }
    if revert_now {
        app.revert_active();
    }
    if summarize_now {
        app.summarize_comments();
    }
    if optimize_now {
        app.optimize_with_draft();
    }
    if let Some(e) = duplicate_now {
        app.duplicate_draft = None;
        app.apply_edit(e);
    }
    if extract_now {
        app.extract_selection();
    }
    if let Some(e) = link_now {
        app.apply_edit(e);
        app.link_draft = None;
    }
    if rotate_now {
        app.rotate_with_draft();
    }
    if import_words {
        app.pick_files(crate::files::FilePurpose::RedactWords, false);
    }
    match redact_now {
        Some(Dialog::RedactPages) => app.redact_pages(),
        Some(Dialog::RedactSearch) => {
            let n = app.redact_search();
            app.redact_search.found = Some(n);
        }
        Some(Dialog::RemoveHidden) => {
            let which: Vec<pdfcraft_engine::Hidden> = app.hidden_draft.found.iter().filter(|f| f.2 && f.1 > 0).map(|f| f.0).collect();
            let n: usize = app.hidden_draft.found.iter().filter(|f| f.2).map(|f| f.1).sum();
            if app.apply_edit(Edit::RemoveHidden { which }) {
                if n == 1 {
                    app.notify_tr("Removed 1 hidden item. Save to remove them from the file.");
                } else {
                    app.notify_fmt("Removed {n} hidden items. Save to remove them from the file.", &[("n", &n.to_string())]);
                }
            }
        }
        Some(Dialog::Sanitize) if app.apply_edit(Edit::Sanitize) => {
            app.notify_tr("Document sanitized. Save to finish: saving rewrites the whole file.");
        }
        Some(Dialog::RedactApply) => {
            let marks = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(0, |d| d.redaction_marks());
            if app.apply_edit(Edit::ApplyRedactions { pages: None }) {
                if marks == 1 {
                    app.notify_tr("Applied 1 redaction mark. Save to remove the content from the file.");
                } else {
                    app.notify_fmt("Applied {n} redaction marks. Save to remove the content from the file.", &[("n", &marks.to_string())]);
                }
            }
        }
        _ => {}
    }
    if bulk_field_props_now {
        close = app.apply_bulk_field_props();
    }
    if field_props_now
        && let Some(d) = app.field_props.take()
        && let Some(props) = d.props()
        && app.apply_edit(Edit::SetFieldProps { name: d.field.clone(), props: Box::new(props) })
        && let Some(i) = app.active
    {
        // Keep the (possibly renamed) field selected.
        let prefix = d.field.rsplit_once('.').map(|(p, _)| format!("{p}.")).unwrap_or_default();
        app.views[i].prepare.selected = Some((format!("{prefix}{}", d.name.trim()), d.widget));
    }
    if props_now && let Some(d) = app.comment_props.take() {
        let mut edits = crate::comment_props::edits(&d);
        match edits.len() {
            0 => {}
            1 => {
                app.apply_edit(edits.remove(0));
            }
            _ => {
                app.apply_edit(Edit::Batch { label: "Change comment properties".into(), edits });
            }
        }
    }
    if export_now && let Dialog::Export(kind) = dialog {
        app.start_export(kind);
    }
    if marks_now && let (Dialog::Marks(kind), Some((_, id))) = (dialog, app.active_ids()) {
        let count = app.session.get(id).map(|d| d.info.pages.len()).unwrap_or(0);
        let edit = crate::marks_ui::edit(&app.marks_draft, kind, count);
        app.apply_edit(edit);
    }
    if boxes_now && let Some((i, id)) = app.active_ids() {
        let count = app.session.get(id).map(|d| d.info.pages.len()).unwrap_or(0);
        let edit = app.boxes_draft.edit(app.views[i].current, count);
        app.apply_edit(edit);
        app.boxes_draft.seeded = None;
    }
    if protect_now && app.apply_edit(app.protect_draft.edit()) {
        app.protect_draft = Default::default();
        app.notify_tr("Password protection will be applied when you save");
    }
    if apply_number && let Some(edit) = number_now {
        app.apply_edit(edit);
    }
    if let Some(by) = split_now {
        app.split_active(&by);
    }
    if apply && let Some(edits) = draft_changes(app) {
        app.apply_edit(Edit::Batch { label: "Change document properties".into(), edits });
    }
    // Protect / Remove security replace the Properties dialog.
    let replaces = link_command.is_some_and(|c| c.starts_with("protect.")) || open_revision.is_some();
    if modal.should_close() || close || replaces {
        app.dialog = None;
        app.props_draft = None;
        app.view_draft = None;
        app.bulk_field_props = None;
    } else {
        app.dialog = Some(next);
    }
    if let Some(cmd) = link_command {
        app.execute(cmd);
    }
    if let Some(n) = open_revision {
        app.open_revision(n);
    }
    if alt_now {
        app.save_alt_text();
    }
    if stamp_now {
        app.save_custom_stamp();
    }
    if images_now {
        app.finish_image_import();
    } else if dialog == Dialog::CreateImages && app.dialog != Some(Dialog::CreateImages) {
        app.image_import = None;
    }
    if ocr_now {
        app.start_ocr();
    }
    if compare_now {
        app.run_compare();
    }
    if a11y_now {
        app.a11y_skipped.clear();
        app.run_accessibility_check();
    }
}

/// Info edits needed to make the document match the Description draft.
fn draft_changes(app: &PdfCraftApp) -> Option<Vec<Edit>> {
    let (id, draft) = app.props_draft.as_ref()?;
    let doc = app.session.get(*id)?;
    let mut edits: Vec<Edit> = INFO_KEYS
        .iter()
        .zip(draft.iter())
        .filter(|(k, v)| doc.info_value(k).unwrap_or_default().trim() != v.trim())
        .map(|(k, v)| Edit::SetInfo { key: (*k).to_string(), value: v.clone() })
        .collect();
    if let Some((vid, v)) = &app.view_draft
        && vid == id
        && *v != doc.initial_view()
    {
        edits.push(Edit::SetInitialView(Box::new(v.clone())));
    }
    Some(edits)
}

/// The "Save changes?" prompt's keys (issue #8), read before anything else can take them: Enter
/// saves (the default button) and Escape cancels; Don't save takes ⌘D / Ctrl+D, the macOS
/// convention, or Alt+D / Alt+N, the mnemonics Windows and Linux desktops use for it.
pub(crate) fn save_prompt_key(ctx: &egui::Context) -> Option<Option<bool>> {
    use egui::{Key, Modifiers};
    ctx.input_mut(|i| {
        if i.consume_key(Modifiers::NONE, Key::Enter) {
            Some(Some(true))
        } else if i.consume_key(Modifiers::NONE, Key::Escape) {
            Some(None)
        } else if [(Modifiers::COMMAND, Key::D), (Modifiers::ALT, Key::D), (Modifiers::ALT, Key::N)].into_iter().any(|(m, k)| i.consume_key(m, k)) {
            Some(Some(false))
        } else {
            None
        }
    })
}

/// "Save changes?" when closing a tab or quitting with unsaved edits.
fn save_prompt(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(req) = app.close_request else { return };
    let index = match req {
        CloseRequest::Tab(id) => app.views.iter().position(|v| v.id == id),
        CloseRequest::Quit | CloseRequest::All => app.first_dirty(),
    };
    let Some(name) = index.and_then(|i| app.views.get(i)).and_then(|v| app.session.get(v.id)).map(|d| d.name.clone()) else {
        // Nothing left to ask about (tab already gone or no dirty documents).
        app.resolve_close(ctx, Some(false));
        return;
    };
    let t = Tokens::get(ctx);
    // Wrapping alone (#161) still let a long enough name (a web `?file=` URL has no limit) grow
    // the prompt taller than the window (#236); 80 characters wrap to a few lines.
    let shown = shorten_middle(&name, 80);
    let mut choice: Option<Option<bool>> = None;
    let modal = egui::Modal::new(egui::Id::new("save_prompt")).show(ctx, |ui| {
        ui.set_width(420.0);
        ui.horizontal(|ui| {
            ui.add(crate::icons::image("save", 22.0, t.accent));
            let title = ui.add(
                egui::Label::new(
                    egui::RichText::new(crate::i18n::fmt(tl!("Save changes to “{name}” before closing?"), &[("name", &shown)]))
                        .font(theme::semibold(16.0)),
                )
                .wrap(),
            );
            if shown != name {
                title.on_hover_text(&name);
            }
        });
        ui.add_space(6.0);
        ui.label(egui::RichText::new(tl!("Your changes will be lost if you don't save them.")).color(t.text_muted));
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Save"), true).clicked() {
                choice = Some(Some(true));
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                choice = Some(None);
            }
            ui.add_space(24.0);
            if widgets::pill_button(ui, tl!("Don't save"), false).clicked() {
                choice = Some(Some(false));
            }
        });
    });
    if choice.is_none() && modal.should_close() {
        choice = Some(None);
    }
    if let Some(c) = choice {
        app.resolve_close(ctx, c);
    }
}

/// `name` cut to at most `max` characters by replacing its middle with "…", keeping the start and
/// the end, where the extension and version suffixes sit. Counts `char`s, so it never splits one.
fn shorten_middle(name: &str, max: usize) -> String {
    let count = name.chars().count();
    if count <= max {
        return name.to_string();
    }
    let tail = max / 4;
    let head: String = name.chars().take(max.saturating_sub(tail + 1)).collect();
    let end: String = name.chars().skip(count.saturating_sub(tail)).collect();
    format!("{head}…{end}")
}

/// "Open this web page?" when a document's link, button or script asks to open an address
/// (#90, #91). Shows where the address really goes and the whole address; Cancel is the default,
/// and Escape or clicking outside cancels.
fn link_prompt(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(pending) = app.pending_link.clone() else { return };
    let t = Tokens::get(ctx);
    let email = pending.url.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("mailto:"));
    let (title, open) = if email { (tl!("Write this email?"), tl!("Open email app")) } else { (tl!("Open this web page?"), tl!("Open link")) };
    let mut choice: Option<bool> = None;
    let modal = egui::Modal::new(egui::Id::new("link_prompt")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.horizontal(|ui| {
            ui.add(crate::icons::image("external-link", 22.0, t.accent));
            ui.label(egui::RichText::new(title).font(theme::semibold(16.0)));
        });
        ui.add_space(6.0);
        let template = if email {
            tl!("{who} in this document wants to open your email app with this address:")
        } else {
            tl!("{who} in this document wants to open this address in your browser:")
        };
        ui.label(crate::i18n::fmt(template, &[("who", tl!(pending.origin.noun()))]));
        ui.add_space(6.0);
        // The host as the browser will connect to it, in punycode when it is international, so a
        // lookalike such as `pаypal.com` (Cyrillic `а`) reads as `xn--pypal-4ve.com`. Its Unicode
        // form isn't repeated here: a whole-script lookalike would read as the real site.
        if let Some(host) = pdfcraft_engine::links::display_host(&pending.url) {
            ui.label(egui::RichText::new(&host.ascii).font(theme::semibold(14.0)));
            if host.mixed_scripts {
                let warning = tl!("This web address mixes letters from different alphabets, a common way to imitate another site's name.");
                ui.add(egui::Label::new(egui::RichText::new(warning).color(egui::Color32::from_rgb(0xD1, 0x3B, 0x3B))).wrap());
            } else if host.international {
                let note = tl!("This web address uses letters from another alphabet, which can look like familiar ones.");
                ui.add(egui::Label::new(egui::RichText::new(note).color(t.text)).wrap());
            }
        }
        egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
            ui.add(egui::Label::new(egui::RichText::new(&pending.url).monospace().small()).wrap().selectable(true));
        });
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(tl!(
                "Only continue if you trust this document. An address can carry information from the document, such as what you typed into its form."
            ))
            .color(t.text_muted)
            .small(),
        );
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Cancel"), true).clicked() {
                choice = Some(false);
            }
            if widgets::pill_button(ui, open, false).clicked() {
                choice = Some(true);
            }
        });
    });
    if choice.is_none() && modal.should_close() {
        choice = Some(false);
    }
    if let Some(open) = choice {
        app.resolve_pending_link(open);
    }
}

fn yes(b: bool) -> String {
    tl!(if b { "Yes" } else { "No" }).to_string()
}

/// Password prompt for encrypted documents (Acrobat: "Password" dialog on open).
fn password(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(prompt) = app.password_prompt.as_mut() else { return };
    let t = Tokens::get(ctx);
    let mut submit = false;
    let mut cancel = false;
    let modal = egui::Modal::new(egui::Id::new("password")).show(ctx, |ui| {
        ui.set_width(400.0);
        ui.horizontal(|ui| {
            ui.add(crate::icons::image("lock", 22.0, t.accent));
            ui.label(egui::RichText::new(tl!("Password required")).font(theme::semibold(17.0)));
        });
        ui.add_space(6.0);
        ui.add(egui::Label::new(crate::i18n::fmt(tl!("“{name}” is protected. Enter a password to open it."), &[("name", &prompt.name)])).wrap());
        ui.add_space(8.0);
        let r = ui.add(egui::TextEdit::singleline(&mut prompt.input).password(true).hint_text(tl!("Password")).desired_width(f32::INFINITY));
        // Enter submits. The field keeps focus (we request it every frame), so check the key
        // while it is focused as well as on the frame focus is lost.
        if (r.has_focus() || r.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            submit = true;
        }
        r.request_focus();
        if let Some(e) = &prompt.error {
            ui.add_space(4.0);
            // Known error text is translated; anything else passes through.
            ui.label(egui::RichText::new(tl!(e)).color(egui::Color32::from_rgb(0xD1, 0x3B, 0x3B)));
        }
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Open"), true).clicked() {
                submit = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
            }
        });
    });
    if submit {
        let pw = prompt.input.clone();
        app.submit_password(Some(pw));
    } else if cancel || modal.should_close() {
        app.submit_password(None);
    }
}
