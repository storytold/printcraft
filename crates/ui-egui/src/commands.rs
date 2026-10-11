//! What each registered command does in this frontend (`pdfcraft_engine::commands` says
//! what it is called, where it appears, which key runs it and when it is enabled).
//!
//! Menus, keyboard shortcuts, the palette, the tool panels and automation (`set_option`, and
//! the control channel later) all call `PdfCraftApp::execute`.

use pdfcraft_engine::Edit;
use pdfcraft_engine::commands::{self, COMMANDS, CommandSpec, Shortcut};

use crate::{
    Dialog, Mode, PdfCraftApp, PropsTab, RightPanel, SaveTarget,
    theme::{ThemeKind, ThemePreference},
    widgets,
};

/// Keys the document view handles itself (`canvas::shortcuts`), for the View menu, tooltips and
/// Help ▸ Keyboard shortcuts. Keep them in step with the bindings there.
pub(crate) const ACTUAL_SIZE: Shortcut = Shortcut::cmd("1");
pub(crate) const PAGE_LEVEL: Shortcut = Shortcut::cmd("0");
pub(crate) const FIT_WIDTH: Shortcut = Shortcut::cmd("2");
pub(crate) const ZOOM_IN: Shortcut = Shortcut::cmd("+");
pub(crate) const ZOOM_OUT: Shortcut = Shortcut::cmd("−");
pub(crate) const ROTATE_CW: Shortcut = Shortcut::cmd_shift("+");
pub(crate) const ROTATE_CCW: Shortcut = Shortcut::cmd_shift("−");
pub(crate) const PREV_VIEW: Shortcut = Shortcut::cmd("[");
pub(crate) const NEXT_VIEW: Shortcut = Shortcut::cmd("]");
pub(crate) const FIND_NEXT: Shortcut = Shortcut::cmd("G");
pub(crate) const FIND_PREV: Shortcut = Shortcut::cmd_shift("G");
pub(crate) const COPY: Shortcut = Shortcut::cmd("C");
pub(crate) const SELECT_ALL: Shortcut = Shortcut::cmd("A");
pub(crate) const PAGE_PREV: Shortcut = Shortcut::cmd("←");
pub(crate) const PAGE_NEXT: Shortcut = Shortcut::cmd("→");

/// Whether shortcuts are written the macOS way (`⇧⌘S`) rather than `Ctrl+Shift+S`. egui knows the
/// platform from the build target, and on the web from the browser's user agent.
pub(crate) fn mac_shortcuts(ctx: &egui::Context) -> bool {
    ctx.os() == egui::os::OperatingSystem::Mac
}

/// How `s` is written on this platform, using the resolved current interface language.
pub(crate) fn shortcut_label(ctx: &egui::Context, s: Shortcut) -> String {
    let key = crate::i18n::key_name(s.key);
    if mac_shortcuts(ctx) {
        Shortcut { key, ..s }.label(true)
    } else {
        let mut parts = Vec::new();
        if s.command || s.mac_ctrl {
            parts.push(crate::i18n::key_name("Ctrl"));
        }
        if s.shift {
            parts.push(crate::i18n::key_name("Shift"));
        }
        parts.push(key);
        parts.join("+")
    }
}

/// The command modifier for help text such as “Strg-scroll”; bindings stay in the engine.
pub(crate) fn command_modifier_label(ctx: &egui::Context) -> &'static str {
    if mac_shortcuts(ctx) { Shortcut::command_name(true) } else { crate::i18n::key_name("Ctrl") }
}

/// How a registered command's shortcut is written on this platform ("" without one).
pub(crate) fn command_shortcut_label(ctx: &egui::Context, id: &str) -> String {
    commands::command(id).and_then(|c| c.shortcut).map(|s| shortcut_label(ctx, s)).unwrap_or_default()
}

/// A translated tooltip with a `{key}` placeholder, filled with `s` as written on this platform:
/// "Zoom in ({key})" → "Zoom in (Ctrl++)".
pub(crate) fn key_tip(ctx: &egui::Context, template: &str, s: Shortcut) -> String {
    crate::i18n::fmt(template, &[("key", &shortcut_label(ctx, s))])
}

