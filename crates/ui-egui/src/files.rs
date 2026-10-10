//! Multi-document page operations: Combine Files, Insert Pages from File, Extract Pages, Split.
//!
//! Files are picked asynchronously and used on a later frame: desktop builds through `pickers`
//! (a blocking picker crashes the app on macOS), browsers through `requests`.

use std::sync::Arc;

use pdfcraft_engine::{DocId, Edit, SplitBy};

use crate::PdfCraftApp;

/// Why files were picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilePurpose {
    Combine,
    InsertPages,
    ReplacePages,
    /// Scan & OCR ▸ Recognize text in multiple files.
    Ocr,
    /// Create a PDF ▸ Multiple files (PDFs, images and text).
    CreateMultiple,
    /// Find text and redact ▸ Import list: a text file with one word or phrase per line.
    RedactWords,
}

/// The picker for `purpose`: PDFs, and for Create and Insert also what they convert.
fn files_picker(purpose: FilePurpose) -> rfd::AsyncFileDialog {
    let dialog = rfd::AsyncFileDialog::new();
    if purpose == FilePurpose::RedactWords {
        return dialog.add_filter(tl!("Text"), &["txt"]);
    }
    if !matches!(purpose, FilePurpose::CreateMultiple | FilePurpose::InsertPages) {
        return dialog.add_filter("PDF", &["pdf"]);
    }
    let all: Vec<&str> = std::iter::once("pdf").chain(pdfcraft_engine::CONVERTIBLE).collect();
    dialog.add_filter(tl!("PDF, images and text"), &all).add_filter("PDF", &["pdf"])
}

/// The Replace Pages dialog: the chosen file and the ranges (1-based, inclusive).
#[derive(Clone, Debug, PartialEq)]
pub struct ReplaceDraft {
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    pub src_pages: usize,
    pub from: usize,
    pub to: usize,
    pub src_from: usize,
    /// The document whose pages are replaced, as it was when the dialog opened: OK replaces
    /// nothing if another document is active by then (a file opened meanwhile), or it changed.
    pub(crate) target: Option<PickTarget>,
}

impl FilePurpose {
    /// Insert and Replace edit the active document, at its selection.
    pub(crate) fn edits_active_document(self) -> bool {
        matches!(self, Self::InsertPages | Self::ReplacePages)
    }
}

/// The document a pick edits, as it was when the picker opened. Page positions captured then
/// are only valid while it is unchanged, and the pick acts on the active document; so a pick
/// that arrives after the document was edited, or stopped being the active one, is refused
/// (see [`PdfCraftApp::still_pick_target`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PickTarget {
    doc: DocId,
    generation: Option<u64>,
}

/// Files picked asynchronously in a browser, waiting for the next frame (see
/// [`PdfCraftApp::file_request`]).
#[derive(Debug)]
pub struct FileRequest {
    purpose: FilePurpose,
    target: Option<PickTarget>,
    files: Vec<(String, Vec<u8>)>,
}

/// Files picked asynchronously (web), waiting to be used.
pub type Requests = Arc<std::sync::Mutex<Vec<FileRequest>>>;

/// Settings for the Split dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct SplitDraft {
    /// Pages per file.
    pub every: usize,
    pub mode: SplitMode,
    /// The largest part, in megabytes (File size mode).
    pub size_mb: f64,
}

impl Default for SplitDraft {
    fn default() -> Self {
        SplitDraft { every: 1, mode: SplitMode::Pages, size_mb: 2.0 }
    }
}

/// Acrobat's Split by: number of pages, file size, top-level bookmarks (and PdfCraft's
/// before-selected-pages).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitMode {
    Pages,
    Selection,
    Size,
    Bookmarks,
}

/// What Split does once confirmed.
#[derive(Clone, Debug, PartialEq)]
pub enum SplitPlan {
    By(SplitBy),
    Size(usize),
    Bookmarks,
}

/// Organize ▸ Extract options.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExtractDraft {
    /// Each page as its own file.
    pub separate: bool,
    /// Delete the pages after extracting them.
    pub delete: bool,
    /// The name the extracted pages are saved under, without `.pdf` (#737). The dialog fills in
    /// the document's name when it opens; empty means that name too. As separate files each
    /// page is `<name> (page N).pdf`, and a single page the user renamed is `<name>.pdf`.
    pub name: String,
}

