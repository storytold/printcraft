//! Redact a PDF (execution plan M8.5–M8.6): mark text and areas with the Redact tool (drag across
//! text to mark it, drag elsewhere to mark a box), mark whole pages, find text or patterns and
//! mark every match, set the default box colour and overlay text, and apply all marks (with a
//! confirmation, as Acrobat asks) or clear them.

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke};
use pdfcraft_engine::{Edit, NewAnnotation, REDACT_PATTERNS, REDACTION_CODE_SETS, RedactPattern, RedactionCodeSet, Rgb, Shape, Style, rect_quad};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageXform};
use crate::theme::Tokens;
use crate::{PdfCraftApp, widgets};

const MARK_RED: Color32 = Color32::from_rgb(0xE3, 0x22, 0x22);

/// Redaction Tool Properties: what new marks look like once applied.
#[derive(Clone, Debug, PartialEq)]
pub struct RedactPrefs {
    /// Box colour (`None` = no box, the content just disappears).
    pub fill: Option<Rgb>,
    pub use_overlay: bool,
    pub overlay: String,
    /// The overlay text is the picked redaction codes instead of `overlay`.
    pub use_code: bool,
    pub code_set: RedactionCodeSet,
    pub codes: Vec<&'static str>,
    /// Overlay text font, size (0 = auto), colour, alignment and repetition.
    pub look: pdfcraft_engine::OverlayLook,
}

impl Default for RedactPrefs {
    fn default() -> Self {
        Self {
            fill: Some([0.0, 0.0, 0.0]),
            use_overlay: false,
            overlay: String::new(),
            use_code: false,
            // Infallible: a non-empty constant array.
            code_set: REDACTION_CODE_SETS[0],
            codes: Vec::new(),
            look: Default::default(),
        }
    }
}

impl RedactPrefs {
    pub fn overlay_text(&self) -> String {
        match (self.use_overlay, self.use_code) {
            (false, _) => String::new(),
            (true, false) => self.overlay.clone(),
            // `codes` only ever holds codes of `code_set`, so this can't fail.
            (true, true) => self.code_set.overlay(&self.codes).unwrap_or_default(),
        }
    }

    pub fn mark(&self, page: usize, quads: Vec<[f64; 8]>, author: &str) -> Edit {
        let shape = Shape::Redact { quads, overlay: self.overlay_text(), look: self.look };
        let mut style = Style::default_for(&shape);
        style.fill = self.fill;
        Edit::AddAnnotation(NewAnnotation { page, shape, style, contents: String::new(), author: author.to_string() })
    }
}

/// Redact pages: which pages to mark.
#[derive(Clone, Debug, PartialEq)]
pub struct PagesDraft {
    pub current: bool,
    pub from: usize,
    pub to: usize,
}

impl Default for PagesDraft {
    fn default() -> Self {
        Self { current: true, from: 1, to: 1 }
    }
}

/// What Find text and redact looks for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchMode {
    #[default]
    Phrase,
    /// Every word or phrase of a list, one per line.
    Words,
    Patterns,
}

/// Find text and redact.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchDraft {
    pub mode: SearchMode,
    pub text: String,
    /// The word list, one word or phrase per line.
    pub words: String,
    pub pattern: RedactPattern,
    /// The result of the last search (matches marked), shown in the dialog.
    pub found: Option<usize>,
}

impl Default for SearchDraft {
    fn default() -> Self {
        Self { mode: SearchMode::Phrase, text: String::new(), words: String::new(), pattern: RedactPattern::Phone, found: None }
    }
}

/// The largest word list file Import list accepts.
pub(crate) const MAX_WORD_LIST_BYTES: usize = 1 << 20;

/// A box being drawn with the Redact tool: (page, start).
pub type AreaDrag = Option<(usize, Pos2)>;

