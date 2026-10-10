//! The Print dialog (Acrobat's File ▸ Print, execution plan M10.5): printer, copies, grayscale;
//! pages to print (all, current, range with labels, the selected pages; odd/even, reverse); page sizing & handling
//! (Size, Poster, Multiple, Booklet); orientation; comments & forms; and a live preview of the
//! sheets. Printing sends the print-ready PDF to the system spooler; "Save as PDF" writes it.

use egui::{Color32, Pos2, Rect, Stroke, pos2, vec2};
use pdfcraft_engine::print::{self, Binding, BookletSubset, Content, Layout, Orientation, PAPERS, PageOrder, SizeMode, Subset, spool};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// The preview pane, and the paper inside it (12 pt inset on each side).
const PREVIEW_W: f32 = 320.0;
const PREVIEW_H: f32 = 380.0;
const PREVIEW_INSET: f32 = 12.0;
pub(crate) const PREVIEW_BOX: (f32, f32) = (PREVIEW_W - 2.0 * PREVIEW_INSET, PREVIEW_H - 2.0 * PREVIEW_INSET);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    All,
    Current,
    Range,
    /// The pages picked in the Pages panel or the organize grid when the dialog opened.
    Selected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handling {
    Size,
    Poster,
    Multiple,
    Booklet,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PrintDraft {
    pub printers: Vec<spool::Printer>,
    /// `None` = Save as PDF.
    pub printer: Option<String>,
    pub copies: u32,
    pub collate: bool,
    pub grayscale: bool,
    pub duplex: spool::Duplex,
    pub which: Which,
    pub range: String,
    /// The pages `Which::Selected` prints (0-based, in page order); empty when none were picked.
    pub selected: Vec<usize>,
    pub subset: Subset,
    pub reverse: bool,
    pub handling: Handling,
    pub size: SizeMode,
    pub custom_scale: f64,
    pub per_sheet: usize,
    pub order: PageOrder,
    pub border: bool,
    pub auto_rotate: bool,
    pub booklet_subset: BookletSubset,
    pub binding: Binding,
    pub poster_scale: f64,
    pub overlap: f64,
    pub cut_marks: bool,
    pub orientation: Orientation,
    pub content: Content,
    pub paper: usize,
    /// The sheet shown in the preview (0-based).
    pub sheet: usize,
    pub current_page: usize,
    /// The printer driver's own options and the printer they were read for (Properties…, CUPS).
    pub driver_options: Option<(String, Vec<spool::PrinterOption>)>,
    /// Driver options set away from the printer's defaults, for `driver_options`' printer.
    pub driver_choices: std::collections::BTreeMap<String, String>,
    pub show_driver_options: bool,
    /// Why the printer's preferences window didn't open (Windows).
    pub driver_error: Option<String>,
}

impl Default for PrintDraft {
    fn default() -> Self {
        PrintDraft {
            printers: Vec::new(),
            printer: None,
            copies: 1,
            collate: true,
            grayscale: false,
            duplex: spool::Duplex::Off,
            which: Which::All,
            range: String::new(),
            selected: Vec::new(),
            subset: Subset::All,
            reverse: false,
            handling: Handling::Size,
            size: SizeMode::Fit,
            custom_scale: 100.0,
            per_sheet: 2,
            order: PageOrder::Horizontal,
            border: false,
            auto_rotate: true,
            booklet_subset: BookletSubset::BothSides,
            binding: Binding::Left,
            poster_scale: 200.0,
            overlap: 18.0,
            cut_marks: true,
            orientation: Orientation::Auto,
            content: Content::DocumentAndMarkups,
            paper: 0,
            sheet: 0,
            current_page: 0,
            driver_options: None,
            driver_choices: Default::default(),
            show_driver_options: false,
            driver_error: None,
        }
    }
}

impl PrintDraft {
    /// The engine settings for this draft (page count and labels from the document).
    pub fn settings(&self, count: usize, labels: &[String]) -> Result<print::Settings, String> {
        if self.handling == Handling::Multiple && self.order == PageOrder::CutStack && self.duplex != spool::Duplex::Off {
            return Err("Cut and stack needs Two-sided: Off. Print single-sided sheets.".into());
        }
        let range = match self.which {
            Which::All | Which::Selected => None,
            Which::Current => Some(self.current_page.saturating_add(1).to_string()),
            Which::Range => Some(self.range.clone()),
        };
        let pages = if self.which == Which::Selected {
            // Positions, not a typed range: a range would read numbers as page labels first.
            print::select_listed(count, &self.selected, self.subset, self.reverse)
        } else {
            print::select_pages(count, range.as_deref(), labels, self.subset, self.reverse)
        }
        .map_err(|e| e.to_string())?;
        let layout = match self.handling {
            Handling::Size => Layout::Size(match self.size {
                SizeMode::Custom(_) => SizeMode::Custom(self.custom_scale),
                m => m,
            }),
            Handling::Multiple => match Layout::multiple(self.per_sheet) {
                Layout::Multiple { cols, rows, .. } => {
                    Layout::Multiple { cols, rows, order: self.order, border: self.border, auto_rotate: self.auto_rotate }
                }
                other => other,
            },
            Handling::Booklet => Layout::Booklet { subset: self.booklet_subset, binding: self.binding },
            Handling::Poster => Layout::Poster { scale: self.poster_scale, overlap: self.overlap, cut_marks: self.cut_marks },
        };
        Ok(print::Settings { pages, paper: PAPERS[self.paper.min(PAPERS.len() - 1)].1, orientation: self.orientation, layout, content: self.content })
    }

    pub fn job(&self, title: &str) -> spool::Job {
        spool::Job {
            printer: self.printer.clone(),
            copies: self.copies.max(1),
            collate: self.collate,
            duplex: self.duplex,
            grayscale: self.grayscale,
            title: title.to_string(),
            // Only for the printer the options were read from.
            options: match (&self.printer, &self.driver_options) {
                (Some(p), Some((read_for, _))) if p == read_for => self.driver_choices.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                _ => Vec::new(),
            },
        }
    }
}

/// The system's printers, read off the UI thread. Listing them runs the spooler's own tools
/// (`lpstat`; PowerShell on Windows, which takes a second or two), so the list is read when the
/// app starts and again each time the Print dialog opens, and the dialog shows the last one read.
#[derive(Default)]
pub(crate) struct Printers {
    known: Option<Vec<spool::Printer>>,
    #[cfg(not(target_arch = "wasm32"))]
    reading: Option<std::sync::mpsc::Receiver<Vec<spool::Printer>>>,
    /// Jobs on their way to the spooler, oldest first.
    #[cfg(not(target_arch = "wasm32"))]
    jobs: Vec<PrintJob>,
    /// The progress card is showing a job (it is shared with other long jobs).
    #[cfg(not(target_arch = "wasm32"))]
    showing_progress: bool,
}

/// A job being sent: its printer, the sheets sent so far of how many, and its worker's news.
#[cfg(not(target_arch = "wasm32"))]
struct PrintJob {
    printer: String,
    sheets: Option<(usize, usize)>,
    news: std::sync::mpsc::Receiver<JobNews>,
    over: bool,
}

#[cfg(not(target_arch = "wasm32"))]
enum JobNews {
    Sent(usize, usize),
    Done(Result<String, String>),
}

impl PdfCraftApp {
    /// Start reading the printer list, unless a read is already running.
    fn read_printers(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if self.printers.reading.is_some() {
                return;
            }
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx = self.ctx.clone();
            let work = move || {
                // The receiver may be gone (the app quit): nothing to report to then.
                let _ = tx.send(spool::printers());
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            };
            match std::thread::Builder::new().name("pdfcraft-printers".into()).spawn(work) {
                Ok(_) => self.printers.reading = Some(rx),
                Err(_) => self.printers.known = Some(spool::printers()),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.printers.known = Some(spool::printers());
        }
    }

    /// Pick up a finished printer list and finished print jobs (each frame). The first call
    /// starts reading the list, so it is ready by the time Print is chosen.
    pub(crate) fn poll_printers(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if self.printers.known.is_none() && self.printers.reading.is_none() {
                self.read_printers();
            }
            if let Some(rx) = &self.printers.reading {
                match rx.try_recv() {
                    Ok(list) => self.take_printers(list),
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => self.printers.reading = None,
                }
            }
            let mut finished = Vec::new();
            for job in &mut self.printers.jobs {
                loop {
                    match job.news.try_recv() {
                        Ok(JobNews::Sent(sent, sheets)) => job.sheets = Some((sent, sheets)),
                        Ok(JobNews::Done(result)) => {
                            finished.push((job.printer.clone(), result));
                            job.over = true;
                            break;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            finished.push((job.printer.clone(), Err("the print job stopped unexpectedly".to_string())));
                            job.over = true;
                            break;
                        }
                    }
                }
            }
            self.printers.jobs.retain(|j| !j.over);
            self.show_print_progress();
            for (printer, result) in finished {
                self.notify_sent(&printer, result);
            }
        }
    }

    /// The oldest running job on the progress card, while no other long job holds it.
    #[cfg(not(target_arch = "wasm32"))]
    fn show_print_progress(&mut self) {
        let Some(job) = self.printers.jobs.first() else {
            if std::mem::take(&mut self.printers.showing_progress) {
                self.progress_notice = None;
            }
            return;
        };
        if self.progress_notice.is_some() && !self.printers.showing_progress {
            return;
        }
        let printer = job.printer.as_str();
        let (label, fraction) = match job.sheets {
            Some((sent, sheets)) if sheets > 0 => {
                let sheet = (sent + 1).min(sheets).to_string();
                let label = crate::i18n::fmt(
                    tl!("Sending sheet {s} of {n} to {printer}…"),
                    &[("s", &sheet), ("n", &sheets.to_string()), ("printer", printer)],
                );
                (label, sent as f32 / sheets as f32)
            }
            _ => (crate::i18n::fmt(tl!("Sending to {printer}…"), &[("printer", printer)]), 0.0),
        };
        self.progress_notice = Some(crate::widgets::ProgressNotice { label, fraction, cancellable: false });
        self.printers.showing_progress = true;
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// A list read in the background: remembered for the next Print, and shown at once by an
    /// open dialog. The printer picked in the dialog stays picked.
    #[cfg(not(target_arch = "wasm32"))]
    fn take_printers(&mut self, list: Vec<spool::Printer>) {
        self.printers.reading = None;
        if self.dialog == Some(crate::Dialog::Print) {
            self.print_draft.printers = list.clone();
        }
        self.printers.known = Some(list);
    }

    /// The printer list for a Print dialog opening now: the last one read, or, when none has
    /// arrived yet (Print chosen just after starting), the one being read.
    fn printer_list(&mut self) -> Vec<spool::Printer> {
        #[cfg(not(target_arch = "wasm32"))]
        if self.printers.known.is_none()
            && let Some(rx) = self.printers.reading.take()
        {
            self.printers.known = Some(rx.recv().unwrap_or_default());
            return self.printers.known.clone().unwrap_or_default();
        }
        let list = match self.printers.known.clone() {
            Some(list) => list,
            None => spool::printers(),
        };
        // Printers added or removed since: the open dialog picks the change up when it arrives.
        self.read_printers();
        list
    }

    /// Report a job the spooler took or refused.
    fn notify_sent(&mut self, printer: &str, result: Result<String, String>) {
        match result {
            Ok(msg) if msg.is_empty() => self.notify(crate::i18n::fmt(tl!("Sent to {printer}"), &[("printer", printer)])),
            Ok(msg) => self.notify(crate::i18n::fmt(tl!("Sent to {printer}: {msg}"), &[("printer", printer), ("msg", &msg)])),
            Err(e) => self.notify_error(e),
        }
    }

    pub fn open_print(&mut self) {
        let Some((i, _)) = self.active_ids() else { return };
        let printers = self.printer_list();
        let default = printers.iter().find(|p| p.default).or(printers.first()).map(|p| p.name.clone());
        let current = self.views[i].current;
        let selected: Vec<usize> = self.views[i].selected.iter().copied().collect();
        let keep = std::mem::take(&mut self.print_draft);
        // Picked pages are what Print is for (Acrobat's "Selected pages"); without any, a choice
        // left over from another document falls back to the whole document.
        let which = match (selected.is_empty(), keep.which) {
            (false, _) => Which::Selected,
            (true, Which::Selected) => Which::All,
            (true, other) => other,
        };
        self.print_draft = PrintDraft { printers, printer: default, current_page: current, sheet: 0, selected, which, ..keep };
        self.dialog = Some(crate::Dialog::Print);
    }

    /// Print (or save) with the dialog's settings. Returns `true` on success. Save as PDF without
    /// a preset path returns `true` once the save picker is showing; the file is written on a
    /// later frame, when the user has chosen where.
    pub fn print_now(&mut self) -> bool {
        // What's typed in a form field is part of what's printed (#166).
        if !self.commit_form_typing() {
            return false;
        }
        let Some((_, id)) = self.active_ids() else { return false };
        let Some(doc) = self.session.get(id) else { return false };
        let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
        let settings = match self.print_draft.settings(doc.info.pages.len(), &labels) {
            Ok(s) => s,
            Err(e) => {
                self.notify_error(e);
                return false;
            }
        };
        let name = doc.name.clone();
        let bytes = match self.session.print_pdf(id, &settings) {
            Ok(b) => b,
            Err(e) => {
                self.notify_error(e);
                return false;
            }
        };
        match self.print_draft.printer.clone() {
            Some(printer) => self.send_to_printer(printer, bytes, self.print_draft.job(&name)),
            None => self.save_print_pdf(&name, bytes),
        }
    }

    /// Hand the print-ready PDF to the spooler on a thread of its own: on Windows that renders
    /// every sheet first, which takes seconds the window must not freeze for. Returns `true` once
    /// the job is on its way; the progress card follows it, and how it ended is reported when it
    /// does.
    #[cfg(not(target_arch = "wasm32"))]
    fn send_to_printer(&mut self, printer: String, bytes: Vec<u8>, job: spool::Job) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        let work = move || {
            let repaint = || {
                if let Some(ctx) = &ctx {
                    ctx.request_repaint();
                }
            };
            // The receiver may be gone (the app quit): nothing to report to then.
            let result = spool::submit_with_progress(&bytes, &job, &mut |sent, sheets| {
                let _ = tx.send(JobNews::Sent(sent, sheets));
                repaint();
            });
            let _ = tx.send(JobNews::Done(result.map_err(|e| e.to_string())));
            repaint();
        };
        if let Err(e) = std::thread::Builder::new().name("pdfcraft-print".into()).spawn(work) {
            self.notify_error(e);
            return false;
        }
        self.printers.jobs.push(PrintJob { printer, sheets: None, news: rx, over: false });
        self.show_print_progress();
        true
    }

    #[cfg(target_arch = "wasm32")]
    fn send_to_printer(&mut self, printer: String, bytes: Vec<u8>, job: spool::Job) -> bool {
        let result = spool::submit(&bytes, &job).map_err(|e| e.to_string());
        let sent = result.is_ok();
        self.notify_sent(&printer, result);
        sent
    }

    /// Print ▸ Save as PDF on the desktop: write to `save_override` (tests and automation), or
    /// ask where and write on a later frame. Returns `true` once written, or once the save picker
    /// is showing.
    #[cfg(not(target_arch = "wasm32"))]
    fn save_print_pdf(&mut self, name: &str, bytes: Vec<u8>) -> bool {
        let write = move |app: &mut Self, path: std::path::PathBuf| match crate::editing::write_atomically(&path.to_string_lossy(), &bytes) {
            Ok(()) => {
                app.notify_fmt("Saved the print-ready PDF to {path}", &[("path", &path.display().to_string())]);
                true
            }
            Err(e) => {
                app.notify_fmt("Could not save: {e}", &[("e", &e.to_string())]);
                false
            }
        };
        match self.save_override.clone() {
            Some(p) => write(self, p.into()),
            None => {
                let stem = name.trim_end_matches(".pdf").trim_end_matches(".PDF");
                let dialog = rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(format!("{stem} (print).pdf"));
                self.ask_one(crate::pickers::Ask::Save(dialog), None, move |app, path| {
                    write(app, path);
                })
            }
        }
    }

    /// Print ▸ Save as PDF in a browser: download it, as Save does (#170). There is no folder to
    /// choose and no file system to write to, so `save_override` doesn't apply.
    #[cfg(target_arch = "wasm32")]
    fn save_print_pdf(&mut self, name: &str, bytes: Vec<u8>) -> bool {
        let stem = name.trim_end_matches(".pdf").trim_end_matches(".PDF");
        let file = format!("{stem} (print).pdf");
        match crate::editing::download(&file, &bytes) {
            Ok(()) => {
                self.notify_fmt("Downloaded {name}", &[("name", &file)]);
                true
            }
            Err(e) => {
                self.notify_fmt("Couldn't download {name}: {e}", &[("name", &file), ("e", &e)]);
                false
            }
        }
    }
}

