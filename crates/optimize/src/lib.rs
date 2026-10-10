//! The PDF Optimizer (execution plan M11.1; Acrobat: Reduce File Size and Optimize PDF ▸
//! Advanced optimization).
//!
//! - **Images:** each image's effective resolution is measured where pages draw it (the
//!   smallest over its uses, through form XObjects); colour and grayscale images above a
//!   threshold are resampled (bicubic) to a target resolution and recompressed as JPEG or Flate.
//!   A new image replaces the old one only if it is smaller. Images PdfCraft can't decode
//!   faithfully (CMYK and other colour spaces, masks, decode arrays other than the default,
//!   JPEG 2000, JBIG2, CCITT, other than 8 bits) are left alone.
//! - **Discard objects:** page thumbnails, alternate images, document tags (structure tree),
//!   print settings.
//! - **Clean up:** drop images and forms that pages list but never draw (such as a deleted
//!   image); Flate-compress streams that have no filter.
//!
//! Metadata, attachments, comments, scripts, private data, hidden layers, bookmarks and form
//! fields are discarded by `pdfcraft-redact`'s Remove Hidden Information, which the engine
//! runs alongside.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod audit;
mod images;
mod links;
mod unused;

pub use audit::{SpaceCategory, SpaceUse, audit_space};

use pdfcraft_cos::{Document, ObjRef, Object, Stream};

pub use images::effective_resolutions;

#[derive(Debug, thiserror::Error)]
pub enum OptimizeError {
    #[error("the document has no page tree")]
    NoPages,
    #[error(transparent)]
    Cos(#[from] pdfcraft_cos::CosError),
    #[error("the optimization was cancelled")]
    Cancelled,
}

/// Where an optimization is, reported to [`optimize_with_progress`]'s callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// About to process image `done` (0-based) of `total`; `done == total` once all are done.
    Images { done: usize, total: usize },
    /// Discarding objects, cleaning up links and compressing unencoded streams.
    CleanUp,
}

/// How resampled (or recompressed) images are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    /// JPEG with a quality from 1 to 100.
    Jpeg(u8),
    /// Lossless (zlib).
    Flate,
    /// Keep the image's own compression (resampled images become Flate if they weren't JPEG).
    Retain,
}

/// Acrobat's JPEG quality steps.
pub const QUALITIES: [(&str, u8); 5] = [("Minimum", 25), ("Low", 45), ("Medium", 60), ("High", 80), ("Maximum", 95)];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageSettings {
    /// Resample images whose effective resolution is above `above_ppi` down to `target_ppi`.
    pub downsample: bool,
    pub target_ppi: f64,
    pub above_ppi: f64,
    pub compression: Compression,
}

impl ImageSettings {
    /// Reduce File Size: bicubic to 150 ppi above 225 ppi, JPEG medium quality.
    pub const REDUCE: ImageSettings = ImageSettings { downsample: true, target_ppi: 150.0, above_ppi: 225.0, compression: Compression::Jpeg(60) };
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub color: ImageSettings,
    pub gray: ImageSettings,
    pub discard_thumbnails: bool,
    pub discard_alternate_images: bool,
    pub discard_tags: bool,
    pub discard_print_settings: bool,
    /// Flate-compress streams stored without a filter.
    pub flate_unencoded: bool,
    /// Clean Up: remove links (and bookmark destinations) that point nowhere.
    pub remove_invalid_links: bool,
    /// Clean Up: remove named destinations nothing refers to.
    pub remove_unreferenced_dests: bool,
}

impl Default for Settings {
    /// Acrobat's Reduce File Size choices.
    fn default() -> Self {
        Self {
            color: ImageSettings::REDUCE,
            gray: ImageSettings::REDUCE,
            discard_thumbnails: true,
            discard_alternate_images: true,
            discard_tags: false,
            discard_print_settings: false,
            flate_unencoded: true,
            remove_invalid_links: true,
            remove_unreferenced_dests: true,
        }
    }
}

/// What an optimization did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub images: usize,
    pub images_resampled: usize,
    pub images_recompressed: usize,
    /// Encoded image bytes before and after (images that changed).
    pub image_bytes_before: usize,
    pub image_bytes_after: usize,
    pub thumbnails: usize,
    pub alternate_images: usize,
    pub tags_removed: bool,
    pub print_settings: usize,
    pub streams_compressed: usize,
    /// Links removed and bookmarks whose destination was cleared (Clean Up).
    pub invalid_links: usize,
    pub invalid_bookmarks: usize,
    pub unreferenced_dests: usize,
    /// Images and forms the pages listed but never drew, dropped (Clean Up).
    pub unused_xobjects: usize,
}

