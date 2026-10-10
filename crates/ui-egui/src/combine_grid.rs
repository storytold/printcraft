//! Combine files as a grid: a thumbnail of the first page each file contributes, drawn as a
//! stack when it contributes more than one, with its name below. Cards are selected and moved
//! with the same [`RowAction`]s as the table's rows, so both views share the selection, the
//! order and the undo history.
//!
//! Thumbnails render off the UI thread through [`RenderPool`] (its panic guard, size caps and
//! watchdog), one small pool per thumbnail. Only missing ones are asked for, those in view first.
//! At most [`MAX_THREADS`] render threads exist at once, counted as threads, not pools: the pools
//! start no watchdog replacements ([`RenderPool::without_replacements`]), and a pool's threads
//! count until they have actually exited ([`RenderPool::retire`]), so a render the watchdog gave
//! up on, or one whose file left the list, holds its place until its thread is gone. A thread
//! stuck for good in one step that can't be interrupted keeps its place for good: threads can't
//! be killed safely. On the web the pool renders on the UI thread inside `try_recv`, with no
//! watchdog: there, at most one thumbnail per frame, only one waits at a time (its pool holds the
//! parsed document), and a very slow page still stalls that frame (as in the document view).

use std::collections::HashMap;
use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, TextureHandle, Vec2, pos2, vec2};
use pdfcraft_render::{RenderConfig, RenderPool, RenderRequest, Retiring};

use crate::PdfCraftApp;
use crate::combine_ui::{CombineFile, ERROR, Lock, RowAction, WARNING};
use crate::icons;
use crate::theme::{self, Tokens};

/// Render threads for thumbnails at once, counting those of retired pools until they exit.
const MAX_THREADS: usize = 3;
/// Results taken per frame, and the bytes they may upload.
const UPLOADS_PER_FRAME: usize = 4;
const UPLOAD_BYTES_PER_FRAME: usize = 8 << 20;
/// Thumbnail textures kept, in bytes. Those out of view go first, longest unseen first; those in
/// view are kept even over budget (at most a screenful of thumbnails of at most [`MAX_SIDE`]).
const TEXTURE_BUDGET: usize = 32 << 20;
/// The longest side of a thumbnail, in pixels.
/// (Room for a Letter page at 200% on a screen of 1.5 pixels per point; sharper screens upscale a
/// little at the largest sizes.)
const MAX_SIDE: f32 = 768.0;
/// Thumbnail sizes round up to this many pixels, so a slightly different layout reuses them.
const SIZE_STEP: u32 = 16;
/// The box a page is fitted in at 100%, and the room the card adds around it (for the hover bar
/// and the stack above it, the name and a note below), which doesn't scale.
const PAPER: Vec2 = vec2(132.0, 172.0);
const AROUND: Vec2 = vec2(52.0, 90.0);
/// From the top of a card to its page box: room for the hover bar, which sits mostly above the
/// page.
const ABOVE: f32 = 32.0;
/// The hover bar's height; a quarter of it overlaps the page.
const BAR_H: f32 = 34.0;
/// The grid's zoom range, and the sizes its buttons and keys step through (the slider, the wheel
/// and a pinch go anywhere between).
pub(crate) const ZOOM_RANGE: std::ops::RangeInclusive<f32> = 0.6..=2.0;
const ZOOM_STEPS: [f32; 7] = [0.6, 0.8, 1.0, 1.25, 1.5, 1.75, 2.0];

/// The next size up (or down) from `zoom`; the end of the range stays put.
pub(crate) fn zoom_step(zoom: f32, larger: bool) -> f32 {
    let next =
        if larger { ZOOM_STEPS.iter().copied().find(|s| *s > zoom + 0.001) } else { ZOOM_STEPS.iter().rev().copied().find(|s| *s < zoom - 0.001) };
    next.unwrap_or(if larger { *ZOOM_RANGE.end() } else { *ZOOM_RANGE.start() })
}
/// Space above the first row of cards.
const TOP: f32 = 12.0;
/// The page shown large (a card's magnifier) is kept beside the thumbnails under this key, so it
/// shares their threads, their limits and their checks.
const PREVIEW: u64 = u64::MAX;
/// The longest side of the page shown large, in pixels.
const PREVIEW_SIDE: f32 = 2048.0;
/// The shape drawn for a page whose size is unknown (US Letter); nothing is rendered for it.
const FALLBACK_PAGE: (f32, f32) = (612.0, 792.0);
/// Icons drawn on the (always white) paper, in either theme: the light theme's muted text, which
/// keeps its contrast on white where the dark theme's doesn't.
const ON_PAPER: Color32 = Color32::from_rgb(0x5E, 0x5E, 0x66);

/// What the grid has for a file (tests and the control channel).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThumbState {
    /// Nothing yet: not in view, or nothing to draw (locked, unreadable, a bad page range, a page
    /// whose size is unknown).
    None,
    /// Being rendered.
    Pending,
    /// A thumbnail of page `page` (0-based), `width` × `height` pixels.
    Ready { page: usize, width: u32, height: u32 },
    /// The page couldn't be drawn.
    Failed(String),
}

/// A card being dragged: which file, and the list as it was (see [`RowAction::DropAt`]).
#[derive(Clone, Copy)]
struct GridDrag {
    file: u64,
    revision: u64,
}

/// Everything a thumbnail depends on: the file's bytes, how it was read (its password), the page
/// and the size. A cached thumbnail is used only while all of them still match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    /// The address of the file's shared bytes: the same file, even after undo brings it back.
    source: usize,
    auth: u64,
    page: usize,
    px: [u32; 2],
}

impl Key {
    fn same_page(&self, other: &Key) -> bool {
        (self.source, self.auth, self.page) == (other.source, other.auth, other.page)
    }
}

struct Entry {
    key: Key,
    tex: Option<TextureHandle>,
    error: Option<String>,
    /// The frame the card last showed it.
    seen: u64,
    bytes: usize,
}

struct Job {
    file: u64,
    key: Key,
    pool: RenderPool,
    tag: u64,
}

/// A thumbnail a card needs this frame.
struct Want {
    file: u64,
    key: Key,
    bytes: Arc<Vec<u8>>,
    password: Option<Arc<str>>,
    scale: f32,
    visible: bool,
}

/// The grid's thumbnails and the renders under way.
#[derive(Default)]
pub(crate) struct Thumbs {
    entries: HashMap<u64, Entry>,
    jobs: Vec<Job>,
    /// Threads of pools let go of, until they exit.
    retiring: Vec<Retiring>,
    frame: u64,
    next_tag: u64,
    wants: Vec<Want>,
    /// Render on the UI thread, as the web build does (tests).
    #[cfg(test)]
    inline: bool,
}

/// What a card can draw now.
enum Look {
    Ready(egui::TextureId),
    Failed(String),
    /// Not ready; an older thumbnail of the same page may stand in.
    Pending(Option<egui::TextureId>),
}