fn to_user(xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> [f64; 2] {
    let (vx, vy) = xf.screen_to_view(p);
    let u = info.pages[page].view_to_user(vx, vy);
    [u[0] as f64, u[1] as f64]
}

/// Redact tool input on a page. Presses on text are left to text selection (the selection is
/// marked when it ends, see [`after_text`]); presses elsewhere draw a box. Returns `true` when
/// the gesture is a box.
pub(crate) fn page_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: &PageXform,
    page: usize,
    info: &DocInfo,
    over_text: impl Fn(Pos2) -> bool,
    view: &mut DocView,
) -> bool {
    let pointer = ui.input(|i| i.pointer.hover_pos().or(i.pointer.interact_pos()));
    if let Some((dp, start)) = view.redact_drag
        && dp == page
    {
        if resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
            view.redact_drag = None;
            let end = pointer.unwrap_or(start);
            let r = Rect::from_two_pos(start, end).intersect(xf.rect);
            if r.width() >= 3.0 && r.height() >= 3.0 {
                let (a, b) = (to_user(xf, info, page, r.min), to_user(xf, info, page, r.max));
                view.pending_redaction = Some((page, vec![rect_quad([a[0], a[1], b[0], b[1]])]));
            }
        }
        return true;
    }
    let Some(p) = pointer.filter(|p| xf.rect.contains(*p)) else { return false };
    if !over_text(p) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    if resp.drag_started() {
        let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
        if !over_text(origin) {
            view.redact_drag = Some((page, origin));
            return true;
        }
    }
    false
}

/// A finished text selection with the Redact tool becomes a mark.
pub(crate) fn after_text(resp: &egui::Response, page: usize, info: &DocInfo, view: &mut DocView) {
    if !(resp.drag_stopped() || resp.double_clicked()) || view.redact_drag.is_some() {
        return;
    }
    if let Some((p, quads)) = view.selection_quads(info).filter(|(p, _)| *p == page) {
        view.clear_selection();
        view.pending_redaction = Some((p, quads));
    }
}

/// The box being drawn.
pub(crate) fn paint(ui: &egui::Ui, painter: &egui::Painter, page: usize, view: &DocView) {
    if let (Some((dp, start)), Some(p)) = (view.redact_drag, ui.input(|i| i.pointer.hover_pos()))
        && dp == page
    {
        let r = Rect::from_two_pos(start, p);
        painter.rect_filled(r, CornerRadius::ZERO, MARK_RED.gamma_multiply(0.12));
        painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.5, MARK_RED), egui::StrokeKind::Inside);
    }
}

impl PdfCraftApp {
    /// Mark every match of the search draft on every page; returns how many were marked.
    pub fn redact_search(&mut self) -> usize {
        let Some((_, id)) = self.active_ids() else { return 0 };
        let Some(doc) = self.session.get(id) else { return 0 };
        let d = self.redact_search.clone();
        let needles = match d.mode {
            SearchMode::Phrase if !d.text.trim().is_empty() => vec![d.text.clone()],
            SearchMode::Words => pdfcraft_engine::redact_word_list(&d.words),
            SearchMode::Phrase | SearchMode::Patterns => Vec::new(),
        };
        if d.mode != SearchMode::Patterns && needles.is_empty() {
            return 0;
        }
        let config = pdfcraft_render::RenderConfig { password: doc.password.as_deref().map(std::sync::Arc::from), ..Default::default() };
        let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
        let mut edits = Vec::new();
        let author = self.comment_prefs.author.clone();
        for page in 0..doc.info.pages.len() {
            let out = r.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
            let Some(text) = out.text else { continue };
            let hits = match d.mode {
                SearchMode::Patterns => text.find_with(|c| pdfcraft_engine::find_pattern(d.pattern, c)),
                _ => needles.iter().flat_map(|w| text.find(w)).collect(),
            };
            for h in hits {
                let quads: Vec<[f64; 8]> = text.line_rects(h).into_iter().map(|r| doc.info.pages[page].view_rect_to_quad(r)).collect();
                if !quads.is_empty() {
                    edits.push(self.redact_prefs.mark(page, quads, &author));
                }
            }
        }
        let n = edits.len();
        if n > 0 {
            self.apply_edit(Edit::Batch { label: "Mark for redaction".into(), edits });
        }
        n
    }

