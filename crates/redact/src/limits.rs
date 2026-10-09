//! Bounds shared by the redaction passes. Every pass that reads decoded content uses the same
//! per-stream cap and spends from a per-page budget, so a document costs a fixed amount of memory
//! and time however its content is arranged. Hitting a bound is a failure (the operation is
//! refused), never a quiet skip.

use pdfcraft_cos::Stream;

/// Deepest nesting of form XObjects, patterns and mask groups that is walked.
pub(crate) const MAX_DEPTH: usize = 12;
/// Most form XObjects, patterns or glyph procedures one scope may expand (a cycle or a fan-out of
/// references can't hang it).
pub(crate) const MAX_FORMS: usize = 4096;
/// Most decoded bytes of one content stream (a page's, a form's, a pattern cell, a glyph
/// procedure or a mask group).
pub(crate) const MAX_STREAM: usize = 64 << 20;
/// Most decoded bytes of an embedded CMap.
pub(crate) const MAX_CMAP: usize = 4 << 20;
/// Most decoded bytes one page may cost in total: its own streams plus every form, pattern and
/// glyph procedure scanned or rewritten for it.
pub(crate) const MAX_PAGE_TOTAL: usize = 256 << 20;

/// Most pixels an image may have to be cleared in place or placed by the proof (larger ones are
/// removed, or can't be inspected).
pub(crate) const MAX_PIXELS: u64 = 64 << 20;
/// Most decoded bytes of an image that is cleared in place.
pub(crate) const MAX_IMAGE_BYTES: u64 = 256 << 20;

/// How far (in points) a glyph box or an image cell must reach into a region before it counts as
/// under it. Only absorbs floating-point noise: a glyph that merely touches an edge stays, any
/// real overlap goes.
pub(crate) const EDGE_EPS: f64 = 1e-3;

// Bounds of the proof (`verify`).

/// Most decoded bytes the whole proof reads, over every stream and every pass.
pub(crate) const MAX_TOTAL_DECODED: u64 = 2 << 30;
/// Most byte comparisons the needle search may spend in one proof (about 4 GiB of haystack).
pub(crate) const MAX_WORK: u64 = 4 << 30;
/// Nested attachment levels that are swept (deeper ones are reported as unswept).
pub(crate) const MAX_NESTING: usize = 4;
/// Nesting bound of object walks (deeper is reported, never skipped silently).
pub(crate) const MAX_WALK: usize = 64;
/// Most CMap range steps one font may cost (a range that never grows the map still costs).
pub(crate) const MAX_CMAP_WORK: usize = 4 << 20;
/// Most entries one ToUnicode CMap may define.
pub(crate) const MAX_CMAP_ENTRIES: usize = 1 << 20;
/// Needles shorter than this (in characters) are too common to look for in raw bytes.
pub(crate) const MIN_NEEDLE: usize = 3;
/// Smallest needle looked for in the raw file and in decoded streams. Those hold object syntax,
/// numbers and binary data, where three characters match by chance (`/Length 612`, a
/// `/MediaBox`, pixels); shorter needles are still swept on page text, strings, names, Info, XMP
/// and attachment text.
pub(crate) const MIN_STRUCTURED_NEEDLE: usize = 6;

/// What is left to decode.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget(usize);

/// Why a stream was not decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// Damaged, truncated, an unsupported codec, or longer than [`MAX_STREAM`].
    Unreadable,
    /// Readable, but the budget is used up.
    OverBudget,
}

impl Budget {
    pub(crate) fn new(total: usize) -> Self {
        Budget(total)
    }

    /// Decode `s` strictly (any damage is an error: nothing after a corrupt spot may go
    /// unexamined) within [`MAX_STREAM`] and what is left of the budget, and spend what it took.
    pub(crate) fn decode(&mut self, s: &Stream) -> Result<Vec<u8>, Refused> {
        let cap = MAX_STREAM.min(self.0);
        match s.decoded_strict_within(cap) {
            Ok(data) => {
                self.0 = self.0.saturating_sub(data.len());
                Ok(data)
            }
            // Out of room rather than damaged: tell the two apart by decoding once more with
            // the full per-stream cap (bounded, and only on this failing path).
            Err(_) if cap < MAX_STREAM && s.decoded_strict_within(MAX_STREAM).is_ok() => Err(Refused::OverBudget),
            Err(_) => Err(Refused::Unreadable),
        }
    }
}