impl Thumbs {
    /// Forget every thumbnail (the tab closed). Renders under way are let go of: told to stop at
    /// their next drawing operator, and counted until their threads exit.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.wants.clear();
        for job in self.jobs.drain(..) {
            self.retiring.push(job.pool.retire());
        }
    }

    /// Render threads that exist now (real threads, not pools): those of pools at work and of
    /// pools let go of.
    fn threads(&mut self) -> usize {
        self.retiring.retain_mut(|r| r.running() > 0);
        let retiring: usize = self.retiring.iter_mut().map(Retiring::running).sum();
        let working: usize = self.jobs.iter().map(|j| j.pool.threads()).sum();
        working.saturating_add(retiring)
    }

    fn look(&mut self, file: u64, key: Key) -> Look {
        let frame = self.frame;
        match self.entries.get_mut(&file) {
            Some(e) if e.key == key => {
                e.seen = frame;
                match (&e.tex, &e.error) {
                    (Some(tex), _) => Look::Ready(tex.id()),
                    (None, Some(err)) => Look::Failed(err.clone()),
                    (None, None) => Look::Pending(None),
                }
            }
            // Another size of the same page stands in until this one arrives. A page that failed
            // isn't tried again at another size: it would fail (or hang) the same way, each zoom.
            Some(e) if e.key.same_page(&key) => {
                e.seen = frame;
                match &e.error {
                    Some(err) if e.tex.is_none() => Look::Failed(err.clone()),
                    _ => Look::Pending(e.tex.as_ref().map(TextureHandle::id)),
                }
            }
            _ => Look::Pending(None),
        }
    }

    fn state(&self, file: u64) -> ThumbState {
        if let Some(e) = self.entries.get(&file) {
            if let Some(tex) = &e.tex {
                let [w, h] = tex.size();
                return ThumbState::Ready {
                    page: e.key.page,
                    width: u32::try_from(w).unwrap_or(u32::MAX),
                    height: u32::try_from(h).unwrap_or(u32::MAX),
                };
            }
            if let Some(err) = &e.error {
                return ThumbState::Failed(err.clone());
            }
        }
        if self.jobs.iter().any(|j| j.file == file) { ThumbState::Pending } else { ThumbState::None }
    }

    /// Once per frame, whatever shows: drop what no longer matches the list, take finished
    /// renders, start those the grid asked for and keep the textures within budget.
    fn finish(&mut self, ctx: &egui::Context, current: &HashMap<u64, Key>) {
        // Reconcile with the list as it is now (undo, redo, unlock, a new range, removal): a
        // thumbnail of another page, password or file is never shown again.
        self.entries.retain(|file, e| current.get(file).is_some_and(|k| k.same_page(&e.key)));
        // Renders nobody wants any more are let go of now (on the web, before they would render).
        let (keep, gone): (Vec<Job>, Vec<Job>) = self.jobs.drain(..).partition(|job| current.get(&job.file).is_some_and(|k| k.same_page(&job.key)));
        self.jobs = keep;
        self.retiring.extend(gone.into_iter().map(|job| job.pool.retire()));

        // Finished renders, a few per frame. A pool without threads (the web) renders inside
        // `try_recv`: at most one of those per frame.
        let gpu_side = ctx.input(|i| i.max_texture_side);
        let max_side = gpu_side.min(2 * MAX_SIDE as usize);
        let (mut taken, mut uploaded, mut inline_used) = (0usize, 0usize, false);
        let mut i = 0;
        while i < self.jobs.len() {
            if taken >= UPLOADS_PER_FRAME || uploaded >= UPLOAD_BYTES_PER_FRAME {
                break;
            }
            let Some(job) = self.jobs.get(i) else { break };
            if job.pool.is_inline() {
                if inline_used {
                    i += 1;
                    continue;
                }
                inline_used = true;
            }
            // Not done yet, or not this job's answer (cannot happen with one request per pool):
            // keep waiting. A pool without threads answers on the call that renders, so there no
            // answer is one that never comes: the thumbnail fails rather than hold up the rest.
            let answer = job.pool.try_recv().filter(|out| out.request.tag == job.tag);
            if answer.is_none() && !job.pool.is_inline() {
                i += 1;
                continue;
            }
            let job = self.jobs.swap_remove(i);
            taken += 1;
            let failed = |e: String| Entry { key: job.key, tex: None, error: Some(e), seen: self.frame, bytes: 0 };
            let entry = match answer {
                None => failed("the page wasn't drawn".to_owned()),
                Some(out) => match out.error {
                    Some(e) => failed(e),
                    None => match texture(
                        ctx,
                        job.file,
                        [out.width, out.height],
                        out.rgba,
                        if job.file == PREVIEW { gpu_side.min(PREVIEW_SIDE as usize) } else { max_side },
                    ) {
                        Ok((tex, bytes)) => {
                            uploaded = uploaded.saturating_add(bytes);
                            Entry { key: job.key, tex: Some(tex), error: None, seen: self.frame, bytes }
                        }
                        Err(e) => failed(e),
                    },
                },
            };
            self.entries.insert(job.file, entry);
            // Its thread (idle now, or still finishing a render the watchdog gave up on) counts
            // until it exits.
            self.retiring.push(job.pool.retire());
        }

        // New renders: the cards in view first, in grid order; one per file at a time.
        let mut wants = std::mem::take(&mut self.wants);
        wants.sort_by_key(|w| (w.file != PREVIEW, !w.visible));
        // A pool without threads (the web) parses its document when it is made and keeps it until
        // it renders, one per frame: while one waits, nothing else starts.
        // (Nor in the frame that rendered one: one heavy step per frame on the UI thread.)
        let mut threads = if inline_used || self.jobs.iter().any(|j| j.pool.is_inline()) { MAX_THREADS } else { self.threads() };
        let mut waiting = false;
        for w in wants {
            // Wanted for the list as it is now (not changed by this frame's action); one render
            // per file at a time; and a want made earlier this frame may have just been answered.
            if current.get(&w.file).is_none_or(|k| !k.same_page(&w.key))
                || self.jobs.iter().any(|j| j.file == w.file)
                || self.entries.get(&w.file).is_some_and(|e| e.key == w.key)
            {
                continue;
            }
            if threads >= MAX_THREADS {
                waiting = true;
                break;
            }
            let config = RenderConfig { password: w.password, ..Default::default() };
            #[cfg(test)]
            let pool = if self.inline { RenderPool::new_inline(w.bytes, config) } else { RenderPool::new(w.bytes, 1, config).without_replacements() };
            #[cfg(not(test))]
            let pool = RenderPool::new(w.bytes, 1, config).without_replacements();
            self.next_tag = self.next_tag.wrapping_add(1);
            let tag = self.next_tag;
            // The request stays in the pool's queue (it is never replaced) until its answer.
            pool.set_queue(vec![RenderRequest { page: w.key.page, scale: w.scale, tag, ..Default::default() }]);
            threads = if pool.is_inline() { MAX_THREADS } else { threads.saturating_add(1) };
            self.jobs.push(Job { file: w.file, key: w.key, pool, tag });
        }

        // Within budget: drop the thumbnails out of view longest unseen first.
        // (The page shown large has its own: one texture, gone when it closes.)
        let mut total: usize = self.entries.iter().filter(|(f, _)| **f != PREVIEW).map(|(_, e)| e.bytes).sum();
        if total > TEXTURE_BUDGET {
            let mut old: Vec<(u64, u64, usize)> = self
                .entries
                .iter()
                .filter(|(f, e)| **f != PREVIEW && e.seen != self.frame && e.bytes > 0)
                .map(|(f, e)| (e.seen, *f, e.bytes))
                .collect();
            old.sort_unstable();
            for (_, file, bytes) in old {
                if total <= TEXTURE_BUDGET {
                    break;
                }
                self.entries.remove(&file);
                total = total.saturating_sub(bytes);
            }
        }

        // The cards were drawn before these results arrived: draw them again. Keep polling while
        // renders are under way, and while a thumbnail waits for a thread to exit (not when
        // nothing waits: a thread exiting changes nothing on screen).
        if taken > 0 || self.jobs.iter().any(|j| j.pool.is_inline()) {
            ctx.request_repaint();
        } else if !self.jobs.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        } else if waiting {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.frame = self.frame.wrapping_add(1);
    }
}