/// [`key_tip`] with a registered command's shortcut, so the tooltip names the real binding.
pub(crate) fn command_tip(ctx: &egui::Context, template: &str, id: &str) -> String {
    crate::i18n::fmt(template, &[("key", &command_shortcut_label(ctx, id))])
}

impl PdfCraftApp {
    /// Whether a registered command can run now. The engine judges the document (security,
    /// contents, undo history); view state it can't see is checked here.
    pub(crate) fn command_enabled(&self, spec: &CommandSpec) -> bool {
        // Undo and Redo act on the Combine files list while its tab shows.
        if self.combine_showing() && matches!(spec.needs, commands::Needs::Undo | commands::Needs::Redo) {
            return self.combine_can_undo(spec.needs == commands::Needs::Undo);
        }
        commands::is_enabled(spec, &self.session, self.active_ids().map(|(_, id)| id))
            && (spec.needs != commands::Needs::TwoPageView || self.active.and_then(|i| self.views.get(i)).is_some_and(crate::DocView::cover_applies))
    }

    /// Run a registered command by id. Returns `false` when the id is unknown or the command
    /// is disabled right now (the user is told why).
    ///
    /// Last-resort guard (AGENTS.md §4): a command that panics is reported and the app, with its
    /// open documents, keeps running. Edits are applied to a copy, so the document is unchanged.
    pub fn execute(&mut self, id: &str) -> bool {
        match pdfcraft_engine::guard(|| self.execute_unguarded(id)) {
            Ok(done) => done,
            Err(m) => {
                self.notify_fmt("That didn't work: an internal error stopped it ({m}). Your documents are unchanged.", &[("m", m.as_str())]);
                true
            }
        }
    }

