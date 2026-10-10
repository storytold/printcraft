//! Comment Properties (Acrobat: right-click a comment ▸ Properties…; audit "Comment
//! properties"): Appearance (colour, fill, opacity, line thickness, note icon), General (author,
//! subject, modified) and Review History (status changes), with Acrobat's Locked box.

use egui::{Align, Layout};
use pdfcraft_engine::{CommentProps, Edit, LineEnding, NoteIcon};

use crate::comments::swatch_grid;
use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropsTab {
    Appearance,
    General,
    ReviewHistory,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PropsDraft {
    pub page: usize,
    pub index: usize,
    pub tab: PropsTab,
    pub original: CommentProps,
    pub edited: CommentProps,
}

const ICONS: [NoteIcon; 7] =
    [NoteIcon::Comment, NoteIcon::Note, NoteIcon::Help, NoteIcon::Insert, NoteIcon::Key, NoteIcon::NewParagraph, NoteIcon::Paragraph];

/// The name shown in the line-ending combo. [`LineEnding::name`] stays the PDF `/LE` token.
fn ending_label(ending: LineEnding) -> &'static str {
    match ending {
        LineEnding::None => tl!("None"),
        LineEnding::Square => tl!("Square"),
        LineEnding::Circle => tl!("Circle"),
        LineEnding::Diamond => tl!("Diamond"),
        LineEnding::OpenArrow => tl!("Open arrow"),
        LineEnding::ClosedArrow => tl!("Closed arrow"),
        LineEnding::Butt => tl!("Butt"),
        LineEnding::ROpenArrow => tl!("Reverse open arrow"),
        LineEnding::RClosedArrow => tl!("Reverse closed arrow"),
        LineEnding::Slash => tl!("Slash"),
    }
}

impl PdfCraftApp {
    /// Attach file: ask for a file (or take `attach_override`) and attach it at `at`.
    pub fn attach_file_comment(&mut self, page: usize, at: [f64; 2]) {
        match self.attach_override.take() {
            Some((file, data)) => self.add_attachment_comment(page, at, file, data),
            #[cfg(not(target_arch = "wasm32"))]
            None => {
                // Attached on a later frame, and only to the document it was started on.
                let target = self.active_ids().map(|(_, id)| id);
                let dialog = rfd::AsyncFileDialog::new().set_title(tl!("Attach a file"));
                self.ask_one(crate::pickers::Ask::File(dialog), target, move |app, p| match std::fs::read(&p) {
                    Ok(data) => {
                        app.add_attachment_comment(page, at, p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), data)
                    }
                    Err(e) => app.notify_fmt("Couldn't read {name}: {e}", &[("name", &p.display().to_string()), ("e", &e.to_string())]),
                });
            }
            #[cfg(target_arch = "wasm32")]
            None => {}
        }
    }

    /// Add a file attachment comment at `at` on `page` of the active document.
    fn add_attachment_comment(&mut self, page: usize, at: [f64; 2], file: String, data: Vec<u8>) {
        let tool = crate::comments::CommentTool::Attach;
        let shape = pdfcraft_engine::Shape::Attachment { at, icon: pdfcraft_engine::AttachIcon::PushPin, file, data };
        let edit = Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
            page,
            shape,
            style: self.comment_prefs.style(tool),
            contents: String::new(),
            author: self.comment_prefs.author.clone(),
        });
        // A one-shot tool: back to Select unless pinned (apply_edit acts on this).
        if let Some((i, _)) = self.active_ids() {
            self.views[i].comments.tool_done = true;
        }
        self.apply_edit(edit);
    }

    /// Make Current Properties Default: new comments of this kind take this one's colour,
    /// opacity and line width.
    pub fn make_comment_default(&mut self, page: usize, index: usize) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let Some(a) = doc.info.annotations.iter().find(|a| a.page == page && a.index == index && a.in_reply_to.is_none()) else { return };
        let Some(tool) = crate::comments::tool_for(a) else { return };
        let Some(p) = doc.comment_props(page, index) else { return };
        let mut style = self.comment_prefs.style(tool);
        if let Some(c) = p.color {
            style.color = c;
        }
        style.opacity = p.opacity;
        if let Some(w) = p.width {
            style.width = w;
        }
        if p.fillable && tool.has_fill() {
            style.fill = p.fill;
        }
        self.comment_prefs.set_style(tool, style);
        self.notify_fmt("New {tool} comments will look like this one", &[("tool", &crate::i18n::in_sentence(tl!(tool.label())))]);
    }

    /// Open Comment Properties for the comment at `(page, index)` of the active document.
    pub fn open_comment_props(&mut self, page: usize, index: usize) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(p) = self.session.get(id).and_then(|d| d.comment_props(page, index)) else { return };
        self.comment_props = Some(PropsDraft { page, index, tab: PropsTab::Appearance, original: p.clone(), edited: p });
        self.dialog = Some(crate::Dialog::CommentProps);
    }
}

/// The edits that make the comment match the draft (empty when nothing changed).
pub fn edits(d: &PropsDraft) -> Vec<Edit> {
    let (o, e) = (&d.original, &d.edited);
    let mut out = Vec::new();
    // A locked comment refuses other changes: unlock first, lock last.
    if o.locked && !e.locked {
        out.push(Edit::LockAnnotation { page: d.page, index: d.index, locked: false });
    }
    let color = (e.color != o.color).then_some(e.color).flatten();
    let opacity = ((e.opacity - o.opacity).abs() > 1e-6).then_some(e.opacity);
    let width = (e.width != o.width).then_some(e.width).flatten();
    let endings = (e.endings != o.endings).then(|| e.endings.clone()).flatten();
    if e.restylable && (color.is_some() || opacity.is_some() || width.is_some() || endings.is_some()) {
        out.push(Edit::StyleAnnotation { page: d.page, index: d.index, color, opacity, width, endings });
    }
    if e.restylable && e.fillable && e.fill != o.fill {
        out.push(Edit::FillAnnotation { page: d.page, index: d.index, fill: e.fill });
    }
    let author = (e.author != o.author).then(|| e.author.clone());
    let subject = (e.subject != o.subject).then(|| e.subject.clone());
    let icon = (e.icon != o.icon).then_some(e.icon).flatten();
    if author.is_some() || subject.is_some() || icon.is_some() {
        out.push(Edit::SetAnnotationInfo { page: d.page, index: d.index, author, subject, icon });
    }
    if !o.locked && e.locked {
        out.push(Edit::LockAnnotation { page: d.page, index: d.index, locked: true });
    }
    out
}