/// A rendered thumbnail as a texture, and its size in bytes. A raster that doesn't add up, or is
/// larger than a thumbnail can be (or than the GPU accepts), is refused, never uploaded.
fn texture(
    ctx: &egui::Context,
    file: u64,
    size: [u32; 2],
    pixels: pdfcraft_render::Pixels,
    max_side: usize,
) -> Result<(TextureHandle, usize), String> {
    let [w, h] = size.map(|v| v as usize);
    if w == 0 || h == 0 || w > max_side || h > max_side {
        return Err(format!("the thumbnail came back {w}×{h} pixels"));
    }
    let bytes = w.checked_mul(h).and_then(|p| p.checked_mul(4)).ok_or("the thumbnail is too large")?;
    if pixels.len() != bytes {
        return Err("the thumbnail came back incomplete".to_owned());
    }
    let image = crate::canvas::texture_image([w, h], pixels);
    Ok((ctx.load_texture(format!("combine-thumb-{file}"), image, egui::TextureOptions::LINEAR), bytes))
}

/// `name` shortened in the middle to what `fits`, keeping its extension ("Pilot Boat Ad….pdf").
pub(crate) fn fit_name(name: &str, fits: impl Fn(&str) -> bool) -> String {
    if fits(name) {
        return name.to_owned();
    }
    // A short extension after the last dot (a dot is one byte, so these are char boundaries).
    let split = name.rfind('.').filter(|p| *p > 0 && name.len() - p <= 6);
    let (stem, ext) = match split {
        Some(p) => (name.get(..p).unwrap_or(name), name.get(p..).unwrap_or("")),
        None => (name, ""),
    };
    let chars: Vec<char> = stem.chars().collect();
    let with = |n: usize| format!("{}…{ext}", chars.iter().take(n).collect::<String>());
    // The longest start of the stem that fits, found by halving.
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if fits(&with(mid)) { lo = mid } else { hi = mid - 1 }
    }
    with(lo)
}

/// The box a page is fitted in at `zoom` (clamped to [`ZOOM_RANGE`]).
fn paper_box(zoom: f32) -> Vec2 {
    let zoom = if zoom.is_finite() { zoom.clamp(*ZOOM_RANGE.start(), *ZOOM_RANGE.end()) } else { 1.0 };
    PAPER * zoom
}

/// The size to draw a page of `w` × `h` points in `fit`; `None` for a size that isn't one.
fn paper_size(w: f32, h: f32, fit: Vec2) -> Option<Vec2> {
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return None;
    }
    let s = (fit.x / w).min(fit.y / h);
    s.is_finite().then(|| vec2(w * s, h * s))
}

/// How many cards of `cell` fit across `width` (at least one).
fn columns_in(width: f32, cell: Vec2) -> usize {
    ((width - 16.0) / cell.x).floor().max(1.0) as usize
}

/// The grid of files, in the card that holds the list. Returns what the user did.
pub(crate) fn grid(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens, checks: &[Result<usize, String>], selected: &[bool]) -> Option<RowAction> {
    let ctx = ui.ctx().clone();
    let ppp = ctx.pixels_per_point();
    let mut action = None;
    let revision = app.combine_tab.revision;
    let drag = egui::DragAndDrop::payload::<GridDrag>(&ctx).map(|d| *d);
    // The card dragged may scroll out of the rows drawn, so its release is watched here.
    let released = drag.is_some() && ctx.input(|i| i.pointer.any_released());
    let n = app.combine_draft.len();
    // The same file twice is allowed (e.g. a cover sheet), but probably a mistake.
    let mut copies: HashMap<(&str, usize), usize> = HashMap::new();
    for f in &app.combine_draft {
        *copies.entry((f.name.as_str(), f.bytes.len())).or_default() += 1;
    }
    let twice: Vec<bool> = app.combine_draft.iter().map(|f| copies.get(&(f.name.as_str(), f.bytes.len())).is_some_and(|n| *n > 1)).collect();
    let reveal = app.combine_tab.reveal.take();
    let fit = paper_box(app.combine_zoom);
    let cell = fit + AROUND;
    let mut cells: Vec<(usize, Rect)> = Vec::new();
    let viewport = ui.available_rect_before_wrap();
    let pointer = ctx.pointer_latest_pos();
    let files = &mut app.combine_draft;
    let thumbs = &mut app.combine_thumbs;
    let columns = &mut app.combine_tab.grid_columns;

    // Over the grid itself, not over a menu or dialog above it.
    let over_grid = ui.rect_contains_pointer(viewport);
    let mut width = viewport.width();
    let mut focused_card = None;
    let output = egui::ScrollArea::vertical().id_salt("combine-grid").auto_shrink([false, false]).show_viewport(ui, |ui, clip| {
        // Dragging near the top or bottom edge scrolls.
        if drag.is_some()
            && let Some(p) = pointer
            && viewport.contains(p)
        {
            let edge = 40.0;
            let delta = if p.y < viewport.top() + edge {
                12.0
            } else if p.y > viewport.bottom() - edge {
                -12.0
            } else {
                0.0
            };
            if delta != 0.0 {
                ui.scroll_with_delta(vec2(0.0, delta));
                ctx.request_repaint();
            }
        }
        ui.add_space(TOP);
        width = ui.available_width();
        let cols = columns_in(width, cell);
        *columns = cols;
        let rows = n.div_ceil(cols);
        let left = ((width - cols as f32 * cell.x) / 2.0).max(0.0);
        let (area, _) = ui.allocate_exact_size(vec2(width, rows as f32 * cell.y), Sense::hover());
        let cell_rect = |i: usize| {
            let (row, col) = (i / cols, i % cols);
            Rect::from_min_size(pos2(area.left() + left + col as f32 * cell.x, area.top() + row as f32 * cell.y), cell)
        };
        if let Some(i) = reveal.and_then(|id| files.iter().position(|f| f.id == id)) {
            ui.scroll_to_rect(cell_rect(i), None);
        }
        for row in crate::canvas::thumbnail_rows(clip.top() - TOP, clip.bottom() - TOP, cell.y, rows) {
            for col in 0..cols {
                let i = row * cols + col;
                let Some(f) = files.get_mut(i) else { break };
                let c = cell_rect(i);
                cells.push((i, c));
                let card = Card {
                    i,
                    rect: c,
                    check: checks.get(i).cloned().unwrap_or(Ok(f.pages)),
                    selected: selected.get(i).copied().unwrap_or(false),
                    twice: twice.get(i).copied().unwrap_or(false),
                    revision,
                    ppp,
                    fit,
                };
                let card_id = ui.id().with(("combine-card", f.id));
                if let Some(a) = card.show(ui, t, f, thumbs) {
                    action = Some(a);
                }
                if ui.memory(|m| m.has_focus(card_id)) {
                    focused_card = Some(card_id);
                }
            }
        }
        // While a card is dragged: the gap it would go to, drawn as a bar, and how many go.
        if let Some(d) = drag
            && let Some(p) = pointer
        {
            let gap = crate::canvas::drop_gap(&cells, p);
            // Drawn at the near edge of the card nearest the pointer (as `drop_gap` decides), so a
            // gap at the end of a row shows on that row, not at the start of the next.
            let nearest = cells.iter().min_by(|(_, a), (_, b)| a.distance_sq_to_pos(p).total_cmp(&b.distance_sq_to_pos(p)));
            if let Some((_, r)) = nearest {
                let x = if p.x < r.center().x { r.left() + 2.0 } else { r.right() - 2.0 };
                ui.painter().line_segment([pos2(x, r.top() + 10.0), pos2(x, r.bottom() - 30.0)], Stroke::new(3.0, t.accent));
            }
            let moving = files
                .iter()
                .position(|f| f.id == d.file)
                .map_or(1, |i| if selected.get(i).copied().unwrap_or(false) { selected.iter().filter(|s| **s).count() } else { 1 });
            let label = if moving == 1 { tl!("1 file").to_string() } else { crate::i18n::fmt(tl!("{n} files"), &[("n", &moving.to_string())]) };
            ui.painter().text(p + vec2(14.0, 14.0), Align2::LEFT_TOP, label, theme::medium(12.0), t.accent_text);
            // Dropped in the grid: there. Anywhere else: nothing moves.
            if released
                && viewport.contains(p)
                && let Some(gap) = gap
            {
                action = Some(RowAction::DropAt { file: d.file, revision: d.revision, gap });
            }
        }
    });

    app.combine_tab.card_focus = focused_card;

    // Pinch, or Ctrl/⌘ with the wheel, over the grid zooms it.
    let pinch = ctx.input(|i| i.zoom_delta());
    if (pinch - 1.0).abs() > 0.001 && over_grid {
        app.combine_tab.zoom_request = Some(app.combine_zoom * pinch);
    }
    // A zoom asked for this frame (toolbar, keys, pinch) applies now the grid has been laid out:
    // the card under the pointer (or else the first in view) keeps its place on screen. Not while
    // a card is dragged: the grid under it would change.
    if let Some(zoom) = app.combine_tab.zoom_request.take()
        && drag.is_none()
    {
        let before = app.combine_zoom;
        app.set_combine_zoom(zoom);
        if app.combine_zoom != before {
            // The same card all through one gesture (a slider drag, a turn of the wheel): the
            // first in view changes as the cards reflow, and following it would drift.
            let now = ctx.input(|i| i.time);
            let gesture = app
                .combine_tab
                .zoom_anchor
                .filter(|a| now - a.2 < 0.5)
                .and_then(|(id, above, _)| app.combine_draft.iter().position(|f| f.id == id).map(|i| (i, above)));
            // Cards just outside the view are drawn too (under the toolbar, say): the pointer
            // counts only over the grid.
            let under = pointer.filter(|_| over_grid).and_then(|p| cells.iter().find(|(_, r)| r.contains(p)));
            let anchor = gesture
                .or_else(|| under.or_else(|| cells.iter().find(|(_, r)| r.bottom() > viewport.top())).map(|(i, r)| (*i, r.top() - viewport.top())));
            app.combine_tab.zoom_anchor = anchor.and_then(|(i, above)| app.combine_draft.get(i).map(|f| (f.id, above, now)));
            if let Some((i, above)) = anchor {
                // Where it will be: its row at the new size, as far below the top as it is now;
                // within what can be scrolled to, so zooming out at the bottom shows no gap.
                let cell = paper_box(app.combine_zoom) + AROUND;
                let cols = columns_in(width, cell);
                let content = TOP + n.div_ceil(cols) as f32 * cell.y;
                let y = (TOP + (i / cols) as f32 * cell.y - above).min(content - viewport.height()).max(0.0);
                // Set as the scroll position itself, before the next frame draws anything: no
                // jump, and no scroll still animating (to a card revealed by the keys, say) can
                // carry it elsewhere.
                let mut state = egui::scroll_area::State::default();
                state.offset = vec2(0.0, y);
                state.store(&ctx, output.id);
            }
            ctx.request_repaint();
        }
    }

    action
}