    fn execute_unguarded(&mut self, id: &str) -> bool {
        let Some(spec) = commands::command(id) else { return false };
        if !self.command_enabled(spec) {
            let why = match spec.needs {
                commands::Needs::Undo => tl!("Nothing to undo").to_string(),
                commands::Needs::Redo => tl!("Nothing to redo").to_string(),
                commands::Needs::FillForms if self.active.is_some() => tl!("This document has no form fields you can fill in").to_string(),
                commands::Needs::HasComments if self.active.is_some() => tl!("This document has no comments to flatten").to_string(),
                commands::Needs::HasFields if self.active.is_some() => tl!("This document has no form fields to flatten").to_string(),
                commands::Needs::HasRedactions if self.active.is_some() => {
                    tl!("There are no redaction marks (mark text, areas or pages first)").to_string()
                }
                commands::Needs::Marks(k) if self.active.is_some() => {
                    let kind = match k {
                        pdfcraft_engine::MarkKind::HeaderFooter => tl!("header or footer"),
                        pdfcraft_engine::MarkKind::Watermark => tl!("watermark"),
                        pdfcraft_engine::MarkKind::Background => tl!("background"),
                    };
                    crate::i18n::fmt(tl!("This document has no {kind} to change"), &[("kind", kind)])
                }
                commands::Needs::Security | commands::Needs::ProtectedSecurity if self.active.is_some() => {
                    if self.active_ids().and_then(|(_, id)| self.session.get(id)).is_some_and(|d| d.allows_security_change()) {
                        tl!("This document isn't password-protected").to_string()
                    } else {
                        tl!("Only the document's owner can change its security (open it with the permissions password)").to_string()
                    }
                }
                commands::Needs::Assembly | commands::Needs::Modification | commands::Needs::Annotate if self.active.is_some() => {
                    tl!("The document's security settings don't allow this change").to_string()
                }
                commands::Needs::TwoPageView if self.active.is_some() => tl!("Switch to two-page view first to show the cover page").to_string(),
                _ => tl!("Open a document first").to_string(),
            };
            self.notify(why);
            return false;
        }
        let active = self.active;
        let targets = active.map(|i| self.views[i].target_pages()).unwrap_or_default();
        match id {
            "file.open" => self.open_dialog(),
            "file.open_recent" => match self.recent.first().map(|r| r.path.clone()) {
                // The palette runs commands without a submenu: open the most recent file.
                Some(p) => self.open_recent(&p),
                None => self.notify_tr("No recent files"),
            },
            "file.clear_recent" if self.recent.is_empty() => self.notify_tr("No recent files"),
            "file.clear_recent" => self.recent.clear(),
            "file.pin_folder" => self.pin_folder_dialog(),
            "page.combine" => self.open_combine_tab(),
            "file.save" => {
                self.save_active(SaveTarget::InPlace);
            }
            "file.save_as" => {
                self.save_active(SaveTarget::As);
            }
            "file.close" => {
                if let Some(i) = active {
                    self.request_close_tab(i);
                }
            }
            "file.close_all" => self.close_all(),
            "file.revert" => self.dialog = Some(Dialog::Revert),
            "file.properties" => self.dialog = Some(Dialog::Properties(PropsTab::Description)),
            "protect.properties" => self.dialog = Some(Dialog::Properties(PropsTab::Security)),
            "protect.password" => {
                self.protect_draft = Default::default();
                self.dialog = Some(Dialog::Protect);
            }
            "protect.remove" => {
                if self.apply_edit(Edit::RemoveProtection) {
                    self.notify_tr("Security will be removed when you save");
                }
            }
            "page.number" => {
                if let Some(i) = active {
                    let v = &self.views[i];
                    let pages: Vec<usize> = if v.selected.is_empty() { vec![v.current] } else { v.selected.iter().copied().collect() };
                    let (lo, hi) = (pages.iter().min().copied().unwrap_or(0), pages.iter().max().copied().unwrap_or(0));
                    self.number_draft = crate::NumberDraft { from: lo + 1, to: hi + 1, ..self.number_draft.clone() };
                }
                self.dialog = Some(Dialog::NumberPages);
            }
            link if pdfcraft_engine::links::for_command(link).is_some() => {
                if let Some(l) = pdfcraft_engine::links::for_command(link) {
                    self.open_url(l.url);
                }
            }
            "bookmark.add" => self.bookmark_action(crate::panels::BmAction::New),
            "bookmark.from_structure" => self.bookmark_action(crate::panels::BmAction::FromStructure),
            "edit.undo" => self.undo(),
            "edit.redo" => self.redo(),
            "edit.find" => {
                if let Some(i) = active {
                    self.views[i].open_find();
                }
            }
            "view.focus_page_input" => {
                if let (Some(ctx), Some(view)) = (self.ctx.clone(), active.and_then(|i| self.views.get(i))) {
                    // The page box in the toolbar (chrome.rs); select its number so typing replaces it.
                    let id = egui::Id::new("page-input");
                    ctx.memory_mut(|m| m.request_focus(id));
                    let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
                    let len = view.page_input.chars().count();
                    state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(len))));
                    state.store(&ctx, id);
                }
            }
            "view.palette" => self.palette_open = !self.palette_open,
            layout if crate::canvas::PageLayout::from_command(layout).is_some() => {
                if let (Some(i), Some(layout)) = (active, crate::canvas::PageLayout::from_command(layout)) {
                    self.views[i].set_layout(layout);
                }
            }
            // The Continuous scrolling checkbox (an independent toggle over the base layout).
            "view.continuous" => {
                if let Some(i) = active {
                    let v = &mut self.views[i];
                    v.set_continuous(!v.continuous());
                }
            }
            "view.layout.cover" => {
                if let Some(i) = active {
                    let v = &mut self.views[i];
                    v.set_cover(!v.cover);
                }
            }
            "view.fit_width_scrolling" | "view.fit_one_page" => {
                use crate::canvas::{Fit, PageLayout};
                // Fit one full page shows a single page on its own; fit width scrolls it.
                let (continuous, fit) = if id == "view.fit_one_page" { (false, Fit::Page) } else { (true, Fit::Width) };
                if let Some(i) = active {
                    self.views[i].set_display(PageLayout::Single, continuous);
                    self.views[i].set_fit(fit);
                }
            }
            "view.full_screen" => {
                let on = !self.full_screen;
                match self.ctx.clone() {
                    Some(ctx) => self.set_full_screen(&ctx, on),
                    None => self.full_screen = on,
                }
            }
            "view.read_mode" => self.mode = if self.mode == Mode::Read { Mode::AllTools } else { Mode::Read },
            "view.theme" => {
                let next = if self.theme == ThemeKind::Light { ThemePreference::Dark } else { ThemePreference::Light };
                self.set_theme_preference(next);
            }
            command if command.starts_with("measure.") => crate::measure_ui::command(self, command),
            "view.theme.system" => self.set_theme_preference(ThemePreference::System),
            "view.theme.light" => self.set_theme_preference(ThemePreference::Light),
            "view.theme.dark" => self.set_theme_preference(ThemePreference::Dark),
            "comment.list" => self.choose_right_panel(Some(RightPanel::Comments)),
            tool if crate::comments::CommentTool::from_command(tool).is_some() => {
                let Some(tool) = crate::comments::CommentTool::from_command(tool) else { return false };
                self.comment_prefs.group_tool[tool.group()] = tool;
                self.quick_tool = crate::QuickTool::Comment(tool);
                // Acrobat opens the Comments panel with the commenting tools, unless the user closed it.
                if self.right.is_none() && !self.comments_panel_closed {
                    self.right = Some(RightPanel::Comments);
                }
                // A text selection made before picking a markup tool is marked right away.
                if let (Some(kind), Some(i)) = (tool.markup(), active)
                    && let Some(doc) = self.session.get(self.views[i].id)
                {
                    let info = &doc.info;
                    if let Some((page, quads)) = self.views[i].selection_quads(info) {
                        self.views[i].clear_selection();
                        let style = self.comment_prefs.style(tool);
                        let author = self.comment_prefs.author.clone();
                        let shape = pdfcraft_engine::Shape::TextMarkup { kind, quads };
                        self.apply_edit(Edit::AddAnnotation(pdfcraft_engine::NewAnnotation { page, shape, style, contents: String::new(), author }));
                    }
                }
            }
            "form.fields" => self.right = Some(RightPanel::Fields),
            "comment.flatten" => {
                self.apply_edit(Edit::Flatten { comments: true, fields: false });
            }
            "form.flatten" => {
                // Flatten what's typed in a field too (#166); a refused value flattens nothing.
                if self.commit_form_typing() {
                    self.apply_edit(Edit::Flatten { comments: false, fields: true });
                }
            }
            "form.clear" => {
                if let Some(i) = active {
                    self.views[i].forms.focus = None;
                }
                self.apply_edit(Edit::ResetForm { names: None });
            }
            "page.organize" => {
                if let Some(i) = active {
                    self.views[i].organize = !self.views[i].organize;
                }
            }
            "page.rotate" => {
                self.apply_edit(Edit::RotatePages { pages: targets, degrees: 90 });
            }
            "page.rotate_ccw" => {
                self.apply_edit(Edit::RotatePages { pages: targets, degrees: -90 });
            }
            "page.delete" => {
                self.apply_edit(Edit::DeletePages { pages: targets });
            }
            "page.insert_blank" => {
                if let (Some(i), Some(&last)) = (active, targets.last())
                    && let Some(p) = self.session.get(self.views[i].id).and_then(|d| d.info.pages.get(last))
                {
                    let (w, h) = ((p.crop[2] - p.crop[0]).abs().max(1.0) as f64, (p.crop[3] - p.crop[1]).abs().max(1.0) as f64);
                    self.apply_edit(Edit::InsertBlankPage { at: last + 1, width: w, height: h });
                }
            }
            "page.insert" => self.insert_from_file_dialog(),
            "page.replace" => self.replace_pages_dialog(),
            "edit.header_footer"
            | "edit.watermark"
            | "edit.background"
            | "edit.header_footer.update"
            | "edit.watermark.update"
            | "edit.background.update" => {
                use pdfcraft_engine::MarkKind as K;
                let kind = if id.starts_with("edit.header_footer") {
                    K::HeaderFooter
                } else if id.starts_with("edit.watermark") {
                    K::Watermark
                } else {
                    K::Background
                };
                self.marks_draft.replace = id.ends_with(".update");
                self.dialog = Some(Dialog::Marks(kind));
            }
            "edit.bates" => {
                // Bates numbering ▸ Add: the header & footer dialog with a Bates number in the
                // bottom-right box (6 digits from 1), ready to edit.
                self.marks_draft.replace = false;
                if self.marks_draft.hf.text.iter().all(|t| !t.contains("<<Bates")) {
                    self.marks_draft.hf.text[5] = "<<Bates Number#6#1##>>".into();
                }
                self.marks_draft.focused_box = 5;
                self.dialog = Some(Dialog::Marks(pdfcraft_engine::MarkKind::HeaderFooter));
            }
            "edit.header_footer.remove" | "edit.watermark.remove" | "edit.background.remove" => {
                use pdfcraft_engine::MarkKind as K;
                let kind = match id {
                    "edit.header_footer.remove" => K::HeaderFooter,
                    "edit.watermark.remove" => K::Watermark,
                    _ => K::Background,
                };
                self.apply_edit(Edit::RemoveMarks { kind });
            }
            "export.image" => self.dialog = Some(Dialog::Export(crate::export_ui::ExportKind::Image)),
            "export.text" => self.dialog = Some(Dialog::Export(crate::export_ui::ExportKind::Text)),
            "a11y.check" => self.start_accessibility_check(),
            "ocr.recognize" => self.dialog = Some(Dialog::RecognizeText),
            "tools.js_console" => self.dialog = Some(Dialog::JsConsole),
            "tools.document_js" => {
                self.doc_js = Default::default();
                self.dialog = Some(Dialog::DocumentJs);
            }
            "app.preferences" => self.dialog = Some(Dialog::Preferences),
            "ocr.recognize_batch" => self.ocr_files_dialog(),
            "edit.edit_text" => {
                self.quick_tool = crate::QuickTool::EditText;
                self.left = crate::LeftPanel::Tool("edit");
                self.left_open = true;
                self.notify_tr("Click text or an image to edit it");
            }
            "edit.advanced_search" => {
                if let Some(i) = self.active {
                    let f = self.views[i].find.get_or_insert_with(Default::default);
                    f.in_panel = true;
                    f.focus = true;
                    self.right = Some(crate::RightPanel::Search);
                }
            }
            "a11y.report" => self.show_accessibility_report(),
            "a11y.alt_text" => self.start_alt_text(),
            "a11y.reading_options" => self.dialog = Some(Dialog::Properties(crate::PropsTab::Advanced)),
            "export.all_images" => self.dialog = Some(Dialog::Export(crate::export_ui::ExportKind::AllImages)),
            "edit.text" => {
                self.quick_tool = crate::QuickTool::AddText;
                self.left = crate::LeftPanel::Tool("edit");
                self.left_open = true;
            }
            "edit.image" => self.add_image_dialog(),
            "edit.link" => {
                self.quick_tool = crate::QuickTool::Link;
                self.notify_tr("Drag a rectangle to create a link; double-click a link to edit it");
            }
            "edit.links_from_urls" => self.links_from_urls(),
            "edit.remove_links" => {
                self.apply_edit(Edit::RemoveLinks { pages: None });
            }
            "redact.mark" => {
                self.quick_tool = crate::QuickTool::Redact;
                self.left = crate::LeftPanel::Tool("redact");
                self.left_open = true;
                // A text selection made first is marked right away.
                if let Some(i) = active
                    && let Some(doc) = self.session.get(self.views[i].id)
                {
                    let info = &doc.info;
                    if let Some((page, quads)) = self.views[i].selection_quads(info) {
                        self.views[i].clear_selection();
                        let author = self.comment_prefs.author.clone();
                        self.apply_edit(self.redact_prefs.mark(page, quads, &author));
                    }
                }
            }
            "redact.pages" => {
                if let Some(i) = active {
                    let n = self.session.get(self.views[i].id).map_or(1, |d| d.info.pages.len());
                    self.redact_pages_draft = crate::RedactPagesDraft { current: true, from: self.views[i].current + 1, to: n };
                }
                self.dialog = Some(Dialog::RedactPages);
            }
            "redact.search" => {
                self.redact_search.found = None;
                self.dialog = Some(Dialog::RedactSearch);
            }
            "redact.properties" => self.dialog = Some(Dialog::RedactProps),
            "redact.apply" => {
                let marks = active.and_then(|i| self.session.get(self.views[i].id)).map_or(0, |d| d.redaction_marks());
                if marks == 0 {
                    self.notify_tr("There are no redaction marks to apply");
                } else {
                    self.dialog = Some(Dialog::RedactApply);
                }
            }
            "protect.remove_hidden" => self.open_remove_hidden(),
            "print.dialog" => self.open_print(),
            "redact.sanitize" => self.dialog = Some(Dialog::Sanitize),
            "redact.clear" => {
                self.apply_edit(Edit::ClearRedactions);
            }
            "form.tab_order.row" | "form.tab_order.column" | "form.tab_order.structure" => {
                let order = match id {
                    "form.tab_order.row" => pdfcraft_engine::TabOrder::Row,
                    "form.tab_order.column" => pdfcraft_engine::TabOrder::Column,
                    _ => pdfcraft_engine::TabOrder::Structure,
                };
                let n = active.and_then(|i| self.session.get(self.views[i].id)).map_or(0, |d| d.info.pages.len());
                self.apply_edit(Edit::SetTabOrder { pages: (0..n).collect(), order });
            }
            "comment.import" | "form.import_data" => self.import_data_dialog(),
            "comment.stamp" => {
                self.left = crate::LeftPanel::Tool("stamp");
                self.left_open = true;
            }
            "comment.export" => self.export_data_dialog(true, false),
            "comment.summarize" => self.dialog = Some(Dialog::SummarizeComments),
            "sign.digital" | "sign.certify" => {
                let certify = id == "sign.certify";
                self.quick_tool = crate::QuickTool::SignArea { certify };
                self.notify_tr("Drag to draw the area where the signature should appear.");
            }
            "sign.certify_invisible" => {
                let page = active.map_or(0, |i| self.views[i].current);
                self.start_signing(page, None, None, Some(2));
            }
            "sign.validate" => {
                let certs = self.session.trusted_certificates().to_vec();
                self.session.set_trusted_certificates(certs);
                self.right = Some(RightPanel::Signatures);
            }
            "sign.panel" => self.right = Some(RightPanel::Signatures),
            "optimize.advanced" => self.dialog = Some(Dialog::Optimize),
            "view.fit_visible" => {
                if let Some(i) = self.active
                    && let Err(e) = self.fit_visible(i)
                {
                    self.notify_fmt("Couldn't fit the visible content: {e}", &[("e", &e.to_string())]);
                }
            }
            "view.marquee_zoom" => self.quick_tool = crate::QuickTool::MarqueeZoom,
            "edit.column_select" => self.quick_tool = crate::QuickTool::ColumnSelect,
            "edit.snapshot" => {
                self.quick_tool = crate::QuickTool::Snapshot;
                self.notify_tr("Drag a rectangle around the area to copy");
            }
            "page.copy" => self.copy_pages(false),
            "page.cut" => self.copy_pages(true),
            "page.paste" => self.paste_pages(),
            "comment.hide_all" => {
                if let Some(i) = active {
                    let id = self.views[i].id;
                    let hide = !self.session.get(id).is_some_and(|d| d.comments_hidden());
                    if self.session.set_hide_comments(id, hide) {
                        self.views[i].invalidate_content();
                    }
                }
            }
            "form.export_data" => self.export_data_dialog(false, true),
            "form.merge_data" => self.merge_data_dialog(),
            "export.docx" => self.export_office_dialog(pdfcraft_engine::compare::OfficeFormat::Docx),
            "export.html" => self.export_office_dialog(pdfcraft_engine::compare::OfficeFormat::Html),
            "export.rtf" => self.export_office_dialog(pdfcraft_engine::compare::OfficeFormat::Rtf),
            "form.prepare" => {
                self.left = crate::LeftPanel::Tool("form");
                self.left_open = true;
                self.right = Some(RightPanel::Fields);
                // Like Acrobat, a document without fields gets them detected on the way in.
                if let Some((_, id)) = self.active_ids()
                    && self.session.get(id).is_some_and(|d| d.form.is_empty() && d.editable())
                {
                    self.detect_fields();
                }
            }
            "form.detect" => self.detect_fields(),
            "doc.compare" => self.dialog = Some(Dialog::CompareFiles),
            "standards.pdfa" => {
                self.pdfa.issues = None;
                self.dialog = Some(Dialog::PdfA);
            }
            "actions.wizard" | "actions.distribution" | "actions.optimize_scans" => {
                self.wizard.editing = None;
                match id {
                    "actions.distribution" => self.wizard.selected = Some("Prepare for Distribution".into()),
                    "actions.optimize_scans" => self.wizard.selected = Some("Optimize Scanned Documents".into()),
                    _ => {}
                }
                self.dialog = Some(Dialog::ActionWizard);
            }
            "form.field.properties" => {
                if let Some((name, w)) = active.and_then(|i| self.views[i].prepare.selected.clone()) {
                    self.open_field_props(&name, w);
                }
            }
            field if crate::prepare::FieldTool::from_command(field).is_some() => {
                let Some(tool) = crate::prepare::FieldTool::from_command(field) else { return false };
                self.quick_tool = crate::QuickTool::Field(tool);
                self.left = crate::LeftPanel::Tool("form");
                self.left_open = true;
                if let Some(i) = active {
                    self.views[i].forms.focus = None;
                }
                self.notify_fmt(
                    "Click on the page to add a {tool}, or drag to set its size",
                    &[("tool", &crate::i18n::in_sentence(tl!(tool.label())))],
                );
            }
            "sign.fill.signature.remove" => self.signature = None,
            "sign.fill.initials.remove" => self.initials = None,
            "sign.fill.signature.change" | "sign.fill.initials.change" => {
                let initials = id == "sign.fill.initials.change";
                let saved = if initials { self.initials.as_ref() } else { self.signature.as_ref() };
                self.signature_draft = saved.map_or_else(
                    || crate::fill_sign::SigDraft::new(initials, &self.comment_prefs.author),
                    |s| crate::fill_sign::SigDraft::from_saved(initials, s),
                );
                self.dialog = Some(Dialog::Signature);
            }
            fill if crate::fill_sign::FillTool::from_command(fill).is_some() => {
                let Some(tool) = crate::fill_sign::FillTool::from_command(fill) else { return false };
                self.quick_tool = crate::QuickTool::Fill(tool);
                let initials = tool == crate::fill_sign::FillTool::Initials;
                if (tool == crate::fill_sign::FillTool::Signature && self.signature.is_none()) || (initials && self.initials.is_none()) {
                    self.signature_draft = crate::fill_sign::SigDraft::new(initials, &self.comment_prefs.author);
                    self.dialog = Some(Dialog::Signature);
                }
            }
            "create.blank" => self.create_blank(),
            "create.file" => self.open_dialog(),
            "create.multiple" => self.create_multiple_dialog(),
            "create.images" => self.create_from_images_dialog(),
            "create.clipboard" => self.create_from_clipboard(),
            "optimize.reduce" => self.reduce_file_size(),
            "page.duplicate" => {
                self.apply_edit(Edit::DuplicatePages { pages: targets });
            }
            "page.crop" => {
                self.quick_tool = crate::QuickTool::Crop;
                self.notify_tr("Drag a rectangle on a page to crop it; double-click a page for Set Page Boxes");
            }
            "page.boxes" => {
                self.boxes_draft.seeded = None;
                self.dialog = Some(Dialog::PageBoxes);
            }
            "page.extract" => self.open_extract_dialog(),
            "page.rotate_dialog" => {
                if let Some(i) = active {
                    let n = self.session.get(self.views[i].id).map_or(1, |d| d.info.pages.len());
                    self.rotate_draft.to = n;
                    self.rotate_draft.which = if self.views[i].selected.is_empty() { 0 } else { 1 };
                }
                self.dialog = Some(Dialog::RotatePages);
            }
            "page.split" => self.dialog = Some(Dialog::Split),
            "help.shortcuts" => self.dialog = Some(Dialog::Shortcuts),
            "help.about" => self.dialog = Some(Dialog::About),
            "help.check_updates" => self.check_for_updates(),
            _ => return false,
        }
        true
    }

    /// Run the registered keyboard shortcuts (more specific combinations first).
    pub(crate) fn registry_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let typing = ctx.egui_wants_keyboard_input();
        // A form field's editor is open on the page: its text isn't in the document until
        // committed. (Not egui's keyboard focus: an Escape in this frame has already cleared that,
        // while the field has yet to see the Escape and discard its draft.)
        let active = self.active_ids();
        let form_typing = active.and_then(|(i, _)| self.views.get(i)).is_some_and(|v| v.forms.focus.is_some());
        let mut specs: Vec<&CommandSpec> = COMMANDS.iter().filter(|c| c.shortcut.is_some()).collect();
        specs.sort_by_key(|c| std::cmp::Reverse(c.shortcut.map(|s| s.modifier_count()).unwrap_or(0)));
        for spec in specs {
            let Some(s) = spec.shortcut else { continue };
            if typing && !spec.in_text {
                continue;
            }
            let Some(key) = Key::from_name(s.key) else { continue };
            let mut m = Modifiers::NONE;
            if s.command {
                m |= Modifiers::COMMAND;
            }
            if s.shift {
                m |= Modifiers::SHIFT;
            }
            if s.mac_ctrl {
                m |= Modifiers::CTRL;
            }
            if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, key))) {
                if form_typing {
                    // A form field has the keyboard: let it take this frame's typing (and
                    // Escape) first, so ⌘S saves what's on screen (#166). Runs next frame, for
                    // this document only.
                    self.deferred_commands.push((spec.id, active.map(|(_, id)| id)));
                    ctx.request_repaint();
                } else {
                    self.execute(spec.id);
                }
            }
        }
    }
}

