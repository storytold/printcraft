//! Optimize PDF ▸ Advanced optimization as a job that reports its progress.
//!
//! Optimizing a large file takes a while (every image is decoded, resampled and re-encoded), so
//! it is split like OCR: [`Session::optimize_job`] captures a copy of the document and the
//! choices, and [`OptimizeJob::run`] works anywhere (the UI runs it on a worker thread),
//! reporting each [`OptimizeStage`] and stopping when asked to.

use std::sync::Arc;

use pdfcraft_cos::{SaveOptions, write_full};

use crate::{DocId, EditError, Hidden, OptimizeReport, Session, guard, optimize};

/// Where an optimization is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OptimizeStage {
    /// Removing the user data chosen under Discard User Data.
    #[default]
    Discarding,
    /// About to process image `done` (0-based) of `total`.
    Images { done: usize, total: usize },
    /// Discarding objects and compressing unencoded streams.
    CleaningUp,
    /// Merging identical fonts and images.
    Merging,
    /// Writing the compressed copy.
    Writing,
}

impl OptimizeStage {
    /// How far along the whole run is, from 0 to 1. Images take most of the time; the rest are
    /// small fixed steps.
    pub fn fraction(self) -> f32 {
        match self {
            OptimizeStage::Discarding => 0.0,
            OptimizeStage::Images { done, total } => 0.05 + 0.75 * if total == 0 { 1.0 } else { done.min(total) as f32 / total as f32 },
            OptimizeStage::CleaningUp => 0.8,
            OptimizeStage::Merging => 0.85,
            OptimizeStage::Writing => 0.9,
        }
    }
}

/// The optimized copy's bytes and what was done.
pub type Optimized = (Arc<Vec<u8>>, OptimizeReport);

/// What an optimization needs, independent of the session.
pub struct OptimizeJob {
    cos: pdfcraft_cos::Document,
    settings: optimize::Settings,
    discard: Vec<Hidden>,
    opts: SaveOptions,
}

impl Session {
    /// Capture what optimizing document `id` with `settings` and the Remove Hidden Information
    /// `discard` categories needs. A full rewrite: signed documents are refused.
    pub fn optimize_job(&self, id: DocId, settings: &optimize::Settings, discard: &[Hidden]) -> Result<OptimizeJob, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        if doc.is_signed() {
            return Err(EditError::Signed);
        }
        let editor = doc.editor.as_ref().ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?;
        Ok(OptimizeJob { cos: editor.cos.clone(), settings: settings.clone(), discard: discard.to_vec(), opts: self.save_options() })
    }
}

impl OptimizeJob {
    /// Optimize and write the copy. `progress` is called as each stage starts and before each
    /// image; returning `false` stops with [`EditError::Cancelled`]. A panic is returned as an
    /// error, so a worker thread always reports back.
    pub fn run(self, mut progress: impl FnMut(OptimizeStage) -> bool) -> Result<Optimized, EditError> {
        guard(move || self.work(&mut progress)).map_err(EditError::Optimize)?
    }

    fn work(self, progress: &mut dyn FnMut(OptimizeStage) -> bool) -> Result<Optimized, EditError> {
        let OptimizeJob { mut cos, settings, discard, opts } = self;
        let mut step = |stage| if progress(stage) { Ok(()) } else { Err(EditError::Cancelled) };
        step(OptimizeStage::Discarding)?;
        let discarded = if discard.is_empty() { Vec::new() } else { pdfcraft_redact::sanitize::remove_hidden(&mut cos, &discard)? };
        let report = optimize::optimize_with_progress(&mut cos, &settings, &mut |s| {
            step(match s {
                optimize::Stage::Images { done, total } => OptimizeStage::Images { done, total },
                optimize::Stage::CleanUp => OptimizeStage::CleaningUp,
            })
            .is_ok()
        })
        .map_err(|e| match e {
            optimize::OptimizeError::Cancelled => EditError::Cancelled,
            e => EditError::Optimize(e.to_string()),
        })?;
        step(OptimizeStage::Merging)?;
        let all: Vec<pdfcraft_cos::ObjRef> = cos.object_numbers().into_iter().map(|n| pdfcraft_cos::ObjRef::new(n, cos.generation(n))).collect();
        let merged = pdfcraft_organize::dedupe_resources(&mut cos, &all, false);
        step(OptimizeStage::Writing)?;
        let bytes = write_full(&cos, &opts).map_err(|e| EditError::Write(e.to_string()))?;
        Ok((Arc::new(bytes), OptimizeReport { optimize: report, merged, discarded }))
    }
}