/// One card's place and what it shows.
struct Card {
    i: usize,
    rect: Rect,
    check: Result<usize, String>,
    selected: bool,
    twice: bool,
    revision: u64,
    ppp: f32,
    /// The box the page is fitted in (the zoom).
    fit: Vec2,
}

impl Card {
    fn show(&self, ui: &mut egui::Ui, t: &Tokens, f: &mut CombineFile, thumbs: &mut Thumbs) -> Option<RowAction> {
        let mut action = None;
        let c = self.rect;
        let id = ui.id().with(("combine-card", f.id));
        let resp = ui.interact(c.shrink(4.0), id, Sense::click_and_drag());
        let name = f.name.clone();
        // As the organize grid's pages: a check box that is on when selected; and, for screen
        // readers, selected as items in a list are.
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, self.selected, &name));
        let pages = if f.pages == 1 { tl!("1 page").to_string() } else { crate::i18n::fmt(tl!("{n} pages"), &[("n", &f.pages.to_string())]) };
        let painter = ui.painter_at(c);

        // Selected and hovered cards.
        if self.selected || resp.hovered() {
            painter.rect_filled(c.shrink(6.0), CornerRadius::same(8), if self.selected { t.accent_soft } else { t.hover });
        }
        if self.selected {
            painter.rect_stroke(c.shrink(6.0), CornerRadius::same(8), Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
        }

        // The page, fitted in the paper box (top-aligned, room above for the stack).
        let page = f.first_page();
        // The page's own size, when it is known and is one.
        let known = page.as_ref().ok().and_then(|p| f.sizes.get(*p).copied()).filter(|(w, h)| paper_size(*w, *h, self.fit).is_some());
        let size_pt = known.unwrap_or(FALLBACK_PAGE);
        let size = paper_size(size_pt.0, size_pt.1, self.fit).unwrap_or(self.fit);
        let top = c.top() + ABOVE;
        let paper = Rect::from_min_size(pos2(c.center().x - size.x / 2.0 - 3.0, top + (self.fit.y - size.y)), size);
        // More than one page: sheets behind it.
        let takes = self.check.as_ref().map_or(f.pages, |k| *k);
        if takes > 1 {
            for d in [6.0, 3.0] {
                let sheet = paper.translate(vec2(d, -d));
                painter.rect_filled(sheet, CornerRadius::ZERO, Color32::WHITE);
                painter.rect_stroke(sheet, CornerRadius::ZERO, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
            }
        }
        painter.rect_filled(paper.translate(vec2(0.0, 1.5)), CornerRadius::same(1), t.page_shadow);
        painter.rect_filled(paper, CornerRadius::ZERO, Color32::WHITE);

        // What goes on the paper: the thumbnail, or why there isn't one.
        let mut why: Option<(&str, Color32, String)> = None;
        if let Some(lock) = f.lock {
            let text = match lock {
                Lock::Open => tl!("Password-protected"),
                Lock::Permissions => tl!("Its security settings don't allow copying pages"),
            };
            why = Some(("lock", ON_PAPER, text.to_owned()));
        } else if let Some(p) = &f.problem {
            why = Some(("circle-x", ERROR, p.clone()));
        } else {
            match page {
                Err(e) => why = Some(("circle-x", ERROR, e)),
                // The pages couldn't be counted, or the page's size is unknown (some passwords
                // open a file for combining but not for reading it here): nothing is drawn (the
                // engine decides at Combine), and the card says so rather than look blank.
                Ok(p) if p >= f.pages || known.is_none() => why = Some(("eye-off", ON_PAPER, tl!("No preview").to_owned())),
                Ok(p) => {
                    // Pixels per point: the size the paper is drawn at, on both sides, and never a
                    // side longer than a thumbnail's (a long, thin page fits by its long side).
                    let (w, h) = size_pt;
                    let scale = (size.x * self.ppp / w).min(size.y * self.ppp / h).min(MAX_SIDE / w.max(h));
                    let px = [w * scale, h * scale].map(|v| (v.max(1.0).ceil() as u32).div_ceil(SIZE_STEP).saturating_mul(SIZE_STEP));
                    let key = Key { source: Arc::as_ptr(&f.bytes) as usize, auth: f.auth, page: p, px };
                    let look = thumbs.look(f.id, key);
                    let mut want = false;
                    match look {
                        Look::Ready(tex) => {
                            painter.image(tex, paper, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                        }
                        Look::Failed(e) => why = Some(("triangle-alert", WARNING, crate::i18n::fmt(tl!("Preview unavailable: {e}"), &[("e", &e)]))),
                        Look::Pending(stand_in) => {
                            if let Some(tex) = stand_in {
                                painter.image(tex, paper, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                            }
                            want = true;
                        }
                    }
                    if want && scale.is_finite() && scale > 0.0 {
                        thumbs.wants.push(Want {
                            file: f.id,
                            key,
                            bytes: f.bytes.clone(),
                            password: f.password().map(Arc::from),
                            scale,
                            visible: ui.is_rect_visible(c),
                        });
                    }
                }
            }
        }
        if let Some((icon, colour, _)) = &why {
            icons::paint(ui, Rect::from_center_size(paper.center(), Vec2::splat(28.0)), icon, 28.0, *colour);
        }
        // For screen readers: the pages, and why no page is shown (otherwise only in the tooltip).
        let description = match &why {
            Some((_, _, reason)) => format!("{pages}. {reason}"),
            None => pages.clone(),
        };
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_selected(self.selected);
            node.set_description(description);
        });
        painter.rect_stroke(paper, CornerRadius::ZERO, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);

        // A warning that doesn't stop it being combined: a badge on the corner.
        let warned = !f.notes.is_empty() || self.twice;
        if warned && why.as_ref().is_none_or(|(icon, ..)| *icon == "eye-off") {
            let r = Rect::from_center_size(paper.right_top() + vec2(-2.0, 2.0), Vec2::splat(18.0));
            painter.circle_filled(r.center(), 10.0, t.card);
            icons::paint(ui, r, "triangle-alert", 15.0, WARNING);
        }

        // The name, shortened in the middle to fit, and the pages taken when a range is set.
        let font = theme::medium(12.5);
        let max_w = c.width() - 20.0;
        let fits = |s: &str| painter.layout_no_wrap(crate::bidi::visual(s).into_owned(), font.clone(), t.text).size().x <= max_w;
        let shown = crate::bidi::visual(&fit_name(&f.name, fits)).into_owned();
        let name_y = top + self.fit.y + 16.0;
        painter.text(pos2(c.center().x, name_y), Align2::CENTER_CENTER, shown, font, t.text);
        let note = if !f.range.trim().is_empty() {
            match &self.check {
                Ok(k) => Some((crate::i18n::fmt(tl!("Pages: {k} of {n}"), &[("k", &k.to_string()), ("n", &f.pages.to_string())]), t.text_muted)),
                Err(_) => Some((tl!("Check the pages").to_owned(), ERROR)),
            }
        } else {
            None
        };
        if let Some((text, colour)) = note {
            painter.text(pos2(c.center().x, name_y + 17.0), Align2::CENTER_CENTER, text, theme::regular(11.5), colour);
        }

        // On hover, the card's own actions along the top of the page (not while a card is
        // dragged): remove this file only, and show it large when there is a page to show. Drawn
        // over the card, so a click on one doesn't select or move it.
        let previewable = why.is_none();
        // Nothing of the card shows over a dialog (the preview, the password box).
        let covered = ui.ctx().memory(|m| m.top_modal_layer().is_some());
        let mut over_bar = false;
        if !covered && ui.rect_contains_pointer(c.shrink(4.0)) && !egui::DragAndDrop::has_any_payload(ui.ctx()) {
            let count = if previewable { 2.0 } else { 1.0 };
            // Mostly above the page (a quarter over its top edge), so it hides little of it, and
            // never the name below.
            let y = paper.top() + BAR_H / 4.0 - BAR_H / 2.0;
            let bar = Rect::from_center_size(pos2(paper.center().x, y), vec2(count * 26.0 + (count - 1.0) * 2.0 + 8.0, BAR_H));
            over_bar = ui.rect_contains_pointer(bar);
            painter.rect(bar, CornerRadius::same(8), t.card, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
            let mut bar_ui = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink(4.0)).layout(egui::Layout::left_to_right(egui::Align::Center)));
            bar_ui.spacing_mut().item_spacing.x = 2.0;
            let remove = crate::i18n::fmt(tl!("Remove {name}"), &[("name", &f.name)]);
            if icons::button(&mut bar_ui, "trash-2", 26.0, false, &remove).clicked() {
                action = Some(RowAction::RemoveFile(f.id));
            }
            if previewable && icons::button(&mut bar_ui, "search", 26.0, false, tl!("Preview")).clicked() {
                action = Some(RowAction::Preview(f.id));
            }
        }

        // The full name and everything known about the file, on hover.
        let mut tip = format!("{}\n{pages} · {}", f.name, crate::panels::human_size(f.bytes.len()));
        for line in why.iter().map(|(_, _, s)| s).chain(&f.notes) {
            tip.push('\n');
            tip.push_str(line);
        }
        if self.twice {
            tip.push('\n');
            tip.push_str(tl!("Added more than once"));
        }
        // Page ranges are typed in the list.
        if self.check.is_err() {
            tip.push('\n');
            tip.push_str(tl!("Switch to List view to change the pages it takes."));
        }
        // (Over the bar, its buttons say what they do instead.)
        let resp = if covered || over_bar { resp } else { resp.on_hover_text(tip) };

        // The same actions on a right click (and Unlock… for a locked file).
        let at = resp.rect.left_bottom();
        let (file, locked) = (f.id, f.lock.is_some());
        resp.context_menu(|ui| {
            let item = |ui: &mut egui::Ui, icon: &str, label: &str| {
                ui.add(egui::Button::image_and_text(icons::image(icon, 15.0, ui.visuals().text_color()), label)).clicked()
            };
            if previewable && item(ui, "search", tl!("Preview")) {
                action = Some(RowAction::Preview(file));
                ui.close();
            }
            // This file only, as its trash button, and named so.
            if item(ui, "trash-2", &crate::i18n::fmt(tl!("Remove {name}"), &[("name", &name)])) {
                action = Some(RowAction::RemoveFile(file));
                ui.close();
            }
            if locked && item(ui, "lock-open", tl!("Unlock…")) {
                action = Some(RowAction::Unlock(Some(self.i), at));
                ui.close();
            }
        });
        if resp.drag_started() {
            egui::DragAndDrop::set_payload(ui.ctx(), GridDrag { file: f.id, revision: self.revision });
        }
        if resp.clicked() {
            action = Some(RowAction::Click(self.i, resp.ctx.input(|i| i.modifiers)));
        }
        action
    }
}