/// The longest side, in pixels, of one page's print-preview raster (also kept within the GPU's
/// `max_texture_side`): an Actual-size print of a large page would otherwise ask for a raster
/// bigger than a texture can be.
pub(crate) const PREVIEW_MAX_SIDE: f32 = 4096.0;
/// The most bytes of print-preview rasters kept at once. The current sheet comes first, then the
/// next and the previous one; a neighbour that doesn't fit isn't rendered ahead.
pub(crate) const PREVIEW_BYTES: f64 = 128.0 * 1024.0 * 1024.0;

/// Pages on the current sheet and its neighbours, each with the device pixels per point that
/// fill the preview pane (so the picture is not a stretched thumbnail), nearest sheet first and
/// within [`PREVIEW_MAX_SIDE`] (and `max_side`, the GPU's texture limit) and [`PREVIEW_BYTES`].
pub(crate) fn preview_rasters(d: &PrintDraft, sizes: &[(f64, f64)], labels: &[String], ppp: f32, max_side: f32) -> Vec<(usize, f32)> {
    let Ok(settings) = d.settings(sizes.len(), labels) else { return Vec::new() };
    let Ok(sheets) = print::layout(sizes, &settings) else { return Vec::new() };
    if sheets.is_empty() {
        return Vec::new();
    }
    let ppp = if ppp.is_finite() { ppp.max(1.0) } else { 1.0 };
    let side = if max_side.is_finite() && max_side >= 1.0 { max_side.min(PREVIEW_MAX_SIDE) } else { PREVIEW_MAX_SIDE };
    let i = d.sheet.min(sheets.len() - 1);
    // Nearest first: the sheet on screen, then the next one, then the previous one.
    let order = [Some(i), i.checked_add(1).filter(|n| *n < sheets.len()), i.checked_sub(1)];
    let mut out: Vec<(usize, f32, f64)> = Vec::new();
    let mut bytes = 0.0f64;
    for sheet in order.into_iter().flatten().filter_map(|s| sheets.get(s)) {
        let (sw, sh) = (sheet.size.0 as f32, sheet.size.1 as f32);
        if !(sw.is_finite() && sh.is_finite()) || sw < 1.0 || sh < 1.0 {
            continue;
        }
        let k = (PREVIEW_BOX.0 / sw).min(PREVIEW_BOX.1 / sh);
        for pl in &sheet.placed {
            let [a, b, _, _, _, _] = pl.matrix.0;
            let placed = (a.hypot(b) as f32) * k * ppp;
            let Some(&(pw, ph)) = sizes.get(pl.page) else { continue };
            let long_pt = pw.max(ph) as f32;
            if !placed.is_finite() || placed < 0.05 || !long_pt.is_finite() || long_pt < 1.0 {
                continue;
            }
            let scale = placed.min(64.0).min(side / long_pt);
            let cost = (pw * f64::from(scale)).ceil() * (ph * f64::from(scale)).ceil() * 4.0;
            if let Some(slot) = out.iter_mut().find(|(page, _, _)| *page == pl.page) {
                if scale > slot.1 && bytes - slot.2 + cost <= PREVIEW_BYTES {
                    bytes += cost - slot.2;
                    (slot.1, slot.2) = (scale, cost);
                }
            } else if out.len() < 48 && (out.is_empty() || bytes + cost <= PREVIEW_BYTES) {
                bytes += cost;
                out.push((pl.page, scale, cost));
            }
        }
    }
    out.into_iter().map(|(page, scale, _)| (page, scale)).collect()
}

