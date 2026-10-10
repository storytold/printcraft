//! Create a PDF (blank, from images, from text) and Reduce File Size (execution plan M10.2,
//! M11.1). Opening an image or a text file converts it to a new, unsaved PDF, as Acrobat does.

use std::sync::Arc;

use crate::PdfCraftApp;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResolutionChoice {
    #[default]
    Image,
    Default72,
    Custom,
}

pub struct ImageImport {
    pub images: Vec<(String, Vec<u8>)>,
    pub choice: ResolutionChoice,
    pub dpi: f64,
}

pub(crate) fn image_import_body(ui: &mut egui::Ui, app: &mut PdfCraftApp) -> (bool, bool) {
    ui.heading(tl!("Create PDF from images"));
    let Some(draft) = app.image_import.as_mut() else { return (false, true) };
    ui.label(crate::i18n::fmt(tl!("Selected image files: {count}"), &[("count", &draft.images.len().to_string())]));
    ui.add_space(8.0);
    ui.radio_value(&mut draft.choice, ResolutionChoice::Image, tl!("Use image resolution"));
    ui.label(tl!("Use each image's embedded DPI; use 72 DPI when it is absent."));
    ui.radio_value(&mut draft.choice, ResolutionChoice::Default72, tl!("Use 72 DPI (one point per pixel)"));
    ui.radio_value(&mut draft.choice, ResolutionChoice::Custom, tl!("Use custom DPI"));
    ui.add_enabled(draft.choice == ResolutionChoice::Custom, egui::DragValue::new(&mut draft.dpi).range(1.0..=1200.0).suffix(" DPI"));
    ui.label(tl!("DPI sets the printed page size without resampling the image."));
    ui.add_space(12.0);
    let mut go = false;
    let mut cancel = false;
    ui.horizontal(|ui| {
        go = crate::widgets::pill_button(ui, tl!("Create"), true).clicked();
        cancel = crate::widgets::pill_button(ui, tl!("Cancel"), false).clicked();
    });
    (go, cancel)
}

/// File types Open accepts besides PDF (converted on open).
pub use pdfcraft_engine::CONVERTIBLE;

/// What Create ▸ Clipboard found on the clipboard.
#[derive(Clone, Debug, PartialEq)]
pub enum Clip {
    /// RGBA pixels, row by row.
    Image {
        width: usize,
        height: usize,
        rgba: Vec<u8>,
    },
    Text(String),
}

/// An image on the system clipboard, or else its text.
#[cfg(not(target_arch = "wasm32"))]
fn read_clipboard() -> Option<Clip> {
    let mut cb = arboard::Clipboard::new().ok()?;
    if let Ok(img) = cb.get_image() {
        return Some(Clip::Image { width: img.width, height: img.height, rgba: img.bytes.into_owned() });
    }
    cb.get_text().ok().filter(|t| !t.trim().is_empty()).map(Clip::Text)
}