/// The file a card's magnifier opened, shown large over the page: one of the pages it adds at a
/// time, flipped with the buttons or the Left and Right keys; Esc, the close button or a click
/// outside closes it. It closes by itself when the file leaves the list or has nothing to show.
pub(crate) fn preview(app: &mut PdfCraftApp, ctx: &egui::Context, t: &Tokens) {
    // The command palette goes over everything: the preview waits under it (drawn again after).
    if app.palette_open {
        return;
    }
    let Some(p) = app.combine_tab.preview.as_mut() else { return };
    let Some(f) = app.combine_draft.iter().find(|f| f.id == p.file).filter(|f| f.lock.is_none() && f.problem.is_none()) else {
        app.combine_tab.preview = None;
        return;
    };
    // The pages it adds, worked out again only when its range changes.
    let pages = match &p.pages {
        Some(((range, count), pages)) if *range == f.range && *count == f.pages => pages.clone(),
        _ => match f.pages_taken() {
            Ok(pages) => {
                let pages: Arc<[usize]> = Arc::from(pages);
                p.pages = Some(((f.range.clone(), f.pages), pages.clone()));
                pages
            }
            Err(_) => {
                app.combine_tab.preview = None;
                return;
            }
        },
    };
    let n = pages.len();
    p.at = p.at.min(n.saturating_sub(1));
    let Some(&page) = pages.get(p.at) else {
        app.combine_tab.preview = None;
        return;
    };
    p.page = Some(page);
    let at = p.at;
    // Flipped with the keys (the grid's arrows wait while this shows), unless a text field has
    // them.
    let typing = ctx.text_edit_focused();
    let (back, on) = ctx.input_mut(|i| {
        for k in [egui::Key::Plus, egui::Key::Equals, egui::Key::Minus, egui::Key::Num0] {
            i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, k));
        }
        if typing {
            return (false, false);
        }
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) | i.consume_key(egui::Modifiers::NONE, egui::Key::PageUp),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) | i.consume_key(egui::Modifiers::NONE, egui::Key::PageDown),
        )
    });
    let (mut step, mut close) = (i64::from(on) - i64::from(back), false);
    let screen = ctx.content_rect();
    let width = (screen.width() * 0.86).max(320.0);
    let body_h = (screen.height() * 0.86 - 110.0).max(200.0);
    let ppp = ctx.pixels_per_point();
    // No larger than the GPU takes, or the render would be refused.
    let longest = ctx.input(|i| i.max_texture_side as f32).min(PREVIEW_SIDE);
    let thumbs = &mut app.combine_thumbs;
    let modal = egui::Modal::new(egui::Id::new("combine-preview")).show(ctx, |ui| {
        ui.set_width(width);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(crate::bidi::visual(&f.name)).font(theme::semibold(15.0)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::button(ui, "x", 30.0, false, tl!("Close")).clicked() {
                    close = true;
                }
                let of = crate::i18n::fmt(tl!("Page {p} of {n}"), &[("p", &(at + 1).to_string()), ("n", &n.to_string())]);
                ui.label(egui::RichText::new(of).color(t.text_muted));
            });
        });
        ui.add_space(8.0);
        // The page, as large as fits.
        let (area, _) = ui.allocate_exact_size(vec2(width, body_h), Sense::hover());
        let known = f.sizes.get(page).copied().filter(|(w, h)| paper_size(*w, *h, area.size()).is_some());
        let (w, h) = known.unwrap_or(FALLBACK_PAGE);
        let size = paper_size(w, h, area.size() - vec2(8.0, 8.0)).unwrap_or(area.size());
        let paper = Rect::from_center_size(area.center(), size);
        let painter = ui.painter_at(area);
        painter.rect_filled(paper.translate(vec2(0.0, 2.0)), CornerRadius::same(1), t.page_shadow);
        painter.rect_filled(paper, CornerRadius::ZERO, Color32::WHITE);
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        if known.is_some() {
            let scale = (size.x * ppp / w).min(size.y * ppp / h).min(longest / w.max(h));
            let px = [w * scale, h * scale].map(|v| (v.max(1.0).ceil() as u32).div_ceil(SIZE_STEP).saturating_mul(SIZE_STEP));
            let key = Key { source: Arc::as_ptr(&f.bytes) as usize, auth: f.auth, page, px };
            match thumbs.look(PREVIEW, key) {
                Look::Ready(tex) => {
                    painter.image(tex, paper, uv, Color32::WHITE);
                }
                Look::Failed(e) => {
                    let text = crate::i18n::fmt(tl!("Preview unavailable: {e}"), &[("e", &e)]);
                    painter.text(paper.center(), Align2::CENTER_CENTER, text, theme::regular(13.0), ON_PAPER);
                }
                Look::Pending(stand_in) => {
                    // A smaller render of this page stands in: the last size shown, or its card's.
                    let card = thumbs.entries.get(&f.id).filter(|e| e.key.page == page && e.key.auth == f.auth).and_then(|e| e.tex.as_ref());
                    if let Some(tex) = stand_in.or(card.map(TextureHandle::id)) {
                        painter.image(tex, paper, uv, Color32::WHITE);
                    }
                    if scale.is_finite() && scale > 0.0 {
                        thumbs.wants.push(Want {
                            file: PREVIEW,
                            key,
                            bytes: f.bytes.clone(),
                            password: f.password().map(Arc::from),
                            scale,
                            visible: true,
                        });
                    }
                }
            }
        } else {
            icons::paint(ui, Rect::from_center_size(paper.center(), Vec2::splat(36.0)), "eye-off", 36.0, ON_PAPER);
        }
        painter.rect_stroke(paper, CornerRadius::ZERO, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
        ui.add_space(8.0);
        // Previous and next, centred below.
        ui.horizontal(|ui| {
            ui.add_space((width - 2.0 * 30.0 - 6.0) / 2.0);
            if ui.add_enabled_ui(at > 0, |ui| icons::button(ui, "chevron-left", 30.0, false, tl!("Previous page"))).inner.clicked() {
                step = -1;
            }
            if ui.add_enabled_ui(at + 1 < n, |ui| icons::button(ui, "chevron-right", 30.0, false, tl!("Next page"))).inner.clicked() {
                step = 1;
            }
        });
    });
    if close || modal.should_close() {
        app.combine_tab.preview = None;
    } else if let Some(p) = app.combine_tab.preview.as_mut() {
        p.at = match step {
            s if s < 0 => p.at.saturating_sub(1),
            s if s > 0 => (p.at + 1).min(n.saturating_sub(1)),
            _ => p.at,
        };
    }
}