    /// Mark the pages of the Redact pages draft.
    pub fn redact_pages(&mut self) {
        let Some((i, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let n = doc.info.pages.len();
        let d = self.redact_pages_draft.clone();
        let pages: Vec<usize> = if d.current { vec![self.views[i].current] } else { (d.from.max(1) - 1..d.to.min(n)).collect() };
        let author = self.comment_prefs.author.clone();
        let edits: Vec<Edit> = pages
            .iter()
            .map(|&p| {
                let c = doc.info.pages[p].crop;
                self.redact_prefs.mark(p, vec![rect_quad([c[0] as f64, c[1] as f64, c[2] as f64, c[3] as f64])], &author)
            })
            .collect();
        match <[Edit; 1]>::try_from(edits) {
            Ok([one]) => {
                self.apply_edit(one);
            }
            Err(edits) if edits.is_empty() => {}
            Err(edits) => {
                self.apply_edit(Edit::Batch { label: "Mark pages for redaction".into(), edits });
            }
        }
    }
}

/// Redact pages ("Mark Page Range"). Returns (apply, cancel).
pub(crate) fn pages_body(ui: &mut egui::Ui, d: &mut PagesDraft, pages: usize, _t: &Tokens) -> (bool, bool) {
    ui.label(egui::RichText::new(tl!("Mark Page Range")).font(crate::theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.radio_value(&mut d.current, true, tl!("Current page"));
    ui.horizontal(|ui| {
        ui.radio_value(&mut d.current, false, tl!("Pages from"));
        ui.add_enabled(!d.current, egui::DragValue::new(&mut d.from).range(1..=pages));
        ui.label(tl!("to"));
        ui.add_enabled(!d.current, egui::DragValue::new(&mut d.to).range(1..=pages));
        ui.label(crate::i18n::fmt(tl!("of {name}"), &[("name", &pages.to_string())]));
    });
    d.to = d.to.max(d.from);
    ui.add_space(12.0);
    buttons(ui, "OK", true)
}

/// Find text and redact. Returns (search, cancel, import a word list).
pub(crate) fn search_body(ui: &mut egui::Ui, d: &mut SearchDraft, t: &Tokens) -> (bool, bool, bool) {
    ui.set_width(420.0);
    ui.label(egui::RichText::new(tl!("Find text and redact")).font(crate::theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.radio_value(&mut d.mode, SearchMode::Phrase, tl!("Single word or phrase"));
    let mut enter = false;
    ui.add_enabled_ui(d.mode == SearchMode::Phrase, |ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut d.text).hint_text(tl!("Text to find")).desired_width(380.0));
        enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    });
    ui.add_space(6.0);
    ui.radio_value(&mut d.mode, SearchMode::Words, tl!("Multiple words or phrases"));
    let mut import = false;
    ui.add_enabled_ui(d.mode == SearchMode::Words, |ui| {
        egui::ScrollArea::vertical().id_salt("redact-words").max_height(120.0).show(ui, |ui| {
            ui.add(egui::TextEdit::multiline(&mut d.words).hint_text(tl!("One word or phrase per line")).desired_width(380.0).desired_rows(4));
        });
        ui.horizontal(|ui| {
            import = ui.button(tl!("Import list…")).clicked();
            let n = pdfcraft_engine::redact_word_list(&d.words).len();
            ui.label(egui::RichText::new(crate::i18n::fmt(tl!("{n} word(s) or phrase(s)"), &[("n", &n.to_string())])).small().color(t.text_faint));
        });
    });
    ui.add_space(6.0);
    ui.radio_value(&mut d.mode, SearchMode::Patterns, tl!("Patterns"));
    ui.add_enabled_ui(d.mode == SearchMode::Patterns, |ui| {
        egui::ComboBox::from_id_salt("redact-pattern").selected_text(tl!(d.pattern.label())).width(240.0).show_ui(ui, |ui| {
            for p in REDACT_PATTERNS {
                ui.selectable_value(&mut d.pattern, p, tl!(p.label()));
            }
        });
    });
    ui.add_space(6.0);
    ui.label(egui::RichText::new(tl!("Text in images isn't found; recognise text (OCR) first.")).small().color(t.text_faint));
    if let Some(n) = d.found {
        ui.label(
            egui::RichText::new(if n == 0 {
                tl!("No matches.").to_string()
            } else {
                crate::i18n::fmt(tl!("{n} match(es) marked for redaction."), &[("n", &n.to_string())])
            })
            .color(t.text_muted),
        );
    }
    ui.add_space(12.0);
    let (go, cancel) = buttons(ui, "Mark all", true);
    (go || enter, cancel, import)
}

/// Redaction Tool Properties. Returns (apply, cancel).
pub(crate) fn props_body(ui: &mut egui::Ui, d: &mut RedactPrefs, _t: &Tokens) -> (bool, bool) {
    ui.set_width(380.0);
    ui.label(egui::RichText::new(tl!("Redaction Tool Properties")).font(crate::theme::semibold(18.0)));
    ui.add_space(8.0);
    egui::Grid::new("redact-props").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
        ui.label(tl!("Redacted area fill colour:"));
        ui.horizontal(|ui| {
            let mut none = d.fill.is_none();
            if ui.checkbox(&mut none, tl!("No colour")).changed() {
                d.fill = if none { None } else { Some([0.0, 0.0, 0.0]) };
            }
        });
        ui.end_row();
        ui.label("");
        if let Some(c) = crate::comments::swatch_grid(ui, d.fill) {
            d.fill = Some(c);
        }
        ui.end_row();
        ui.label("");
        ui.checkbox(&mut d.use_overlay, tl!("Use overlay text"));
        ui.end_row();
        let on = d.use_overlay;
        let l = ui.add_enabled(on, egui::RadioButton::new(!d.use_code, tl!("Custom text:")));
        if l.clicked() {
            d.use_code = false;
        }
        ui.add_enabled(on && !d.use_code, egui::TextEdit::singleline(&mut d.overlay).desired_width(220.0)).labelled_by(l.id);
        ui.end_row();
        if ui.add_enabled(on, egui::RadioButton::new(d.use_code, tl!("Redaction code:"))).clicked() {
            d.use_code = true;
        }
        let codes_on = on && d.use_code;
        ui.add_enabled_ui(codes_on, |ui| {
            egui::ComboBox::from_id_salt("redaction-code-set").selected_text(d.code_set.name).show_ui(ui, |ui| {
                for set in REDACTION_CODE_SETS {
                    if ui.selectable_label(d.code_set == set, set.name).clicked() && d.code_set != set {
                        d.code_set = set;
                        d.codes.clear();
                    }
                }
            });
        });
        ui.end_row();
        for row in d.code_set.codes.chunks(4) {
            ui.label("");
            ui.add_enabled_ui(codes_on, |ui| {
                ui.horizontal(|ui| {
                    for &code in row {
                        let mut picked = d.codes.contains(&code);
                        if ui.add_sized([62.0, 18.0], egui::Checkbox::new(&mut picked, code)).changed() {
                            d.codes.retain(|c| *c != code);
                            if picked {
                                d.codes.push(code);
                            }
                        }
                    }
                });
            });
            ui.end_row();
        }
        let look = &mut d.look;
        ui.label(tl!("Font:"));
        ui.add_enabled_ui(on, |ui| {
            egui::ComboBox::from_id_salt("overlay-font").selected_text(look.font.name()).show_ui(ui, |ui| {
                for f in pdfcraft_engine::OverlayFont::ALL {
                    ui.selectable_value(&mut look.font, f, f.name());
                }
            });
        });
        ui.end_row();
        ui.label(tl!("Font size:"));
        ui.add_enabled_ui(on, |ui| {
            ui.horizontal(|ui| {
                let mut auto = look.size <= 0.0;
                if ui.checkbox(&mut auto, tl!("Auto-size text to fit redaction region")).changed() {
                    look.size = if auto { 0.0 } else { 10.0 };
                }
                if !auto {
                    ui.add(egui::DragValue::new(&mut look.size).range(2.0..=144.0).suffix(" pt"));
                }
            });
        });
        ui.end_row();
        ui.label(tl!("Font colour:"));
        ui.add_enabled_ui(on, |ui| {
            if let Some(c) = crate::comments::swatch_grid(ui, Some(look.color)) {
                look.color = c;
            }
        });
        ui.end_row();
        ui.label("");
        ui.add_enabled(on, egui::Checkbox::new(&mut look.repeat, tl!("Repeat overlay text")));
        ui.end_row();
        ui.label(tl!("Text alignment:"));
        ui.add_enabled_ui(on, |ui| {
            ui.horizontal(|ui| {
                for (a, label) in [(0u8, tl!("Left")), (1, tl!("Center")), (2, tl!("Right"))] {
                    ui.radio_value(&mut look.align, a, label);
                }
            });
        });
        ui.end_row();
    });
    ui.add_space(12.0);
    buttons(ui, "OK", true)
}

/// Apply redactions confirmation. Applying can't be undone, so it asks for an explicit
/// acknowledgement (`ack`); `blocked` is why redaction isn't possible in this document, if so.
/// Returns (apply, cancel).
pub(crate) fn apply_body(ui: &mut egui::Ui, marks: usize, blocked: Option<&str>, ack: &mut bool, t: &Tokens) -> (bool, bool) {
    ui.set_width(420.0);
    ui.label(egui::RichText::new(tl!("Apply redactions")).font(crate::theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.label(if marks == 1 {
        tl!("You are about to apply 1 redaction mark. Text, images and drawings under the marks, and comments and form fields that overlap them, are removed permanently.").to_string()
    } else {
        crate::i18n::fmt(tl!("You are about to apply {n} redaction marks. Text, images and drawings under the marks, and comments and form fields that overlap them, are removed permanently."), &[("n", &marks.to_string())])
    });
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(tl!("Applying also sanitizes the document: bookmarks, named destinations, attachments, document information, metadata and XFA form data are deleted, and hidden layers are locked hidden for good."))
            .small()
            .color(t.text_muted),
    );
    ui.add_space(4.0);
    ui.label(egui::RichText::new(tl!("This cannot be undone: applying redactions clears the undo history.")).color(t.text_muted));
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(tl!(
            "Save the result under a new name: saving rewrites the whole file so no trace of the removed content stays in it, and the original file stays as it is."
        ))
        .small()
        .color(t.text_muted),
    );
    ui.add_space(8.0);
    if let Some(reason) = blocked {
        ui.label(egui::RichText::new(reason).color(Color32::from_rgb(0xC0, 0x30, 0x30)));
        ui.add_space(8.0);
    } else {
        ui.checkbox(ack, tl!("I understand that the redacted content is removed for good"));
        ui.add_space(8.0);
    }
    let go = blocked.is_none() && *ack;
    let (mut a, mut c) = (false, false);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.add_enabled_ui(go, |ui| widgets::pill_button(ui, "Apply", true)).inner.clicked() {
            a = true;
        }
        if widgets::pill_button(ui, "Cancel", false).clicked() {
            c = true;
        }
    });
    (a, c)
}