/// Render a top-level menu's registered commands (with live labels, shortcuts and enablement).
pub(crate) fn registry_menu(app: &mut PdfCraftApp, ui: &mut egui::Ui, menu: &str) {
    for spec in commands::menu(menu) {
        let label = commands::current_label(spec, &app.session, app.active_ids().map(|(_, id)| id));
        let label = crate::i18n::menu_label(spec.id, &label);
        // Open Recent is a submenu of the live recent list, not one action: disabled while the
        // list is empty, otherwise each entry opens its file (or focuses the tab showing it), and
        // Clear Recent Files at the foot empties the list (#430).
        if spec.id == "file.open_recent" {
            if app.recent.is_empty() {
                ui.add_enabled(false, egui::Button::new(label));
                continue;
            }
            let mut open: Option<String> = None;
            let mut clear = false;
            ui.menu_button(label, |ui| {
                for r in &app.recent {
                    if ui.button(&r.name).on_hover_text(&r.path).clicked() {
                        open = Some(r.path.clone());
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button(tl!("Clear Recent Files")).clicked() {
                    clear = true;
                    ui.close();
                }
            });
            if let Some(p) = open {
                app.open_recent(&p);
                ui.close();
            }
            if clear {
                app.execute("file.clear_recent");
                ui.close();
            }
            continue;
        }
        let shortcut = spec.shortcut.map(|s| shortcut_label(ui.ctx(), s)).unwrap_or_default();
        let enabled = app.command_enabled(spec);
        let resp = ui.add_enabled(enabled, egui::Button::new(label).shortcut_text(shortcut));
        if resp.clicked() {
            app.execute(spec.id);
            ui.close();
        }
    }
    let _ = widgets::menu_item;
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::os::OperatingSystem;

    #[test]
    fn localized_shortcut_labels_follow_language_and_platform() {
        let previous = crate::i18n::current();
        let ctx = egui::Context::default();
        for os in [OperatingSystem::Windows, OperatingSystem::Nix] {
            ctx.set_os(os);
            crate::i18n::set_current(crate::i18n::Lang::from_code("de").unwrap());
            assert_eq!(shortcut_label(&ctx, Shortcut::cmd("K")), "Strg+K");
            assert_eq!(shortcut_label(&ctx, Shortcut::cmd_shift("S")), "Strg+Umschalt+S");
            assert_eq!(command_shortcut_label(&ctx, "view.read_mode"), "Strg+H");
            assert_eq!(command_modifier_label(&ctx), "Strg");
            assert_eq!(Shortcut::cmd_shift("S").label(false), "Ctrl+Shift+S", "engine/control labels keep their stable spelling");
            crate::i18n::set_current(crate::i18n::Lang::EN);
            assert_eq!(shortcut_label(&ctx, Shortcut::cmd_shift("S")), "Ctrl+Shift+S");
        }
        ctx.set_os(OperatingSystem::Mac);
        crate::i18n::set_current(crate::i18n::Lang::from_code("de").unwrap());
        assert_eq!(shortcut_label(&ctx, Shortcut::cmd_shift("S")), "⇧⌘S");
        assert_eq!(command_shortcut_label(&ctx, "view.read_mode"), "⌃⌘H");
        assert_eq!(command_modifier_label(&ctx), "⌘");
        crate::i18n::set_current(previous);
    }
}