impl PdfCraftApp {
    /// The file shown large, the page on screen (0-based, in the file) and its render, while it
    /// shows (tests and the control channel).
    pub fn combine_preview(&self) -> Option<(String, usize, ThumbState)> {
        let p = self.combine_tab.preview.as_ref()?;
        let f = self.combine_draft.iter().find(|f| f.id == p.file)?;
        Some((f.name.clone(), p.page?, self.combine_thumbs.state(PREVIEW)))
    }

    /// Size the Combine grid's cards (1.0 = the usual); out-of-range values are clamped and
    /// nonsense is ignored.
    pub fn set_combine_zoom(&mut self, zoom: f32) {
        if zoom.is_finite() {
            self.combine_zoom = zoom.clamp(*ZOOM_RANGE.start(), *ZOOM_RANGE.end());
        }
    }

    /// Every frame, whatever shows: take finished thumbnails, start those the grid asked for (only
    /// while it is drawn), and let go of what no longer matches the list, so renders under way
    /// finish and release their threads in the list view, on Home or in a document tab too.
    pub(crate) fn combine_thumbs_frame(&mut self, ctx: &egui::Context) {
        // What each file's thumbnail must match now (cards out of view included).
        let current: HashMap<u64, Key> = self
            .combine_draft
            .iter_mut()
            .filter_map(|f| {
                let page = f.first_page().ok()?;
                Some((f.id, Key { source: Arc::as_ptr(&f.bytes) as usize, auth: f.auth, page, px: [0, 0] }))
            })
            .collect();
        // The page shown large, while it shows: leaving the tab closes it.
        if !self.combine_showing() {
            self.combine_tab.preview = None;
        }
        let mut current = current;
        if let Some(p) = &self.combine_tab.preview
            && let Some(page) = p.page
            && let Some(f) = self.combine_draft.iter().find(|f| f.id == p.file)
        {
            current.insert(PREVIEW, Key { source: Arc::as_ptr(&f.bytes) as usize, auth: f.auth, page, px: [0, 0] });
        }
        self.combine_thumbs.finish(ctx, &current);
    }

    /// Each listed file's grid thumbnail, in list order (tests and the control channel).
    pub fn combine_thumbnails(&self) -> Vec<ThumbState> {
        self.combine_draft.iter().map(|f| self.combine_thumbs.state(f.id)).collect()
    }

    /// Thumbnail renders under way (tests: never more than a few).
    pub fn combine_thumbnails_rendering(&self) -> usize {
        self.combine_thumbs.jobs.len()
    }

    /// Render threads for thumbnails that exist now (counted from the threads themselves),
    /// including those of finished or abandoned renders that haven't exited yet (tests: never
    /// more than three).
    pub fn combine_thumbnail_threads(&mut self) -> usize {
        self.combine_thumbs.threads()
    }

    /// Whether no thumbnail render is under way and every render thread has exited (tests).
    pub fn combine_thumbnail_threads_exited(&self) -> bool {
        self.combine_thumbs.jobs.is_empty() && self.combine_thumbs.retiring.iter().all(Retiring::exited)
    }