/// Properties… on CUPS: the driver's options for this print, grouped as the driver groups them.
/// Only choices away from the printer's defaults are kept, and sent with the job.
fn driver_options_panel(ui: &mut egui::Ui, d: &mut PrintDraft, t: &Tokens) {
    let PrintDraft { driver_options: Some((_, options)), driver_choices: choices, show_driver_options: show, .. } = d else { return };
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tl!("Printer properties")).font(theme::semibold(13.0)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tl!("Done")).clicked() {
                    *show = false;
                }
                if ui.add_enabled(!choices.is_empty(), egui::Button::new(tl!("Reset to printer defaults"))).clicked() {
                    choices.clear();
                }
            });
        });
        if options.is_empty() {
            ui.label(egui::RichText::new(tl!("This printer offers no settings of its own.")).small().color(t.text_muted));
            return;
        }
        egui::ScrollArea::vertical().id_salt("driver-options").max_height(240.0).show(ui, |ui| {
            for (n, chunk) in options.chunk_by(|a, b| a.group == b.group).enumerate() {
                let group = chunk.first().map_or("", |o| o.group.as_str());
                if !group.is_empty() {
                    ui.add_space(4.0);
                    // Option and group names are the driver's own text.
                    ui.label(egui::RichText::new(group).small().color(t.text_muted));
                }
                egui::Grid::new(("driver-group", n)).num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                    for o in chunk {
                        ui.label(&o.label);
                        let current = choices.get(&o.key).cloned().unwrap_or_else(|| o.default.clone());
                        let label_of = |c: &str| o.choices.iter().find(|(k, _)| k == c).map_or_else(|| c.to_string(), |(_, l)| l.clone());
                        let mut pick = current.clone();
                        egui::ComboBox::from_id_salt(("driver-option", &o.key)).selected_text(label_of(&current)).width(200.0).show_ui(ui, |ui| {
                            for (k, l) in &o.choices {
                                ui.selectable_value(&mut pick, k.clone(), l.as_str());
                            }
                        });
                        if pick != current {
                            if pick == o.default {
                                choices.remove(&o.key);
                            } else {
                                choices.insert(o.key.clone(), pick);
                            }
                        }
                        ui.end_row();
                    }
                });
            }
        });
    });
}