/// Draw the dialog; returns (apply, cancel).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let history: Vec<(String, String, String)> = app
        .active_ids()
        .and_then(|(_, id)| app.session.get(id))
        .zip(app.comment_props.as_ref())
        .map(|(doc, d)| {
            let me = doc.info.annotations.iter().find(|a| a.page == d.page && a.index == d.index && a.in_reply_to.is_none());
            let nm = me.and_then(|a| a.name.clone());
            doc.info
                .annotations
                .iter()
                .filter(|r| r.state.is_some() && nm.is_some() && r.in_reply_to == nm)
                .map(|r| (r.state.clone().unwrap_or_default(), r.author.clone().unwrap_or_default(), r.modified.clone().unwrap_or_default()))
                .collect()
        })
        .unwrap_or_default();
    let Some(d) = app.comment_props.as_mut() else { return (false, true) };
    let kind = tl!(crate::comments_panel::subtype_label(&d.edited.subtype)).to_string();
    ui.label(egui::RichText::new(crate::i18n::fmt(tl!("{kind} Properties"), &[("kind", &kind)])).font(theme::semibold(18.0)));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        for (tab, label) in
            [(PropsTab::Appearance, tl!("Appearance")), (PropsTab::General, tl!("General")), (PropsTab::ReviewHistory, tl!("Review History"))]
        {
            if widgets::mode_tab(ui, label, d.tab == tab).clicked() {
                d.tab = tab;
            }
        }
    });
    ui.separator();
    ui.add_space(6.0);
    let e = &mut d.edited;
    match d.tab {
        PropsTab::Appearance => {
            if !e.restylable {
                ui.label(egui::RichText::new(tl!("This comment's appearance can't be changed yet.")).color(t.text_muted));
            }
            ui.add_enabled_ui(e.restylable, |ui| {
                egui::Grid::new("comment-props").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                    if let Some(icon) = e.icon.as_mut() {
                        ui.label(tl!("Icon"));
                        egui::ComboBox::from_id_salt("note-icon").selected_text(icon.name()).show_ui(ui, |ui| {
                            for i in ICONS {
                                ui.selectable_value(icon, i, i.name());
                            }
                        });
                        ui.end_row();
                    }
                    ui.label(tl!("Colour"));
                    if let Some(c) = swatch_grid(ui, e.color) {
                        e.color = Some(c);
                    }
                    ui.end_row();
                    if e.fillable {
                        ui.label(tl!("Fill colour"));
                        if let Some(f) = crate::comments::fill_picker(ui, e.fill) {
                            e.fill = f;
                        }
                        ui.end_row();
                    }
                    ui.label(tl!("Opacity"));
                    let mut pct = e.opacity * 100.0;
                    if ui.add(egui::Slider::new(&mut pct, 0.0..=100.0).suffix("%")).changed() {
                        e.opacity = pct / 100.0;
                    }
                    ui.end_row();
                    if let Some(w) = e.width.as_mut() {
                        ui.label(tl!("Thickness"));
                        ui.add(egui::Slider::new(w, 0.5..=12.0).step_by(0.5).suffix(" pt"));
                        ui.end_row();
                    }
                    if let Some(ends) = e.endings.as_mut() {
                        ui.label(tl!("Line ending"));
                        ui.horizontal(|ui| {
                            for (i, ending) in ends.iter_mut().enumerate() {
                                egui::ComboBox::from_id_salt(("line-ending", i)).selected_text(ending_label(*ending)).width(188.0).show_ui(
                                    ui,
                                    |ui| {
                                        for style in LineEnding::ALL {
                                            ui.selectable_value(ending, style, ending_label(style));
                                        }
                                    },
                                );
                            }
                        });
                        ui.end_row();
                    }
                });
            });
        }
        PropsTab::General => {
            egui::Grid::new("comment-general").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                let l = ui.label(tl!("Author"));
                ui.add(egui::TextEdit::singleline(&mut e.author).desired_width(280.0)).labelled_by(l.id);
                ui.end_row();
                let l = ui.label(tl!("Subject"));
                ui.add(egui::TextEdit::singleline(&mut e.subject).desired_width(280.0)).labelled_by(l.id);
                ui.end_row();
                ui.label(tl!("Modified"));
                ui.label(e.modified.as_deref().map(pdfcraft_render::pretty_date).unwrap_or_else(|| "—".into()));
                ui.end_row();
            });
        }
        PropsTab::ReviewHistory => {
            if history.is_empty() {
                ui.label(egui::RichText::new(tl!("No status has been set.")).color(t.text_muted));
            }
            for (state, who, when) in &history {
                ui.label(format!("{} — {who}  {when}", tl!(state)));
            }
        }
    }
    ui.add_space(12.0);
    ui.checkbox(&mut d.edited.locked, tl!("Locked"));
    let (mut apply, mut cancel) = (false, false);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if widgets::pill_button(ui, tl!("OK"), true).clicked() {
            apply = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    (apply, cancel)
}