    /// Thumbnail textures held (tests: none once the tab is closed).
    pub fn combine_thumbnail_textures(&self) -> usize {
        self.combine_thumbs.entries.values().filter(|e| e.tex.is_some()).count()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::combine_ui::{moved_order, step_to};

    #[test]
    fn long_names_keep_their_extension_and_whole_characters() {
        let fits = |n: usize| move |s: &str| s.chars().count() <= n;
        assert_eq!(fit_name("short.pdf", fits(20)), "short.pdf");
        assert_eq!(fit_name("# 227 Pilot Boat Advance.pdf", fits(18)), "# 227 Pilot B….pdf");
        // Multi-byte characters are never cut in half.
        assert_eq!(fit_name("Überweisungsbestätigung.pdf", fits(12)), "Überwei….pdf");
        assert_eq!(fit_name("日本語のファイル名です.pdf", fits(8)), "日本語….pdf");
        // No extension, or a dot that isn't one.
        assert_eq!(fit_name("README-without-extension", fits(10)), "README-wi…");
        assert_eq!(fit_name(".hidden-file-name", fits(8)), ".hidden…");
        // Nothing fits: the ellipsis and the extension.
        assert_eq!(fit_name("abcdef.pdf", fits(0)), "….pdf");
    }

    #[test]
    fn paper_sizes_fit_the_box_and_bad_sizes_are_refused() {
        // The box follows the zoom, within its range; nonsense is 100%.
        assert_eq!(paper_box(1.0), PAPER);
        assert_eq!(paper_box(2.0), PAPER * 2.0);
        assert_eq!(paper_box(99.0), PAPER * 2.0);
        assert_eq!(paper_box(-1.0), PAPER * 0.6);
        assert_eq!(paper_box(f32::NAN), PAPER);
        for zoom in [0.6, 1.0, 2.0] {
            let fit = paper_box(zoom);
            // Letter is a little wider than the box: it fills the width. A tall page fills the height.
            let letter = paper_size(612.0, 792.0, fit).unwrap();
            assert!((letter.x - fit.x).abs() < 0.01 && letter.y <= fit.y + 0.01, "{letter:?}");
            let tall = paper_size(612.0, 1008.0, fit).unwrap();
            assert!((tall.y - fit.y).abs() < 0.01 && tall.x <= fit.x + 0.01, "{tall:?}");
            let landscape = paper_size(792.0, 612.0, fit).unwrap();
            assert!((landscape.x - fit.x).abs() < 0.01 && landscape.y < fit.y, "{landscape:?}");
            for (w, h) in [(0.0, 792.0), (612.0, -1.0), (f32::NAN, 1.0), (f32::INFINITY, 1.0), (1e-30, 1e30)] {
                let s = paper_size(w, h, fit);
                assert!(s.is_none_or(|s| s.x.is_finite() && s.y.is_finite() && s.x <= fit.x + 0.01 && s.y <= fit.y + 0.01), "{w}×{h}: {s:?}");
            }
        }
    }

    #[test]
    fn zoom_buttons_step_through_round_sizes() {
        let mut z = 0.6;
        let mut up = vec![z];
        while z < 2.0 {
            z = zoom_step(z, true);
            up.push(z);
        }
        assert_eq!(up, ZOOM_STEPS);
        assert_eq!(zoom_step(2.0, true), 2.0, "the largest stays put");
        assert_eq!(zoom_step(0.6, false), 0.6, "the smallest stays put");
        // From anywhere the slider left it, to the next round size.
        assert_eq!(zoom_step(1.1, true), 1.25);
        assert_eq!(zoom_step(1.1, false), 1.0);
        assert_eq!(zoom_step(1.953, true), 2.0);
        assert_eq!(zoom_step(1.0000001, true), 1.25, "a hair over a size counts as that size");
    }

    #[test]
    fn arrow_keys_step_by_file_and_by_row() {
        // The list: one at a time, staying put at both ends.
        assert_eq!(step_to(0, 5, 1, false), 0);
        assert_eq!(step_to(4, 5, 1, true), 4);
        assert_eq!(step_to(2, 5, 1, true), 3);
        // A grid of 3 per row and 8 files: rows [0 1 2] [3 4 5] [6 7].
        assert_eq!(step_to(1, 8, 3, true), 4, "down one row, same column");
        assert_eq!(step_to(5, 8, 3, true), 7, "a short last row is reached at its last file");
        assert_eq!(step_to(6, 8, 3, true), 6, "the last row goes no further down");
        assert_eq!(step_to(2, 8, 3, false), 2, "the first row goes no further up");
        assert_eq!(step_to(7, 8, 3, false), 4);
        // Out-of-range starts and empty lists don't panic.
        assert_eq!(step_to(99, 8, 3, false), 4);
        assert_eq!(step_to(0, 0, 3, true), 0);
        assert_eq!(step_to(0, 1, 0, true), 0);
    }

    #[test]
    fn moves_to_a_gap_keep_order_and_report_no_ops() {
        let ids = [10, 11, 12, 13, 14];
        let set = |v: &[u64]| v.iter().copied().collect::<BTreeSet<u64>>();
        // One file to the start, to the end, and between others.
        assert_eq!(moved_order(&ids, &set(&[13]), 0), Some(vec![13, 10, 11, 12, 14]));
        assert_eq!(moved_order(&ids, &set(&[10]), 5), Some(vec![11, 12, 13, 14, 10]));
        assert_eq!(moved_order(&ids, &set(&[10]), 3), Some(vec![11, 12, 10, 13, 14]));
        // A discontiguous selection gathers at the gap, in list order.
        assert_eq!(moved_order(&ids, &set(&[10, 12, 14]), 2), Some(vec![11, 10, 12, 14, 13]));
        assert_eq!(moved_order(&ids, &set(&[11, 13]), 5), Some(vec![10, 12, 14, 11, 13]));
        // Dropping a file next to itself, or a block inside itself, changes nothing.
        assert_eq!(moved_order(&ids, &set(&[12]), 2), None);
        assert_eq!(moved_order(&ids, &set(&[12]), 3), None);
        assert_eq!(moved_order(&ids, &set(&[11, 12, 13]), 2), None);
        // Nothing to move, unknown files and gaps past the end are harmless.
        assert_eq!(moved_order(&ids, &set(&[]), 1), None);
        assert_eq!(moved_order(&ids, &set(&[99]), 1), None);
        assert_eq!(moved_order(&ids, &set(&[10]), 99), Some(vec![11, 12, 13, 14, 10]));
    }

    /// A PDF of `pages` pages of `w` × `h` points.
    fn pdf(pages: usize, w: u32, h: u32) -> Arc<Vec<u8>> {
        let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
        let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 3 + 2 * i)).collect();
        objs.push(format!("<< /Type /Pages /Kids [{}] /Count {pages} /MediaBox [0 0 {w} {h}] >>", kids.join(" ")));
        for i in 0..pages {
            objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R >>", 4 + 2 * i));
            let body = "0.2 0.4 0.8 rg 0 0 10 10 re f";
            objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
        }
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        Arc::new(out)
    }

    fn key_of(bytes: &Arc<Vec<u8>>) -> Key {
        Key { source: Arc::as_ptr(bytes) as usize, auth: 1, page: 0, px: [32, 48] }
    }

    fn want(file: u64, bytes: &Arc<Vec<u8>>) -> Want {
        Want { file, key: key_of(bytes), bytes: bytes.clone(), password: None, scale: 0.15, visible: true }
    }

    fn listing(files: &[(u64, &Arc<Vec<u8>>)]) -> HashMap<u64, Key> {
        files.iter().map(|(f, b)| (*f, key_of(b))).collect()
    }

    /// One frame of `finish`, as the app runs it; how soon it asks to be drawn again.
    fn frame(ctx: &egui::Context, thumbs: &mut Thumbs, current: &HashMap<u64, Key>) -> std::time::Duration {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| thumbs.finish(ui.ctx(), current));
        // No GPU here: the texture uploads are taken as done.
        out.textures_delta.clear();
        out.viewport_output.get(&egui::ViewportId::ROOT).map_or(std::time::Duration::MAX, |v| v.repaint_delay)
    }

    #[test]
    fn a_thumbnail_that_arrives_is_drawn_without_waiting_for_input() {
        // Regression: the cards are drawn before results are taken; the last result used to
        // leave its card blank until the next mouse move.
        let ctx = egui::Context::default();
        let mut thumbs = Thumbs::default();
        let bytes = pdf(1, 200, 300);
        let current = listing(&[(1, &bytes)]);
        thumbs.wants.push(want(1, &bytes));
        frame(&ctx, &mut thumbs, &current);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let delay = frame(&ctx, &mut thumbs, &current);
            if thumbs.entries.get(&1).is_some_and(|e| e.tex.is_some()) {
                assert_eq!(delay, std::time::Duration::ZERO, "the frame that took the result asks for another");
                break;
            }
            assert!(std::time::Instant::now() < deadline, "the thumbnail arrives");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn a_result_for_a_file_that_changed_is_never_shown() {
        let ctx = egui::Context::default();
        let mut thumbs = Thumbs::default();
        let bytes = pdf(1, 200, 300);
        thumbs.wants.push(want(1, &bytes));
        frame(&ctx, &mut thumbs, &listing(&[(1, &bytes)]));
        assert_eq!(thumbs.jobs.len(), 1, "the render started");
        // The file leaves the list (or is unlocked differently) while it renders.
        let other = pdf(1, 200, 300);
        let changed = listing(&[(1, &other)]);
        frame(&ctx, &mut thumbs, &changed);
        assert!(thumbs.jobs.is_empty(), "let go of at once");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while thumbs.threads() > 0 {
            assert!(std::time::Instant::now() < deadline, "its thread exits");
            frame(&ctx, &mut thumbs, &changed);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(thumbs.entries.is_empty(), "its result is never kept");
    }

    #[test]
    fn textures_over_budget_go_longest_unseen_first_and_those_in_view_stay() {
        let ctx = egui::Context::default();
        let mut thumbs = Thumbs::default();
        let tex = |n: &str| ctx.load_texture(n, egui::ColorImage::new([1, 1], vec![Color32::WHITE]), egui::TextureOptions::LINEAR);
        let files: Vec<Arc<Vec<u8>>> = (0..4).map(|_| pdf(1, 200, 300)).collect();
        thumbs.frame = 10;
        // Seen at frames 1, 5 and 9 (out of view now), and 10 (in view), 20 MB each as counted.
        for (i, seen) in [1u64, 5, 9, 10].into_iter().enumerate() {
            let key = key_of(&files[i]);
            thumbs.entries.insert(i as u64, Entry { key, tex: Some(tex(&format!("t{i}"))), error: None, seen, bytes: 20 << 20 });
        }
        let current: HashMap<u64, Key> = files.iter().enumerate().map(|(i, b)| (i as u64, key_of(b))).collect();
        frame(&ctx, &mut thumbs, &current);
        let mut kept: Vec<u64> = thumbs.entries.keys().copied().collect();
        kept.sort_unstable();
        assert_eq!(kept, [3], "80 MB: the three out of view go, oldest first; the one in view stays even over budget");
    }

    #[test]
    fn render_threads_never_exceed_three_while_files_come_and_go() {
        let ctx = egui::Context::default();
        let mut thumbs = Thumbs::default();
        let files: Vec<Arc<Vec<u8>>> = (0..12).map(|i| pdf(1 + i % 3, 200, 300)).collect();
        let mut most = 0;
        for round in 0..120u64 {
            // A different handful of files every few frames: renders are abandoned all the time.
            let start = (round / 3 % 4 * 3) as usize;
            let shown: Vec<(u64, &Arc<Vec<u8>>)> = (start..start + 4).map(|i| (i as u64, &files[i % files.len()])).collect();
            for (file, bytes) in &shown {
                thumbs.wants.push(want(*file, bytes));
            }
            frame(&ctx, &mut thumbs, &listing(&shown));
            let threads = thumbs.threads();
            most = most.max(threads);
            assert!(threads <= MAX_THREADS, "round {round}: {threads} render threads");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(most > 0, "renders did run");
    }

    #[test]
    fn on_the_web_one_thumbnail_waits_at_a_time() {
        // Regression: pools that render on the UI thread have no threads to count, so three
        // started every frame while one finished, each holding its parsed document.
        let ctx = egui::Context::default();
        let mut thumbs = Thumbs { inline: true, ..Default::default() };
        let files: Vec<Arc<Vec<u8>>> = (0..6).map(|_| pdf(1, 200, 300)).collect();
        let shown: Vec<(u64, &Arc<Vec<u8>>)> = files.iter().enumerate().map(|(i, b)| (i as u64, b)).collect();
        // One heavy step a frame on the UI thread: a parse, then the next frame its render.
        for round in 0..14 {
            for (file, bytes) in &shown {
                thumbs.wants.push(want(*file, bytes));
            }
            frame(&ctx, &mut thumbs, &listing(&shown));
            assert!(thumbs.jobs.len() <= 1, "round {round}: {} renders waiting", thumbs.jobs.len());
            assert!(thumbs.jobs.iter().all(|j| j.pool.is_inline()));
        }
        assert!(thumbs.entries.values().filter(|e| e.tex.is_some()).count() == 6, "all six drawn, one heavy step per frame");
    }

    #[test]
    fn a_page_that_failed_is_not_tried_again_at_another_size() {
        // Regression: each zoom step asked for a new render of a page that had failed (or hung
        // for the watchdog's 20 s), filling the render threads.
        // (A card asks for a render only when it looks Pending.)
        let mut thumbs = Thumbs::default();
        let bytes = pdf(1, 200, 300);
        let key = key_of(&bytes);
        thumbs.entries.insert(1, Entry { key, tex: None, error: Some("broken".into()), seen: 0, bytes: 0 });
        let larger = Key { px: [key.px[0] + 64, key.px[1] + 64], ..key };
        assert!(matches!(thumbs.look(1, larger), Look::Failed(e) if e == "broken"), "the failure stands at another size");
        // Another read of the file (a new password) is a new page: tried again.
        let reread = Key { auth: 2, ..key };
        assert!(matches!(thumbs.look(1, reread), Look::Pending(None)));
    }

    #[test]
    fn on_the_web_a_render_that_never_answers_fails_and_lets_the_rest_through() {
        // A pool without threads answers on the call that renders: no answer then never comes.
        // It used to wait for good, so nothing else started and every frame asked for another.
        let ctx = egui::Context::default();
        let mut thumbs = Thumbs { inline: true, ..Default::default() };
        let (silent, next) = (pdf(1, 200, 300), pdf(1, 200, 300));
        let pool = RenderPool::new_inline(silent.clone(), RenderConfig::default());
        // Nothing queued: this pool has nothing to answer with.
        thumbs.jobs.push(Job { file: 1, key: key_of(&silent), pool, tag: 7 });
        let current = listing(&[(1, &silent), (2, &next)]);
        for _ in 0..4 {
            thumbs.wants.push(want(2, &next));
            frame(&ctx, &mut thumbs, &current);
        }
        assert!(thumbs.entries.get(&1).is_some_and(|e| e.tex.is_none() && e.error.is_some()), "the silent one failed");
        assert!(thumbs.entries.get(&2).is_some_and(|e| e.tex.is_some()), "the next one was drawn");
        assert!(thumbs.jobs.is_empty());
        assert!(frame(&ctx, &mut thumbs, &current) > std::time::Duration::ZERO, "no more frames asked for at once");
    }
}