fn combo<T: PartialEq + Copy>(ui: &mut egui::Ui, id: &str, value: &mut T, choices: &[(T, &str)], width: f32) {
    let shown = choices.iter().find(|c| c.0 == *value).map_or("", |c| c.1);
    egui::ComboBox::from_id_salt(id).selected_text(shown).width(width).show_ui(ui, |ui| {
        for (v, label) in choices {
            ui.selectable_value(value, *v, *label);
        }
    });
}

/// Draw the dialog. `thumb` gives a page's thumbnail texture when there is one. Returns
/// (print, cancel).
pub(crate) fn body(
    ui: &mut egui::Ui,
    d: &mut PrintDraft,
    t: &Tokens,
    sizes: &[(f64, f64)],
    labels: &[String],
    thumb: &mut dyn FnMut(usize) -> Option<egui::TextureId>,
) -> (bool, bool) {
    ui.set_width(820.0);
    ui.label(egui::RichText::new(tl!("Print")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let settings = d.settings(sizes.len(), labels);
    let sheets = settings.as_ref().ok().and_then(|s| print::layout(sizes, s).ok()).unwrap_or_default();
    d.sheet = d.sheet.min(sheets.len().saturating_sub(1));
    ui.horizontal_top(|ui| {
        // Settings.
        ui.vertical(|ui| {
            ui.set_width(470.0);
            egui::Grid::new("print-top").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                ui.label(tl!("Printer:"));
                let before = d.printer.clone();
                ui.horizontal(|ui| {
                    let shown = d.printer.clone().unwrap_or_else(|| tl!("Save as PDF").to_string());
                    egui::ComboBox::from_id_salt("printer").selected_text(shown).width(260.0).show_ui(ui, |ui| {
                        for p in &d.printers {
                            let label = if p.default { crate::i18n::fmt(tl!("{name} (default)"), &[("name", &p.name)]) } else { p.name.clone() };
                            ui.selectable_value(&mut d.printer, Some(p.name.clone()), label);
                        }
                        ui.selectable_value(&mut d.printer, None, tl!("Save as PDF"));
                    });
                    let tip = if spool::HAS_PRINTER_PREFERENCES {
                        tl!("Opens the printer driver's preferences. They are saved for every print from this computer; the copies, two-sided, grayscale and paper chosen here still apply.")
                    } else {
                        tl!("The printer driver's own settings for this print: paper tray, paper type, finishing and more.")
                    };
                    let properties = ui.add_enabled(d.printer.is_some(), egui::Button::new(tl!("Properties…"))).on_hover_text(tip);
                    if properties.clicked()
                        && let Some(p) = d.printer.clone()
                    {
                        if spool::HAS_PRINTER_PREFERENCES {
                            d.driver_error = spool::open_printer_preferences(&p).err().map(|e| e.to_string());
                        } else {
                            if d.driver_options.as_ref().is_none_or(|(read_for, _)| *read_for != p) {
                                d.driver_options = Some((p.clone(), spool::printer_options(&p)));
                                d.driver_choices.clear();
                            }
                            d.show_driver_options = !d.show_driver_options;
                        }
                    }
                });
                if d.printer != before {
                    d.show_driver_options = false;
                    d.driver_error = None;
                }
                ui.end_row();
                ui.label(tl!("Copies:"));
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut d.copies).range(1..=999));
                    ui.checkbox(&mut d.collate, tl!("Collate"));
                    ui.checkbox(&mut d.grayscale, tl!("Print in grayscale"));
                });
                ui.end_row();
                ui.label(tl!("Two-sided:"));
                combo(
                    ui,
                    "duplex",
                    &mut d.duplex,
                    &[
                        (spool::Duplex::Off, tl!("Off")),
                        (spool::Duplex::LongEdge, tl!("Flip on long edge")),
                        (spool::Duplex::ShortEdge, tl!("Flip on short edge")),
                    ],
                    160.0,
                );
                ui.end_row();
                ui.label(tl!("Paper:"));
                egui::ComboBox::from_id_salt("paper").selected_text(PAPERS[d.paper].0).width(160.0).show_ui(ui, |ui| {
                    for (i, (name, _)) in PAPERS.iter().enumerate() {
                        ui.selectable_value(&mut d.paper, i, *name);
                    }
                });
                ui.end_row();
            });
            if let Some(e) = &d.driver_error {
                ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Couldn't open the printer's preferences: {e}"), &[("e", e)])).small().color(t.text_muted));
            }
            if d.show_driver_options {
                driver_options_panel(ui, d, t);
            }
            ui.add_space(6.0);
            widgets::section_title(ui, tl!("Pages to Print"));
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.which, Which::All, tl!("All"));
                ui.radio_value(&mut d.which, Which::Current, tl!("Current page"));
                ui.radio_value(&mut d.which, Which::Range, tl!("Pages"));
                let r = ui.add_enabled(
                    d.which == Which::Range,
                    egui::TextEdit::singleline(&mut d.range).hint_text(format!("1-{}", sizes.len())).desired_width(110.0),
                );
                if r.gained_focus() {
                    d.which = Which::Range;
                }
            });
            // Offered only when pages were picked before the dialog opened.
            if !d.selected.is_empty() {
                let label = format!("{} ({})", tl!("Selected pages"), d.selected.len());
                ui.radio_value(&mut d.which, Which::Selected, label);
            }
            ui.horizontal(|ui| {
                ui.label(tl!("More options:"));
                combo(
                    ui,
                    "subset",
                    &mut d.subset,
                    &[(Subset::All, tl!("All pages in range")), (Subset::Odd, tl!("Odd pages only")), (Subset::Even, tl!("Even pages only"))],
                    150.0,
                );
                ui.checkbox(&mut d.reverse, tl!("Reverse pages"));
            });
            ui.add_space(6.0);
            widgets::section_title(ui, tl!("Page Sizing & Handling"));
            ui.horizontal(|ui| {
                for (h, label) in [
                    (Handling::Size, tl!("Size")),
                    (Handling::Poster, tl!("Poster")),
                    (Handling::Multiple, tl!("Multiple")),
                    (Handling::Booklet, tl!("Booklet")),
                ] {
                    if widgets::mode_tab(ui, label, d.handling == h).clicked() {
                        d.handling = h;
                        d.sheet = 0;
                    }
                }
            });
            ui.add_space(4.0);
            match d.handling {
                Handling::Size => {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut d.size, SizeMode::Fit, tl!("Fit"));
                        ui.radio_value(&mut d.size, SizeMode::Actual, tl!("Actual size"));
                        ui.radio_value(&mut d.size, SizeMode::Shrink, tl!("Shrink oversized pages"));
                    });
                    ui.horizontal(|ui| {
                        let custom = matches!(d.size, SizeMode::Custom(_));
                        if ui.radio(custom, tl!("Custom scale:")).clicked() {
                            d.size = SizeMode::Custom(d.custom_scale);
                        }
                        ui.add_enabled(custom, egui::DragValue::new(&mut d.custom_scale).range(1.0..=1000.0).suffix(" %"));
                    });
                }
                Handling::Poster => {
                    ui.horizontal(|ui| {
                        ui.label(tl!("Tile scale:"));
                        ui.add(egui::DragValue::new(&mut d.poster_scale).range(10.0..=1000.0).suffix(" %"));
                        ui.label(tl!("Overlap:"));
                        ui.add(egui::DragValue::new(&mut d.overlap).range(0.0..=144.0).suffix(" pt"));
                        ui.checkbox(&mut d.cut_marks, tl!("Cut marks"));
                    });
                }
                Handling::Multiple => {
                    ui.horizontal(|ui| {
                        ui.label(tl!("Pages per sheet:"));
                        combo(ui, "per-sheet", &mut d.per_sheet, &[(2, "2"), (4, "4"), (6, "6"), (9, "9"), (16, "16")], 60.0);
                        ui.label(tl!("Page order:"));
                        combo(
                            ui,
                            "order",
                            &mut d.order,
                            &[
                                (PageOrder::Horizontal, tl!("Horizontal")),
                                (PageOrder::HorizontalReversed, tl!("Horizontal reversed")),
                                (PageOrder::Vertical, tl!("Vertical")),
                                (PageOrder::VerticalReversed, tl!("Vertical reversed")),
                                (PageOrder::CutStack, tl!("Cut and stack")),
                            ],
                            150.0,
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut d.border, tl!("Print page border"));
                        ui.checkbox(&mut d.auto_rotate, tl!("Auto-rotate pages"));
                    });
                    if d.order == PageOrder::CutStack {
                        ui.label(tl!(
                            "Print single-sided. Keep the sheets in order, cut at the marks, then stack the piles left to right, top to bottom."
                        ));
                    }
                }
                Handling::Booklet => {
                    ui.horizontal(|ui| {
                        ui.label(tl!("Booklet subset:"));
                        combo(
                            ui,
                            "booklet",
                            &mut d.booklet_subset,
                            &[
                                (BookletSubset::BothSides, tl!("Both sides")),
                                (BookletSubset::FrontOnly, tl!("Front side only")),
                                (BookletSubset::BackOnly, tl!("Back side only")),
                            ],
                            130.0,
                        );
                        ui.label(tl!("Binding:"));
                        combo(ui, "binding", &mut d.binding, &[(Binding::Left, tl!("Left")), (Binding::Right, tl!("Right"))], 80.0);
                    });
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(tl!("Orientation:"));
                ui.radio_value(&mut d.orientation, Orientation::Auto, tl!("Auto portrait/landscape"));
                ui.radio_value(&mut d.orientation, Orientation::Portrait, tl!("Portrait"));
                ui.radio_value(&mut d.orientation, Orientation::Landscape, tl!("Landscape"));
            });
            ui.add_space(6.0);
            widgets::section_title(ui, tl!("Comments & Forms"));
            combo(
                ui,
                "content",
                &mut d.content,
                &[
                    (Content::Document, tl!("Document")),
                    (Content::DocumentAndMarkups, tl!("Document and markups")),
                    (Content::DocumentAndStamps, tl!("Document and stamps")),
                    (Content::FormFieldsOnly, tl!("Form fields only")),
                ],
                220.0,
            );
        });
        ui.add_space(12.0);
        // Preview.
        ui.vertical(|ui| {
            ui.set_width(PREVIEW_W);
            let (area, _) = ui.allocate_exact_size(vec2(PREVIEW_W, PREVIEW_H), egui::Sense::hover());
            ui.painter().rect_filled(area, 6.0, t.hover);
            match (&settings, sheets.get(d.sheet)) {
                (Err(e), _) => {
                    ui.put(area.shrink(16.0), egui::Label::new(egui::RichText::new(e).color(t.text_muted)).wrap());
                }
                (Ok(_), Some(sheet)) => {
                    let k = (PREVIEW_BOX.0 / sheet.size.0 as f32).min(PREVIEW_BOX.1 / sheet.size.1 as f32);
                    let paper = Rect::from_center_size(area.center(), vec2(sheet.size.0 as f32 * k, sheet.size.1 as f32 * k));
                    ui.painter().rect_filled(paper, 0.0, Color32::WHITE);
                    ui.painter().rect_stroke(paper, 0.0, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
                    let to_screen = |x: f64, y: f64| -> Pos2 { pos2(paper.left() + x as f32 * k, paper.bottom() - y as f32 * k) };
                    let painter = ui.painter().with_clip_rect(paper);
                    for pl in &sheet.placed {
                        let (dw, dh) = sizes[pl.page];
                        let [x0, y0, x1, y1] = pl.clip;
                        let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| {
                            let (sx, sy) = pl.matrix.apply(x, y);
                            to_screen(sx, sy)
                        });
                        // UVs of the visible part (texture y runs down from the page top).
                        let uv = |x: f64, y: f64| pos2((x / dw) as f32, (1.0 - y / dh) as f32);
                        let uvs = [uv(x0, y0), uv(x1, y0), uv(x1, y1), uv(x0, y1)];
                        match thumb(pl.page) {
                            Some(tex) => {
                                let mut mesh = egui::Mesh::with_texture(tex);
                                for (p, u) in corners.iter().zip(uvs) {
                                    mesh.vertices.push(egui::epaint::Vertex {
                                        pos: *p,
                                        uv: u,
                                        color: if d.grayscale { Color32::from_gray(235) } else { Color32::WHITE },
                                    });
                                }
                                mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
                                painter.add(egui::Shape::mesh(mesh));
                            }
                            None => {
                                painter.add(egui::Shape::convex_polygon(
                                    corners.to_vec(),
                                    Color32::from_gray(245),
                                    Stroke::new(0.5, Color32::from_gray(180)),
                                ));
                                let c = corners.iter().fold(vec2(0.0, 0.0), |a, p| a + p.to_vec2()) / 4.0;
                                painter.text(
                                    c.to_pos2(),
                                    egui::Align2::CENTER_CENTER,
                                    (pl.page + 1).to_string(),
                                    theme::regular(11.0),
                                    Color32::from_gray(120),
                                );
                            }
                        }
                    }
                    for b in &sheet.borders {
                        painter.rect_stroke(
                            Rect::from_two_pos(to_screen(b[0], b[1]), to_screen(b[2], b[3])),
                            0.0,
                            Stroke::new(0.6, Color32::BLACK),
                            egui::StrokeKind::Middle,
                        );
                    }
                    for l in &sheet.lines {
                        painter.line_segment([to_screen(l[0], l[1]), to_screen(l[2], l[3])], Stroke::new(0.6, Color32::BLACK));
                    }
                }
                _ => {}
            }
            ui.horizontal(|ui| {
                let n = sheets.len();
                if ui.add_enabled(d.sheet > 0, egui::Button::new("‹")).on_hover_text(tl!("Previous sheet")).clicked() {
                    d.sheet -= 1;
                }
                ui.label(if n == 0 {
                    tl!("No sheets").to_string()
                } else {
                    crate::i18n::fmt(tl!("Sheet {s} of {n}"), &[("s", &(d.sheet + 1).to_string()), ("n", &n.to_string())])
                });
                if ui.add_enabled(d.sheet + 1 < n, egui::Button::new("›")).on_hover_text(tl!("Next sheet")).clicked() {
                    d.sheet += 1;
                }
            });
            if let Some(s) = sheets.first() {
                let (w, h) = (s.size.0 / 72.0, s.size.1 / 72.0);
                ui.label(egui::RichText::new(format!("{w:.2} × {h:.2} in")).small().color(t.text_muted));
            }
        });
    });
    ui.add_space(12.0);
    let (mut go, mut cancel) = (false, false);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let label = if d.printer.is_some() { "Print" } else { "Save as PDF" };
        if ui.add_enabled_ui(settings.is_ok() && !sheets.is_empty(), |ui| widgets::pill_button(ui, label, true)).inner.clicked() {
            go = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    (go, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_preview_rasters_stay_within_the_texture_limit() {
        // Actual size on a 14,400 pt (200 in) page would ask for a raster far past any texture.
        let d = PrintDraft { size: SizeMode::Actual, ..PrintDraft::default() };
        let sizes = vec![(14_400.0, 14_400.0); 2];
        let labels = vec!["1".into(), "2".into()];
        for max_side in [2048.0, 8192.0, 16384.0, f32::NAN, 0.0] {
            for (page, scale) in preview_rasters(&d, &sizes, &labels, 4.0, max_side) {
                let long = 14_400.0 * scale;
                let cap = if max_side.is_finite() && max_side >= 1.0 { max_side.min(PREVIEW_MAX_SIDE) } else { PREVIEW_MAX_SIDE };
                assert!(long <= cap + 1.0, "page {page} at {max_side}: {long} px");
            }
        }
    }

    #[test]
    fn print_preview_rasters_keep_the_current_sheet_within_the_byte_budget() {
        // Many large pages: the current sheet always gets its raster; neighbours only while the
        // budget lasts, and the total stays within it.
        let d = PrintDraft { sheet: 5, ..PrintDraft::default() };
        let sizes = vec![(2_000.0, 2_000.0); 12];
        let labels: Vec<String> = (1..=12).map(|n| n.to_string()).collect();
        let rasters = preview_rasters(&d, &sizes, &labels, 4.0, 16384.0);
        assert_eq!(rasters.first().map(|r| r.0), Some(5), "the sheet on screen comes first: {rasters:?}");
        let total: f64 = rasters.iter().map(|(_, s)| (2_000.0 * f64::from(*s)).ceil().powi(2) * 4.0).sum();
        assert!(total <= PREVIEW_BYTES || rasters.len() == 1, "{total} bytes for {rasters:?}");
    }

    #[test]
    fn print_preview_is_sharper_than_a_thumbnail_on_a_high_dpi_screen() {
        let d = PrintDraft::default();
        let sizes = vec![(612.0, 792.0); 3];
        let labels = vec!["1".into(), "2".into(), "3".into()];
        let rasters = preview_rasters(&d, &sizes, &labels, 2.0, 8192.0);
        let scale = rasters.iter().find(|(page, _)| *page == 0).map(|(_, s)| *s).unwrap();
        // A 132 pt thumbnail at 2× is about 0.43 px/pt. The preview pane needs roughly twice that.
        assert!(scale > 0.7, "letter page in the preview at 2 px/pt: {scale}");
        assert!(rasters.iter().any(|(page, _)| *page == 1), "the next sheet is ready");
        let mut later = d.clone();
        later.sheet = 2;
        let rasters = preview_rasters(&later, &sizes, &labels, 2.0, 8192.0);
        assert!(rasters.iter().any(|(page, _)| *page == 2));
    }
}