/// `name` made safe as a file name on every platform: no path separators, reserved or control
/// characters, no trailing dots or spaces (Windows drops them), no `.pdf` (added back on
/// writing), at most 200 characters. Empty when nothing usable is left.
pub(crate) fn file_stem_from_user(name: &str) -> String {
    let name = name.trim();
    // Any case of ".pdf": it's added back on writing.
    let name =
        name.len().checked_sub(4).and_then(|i| name.get(i..).filter(|ext| ext.eq_ignore_ascii_case(".pdf")).and(name.get(..i))).unwrap_or(name);
    let cleaned: String =
        name.chars().map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c }).collect();
    // At most 200 bytes, so " (page N).pdf" still fits the usual 255-byte file name limit.
    let mut end = cleaned.len().min(200);
    while !cleaned.is_char_boundary(end) {
        end -= 1;
    }
    let cleaned = cleaned.get(..end).unwrap_or_default();
    // Trimming trailing dots also leaves "." and ".." empty.
    let stem = cleaned.trim().trim_end_matches(['.', ' ']).trim_start();
    // Windows reserves these device names whatever the extension: "NUL.pdf" writes nowhere.
    let device = stem.split('.').next().unwrap_or_default().to_ascii_uppercase();
    let reserved = matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (device.len() == 4
            && (device.starts_with("COM") || device.starts_with("LPT"))
            && device.as_bytes().get(3).is_some_and(|d| (b'1'..=b'9').contains(d)));
    if reserved { format!("_{stem}") } else { stem.to_string() }
}

/// Pages ▸ Rotate Pages.
#[derive(Clone, Debug, PartialEq)]
pub struct RotateDraft {
    /// Degrees clockwise: 90, 180 or 270.
    pub degrees: i64,
    /// 0 all pages, 1 the selection, 2 the range below.
    pub which: u8,
    pub from: usize,
    pub to: usize,
    pub parity: pdfcraft_engine::PageParity,
    pub orientation: pdfcraft_engine::PageOrientation,
}

impl Default for RotateDraft {
    fn default() -> Self {
        RotateDraft { degrees: 90, which: 0, from: 1, to: 1, parity: Default::default(), orientation: Default::default() }
    }
}

impl PdfCraftApp {
    /// Ask for files to add to the Combine files list (its Add files… button).
    pub fn combine_dialog(&mut self) {
        self.pick_files(FilePurpose::Combine, true);
    }

    /// Create a PDF ▸ Multiple files: ask for the files to convert.
    pub fn create_multiple_dialog(&mut self) {
        self.pick_files(FilePurpose::CreateMultiple, true);
    }

    /// Scan & OCR ▸ Recognize text ▸ In multiple files: ask for the PDFs.
    pub fn ocr_files_dialog(&mut self) {
        self.pick_files(FilePurpose::Ocr, true);
    }

    /// Ask for files whose pages to insert after the selection (Organize ▸ Insert from file).
    pub fn insert_from_file_dialog(&mut self) {
        self.insert_from_file_at(None);
    }

    /// Ask for files to insert at grid gap `at` (0 = before the first page), or after the
    /// selection.
    pub(crate) fn insert_from_file_at(&mut self, at: Option<usize>) {
        let Some(i) = self.active else {
            self.notify_tr("Open a document first");
            return;
        };
        if let Some(v) = self.views.get_mut(i) {
            v.insert_at = at;
        }
        self.pick_files(FilePurpose::InsertPages, true);
    }

    pub(crate) fn pick_files(&mut self, purpose: FilePurpose, multiple: bool) {
        #[cfg(not(target_arch = "wasm32"))]
        self.pick(crate::pickers::PickFor::Files(purpose), files_picker(purpose), multiple);
        #[cfg(target_arch = "wasm32")]
        {
            let requests = self.requests.clone();
            let ctx = self.ctx.clone();
            // The document to insert into is the one active now, not whichever is active when
            // the browser has finished reading the file (#167).
            let request = self.file_request(purpose, Vec::new());
            wasm_bindgen_futures::spawn_local(async move {
                let dialog = files_picker(purpose);
                let handles = if multiple { dialog.pick_files().await.unwrap_or_default() } else { dialog.pick_file().await.into_iter().collect() };
                let mut files = Vec::new();
                for h in handles {
                    files.push((h.file_name(), h.read().await));
                }
                if !files.is_empty()
                    && let Ok(mut q) = requests.lock()
                {
                    q.push(FileRequest { files, ..request });
                }
                // The read may finish while the app is idle: wake it to use the files.
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            });
        }
    }