fn png_from_rgba(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let (w, h) = (u32::try_from(width).map_err(|e| e.to_string())?, u32::try_from(height).map_err(|e| e.to_string())?);
    if w == 0 || h == 0 || rgba.len() != width * height * 4 {
        return Err("the clipboard image is empty or malformed".into());
    }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

impl PdfCraftApp {
    /// Convert a non-PDF file (image, text) into a new tab. Returns `None` when `bytes` is not
    /// something Create understands (the caller then tries to open it as a PDF).
    pub(crate) fn open_converted(&mut self, name: &str, bytes: &[u8]) -> Option<Result<(), String>> {
        use pdfcraft_engine::SourceKind;
        if !matches!(pdfcraft_engine::source_kind(name, bytes), Some(SourceKind::Image | SourceKind::Text)) {
            return None;
        }
        let created = self.session.convert_to_pdf(name, &Arc::new(bytes.to_vec())).map(|(_, pdf)| pdf);
        Some(self.open_created_bytes(&format!("{}.pdf", stem(name)), created.map_err(|e| e.to_string())))
    }

    fn open_created_bytes(&mut self, name: &str, created: Result<Arc<Vec<u8>>, String>) -> Result<(), String> {
        let bytes = created?;
        let id = self.session.open_new(name, bytes).map_err(|e| e.to_string())?;
        let info = &self.session.get(id).ok_or("the new document could not be opened")?.info;
        self.views.push(crate::DocView::new(id, info, self.view_defaults));
        self.active = Some(self.views.len() - 1);
        Ok(())
    }

    /// Create ▸ Clipboard: a new document from the image (one page, its size) or the text on
    /// the clipboard, as Acrobat does.
    pub(crate) fn create_from_clipboard(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        let clip = read_clipboard();
        #[cfg(target_arch = "wasm32")]
        let clip: Option<Clip> = None;
        match clip {
            Some(c) => {
                if let Err(e) = self.create_from_clip(c) {
                    self.notify_fmt("Couldn't create a PDF: {e}", &[("e", &e.to_string())]);
                }
            }
            None => self.notify_tr("The clipboard has no image or text"),
        }
    }

    /// Create a new document from clipboard contents.
    pub fn create_from_clip(&mut self, clip: Clip) -> Result<(), String> {
        let created = match clip {
            Clip::Image { width, height, rgba } => {
                let png = png_from_rgba(width, height, &rgba)?;
                self.session.create_from_images(&[("Clipboard.png".into(), png)])
            }
            Clip::Text(t) => self.session.create_from_text("Clipboard", &t),
        };
        self.open_created_bytes("Clipboard.pdf", created.map_err(|e| e.to_string()))
    }

    /// Create ▸ Blank page: a new untitled US Letter document.
    pub(crate) fn create_blank(&mut self) {
        let created = self.session.create_blank(612.0, 792.0, 1).map_err(|e| e.to_string());
        if let Err(e) = self.open_created_bytes("Untitled.pdf", created) {
            self.notify_fmt("Couldn't create a PDF: {e}", &[("e", &e.to_string())]);
        }
    }

    /// Create ▸ Images: several images, one page each, in one new document.
    pub(crate) fn create_from_images_dialog(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let dialog = rfd::AsyncFileDialog::new()
                .add_filter(tl!("Images"), &["png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "jp2", "j2k", "jpx"])
                .set_title(tl!("Choose images"));
            self.ask(crate::pickers::Ask::Files(dialog), None, |app, files| {
                let mut images = Vec::new();
                for f in files {
                    match std::fs::read(&f) {
                        Ok(b) => images.push((f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), b)),
                        Err(e) => {
                            app.notify_fmt("Couldn't read {name}: {e}", &[("name", &f.display().to_string()), ("e", &e.to_string())]);
                            return;
                        }
                    }
                }
                app.begin_image_import(images);
            });
        }
        #[cfg(target_arch = "wasm32")]
        self.notify_tr("On the web, open or drop an image to convert it");
    }

    /// One new document from images (tests and automation call this directly).
    pub fn create_from_images(&mut self, images: Vec<(String, Vec<u8>)>) {
        self.create_from_images_with_resolution(images, pdfcraft_engine::ImageResolution::Embedded);
    }

    /// Stage selected images for the DPI chooser; no document is created until confirmed.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn begin_image_import_paths(&mut self, paths: &[String]) -> Result<(), String> {
        let images = paths
            .iter()
            .map(|p| {
                let path = std::path::Path::new(p);
                let bytes = std::fs::read(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                Ok((name, bytes))
            })
            .collect::<Result<Vec<_>, String>>()?;
        self.begin_image_import(images);
        Ok(())
    }

    /// Stage selected images for the DPI chooser; no document is created until confirmed.
    pub fn begin_image_import(&mut self, images: Vec<(String, Vec<u8>)>) {
        if images.is_empty() {
            return;
        }
        self.image_import = Some(ImageImport { images, choice: ResolutionChoice::Image, dpi: 300.0 });
        self.dialog = Some(crate::Dialog::CreateImages);
    }

    pub(crate) fn finish_image_import(&mut self) {
        let Some(draft) = self.image_import.take() else { return };
        let resolution = match draft.choice {
            ResolutionChoice::Image => pdfcraft_engine::ImageResolution::Embedded,
            ResolutionChoice::Default72 => pdfcraft_engine::ImageResolution::Dpi(72.0),
            ResolutionChoice::Custom => pdfcraft_engine::ImageResolution::Dpi(draft.dpi),
        };
        self.create_from_images_with_resolution(draft.images, resolution);
    }

    /// Create directly at the requested resolution (also used by UI tests).
    pub fn create_from_images_with_resolution(&mut self, images: Vec<(String, Vec<u8>)>, resolution: pdfcraft_engine::ImageResolution) {
        if images.is_empty() {
            return;
        }
        let name =
            if let Some((name, _)) = images.first().filter(|_| images.len() == 1) { format!("{}.pdf", stem(name)) } else { "Images.pdf".to_string() };
        let created = self.session.create_from_images_with_resolution(&images, resolution).map_err(|e| e.to_string());
        if let Err(e) = self.open_created_bytes(&name, created) {
            self.notify_fmt("Couldn't create a PDF: {e}", &[("e", &e.to_string())]);
        }
    }

    /// Reduce File Size: write a compacted copy with images downsampled (the open document is
    /// unchanged). It runs in the background with a progress bar, like the PDF Optimizer.
    pub(crate) fn reduce_file_size(&mut self) {
        self.start_optimize(crate::optimize_ui::OptimizeKind::Reduce, &pdfcraft_engine::optimize::Settings::default(), &[]);
    }

    /// Save an optimized copy (Reduce File Size, Optimize PDF) next to the original, reporting
    /// the saving. A Reduce File Size copy that isn't smaller is not saved (#490).
    pub(crate) fn save_optimized(
        &mut self,
        id: pdfcraft_engine::DocId,
        kind: crate::optimize_ui::OptimizeKind,
        result: Result<(Arc<Vec<u8>>, String), pdfcraft_engine::EditError>,
    ) {
        let Some(doc) = self.session.get(id) else { return };
        let (before, name) = (doc.bytes.len(), format!("{} ({}).pdf", stem(&doc.name), kind.suffix()));
        let (bytes, detail) = match result {
            Ok(r) => r,
            Err(e) => {
                self.notify_fmt("Couldn't optimize the file: {e}", &[("e", &e.to_string())]);
                return;
            }
        };
        let after = bytes.len();
        if kind == crate::optimize_ui::OptimizeKind::Reduce && after >= before {
            self.notify_fmt(
                "This file is already as small as it can be made ({size}). No copy was saved.",
                &[("size", &crate::panels::human_size(before))],
            );
            return;
        }
        let saved = move |app: &mut PdfCraftApp, place: String| {
            let (b, a) = (crate::panels::human_size(before), crate::panels::human_size(after));
            if after < before {
                let pct = 100.0 * (1.0 - after as f64 / before.max(1) as f64);
                app.notify_fmt(
                    "Saved {place}: {before} → {after} ({pct}% smaller){detail}",
                    &[("place", &place), ("before", &b), ("after", &a), ("pct", &format!("{pct:.0}")), ("detail", &detail)],
                );
            } else {
                // Optimize PDF still saves: what it discards may matter more than the size.
                app.notify_fmt(
                    "Saved {place}: {before} → {after} (not smaller){detail}",
                    &[("place", &place), ("before", &b), ("after", &a), ("detail", &detail)],
                );
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let write = move |app: &mut Self, path: String| match crate::editing::write_atomically(&path, &bytes) {
                Ok(()) => saved(app, path),
                Err(e) => app.notify_fmt("Couldn't write {path}: {e}", &[("path", &path), ("e", &e.to_string())]),
            };
            match self.save_override.clone() {
                Some(p) => write(self, p),
                None => {
                    let dialog = rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).set_file_name(&name);
                    self.ask_one(crate::pickers::Ask::Save(dialog), None, move |app, p| write(app, p.to_string_lossy().into_owned()));
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        match crate::editing::download(&name, &bytes) {
            Ok(()) => saved(self, name),
            Err(e) => self.notify_fmt("Couldn't download {name}: {e}", &[("name", &name), ("e", &e.to_string())]),
        }
    }
}