fn buttons(ui: &mut egui::Ui, ok: &str, primary: bool) -> (bool, bool) {
    let (mut a, mut c) = (false, false);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        // `ok` stays English internally; only the display is translated.
        if widgets::pill_button(ui, tl!(ok), primary).clicked() {
            a = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            c = true;
        }
    });
    (a, c)
}

/// Remove Hidden Information: each category with what was found, checked by default when
/// something was.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HiddenDraft {
    pub found: Vec<(pdfcraft_engine::Hidden, usize, bool)>,
}

impl PdfCraftApp {
    pub fn open_remove_hidden(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        self.hidden_draft = HiddenDraft { found: doc.hidden_info().into_iter().map(|(h, n)| (h, n, n > 0)).collect() };
        self.dialog = Some(crate::Dialog::RemoveHidden);
    }
}

/// Returns (remove, cancel).
pub(crate) fn hidden_body(ui: &mut egui::Ui, d: &mut HiddenDraft, t: &Tokens) -> (bool, bool) {
    ui.set_width(440.0);
    ui.label(egui::RichText::new(tl!("Remove hidden information")).font(crate::theme::semibold(18.0)));
    ui.add_space(4.0);
    ui.label(egui::RichText::new(tl!("Select the items to remove from this document.")).color(t.text_muted));
    ui.add_space(8.0);
    let total: usize = d.found.iter().map(|f| f.1).sum();
    for (h, n, on) in d.found.iter_mut() {
        ui.add_enabled_ui(*n > 0, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(on, tl!(h.label()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(if *n == 0 { tl!("None found").to_string() } else { n.to_string() }).color(t.text_muted));
                });
            });
        });
    }
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(tl!("Form fields are flattened: their values stay visible. Saving rewrites the whole file.")).small().color(t.text_faint),
    );
    ui.add_space(12.0);
    let any = d.found.iter().any(|f| f.2 && f.1 > 0);
    let (mut a, mut c) = (false, false);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.add_enabled_ui(any && total > 0, |ui| widgets::pill_button(ui, tl!("Remove"), true)).inner.clicked() {
            a = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            c = true;
        }
    });
    (a, c)
}

/// Sanitize Document confirmation. Returns (sanitize, cancel).
pub(crate) fn sanitize_body(ui: &mut egui::Ui, t: &Tokens) -> (bool, bool) {
    ui.set_width(440.0);
    ui.label(egui::RichText::new(tl!("Sanitize document")).font(crate::theme::semibold(18.0)));
    ui.add_space(8.0);
    ui.label(tl!("Sanitizing removes hidden information from the document: metadata, file attachments, comments, form fields (flattened), hidden text and layers, bookmarks, links, actions and scripts, and private application data."));
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(tl!("Save the document afterwards; saving rewrites the whole file so nothing removed stays in it."))
            .small()
            .color(t.text_muted),
    );
    ui.add_space(12.0);
    buttons(ui, "Sanitize", true)
}