    /// Read the picked files and use them.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn use_paths(&mut self, purpose: FilePurpose, paths: &[std::path::PathBuf]) {
        let mut files = Vec::new();
        let mut modified = Vec::new();
        for p in paths {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file.pdf".into());
            if purpose == FilePurpose::RedactWords && std::fs::metadata(p).is_ok_and(|m| m.len() > crate::redact_ui::MAX_WORD_LIST_BYTES as u64) {
                self.notify_fmt("{name} is too large for a word list (at most 1 MB).", &[("name", &name)]);
                return;
            }
            match std::fs::read(p) {
                Ok(b) => files.push((name, b)),
                Err(e) => {
                    self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                    return;
                }
            }
            modified.push(std::fs::metadata(p).and_then(|m| m.modified()).ok());
        }
        if files.is_empty() {
            return;
        }
        // Combine files lists when each file was last modified.
        if purpose == FilePurpose::Combine {
            let incoming = files
                .into_iter()
                .zip(modified)
                .map(|((name, bytes), modified)| crate::combine_ui::Incoming { name, bytes: Arc::new(bytes), modified, note: None })
                .collect();
            self.stage_combine_with(incoming);
        } else {
            self.use_files(purpose, files);
        }
    }

    /// A file dropped on the Combine files tab: added to its list.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn drop_into_combine(&mut self, f: egui::DroppedFileHandle, _ctx: &egui::Context) {
        // A folder: the PDFs in it and in the folders inside it.
        if f.path().is_absolute() && f.path().is_dir() {
            self.add_folder_to_combine(f.path(), true);
            return;
        }
        if f.path().is_absolute() {
            self.use_paths(FilePurpose::Combine, &[f.path().to_path_buf()]);
            return;
        }
        let name = f.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "dropped.pdf".into());
        match f.bytes() {
            Ok(bytes) => self.use_files(FilePurpose::Combine, vec![(name, bytes)]),
            Err(e) => self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e.to_string())]),
        }
    }

    /// Browsers read dropped files asynchronously; they join the Combine files list next frame.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn drop_into_combine(&mut self, f: egui::DroppedFileHandle, ctx: &egui::Context) {
        let requests = self.requests.clone();
        let request = self.file_request(FilePurpose::Combine, Vec::new());
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let name = f.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "dropped.pdf".into());
            if let Ok(bytes) = f.bytes_async().await
                && let Ok(mut q) = requests.lock()
            {
                q.push(FileRequest { files: vec![(name, bytes)], ..request });
                ctx.request_repaint();
            }
        });
    }

    /// A request to use `files` for `purpose`, bound to the document it will edit: the active
    /// one now, for Insert and Replace. Browsers make it when the picker opens and queue it on
    /// `requests` once the files are read; it is used on a later frame.
    pub fn file_request(&self, purpose: FilePurpose, files: Vec<(String, Vec<u8>)>) -> FileRequest {
        let target = if purpose.edits_active_document() { self.pick_target(self.active_ids().map(|(_, id)| id)) } else { None };
        FileRequest { purpose, target, files }
    }

    /// `doc` as it is now, for a pick that will edit it.
    pub(crate) fn pick_target(&self, doc: Option<DocId>) -> Option<PickTarget> {
        doc.map(|doc| PickTarget { doc, generation: self.session.get(doc).map(|d| d.edit_generation()) })
    }

    /// Whether a pick for `target` may still be used: the document is still the active one and
    /// unedited. Tells the user when not.
    pub(crate) fn still_pick_target(&mut self, target: Option<PickTarget>) -> bool {
        let Some(t) = target else { return true };
        let active = self.active_ids().map(|(_, id)| id);
        let generation = self.session.get(t.doc).map(|d| d.edit_generation());
        if active == Some(t.doc) && generation == t.generation {
            return true;
        }
        self.notify_tr("The document changed while you were choosing a file, so nothing was changed.");
        false
    }

    /// Handle files picked asynchronously.
    pub(crate) fn process_file_requests(&mut self) {
        let pending: Vec<_> = self.requests.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        for FileRequest { purpose, target, files } in pending {
            if !self.still_pick_target(target) {
                continue;
            }
            // Last-resort guard (AGENTS.md §4), as for desktop picks and commands.
            if let Err(m) = pdfcraft_engine::guard(|| self.use_files(purpose, files)) {
                self.notify_fmt("That didn't work: an internal error stopped it ({m}).", &[("m", m.as_str())]);
            }
        }
    }

    /// Use picked files (also the entry point for tests and automation).
    pub fn use_files(&mut self, purpose: FilePurpose, files: Vec<(String, Vec<u8>)>) {
        match purpose {
            FilePurpose::Combine => self.stage_combine(files),
            FilePurpose::InsertPages => self.insert_files(files),
            FilePurpose::ReplacePages => {
                if let Some((name, bytes)) = files.into_iter().next() {
                    self.start_replace(name, bytes);
                }
            }
            FilePurpose::Ocr => self.ocr_files(files),
            FilePurpose::CreateMultiple => self.stage_create_multiple(files),
            FilePurpose::RedactWords => {
                let Some((name, bytes)) = files.into_iter().next() else { return };
                if bytes.len() > crate::redact_ui::MAX_WORD_LIST_BYTES {
                    self.notify_fmt("{name} is too large for a word list (at most 1 MB).", &[("name", &name)]);
                    return;
                }
                self.redact_search.words = pdfcraft_engine::redact_word_list(&pdfcraft_engine::decode_text(&bytes)).join("\n");
                self.redact_search.mode = crate::redact_ui::SearchMode::Words;
            }
        }
    }

    pub fn replace_pages_dialog(&mut self) {
        if self.active.is_none() {
            self.notify_tr("Open a document first");
            return;
        }
        self.pick_files(FilePurpose::ReplacePages, false);
    }

    /// Open the Replace Pages dialog for `bytes`, replacing the selection (or the current page).
    pub fn start_replace(&mut self, name: String, bytes: Vec<u8>) {
        let Some(i) = self.active else { return };
        let bytes = Arc::new(bytes);
        let src_pages = match self.session.page_count_of(&name, &bytes) {
            Ok(n) => n,
            Err(e) => {
                self.notify_fmt("Couldn't use {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                return;
            }
        };
        let targets = self.views[i].target_pages();
        let (from, to) = (targets.first().map_or(1, |p| p + 1), targets.last().map_or(1, |p| p + 1));
        let target = self.pick_target(self.active_ids().map(|(_, id)| id));
        self.replace_draft = Some(ReplaceDraft { name, bytes, src_pages, from, to, src_from: 1, target });
        self.dialog = Some(crate::Dialog::ReplacePages);
    }

    /// Insert all pages of a file after the organize selection (or the current page).
    pub fn insert_pages_from(&mut self, name: &str, bytes: Vec<u8>) {
        self.insert_files(vec![(name.to_string(), bytes)]);
    }

    /// Insert the pages of `files` (PDFs, images, text), in order, at the gap a "+" in the page
    /// grid chose, or else after the selection (or the current page); then select them.
    pub fn insert_files(&mut self, files: Vec<(String, Vec<u8>)>) {
        let Some(view) = self.active.and_then(|i| self.views.get_mut(i)) else { return };
        let (id, chosen) = (view.id, view.insert_at.take());
        let after = view.target_pages().last().map_or(0, |p| p + 1);
        let count = self.session.get(id).map_or(0, |d| d.info.pages.len());
        let start = chosen.unwrap_or(after).min(count);
        let mut at = start;
        for (name, bytes) in files {
            let converted =
                self.session.convert_to_pdf(&name, &Arc::new(bytes)).and_then(|(_, pdf)| Ok((self.session.page_count_of(&name, &pdf)?, pdf)));
            match converted {
                Ok((pages, bytes)) => {
                    if self.apply_edit(Edit::InsertPagesFrom { name, bytes, pages: None, at }) {
                        at = at.saturating_add(pages);
                    }
                }
                Err(e) => self.notify_error(e),
            }
        }
        if at > start
            && let Some(view) = self.active.and_then(|i| self.views.get_mut(i))
        {
            view.select_pages(&(start..at).collect::<Vec<_>>());
        }
    }

    /// Show Extract pages, its file name filled in with the active document's (#737).
    pub fn open_extract_dialog(&mut self) {
        self.extract_draft.name =
            self.active_ids().and_then(|(_, id)| self.session.get(id)).map(|d| strip_pdf(&d.name).to_string()).unwrap_or_default();
        self.dialog = Some(crate::Dialog::Extract);
    }

    /// Copy the selected pages (or the current page) into a new unsaved document tab.
    pub fn extract_selection(&mut self) {
        // What's typed in a form field is part of the document (#166).
        if !self.commit_form_typing() {
            return;
        }
        let Some((i, id)) = self.active_ids() else { return };
        let pages = self.views[i].target_pages();
        let stem = self.session.get(id).map(|d| strip_pdf(&d.name).to_string()).unwrap_or_default();
        let opts = self.extract_draft.clone();
        // The name typed in the dialog (#737), else the document's.
        let chosen = file_stem_from_user(&opts.name);
        let renamed = !chosen.is_empty() && chosen != file_stem_from_user(&stem);
        let base = if chosen.is_empty() { stem.clone() } else { chosen };
        if opts.separate {
            // Each page as its own file, in a chosen folder.
            let mut named = Vec::new();
            for &p in &pages {
                match self.session.extract(id, &[p]) {
                    // One page the user named is saved under exactly that name.
                    Ok(bytes) if renamed && pages.len() == 1 => named.push((format!("{base}.pdf"), bytes)),
                    Ok(bytes) => named.push((format!("{base} (page {}).pdf", p + 1), bytes)),
                    Err(e) => {
                        self.notify_fmt("Couldn't extract pages: {e}", &[("e", &e.to_string())]);
                        return;
                    }
                }
            }
            // Delete the pages only once their files are written, which is on a later frame when
            // the user chooses the folder, and only if they are still what was written: the
            // document must be unedited since (page numbers shift, and edits to those pages
            // aren't in the files) and still the active one (deleting edits the active document).
            let generation = self.session.get(id).map(|d| d.edit_generation());
            self.write_files_then(named, tl!("Choose a folder for the extracted pages"), move |app| {
                if !opts.delete {
                    return;
                }
                let unchanged = app.session.get(id).map(|d| d.edit_generation()) == generation;
                if unchanged && app.active_ids().map(|(_, active)| active) == Some(id) {
                    app.apply_edit(pdfcraft_engine::Edit::DeletePages { pages });
                } else {
                    app.notify_tr("The pages were extracted but not deleted, because the document changed while you were choosing a folder.");
                }
            });
            return;
        } else {
            match self.session.extract(id, &pages) {
                Ok(bytes) => {
                    let message = if pages.len() == 1 {
                        tl!("Extracted 1 page").to_string()
                    } else {
                        crate::i18n::fmt(tl!("Extracted {n} pages"), &[("n", &pages.len().to_string())])
                    };
                    // Save As offers the tab's name, so a name typed in the dialog is kept.
                    let name = if renamed { format!("{base}.pdf") } else { format!("{stem} (extract).pdf") };
                    self.open_created(&name, bytes, &message)
                }
                Err(e) => {
                    self.notify_fmt("Couldn't extract pages: {e}", &[("e", &e.to_string())]);
                    return;
                }
            }
        }
        if opts.delete {
            // Back on the original document.
            self.active = Some(i);
            self.apply_edit(pdfcraft_engine::Edit::DeletePages { pages });
        }
    }

    /// Write named files into a chosen folder (desktop) or as downloads (web).
    pub(crate) fn write_files(&mut self, named: &[(String, Arc<Vec<u8>>)], title: &str) {
        self.write_files_then(named.to_vec(), title, |_| {});
    }

    /// [`Self::write_files`], then `after` once every file is written: now, or on a later frame
    /// when the user chooses the folder. `after` never runs when writing fails or is cancelled.
    pub(crate) fn write_files_then(&mut self, named: Vec<(String, Arc<Vec<u8>>)>, title: &str, after: impl FnOnce(&mut Self) + Send + 'static) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let write = move |app: &mut Self, dir: std::path::PathBuf| {
                for (name, bytes) in &named {
                    if let Err(e) = crate::editing::write_atomically(&dir.join(name).to_string_lossy(), bytes) {
                        app.notify_fmt("Couldn't write {name}: {e}", &[("name", name), ("e", &e.to_string())]);
                        return;
                    }
                }
                if named.len() == 1 {
                    app.notify_fmt("Wrote 1 file to {dir}", &[("dir", &dir.display().to_string())]);
                } else {
                    app.notify_fmt("Wrote {n} files to {dir}", &[("n", &named.len().to_string()), ("dir", &dir.display().to_string())]);
                }
                after(app);
            };
            match &self.export_dir_override {
                Some(d) => write(self, std::path::PathBuf::from(d)),
                None => {
                    self.ask_one(crate::pickers::Ask::Folder(rfd::AsyncFileDialog::new().set_title(title)), None, write);
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = title;
            for (name, bytes) in &named {
                if let Err(e) = crate::editing::download(name, bytes) {
                    self.notify_fmt("Couldn't download {name}: {e}", &[("name", name), ("e", &e.to_string())]);
                    return;
                }
            }
            after(self);
        }
    }

    /// Pages ▸ Rotate Pages with the dialog's range and filters.
    pub fn rotate_with_draft(&mut self) {
        let Some((i, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let n = doc.info.pages.len();
        let d = self.rotate_draft.clone();
        let base: Vec<usize> = match d.which {
            1 => self.views[i].target_pages(),
            2 => (d.from.max(1) - 1..d.to.min(n)).collect(),
            _ => (0..n).collect(),
        };
        let pages = pdfcraft_engine::filter_pages(&doc.info, &base, d.parity, d.orientation);
        if pages.is_empty() {
            self.notify_tr("No pages match those choices");
            return;
        }
        self.apply_edit(pdfcraft_engine::Edit::RotatePages { pages, degrees: d.degrees });
    }

    /// Split the active document and write the parts: into a chosen folder (desktop, written on
    /// a later frame once the user has chosen it) or as downloads (web).
    pub fn split_active(&mut self, plan: &SplitPlan) {
        // What's typed in a form field is part of the document (#166).
        if !self.commit_form_typing() {
            return;
        }
        let Some((_, id)) = self.active_ids() else { return };
        let stem = self.session.get(id).map(|d| strip_pdf(&d.name).to_string()).unwrap_or_else(|| "document".into());
        let (parts, titles) = match plan {
            SplitPlan::By(by) => (self.session.split(id, by), Vec::new()),
            SplitPlan::Size(max) => (self.session.split_by_size(id, *max), Vec::new()),
            SplitPlan::Bookmarks => {
                let marks = self.session.bookmark_splits(id);
                let cuts: Vec<usize> = marks.iter().map(|m| m.0).collect();
                (self.session.split(id, &SplitBy::Before(cuts)), marks)
            }
        };
        let parts = match parts {
            Ok(p) => p,
            Err(e) => {
                self.notify_fmt("Couldn't split: {e}", &[("e", &e.to_string())]);
                return;
            }
        };
        let safe = |t: &str| t.chars().map(|c| if c.is_alphanumeric() || " -_.,()".contains(c) { c } else { '_' }).collect::<String>();
        let mut used = std::collections::HashSet::new();
        let named: Vec<(String, Arc<Vec<u8>>)> = parts
            .into_iter()
            .map(|(a, b, bytes)| {
                // Bookmark splits are named after the bookmark that starts the part.
                let title = titles.iter().find(|(p, _)| *p + 1 == a).map(|(_, t)| safe(t));
                let name = match title {
                    Some(t) => format!("{stem} - {t}.pdf"),
                    None if a == b => format!("{stem} (page {a}).pdf"),
                    None => format!("{stem} (pages {a}-{b}).pdf"),
                };
                // Equal titles (or titles that sanitize alike) get (2), (3), ... so no part overwrites another; compared case-insensitively.
                let mut unique = name.clone();
                let mut n = 1;
                while !used.insert(unique.to_lowercase()) {
                    n += 1;
                    unique = format!("{} ({n}).pdf", name.trim_end_matches(".pdf"));
                }
                (unique, bytes)
            })
            .collect();
        self.write_files(&named, "Choose a folder for the split files");
    }

    /// Summarize Comments: make the summary and open it as a new document.
    pub fn summarize_comments(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let stem =
            self.session.get(id).map(|d| std::path::Path::new(&d.name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
        match self.session.summarize_comments(id, self.summary_sort) {
            Ok(bytes) => self.open_created(&format!("Summary of comments on {}.pdf", stem.unwrap_or_default()), bytes, "Created a comment summary"),
            Err(e) => self.notify_fmt("Couldn't summarize comments: {e}", &[("e", &e.to_string())]),
        }
    }

    pub(crate) fn open_created(&mut self, name: &str, bytes: Arc<Vec<u8>>, message: &str) {
        match self.session.open_new(name, bytes) {
            Ok(id) => {
                let Some(doc) = self.session.get(id) else { return };
                self.views.push(crate::DocView::new(id, &doc.info, self.view_defaults));
                self.active = Some(self.views.len() - 1);
                self.notify_tr(message);
            }
            Err(e) => self.notify_fmt("Couldn't open the result: {e}", &[("e", &e.to_string())]),
        }
    }
}

pub(crate) fn strip_pdf(name: &str) -> &str {
    name.strip_suffix(".pdf").or_else(|| name.strip_suffix(".PDF")).unwrap_or(name)
}

impl PdfCraftApp {
    /// Comments ▸ Import comments / Prepare a form ▸ Import data: XFDF, FDF, XML, CSV or text.
    pub fn import_data_dialog(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let import = |app: &mut Self, path: std::path::PathBuf| match std::fs::read(&path) {
                Ok(bytes) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    app.apply_edit(pdfcraft_engine::Edit::ImportData { name, bytes: std::sync::Arc::new(bytes) });
                }
                Err(e) => app.notify_fmt("Couldn't read {name}: {e}", &[("name", &path.display().to_string()), ("e", &e.to_string())]),
            };
            match self.save_override.clone() {
                Some(p) if [".xfdf", ".fdf", ".xml", ".csv", ".txt"].iter().any(|e| p.ends_with(e)) => import(self, p.into()),
                Some(_) => {}
                None => {
                    let dialog = rfd::AsyncFileDialog::new()
                        .add_filter(tl!("Comment and form data").to_string(), &["xfdf", "fdf", "xml", "csv", "txt"])
                        .set_title(tl!("Import data").to_string());
                    let target = self.active_ids().map(|(_, id)| id);
                    self.ask_one(crate::pickers::Ask::File(dialog), target, import);
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        self.notify_tr("Importing data arrives on the web with file pickers for data files");
    }

    /// Export all comments / form data: the format follows the file name's extension.
    /// Export a PDF ▸ Word, HTML or RTF: ask where (`save_override` in tests), then write.
    pub fn export_office_dialog(&mut self, format: pdfcraft_engine::compare::OfficeFormat) {
        // What's typed in a form field is part of the document (#166).
        if !self.commit_form_typing() {
            return;
        }
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let stem = doc.name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string();
        let ext = format.extension();
        let bytes = doc.export_office(format);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let write = move |app: &mut Self, path: std::path::PathBuf| match crate::editing::write_atomically(&path.to_string_lossy(), &bytes) {
                Ok(()) => app.notify_fmt("Exported to {path}", &[("path", &path.display().to_string())]),
                Err(e) => app.notify_fmt("Couldn't write {name}: {e}", &[("name", &path.display().to_string()), ("e", &e.to_string())]),
            };
            match self.save_override.clone() {
                Some(p) => write(self, p.into()),
                None => {
                    let dialog = rfd::AsyncFileDialog::new()
                        .set_title(tl!("Export").to_string())
                        .add_filter(ext.to_uppercase(), &[ext])
                        .set_file_name(format!("{stem}.{ext}"));
                    self.ask_one(crate::pickers::Ask::Save(dialog), None, write);
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        if let Err(e) = crate::editing::download(&format!("{stem}.{ext}"), &bytes) {
            self.notify_error(e);
        }
    }

    /// Forms ▸ Merge data files into spreadsheet: choose data files (FDF, XFDF or filled-in PDF
    /// forms), then where to save the CSV.
    pub fn merge_data_dialog(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let dialog = rfd::AsyncFileDialog::new()
                .set_title(tl!("Select data files to merge").to_string())
                .add_filter(tl!("Form data and PDF forms").to_string(), &["fdf", "xfdf", "pdf"]);
            self.ask(crate::pickers::Ask::Files(dialog), None, |app, paths| {
                let mut files = Vec::new();
                for p in paths {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    match std::fs::read(&p) {
                        Ok(b) => files.push((name, b)),
                        Err(e) => return app.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e.to_string())]),
                    }
                }
                if !files.is_empty() {
                    // Asks where to save: a second picker, now that the first one has closed.
                    app.merge_data_files(files);
                }
            });
        }
        #[cfg(target_arch = "wasm32")]
        self.notify_tr("Merging data files needs the desktop app");
    }

    /// Merge the given data files and save the spreadsheet (asks where; `save_override` in tests).
    pub fn merge_data_files(&mut self, files: Vec<(String, Vec<u8>)>) {
        let csv = match pdfcraft_engine::merge_data_files(&files) {
            Ok(c) => c,
            Err(e) => return self.notify_error(e),
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let count = files.len();
            let write =
                move |app: &mut Self, path: std::path::PathBuf| match crate::editing::write_atomically(&path.to_string_lossy(), csv.as_bytes()) {
                    Ok(()) => {
                        if count == 1 {
                            app.notify_fmt("Merged 1 file into {path}", &[("path", &path.display().to_string())]);
                        } else {
                            app.notify_fmt("Merged {n} files into {path}", &[("n", &count.to_string()), ("path", &path.display().to_string())]);
                        }
                    }
                    Err(e) => app.notify_fmt("Couldn't write {name}: {e}", &[("name", &path.display().to_string()), ("e", &e.to_string())]),
                };
            match self.save_override.clone() {
                Some(p) => write(self, p.into()),
                None => {
                    let dialog = rfd::AsyncFileDialog::new()
                        .set_title(tl!("Save the spreadsheet").to_string())
                        .add_filter("CSV", &["csv"])
                        .set_file_name("report.csv");
                    self.ask_one(crate::pickers::Ask::Save(dialog), None, write);
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = crate::editing::download("report.csv", csv.as_bytes());
    }

    pub fn export_data_dialog(&mut self, comments: bool, fields: bool) {
        // What's typed in a form field is part of the document (#166).
        if !self.commit_form_typing() {
            return;
        }
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let stem = doc.name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let write = move |app: &mut Self, path: std::path::PathBuf| {
                // Include what was typed while the save panel was open (#166).
                if !app.commit_typing_in(id) {
                    return;
                }
                let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
                let format = pdfcraft_engine::DataFormat::from_extension(&ext).unwrap_or(pdfcraft_engine::DataFormat::Xfdf);
                match app.session.export_data(id, format, comments, fields) {
                    Ok(bytes) => match crate::editing::write_atomically(&path.to_string_lossy(), &bytes) {
                        Ok(()) => app.notify_fmt("Exported to {path}", &[("path", &path.display().to_string())]),
                        Err(e) => app.notify_fmt("Couldn't write {name}: {e}", &[("name", &path.display().to_string()), ("e", &e.to_string())]),
                    },
                    Err(e) => app.notify_error(e),
                }
            };
            match self.save_override.clone() {
                Some(p) => write(self, p.into()),
                None => {
                    let title = if comments { tl!("Export comments") } else { tl!("Export form data") };
                    let d = rfd::AsyncFileDialog::new().set_title(title).set_file_name(format!("{stem}.xfdf"));
                    let d = if comments {
                        d.add_filter("XFDF", &["xfdf"]).add_filter("FDF", &["fdf"])
                    } else {
                        d.add_filter("XFDF", &["xfdf"])
                            .add_filter("FDF", &["fdf"])
                            .add_filter("XML", &["xml"])
                            .add_filter("CSV", &["csv"])
                            .add_filter(tl!("Text").to_string(), &["txt"])
                    };
                    self.ask_one(crate::pickers::Ask::Save(d), None, write);
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        match self.session.export_data(id, pdfcraft_engine::DataFormat::Xfdf, comments, fields) {
            Ok(bytes) => {
                let _ = crate::editing::download(&format!("{stem}.xfdf"), &bytes);
            }
            Err(e) => self.notify_error(e),
        }
    }
}

#[cfg(test)]
mod stem_tests {
    use super::file_stem_from_user;

    #[test]
    fn user_file_names_stay_inside_the_folder_and_writable_everywhere() {
        assert_eq!(file_stem_from_user("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(file_stem_from_user(".."), "");
        assert_eq!(file_stem_from_user("Report.PDF"), "Report");
        // Windows device names, in any case and with any extension, get a prefix.
        assert_eq!(file_stem_from_user("nul"), "_nul");
        assert_eq!(file_stem_from_user("COM1.pdf"), "_COM1");
        assert_eq!(file_stem_from_user("lpt9.tar"), "_lpt9.tar");
        assert_eq!(file_stem_from_user("COM10"), "COM10");
        assert_eq!(file_stem_from_user("Console"), "Console");
        // At most 200 bytes, cut on a character boundary.
        let long = file_stem_from_user(&"文".repeat(300));
        assert!(long.len() <= 200 && long.chars().all(|c| c == '文'), "{}", long.len());
    }
}