/// Optimize `doc` in place.
pub fn optimize(doc: &mut Document, settings: &Settings) -> Result<Report, OptimizeError> {
    optimize_with_progress(doc, settings, &mut |_| true)
}

/// [`optimize`], calling `progress` before each image and before the clean-up (and now and then
/// during a long one). Returning `false` stops with [`OptimizeError::Cancelled`]; `doc` is then
/// partly optimized and should be dropped.
pub fn optimize_with_progress(doc: &mut Document, settings: &Settings, progress: &mut dyn FnMut(Stage) -> bool) -> Result<Report, OptimizeError> {
    let mut report = Report::default();
    let pages = pdfcraft_annot::page_refs(doc).map_err(|_| OptimizeError::NoPages)?;
    images::run(doc, &pages, settings, &mut report, progress)?;
    if !progress(Stage::CleanUp) {
        return Err(OptimizeError::Cancelled);
    }
    report.unused_xobjects = unused::remove_unused(doc, &pages, progress)?;
    if settings.discard_thumbnails {
        for p in &pages {
            if doc.get(*p).as_dict().is_some_and(|d| d.contains(b"Thumb")) {
                doc.update_dict(*p, |d| {
                    d.remove(b"Thumb");
                })?;
                report.thumbnails += 1;
            }
        }
    }
    if settings.discard_alternate_images {
        for num in doc.object_numbers() {
            let r = ObjRef { num, generation: doc.generation(num) };
            let alt = matches!(&*doc.get(r), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image") && s.dict.contains(b"Alternates"));
            if alt {
                if let Object::Stream(s) = &*doc.get(r) {
                    let mut s = s.clone();
                    s.dict.remove(b"Alternates");
                    doc.set(r, Object::Stream(s));
                }
                report.alternate_images += 1;
            }
        }
    }
    let root = doc.root();
    if settings.discard_tags
        && let Some(root) = root
        && doc.get(root).as_dict().is_some_and(|c| c.contains(b"StructTreeRoot"))
    {
        doc.update_dict(root, |c| {
            c.remove(b"StructTreeRoot");
            c.remove(b"MarkInfo");
        })?;
        for p in &pages {
            if doc.get(*p).as_dict().is_some_and(|d| d.contains(b"StructParents")) {
                doc.update_dict(*p, |d| {
                    d.remove(b"StructParents");
                })?;
            }
        }
        report.tags_removed = true;
    }
    if settings.discard_print_settings
        && let Some(root) = root
    {
        const PRINT: [&[u8]; 7] = [b"PrintScaling", b"Duplex", b"PickTrayByPDFSize", b"PrintPageRange", b"NumCopies", b"PrintArea", b"PrintClip"];
        let vp = doc.get(root).as_dict().and_then(|c| c.get(b"ViewerPreferences").cloned());
        let edit = |d: &mut pdfcraft_cos::Dict| PRINT.iter().filter(|k| d.remove(k).is_some()).count();
        match vp {
            Some(Object::Ref(r)) => {
                let mut n = 0;
                doc.update_dict(r, |d| n = edit(d))?;
                report.print_settings += n;
            }
            Some(Object::Dict(_)) => {
                let mut n = 0;
                doc.update_dict(root, |c| {
                    if let Some(Object::Dict(d)) = c.get_mut(b"ViewerPreferences") {
                        n = edit(d);
                    }
                })?;
                report.print_settings += n;
            }
            _ => {}
        }
    }
    if settings.remove_invalid_links {
        let (l, b) = links::remove_invalid(doc, &pages)?;
        report.invalid_links = l;
        report.invalid_bookmarks = b;
    }
    if settings.remove_unreferenced_dests {
        report.unreferenced_dests = links::remove_unreferenced_dests(doc)?;
    }
    if settings.flate_unencoded {
        for num in doc.object_numbers() {
            let r = ObjRef { num, generation: doc.generation(num) };
            let obj = doc.get(r);
            let Object::Stream(s) = &*obj else { continue };
            // XMP stays readable as plain text (PDF/A requires it); tiny streams don't gain.
            if s.dict.contains(b"Filter") || s.dict.name(b"Type") == Some(b"Metadata") || s.dict.name(b"Type") == Some(b"XRef") || s.raw.len() < 64 {
                continue;
            }
            let mut d = s.dict.clone();
            d.remove(b"Length");
            let packed = Stream::flate(d, &s.raw);
            if packed.raw.len() < s.raw.len() {
                doc.set(r, Object::Stream(packed));
                report.streams_compressed += 1;
            }
        }
    }
    Ok(report)
}
