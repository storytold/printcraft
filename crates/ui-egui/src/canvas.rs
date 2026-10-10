//! The document area: page layout, zoom, render scheduling, overlays (links, annotation hovers,
//! form-field highlights), and the organize-pages grid.
//!
//! Page images come from the engine's render pool as whole-page rasters at the current zoom;
//! stale rasters are shown stretched until the sharp one arrives (tiling comes in M3.3).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2, pos2, vec2};
use pdfcraft_engine::{DocId, Edit};
use pdfcraft_render::{DestView, DocInfo, LayerOp, LinkTarget, PageText, RenderPool, RenderRequest, RequestKind, Tile, device_pixels};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, QuickTool, RightPanel, comments, icons, widgets};

/// Logical pixels per PDF point at 100% (96 dpi, like browsers).
pub const PT: f32 = 96.0 / 72.0;
const GAP: f32 = 14.0;
const MARGIN: f32 = 28.0;
/// Horizontal gutter that keeps pages clear of the floating quick-action bar.
const SIDE: f32 = 70.0;
const THUMB_TAG: u64 = 1 << 63;
const TEXT_TAG: u64 = 1 << 62;
/// A page rendered for the organize grid at the size it is drawn there.
const GRID_TAG: u64 = 1 << 61;
/// The tag of a raster that is out of date (shown until its replacement arrives).
const STALE_TAG: u64 = u64::MAX;
/// Pages whose raster would exceed this many device pixels on a side are drawn in tiles.
const TILE_THRESHOLD: f32 = 4096.0;
const TILE: u32 = 1024;
/// Longest side of the backdrop drawn under tiles, and of a live page preview (signature drag).
pub(crate) const BASE_SIDE: f32 = 2048.0;
/// Print-preview rasters (distinct from thumbnails and from organize-grid renders).
const PRINT_TAG: u64 = 1 << 60;
/// Thumbnail slot before density scaling. The pages panel draws at most 150 logical points and
/// the organize grid about 146; when the byte budget allows, density reaches the screen so those
/// images are sampled down instead of stretched.
const THUMB_W: f32 = 240.0;
const THUMB_H: f32 = 320.0;
const THUMB_BYTES: usize = 24 * 1024 * 1024;
const PAGE_BYTES: usize = 128 * 1024 * 1024;
const TILE_BYTES: usize = 128 * 1024 * 1024;
const UPLOAD_BYTES_PER_FRAME: usize = 16 * 1024 * 1024;
const RESULTS_PER_FRAME: usize = 8;
/// A defensive ceiling for unusually large viewports / n-up previews.
const MAX_THUMB_DEMAND: usize = 256;
/// At most this many whole-page rasters are cached beyond the ones on screen.
const MAX_CACHED_PAGES: usize = 24;

fn rgba_bytes(width: usize, height: usize) -> usize {
    width.saturating_mul(height).saturating_mul(4)
}

fn texture_bytes(tex: &TextureHandle) -> usize {
    let [w, h] = tex.size();
    rgba_bytes(w, h)
}

/// Visible grid rows, plus one adjacent row in either direction. Work is independent of
/// document length, including after a large scroll jump.
pub(crate) fn thumbnail_rows(top: f32, bottom: f32, row_height: f32, rows: usize) -> Range<usize> {
    let first = (top.max(0.0) / row_height).floor() as usize;
    let end = (bottom.max(0.0) / row_height).ceil() as usize;
    first.saturating_sub(1).min(rows)..end.saturating_add(1).min(rows)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    Width,
    Page,
    Height,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageLayout {
    Continuous,
    TwoUp,
    Single,
}

impl PageLayout {
    /// Every page display, in the order the View menu, the rail's Page display button and
    /// Preferences list them.
    pub const ORDER: [Self; 3] = [Self::Continuous, Self::Single, Self::TwoUp];

    /// The name settings and view options use: `continuous`, `single` or `two-up`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Continuous => "continuous",
            Self::Single => "single",
            Self::TwoUp => "two-up",
        }
    }

    /// The layout an [`as_str`](Self::as_str) name stands for, in any case. `None` for
    /// anything else, so callers can reject a typo rather than switch layouts silently.
    pub fn try_parse(value: &str) -> Option<Self> {
        Self::ORDER.into_iter().find(|l| l.as_str().eq_ignore_ascii_case(value.trim()))
    }

    /// Rail icon for this layout.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Continuous => "arrow-up-down",
            Self::Single => "file-text",
            Self::TwoUp => "columns-2",
        }
    }

    /// Registry command that switches to this layout (`PdfCraftApp::execute`).
    pub fn command(self) -> &'static str {
        match self {
            Self::Continuous => "view.layout.continuous",
            Self::Single => "view.layout.single",
            Self::TwoUp => "view.layout.two_up",
        }
    }

    /// The layout a [`command`](Self::command) id switches to.
    pub fn from_command(id: &str) -> Option<Self> {
        Self::ORDER.into_iter().find(|l| l.command() == id)
    }

    /// Menu label: the label of [`command`](Self::command), which the catalogs translate.
    pub fn label(self) -> &'static str {
        match self {
            Self::Continuous => "Continuous scrolling",
            Self::Single => "Single page",
            Self::TwoUp => "Two-page view",
        }
    }
}

/// How a newly opened document is shown (Preferences ▸ Documents and view). A PDF that asks
/// for its own layout or zoom gets that instead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewDefaults {
    pub layout: PageLayout,
    /// [`Fit::Width`] or [`Fit::Page`], or [`Fit::None`] for a fixed `zoom`.
    pub fit: Fit,
    /// The zoom with [`Fit::None`] (1.0 = 100%).
    pub zoom: f32,
    /// Highlight existing fields: a preference in Acrobat, so it carries over to the next
    /// document and the next session.
    pub highlight_fields: bool,
}

impl Default for ViewDefaults {
    fn default() -> Self {
        Self { layout: PageLayout::Continuous, fit: Fit::Width, zoom: 1.0, highlight_fields: false }
    }
}

impl ViewDefaults {
    /// The zoom as settings and view options name it: `fit-width`, `fit-page` or a percentage.
    pub fn zoom_name(&self) -> String {
        match self.fit {
            Fit::Page => "fit-page".into(),
            Fit::None => format!("{}%", (self.zoom * 100.0).round()),
            Fit::Width | Fit::Height => "fit-width".into(),
        }
    }

    /// These defaults with the zoom a [`zoom_name`](Self::zoom_name) names, in any case. `None`
    /// for anything else, including a percentage outside the 8–6400% the view can show.
    pub fn with_zoom(self, name: &str) -> Option<Self> {
        let name = name.trim();
        let (fit, zoom) = if name.eq_ignore_ascii_case("fit-width") {
            (Fit::Width, self.zoom)
        } else if name.eq_ignore_ascii_case("fit-page") {
            (Fit::Page, self.zoom)
        } else {
            let percent = name.strip_suffix('%').unwrap_or(name).trim().parse::<f32>().ok().filter(|p| (8.0..=6400.0).contains(p))?;
            (Fit::None, percent / 100.0)
        };
        Some(Self { fit, zoom, ..self })
    }
}

/// The find bar (⌘F): query, matches across the document, current match.
#[derive(Default)]
pub struct Find {
    pub query: String,
    /// (page, glyph range) in document order.
    pub matches: Vec<(usize, Range<usize>)>,
    pub current: Option<usize>,
    pub focus: bool,
    pub case_query: String,
    /// Find options (Acrobat's: case-sensitive, whole words only).
    pub case_sensitive: bool,
    pub whole_words: bool,
    /// Shown in the Search panel (Advanced Search) instead of the find bar.
    pub in_panel: bool,
}

/// A text selection on one page.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Selection {
    page: usize,
    span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Span {
    /// Reading order, between two glyph indices.
    Run { anchor: usize, head: usize },
    /// Column select (#740): the glyphs inside a rectangle in page view space, drawn from the
    /// corner the drag started at (`rect[0..2]`) to the pointer (`rect[2..4]`).
    Column { rect: [f32; 4] },
}

impl Selection {
    fn run(page: usize, anchor: usize, head: usize) -> Self {
        Self { page, span: Span::Run { anchor, head } }
    }

    fn range(anchor: usize, head: usize) -> Range<usize> {
        anchor.min(head)..anchor.max(head).saturating_add(1)
    }

    /// One rectangle per selected piece of a line, in view space (highlighting, markup quads).
    fn rects(&self, text: &PageText) -> Vec<[f32; 4]> {
        match self.span {
            Span::Run { anchor, head } => text.line_rects(Self::range(anchor, head)),
            Span::Column { rect } => text.glyph_rects(&text.glyphs_in(rect)),
        }
    }

    /// The selected text, as Copy puts it on the clipboard.
    fn text(&self, text: &PageText) -> String {
        match self.span {
            Span::Run { anchor, head } => text.text_of(Self::range(anchor, head)),
            Span::Column { rect } => text.column_text(&text.glyphs_in(rect)),
        }
    }
}

struct PageTex {
    tag: u64,
    tex: TextureHandle,
}

/// Retained RGBA texture accounting for automation and performance regressions. These
/// surface bytes exclude driver overhead and temporary upload buffers; they are not RSS.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct RasterMemory {
    pub thumbnail_count: usize,
    pub thumbnail_bytes: usize,
    pub page_count: usize,
    pub page_bytes: usize,
    pub tile_count: usize,
    pub tile_bytes: usize,
    pub thumbnail_demand: usize,
    /// Last submitted missing results; workers may have completed some since submission.
    pub pending_requests: usize,
}

pub struct DocView {
    pub id: DocId,
    pub zoom: f32,
    pub fit: Fit,
    pub layout: PageLayout,
    /// View rotation in degrees clockwise (0, 90, 180, 270); display only, never saved.
    pub rotation: u16,
    pub current: usize,
    pub organize: bool,
    pub highlight_fields: bool,
    pub page_input: String,
    pub notice_dismissed: bool,
    /// Two-page view: show the first page alone, as a cover (View ▸ Page display).
    pub cover: bool,
    /// Previous view / Next view: pages visited before (and after, once going back).
    pub back: Vec<usize>,
    pub forward: Vec<usize>,
    /// Pending navigation: page and fraction down the page to align with the viewport top.
    pub goto: Option<(usize, f32)>,
    /// Pending position within that page from a destination ([`Self::go_to_dest`]): the point
    /// (fractions of the displayed page; `None` leaves that axis to `goto`) to put `rel` points
    /// from the viewport's top-left corner.
    goto_point: Option<(usize, Option<f32>, Option<f32>, Vec2)>,
    /// Pending keyboard scrolling, in points down (negative: up): ↓ / ↑, and Page Down /
    /// Page Up where the pages scroll.
    pub key_scroll: f32,
    /// Single-page view, as last drawn: the page shown and whether the view was at its top and
    /// at its bottom, so ↑ / ↓ turn the page there instead of doing nothing (#273).
    single_edges: Option<(usize, bool, bool)>,
    /// Briefly outline an annotation after navigating to it from a panel.
    pub flash: Option<(usize, [f32; 4], f64)>,
    /// Compare files: differences shaded on this document's pages (page, user-space box, colour).
    pub compare_marks: Vec<(usize, [f32; 4], Color32)>,
    pages: HashMap<usize, PageTex>,
    /// Pages the renderer could not draw, with the reason (never re-requested).
    errors: HashMap<usize, String>,
    /// When each still-missing page was first shown, to flag unusually slow renders.
    waiting_since: HashMap<usize, f64>,
    thumbs: HashMap<usize, PageTex>,
    /// This frame's consumers, with visible cells ahead of prefetch.
    thumb_demand: HashMap<usize, bool>,
    frame_queue: Vec<RenderRequest>,
    /// Pages on screen this frame. Their rasters and tiles are always admitted: the viewport
    /// bounds them, and the byte and count limits apply to prefetch only.
    frame_visible: HashSet<usize>,
    /// Thumbnails that are out of date (still shown until their replacement arrives).
    stale_thumbs: HashSet<usize>,
    /// Print dialog: the current sheet's pages at the preview pane's device resolution.
    print_pages: HashMap<usize, (u64, TextureHandle)>,
    /// Sharp tiles of large pages: (page, scale tag, tile x, tile y) → texture. Tiles of an
    /// earlier zoom stay, drawn stretched, until those of the current one cover the page.
    tiles: HashMap<(usize, u64, u32, u32), TextureHandle>,
    /// Text layers, extracted in the background on demand (selection, find, copy).
    pub(crate) texts: HashMap<usize, Arc<PageText>>,
    pub(crate) text_failed: HashSet<usize>,
    pub find: Option<Find>,
    selection: Option<Selection>,
    /// Clicks in the current run on the text layer: two select a word, three the line, four the page.
    clicks: u32,
    last_queue: Vec<RenderRequest>,
    viewport_w: f32,
    viewport_h: f32,
    page_count: usize,
    /// Page heights in points (view space), for mapping text positions to scroll offsets.
    page_heights: Vec<f32>,
    /// Screen rects of the pages drawn last frame (hit-testing, tests, automation).
    screen_rects: Vec<(usize, Rect)>,
    screen_xforms: Vec<(usize, PageXform)>,
    /// The scroll viewport on screen last frame.
    viewport_screen: Rect,
    /// Pending zoom anchor: page, position within it (0..1), and offset from the viewport corner.
    zoom_anchor: Option<(usize, f32, f32, Vec2)>,
    /// The page view has been shown, so its scroll offset in egui's memory is this document's.
    shown: bool,
    /// Turns wheel input into page turns in single-page view.
    wheel: crate::wheel_pager::WheelPager,
    /// Until this time, apply precise (touchpad) wheel input 1:1 and suppress egui's eased copy.
    /// A precision touchpad sends pixel-precise `MouseWheel` events; on Windows they arrive with
    /// no `TouchPhase::Start`, so egui eases deltas of 8 px or more and the page lags the finger
    /// (#759). The window outlasts a single frame so egui's internal easing tail is suppressed as
    /// it decays, instead of adding on top of the 1:1 motion.
    precise_scroll_until: f64,
    pub(crate) auto_scroll: crate::autoscroll::AutoScroll,
    /// Pages selected in the organize grid or the Pages panel (0-based). Empty means "the current page".
    pub selected: BTreeSet<usize>,
    /// Anchor for ⇧-click range selection in the organize grid.
    select_anchor: Option<usize>,
    /// An edit requested by the view (organize toolbar, keys), applied by the app this frame.
    pub pending_edit: Option<Edit>,
    /// Edit text: the lines per page (with the document generation they were read at), and the
    /// line being edited.
    pub(crate) edit_lines: HashMap<usize, (u64, Vec<pdfcraft_engine::TextBlock>)>,
    pub line_editor: Option<crate::edit_text_ui::LineEditor>,
    /// Edit text & images: the images per page (by document generation), and the selected one.
    pub(crate) edit_images: HashMap<usize, (u64, Vec<pdfcraft_engine::PageImage>)>,
    pub image_selection: Option<crate::edit_text_ui::ImageSelection>,
    /// A paragraph box being dragged (moved, or resized from its right edge) in Edit text.
    pub block_drag: Option<crate::edit_text_ui::BlockDrag>,
    /// Commenting state: selected comment, gestures, composer.
    pub comments: crate::comments::CommentView,
    pub measure: crate::measure_ui::MeasureView,
    /// Form filling state: the focused field.
    pub forms: crate::forms_ui::FormView,
    /// Prepare a form: the selected field and the gesture in progress.
    pub prepare: crate::prepare::PrepareView,
    /// Edit a PDF: the selected added item, the text being typed.
    pub content: crate::content_ui::ContentView,
    /// Edit a PDF ▸ Link tool state.
    pub links: crate::link_ui::LinkView,
    /// Redact tool: the box being drawn, and a mark to add (page, quads) for the app to style.
    pub redact_drag: crate::redact_ui::AreaDrag,
    pub pending_redaction: Option<(usize, Vec<[f64; 8]>)>,
    /// A crop rectangle being dragged (Crop tool).
    pub crop_drag: crate::crop::CropDrag,
    /// Use a certificate: a signature rectangle being drawn, or an empty signature field clicked.
    pub sign: crate::sign_ui::SignView,
    /// Organize: the pages being dragged to a new place.
    pub org_drag: Option<Vec<usize>>,
    /// Pages panel: the pages being dragged to a new place.
    pub panel_drag: Option<Vec<usize>>,
    /// Marquee Zoom / Snapshot: the rectangle being dragged (page, start), and a finished one.
    pub marquee: Option<(usize, Pos2)>,
    pub marquee_done: Option<crate::zoom_snap::Marquee>,
    /// Fill & Sign text being typed.
    pub fill_text: Option<crate::fill_sign::TypeBox>,
    /// A queued Fill & Sign signature: select it after its edit succeeds, then leave placement.
    pub(crate) fill_signature_page: Option<usize>,
    pub(crate) signature_drag: crate::signature_drag::SignatureDrag,
    /// A non-edit action requested by the organize toolbar, handled by the app.
    pub pending_action: Option<ViewAction>,
    /// The grid gap the next inserted files go to (set by a "+" between pages); otherwise they
    /// go after the selection.
    pub insert_at: Option<usize>,
    /// The page-grid gap under the pointer this frame (where dropped files go), when it can
    /// take pages.
    pub grid_gap: Option<usize>,
    /// How large the page grid draws its pages (1.0 = the usual thumbnails).
    grid_zoom: f32,
    /// A zoom asked for this frame (toolbar, pinch, keys); the grid applies it once it has laid
    /// out, so it can keep the same pages in view.
    grid_zoom_request: Option<f32>,
    /// The page to keep in place after a grid zoom, and how far below the top of the grid its
    /// cell was.
    grid_anchor: Option<(usize, f32)>,
    /// Sharp renders of the pages the grid shows larger than a thumbnail: only those in view,
    /// at the size drawn (device pixels wide).
    grid_pages: HashMap<usize, (u32, TextureHandle)>,
}

/// The page grid's zoom range and the step of its buttons and keys.
pub const GRID_ZOOM_RANGE: std::ops::RangeInclusive<f32> = 0.5..=3.0;
const GRID_ZOOM_STEP: f32 = 1.25;
/// How far a sharp grid render may be from the size drawn before it is redone: a pinch changes
/// the size on every frame, and a slightly soft or slightly large image is fine meanwhile.
const GRID_SHARP: std::ops::RangeInclusive<f32> = 0.9..=1.6;

/// Whether an image `have` device pixels wide is good enough to draw `want` pixels wide.
fn sharp_enough(have: u32, want: f32) -> bool {
    want > 0.0 && GRID_SHARP.contains(&(have as f32 / want))
}

/// Thumbnail device-pixel density when `demand` pages share the thumbnail allowance.
fn thumbnail_density(demand: usize, ppp: f32) -> f32 {
    let per_thumb = THUMB_BYTES / demand.max(1).next_power_of_two();
    ppp.clamp(0.25, 4.0).min((per_thumb as f32 / (4.0 * THUMB_W * THUMB_H)).sqrt() * 0.98)
}

fn thumbnail_scale(width: f32, height: f32, density: f32) -> f32 {
    (THUMB_W / width.max(1.0)).min(THUMB_H / height.max(1.0)) * density
}

/// Organize-toolbar actions that need the app (file pickers, new tabs, dialogs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewAction {
    InsertFromFile,
    /// Insert files at a gap of the page grid (0 = before the first page).
    InsertFromFileAt(usize),
    /// Save the document as the grid shows it.
    Save,
    Extract,
    Split,
    /// Copy the selected pages (Cut also deletes them).
    CopyPages {
        cut: bool,
    },
    /// Paste copied pages after the selection.
    PastePages,
}

impl DocView {
    pub fn raster_memory(&self) -> RasterMemory {
        RasterMemory {
            thumbnail_count: self.thumbs.len(),
            thumbnail_bytes: self.thumbs.values().fold(0usize, |n, p| n.saturating_add(texture_bytes(&p.tex))),
            page_count: self.pages.len(),
            page_bytes: self.pages.values().fold(0usize, |n, p| n.saturating_add(texture_bytes(&p.tex))),
            tile_count: self.tiles.len(),
            tile_bytes: self.tiles.values().fold(0usize, |n, tex| n.saturating_add(texture_bytes(tex))),
            thumbnail_demand: self.thumb_demand.len(),
            pending_requests: self.last_queue.len(),
        }
    }

    /// Where `page` is drawn on screen this frame (`None` when it is not on screen).
    pub fn page_screen_rect(&self, page: usize) -> Option<Rect> {
        self.screen_xforms.iter().find(|(p, _)| *p == page).map(|(_, xf)| xf.rect)
    }

    /// The document area on screen.
    pub fn viewport_rect(&self) -> Rect {
        self.viewport_screen
    }

    /// Whether middle-button auto-scroll is on, latched or held (tests and automation).
    pub fn auto_scrolling(&self) -> bool {
        self.auto_scroll.active()
    }

    /// Pages that could not be rendered, with the reason (for automation; 0-based pages).
    pub fn page_errors(&self) -> Vec<(usize, &str)> {
        let mut v: Vec<(usize, &str)> = self.errors.iter().map(|(p, e)| (*p, e.as_str())).collect();
        v.sort_unstable_by_key(|(p, _)| *p);
        v
    }

    /// A view of a newly opened document, shown as `defaults` say. Opening a file then applies
    /// any layout or zoom the PDF itself asks for.
    pub fn new(id: DocId, info: &DocInfo, defaults: ViewDefaults) -> Self {
        Self {
            id,
            zoom: defaults.zoom,
            fit: defaults.fit,
            layout: defaults.layout,
            rotation: 0,
            current: 0,
            organize: false,
            highlight_fields: defaults.highlight_fields,
            page_input: "1".into(),
            notice_dismissed: false,
            cover: false,
            back: Vec::new(),
            forward: Vec::new(),
            goto: None,
            goto_point: None,
            key_scroll: 0.0,
            single_edges: None,
            flash: None,
            compare_marks: Vec::new(),
            pages: HashMap::new(),
            errors: HashMap::new(),
            waiting_since: HashMap::new(),
            thumbs: HashMap::new(),
            thumb_demand: HashMap::new(),
            frame_queue: Vec::new(),
            frame_visible: HashSet::new(),
            stale_thumbs: HashSet::new(),
            print_pages: HashMap::new(),
            tiles: HashMap::new(),
            texts: HashMap::new(),
            text_failed: HashSet::new(),
            find: None,
            selection: None,
            clicks: 0,
            last_queue: Vec::new(),
            viewport_w: 800.0,
            viewport_h: 600.0,
            page_count: info.pages.len(),
            page_heights: info.pages.iter().map(|p| p.height).collect(),
            screen_rects: Vec::new(),
            screen_xforms: Vec::new(),
            viewport_screen: Rect::NOTHING,
            zoom_anchor: None,
            shown: false,
            wheel: Default::default(),
            precise_scroll_until: 0.0,
            auto_scroll: Default::default(),
            selected: BTreeSet::new(),
            select_anchor: None,
            pending_edit: None,
            edit_lines: HashMap::new(),
            line_editor: None,
            edit_images: HashMap::new(),
            image_selection: None,
            block_drag: None,
            pending_action: None,
            insert_at: None,
            grid_gap: None,
            grid_zoom: 1.0,
            grid_zoom_request: None,
            grid_anchor: None,
            grid_pages: HashMap::new(),
            comments: Default::default(),
            measure: Default::default(),
            forms: Default::default(),
            prepare: Default::default(),
            content: Default::default(),
            links: Default::default(),
            redact_drag: None,
            pending_redaction: None,
            crop_drag: None,
            sign: Default::default(),
            org_drag: None,
            panel_drag: None,
            marquee: None,
            marquee_done: None,
            fill_text: None,
            fill_signature_page: None,
            signature_drag: Default::default(),
        }
    }

    /// The document's content or page list changed (an edit, undo or redo): drop caches and
    /// adopt the new page geometry, keeping the reader's place where possible.
    pub fn document_changed(&mut self, info: &DocInfo) {
        self.invalidate_content();
        self.page_count = info.pages.len();
        self.page_heights = info.pages.iter().map(|p| p.height).collect();
        let last = self.page_count.saturating_sub(1);
        self.current = self.current.min(last);
        self.page_input = (self.current + 1).to_string();
        self.selected.retain(|p| *p <= last);
        // Text being typed on a page that no longer exists goes with the page.
        if self.content.draft.as_ref().is_some_and(|d| d.page > last || self.page_count == 0) {
            self.content.draft = None;
        }
        if self.select_anchor.is_some_and(|a| a > last) {
            self.select_anchor = None;
        }
        self.goto = None;
        self.goto_point = None;
        self.zoom_anchor = None;
        self.flash = None;
        // A thumbnail drag holds page indexes from before the change.
        self.panel_drag = None;
    }

    /// Pages an organize command acts on: the selection, or the current page.
    pub fn target_pages(&self) -> Vec<usize> {
        if self.selected.is_empty() { vec![self.current] } else { self.selected.iter().copied().collect() }
    }

    /// Select pages in the organize grid (tests, automation). Empty clears the selection.
    pub fn select_pages(&mut self, pages: &[usize]) {
        self.selected = pages.iter().copied().filter(|p| *p < self.page_count).collect();
        if let Some(first) = self.selected.first() {
            self.current = *first;
        }
    }

    /// A click on page `i`'s thumbnail, in the organize grid or the Pages panel. ⇧ selects the
    /// range from the anchor (or the current page); ⌘/Ctrl toggles the page; a plain click
    /// selects only it. `seed_current` is for the Pages panel, where an empty selection means
    /// the current page: the first ⌘-click on another page keeps the current one selected too.
    pub fn click_page(&mut self, i: usize, modifiers: egui::Modifiers, seed_current: bool) {
        if i >= self.page_count {
            return;
        }
        if modifiers.shift {
            let a = self.select_anchor.unwrap_or(self.current);
            self.selected = (a.min(i)..=a.max(i)).collect();
        } else if modifiers.command {
            if seed_current && self.selected.is_empty() && i != self.current {
                self.selected.insert(self.current);
            }
            if !self.selected.remove(&i) {
                self.selected.insert(i);
            }
            self.select_anchor = Some(i);
        } else {
            self.selected = [i].into();
            self.select_anchor = Some(i);
        }
    }

    /// Drop the page selection; a later ⇧-click ranges from `anchor`.
    pub fn clear_page_selection(&mut self, anchor: Option<usize>) {
        self.selected.clear();
        self.select_anchor = anchor.filter(|a| *a < self.page_count);
    }

    pub fn render_pending(&self) -> bool {
        !self.last_queue.is_empty()
    }

    /// Re-render every page and text layer (the document's appearance changed, e.g. a layer
    /// was toggled). Out-of-date rasters stay on screen until their replacements arrive, so the
    /// view never flashes blank.
    pub fn invalidate_content(&mut self) {
        for p in self.pages.values_mut() {
            p.tag = STALE_TAG;
        }
        self.stale_thumbs.extend(self.thumbs.keys().copied());
        for slot in self.print_pages.values_mut() {
            slot.0 = STALE_TAG;
        }
        // Pages may have moved: a sharp render of another page would be worse than a soft one.
        self.grid_pages.clear();
        self.tiles.clear();
        self.texts.clear();
        self.text_failed.clear();
        self.errors.clear();
        self.waiting_since.clear();
        self.last_queue.clear();
        self.selection = None;
        if let Some(f) = self.find.as_mut() {
            f.matches.clear();
            f.current = None;
        }
    }

    /// Only `page` changed (a comment was added, edited or removed): re-render that page and
    /// re-read its text, keep everything else.
    pub fn page_changed(&mut self, page: usize) {
        if let Some(p) = self.pages.get_mut(&page) {
            p.tag = STALE_TAG;
        }
        if self.thumbs.contains_key(&page) {
            self.stale_thumbs.insert(page);
        }
        if let Some(slot) = self.print_pages.get_mut(&page) {
            slot.0 = STALE_TAG;
        }
        self.tiles.retain(|(p, _, _, _), _| *p != page);
        self.texts.remove(&page);
        self.text_failed.remove(&page);
        self.errors.remove(&page);
        self.last_queue.clear();
        if self.selection.is_some_and(|s| s.page == page) {
            self.selection = None;
        }
        if let Some(f) = self.find.as_mut() {
            f.matches.retain(|(p, _)| *p != page);
            f.current = None;
        }
    }

    /// Pages drawn last frame and their screen rectangles.
    pub fn visible_page_rects(&self) -> Vec<(usize, Rect)> {
        self.screen_xforms.iter().map(|(p, xf)| (*p, xf.rect)).collect()
    }

    /// Where `page` is drawn this frame, with its coordinate mapping.
    pub(crate) fn page_xform(&self, page: usize) -> Option<PageXform> {
        self.screen_xforms.iter().find(|(p, _)| *p == page).map(|(_, xf)| *xf)
    }

    /// The text selection as markup quadrilaterals in user space: (page, quads).
    pub(crate) fn selection_quads(&self, info: &DocInfo) -> Option<(usize, Vec<[f64; 8]>)> {
        let s = self.selection?;
        let text = self.texts.get(&s.page)?;
        let page = info.pages.get(s.page)?;
        let quads: Vec<[f64; 8]> = s.rects(text).into_iter().map(|r| page.view_rect_to_quad(r)).collect();
        (!quads.is_empty()).then_some((s.page, quads))
    }

    /// A page's thumbnail texture, when rendered.
    pub(crate) fn thumb_id(&self, page: usize) -> Option<egui::TextureId> {
        self.thumbs.get(&page).map(|t| t.tex.id())
    }

    /// The print preview's picture of `page`: the sheet raster when it has arrived, otherwise the
    /// thumbnail.
    pub(crate) fn page_preview(&self, page: usize) -> Option<egui::TextureId> {
        self.print_pages.get(&page).map(|(_, tex)| tex.id()).or_else(|| self.thumb_id(page))
    }

    pub(crate) fn page_text(&self, page: usize) -> Option<Arc<PageText>> {
        self.texts.get(&page).cloned()
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Select text on `page` from glyph `from` to glyph `to` (tests and automation).
    pub fn select_text(&mut self, page: usize, from: usize, to: usize) {
        self.selection = Some(Selection::run(page, from, to));
    }

    /// Open the find bar (or focus it if already open).
    pub fn open_find(&mut self) {
        let f = self.find.get_or_insert_with(Find::default);
        f.focus = true;
    }

    /// Re-run the search on all pages whose text is known (after the query changed).
    pub fn rerun_find(&mut self) {
        let Some(f) = self.find.as_mut() else { return };
        f.case_query = f.query.clone();
        f.matches.clear();
        f.current = None;
        let pages: Vec<usize> = self.texts.keys().copied().collect();
        for p in pages {
            self.refresh_find_page(p);
        }
    }

    fn refresh_find_page(&mut self, page: usize) {
        let (Some(f), Some(text)) = (self.find.as_mut(), self.texts.get(&page)) else { return };
        if f.case_query.trim().is_empty() {
            return;
        }
        let current_key = f.current.and_then(|c| f.matches.get(c).cloned());
        f.matches.retain(|(p, _)| *p != page);
        f.matches.extend(text.find_opts(&f.case_query, f.case_sensitive, f.whole_words).into_iter().map(|r| (page, r)));
        f.matches.sort_by_key(|(p, r)| (*p, r.start));
        f.current = match current_key {
            Some(k) => f.matches.iter().position(|m| *m == k),
            None => None,
        };
        if f.current.is_none() && !f.matches.is_empty() {
            // First match at or after the page being viewed.
            let cur = self.current;
            let i = f.matches.iter().position(|(p, _)| *p >= cur).unwrap_or(0);
            f.current = Some(i);
            let (p, _) = f.matches[i];
            self.flash_match(p);
        }
    }

    /// Go to match `i` (the Search panel's results).
    pub fn go_to_match(&mut self, i: usize) {
        let Some(f) = self.find.as_mut() else { return };
        let Some(&(p, _)) = f.matches.get(i) else { return };
        f.current = Some(i);
        self.flash_match(p);
    }

    /// Move to the next/previous match.
    pub fn find_step(&mut self, forward: bool) {
        let Some(f) = self.find.as_mut() else { return };
        if f.matches.is_empty() {
            return;
        }
        let n = f.matches.len();
        let c = f.current.unwrap_or(0);
        let next = if forward { (c + 1) % n } else { (c + n - 1) % n };
        f.current = Some(next);
        let p = f.matches[next].0;
        self.flash_match(p);
    }

    fn flash_match(&mut self, page: usize) {
        let frac = self
            .find
            .as_ref()
            .and_then(|f| f.current.and_then(|c| f.matches.get(c)))
            .and_then(|(p, r)| self.texts.get(p).and_then(|t| t.glyphs.get(r.start)).map(|g| g.rect[1]))
            .zip(self.page_heights.get(page).copied())
            .map(|(y, h)| ((y - 60.0) / h).clamp(0.0, 1.0))
            .unwrap_or(0.0);
        self.goto = Some((page, frac));
        self.current = page;
    }

    /// Screen position of the centre of glyph `glyph` on `page`, if that page is on screen and its
    /// text layer is loaded (used by UI tests and automation to aim pointer input).
    pub fn glyph_screen_pos(&self, page: usize, glyph: usize) -> Option<Pos2> {
        let (_, xf) = self.screen_xforms.iter().find(|(p, _)| *p == page)?;
        let g = self.texts.get(&page)?.glyphs.get(glyph)?;
        Some(xf.view_rect(g.rect).center())
    }

    /// Selected text, if any (⌘C).
    pub fn selected_text(&self) -> Option<String> {
        let s = self.selection?;
        let t = self.texts.get(&s.page)?;
        Some(s.text(t)).filter(|x| !x.is_empty())
    }

    pub fn thumb(&self, page: usize) -> Option<&TextureHandle> {
        self.thumbs.get(&page).map(|t| &t.tex)
    }

    pub fn go_to_page(&mut self, page: usize) {
        let page = page.min(self.page_count.saturating_sub(1));
        if page != self.current {
            // The view history (Previous view / Next view).
            if self.back.last() != Some(&self.current) {
                self.back.push(self.current);
                if self.back.len() > 200 {
                    self.back.remove(0);
                }
            }
            self.forward.clear();
        }
        // Partial wheel motion belongs to the old place.
        self.wheel.clear_motion();
        self.goto = Some((page, 0.0));
        self.current = page;
        self.page_input = (page + 1).to_string();
    }

    /// Go to a destination, a bookmark's or a link's (ISO 32000-2 §12.3.2.2): its page, scrolled
    /// and zoomed as `dest` asks, as Acrobat does. `/XYZ` puts (left, top) at the window's
    /// top-left at its zoom (null or 0 keeps the zoom); `/Fit`, `/FitH` and `/FitV` switch to fit
    /// page, width and height with `top` or `left` at the window's edge; `/FitR` zooms so the
    /// rectangle fits, centred. The `B` (content box) forms are read as the page. A null
    /// coordinate keeps that scroll position (null `top` on a new page: its top), and a point
    /// past the end of the document scrolls as far as it can.
    pub fn go_to_dest(&mut self, page: usize, dest: DestView, info: &DocInfo) {
        self.go_to_page(page);
        let page = self.current;
        self.goto_point = None;
        let Some(p) = info.pages.get(page) else { return };
        let point = |x: Option<f32>, y: Option<f32>| p.dest_fraction(x, y, self.rotation);
        // The window's top-left as far as pages go: its left edge is the gutter that keeps them
        // clear of the floating quick-action bar, so `left` 0 shows the page's edge beside it.
        let corner = vec2(SIDE, 0.0);
        let (fx, fy, rel) = match dest {
            DestView::Top => return,
            DestView::Xyz { left, top, zoom } => {
                if let Some(z) = zoom {
                    self.zoom = z.clamp(0.08, 64.0);
                    self.fit = Fit::None;
                    self.zoom_anchor = None;
                }
                let [fx, fy] = point(left, top);
                (fx, fy, corner)
            }
            DestView::Fit => {
                self.fit = Fit::Page;
                return;
            }
            DestView::FitH { top } => {
                self.fit = Fit::Width;
                let [fx, fy] = point(None, top);
                (fx, fy, corner)
            }
            DestView::FitV { left } => {
                self.fit = Fit::Height;
                let [fx, fy] = point(left, None);
                (fx, fy, corner)
            }
            DestView::FitR { rect } => {
                let ([Some(ax), Some(ay)], [Some(bx), Some(by)]) = (point(Some(rect[0]), Some(rect[3])), point(Some(rect[2]), Some(rect[1]))) else {
                    return;
                };
                // The rectangle as displayed, in points (it can be turned a quarter).
                let (dw, dh) = self.display_size(p);
                let (w, h) = ((ax - bx).abs() * dw, (ay - by).abs() * dh);
                if w >= 1.0 && h >= 1.0 && self.viewport_w >= 1.0 && self.viewport_h >= 1.0 {
                    self.zoom = (self.viewport_w / (w * PT)).min(self.viewport_h / (h * PT)).clamp(0.08, 64.0);
                    self.fit = Fit::None;
                    self.zoom_anchor = None;
                }
                (Some((ax + bx) / 2.0), Some((ay + by) / 2.0), vec2(self.viewport_w, self.viewport_h) / 2.0)
            }
        };
        if fx.is_some() || fy.is_some() {
            self.goto_point = Some((page, fx, fy, rel));
        }
    }

    /// Next page (`true`) or previous page. In two-page view this moves a whole spread: the other
    /// page of the spread is already on screen, so stepping to it wouldn't move the view (#70).
    pub fn step_page(&mut self, forward: bool) {
        let c = self.current;
        let target = match self.layout {
            PageLayout::TwoUp => {
                // Spreads are [0, 1], [2, 3], … or, with a cover page, [0], [1, 2], [3, 4], ….
                let first = if self.cover { c.saturating_sub((c + 1) % 2) } else { c - c % 2 };
                match (forward, self.cover && c == 0) {
                    (true, true) => 1,
                    (true, false) => first.saturating_add(2),
                    (false, _) if self.cover && first <= 1 => 0,
                    (false, _) => first.saturating_sub(2),
                }
            }
            PageLayout::Continuous | PageLayout::Single if forward => c + 1,
            PageLayout::Continuous | PageLayout::Single => c.saturating_sub(1),
        };
        self.go_to_page(target);
    }

    /// Go to the page typed in the page box: a page label (logical page numbers, as Acrobat
    /// does), else a page number. `false` when it names no page.
    pub fn go_to_typed(&mut self, typed: &str, labels: &[String]) -> bool {
        let t = typed.trim();
        let page = labels
            .iter()
            .position(|l| !l.is_empty() && l.eq_ignore_ascii_case(t))
            .or_else(|| t.parse::<usize>().ok().filter(|n| *n >= 1).map(|n| n - 1));
        match page {
            Some(p) if p < self.page_count => {
                self.go_to_page(p);
                true
            }
            _ => false,
        }
    }

    /// View ▸ Page navigation ▸ Previous view (`false`) / Next view (`true`).
    pub fn view_history(&mut self, forward: bool) -> bool {
        let (from, to) = if forward { (&mut self.forward, &mut self.back) } else { (&mut self.back, &mut self.forward) };
        let Some(page) = from.pop() else { return false };
        to.push(self.current);
        let page = page.min(self.page_count.saturating_sub(1));
        self.goto = Some((page, 0.0));
        self.current = page;
        self.page_input = (page + 1).to_string();
        true
    }

    /// Select all: every page in Organize, or every word on the current page.
    pub fn select_all(&mut self) -> bool {
        if self.organize {
            self.selected = (0..self.page_count).collect();
            // Keep the current page as the anchor for the next Shift-click.
            self.select_anchor = (self.page_count > 0).then_some(self.current);
            return !self.selected.is_empty();
        }
        let page = self.current;
        let Some(t) = self.texts.get(&page) else { return false };
        if t.glyphs.is_empty() {
            return false;
        }
        self.selection = Some(Selection::run(page, 0, t.glyphs.len() - 1));
        true
    }

    /// Rotate the view 90° clockwise or counter-clockwise (View ▸ Rotate View, ⇧⌘+ / ⇧⌘−).
    pub fn rotate_view(&mut self, clockwise: bool) {
        self.rotation = (self.rotation + if clockwise { 90 } else { 270 }) % 360;
        self.goto = Some((self.current, 0.0));
    }

    /// Switch the zoom mode, keeping the current page in view.
    pub fn set_fit(&mut self, fit: Fit) {
        self.fit = fit;
        self.goto = Some((self.current, 0.0));
    }

    /// Switch the page display. A no-op when the layout is already current, so re-selecting
    /// it never yanks a scrolled view back to the top.
    pub fn set_layout(&mut self, layout: PageLayout) {
        if self.layout == layout {
            return;
        }
        self.layout = layout;
        self.goto = Some((self.current, 0.0));
    }

    /// Set the cover page in two-page view. A no-op unless something changed.
    pub fn set_cover(&mut self, cover: bool) {
        if self.cover != cover {
            self.cover = cover;
            self.goto = Some((self.current, 0.0));
        }
    }

    /// Whether the cover page setting applies: only two-page view has a cover page.
    pub fn cover_applies(&self) -> bool {
        self.layout == PageLayout::TwoUp
    }

    /// One wheel event in single-page view (the rules are in `wheel_pager`): `dy` is its
    /// vertical delta (negative scrolls down) and `now` is egui time. With `can_turn` false
    /// the gesture is followed but the page stays. Returns whether the page turned.
    pub fn single_page_wheel(&mut self, unit: egui::MouseWheelUnit, dy: f32, phase: egui::TouchPhase, now: f64, can_turn: bool) -> bool {
        if self.layout != PageLayout::Single {
            return false;
        }
        let Some(forward) = self.wheel.feed(unit, dy, phase, now, can_turn) else { return false };
        let before = self.current;
        self.step_page(forward);
        self.current != before
    }

    /// Displayed page size in points for this view rotation.
    fn display_size(&self, p: &pdfcraft_render::PageInfo) -> (f32, f32) {
        if self.rotation % 180 == 90 { (p.height, p.width) } else { (p.width, p.height) }
    }

    /// Marquee Zoom: zoom so `r` (on screen) fills the window, centred.
    pub fn zoom_to_rect(&mut self, r: Rect) {
        let Some((page, pr)) = self.screen_rects.iter().find(|(_, pr)| pr.contains(r.center())).copied() else { return };
        let k = (self.viewport_screen.width() / r.width().max(1.0)).min(self.viewport_screen.height() / r.height().max(1.0));
        let f = (r.center() - pr.min) / pr.size();
        self.zoom = (self.zoom * k).clamp(0.08, 64.0);
        self.fit = Fit::None;
        self.zoom_anchor = Some((page, f.x.clamp(0.0, 1.0), f.y.clamp(0.0, 1.0), self.viewport_screen.center() - self.viewport_screen.min));
    }

    /// View ▸ Zoom ▸ Fit Visible: zoom so the visible content of `page` (display-normalised
    /// [x0, y0, x1, y1], i.e. after view rotation) spans the window's width, its left edge at
    /// the window's left and its top at the top.
    pub fn fit_content(&mut self, page: usize, content: [f32; 4]) -> bool {
        const PAD: f32 = 16.0;
        let Some((_, pr)) = self.screen_rects.iter().find(|(p, _)| *p == page).copied() else { return false };
        let w = (content[2] - content[0]).max(0.01) * pr.width();
        let k = (self.viewport_screen.width() - 2.0 * PAD).max(50.0) / w.max(1.0);
        self.zoom = (self.zoom * k).clamp(0.08, 64.0);
        self.fit = Fit::None;
        self.zoom_anchor = Some((page, content[0].clamp(0.0, 1.0), content[1].clamp(0.0, 1.0), vec2(PAD, MARGIN)));
        true
    }

    /// How large the page grid draws its pages (1.0 = the usual thumbnails).
    pub fn grid_zoom(&self) -> f32 {
        self.grid_zoom
    }

    /// Zoom the page grid; out-of-range values are clamped and nonsense is ignored.
    pub fn set_grid_zoom(&mut self, zoom: f32) {
        if zoom.is_finite() {
            self.grid_zoom = zoom.clamp(*GRID_ZOOM_RANGE.start(), *GRID_ZOOM_RANGE.end());
        }
    }

    /// How many device pixels wide the grid's sharp render of `page` is, if it has one.
    pub fn grid_page_pixels(&self, page: usize) -> Option<u32> {
        self.grid_pages.get(&page).map(|(w, _)| *w)
    }

    /// Zoom keeping the centre of the view still.
    pub fn set_zoom(&mut self, zoom: f32) {
        let centre = self.viewport_screen.center();
        self.zoom_at(zoom, centre);
    }

    /// Zoom keeping the document point under `screen_pos` still (pinch, ⌘-scroll).
    pub fn zoom_at(&mut self, zoom: f32, screen_pos: Pos2) {
        let anchor = self.screen_rects.iter().find(|(_, r)| r.expand(GAP).contains(screen_pos)).map(|(p, r)| {
            let f = (screen_pos - r.min) / r.size();
            (*p, f.x.clamp(0.0, 1.0), f.y.clamp(0.0, 1.0), screen_pos - self.viewport_screen.min)
        });
        self.zoom = zoom.clamp(0.08, 64.0);
        self.fit = Fit::None;
        match anchor {
            Some(a) => self.zoom_anchor = Some(a),
            // Nothing on screen yet (e.g. a launch option): align the current page instead.
            None => self.goto = Some((self.current, 0.0)),
        }
    }

    /// Standard zoom steps (as in Acrobat's zoom menu).
    pub fn zoom_step(&mut self, up: bool) {
        const STEPS: [f32; 17] = [0.1, 0.25, 0.33, 0.5, 0.66, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0, 32.0];
        let z = self.zoom;
        let next = if up { STEPS.iter().copied().find(|s| *s > z * 1.01) } else { STEPS.iter().rev().copied().find(|s| *s < z * 0.99) };
        if let Some(n) = next {
            self.set_zoom(n);
        }
    }

    /// Start collecting demand from every consumer before scheduling or uploading results.
    pub(crate) fn begin_render_frame(&mut self) {
        self.thumb_demand.clear();
        self.frame_queue.clear();
        self.frame_visible.clear();
    }

    pub(crate) fn need_thumbnail(&mut self, page: usize, visible: bool) {
        if page >= self.page_count {
            return;
        }
        if let Some(priority) = self.thumb_demand.get_mut(&page) {
            *priority |= visible;
        } else if self.thumb_demand.len() < MAX_THUMB_DEMAND {
            self.thumb_demand.insert(page, visible);
        } else if visible && let Some(prefetch) = self.thumb_demand.iter().find_map(|(&p, &v)| (!v).then_some(p)) {
            self.thumb_demand.remove(&prefetch);
            self.thumb_demand.insert(page, true);
        }
    }

    /// Only the displayed document owns raster textures. Tabs keep their document and view
    /// state, but do not each retain a separate full application texture allowance.
    pub(crate) fn suspend_rendering(&mut self, pool: &RenderPool) {
        self.begin_render_frame();
        self.pages.clear();
        self.tiles.clear();
        self.thumbs.clear();
        self.grid_pages.clear();
        self.print_pages.clear();
        self.stale_thumbs.clear();
        self.waiting_since.clear();
        // Signature previews own a separate renderer and image/background textures.
        self.signature_drag = Default::default();
        if !self.last_queue.is_empty() {
            pool.set_queue(Vec::new());
            self.last_queue.clear();
        }
        // Results already finished for this tab would otherwise sit in its pool, rasters and
        // all, until the tab is shown again. (An inline pool renders inside try_recv: skip it.)
        if !pool.is_inline() {
            while pool.try_recv().is_some() {}
        }
    }

    fn thumbnail_requests(&self, info: &DocInfo, ppp: f32) -> Vec<RenderRequest> {
        let mut pages: Vec<_> = self.thumb_demand.iter().map(|(&p, &visible)| (p, visible)).collect();
        pages.sort_unstable_by_key(|&(p, visible)| (!visible, p));
        // At high DPI or in a very large viewport every demanded cell still fits the same
        // byte allowance. Both dimensions are capped, including tall/wide page boxes.
        // The count is rounded up to a power of two: the scale is part of each request's tag,
        // so a density that followed every row scrolling in or out would re-render them all.
        let density = thumbnail_density(pages.len(), ppp);
        pages
            .into_iter()
            .filter_map(|(page, _)| {
                let p = info.pages.get(page)?;
                let scale = thumbnail_scale(p.width, p.height, density);
                Some(RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale, tag: THUMB_TAG | u64::from(scale.to_bits()) })
            })
            .collect()
    }

    /// Admit current demand within explicit byte limits. Cached and pending surfaces share
    /// the allowance, so an evicted page can be requested again without a permanent done set.
    fn prepare_render_queue(&mut self, info: &DocInfo, ppp: f32) -> Vec<RenderRequest> {
        let requests = std::mem::take(&mut self.frame_queue);
        let mut queue = Vec::new();
        let mut pages = HashSet::new();
        let mut tiles = HashSet::new();
        let mut tile_tags = HashSet::new();
        let mut grid = Vec::new();
        let (mut page_bytes, mut tile_bytes) = (0usize, 0usize);
        for req in requests.iter().filter(|r| r.kind == RequestKind::Pixels) {
            let Some(p) = info.pages.get(req.page) else { continue };
            // Organize-grid renders: only the cells in view that need one (organize_grid).
            if req.tag & GRID_TAG != 0 {
                grid.push(*req);
                continue;
            }
            // Print-preview rasters: a few sheets at the pane's size (the dialog bounds them).
            if req.tag & PRINT_TAG != 0 {
                if !self.errors.contains_key(&req.page) {
                    queue.push(*req);
                }
                continue;
            }
            // On-screen work is always admitted (the viewport bounds it); the limits only
            // decide how much prefetch fits beside it.
            let visible = self.frame_visible.contains(&req.page);
            if let Some(tile) = req.tile {
                let key = (req.page, req.tag, tile.x / TILE, tile.y / TILE);
                let bytes = rgba_bytes(tile.w as usize, tile.h as usize);
                if (!visible && tile_bytes.saturating_add(bytes) > TILE_BYTES) || !tiles.insert(key) {
                    continue;
                }
                tile_bytes = tile_bytes.saturating_add(bytes);
                tile_tags.insert(req.tag);
                if !self.tiles.contains_key(&key) {
                    queue.push(*req);
                }
            } else {
                let bytes = rgba_bytes(device_pixels(p.width, req.scale) as usize, device_pixels(p.height, req.scale) as usize);
                if (!visible && (page_bytes.saturating_add(bytes) > PAGE_BYTES || pages.len() >= MAX_CACHED_PAGES)) || !pages.insert(req.page) {
                    continue;
                }
                page_bytes = page_bytes.saturating_add(bytes);
                if self.pages.get(&req.page).is_none_or(|p| p.tag != req.tag) {
                    queue.push(*req);
                }
            }
        }
        // Pages no longer demanded stay cached while the allowance has room, nearest to the
        // current page first, so turning back a page shows it at once instead of rendering.
        let mut undemanded: Vec<usize> = self.pages.keys().copied().filter(|p| !pages.contains(p)).collect();
        undemanded.sort_by_key(|p| p.abs_diff(self.current));
        let mut kept = pages.len();
        for page in undemanded {
            let bytes = self.pages.get(&page).map_or(0, |p| texture_bytes(&p.tex));
            if kept < MAX_CACHED_PAGES && page_bytes.saturating_add(bytes) <= PAGE_BYTES {
                page_bytes = page_bytes.saturating_add(bytes);
                kept += 1;
                pages.insert(page);
            }
        }
        self.pages.retain(|p, _| pages.contains(p));
        // Tiles of an earlier zoom (not a demanded scale) stay while the canvas keeps them, a
        // bounded few under a page whose current tiles are still missing (OLD_TILES).
        self.tiles.retain(|key, _| tiles.contains(key) || !tile_tags.contains(&key.1));
        queue.extend(grid);
        let thumbs = self.thumbnail_requests(info, ppp);
        let mut thumb_bytes = 0usize;
        let mut admitted = HashSet::new();
        for req in thumbs {
            let Some(p) = info.pages.get(req.page) else { continue };
            let bytes = rgba_bytes(device_pixels(p.width, req.scale) as usize, device_pixels(p.height, req.scale) as usize);
            if thumb_bytes.saturating_add(bytes) > THUMB_BYTES || self.errors.contains_key(&req.page) {
                continue;
            }
            thumb_bytes += bytes;
            admitted.insert(req.page);
            if self.thumbs.get(&req.page).is_none_or(|t| t.tag != req.tag) || self.stale_thumbs.contains(&req.page) {
                queue.push(req);
            }
        }
        self.thumbs.retain(|p, _| admitted.contains(p));
        self.stale_thumbs.retain(|p| admitted.contains(p));
        // Visible main pages and thumbnails take precedence over background search work.
        queue.extend(requests.into_iter().filter(|r| r.kind == RequestKind::Text));
        queue
    }

    pub(crate) fn finish_render_frame(&mut self, ctx: &egui::Context, info: &DocInfo, pool: &RenderPool) {
        let queue = self.prepare_render_queue(info, ctx.pixels_per_point());
        if queue != self.last_queue {
            pool.set_queue(queue.clone());
            self.last_queue = queue;
        }
        self.receive(ctx, pool);
        if !self.last_queue.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
    }

    /// Pull a bounded batch of current results into textures. Demand is committed first, so
    /// offscreen results and obsolete scale/configuration tags never allocate a ColorImage.
    pub fn receive(&mut self, ctx: &egui::Context, pool: &RenderPool) {
        let budget = if pool.is_inline() { 1 } else { RESULTS_PER_FRAME };
        let mut uploaded = 0usize;
        #[cfg(not(target_arch = "wasm32"))]
        let started = std::time::Instant::now();
        for _ in 0..budget {
            let Some(r) = pool.try_recv() else { break };
            uploaded = uploaded.saturating_add(self.receive_result(ctx, r));
            // One whole-page upload may exceed the frame allowance; it must still progress.
            if uploaded >= UPLOAD_BYTES_PER_FRAME {
                break;
            }
            #[cfg(not(target_arch = "wasm32"))]
            if started.elapsed() >= std::time::Duration::from_millis(4) {
                break;
            }
        }
    }

    fn receive_result(&mut self, ctx: &egui::Context, r: pdfcraft_render::RenderedPage) -> usize {
        if !self.last_queue.contains(&r.request) {
            return 0;
        }
        ctx.request_repaint();
        if r.request.kind == RequestKind::Text {
            match r.text {
                Some(t) => {
                    self.texts.insert(r.request.page, t);
                    self.refresh_find_page(r.request.page);
                }
                None => {
                    log::warn!("page {} text: {}", r.request.page + 1, r.error.unwrap_or_default());
                    self.text_failed.insert(r.request.page);
                }
            }
            return 0;
        }
        if let Some(e) = r.error {
            log::warn!("page {}: {e}", r.request.page + 1);
            self.errors.insert(r.request.page, e);
            return 0;
        }
        let page = r.request.page;
        let bytes = rgba_bytes(r.width as usize, r.height as usize);
        if bytes != r.rgba.len() || !self.admit_texture(r.request, bytes) {
            return 0;
        }
        // The length was checked above, so the renderer's buffer becomes the texture data as is.
        let img = texture_image([r.width as usize, r.height as usize], r.rgba);
        if let Some(t) = r.request.tile {
            let tex = ctx.load_texture(format!("tile-{:?}-{page}-{}-{}", self.id, t.x, t.y), img, TextureOptions::LINEAR);
            self.tiles.insert((page, r.request.tag, t.x / TILE, t.y / TILE), tex);
        } else if r.request.tag & PRINT_TAG != 0 {
            let tex = ctx.load_texture(format!("print-{:?}-{page}", self.id), img, TextureOptions::LINEAR);
            self.print_pages.insert(page, (r.request.tag, tex));
        } else if r.request.tag & GRID_TAG != 0 {
            let tex = ctx.load_texture(format!("grid-{:?}-{page}", self.id), img, TextureOptions::LINEAR);
            self.grid_pages.insert(page, (r.width, tex));
        } else if r.request.tag & THUMB_TAG != 0 {
            let tex = ctx.load_texture(format!("thumb-{:?}-{page}", self.id), img, TextureOptions::LINEAR);
            self.thumbs.insert(page, PageTex { tag: r.request.tag, tex });
            self.stale_thumbs.remove(&page);
        } else {
            let tex = ctx.load_texture(format!("page-{:?}-{page}", self.id), img, TextureOptions::LINEAR);
            self.pages.insert(page, PageTex { tag: r.request.tag, tex });
            self.signature_drag.page_received(page);
            self.waiting_since.remove(&page);
        }
        bytes
    }

    /// Check real dimensions before conversion, including stale textures kept during edits.
    /// Dropping an old handle releases its backend allocation after the current frame.
    fn admit_texture(&mut self, req: RenderRequest, bytes: usize) -> bool {
        // An on-screen page or tile is always taken (the viewport bounds it): it may evict
        // off-screen rasters but never another on-screen one, and is never refused.
        let visible = req.tag & THUMB_TAG == 0 && self.frame_visible.contains(&req.page);
        // Organize-grid renders are bounded by the cells in view (organize_grid). Print
        // previews are the sheets around the one on screen.
        if req.tag & (GRID_TAG | PRINT_TAG) != 0 {
            return true;
        }
        if let Some(t) = req.tile {
            let key = (req.page, req.tag, t.x / TILE, t.y / TILE);
            self.tiles.remove(&key);
            let retained = self.tiles.values().fold(0usize, |n, tex| n.saturating_add(texture_bytes(tex)));
            visible || bytes <= TILE_BYTES.saturating_sub(retained)
        } else {
            let thumb = req.tag & THUMB_TAG != 0;
            let (cache, limit) = if thumb { (&mut self.thumbs, THUMB_BYTES) } else { (&mut self.pages, PAGE_BYTES) };
            cache.remove(&req.page);
            let mut retained = cache.values().fold(0usize, |n, p| n.saturating_add(texture_bytes(&p.tex)));
            // A zoom/DPI change can temporarily retain larger stale images than the new
            // demand estimates. Evict those first, then let the current demand refill them.
            let on_screen = |p: &usize| !thumb && self.frame_visible.contains(p);
            while retained.saturating_add(bytes) > limit {
                let Some(page) = cache.keys().copied().filter(|p| !on_screen(p)).max_by_key(|p| p.abs_diff(self.current)) else { break };
                if let Some(old) = cache.remove(&page) {
                    retained = retained.saturating_sub(texture_bytes(&old.tex));
                }
            }
            visible || bytes <= limit.saturating_sub(retained)
        }
    }

    fn fit_zoom(&mut self, info: &DocInfo) {
        let largest = |side: fn((f32, f32)) -> f32| info.pages.iter().map(|p| side(self.display_size(p))).fold(1.0, f32::max);
        let max_w = largest(|s| s.0);
        // Single-page view fits the page it shows. The scrolling views fit their largest page,
        // so the zoom holds still while pages of other sizes scroll past.
        let (w, h) = match self.layout {
            PageLayout::Single => info.pages.get(self.current).map_or_else(|| (max_w, largest(|s| s.1)), |p| self.display_size(p)),
            PageLayout::Continuous | PageLayout::TwoUp => (max_w, largest(|s| s.1)),
        };
        let avail_w = (self.viewport_w - 2.0 * SIDE).max(100.0);
        let per_row = if self.layout == PageLayout::TwoUp { 2.0 } else { 1.0 };
        match self.fit {
            Fit::Width => self.zoom = (avail_w - GAP * (per_row - 1.0)) / (w * PT * per_row),
            Fit::Page => {
                let zw = (avail_w - GAP * (per_row - 1.0)) / (w * PT * per_row);
                let zh = (self.viewport_h - 2.0 * MARGIN) / (h * PT);
                self.zoom = zw.min(zh);
            }
            Fit::Height => self.zoom = (self.viewport_h - 2.0 * MARGIN) / (h * PT),
            Fit::None => {}
        }
        self.zoom = self.zoom.clamp(0.08, 64.0);
    }

    /// Width of the scroll content. Single-page view sizes it to the page it shows, so a
    /// wider page elsewhere in the document can't push the shown one off-centre.
    fn content_width(&self, info: &DocInfo, avail_w: f32) -> f32 {
        let single = (self.layout == PageLayout::Single).then(|| self.current.min(info.pages.len().saturating_sub(1)));
        let shown = info.pages.iter().enumerate().filter(|&(i, _)| single.is_none_or(|c| c == i));
        let max_w = shown.map(|(_, p)| self.display_size(p).0).fold(0.0, f32::max);
        let per_row = if self.layout == PageLayout::TwoUp { 2.0 } else { 1.0 };
        (max_w * self.zoom * PT * per_row + 2.0 * SIDE).max(avail_w)
    }

    /// Page rects in content coordinates (origin at the scroll content's top-left).
    fn layout(&self, info: &DocInfo, content_w: f32) -> Vec<Rect> {
        let s = self.zoom * PT;
        let mut rects = Vec::with_capacity(info.pages.len());
        let mut y = MARGIN;
        match self.layout {
            PageLayout::Continuous | PageLayout::Single => {
                for p in &info.pages {
                    let (w, h) = self.display_size(p);
                    let size = vec2(w * s, h * s);
                    rects.push(Rect::from_min_size(pos2(((content_w - size.x) / 2.0).max(SIDE), y), size));
                    y += size.y + GAP;
                }
            }
            PageLayout::TwoUp => {
                // With a cover page, the first page sits alone on the right.
                let rows: Vec<&[pdfcraft_render::PageInfo]> = if self.cover && !info.pages.is_empty() {
                    std::iter::once(&info.pages[..1]).chain(info.pages[1..].chunks(2)).collect()
                } else {
                    info.pages.chunks(2).collect()
                };
                for (ri, pair) in rows.into_iter().enumerate() {
                    let sizes: Vec<Vec2> = pair.iter().map(|p| self.display_size(p)).map(|(w, h)| vec2(w * s, h * s)).collect();
                    let row_w: f32 = sizes.iter().map(|v| v.x).sum::<f32>() + GAP * (sizes.len() as f32 - 1.0);
                    let row_h = sizes.iter().map(|v| v.y).fold(0.0, f32::max);
                    let mut x = if self.cover && ri == 0 { (content_w / 2.0 + GAP / 2.0).max(SIDE) } else { ((content_w - row_w) / 2.0).max(SIDE) };
                    for size in sizes {
                        rects.push(Rect::from_min_size(pos2(x, y + (row_h - size.y) / 2.0), size));
                        x += size.x + GAP;
                    }
                    y += row_h + GAP;
                }
            }
        }
        rects
    }

    fn render_scale(&self, ppp: f32) -> f32 {
        // Exactly the device scale: a raster at any other scale is resampled on screen, which
        // blurs every line and glyph (#260).
        self.zoom * PT * ppp
    }

    /// Queue sharp rasters for the print preview. Pages that leave the nearby sheets are dropped.
    pub(crate) fn queue_print_previews(&mut self, pages: &[(usize, f32)]) {
        self.print_pages.retain(|page, _| pages.iter().any(|(p, _)| p == page));
        for &(page, scale) in pages {
            if self.errors.contains_key(&page) || !scale.is_finite() || scale <= 0.0 {
                continue;
            }
            let tag = scale_tag(scale) | PRINT_TAG;
            let fresh = self.print_pages.get(&page).is_some_and(|(have, _)| *have == tag);
            if !fresh {
                self.frame_queue.push(RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale, tag });
            }
        }
    }
}

/// The request tag for a raster at `scale`: equal tags mean the same scale (to 1/65536).
fn scale_tag(scale: f32) -> u64 {
    (f64::from(scale) * 65536.0).round() as u64
}

/// The scale of a raster tagged by [`scale_tag`].
fn tag_scale(tag: u64) -> f32 {
    (tag as f64 / 65536.0) as f32
}

/// Tiles of earlier zooms kept for a page while its current tiles are rendered.
const OLD_TILES: usize = 64;

/// `r` moved so its corner lies on a whole physical pixel, so that a raster drawn from there
/// maps texel for texel onto the screen.
fn snap_to_pixels(r: Rect, ppp: f32) -> Rect {
    let snap = |v: f32| (v * ppp).round() / ppp;
    Rect::from_min_size(pos2(snap(r.min.x), snap(r.min.y)), r.size())
}

/// Maps between a page's coordinate spaces and the screen, including view rotation.
///
/// *View space* is the page as rendered (points, y down, the document's own `/Rotate` applied);
/// normalised page coordinates `(u, v)` are view space divided by the page size. The view
/// rotation (View ▸ Rotate View) turns the page clockwise on screen by `rot` degrees.
#[derive(Clone, Copy)]
pub struct PageXform {
    /// The page's rectangle on screen (already rotated, so width/height may be swapped).
    pub rect: Rect,
    pub rot: u16,
    /// Page size in view space (unrotated by the view).
    pub pw: f32,
    pub ph: f32,
}

impl PageXform {
    /// Draw an image in the PDF user-space `rect` as a custom stamp's appearance draws it: turned
    /// back by `turn` (the page /Rotate it was placed for), so with the page's own rotation it
    /// reads upright as displayed.
    pub(crate) fn paint_user_image(
        &self,
        painter: &egui::Painter,
        tex: egui::TextureId,
        p: &pdfcraft_render::PageInfo,
        rect: [f64; 4],
        turn: i64,
        color: Color32,
    ) {
        let mut mesh = egui::Mesh::with_texture(tex);
        let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
        let (shown_w, shown_h) = if turn % 180 == 0 { (w, h) } else { (h, w) };
        let [a, b, c, d, e, f] = pdfcraft_model::view_matrix_for(turn, rect);
        for (u, v) in [(0.0_f32, 0.0_f32), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            // The texture's (u, v), from its top-left, in the picture's upright frame, then in user space.
            let (dx, dy) = (f64::from(u) * shown_w, f64::from(1.0 - v) * shown_h);
            let (x, y) = (a * dx + c * dy + e, b * dx + d * dy + f);
            let q = p.user_to_view(x as f32, y as f32);
            mesh.vertices.push(egui::epaint::Vertex { pos: self.norm_to_screen(q[0] / self.pw, q[1] / self.ph), uv: pos2(u, v), color });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(mesh));
    }
    pub fn norm_to_screen(&self, u: f32, v: f32) -> Pos2 {
        let (a, b) = match self.rot {
            90 => (1.0 - v, u),
            180 => (1.0 - u, 1.0 - v),
            270 => (v, 1.0 - u),
            _ => (u, v),
        };
        pos2(self.rect.left() + a * self.rect.width(), self.rect.top() + b * self.rect.height())
    }

    pub fn screen_to_norm(&self, p: Pos2) -> (f32, f32) {
        let (a, b) = ((p.x - self.rect.left()) / self.rect.width().max(1e-3), (p.y - self.rect.top()) / self.rect.height().max(1e-3));
        match self.rot {
            90 => (b, 1.0 - a),
            180 => (1.0 - a, 1.0 - b),
            270 => (1.0 - b, a),
            _ => (a, b),
        }
    }

    /// Screen point → view space (points).
    pub fn screen_to_view(&self, p: Pos2) -> (f32, f32) {
        let (u, v) = self.screen_to_norm(p);
        (u * self.pw, v * self.ph)
    }

    /// A view-space rect [x0, y0, x1, y1] → screen rect.
    pub fn view_rect(&self, g: [f32; 4]) -> Rect {
        Rect::from_two_pos(self.norm_to_screen(g[0] / self.pw, g[1] / self.ph), self.norm_to_screen(g[2] / self.pw, g[3] / self.ph))
    }

    /// A PDF user-space rect (crop box, document `/Rotate`) → screen rect.
    pub fn user_rect(&self, info: &DocInfo, page: usize, r: [f32; 4]) -> Rect {
        let p = &info.pages[page];
        let [cx0, cy0, cx1, cy1] = p.crop;
        let (cw, ch) = ((cx1 - cx0).max(1.0), (cy1 - cy0).max(1.0));
        let norm = |x: f32, y: f32| -> (f32, f32) {
            let (u, v) = ((x - cx0) / cw, (cy1 - y) / ch);
            match p.rotation {
                90 => (1.0 - v, u),
                180 => (1.0 - u, 1.0 - v),
                270 => (v, 1.0 - u),
                _ => (u, v),
            }
        };
        let (a, b) = (norm(r[0], r[1]), norm(r[2], r[3]));
        Rect::from_two_pos(self.norm_to_screen(a.0, a.1), self.norm_to_screen(b.0, b.1))
    }

    /// This transform resized to a raster of `px` device pixels (width, height before the view
    /// rotation), so each of its texels covers one screen pixel. The rectangle's corner must
    /// already be on a whole pixel ([`snap_to_pixels`]).
    fn texel_aligned(&self, px: [usize; 2], ppp: f32) -> PageXform {
        let (w, h) = (px[0] as f32 / ppp, px[1] as f32 / ppp);
        let size = if self.rot % 180 == 90 { vec2(h, w) } else { vec2(w, h) };
        PageXform { rect: Rect::from_min_size(self.rect.min, size), ..*self }
    }

    /// Draw a texture covering the normalised page region, rotated with the view.
    pub fn paint_image(&self, painter: &egui::Painter, tex: egui::TextureId, u0: f32, v0: f32, u1: f32, v1: f32) {
        let mut mesh = egui::Mesh::with_texture(tex);
        let corners = [(u0, v0, 0.0, 0.0), (u1, v0, 1.0, 0.0), (u1, v1, 1.0, 1.0), (u0, v1, 0.0, 1.0)];
        for (u, v, tu, tv) in corners {
            mesh.vertices.push(egui::epaint::Vertex { pos: self.norm_to_screen(u, v), uv: pos2(tu, tv), color: Color32::WHITE });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(egui::Shape::mesh(mesh));
    }
}

pub fn shortcuts(view: &mut DocView, ctx: &egui::Context) {
    use egui::{Key, KeyboardShortcut, Modifiers};
    if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::F))) {
        view.open_find();
    }
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    let cmd = |k| KeyboardShortcut::new(Modifiers::COMMAND, k);
    let pressed = |s: KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(&s));
    // Rotation first: egui matches ⌘+ loosely with respect to Shift.
    if pressed(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Plus))
        || pressed(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Equals))
    {
        view.rotate_view(true);
    }
    if pressed(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Minus)) {
        view.rotate_view(false);
    }
    // In the page grid ⌘+ / ⌘− / ⌘0 size its pages instead (looking is always allowed).
    let (zoom_in, zoom_out, zoom_reset) = (pressed(cmd(Key::Plus)) || pressed(cmd(Key::Equals)), pressed(cmd(Key::Minus)), pressed(cmd(Key::Num0)));
    if view.organize {
        if zoom_in {
            view.grid_zoom_request = Some(view.grid_zoom * GRID_ZOOM_STEP);
        }
        if zoom_out {
            view.grid_zoom_request = Some(view.grid_zoom / GRID_ZOOM_STEP);
        }
        if zoom_reset {
            view.grid_zoom_request = Some(1.0);
        }
    } else {
        if zoom_in {
            view.zoom_step(true);
        }
        if zoom_out {
            view.zoom_step(false);
        }
        if zoom_reset {
            view.fit = Fit::Page;
            view.goto = Some((view.current, 0.0));
        }
    }
    if pressed(cmd(Key::Num1)) {
        view.set_zoom(1.0);
    }
    if pressed(cmd(Key::Num2)) {
        view.fit = Fit::Width;
        view.goto = Some((view.current, 0.0));
    }
    if pressed(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::G)) {
        view.find_step(false);
    } else if pressed(cmd(Key::G)) {
        view.find_step(true);
    }
    if pressed(cmd(Key::OpenBracket)) {
        view.view_history(false);
    }
    if pressed(cmd(Key::CloseBracket)) {
        view.view_history(true);
    }
    // ⌘C arrives as a Copy event on most platforms.
    let copy = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
    if copy && let Some(text) = view.selected_text() {
        ctx.copy_text(text);
    }
    let key = |k| ctx.input(|i| i.key_pressed(k));
    if key(Key::Escape) {
        if view.selection.is_some() {
            view.selection = None;
        } else {
            view.find = None;
        }
    }
    if key(Key::Home) {
        view.go_to_page(0);
    }
    if key(Key::End) {
        view.go_to_page(usize::MAX);
    }
    // As in Acrobat: → / ← go to the next / previous page in every layout (#185), and so do
    // ⌘Page Down / ⌘Page Up, or plain Page Down / Page Up in single-page view.
    let command = ctx.input(|i| i.modifiers.command);
    let scrolls = view.layout != PageLayout::Single;
    if key(Key::ArrowRight) || (key(Key::PageDown) && (command || !scrolls)) {
        view.step_page(true);
    }
    if key(Key::ArrowLeft) || (key(Key::PageUp) && (command || !scrolls)) {
        view.step_page(false);
    }
    // ↓ / ↑ scroll a line, and Page Down / Page Up a screen where the pages scroll. In
    // single-page view, ↓ / ↑ turn the page once there is nothing left to scroll that way, as
    // the wheel does (#273): always on a page that fits.
    if !command {
        let screen = (view.viewport_h - KEY_SCROLL_LINE).max(KEY_SCROLL_LINE);
        let steps = [(Key::ArrowDown, KEY_SCROLL_LINE), (Key::ArrowUp, -KEY_SCROLL_LINE), (Key::PageDown, screen), (Key::PageUp, -screen)];
        for (k, by) in steps {
            let arrow = matches!(k, Key::ArrowDown | Key::ArrowUp);
            if !key(k) || !(scrolls || arrow) {
                continue;
            }
            let forward = k == Key::ArrowDown;
            // Only an up-to-date view counts: the page drawn is the current one, with no jump pending.
            let at_end = !scrolls
                && view.goto.is_none()
                && view.single_edges.is_some_and(|(page, top, bottom)| page == view.current && if forward { bottom } else { top });
            if at_end {
                let before = view.current;
                view.step_page(forward);
                if !forward && view.current != before {
                    // Going back up lands on the bottom of the previous page.
                    view.goto = Some((view.current, 1.0));
                }
                view.single_edges = None;
            } else {
                view.key_scroll += by;
            }
        }
    }
}

/// How far ↓ / ↑ scroll: a mouse-wheel line on the desktop. Page Down / Page Up keep this much
/// of the previous screen in view.
const KEY_SCROLL_LINE: f32 = 40.0;

pub fn document_area(app: &mut PdfCraftApp, index: usize, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    // Keyboard scrolling applies to this frame's page view only, never later.
    let key_scroll = std::mem::take(&mut app.views[index].key_scroll);
    // The Search panel closed: its search moves to the find bar.
    let search_open = app.right == Some(crate::RightPanel::Search);
    if let Some(f) = app.views[index].find.as_mut()
        && f.in_panel
        && !search_open
    {
        f.in_panel = false;
    }
    let Some(doc) = app.session.get(app.views[index].id) else { return };
    let info = &doc.info;
    if info.pages.is_empty() {
        app.views[index].auto_scroll.cancel();
        ui.centered_and_justified(|ui| ui.label(tl!("This document has no pages.")));
        return;
    }
    // The print dialog queues its own rasters later in the frame. Closing it drops them.
    if app.dialog != Some(crate::Dialog::Print) {
        app.views[index].print_pages.clear();
    }
    // The Prepare a form panel is open (or a field tool is picked): fields are edited, not filled.
    let preparing = app.is_preparing();
    // Edit a PDF: added text and images can be selected, moved and edited.
    let editing_content = (app.left_open && app.left == crate::LeftPanel::Tool("edit")) || app.quick_tool == QuickTool::AddText;
    let text_style = app.text_style.clone();
    let view = &mut app.views[index];
    // Opened without the owner password and something is restricted.
    let secured = doc.security_summary().is_some_and(|s| !(s.owner || (s.permissions.modify() && s.permissions.assemble())));
    let repaired = !doc.repair_log().is_empty();
    match notices(view, doc, secured, repaired, crate::sign_ui::banner(&doc.signatures), ui, &t) {
        Some(Notice::Repairs) => app.dialog = Some(crate::Dialog::Properties(crate::PropsTab::Advanced)),
        Some(Notice::Security) => app.dialog = Some(crate::Dialog::Properties(crate::PropsTab::Security)),
        Some(Notice::Signatures) => app.right = Some(RightPanel::Signatures),
        Some(Notice::FieldHighlights(on)) => app.view_defaults.highlight_fields = on,
        None => {}
    }
    // No dialog, close prompt or palette over the page: only then does page input count.
    let unobstructed = !app.modal_open() && !app.palette_open;
    let view = &mut app.views[index];
    if view.organize {
        organize_grid(view, info, doc.allows_assembly(), doc.dirty, unobstructed, ui, &t);
        return;
    }

    let avail = ui.available_rect_before_wrap();
    view.viewport_w = avail.width();
    view.viewport_h = avail.height();
    view.fit_zoom(info);

    // Pinch / ctrl+scroll zoom, anchored on the current page.
    let zoom_delta = ui.input(|i| i.zoom_delta());
    if (zoom_delta - 1.0).abs() > 0.001 && ui.rect_contains_pointer(avail) {
        let z = view.zoom * zoom_delta;
        match ui.input(|i| i.pointer.hover_pos()) {
            Some(p) => view.zoom_at(z, p),
            None => view.set_zoom(z),
        }
    }
    // Text selection runs for the Select tool and for the markup tools (highlight…).
    let selects_text = match app.quick_tool {
        QuickTool::Comment(t) => t.markup().is_some() || t == comments::CommentTool::ReplaceText,
        QuickTool::Select => !preparing,
        QuickTool::Redact | QuickTool::ColumnSelect => true,
        QuickTool::Hand
        | QuickTool::Measure(_)
        | QuickTool::Crop
        | QuickTool::Fill(_)
        | QuickTool::Field(_)
        | QuickTool::AddText
        | QuickTool::EditText
        | QuickTool::Stamp(_)
        | QuickTool::CustomStamp(_)
        | QuickTool::Link
        | QuickTool::SignArea { .. }
        | QuickTool::MarqueeZoom
        | QuickTool::Snapshot => false,
    };
    view.viewport_screen = avail;
    let auto_delta = if unobstructed {
        // Latched auto-scroll leaves the left button to the text tools, so text can be selected
        // and highlighted while the page scrolls.
        view.auto_scroll.update(ui, avail, false, selects_text)
    } else {
        view.auto_scroll.cancel();
        Vec2::ZERO
    };

    let content_w = view.content_width(info, avail.width());
    let rects = view.layout(info, content_w);
    let middle_gesture = view.auto_scroll.blocks_input();
    // Single page: when the whole page fits, the wheel would do nothing, so it turns pages
    // instead; zoomed in far enough to pan, it pans. Touch drags are untouched: with nothing
    // to pan, touch users turn pages with the rail buttons, the page box and the arrow keys.
    // Runs before `visible_pages` so the frame that turns the page draws it.
    let mut single_paging = false;
    if view.layout == PageLayout::Single {
        // Within a point, so layout rounding can't stop a fitting page from turning.
        let fits = rects.get(view.current.min(rects.len().saturating_sub(1))).is_some_and(|r| r.height() + 2.0 * MARGIN <= avail.height() + 1.0);
        let can_turn = fits && !middle_gesture && unobstructed && ui.rect_contains_pointer(avail);
        single_paging = can_turn;
        // Every wheel event goes to the pager, so it follows each trackpad touch to its end
        // even while the page can't turn.
        ui.input(|i| {
            for e in &i.events {
                if let egui::Event::MouseWheel { unit, delta, phase, modifiers } = e {
                    // Zooming (⌘ or Ctrl) and sideways scrolling (Shift) never turn pages.
                    let dy = if modifiers.command || modifiers.ctrl || modifiers.shift { 0.0 } else { delta.y };
                    view.single_page_wheel(*unit, dy, *phase, i.time, can_turn);
                }
            }
        });
        if can_turn {
            // Paging owns vertical wheel motion here. A diagonal gesture keeps its sideways
            // part, as `ScrollArea` itself handles each axis.
            ui.input_mut(|i| i.smooth_scroll_delta.y = 0.0);
        }
    }
    // Touchpad scrolling 1:1 (#759). A precision touchpad sends pixel-precise `MouseWheel`
    // events; on Windows they carry no `TouchPhase::Start`, so egui never takes its no-smoothing
    // path and eases deltas of 8 px or more, leaving the page a frame behind the finger. Apply
    // the precise delta ourselves this frame through the un-animated scroll path, and suppress
    // egui's eased copy while it decays so it neither doubles nor trails the motion. A coarse
    // mouse wheel keeps egui's easing (pleasant for chunky notches).
    let now = ui.input(|i| i.time);
    let mut precise_scroll = Vec2::ZERO;
    if ui.rect_contains_pointer(avail) {
        let mut coarse_wheel = false;
        ui.input(|i| {
            for e in &i.events {
                if let egui::Event::MouseWheel { unit, delta, modifiers, .. } = e {
                    if modifiers.command || modifiers.ctrl {
                        continue; // Zooming, not scrolling.
                    }
                    match unit {
                        // Shift scrolls sideways, matching egui's own wheel handling.
                        egui::MouseWheelUnit::Point if modifiers.shift => precise_scroll.x += delta.x + delta.y,
                        egui::MouseWheelUnit::Point => precise_scroll += *delta,
                        egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => coarse_wheel = true,
                    }
                }
            }
        });
        if coarse_wheel {
            view.precise_scroll_until = 0.0;
        } else if precise_scroll != Vec2::ZERO {
            view.precise_scroll_until = now + 0.25;
        }
    }
    // Paging owns the vertical axis in single-page view; leave it to the pager.
    if single_paging {
        precise_scroll.y = 0.0;
    }
    if now < view.precise_scroll_until {
        ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
    } else {
        precise_scroll = Vec2::ZERO;
    }
    let visible_pages: Vec<usize> = match view.layout {
        PageLayout::Single => vec![view.current.min(rects.len() - 1)],
        _ => (0..rects.len()).collect(),
    };
    let (y_shift, content_h) = match view.layout {
        PageLayout::Single => {
            let r = rects[visible_pages[0]];
            (r.top() - MARGIN, r.height() + 2.0 * MARGIN)
        }
        _ => (0.0, rects.last().map(|r| r.bottom() + MARGIN).unwrap_or(0.0)),
    };

    let mut scroll = egui::ScrollArea::both().auto_shrink([false, false]).scroll_source(egui::scroll_area::ScrollSource {
        drag: if middle_gesture {
            egui::scroll_area::DragScroll::Never
        } else if app.quick_tool == QuickTool::Hand {
            egui::scroll_area::DragScroll::Always
        } else {
            egui::scroll_area::DragScroll::OnTouch
        },
        ..Default::default()
    });
    // Each document keeps its own scroll position when switching tabs (#189).
    scroll = scroll.id_salt(("page-view", view.id));
    // A newly opened document starts at the top. eframe saves egui's memory with the settings
    // and document ids start again at 1 every launch, so the offset stored under this id can be
    // where an earlier session left another document.
    if !std::mem::replace(&mut view.shown, true) {
        scroll = scroll.scroll_offset(Vec2::ZERO);
    }
    if let Some((page, fx, fy, rel)) = view.zoom_anchor.take() {
        let r = rects[page.min(rects.len() - 1)];
        let point = pos2(r.left() + fx * r.width(), r.top() - y_shift + fy * r.height());
        scroll = scroll.scroll_offset(vec2((point.x - rel.x).max(0.0), (point.y - rel.y).max(0.0)));
    } else if let Some((page, frac)) = view.goto.take() {
        let page = page.min(rects.len() - 1);
        let r = rects[page];
        scroll = scroll.vertical_scroll_offset((r.top() - y_shift - GAP + frac * r.height()).max(0.0));
    }
    let ppp = ui.ctx().pixels_per_point();
    let scale = view.render_scale(ppp);
    let tag = scale_tag(scale);
    let hand = app.quick_tool == QuickTool::Hand;
    let tool = app.quick_tool;
    let prefs = &app.comment_prefs;
    let allowed = doc.allows_annotation();
    let comments_hidden = doc.comments_hidden();
    let form = doc.form.clone();
    let can_fill = doc.allows_form_filling();
    let can_crop = doc.allows_assembly();
    let mut open_boxes = false;
    let signature = app.signature.clone();
    let initials = app.initials.clone();
    let mut open_initials = false;
    let author = app.comment_prefs.author.clone();
    let today = app.session.today();
    // In Preferences ▸ Date format, or why it can't be stamped into the PDF.
    let date_text = crate::date_text_for_pdf(&app.session);
    let mut refused: Option<String> = None;
    let by_line = app.session.stamp_by_line(&author);
    let mut stamp_placed = false;
    let mut image_action: Option<crate::edit_text_ui::ImageAction> = None;
    let custom_stamp = match app.quick_tool {
        QuickTool::CustomStamp(i) => app.custom_stamps.get(i).cloned(),
        _ => None,
    };
    let mut open_signature = false;
    let mut hover_text: Option<(Pos2, String)> = None;
    let mut clicked_link: Option<LinkTarget> = None;
    let mut canvas_action: Option<comments::CanvasAction> = None;
    let mut field_menu: Option<FieldMenu> = None;
    let mut open_props: Option<(usize, usize)> = None;
    let mut field_props = false;
    let mut field_placed = false;
    let can_modify = doc.allows_modification();
    if preparing {
        crate::prepare::after_refresh(view, &form);
    } else {
        view.prepare.selected = None;
    }
    let added = doc.added.clone();
    let doc_links = if tool == QuickTool::Link { doc.links.clone() } else { Vec::new() };
    if editing_content {
        crate::content_ui::after_refresh(view, &added);
    } else {
        view.content.selected = None;
    }

    // A destination's position (bookmarks, links) refines `goto`: its point at the window's
    // top-left, or the centre of a /FitR rectangle at the window's centre.
    if let Some((page, fx, fy, rel)) = view.goto_point.take() {
        let r = rects[page.min(rects.len() - 1)];
        // Within the content, so an unreachable point never shows a frame scrolled past the end.
        let (max_x, max_y) = ((content_w - avail.width()).max(0.0), (content_h - avail.height()).max(0.0));
        if let Some(fx) = fx {
            scroll = scroll.horizontal_scroll_offset((r.left() + fx * r.width() - rel.x).clamp(0.0, max_x));
        }
        if let Some(fy) = fy {
            scroll = scroll.vertical_scroll_offset((r.top() - y_shift + fy * r.height() - rel.y).clamp(0.0, max_y));
        }
    }
    let out = scroll.show_viewport(ui, |ui, viewport| {
        // egui's drag responses accept every pointer button. Keep a wheel gesture from also
        // selecting text, drawing a mark, or panning with the Hand tool.
        if middle_gesture {
            let opacity = ui.opacity();
            ui.disable();
            // Only interactions pause; the document must keep its original colours.
            ui.set_opacity(opacity);
        }
        // Middle-button auto-scroll, the keyboard and precise touchpad input (#759); positive y
        // moves the content down. All three scroll without easing, unlike egui's wheel default.
        let delta = auto_delta - vec2(0.0, key_scroll) + precise_scroll;
        if delta != Vec2::ZERO {
            ui.scroll_with_delta_animation(delta, egui::style::ScrollAnimation::none());
        }
        let (resp_rect, resp) = ui.allocate_exact_size(vec2(content_w, content_h), Sense::click_and_drag());
        // The Hand tool pans: the content widget takes every drag, so scroll by its delta.
        if hand {
            if resp.dragged() {
                ui.scroll_with_delta(resp.drag_delta());
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            } else if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
        }
        let origin = resp_rect.min - vec2(0.0, y_shift);
        let painter = ui.painter();
        let visible = viewport.translate(resp_rect.min.to_vec2());
        let mut wanted = Vec::new();
        let mut visible_now = Vec::new();
        view.screen_rects.clear();
        view.screen_xforms.clear();
        // Visible heights (points) closer than this are equal: one page size can meet the
        // viewport a fraction of a point differently once the scroll offset is applied.
        const TIE: f32 = 0.5;
        let mut current = view.current;
        let mut best_overlap = -1.0f32;
        let mut current_overlap = -1.0f32;
        let mut current_height = f32::INFINITY;
        let pointer = ui.input(|i| i.pointer.hover_pos());
        for &i in &visible_pages {
            let r = snap_to_pixels(rects[i].translate(origin.to_vec2()), ppp);
            if !r.intersects(visible.expand(400.0)) {
                continue;
            }
            if r.intersects(visible) {
                visible_now.push(i);
                view.screen_rects.push((i, r));
                view.screen_xforms
                    .push((i, PageXform { rect: r, rot: view.rotation, pw: info.pages[i].width.max(1.0), ph: info.pages[i].height.max(1.0) }));
            }
            let overlap = r.intersect(visible).height();
            if i == view.current {
                current_overlap = overlap;
                current_height = r.height();
            }
            // Pages shown equally (two rows wholly on screen) differ only by rounding: the
            // topmost one counts.
            if overlap > best_overlap + TIE {
                best_overlap = overlap;
                current = i;
            }
            let xf = PageXform { rect: r, rot: view.rotation, pw: info.pages[i].width.max(1.0), ph: info.pages[i].height.max(1.0) };
            // Soft shadow + paper.
            painter.add(egui::epaint::Shadow { offset: [0, 3], blur: 14, spread: 0, color: t.page_shadow }.as_shape(r, CornerRadius::ZERO));
            painter.rect_filled(r, CornerRadius::ZERO, Color32::WHITE);
            if let Some(err) = view.errors.get(&i) {
                painter.rect_filled(r, CornerRadius::ZERO, Color32::from_rgb(0xFB, 0xF4, 0xF4));
                icons::paint(
                    ui,
                    Rect::from_center_size(r.center() - vec2(0.0, 26.0), vec2(28.0, 28.0)),
                    "triangle-alert",
                    26.0,
                    Color32::from_rgb(0xC8, 0x3A, 0x3A),
                );
                let message = crate::i18n::fmt(tl!("This page couldn't be displayed.\n{e}"), &[("e", err)]);
                let msg =
                    ui.fonts_mut(|f| f.layout(message, theme::regular(12.5), Color32::from_rgb(0x6A, 0x2A, 0x2A), (r.width() - 40.0).max(80.0)));
                painter.galley(pos2(r.center().x - msg.size().x / 2.0, r.center().y), msg, Color32::BLACK);
            } else {
                let (pw_pt, ph_pt) = (info.pages[i].width.max(1.0), info.pages[i].height.max(1.0));
                // A GPU may take smaller textures than these (OpenGL drivers report as little as
                // 2048 pixels, and uploading a bigger one aborts), so both stay within its limit.
                let max_side = ui.ctx().input(|inp| inp.max_texture_side) as f32;
                let tiled = pw_pt.max(ph_pt) * scale > TILE_THRESHOLD.min(max_side);
                // Whole-page raster: sharp when small, a low-res backdrop when tiled.
                let (want_scale, want_tag) = if tiled {
                    let bs = BASE_SIDE.min(max_side) / pw_pt.max(ph_pt);
                    (bs, scale_tag(bs))
                } else {
                    (scale, tag)
                };
                // Under tiles, a whole-page raster sharper than the backdrop (from before the page
                // grew past the tiling size) is a better backdrop: keep demanding it as it is.
                match view.pages.get(&i).filter(|p| tiled && p.tag != STALE_TAG && tag_scale(p.tag) >= want_scale) {
                    Some(p) => wanted.push((i, tag_scale(p.tag), p.tag, None)),
                    None => wanted.push((i, want_scale, want_tag, None)),
                }
                match view.pages.get(&i) {
                    Some(p) => {
                        if !tiled && p.tag == want_tag {
                            xf.texel_aligned(p.tex.size(), ppp).paint_image(painter, p.tex.id(), 0.0, 0.0, 1.0, 1.0);
                        } else {
                            // A backdrop, or a raster at an older scale until the new one arrives.
                            xf.paint_image(painter, p.tex.id(), 0.0, 0.0, 1.0, 1.0);
                        }
                    }
                    None => {
                        let now = ui.input(|inp| inp.time);
                        let since = *view.waiting_since.entry(i).or_insert(now);
                        let msg = if now - since > 6.0 { "Still rendering — this page is unusually complex…" } else { "Rendering…" };
                        painter.text(r.center(), Align2::CENTER_CENTER, msg, theme::regular(12.0), t.text_faint);
                    }
                }
                if tiled && r.intersects(visible) {
                    // Device-pixel geometry of the scaled page, and the visible part of it
                    // (found by mapping the visible screen corners back into the page). Sides round
                    // up as whole-page rasters do, so the last partial row and column get tiles.
                    let (dw, dh) = (device_pixels(pw_pt, scale), device_pixels(ph_pt, scale));
                    let vis = r.intersect(visible);
                    let corners = [vis.left_top(), vis.right_top(), vis.right_bottom(), vis.left_bottom()].map(|c| xf.screen_to_norm(c));
                    let (u0, u1) = corners.iter().fold((1.0f32, 0.0f32), |(a, b), c| (a.min(c.0), b.max(c.0)));
                    let (v0, v1) = corners.iter().fold((1.0f32, 0.0f32), |(a, b), c| (a.min(c.1), b.max(c.1)));
                    let (vx0, vy0) = ((u0.max(0.0) * dw as f32) as u32, (v0.max(0.0) * dh as f32) as u32);
                    let (vx1, vy1) = (((u1.min(1.0) * dw as f32).ceil() as u32).min(dw), ((v1.min(1.0) * dh as f32).ceil() as u32).min(dh));
                    // Tiles of earlier zooms first, stretched, the nearest scale last (on top):
                    // until the current tiles arrive, a zoom shows them rather than the backdrop.
                    let mut older: Vec<(f32, u32, u32, &TextureHandle)> = view
                        .tiles
                        .iter()
                        .filter(|((p, t, _, _), _)| *p == i && *t != tag)
                        .map(|((_, t, tx, ty), tex)| (tag_scale(*t), *tx, *ty, tex))
                        .collect();
                    older.sort_by(|a, b| (b.0 / scale).ln().abs().total_cmp(&(a.0 / scale).ln().abs()));
                    for (s, tx, ty, tex) in older {
                        let (ow, oh) = (device_pixels(pw_pt, s) as f32, device_pixels(ph_pt, s) as f32);
                        let [w, h] = tex.size();
                        let (x, y) = ((tx * TILE) as f32, (ty * TILE) as f32);
                        xf.paint_image(painter, tex.id(), x / ow, y / oh, (x + w as f32) / ow, (y + h as f32) / oh);
                    }
                    for ty in vy0 / TILE..=(vy1.saturating_sub(1)) / TILE {
                        for tx in vx0 / TILE..=(vx1.saturating_sub(1)) / TILE {
                            let (x, y) = (tx * TILE, ty * TILE);
                            let (w, h) = (TILE.min(dw.saturating_sub(x)), TILE.min(dh.saturating_sub(y)));
                            if w == 0 || h == 0 {
                                continue;
                            }
                            wanted.push((i, scale, tag, Some(Tile { x, y, w, h })));
                            if let Some(tex) = view.tiles.get(&(i, tag, tx, ty)) {
                                let (fw, fh) = (dw as f32, dh as f32);
                                xf.texel_aligned([dw as usize, dh as usize], ppp).paint_image(
                                    painter,
                                    tex.id(),
                                    x as f32 / fw,
                                    y as f32 / fh,
                                    (x + w) as f32 / fw,
                                    (y + h) as f32 / fh,
                                );
                            }
                        }
                    }
                }
            }
            painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(0.5, t.border.gamma_multiply(0.8)), egui::StrokeKind::Outside);

            // Comments: tools, selection, moving and resizing come before text selection.
            let pcx = comments::PageCx { page: i, xf: &xf, info, tool, prefs, allowed, hidden: comments_hidden };
            if let QuickTool::Measure(measure_tool) = tool {
                crate::measure_ui::page_input(ui, &resp, doc, view, &pcx, measure_tool);
            }

            // Form fields take clicks first with the Select tool (as Acrobat fills fields in
            // every viewing mode); then comments; then text selection.
            if let QuickTool::Stamp(kind) = tool
                && allowed
                && let Some(p) = ui.input(|inp| inp.pointer.hover_pos()).filter(|p| xf.rect.contains(*p))
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                if resp.clicked() {
                    // Centred on the click, upright as the page is shown.
                    let (vx, vy) = xf.screen_to_view(p);
                    let (w, h) = kind.size();
                    let corners = [(vx as f64 - w / 2.0, vy as f64 - h / 2.0), (vx as f64 + w / 2.0, vy as f64 + h / 2.0)];
                    let u: Vec<[f32; 2]> = corners.iter().map(|(x, y)| info.pages[i].view_to_user(*x as f32, *y as f32)).collect();
                    let rect = [u[0][0].min(u[1][0]) as f64, u[0][1].min(u[1][1]) as f64, u[0][0].max(u[1][0]) as f64, u[0][1].max(u[1][1]) as f64];
                    let by = (kind.group() == pdfcraft_engine::StampGroup::Dynamic).then(|| by_line.clone());
                    let shape = pdfcraft_engine::Shape::Stamp { rect, stamp: kind, by };
                    view.pending_edit = Some(pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
                        page: i,
                        style: pdfcraft_engine::Style::default_for(&shape),
                        shape,
                        contents: String::new(),
                        author: author.clone(),
                    }));
                    stamp_placed = true;
                }
            }
            if let Some(cs) = custom_stamp.as_ref()
                && allowed
                && let Some(p) = ui.input(|inp| inp.pointer.hover_pos()).filter(|p| xf.rect.contains(*p))
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                if resp.clicked() {
                    // Centred on the click at its natural size (the engine sizes it).
                    let (vx, vy) = xf.screen_to_view(p);
                    let u = info.pages[i].view_to_user(vx, vy);
                    let (x, y) = (u[0] as f64, u[1] as f64);
                    view.pending_edit = Some(pdfcraft_engine::Edit::AddCustomStamp {
                        page: i,
                        rect: [x, y, x, y],
                        name: cs.name.clone(),
                        file: pdfcraft_engine::MarkFile { name: cs.file.clone(), bytes: cs.data.clone(), page: cs.page },
                        author: author.clone(),
                    });
                    stamp_placed = true;
                }
            }
            // Over a mark already placed, the Fill & Sign tool picks it up (comments, below).
            if let QuickTool::Fill(ft) = tool
                && allowed
                && !comments::fill_grabs(ui, &pcx, view)
            {
                match crate::fill_sign::page_input(
                    ui,
                    &resp,
                    &xf,
                    i,
                    info,
                    ft,
                    view,
                    signature.as_ref(),
                    initials.as_ref(),
                    &mut app.signature_preview,
                    &author,
                    &date_text,
                ) {
                    Some(crate::fill_sign::FillAction::Edit(e)) => view.pending_edit = Some(*e),
                    Some(crate::fill_sign::FillAction::Signature(e)) => {
                        view.pending_edit = Some(*e);
                        view.fill_signature_page = Some(i);
                    }
                    Some(crate::fill_sign::FillAction::CreateSignature) => open_signature = true,
                    Some(crate::fill_sign::FillAction::CreateInitials) => open_initials = true,
                    Some(crate::fill_sign::FillAction::Refused(why)) => refused = Some(why),
                    None => {}
                }
            }
            if matches!(tool, QuickTool::SignArea { .. }) {
                crate::sign_ui::page_input(ui, &resp, &xf, i, info, view);
            }
            if matches!(tool, QuickTool::MarqueeZoom | QuickTool::Snapshot) {
                crate::zoom_snap::page_input(ui, &resp, &xf, i, view);
            }
            if tool == QuickTool::Crop && crate::crop::page_input(ui, &resp, &xf, i, info, view, can_crop) {
                view.current = i;
                open_boxes = true;
            }
            let on_field = if preparing {
                let field_tool = match tool {
                    QuickTool::Field(f) => Some(f),
                    _ => None,
                };
                let o = crate::prepare::page_input(ui, &resp, &xf, i, info, &form, field_tool, can_modify, view);
                field_props |= o.properties;
                field_placed |= o.placed;
                o.consumed || field_tool.is_some()
            } else {
                tool == QuickTool::Select && crate::forms_ui::page_input(ui, &resp, &xf, i, info, &form, can_fill, view)
            };
            // Redact draws boxes off text; so does Highlight (an area highlight, as in Acrobat).
            let area_tool = tool == QuickTool::Redact && can_modify || tool == QuickTool::Comment(comments::CommentTool::Highlight) && allowed;
            let boxing = area_tool && {
                let text = view.page_text(i);
                let over_text = |p: Pos2| {
                    let (vx, vy) = xf.screen_to_view(p);
                    text.as_ref().is_some_and(|t| t.glyphs.iter().any(|g| vx >= g.rect[0] && vx <= g.rect[2] && vy >= g.rect[1] && vy <= g.rect[3]))
                };
                crate::redact_ui::page_input(ui, &resp, &xf, i, info, over_text, view)
            };
            let on_content = editing_content && can_modify && {
                crate::content_ui::page_input(ui, &resp, &xf, i, info, &added, tool == QuickTool::AddText, &text_style, view)
                    || tool == QuickTool::AddText
            };
            let on_edit_text = tool == QuickTool::EditText && can_modify && {
                let generation = doc.edit_generation();
                let lines = match view.edit_lines.get(&i) {
                    Some((g, l)) if *g == generation => l.clone(),
                    _ => {
                        let l = doc.text_blocks(i);
                        view.edit_lines.insert(i, (generation, l.clone()));
                        l
                    }
                };
                let images = match view.edit_images.get(&i) {
                    Some((g, l)) if *g == generation => l.clone(),
                    _ => {
                        let l = doc.page_images(i);
                        view.edit_images.insert(i, (generation, l.clone()));
                        l
                    }
                };
                // Images first (they can sit under text boxes' corners); then paragraphs.
                crate::edit_text_ui::image_input(ui, &resp, &xf, i, info, &images, view, &mut image_action)
                    || crate::edit_text_ui::page_input(ui, &resp, &xf, i, info, &lines, view)
            };
            let on_link = tool == QuickTool::Link && can_modify && crate::link_ui::page_input(ui, &resp, &xf, i, info, &doc_links, view);
            let consumed = on_edit_text || on_link || on_content || boxing || on_field || comments::page_input(ui, &resp, &pcx, view);

            let preview_target =
                (matches!(tool, QuickTool::Select | QuickTool::Fill(_)) && !comments_hidden).then_some(view.comments.selected).flatten();
            view.signature_drag.prepare(ui.ctx(), doc, preview_target, scale);
            view.signature_drag.paint(painter, &pcx, &view.comments, view.pending_edit.as_ref());

            // Text layer: find matches, selection, I-beam and drag-to-select.
            let to_screen = |g: [f32; 4]| xf.view_rect(g);
            if let Some(text) = view.texts.get(&i).cloned() {
                if let Some(f) = &view.find {
                    for (k, (mp, range)) in f.matches.iter().enumerate() {
                        if *mp != i {
                            continue;
                        }
                        let current = f.current == Some(k);
                        for lr in text.line_rects(range.clone()) {
                            let fill = if current {
                                Color32::from_rgba_unmultiplied(255, 140, 0, 110)
                            } else {
                                Color32::from_rgba_unmultiplied(255, 214, 0, 90)
                            };
                            painter.rect_filled(to_screen(lr).expand(1.0), CornerRadius::same(2), fill);
                        }
                    }
                }
                if let Some(sel) = view.selection.filter(|s| s.page == i) {
                    for lr in sel.rects(&text) {
                        painter.rect_filled(to_screen(lr), CornerRadius::same(1), Color32::from_rgba_unmultiplied(0x3A, 0x7B, 0xF0, 70));
                    }
                    // The rectangle being drawn, as Acrobat shows it while column-selecting.
                    if let Span::Column { rect } = sel.span
                        && resp.dragged()
                    {
                        let outline = Stroke::new(1.0, Color32::from_rgb(0x3A, 0x7B, 0xF0));
                        painter.rect_stroke(to_screen(rect), CornerRadius::ZERO, outline, egui::StrokeKind::Middle);
                    }
                }
                if selects_text
                    && !consumed
                    && let Some(p) = pointer.filter(|p| r.contains(*p))
                {
                    let (vx, vy) = xf.screen_to_view(p);
                    let over_text = text.glyphs.iter().any(|g| vx >= g.rect[0] && vx <= g.rect[2] && vy >= g.rect[1] && vy <= g.rect[3]);
                    let over_link = info.links.iter().any(|l| l.page == i && xf.user_rect(info, i, l.rect).contains(p));
                    // Column select (#740): the Column select tool, or Alt (Option on macOS) held
                    // while a drag starts, as in Acrobat.
                    let column = tool == QuickTool::ColumnSelect || ui.input(|inp| inp.modifiers.alt);
                    if column && !over_link {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                    } else if over_text && !over_link {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                    }
                    let press_here = ui.input(|inp| inp.pointer.press_origin()).is_some_and(|o| r.contains(o));
                    if resp.drag_started() && press_here && !over_link {
                        let origin = ui.input(|inp| inp.pointer.press_origin()).unwrap_or(p);
                        let (ox, oy) = xf.screen_to_view(origin);
                        view.selection = if column {
                            Some(Selection { page: i, span: Span::Column { rect: [ox, oy, vx, vy] } })
                        } else {
                            text.nearest(ox, oy).map(|a| Selection::run(i, a, a))
                        };
                    }
                    if resp.dragged()
                        && let Some(sel) = view.selection.as_mut().filter(|s| s.page == i)
                    {
                        match &mut sel.span {
                            Span::Run { head, .. } => {
                                if let Some(h) = text.nearest(vx, vy) {
                                    *head = h;
                                }
                            }
                            Span::Column { rect } => {
                                rect[2] = vx;
                                rect[3] = vy;
                            }
                        }
                    }
                    // A rectangle around no text selects nothing.
                    if resp.drag_stopped()
                        && view.selection.is_some_and(|s| s.page == i && matches!(s.span, Span::Column { .. }) && s.text(&text).is_empty())
                    {
                        view.selection = None;
                    }
                    // Quick clicks widen the selection: two a word, three the line, four the page.
                    // egui counts up to three, so four come from the run kept here. The markup and
                    // Redact tools mark a double-clicked word at once, so they stop at the word.
                    let clicks = if resp.triple_clicked() {
                        view.clicks.max(2).saturating_add(1)
                    } else if resp.double_clicked() {
                        2
                    } else {
                        u32::from(resp.clicked())
                    };
                    if clicks > 0 {
                        view.clicks = clicks;
                    }
                    let clicks = if clicks > 2 && tool != QuickTool::Select { 1 } else { clicks };
                    if clicks >= 2 && over_text {
                        let span = text.nearest(vx, vy).and_then(|a| match clicks {
                            2 => text.word_at(a),
                            3 => text.line_at(a),
                            _ => text.glyphs.len().checked_sub(1).map(|last| (0, last)),
                        });
                        if let Some((first, last)) = span {
                            view.selection = Some(Selection::run(i, first, last));
                        }
                    } else if resp.clicked() && !over_link {
                        // ⇧-click extends a selection on this page to the click, keeping its
                        // anchor (#527). Otherwise (no shift, no selection on this page, or a
                        // column selection, which has no anchor glyph; a selection cannot yet
                        // span pages) a click clears the selection.
                        let shift = ui.input(|inp| inp.modifiers.shift);
                        match (view.selection.as_mut().filter(|s| shift && s.page == i).map(|s| &mut s.span), text.nearest(vx, vy)) {
                            (Some(Span::Run { head, .. }), Some(h)) => *head = h,
                            _ => view.selection = None,
                        }
                    }
                }
            }

            comments::page_after_text(&resp, &pcx, view);
            if tool == QuickTool::Redact && can_modify {
                crate::redact_ui::after_text(&resp, i, info, view);
            }
            if area_tool {
                crate::redact_ui::paint(ui, painter, i, view);
            }
            comments::paint_page(ui, painter, &pcx, view);
            if editing_content {
                crate::content_ui::paint_page(ui, painter, &xf, i, info, &added, view);
            }
            if tool == QuickTool::Link {
                crate::link_ui::paint(ui, painter, &xf, i, info, &doc_links, view);
            }
            if preparing {
                crate::prepare::paint_page(ui, painter, &xf, i, info, &form, view);
            } else {
                crate::forms_ui::paint_page(ui, painter, &xf, i, info, &form, view);
            }

            // Form-field highlight (Acrobat's "Highlight existing fields"); required fields get a
            // red border. Radio buttons are round, and so is theirs (as in Acrobat).
            if view.highlight_fields {
                for f in form.iter() {
                    let required = f.has(pdfcraft_engine::field_flags::REQUIRED);
                    let round = f.kind == pdfcraft_engine::FormFieldKind::Radio;
                    for w in f.widgets.iter().filter(|w| w.page == Some(i) && !w.hidden) {
                        let r = w.rect;
                        let sr = xf.user_rect(info, i, [r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32]);
                        let tint = Color32::from_rgba_unmultiplied(0x6E, 0x8E, 0xF5, 48);
                        let (width, color) =
                            if required { (2.0, Color32::from_rgb(0xE3, 0x22, 0x22)) } else { (1.0, Color32::from_rgb(0x6E, 0x8E, 0xF5)) };
                        if round {
                            let radius = sr.width().min(sr.height()) / 2.0;
                            painter.circle_filled(sr.center(), radius, tint);
                            painter.circle_stroke(sr.center(), radius - width / 2.0, Stroke::new(width, color));
                        } else {
                            painter.rect_filled(sr, CornerRadius::same(1), tint);
                            painter.rect_stroke(sr, CornerRadius::same(1), Stroke::new(width, color), egui::StrokeKind::Inside);
                        }
                    }
                }
            }
            // Fill & Sign keeps its placement cursor clear of link/comment hover feedback.
            if !matches!(tool, QuickTool::Fill(_))
                && let Some(p) = pointer
            {
                for l in info.links.iter().filter(|l| l.page == i) {
                    let sr = xf.user_rect(info, i, l.rect);
                    if sr.contains(p) {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        let label = match &l.target {
                            LinkTarget::Page(n, _) => {
                                crate::i18n::fmt(tl!("Go to page {p}"), &[("p", info.pages.get(*n).map(|p| p.label.as_str()).unwrap_or("?"))])
                            }
                            LinkTarget::Uri(u) => u.clone(),
                            LinkTarget::SetLayers { .. } => tl!("Set layer visibility").to_string(),
                            LinkTarget::Other(s) => crate::i18n::fmt(tl!("{s} action"), &[("s", s)]),
                        };
                        hover_text = Some((p, label));
                        if resp.clicked() && tool == QuickTool::Select && !consumed {
                            clicked_link = Some(l.target.clone());
                        }
                    }
                }
                // Annotation hover shows the comment, as Acrobat's popups do; not while drawing
                // freehand, where it would cover the next stroke (#429).
                let freehand = matches!(tool, QuickTool::Comment(comments::CommentTool::Ink | comments::CommentTool::Eraser));
                let gesturing = view.comments.gesture.is_some() || freehand;
                for a in info.annotations.iter().filter(|a| a.page == i && a.in_reply_to.is_none() && !gesturing) {
                    let sr = xf.user_rect(info, i, a.rect);
                    if sr.contains(p) && hover_text.is_none() {
                        let who = a.author.clone().unwrap_or_else(|| a.subtype.clone());
                        let body = a.contents.clone().unwrap_or_default();
                        hover_text = Some((p, if body.is_empty() { who } else { format!("{who}\n{body}") }));
                    }
                }
            }
            for (mp, mr, mc) in &view.compare_marks {
                if *mp == i {
                    let sr = xf.user_rect(info, i, *mr).expand(1.5);
                    painter.rect_filled(sr, CornerRadius::same(2), mc.gamma_multiply(0.28));
                }
            }
            if let Some((fp, fr, t0)) = view.flash
                && fp == i
            {
                let now = ui.input(|inp| inp.time);
                let t0 = if t0 == 0.0 { now } else { t0 };
                view.flash = Some((fp, fr, t0));
                let age = (now - t0) as f32;
                if age < 1.6 {
                    let a = ((1.6 - age) / 1.6 * 255.0) as u8;
                    let sr = xf.user_rect(info, i, fr).expand(4.0);
                    painter.rect_stroke(
                        sr,
                        CornerRadius::same(3),
                        Stroke::new(2.5, Color32::from_rgba_unmultiplied(0x1B, 0x63, 0xE0, a)),
                        egui::StrokeKind::Outside,
                    );
                    ui.ctx().request_repaint();
                } else {
                    view.flash = None;
                }
            }
        }
        // The page navigated to stays current while no page shows more of itself, so the next
        // page step starts from it rather than from a page further down the screen (#188).
        // It also stays current while it is wholly on screen: a short page (a cheque) gone to
        // from the Pages panel shows less of itself than the tall page below it, and must not
        // hand the highlight to that page.
        if current_overlap >= best_overlap - TIE || current_overlap >= current_height - TIE {
            current = view.current;
        }
        if view.layout != PageLayout::Single {
            view.current = current;
            if !ui.memory(|m| m.has_focus(egui::Id::new("page-input"))) {
                view.page_input = (current + 1).to_string();
            }
        }
        resp.context_menu(|ui| {
            // Preparing a form: the selected field's menu.
            if preparing && let Some((name, wi)) = view.prepare.selected.clone() {
                // Several fields: Align, Center, Distribute, Set Fields to Same Size.
                if !view.prepare.also.is_empty() {
                    use crate::prepare::Arrange as A;
                    let others = view.prepare.also.clone();
                    let anchor = (name.clone(), wi);
                    let mut pick = |ui: &mut egui::Ui, op: A, label: &str| {
                        if ui.add_enabled(can_modify, egui::Button::new(tl!(label))).clicked() {
                            view.pending_edit = crate::prepare::arrange(&form, &anchor, &others, op);
                            ui.close();
                        }
                    };
                    ui.menu_button(tl!("Align"), |ui| {
                        pick(ui, A::AlignLeft, "Left");
                        pick(ui, A::AlignRight, "Right");
                        pick(ui, A::AlignTop, "Top");
                        pick(ui, A::AlignBottom, "Bottom");
                        pick(ui, A::AlignCenterV, "Center vertically");
                        pick(ui, A::AlignCenterH, "Center horizontally");
                    });
                    ui.menu_button(tl!("Distribute"), |ui| {
                        pick(ui, A::DistributeH, "Horizontally");
                        pick(ui, A::DistributeV, "Vertically");
                    });
                    ui.menu_button(tl!("Set Fields to Same Size"), |ui| {
                        pick(ui, A::SameHeight, "Height");
                        pick(ui, A::SameWidth, "Width");
                        pick(ui, A::SameSize, "Both");
                    });
                    ui.separator();
                }
                if ui.button(tl!("Properties…")).clicked() {
                    field_menu = Some(FieldMenu::Properties);
                    ui.close();
                }
                if ui.add_enabled(can_modify, egui::Button::new(tl!("Duplicate…"))).clicked() {
                    field_menu = Some(FieldMenu::Duplicate(name.clone()));
                    ui.close();
                }
                ui.separator();
                let delete = if view.prepare.also.is_empty() { "Delete" } else { "Delete selected fields" };
                if ui.add_enabled(can_modify, egui::Button::new(tl!(delete))).clicked() {
                    view.pending_edit = crate::prepare::delete_selected(view);
                    ui.close();
                }
                return;
            }
            canvas_action = comments::context_menu(ui, view, info, prefs, allowed);
        });
        (wanted, visible_now)
    });
    view.single_edges = visible_pages.first().copied().filter(|_| view.layout == PageLayout::Single).map(|page| {
        // At the top once the page's top edge shows where going to a page puts it (a gap
        // below the window's top). Within a point, as for wheel paging, so layout rounding
        // can't hide an edge.
        let max_y = (out.content_size.y - out.inner_rect.height()).max(0.0);
        (page, out.state.offset.y <= MARGIN - GAP + 1.0, out.state.offset.y >= max_y - 1.0)
    });

    view.auto_scroll.paint(ui, avail);

    // Collect all main-page demand, including cached surfaces, for byte admission later.
    let (mut wanted, visible_now) = out.inner;
    let cur = view.current;
    wanted.sort_by_key(|w| (!visible_now.contains(&w.0), w.0.abs_diff(cur), w.3.is_some()));
    // Tiles of far-away pages are useless: free them. Tiles of other zooms stay only while a
    // current tile of their page is still missing, at most `OLD_TILES` of them, nearest scale first.
    let missing: HashSet<usize> = wanted.iter().filter(|w| w.3.is_some()).map(|w| w.0).collect();
    view.tiles.retain(|(p, t, _, _), _| p.abs_diff(cur) <= 2 && (*t == tag || missing.contains(p)));
    let mut older: Vec<(f32, (usize, u64, u32, u32))> =
        view.tiles.keys().filter(|k| k.1 != tag).map(|k| ((tag_scale(k.1) / scale).ln().abs(), *k)).collect();
    if older.len() > OLD_TILES {
        older.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, k) in older.drain(OLD_TILES..) {
            view.tiles.remove(&k);
        }
    }
    let mut queue: Vec<RenderRequest> =
        wanted.iter().map(|&(page, scale, tag, tile)| RenderRequest { page, kind: RequestKind::Pixels, tile, scale, tag }).collect();
    // Text layers: visible pages for selection, every page while a search is active.
    let need_text = |p: &usize| !view.texts.contains_key(p) && !view.text_failed.contains(p);
    let mut text_pages: Vec<usize> = if hand { Vec::new() } else { visible_now.iter().copied().filter(need_text).collect() };
    if view.find.as_ref().is_some_and(|f| !f.case_query.trim().is_empty()) {
        let n = info.pages.len();
        let rest: Vec<usize> = (0..n).map(|k| (cur + k) % n).filter(need_text).filter(|p| !text_pages.contains(p)).collect();
        text_pages.extend(rest);
    }
    queue.extend(text_pages.into_iter().map(|page| RenderRequest { page, kind: RequestKind::Text, tile: None, scale: 1.0, tag: TEXT_TAG }));
    view.frame_queue = queue;
    view.frame_visible = visible_now.iter().copied().collect();

    if let Some((pos, text)) = hover_text {
        egui::Area::new(egui::Id::new("canvas-hover")).order(egui::Order::Tooltip).fixed_pos(pos + vec2(14.0, 16.0)).show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(320.0);
                ui.label(text);
            });
        });
    }
    find_bar(view, info.pages.len(), avail, ui, &t);
    if let Some(e) = comments::composer(ui.ctx(), view, info, prefs) {
        view.pending_edit = Some(e);
    }
    if let Some(e) = crate::edit_text_ui::overlay(ui.ctx(), view, info) {
        view.pending_edit = Some(e);
    }
    if let Some(e) = crate::forms_ui::overlay(ui.ctx(), view, info, &form, today) {
        view.pending_edit = Some(e);
    }
    if let Some(e) = crate::fill_sign::type_box(ui.ctx(), view, info, &author) {
        view.pending_edit = Some(e);
    }
    let (typed, text_done) = crate::content_ui::editor(ui.ctx(), view, info, &added);
    if typed.is_some() {
        view.pending_edit = typed;
    }
    let form_notice = view.forms.notice.take();
    // One crop, then back to selecting (as Acrobat does).
    let cropped = view.pending_edit.as_ref().is_some_and(|e| matches!(e, pdfcraft_engine::Edit::SetPageBox { .. }));
    let mut tool = app.quick_tool;
    comments::keys(ui.ctx(), view, &mut tool, allowed);
    if preparing {
        crate::prepare::keys(ui.ctx(), view);
    }
    if editing_content {
        crate::content_ui::keys(ui.ctx(), view);
    }
    if tool == QuickTool::Link {
        crate::link_ui::keys(ui.ctx(), view);
    } else {
        view.links.selected = None;
    }
    if stamp_placed {
        tool = QuickTool::Select;
    }
    // Text: each click on the page starts a box (#74); Done in the editor goes back to selecting.
    if text_done && tool == QuickTool::AddText {
        tool = QuickTool::Select;
    }
    // One field, then back to selecting (Acrobat's default without "Keep tools pinned").
    if field_placed {
        tool = QuickTool::Select;
    }
    if matches!(field_menu, Some(FieldMenu::Properties)) {
        field_props = true;
    }
    let field_props = field_props.then(|| view.prepare.selected.clone()).flatten();
    match canvas_action {
        Some(comments::CanvasAction::Edit(e)) => view.pending_edit = Some(*e),
        // `choose_right_panel` would borrow all of `app` while `view` is held; same effect.
        Some(comments::CanvasAction::OpenComments) => {
            app.right = Some(RightPanel::Comments);
            app.comments_panel_closed = false;
        }
        Some(comments::CanvasAction::Properties(p, i)) => open_props = Some((p, i)),
        None => {}
    }
    match clicked_link {
        Some(LinkTarget::Page(p, dest)) => view.go_to_dest(p, dest, info),
        Some(LinkTarget::Uri(u)) => app.request_document_url(&u, crate::LinkOrigin::Link),
        Some(LinkTarget::SetLayers { changes, preserve_rb }) => set_layers(app, index, &changes, preserve_rb),
        Some(LinkTarget::Other(s)) => app.notify_fmt("{s} actions run in the JavaScript engine (M6)", &[("s", &s)]),
        None => {}
    }
    if tool == QuickTool::Crop && cropped {
        tool = QuickTool::Select;
    }
    if let Some(done) = app.views[index].marquee_done.take() {
        app.finish_marquee(index, done);
    }
    // A signature rectangle was drawn, or an empty signature field clicked.
    let drawn = app.views[index].sign.drawn.take();
    if let (Some((page, rect)), QuickTool::SignArea { certify }) = (drawn, tool) {
        tool = QuickTool::Select;
        app.quick_tool = tool;
        app.start_signing(page, Some(rect), None, certify.then_some(2));
    }
    if let Some(field) = app.views[index].sign.field.take() {
        let signed = app.session.get(app.views[index].id).is_some_and(|d| d.signatures.iter().any(|s| s.field == field && s.signed));
        if signed {
            app.right = Some(RightPanel::Signatures);
            if !app.sig_expanded.contains(&field) {
                app.sig_expanded.push(field);
            }
        } else {
            let page = app.views[index].current;
            app.start_signing(page, None, Some(field), None);
        }
    }
    app.quick_tool = tool;
    if let Some((page, at)) = app.views[index].comments.attach_at.take() {
        app.attach_file_comment(page, at);
    }
    if let Some((p, i)) = open_props {
        app.open_comment_props(p, i);
    }
    if let Some((p, i)) = app.views[index].comments.default_request.take() {
        app.make_comment_default(p, i);
    }
    if let Some((name, w)) = field_props {
        app.open_field_props(&name, w);
    }
    if let Some(FieldMenu::Duplicate(name)) = field_menu {
        let pages = app.session.get(app.views[index].id).map_or(1, |d| d.info.pages.len());
        app.duplicate_draft = Some(crate::DuplicateDraft { name, all: true, from: 1, to: pages });
        app.dialog = Some(crate::Dialog::DuplicateField);
    }
    if let Some((page, rect)) = app.views[index].links.open_new.take() {
        app.open_link_props(page, Some(rect), None);
    }
    if let Some((page, i)) = app.views[index].links.open_existing.take() {
        app.open_link_props(page, None, Some(i));
    }
    if let Some((page, quads)) = app.views[index].pending_redaction.take() {
        let author = app.comment_prefs.author.clone();
        app.views[index].pending_edit = Some(if app.quick_tool == QuickTool::Comment(comments::CommentTool::Highlight) {
            // An area highlight: a highlight over the box.
            let style = app.comment_prefs.style(comments::CommentTool::Highlight);
            pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
                page,
                shape: pdfcraft_engine::Shape::TextMarkup { kind: pdfcraft_engine::Markup::Highlight, quads },
                style,
                contents: String::new(),
                author,
            })
        } else {
            app.redact_prefs.mark(page, quads, &author)
        });
    }
    match image_action {
        Some(crate::edit_text_ui::ImageAction::Replace(page, index)) => app.replace_page_image_dialog(page, index),
        Some(crate::edit_text_ui::ImageAction::Save(page, index)) => app.save_page_image(page, index),
        None => {}
    }
    if let Some(why) = refused {
        app.notify_error(why);
    }
    if open_signature || open_initials {
        app.signature_draft = crate::fill_sign::SigDraft::new(open_initials, &app.comment_prefs.author);
        app.dialog = Some(crate::Dialog::Signature);
    }
    if open_boxes {
        app.boxes_draft.range = crate::pageboxes::Range::Current;
        app.boxes_draft.seeded = None;
        app.dialog = Some(crate::Dialog::PageBoxes);
    }
    if let Some(n) = form_notice {
        match n {
            crate::forms_ui::FormNotice::Security => app.notify_tr("The document's security settings don't allow filling in form fields"),
            crate::forms_ui::FormNotice::ReadOnly(name) => app.notify_fmt("{name} is read-only", &[("name", &name)]),
            crate::forms_ui::FormNotice::NoAction(name) => app.notify_fmt("{name} has no action", &[("name", &name)]),
        }
    }
    if let Some((name, action)) = app.views[index].forms.button.take() {
        run_button(app, index, &name, action);
    }
    quick_bar(app, avail, ui);
}

/// Run a push button's action (the ones that need no JavaScript engine).
fn run_button(app: &mut PdfCraftApp, index: usize, name: &str, action: pdfcraft_engine::form_scripts::ButtonAction) {
    use pdfcraft_engine::form_scripts::ButtonAction as B;
    let pages = app.session.get(app.views[index].id).map_or(0, |d| d.info.pages.len());
    match action {
        B::Reset { fields, exclude } => {
            let all: Vec<String> = app.session.get(app.views[index].id).map(|d| d.form.iter().map(|f| f.name.clone()).collect()).unwrap_or_default();
            let listed = |n: &String| fields.iter().any(|f| n == f || n.starts_with(&format!("{f}.")));
            let names = match (fields.is_empty(), exclude) {
                (true, _) => None,
                (false, false) => Some(all.iter().filter(|n| listed(n)).cloned().collect()),
                (false, true) => Some(all.iter().filter(|n| !listed(n)).cloned().collect()),
            };
            app.views[index].forms.focus = None;
            app.views[index].pending_edit = Some(pdfcraft_engine::Edit::ResetForm { names });
        }
        B::Named(n) => match n.as_str() {
            "Print" => app.open_print(),
            // What PdfCraft's XFA buttons use, and what a script's `execMenuItem("Save")`
            // does too: the Save As dialog, so a click never overwrites the file unasked.
            "SaveAs" => app.run_command("file.save_as"),
            "NextPage" => app.views[index].step_page(true),
            "PrevPage" => app.views[index].step_page(false),
            "FirstPage" => app.views[index].go_to_page(0),
            "LastPage" => app.views[index].go_to_page(pages.saturating_sub(1)),
            other => app.notify_fmt("{name}: the {other} action isn't supported yet", &[("name", name), ("other", other)]),
        },
        B::Uri(u) => app.request_document_url(&u, crate::LinkOrigin::Button),
        B::GoTo(p) => app.views[index].go_to_page(p.min(pages.saturating_sub(1))),
        B::ShowHide { fields, hide } => {
            let listed = |n: &String| fields.iter().any(|f| n == f || n.starts_with(&format!("{f}.")));
            // `display.hidden` 1, `display.visible` 0, as a script setting `field.display` would.
            let changes: Vec<pdfcraft_engine::FieldChange> = app
                .session
                .get(app.views[index].id)
                .map(|d| d.form.iter().filter(|f| listed(&f.name)).map(|f| f.name.clone()).collect::<Vec<_>>())
                .unwrap_or_default()
                .into_iter()
                .map(|name| pdfcraft_engine::FieldChange {
                    name,
                    value: None,
                    read_only: None,
                    required: None,
                    display: Some(if hide { 1 } else { 0 }),
                })
                .collect();
            if !changes.is_empty() {
                let label = if hide { "Hide a field" } else { "Show a field" };
                app.views[index].pending_edit =
                    Some(pdfcraft_engine::Edit::Batch { label: label.into(), edits: vec![pdfcraft_engine::Edit::ApplyScriptChanges { changes }] });
            }
        }
        B::SetLayers { changes, preserve_rb } => {
            use pdfcraft_engine::form_scripts::LayerOp as Op;
            let changes: Vec<(LayerOp, (u32, u16))> = changes
                .into_iter()
                .map(|(op, ocg)| {
                    let op = match op {
                        Op::On => LayerOp::On,
                        Op::Off => LayerOp::Off,
                        Op::Toggle => LayerOp::Toggle,
                    };
                    (op, ocg)
                })
                .collect();
            set_layers(app, index, &changes, preserve_rb);
        }
        B::Alert(m) => app.notify(m),
        B::Submit(url) => app.notify_fmt(
            "{name} submits the form to {url}; PdfCraft doesn't send form data. Save the document to keep your entries.",
            &[("name", name), ("url", &url)],
        ),
        B::ImportIcon => app.choose_field_image(name),
        B::Script(js) => {
            let id = app.views[index].id;
            app.run_button_script(id, name, &js);
        }
    }
}

/// Run a set-layer-visibility action from a button or link. It changes what is shown, as the
/// Layers panel does (which follows), not the document.
fn set_layers(app: &mut PdfCraftApp, index: usize, changes: &[(LayerOp, (u32, u16))], preserve_rb: bool) {
    let id = app.views[index].id;
    if app.session.set_layer_state(id, changes, preserve_rb) {
        app.views[index].invalidate_content();
    }
}

/// Acrobat-style floating find bar at the top-right of the document area.
fn find_bar(view: &mut DocView, pages: usize, area: Rect, ui: &mut egui::Ui, t: &Tokens) {
    let Some(find) = view.find.as_mut() else { return };
    // The Search panel shows this search.
    if find.in_panel {
        return;
    }
    let mut close = false;
    let mut step: Option<bool> = None;
    let searched = view.texts.len() + view.text_failed.len();
    egui::Area::new(egui::Id::new("find-bar"))
        .order(egui::Order::Middle)
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(area.right_top() + vec2(-18.0, 12.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(t.card)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(10))
                .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if t.dark() { 90 } else { 30 }) })
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(icons::image("search", 16.0, t.text_muted));
                        let edit = egui::TextEdit::singleline(&mut find.query)
                            .id(egui::Id::new("find-input"))
                            .hint_text(tl!("Find text"))
                            .desired_width(220.0)
                            .frame(egui::Frame::NONE);
                        let r = ui.add(edit);
                        if find.focus {
                            r.request_focus();
                            find.focus = false;
                        }
                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            step = Some(!ui.input(|i| i.modifiers.shift));
                            r.request_focus();
                        }
                        if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            close = true;
                        }
                        let status = if find.query.trim().is_empty() {
                            String::new()
                        } else if find.matches.is_empty() {
                            if searched < pages {
                                crate::i18n::fmt(tl!("Searching… {s}/{p}"), &[("s", &searched.to_string()), ("p", &pages.to_string())])
                            } else {
                                tl!("No matches").to_string()
                            }
                        } else {
                            let more = if searched < pages { "+" } else { "" };
                            crate::i18n::fmt(
                                tl!("{c} of {n}{more}"),
                                &[
                                    ("c", &find.current.map(|c| c + 1).unwrap_or(0).to_string()),
                                    ("n", &find.matches.len().to_string()),
                                    ("more", more),
                                ],
                            )
                        };
                        ui.label(egui::RichText::new(status).font(theme::regular(12.0)).color(t.text_muted));
                        let tip = crate::commands::key_tip(ui.ctx(), tl!("Previous ({key})"), crate::commands::FIND_PREV);
                        if icons::button(ui, "chevron-up", 26.0, false, &tip).clicked() {
                            step = Some(false);
                        }
                        let tip = crate::commands::key_tip(ui.ctx(), tl!("Next ({key})"), crate::commands::FIND_NEXT);
                        if icons::button(ui, "chevron-down", 26.0, false, &tip).clicked() {
                            step = Some(true);
                        }
                        let opts = icons::button(ui, "settings-2", 26.0, find.case_sensitive || find.whole_words, tl!("Find options"));
                        egui::Popup::menu(&opts).show(|ui| {
                            let a = ui.checkbox(&mut find.whole_words, tl!("Whole words only")).changed();
                            let b = ui.checkbox(&mut find.case_sensitive, tl!("Case-sensitive")).changed();
                            if a || b {
                                // Search again with the new options.
                                find.case_query.clear();
                            }
                        });
                        if icons::button(ui, "x", 26.0, false, tl!("Close (Esc)")).clicked() {
                            close = true;
                        }
                    });
                });
        });
    if close {
        view.find = None;
        return;
    }
    if find.query != find.case_query {
        view.rerun_find();
    }
    if let Some(forward) = step {
        view.find_step(forward);
    }
}

/// The prepare-mode field menu's choices.
enum FieldMenu {
    Properties,
    Duplicate(String),
}

/// What the notice bar's buttons ask for.
enum Notice {
    Security,
    Signatures,
    Repairs,
    /// Highlight fields was turned on or off.
    FieldHighlights(bool),
}

/// The notice bar above the pages: the signature status first (Acrobat's signature bar), then
/// security, forms and warnings.
fn notices(
    view: &mut DocView,
    doc: &pdfcraft_engine::Document,
    secured: bool,
    repaired: bool,
    signed: Option<(&str, Color32, &str, String)>,
    ui: &mut egui::Ui,
    t: &Tokens,
) -> Option<Notice> {
    let info = &doc.info;
    let xfa = doc.xfa.as_ref();
    if let Some((icon, color, template, arg)) = signed {
        let mut open = false;
        notice_bar(ui, t, icon, color, crate::i18n::fmt(tl!(template), &[("by", &arg)]), |ui| {
            if crate::widgets::pill_button(ui, tl!("Signature panel"), false).clicked() {
                open = true;
            }
        });
        return open.then_some(Notice::Signatures);
    }
    if view.notice_dismissed {
        return None;
    }
    let mut open_security = false;
    let mut open_repairs = false;
    let mut toggled = None;
    let msg = if secured {
        Some(("lock", tl!("This document is secured. Some changes are restricted by its security settings.").to_string(), false))
    } else if let Some(x) = xfa {
        let lang = crate::i18n::current();
        let pages = crate::i18n::trn(lang, x.pages as u64, "{n} page", "{n} pages");
        let fields = crate::i18n::trn(lang, x.fields as u64, "{n} field", "{n} fields");
        let mut text =
            crate::i18n::fmt(tl!("Dynamic XFA form laid out from its template: {pages}, {fields}."), &[("pages", &pages), ("fields", &fields)]);
        if !x.warnings.is_empty() {
            text.push(' ');
            text.push_str(&x.warnings.join("; "));
            text.push('.');
        }
        Some(("text-cursor-input", text, true))
    } else if info.xfa == Some(pdfcraft_render::Xfa::Dynamic) {
        Some((
            "triangle-alert",
            tl!("This is a dynamic XFA form, which PdfCraft can't display yet. What you see is the file's placeholder page.").to_string(),
            false,
        ))
    } else if info.xfa == Some(pdfcraft_render::Xfa::Static) {
        Some((
            "text-cursor-input",
            tl!("XFA form: its fields and its XFA data are kept in step, so other viewers show what you fill in.").to_string(),
            true,
        ))
    } else if info.fields.iter().any(|f| f.kind != pdfcraft_render::FieldKind::PushButton) {
        // Push buttons alone (LaTeX `animate` frames, navigation buttons) leave nothing to fill in.
        Some((
            "text-cursor-input",
            crate::i18n::fmt(tl!("This document contains {n} interactive form fields."), &[("n", &info.fields.len().to_string())]),
            true,
        ))
    } else if repaired {
        Some(("bandage", tl!("This file was damaged and has been repaired. Saving keeps the repaired version.").to_string(), false))
    } else if !info.warnings.is_empty() {
        Some(("triangle-alert", info.warnings[0].clone(), false))
    } else {
        None
    };
    let (icon, text, fields) = msg?;
    notice_bar(ui, t, icon, t.accent_text, text, |ui| {
        if icons::button(ui, "x", 22.0, false, tl!("Dismiss")).clicked() {
            view.notice_dismissed = true;
        }
        if fields {
            let label = if view.highlight_fields { tl!("Hide field highlights") } else { tl!("Highlight fields") };
            if crate::widgets::pill_button(ui, label, view.highlight_fields).clicked() {
                view.highlight_fields = !view.highlight_fields;
                toggled = Some(Notice::FieldHighlights(view.highlight_fields));
            }
        }
        if secured && crate::widgets::pill_button(ui, tl!("Security settings"), false).clicked() {
            open_security = true;
        }
        if repaired && !secured && info.fields.is_empty() && crate::widgets::pill_button(ui, tl!("Details"), false).clicked() {
            open_repairs = true;
        }
    });
    if open_repairs {
        return Some(Notice::Repairs);
    }
    open_security.then_some(Notice::Security).or(toggled)
}

/// One notice bar: an icon and a message on the left, `buttons` (added right to left) on the
/// right. The buttons are laid out first, so the message only gets the width they leave: it
/// wraps there (or, when very little is left, is cut short with the full text on hover) instead
/// of running under the buttons and past the document area into the side panel (#785). The
/// buttons are centred on a row of button height, and the message's first line is centred on
/// the same row; a wrapped message grows the bar downwards.
fn notice_bar(ui: &mut egui::Ui, t: &Tokens, icon: &str, icon_color: Color32, text: String, buttons: impl FnOnce(&mut egui::Ui)) {
    const ROW: f32 = 28.0;
    const ICON: f32 = 16.0;
    egui::Frame::NONE.fill(t.accent_soft).inner_margin(egui::Margin::symmetric(14, 7)).show(ui, |ui| {
        let width = ui.available_width();
        ui.allocate_ui_with_layout(vec2(width, ROW), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.set_min_height(ROW);
            buttons(ui);
            let rest = ui.available_width().max(0.0);
            ui.allocate_ui_with_layout(vec2(rest, ROW), egui::Layout::top_down(egui::Align::Min), |ui| {
                let font = egui::TextStyle::Body.resolve(ui.style());
                let line = ui.fonts_mut(|f| f.row_height(&font)).max(ICON);
                ui.add_space(((ROW - line) / 2.0).max(0.0));
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    let (slot, _) = ui.allocate_exact_size(vec2(ICON, line), Sense::hover());
                    icons::paint(ui, Rect::from_center_size(slot.center(), vec2(ICON, ICON)), icon, ICON, icon_color);
                    let label = egui::Label::new(egui::RichText::new(text).color(t.text));
                    // Wrapping into a sliver would stack the message a few letters per line.
                    ui.add(if ui.available_width() >= 120.0 { label.wrap() } else { label.truncate() });
                });
            });
        });
    });
}

/// The floating quick-action bar at the left edge of the document area.
fn quick_bar(app: &mut PdfCraftApp, area: Rect, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let pos = area.left_top() + vec2(14.0, 14.0);
    egui::Area::new(egui::Id::new("quick-bar")).order(egui::Order::Middle).fixed_pos(pos).show(ui.ctx(), |ui| {
        egui::Frame::NONE
            .fill(t.card)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(CornerRadius::same(10))
            .shadow(egui::Shadow { offset: [0, 2], blur: 10, spread: 0, color: Color32::from_black_alpha(if t.dark() { 80 } else { 22 }) })
            .inner_margin(egui::Margin::same(4))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.vertical(|ui| {
                    if icons::button(ui, "mouse-pointer-2", 32.0, app.quick_tool == QuickTool::Select, tl!("Select (V)")).clicked() {
                        app.quick_tool = QuickTool::Select;
                    }
                    if icons::button(ui, "hand", 32.0, app.quick_tool == QuickTool::Hand, tl!("Hand (H)")).clicked() {
                        app.quick_tool = QuickTool::Hand;
                    }
                    // Comment ▸, Highlight ▸, Draw ▸ (Acrobat's comment toolbar groups). Clicking a
                    // group selects its last-used tool; clicking it again opens the flyout.
                    for g in 0..comments::GROUPS.len() {
                        let current = app.comment_prefs.group_tool[g];
                        let active = matches!(app.quick_tool, QuickTool::Comment(t) if t.group() == g);
                        let resp = icons::button(ui, current.icon(), 32.0, active, tl!(current.label()));
                        // A small corner triangle marks the flyout.
                        let r = resp.rect;
                        let tri = [r.right_bottom() + vec2(-4.0, -4.0), r.right_bottom() + vec2(-9.0, -4.0), r.right_bottom() + vec2(-4.0, -9.0)];
                        ui.painter().add(egui::Shape::convex_polygon(tri.to_vec(), if active { Color32::WHITE } else { t.text_muted }, Stroke::NONE));
                        let open = (resp.clicked() && active) || resp.secondary_clicked();
                        if resp.clicked() && !active {
                            app.execute(current.command());
                        }
                        egui::Popup::menu(&resp)
                            .open_memory(open.then_some(egui::SetOpenCommand::Toggle))
                            .align(egui::RectAlign::RIGHT_START)
                            .gap(6.0)
                            .show(|ui| {
                                for tool in comments::GROUPS[g] {
                                    let on = app.quick_tool == QuickTool::Comment(*tool);
                                    let (row, click) = ui.allocate_exact_size(vec2(180.0, 28.0), Sense::click());
                                    if click.hovered() {
                                        ui.painter().rect_filled(row, CornerRadius::same(4), t.hover);
                                    }
                                    icons::paint(ui, Rect::from_min_size(row.min + vec2(8.0, 6.0), vec2(16.0, 16.0)), tool.icon(), 16.0, t.text);
                                    ui.painter().text(
                                        row.left_center() + vec2(34.0, 0.0),
                                        Align2::LEFT_CENTER,
                                        tl!(tool.label()),
                                        theme::regular(13.0),
                                        t.text,
                                    );
                                    if on {
                                        icons::paint(
                                            ui,
                                            Rect::from_min_size(row.right_top() + vec2(-24.0, 7.0), vec2(14.0, 14.0)),
                                            "check",
                                            14.0,
                                            t.accent,
                                        );
                                    }
                                    let click = click.on_hover_cursor(egui::CursorIcon::PointingHand);
                                    let info = tl!(tool.label()).to_string();
                                    click.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, info.clone()));
                                    if click.clicked() {
                                        app.execute(tool.command());
                                        ui.close();
                                    }
                                }
                            });
                    }
                    // Fill & Sign ▸ (text, marks, date, signature).
                    let current_fill = match app.quick_tool {
                        QuickTool::Fill(f) => Some(f),
                        _ => None,
                    };
                    let shown = current_fill.unwrap_or(crate::fill_sign::FillTool::Text);
                    let resp = icons::button(
                        ui,
                        if current_fill.is_some() { shown.icon() } else { "pen-line" },
                        32.0,
                        current_fill.is_some(),
                        tl!("Fill & Sign"),
                    );
                    let r = resp.rect;
                    let tri = [r.right_bottom() + vec2(-4.0, -4.0), r.right_bottom() + vec2(-9.0, -4.0), r.right_bottom() + vec2(-4.0, -9.0)];
                    ui.painter().add(egui::Shape::convex_polygon(
                        tri.to_vec(),
                        if current_fill.is_some() { Color32::WHITE } else { t.text_muted },
                        Stroke::NONE,
                    ));
                    if resp.clicked() && current_fill.is_none() {
                        app.execute(shown.command());
                    }
                    let open = (resp.clicked() && current_fill.is_some()) || resp.secondary_clicked();
                    egui::Popup::menu(&resp)
                        .open_memory(open.then_some(egui::SetOpenCommand::Toggle))
                        .align(egui::RectAlign::RIGHT_START)
                        .gap(6.0)
                        .show(|ui| {
                            for tool in crate::fill_sign::FILL_TOOLS {
                                if matches!(tool, crate::fill_sign::FillTool::Signature | crate::fill_sign::FillTool::Initials) {
                                    continue;
                                }
                                let on = current_fill == Some(tool);
                                let (row, click) = ui.allocate_exact_size(vec2(180.0, 28.0), Sense::click());
                                if click.hovered() {
                                    ui.painter().rect_filled(row, CornerRadius::same(4), t.hover);
                                }
                                icons::paint(ui, Rect::from_min_size(row.min + vec2(8.0, 6.0), vec2(16.0, 16.0)), tool.icon(), 16.0, t.text);
                                ui.painter().text(
                                    row.left_center() + vec2(34.0, 0.0),
                                    Align2::LEFT_CENTER,
                                    tl!(tool.label()),
                                    theme::regular(13.0),
                                    t.text,
                                );
                                if on {
                                    icons::paint(
                                        ui,
                                        Rect::from_min_size(row.right_top() + vec2(-24.0, 7.0), vec2(14.0, 14.0)),
                                        "check",
                                        14.0,
                                        t.accent,
                                    );
                                }
                                let info = tl!(tool.label()).to_string();
                                click.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, info.clone()));
                                if click.clicked() {
                                    app.execute(tool.command());
                                    ui.close();
                                }
                            }
                            ui.separator();
                            if let Some(command) = crate::fill_sign::signature_entries(ui, app, &t) {
                                app.execute(command);
                                ui.close();
                            }
                        });
                    if let QuickTool::Comment(tool) = app.quick_tool {
                        let (r, _) = ui.allocate_exact_size(vec2(32.0, 9.0), Sense::hover());
                        ui.painter().hline(r.x_range().shrink(6.0), r.center().y, Stroke::new(1.0, t.divider));
                        comments::quick_bar_controls(ui, tool, &mut app.comment_prefs);
                    }
                });
            });
    });
}

/// The organize toolbar: page operations on the selection (Acrobat's Organize Pages bar).
fn organize_toolbar(view: &mut DocView, info: &DocInfo, editable: bool, dirty: bool, unobstructed: bool, ui: &mut egui::Ui, t: &Tokens) {
    let targets = view.target_pages();
    let n = info.pages.len();
    let (first, last) = (targets.first().copied().unwrap_or(0), targets.last().copied().unwrap_or(0));
    egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::symmetric(16, 8)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let label = match view.selected.len() {
                0 => crate::i18n::fmt(tl!("Page {p} of {n}"), &[("p", &(view.current + 1).to_string()), ("n", &n.to_string())]),
                1 => tl!("1 page selected").to_string(),
                k => crate::i18n::fmt(tl!("{n} pages selected"), &[("n", &k.to_string())]),
            };
            // Fixed width so the buttons never shift as the selection text changes.
            ui.add_sized([150.0, 30.0], egui::Label::new(egui::RichText::new(label).font(theme::medium(13.0)).color(t.text_muted)).truncate());
            ui.add_enabled_ui(editable, |ui| {
                if icons::button(ui, "rotate-ccw", 30.0, false, "Rotate counterclockwise").clicked() {
                    view.pending_edit = Some(Edit::RotatePages { pages: targets.clone(), degrees: -90 });
                }
                if icons::button(ui, "rotate-cw", 30.0, false, "Rotate clockwise").clicked() {
                    view.pending_edit = Some(Edit::RotatePages { pages: targets.clone(), degrees: 90 });
                }
                let can_delete = targets.len() < n;
                if ui.add_enabled_ui(can_delete, |ui| icons::button(ui, "trash-2", 30.0, false, tl!("Delete pages (Delete)"))).inner.clicked() {
                    view.pending_edit = Some(Edit::DeletePages { pages: targets.clone() });
                }
                if icons::button(ui, "file-plus", 30.0, false, tl!("Insert a blank page after the selection")).clicked() {
                    let c = info.pages[last].crop;
                    let (w, h) = ((c[2] - c[0]).abs().max(1.0) as f64, (c[3] - c[1]).abs().max(1.0) as f64);
                    view.pending_edit = Some(Edit::InsertBlankPage { at: last + 1, width: w, height: h });
                }
                if icons::button(ui, "file-input", 30.0, false, tl!("Insert pages from a file…")).clicked() {
                    view.pending_action = Some(ViewAction::InsertFromFile);
                }
                if icons::button(ui, "file-output", 30.0, false, tl!("Extract pages to a new document")).clicked() {
                    view.pending_action = Some(ViewAction::Extract);
                }
                if icons::button(ui, "scissors", 30.0, false, tl!("Split into files…")).clicked() {
                    view.pending_action = Some(ViewAction::Split);
                }
                // Select ▸ all, odd, even, landscape, portrait pages (Acrobat's page range
                // selection in Organize Pages).
                let sel = icons::button(ui, "list", 30.0, false, tl!("Select pages"));
                egui::Popup::menu(&sel).show(|ui| {
                    use pdfcraft_engine::{PageOrientation as O, PageParity as P, filter_pages};
                    let all: Vec<usize> = (0..n).collect();
                    for (label, parity, orient) in [
                        ("All pages", P::Both, O::Both),
                        ("Odd pages", P::Odd, O::Both),
                        ("Even pages", P::Even, O::Both),
                        ("Landscape pages", P::Both, O::Landscape),
                        ("Portrait pages", P::Both, O::Portrait),
                    ] {
                        if ui.button(tl!(label)).clicked() {
                            view.select_pages(&filter_pages(info, &all, parity, orient));
                            ui.close();
                        }
                    }
                    if ui.button(tl!("None")).clicked() {
                        view.select_pages(&[]);
                        ui.close();
                    }
                });
                ui.add_space(8.0);
                if ui.add_enabled_ui(first > 0, |ui| icons::button(ui, "chevron-left", 30.0, false, tl!("Move earlier"))).inner.clicked() {
                    view.pending_edit = Some(Edit::MovePages { pages: targets.clone(), to: first - 1 });
                }
                if ui.add_enabled_ui(last + 1 < n, |ui| icons::button(ui, "chevron-right", 30.0, false, tl!("Move later"))).inner.clicked() {
                    view.pending_edit = Some(Edit::MovePages { pages: targets.clone(), to: first + 1 });
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::ghost_button(ui, "x", tl!("Close")).on_hover_text(tl!("Back to the document")).clicked() {
                    view.organize = false;
                }
                // What the grid shows is what is saved.
                let save = ui.add_enabled_ui(dirty, |ui| widgets::pill_button(ui, tl!("Save pages"), true)).inner;
                if save.on_hover_text(tl!("Save these pages as one PDF")).on_disabled_hover_text(tl!("No changes to save")).clicked() {
                    view.pending_action = Some(ViewAction::Save);
                }
                ui.add_space(8.0);
                // Page size in the grid (right to left: in, the percentage, out).
                let zoom = view.grid_zoom;
                if ui
                    .add_enabled_ui(zoom < *GRID_ZOOM_RANGE.end(), |ui| icons::button(ui, "zoom-in", 30.0, false, tl!("Larger pages")))
                    .inner
                    .clicked()
                {
                    view.grid_zoom_request = Some(zoom * GRID_ZOOM_STEP);
                }
                let percent = egui::Button::new(egui::RichText::new(format!("{:.0}%", zoom * 100.0)).font(theme::medium(12.0)).color(t.text_muted))
                    .frame(false);
                let percent = ui.add_sized([44.0, 30.0], percent);
                percent.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tl!("Reset page size")));
                if percent.on_hover_text(tl!("Reset page size")).clicked() {
                    view.grid_zoom_request = Some(1.0);
                }
                if ui
                    .add_enabled_ui(zoom > *GRID_ZOOM_RANGE.start(), |ui| icons::button(ui, "zoom-out", 30.0, false, tl!("Smaller pages")))
                    .inner
                    .clicked()
                {
                    view.grid_zoom_request = Some(zoom / GRID_ZOOM_STEP);
                }
            });
        });
    });
    // Keys act on the selection unless a text field or a dialog has them (Delete under
    // Preferences deleted the selected pages).
    if editable && unobstructed && !ui.ctx().egui_wants_keyboard_input() {
        use egui::{Key, Modifiers};
        let (del, esc) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        // ⌘C / ⌘X / ⌘V copy, cut and paste pages (egui delivers them as clipboard events).
        let (copy, cut, paste) = ui.input(|i| {
            let any = |f: &dyn Fn(&egui::Event) -> bool| i.events.iter().any(f);
            (any(&|e| matches!(e, egui::Event::Copy)), any(&|e| matches!(e, egui::Event::Cut)), any(&|e| matches!(e, egui::Event::Paste(_))))
        });
        if copy || cut {
            view.pending_action = Some(ViewAction::CopyPages { cut });
        }
        if paste {
            view.pending_action = Some(ViewAction::PastePages);
        }
        if del && targets.len() < n {
            view.pending_edit = Some(Edit::DeletePages { pages: targets });
        }
        if esc {
            view.selected.clear();
        }
    }
}

/// Organize pages: a thumbnail grid (Acrobat's Organize Pages view). Click selects, ⌘-click
/// toggles, ⇧-click extends; double-click opens the page.
/// The gap (0 = before the first page, n = after the last) the pointer points at in the grid.
pub(crate) fn drop_gap(cells: &[(usize, Rect)], p: Pos2) -> Option<usize> {
    let (i, r) = cells.iter().min_by(|(_, a), (_, b)| a.distance_sq_to_pos(p).total_cmp(&b.distance_sq_to_pos(p)))?;
    Some(if p.x < r.center().x { *i } else { i + 1 })
}

/// A "+" on a gap of the page grid: insert files there. Returns whether it was clicked.
fn gap_button(ui: &mut egui::Ui, gap: usize, at: Pos2, height: f32, label: String, t: &Tokens) -> bool {
    let resp = ui.interact(Rect::from_center_size(at, Vec2::splat(22.0)), ui.id().with(("org-gap", gap)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label.clone()));
    let hot = resp.hovered();
    if hot {
        ui.painter().line_segment([pos2(at.x, at.y - height / 2.0), pos2(at.x, at.y + height / 2.0)], Stroke::new(3.0, t.accent));
    }
    ui.painter().circle(at, 9.0, if hot { t.accent } else { t.chrome }, Stroke::new(1.0, if hot { t.accent } else { t.border }));
    let ink = Stroke::new(1.5, if hot { Color32::WHITE } else { t.text_muted });
    ui.painter().line_segment([at - vec2(4.0, 0.0), at + vec2(4.0, 0.0)], ink);
    ui.painter().line_segment([at - vec2(0.0, 4.0), at + vec2(0.0, 4.0)], ink);
    resp.on_hover_text(label).clicked()
}

fn organize_grid(view: &mut DocView, info: &DocInfo, editable: bool, dirty: bool, unobstructed: bool, ui: &mut egui::Ui, t: &Tokens) {
    let ppp = ui.ctx().pixels_per_point();
    // The page image scales with the zoom; the padding and the page number don't.
    let cell = vec2(146.0 * view.grid_zoom + 44.0, 194.0 * view.grid_zoom + 56.0);
    let anchor = view.grid_anchor.take();
    let mut open_page = None;
    organize_toolbar(view, info, editable, dirty, unobstructed, ui, t);
    let viewport = ui.available_rect_before_wrap();
    view.viewport_screen = viewport;
    let auto_delta = if unobstructed {
        view.auto_scroll.update(ui, viewport, true, false)
    } else {
        view.auto_scroll.cancel();
        Vec2::ZERO
    };
    let middle_gesture = view.auto_scroll.blocks_input();
    let mut cells: Vec<(usize, Rect)> = Vec::new();
    // The pages in view that are drawn larger than their thumbnails, and those of them that
    // need a sharper render than they have. The width is the size drawn, in device pixels.
    let (mut in_view, mut sharper): (Vec<(usize, f32)>, Vec<RenderRequest>) = (Vec::new(), Vec::new());
    // The source cell can scroll out of the virtualized rows during a drag.
    let mut drop = view.org_drag.is_some() && ui.input(|i| i.pointer.primary_released());
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_viewport(ui, |ui, clip| {
        if middle_gesture {
            let opacity = ui.opacity();
            ui.disable();
            ui.set_opacity(opacity);
        }
        if auto_delta != Vec2::ZERO {
            ui.scroll_with_delta_animation(auto_delta, egui::style::ScrollAnimation::none());
        }
        ui.add_space(20.0);
        let cols = ((ui.available_width() - 40.0) / cell.x).floor().max(1.0) as usize;
        let rows = info.pages.len().div_ceil(cols);
        let left = (ui.available_width() - cols as f32 * cell.x) / 2.0;
        let (grid, _) = ui.allocate_exact_size(vec2(ui.available_width(), rows as f32 * cell.y), Sense::hover());
        for row in thumbnail_rows(clip.top() - 20.0, clip.bottom() - 20.0, cell.y, rows) {
            let row_rect = Rect::from_min_size(grid.min + vec2(0.0, row as f32 * cell.y), vec2(grid.width(), cell.y));
            for col in 0..cols {
                let i = row * cols + col;
                let Some(p) = info.pages.get(i) else { break };
                let c = Rect::from_min_size(pos2(row_rect.left() + left + col as f32 * cell.x, row_rect.top()), cell);
                view.need_thumbnail(i, c.intersects(ui.clip_rect()));
                let resp = ui.interact(c, ui.id().with(("org", i)), if editable { Sense::click_and_drag() } else { Sense::click() });
                cells.push((i, c));
                if let Some((_, above)) = anchor.filter(|(page, _)| *page == i) {
                    ui.scroll_to_rect_animation(c.translate(vec2(0.0, -above)), Some(egui::Align::Min), egui::style::ScrollAnimation::none());
                }
                // Drag pages to move them (the selection, or the page grabbed).
                if resp.drag_started() {
                    if !view.selected.contains(&i) {
                        view.selected = [i].into();
                        view.select_anchor = Some(i);
                    }
                    let mut pages: Vec<usize> = view.selected.iter().copied().collect();
                    pages.sort_unstable();
                    view.org_drag = Some(pages);
                }
                if resp.drag_stopped() {
                    drop = true;
                }
                let info = crate::i18n::fmt(tl!("Page {label}"), &[("label", &p.label)]);
                resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, view.selected.contains(&i), info.clone()));
                let s = (cell.x - 44.0) / p.width.max(1.0);
                let size = vec2(p.width * s, p.height * s).min(vec2(cell.x - 44.0, cell.y - 56.0));
                let pr = Rect::from_center_size(pos2(c.center().x, c.top() + 16.0 + size.y / 2.0), size);
                let selected = view.selected.contains(&i) || (view.selected.is_empty() && i == view.current);
                if selected || resp.hovered() {
                    ui.painter().rect_filled(c.shrink(6.0), CornerRadius::same(8), if selected { t.accent_soft } else { t.hover });
                }
                if view.selected.contains(&i) {
                    ui.painter().rect_stroke(c.shrink(6.0), CornerRadius::same(8), Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
                }
                ui.painter().rect_filled(pr.translate(vec2(0.0, 1.5)), CornerRadius::same(1), t.page_shadow);
                ui.painter().rect_filled(pr, CornerRadius::ZERO, Color32::WHITE);
                // A page drawn larger than its thumbnail gets a render at the size drawn, while
                // it is in view; until that arrives the thumbnail stands in.
                let want = size.x * ppp;
                let thumb = view.thumbs.get(&i).map(|t| &t.tex);
                let thumb_ok = thumb.is_some_and(|t| t.size()[0] as f32 >= want * GRID_SHARP.start());
                let sharp = view.grid_pages.get(&i).filter(|_| !thumb_ok);
                if !thumb_ok && ui.is_rect_visible(c) {
                    in_view.push((i, want));
                    if !sharp.is_some_and(|(w, _)| sharp_enough(*w, want)) && !view.errors.contains_key(&i) {
                        sharper.push(RenderRequest { page: i, kind: RequestKind::Pixels, tile: None, scale: want / p.width.max(1.0), tag: GRID_TAG });
                    }
                }
                if let Some(tex) = sharp.map(|(_, t)| t).or(thumb) {
                    ui.painter().image(tex.id(), pr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                ui.painter().rect_stroke(pr, CornerRadius::ZERO, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
                ui.painter().text(pos2(c.center().x, pr.bottom() + 16.0), Align2::CENTER_CENTER, &p.label, theme::medium(12.0), t.text_muted);
                if resp.clicked() {
                    let m = ui.input(|i| i.modifiers);
                    view.click_page(i, m, false);
                    view.current = i;
                }
                if resp.double_clicked() {
                    open_page = Some(i);
                }
                // Right-click: Cut, Copy, Paste (on the selection, or this page).
                resp.context_menu(|ui| {
                    if !view.selected.contains(&i) {
                        view.selected = [i].into();
                        view.current = i;
                    }
                    if ui.add_enabled(editable, egui::Button::new(tl!("Cut"))).clicked() {
                        view.pending_action = Some(ViewAction::CopyPages { cut: true });
                        ui.close();
                    }
                    if ui.button(tl!("Copy")).clicked() {
                        view.pending_action = Some(ViewAction::CopyPages { cut: false });
                        ui.close();
                    }
                    if ui.add_enabled(editable, egui::Button::new(tl!("Paste after"))).clicked() {
                        view.pending_action = Some(ViewAction::PastePages);
                        ui.close();
                    }
                });
            }
        }
        // Between pages (and at both ends): insert files right there.
        if editable && !middle_gesture && view.org_drag.is_none() {
            let mid = |r: &Rect| r.top() + 16.0 + (cell.y - 56.0) / 2.0;
            for (i, c) in &cells {
                let label = info
                    .pages
                    .get(*i)
                    .map(|p| crate::i18n::fmt(tl!("Insert a file before page {label}"), &[("label", &p.label)]))
                    .unwrap_or_default();
                if gap_button(ui, *i, pos2(c.left(), mid(c)), cell.y - 56.0, label, t) {
                    view.pending_action = Some(ViewAction::InsertFromFileAt(*i));
                }
            }
            if let Some((i, c)) = cells.last()
                && i + 1 == info.pages.len()
                && gap_button(ui, i + 1, pos2(c.right(), mid(c)), cell.y - 56.0, tl!("Insert a file at the end").to_string(), t)
            {
                view.pending_action = Some(ViewAction::InsertFromFileAt(i + 1));
            }
        }
        // While dragging: the gap the pages would go to, drawn as a bar.
        if let (Some(pages), Some(p)) = (view.org_drag.clone(), ui.input(|i| i.pointer.hover_pos())) {
            if let Some(gap) = drop_gap(&cells, p) {
                let x = match cells.iter().find(|(i, _)| *i == gap) {
                    Some((_, r)) => r.left() + 3.0,
                    None => cells.last().map_or(0.0, |(_, r)| r.right() - 3.0),
                };
                let row = cells.iter().find(|(i, _)| *i == gap).or(cells.last()).map(|(_, r)| r.y_range()).unwrap_or(egui::Rangef::new(0.0, 0.0));
                ui.painter().line_segment([pos2(x, row.min + 10.0), pos2(x, row.max - 10.0)], Stroke::new(3.0, t.accent));
                ui.painter().text(
                    p + vec2(14.0, 14.0),
                    Align2::LEFT_TOP,
                    if pages.len() == 1 { tl!("1 page").to_string() } else { crate::i18n::fmt(tl!("{n} pages"), &[("n", &pages.len().to_string())]) },
                    theme::medium(12.0),
                    t.accent_text,
                );
            }
            if drop {
                view.org_drag = None;
                if let Some(gap) = drop_gap(&cells, p) {
                    // `to` counts positions without the moving pages.
                    let to = gap - pages.iter().filter(|x| **x < gap).count();
                    let first = pages[0];
                    let contiguous = pages.windows(2).all(|w| w[1] == w[0] + 1);
                    if !(contiguous && to == first) {
                        view.pending_edit = Some(Edit::MovePages { pages: pages.clone(), to });
                        view.selected = (to..to + pages.len()).collect();
                    }
                }
            }
        } else if drop {
            view.org_drag = None;
        }
    });
    // Files dragged in from outside go to the gap under the pointer. Some platforms don't say
    // where the pointer is until the files are dropped; then the whole grid is the target.
    let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|p| viewport.contains(*p));
    view.grid_gap = pointer.filter(|_| editable && view.org_drag.is_none()).and_then(|p| drop_gap(&cells, p));
    if editable && ui.input(|i| !i.raw.hovered_files.is_empty()) {
        let painter = ui.painter().with_clip_rect(viewport);
        painter.rect_stroke(viewport.shrink(2.0), CornerRadius::same(6), Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
        if let Some(gap) = view.grid_gap {
            let at = cells.iter().find(|(i, _)| *i == gap).map(|(_, r)| (r.left() + 3.0, r.y_range()));
            if let Some((x, row)) = at.or(cells.last().map(|(_, r)| (r.right() - 3.0, r.y_range()))) {
                painter.line_segment([pos2(x, row.min + 10.0), pos2(x, row.max - 10.0)], Stroke::new(3.0, t.accent));
            }
        }
        // A "+" that follows the pointer.
        if let Some(p) = pointer {
            let at = p + vec2(16.0, 16.0);
            painter.circle_filled(at, 9.0, t.accent);
            painter.line_segment([at - vec2(4.0, 0.0), at + vec2(4.0, 0.0)], Stroke::new(1.5, Color32::WHITE));
            painter.line_segment([at - vec2(0.0, 4.0), at + vec2(0.0, 4.0)], Stroke::new(1.5, Color32::WHITE));
        }
    }
    view.auto_scroll.paint(ui, viewport);
    // Pinch, or Ctrl/⌘ with the wheel, zooms the grid about the page under the pointer.
    let pinch = ui.input(|i| i.zoom_delta());
    if (pinch - 1.0).abs() > 0.001 && pointer.is_some() && !middle_gesture {
        view.grid_zoom_request = Some(view.grid_zoom * pinch);
    }
    if let Some(zoom) = view.grid_zoom_request.take() {
        let before = view.grid_zoom;
        view.set_grid_zoom(zoom);
        if view.grid_zoom != before {
            // Keep the page under the pointer (or else the first one in view) where it is.
            let under = pointer.and_then(|p| cells.iter().find(|(_, r)| r.contains(p)));
            let anchor = under.or_else(|| cells.iter().find(|(_, r)| r.bottom() > viewport.top()));
            view.grid_anchor = anchor.map(|(i, r)| (*i, r.top() - viewport.top()));
            ui.ctx().request_repaint();
        }
    }
    // When this frame's thumbnail will cover the cell, drop the sharp render without waiting
    // for that texture to arrive.
    let density = thumbnail_density(view.thumb_demand.len(), ppp);
    in_view.retain(|&(page, want)| {
        info.pages.get(page).is_none_or(|p| (device_pixels(p.width, thumbnail_scale(p.width, p.height, density)) as f32) < want * GRID_SHARP.start())
    });
    sharper.retain(|req| in_view.iter().any(|(page, _)| *page == req.page));
    view.grid_pages.retain(|page, _| in_view.iter().any(|(kept, _)| kept == page));
    // Sharp renders of the pages in view join this frame's demand ahead of the thumbnails.
    view.frame_queue.extend(sharper);
    if let Some(p) = open_page {
        view.organize = false;
        view.go_to_page(p);
    }
}

/// A rendered raster as texture data. Its words are premultiplied RGBA bytes, exactly
/// `Color32`s, so the renderer's buffer becomes the image without a copy on the UI thread.
pub(crate) fn texture_image(size: [usize; 2], pixels: pdfcraft_render::Pixels) -> egui::ColorImage {
    match bytemuck::allocation::try_cast_vec::<u32, Color32>(pixels.into_words()) {
        Ok(px) if px.len() == size[0].saturating_mul(size[1]) => egui::ColorImage::new(size, px),
        Ok(px) => egui::ColorImage::from_rgba_premultiplied(size, bytemuck::cast_slice(&px)),
        Err((_, words)) => egui::ColorImage::from_rgba_premultiplied(size, bytemuck::cast_slice(&words)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_rasters_become_texture_images_without_a_copy() {
        let pdf = b"%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 50] /Contents 4 0 R >> endobj
4 0 obj << /Length 35 >> stream
0 0 1 rg 10 10 30 20 re f
endstream endobj
trailer << /Root 1 0 R >>
%%EOF";
        let mut r = pdfcraft_render::PageRenderer::new(Arc::new(pdf.to_vec()), Default::default());
        let out = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.5, tag: 0 });
        assert!(out.error.is_none(), "{:?}", out.error);
        let size = [out.width as usize, out.height as usize];
        let copied = egui::ColorImage::from_rgba_premultiplied(size, &out.rgba);
        let at = out.rgba.as_ptr();
        let img = texture_image(size, out.rgba);
        assert_eq!(img, copied);
        assert_eq!(img.pixels.as_ptr().cast::<u8>(), at, "the renderer's own buffer");
    }

    #[test]
    fn grid_zoom_is_clamped_and_sharp_renders_tolerate_a_pinch() {
        assert!(sharp_enough(900, 876.0) && sharp_enough(800, 876.0) && sharp_enough(1300, 876.0));
        assert!(!sharp_enough(264, 876.0), "a thumbnail stretched over a zoomed page");
        assert!(!sharp_enough(876, 200.0), "far larger than drawn: wasteful");
        assert!(!sharp_enough(100, 0.0) && !sharp_enough(100, f32::NAN));
        let mut v = view(3, PageLayout::Continuous);
        for odd in [f32::NAN, f32::INFINITY] {
            v.set_grid_zoom(odd);
            assert_eq!(v.grid_zoom(), 1.0);
        }
        v.set_grid_zoom(99.0);
        assert_eq!(v.grid_zoom(), 3.0);
        v.set_grid_zoom(-4.0);
        assert_eq!(v.grid_zoom(), 0.5);
    }

    fn view(pages: usize, layout: PageLayout) -> DocView {
        let info = pdfcraft_render::DocInfo {
            pages: (0..pages)
                .map(|_| pdfcraft_render::PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 })
                .collect(),
            ..Default::default()
        };
        DocView::new(DocId(1), &info, ViewDefaults { layout, ..Default::default() })
    }

    fn thumbnail_info(count: usize) -> DocInfo {
        DocInfo {
            pages: (0..count)
                .map(|i| {
                    let (width, height) = match i % 3 {
                        0 => (612.0, 792.0),
                        1 => (200.0, 4000.0),
                        _ => (4000.0, 200.0),
                    };
                    pdfcraft_render::PageInfo { width, height, label: (i + 1).to_string(), crop: [0.0, 0.0, width, height], rotation: 0 }
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn ten_thousand_pages_only_demand_viewport_and_prefetch_at_both_densities() {
        let info = thumbnail_info(10_000);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        let cols = 4;
        let rows = info.pages.len().div_ceil(cols);
        for ppp in [1.0, 2.0] {
            for row in [0, 1234, rows - 1] {
                v.begin_render_frame();
                let range = thumbnail_rows(row as f32 * 250.0, row as f32 * 250.0 + 750.0, 250.0, rows);
                for p in range.start * cols..(range.end * cols).min(info.pages.len()) {
                    v.need_thumbnail(p, p / cols == row);
                }
                let queue = v.prepare_render_queue(&info, ppp);
                assert!(queue.len() <= 5 * cols, "only three visible rows and two prefetch rows");
                assert_eq!(queue.first().unwrap().page / cols, row, "visible cells precede prefetch");
                let bytes: usize = queue
                    .iter()
                    .map(|r| {
                        let p = &info.pages[r.page];
                        let (w, h) = (device_pixels(p.width, r.scale), device_pixels(p.height, r.scale));
                        assert!(w <= (THUMB_W * ppp).ceil() as u32);
                        assert!(h <= (THUMB_H * ppp).ceil() as u32);
                        rgba_bytes(w as usize, h as usize)
                    })
                    .sum();
                assert!(bytes <= THUMB_BYTES);
                assert_eq!(queue, v.prepare_render_queue(&info, ppp), "unchanged demand reconstructs the same work");
            }
        }
    }

    #[test]
    fn oversized_thumbnail_viewport_scales_all_admitted_cells_to_one_budget() {
        let info = thumbnail_info(10_000);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        for page in 0..MAX_THUMB_DEMAND {
            v.need_thumbnail(page, true);
        }
        let queue = v.prepare_render_queue(&info, 2.0);
        assert_eq!(queue.len(), MAX_THUMB_DEMAND);
        let bytes: usize = queue
            .iter()
            .map(|r| {
                let p = &info.pages[r.page];
                rgba_bytes(device_pixels(p.width, r.scale) as usize, device_pixels(p.height, r.scale) as usize)
            })
            .sum();
        assert!(bytes <= THUMB_BYTES);
    }

    fn solid_result(request: RenderRequest, w: u32, h: u32) -> pdfcraft_render::RenderedPage {
        pdfcraft_render::RenderedPage {
            request,
            width: w,
            height: h,
            rgba: pdfcraft_render::Pixels::from(vec![u32::MAX; rgba_bytes(w as usize, h as usize) / 4]),
            text: None,
            error: None,
            warnings: Vec::new(),
            millis: 0,
        }
    }

    #[test]
    fn thumbnails_release_on_scroll_panel_close_and_request_again_after_eviction() {
        let ctx = egui::Context::default();
        let info = thumbnail_info(10_000);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        v.need_thumbnail(0, true);
        let first = v.prepare_render_queue(&info, 1.0);
        v.last_queue = first.clone();
        assert_eq!(v.receive_result(&ctx, solid_result(first[0], 132, 171)), 132 * 171 * 4);
        assert!(v.prepare_render_queue(&info, 1.0).is_empty());
        v.begin_render_frame();
        v.need_thumbnail(9999, true);
        v.prepare_render_queue(&info, 1.0);
        assert!(v.thumbs.is_empty(), "jump releases old handles");
        v.begin_render_frame();
        v.need_thumbnail(0, true);
        assert_eq!(v.prepare_render_queue(&info, 1.0), first, "an evicted result must be requested again");
        v.begin_render_frame();
        assert!(v.prepare_render_queue(&info, 1.0).is_empty(), "closing consumers cancels thumbnail work");
    }

    #[test]
    fn stale_results_are_rejected_before_conversion_after_dpi_invalidation_and_suspend() {
        let ctx = egui::Context::default();
        let info = thumbnail_info(1);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        v.need_thumbnail(0, true);
        let first = v.prepare_render_queue(&info, 1.0)[0];
        v.last_queue = v.prepare_render_queue(&info, 2.0);
        // Valid pixels: only the request gate can keep these out (a size check can't).
        let obsolete = || solid_result(first, 132, 171);
        assert_eq!(v.receive_result(&ctx, obsolete()), 0);
        assert!(v.thumbs.is_empty(), "a result for the old DPI is not uploaded");
        // The same result is uploaded while its request is current.
        v.last_queue = vec![first];
        assert_eq!(v.receive_result(&ctx, obsolete()), 132 * 171 * 4);
        v.thumbs.clear();
        v.invalidate_content();
        assert_eq!(v.receive_result(&ctx, obsolete()), 0);
        assert!(v.thumbs.is_empty(), "nor after the content changed");
        let pool = RenderPool::new_inline(Arc::new(Vec::new()), Default::default());
        v.last_queue = vec![first];
        v.suspend_rendering(&pool);
        assert_eq!(v.receive_result(&ctx, obsolete()), 0);
        assert!(v.thumbs.is_empty(), "nor after the tab was suspended");
    }

    #[test]
    fn editing_after_layout_preserves_stale_page_until_replacement() {
        let ctx = egui::Context::default();
        let info = thumbnail_info(1);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        let req = RenderRequest { scale: 1.0, tag: 1000, ..Default::default() };
        v.last_queue = vec![req];
        v.receive_result(&ctx, solid_result(req, 10, 10));
        v.frame_queue = vec![req];
        v.page_changed(0);
        assert_eq!(v.prepare_render_queue(&info, 1.0), vec![req]);
        assert_eq!(v.pages.get(&0).unwrap().tag, STALE_TAG);
    }

    #[test]
    fn page_and_tile_admission_accounts_for_bytes_before_count_limits() {
        let mut info = thumbnail_info(40);
        for p in &mut info.pages {
            p.width = 4000.0;
            p.height = 4000.0;
        }
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        v.frame_queue = (0..40).map(|page| RenderRequest { page, scale: 1.0, tag: 1000, ..Default::default() }).collect();
        let queue = v.prepare_render_queue(&info, 1.0);
        assert_eq!(queue.len(), 2, "128 MiB admits two 4000-square pages, not 24");
        v.frame_queue = (0..40)
            .map(|page| RenderRequest { page, scale: 1.0, tag: 1000, tile: Some(Tile { x: 0, y: 0, w: 1024, h: 1024 }), ..Default::default() })
            .collect();
        assert_eq!(v.prepare_render_queue(&info, 1.0).len(), 32);
        assert!(!v.admit_texture(RenderRequest::default(), PAGE_BYTES + 1));
    }

    #[test]
    fn on_screen_pages_and_tiles_are_admitted_beyond_the_prefetch_allowance() {
        let ctx = egui::Context::default();
        let mut info = thumbnail_info(40);
        for p in &mut info.pages {
            p.width = 2000.0;
            p.height = 2000.0;
        }
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        // Ten 16 MB pages on screen (a zoomed-out HiDPI view) exceed the 128 MiB allowance.
        v.frame_visible = (0..10).collect();
        v.frame_queue = (0..40).map(|page| RenderRequest { page, scale: 1.0, tag: 1000, ..Default::default() }).collect();
        let queue = v.prepare_render_queue(&info, 1.0);
        let pages: Vec<usize> = queue.iter().map(|r| r.page).collect();
        assert_eq!(pages, (0..10).collect::<Vec<_>>(), "every on-screen page, and no prefetch past the allowance");
        v.last_queue = queue.clone();
        for req in &queue {
            assert!(v.receive_result(&ctx, solid_result(*req, 2000, 2000)) > 0, "on-screen page {} uploaded", req.page);
        }
        assert_eq!(v.pages.len(), 10, "none evicted another on-screen page");
        // A 5K viewport needs more 1024-pixel tiles than the tile allowance holds.
        v.frame_queue = (0..40)
            .map(|x| RenderRequest { page: 0, scale: 1.0, tag: 1000, tile: Some(Tile { x: x * TILE, y: 0, w: 1024, h: 1024 }), ..Default::default() })
            .collect();
        assert_eq!(v.prepare_render_queue(&info, 1.0).len(), 40);
    }

    #[test]
    fn a_page_turned_back_to_is_still_cached() {
        let ctx = egui::Context::default();
        let info = thumbnail_info(10);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        let req = |page| RenderRequest { page, scale: 1.0, tag: 1000, ..Default::default() };
        v.frame_visible = [0].into();
        v.frame_queue = vec![req(0)];
        v.last_queue = v.prepare_render_queue(&info, 1.0);
        v.receive_result(&ctx, solid_result(req(0), 100, 130));
        // Single-page view moves on to page 1: page 0 is no longer demanded.
        v.begin_render_frame();
        v.current = 1;
        v.frame_visible = [1].into();
        v.frame_queue = vec![req(1)];
        assert_eq!(v.prepare_render_queue(&info, 1.0), vec![req(1)]);
        assert!(v.pages.contains_key(&0), "kept while the allowance has room");
        // And back: nothing to render.
        v.begin_render_frame();
        v.current = 0;
        v.frame_visible = [0].into();
        v.frame_queue = vec![req(0)];
        assert!(v.prepare_render_queue(&info, 1.0).is_empty());
    }

    #[test]
    fn thumbnail_tags_do_not_change_as_a_row_scrolls_in_or_out() {
        let info = thumbnail_info(10_000);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        let tags = |v: &mut DocView, cells: usize| {
            v.begin_render_frame();
            for page in 0..cells {
                v.need_thumbnail(page, true);
            }
            v.thumbnail_requests(&info, 2.0).into_iter().map(|r| (r.page, r.tag)).collect::<HashMap<_, _>>()
        };
        // 2x on a large screen: about 70 cells, give or take a row of six.
        let (a, b) = (tags(&mut v, 70), tags(&mut v, 76));
        assert!((0..70).all(|p| a[&p] == b[&p]), "a row more or less re-renders nothing");
    }

    #[test]
    fn many_open_documents_share_one_raster_allowance_and_close_releases_handles() {
        let ctx = egui::Context::default();
        let mut app = PdfCraftApp::new();
        let bytes = app.session.create_blank(612.0, 792.0, 1).unwrap();
        let mut expected = Vec::new();
        for index in 0..6 {
            app.open_bytes(&format!("synthetic-{index}.pdf"), None, bytes.as_ref().clone()).unwrap();
            let info = app.session.get(app.views.last().unwrap().id).unwrap().info.clone();
            let view = app.views.last_mut().unwrap();
            view.begin_render_frame();
            view.need_thumbnail(0, true);
            let request = view.thumbnail_requests(&info, ctx.pixels_per_point()).into_iter().next().unwrap();
            let page = info.pages.get(request.page).unwrap();
            let dimensions = (device_pixels(page.width, request.scale), device_pixels(page.height, request.scale));
            view.last_queue = vec![request];
            assert_eq!(
                view.receive_result(&ctx, solid_result(request, dimensions.0, dimensions.1)),
                rgba_bytes(dimensions.0 as usize, dimensions.1 as usize)
            );
            assert_eq!(view.thumbs.get(&0).unwrap().tag, request.tag);
            assert_eq!(view.thumbs.get(&0).unwrap().tex.size(), [dimensions.0 as usize, dimensions.1 as usize]);

            view.begin_render_frame();
            view.need_thumbnail(0, true);
            let queue = view.prepare_render_queue(&info, ctx.pixels_per_point());
            assert!(queue.is_empty(), "the synthetic thumbnail satisfies current demand");
            view.last_queue = queue;
            expected.push((request, dimensions));
        }
        assert_eq!(app.views.len(), 6, "all six documents were opened");
        assert_eq!(app.active_ids().map(|(index, _)| index), Some(5));
        assert!(app.views.iter().zip(&expected).all(|(view, (request, dimensions))| {
            view.thumbs.len() == 1
                && view
                    .thumbs
                    .get(&0)
                    .is_some_and(|thumb| thumb.tag == request.tag && thumb.tex.size() == [dimensions.0 as usize, dimensions.1 as usize])
        }));

        app.finish_render_frame(&ctx);
        let total: usize = app.views.iter().map(|v| v.thumbs.values().map(|p| texture_bytes(&p.tex)).sum::<usize>()).sum();
        let (active_request, active_dimensions) = expected.last().unwrap();
        let active_bytes = rgba_bytes(active_dimensions.0 as usize, active_dimensions.1 as usize);
        assert_eq!(total, active_bytes, "only the active view retains a thumbnail");
        assert!(total <= THUMB_BYTES, "retained thumbnails stay within the existing allowance");
        assert!(app.views.iter().take(5).all(|v| v.thumbs.is_empty() && v.last_queue.is_empty()));
        let active = app.views.last().unwrap();
        assert_eq!(active.thumbs.len(), 1);
        assert_eq!(active.thumbs.get(&0).unwrap().tag, active_request.tag);
        assert!(active.last_queue.is_empty(), "the current thumbnail is already cached");
        for view in &app.views {
            let doc = app.session.get(view.id).unwrap();
            assert_eq!(doc.renderer.stats().render_requests, 0, "the fixture must not launch a native render");
        }

        app.close_tab(5);
        assert_eq!(app.views.len(), 5, "closing the active tab drops its owning view");
        assert!(app.views.iter().all(|v| v.thumbs.is_empty()));
    }

    #[test]
    fn suspended_signature_preview_releases_layers_and_rebuilds_without_editing() {
        let ctx = egui::Context::default();
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(12, 4, image::Rgba([20, 30, 40, 255])).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let image = pdfcraft_engine::SignatureImage::from_bytes(png.get_ref()).unwrap();
        let mut session = pdfcraft_engine::Session::new();
        let blank = session.create_blank(300.0, 400.0, 1).unwrap();
        let id = session.open_new("signature-preview.pdf", blank).unwrap();
        let info = session.get(id).unwrap().info.pages[0].clone();
        session.apply(id, image.edit(0, &info, [20.0, 100.0], false, "Test").unwrap()).unwrap();
        let doc = session.get(id).unwrap();
        let bytes = doc.bytes.clone();
        let generation = doc.edit_generation();
        let mut v = DocView::new(id, &doc.info, ViewDefaults::default());
        v.comments.selected = Some((0, 0));
        v.signature_drag.prepare(&ctx, doc, v.comments.selected, 1.0);
        assert!(v.signature_drag.contains(0, 0));

        v.suspend_rendering(&doc.renderer);
        assert!(!v.signature_drag.contains(0, 0), "inactive tabs release the preview pool and textures");
        assert_eq!(v.comments.selected, Some((0, 0)), "the user's selection survives suspension");
        v.signature_drag.prepare(&ctx, doc, v.comments.selected, 1.0);
        assert!(v.signature_drag.contains(0, 0), "reactivating the view rebuilds the selected image's preview");
        assert!(Arc::ptr_eq(&bytes, &doc.bytes));
        assert_eq!(doc.edit_generation(), generation);
        assert_eq!(doc.can_undo(), Some("Add signature"));
        assert!(doc.dirty);
    }

    #[test]
    fn suspending_a_view_releases_owned_raster_textures_and_queue_state() {
        let ctx = egui::Context::default();
        let mut view = view(2, PageLayout::Continuous);
        let texture = |name| ctx.load_texture(name, egui::ColorImage::filled([1, 1], Color32::WHITE), TextureOptions::LINEAR);
        let request = RenderRequest { page: 0, scale: 1.0, tag: 1, ..Default::default() };
        view.pages.insert(0, PageTex { tag: request.tag, tex: texture("suspended-page") });
        view.thumbs.insert(1, PageTex { tag: THUMB_TAG, tex: texture("suspended-thumbnail") });
        view.tiles.insert((0, request.tag, 0, 0), texture("suspended-tile"));
        view.grid_pages.insert(1, (1, texture("suspended-grid-page")));
        view.print_pages.insert(1, (PRINT_TAG, texture("suspended-print-page")));
        view.stale_thumbs.insert(1);
        view.waiting_since.insert(0, 1.0);
        view.last_queue = vec![request];
        view.frame_queue = vec![request];
        view.frame_visible.insert(0);
        view.need_thumbnail(1, true);

        let pool = RenderPool::new_inline(Arc::new(Vec::new()), Default::default());
        view.suspend_rendering(&pool);

        assert!(view.pages.is_empty(), "page handles are released");
        assert!(view.thumbs.is_empty(), "thumbnail handles are released");
        assert!(view.tiles.is_empty(), "tile handles are released");
        assert!(view.grid_pages.is_empty(), "organize-grid handles are released");
        assert!(view.print_pages.is_empty(), "print-preview handles are released");
        assert!(view.stale_thumbs.is_empty(), "stale thumbnail bookkeeping is cleared");
        assert!(view.waiting_since.is_empty(), "waiting bookkeeping is cleared");
        assert!(view.last_queue.is_empty() && !view.render_pending(), "queued demand is retired");
        assert!(view.frame_queue.is_empty() && view.frame_visible.is_empty() && view.thumb_demand.is_empty());
    }

    #[test]
    fn virtualized_organizer_keeps_insert_gaps_at_document_positions() {
        use egui_kittest::kittest::Queryable;

        let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(|_cc| {
            let mut app = PdfCraftApp::new();
            let bytes = app.session.create_blank(300.0, 400.0, 100).unwrap();
            app.open_bytes("organize-gaps.pdf", None, bytes.as_ref().clone()).unwrap();
            app.views[0].organize = true;
            app
        });
        h.run_steps(4);
        assert!(h.state().views[0].thumb_demand.len() < 50);
        assert_eq!(h.query_all_by_label("Insert a file at the end").count(), 0, "the last virtualized cell is not the final page");
        let center = h.state().views[0].viewport_rect().center();
        h.hover_at(center);
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -100_000.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        // Well past egui's longest scroll animation (about 0.3 s).
        h.run_steps(40);
        h.get_by_label("Insert a file at the end");
        let before_last = h.get_by_label("Insert a file before page 100").rect().center();
        h.hover_at(before_last);
        h.run_steps(2);
        assert_eq!(h.state().views[0].grid_gap, Some(99), "external drops use the document index after virtualized scrolling");
        assert!(h.state().views[0].thumb_demand.len() < 50);
    }

    #[test]
    fn real_shell_pages_organizer_and_print_only_collect_current_thumbnail_consumers() {
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(|_cc| {
            let mut app = PdfCraftApp::new();
            let bytes = app.session.create_blank(612.0, 792.0, 10_000).unwrap();
            app.open_bytes("synthetic-ten-thousand.pdf", None, bytes.as_ref().clone()).unwrap();
            app.right = Some(RightPanel::Pages);
            app
        });
        h.run_steps(4);
        assert!((1..20).contains(&h.state().views[0].thumb_demand.len()));
        h.state_mut().right = None;
        h.state_mut().views[0].organize = true;
        h.run_steps(2);
        assert!((1..50).contains(&h.state().views[0].thumb_demand.len()));
        h.state_mut().views[0].organize = false;
        h.state_mut().dialog = Some(crate::Dialog::Print);
        h.state_mut().print_draft.sheet = 9999;
        h.run_steps(2);
        assert_eq!(h.state().views[0].thumb_demand.keys().copied().collect::<Vec<_>>(), vec![9999]);
        h.state_mut().print_draft.handling = crate::PrintHandling::Booklet;
        h.state_mut().print_draft.sheet = 0;
        h.run_steps(2);
        assert_eq!(h.state().views[0].thumb_demand.len(), 2, "booklet only needs its two placed pages");
        h.state_mut().dialog = None;
        h.run_steps(2);
        assert!(h.state().views[0].thumb_demand.is_empty());
        assert!(h.state().views[0].thumbs.is_empty());
    }

    /// One mouse-wheel notch (a line) at `at` seconds.
    fn notch(v: &mut DocView, dy: f32, at: f64) -> bool {
        v.single_page_wheel(egui::MouseWheelUnit::Line, dy, egui::TouchPhase::Move, at, true)
    }

    #[test]
    fn thumbnails_cover_the_panel_on_a_high_dpi_screen() {
        // Pages panel slot is at most 150 logical points. At 2 px/pt a letter page's thumbnail
        // must be at least that wide, so the panel samples it down instead of stretching it.
        let info = thumbnail_info(1);
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        v.need_thumbnail(0, true);
        let queue = v.prepare_render_queue(&info, 2.0);
        let scale = queue.first().expect("one thumbnail").scale;
        assert!(612.0 * scale >= 150.0 * 2.0, "{scale}");
    }

    #[test]
    fn notches_turn_pages_and_stop_at_the_ends() {
        let mut v = view(3, PageLayout::Single);
        assert!(notch(&mut v, -1.0, 1.0) && notch(&mut v, -1.0, 1.05));
        assert_eq!(v.current, 2);
        assert!(!notch(&mut v, -1.0, 1.1), "nothing turns past the last page");
        assert!(notch(&mut v, 1.0, 1.2));
        assert_eq!(v.current, 1);
    }

    #[test]
    fn wheel_is_single_page_only() {
        for layout in [PageLayout::Continuous, PageLayout::TwoUp] {
            let mut v = view(3, layout);
            assert!(!notch(&mut v, -1.0, 1.0), "{layout:?} ignores the wheel");
            assert_eq!(v.current, 0);
        }
    }

    #[test]
    fn navigation_drops_partial_wheel_motion() {
        use egui::{MouseWheelUnit::Point, TouchPhase::Move, TouchPhase::Start};
        let mut v = view(3, PageLayout::Single);
        assert!(!v.single_page_wheel(Point, 0.0, Start, 1.0, true));
        assert!(!v.single_page_wheel(Point, -30.0, Move, 1.02, true));
        v.go_to_page(1);
        // The same touch has to move a full notch's worth again before it turns.
        assert!(!v.single_page_wheel(Point, -30.0, Move, 1.04, true));
        assert!(v.single_page_wheel(Point, -30.0, Move, 1.06, true));
        assert_eq!(v.current, 2);
    }

    #[test]
    fn reselecting_the_layout_does_not_scroll_to_top() {
        let mut v = view(3, PageLayout::Continuous);
        v.goto = None;
        v.set_layout(PageLayout::Continuous);
        assert!(v.goto.is_none(), "re-selecting the current layout is a no-op");
        v.set_layout(PageLayout::Single);
        assert_eq!(v.goto, Some((0, 0.0)));
    }

    #[test]
    fn fit_page_and_height_hold_still_while_mixed_page_sizes_scroll_past() {
        let page =
            |width: f32, height: f32| pdfcraft_render::PageInfo { width, height, label: String::new(), crop: [0.0, 0.0, width, height], rotation: 0 };
        let info = DocInfo { pages: vec![page(300.0, 400.0), page(600.0, 800.0)], ..Default::default() };
        for fit in [Fit::Page, Fit::Height] {
            let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
            (v.fit, v.viewport_w, v.viewport_h) = (fit, 1000.0, 800.0);
            v.fit_zoom(&info);
            let zoom = v.zoom;
            v.current = 1;
            v.fit_zoom(&info);
            assert_eq!(v.zoom, zoom, "{fit:?}: scrolling onto a bigger page keeps the zoom");
            // Single-page view shows one page at a time, so it fits each one.
            v.layout = PageLayout::Single;
            v.fit_zoom(&info);
            let big = v.zoom;
            v.current = 0;
            v.fit_zoom(&info);
            assert!(v.zoom > big, "{fit:?}: single-page view fits the page it shows");
        }
    }

    #[test]
    fn fit_width_fits_the_shown_page_in_single_page_view_only() {
        let page =
            |width: f32, height: f32| pdfcraft_render::PageInfo { width, height, label: String::new(), crop: [0.0, 0.0, width, height], rotation: 0 };
        let info = DocInfo { pages: vec![page(300.0, 400.0), page(600.0, 400.0)], ..Default::default() };
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        (v.fit, v.viewport_w, v.viewport_h) = (Fit::Width, 1000.0, 800.0);
        v.layout = PageLayout::Single;
        v.fit_zoom(&info);
        let narrow = v.zoom;
        // The shown page fills the width between the side gutters, not centred for the wide page.
        let shown = v.layout(&info, v.content_width(&info, 1000.0))[0];
        assert!((shown.left() - SIDE).abs() < 1e-2 && (shown.width() - 860.0).abs() < 1e-2, "{shown:?}");
        v.current = 1;
        v.fit_zoom(&info);
        assert!((narrow / v.zoom - 2.0).abs() < 1e-3, "the narrow page gets twice the zoom of the wide one");
        // Scrolling views keep one zoom for the whole document.
        v.layout = PageLayout::Continuous;
        v.fit_zoom(&info);
        let wide = v.zoom;
        v.current = 0;
        v.fit_zoom(&info);
        assert_eq!(v.zoom, wide, "continuous fit width holds still across page sizes");
    }

    #[test]
    fn layouts_match_their_commands_and_names() {
        for l in PageLayout::ORDER {
            let spec = pdfcraft_engine::commands::command(l.command()).expect("registered");
            assert_eq!((l.label(), l.icon()), (spec.label, spec.icon), "{l:?}");
            assert_eq!(PageLayout::from_command(l.command()), Some(l));
            assert_eq!(PageLayout::try_parse(l.as_str()), Some(l));
        }
        assert_eq!(PageLayout::try_parse(" Two-Up "), Some(PageLayout::TwoUp));
        assert_eq!(PageLayout::try_parse("facing"), None);
    }

    #[test]
    fn destinations_set_the_zoom_mode_and_the_point_to_show() {
        let info = pdfcraft_render::DocInfo {
            pages: (0..3)
                .map(|_| pdfcraft_render::PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 })
                .collect(),
            ..Default::default()
        };
        let mut v = view(3, PageLayout::Continuous);
        v.fit = Fit::Width;
        // No position (or a malformed one): the top of the page, zoom as it was.
        v.go_to_dest(1, DestView::Top, &info);
        assert_eq!((v.current, v.goto, v.goto_point, v.fit), (1, Some((1, 0.0)), None, Fit::Width));
        // /XYZ with a null zoom keeps the zoom mode; its point is a quarter down page 3.
        v.go_to_dest(2, DestView::Xyz { left: None, top: Some(300.0), zoom: None }, &info);
        assert_eq!((v.current, v.fit, v.goto_point), (2, Fit::Width, Some((2, None, Some(0.25), vec2(SIDE, 0.0)))));
        v.go_to_dest(0, DestView::Xyz { left: Some(150.0), top: None, zoom: Some(2.5) }, &info);
        assert_eq!((v.fit, v.zoom, v.goto_point), (Fit::None, 2.5, Some((0, Some(0.5), None, vec2(SIDE, 0.0)))));
        // A zoom beyond the view's range is clamped to it.
        v.go_to_dest(0, DestView::Xyz { left: None, top: None, zoom: Some(1e30) }, &info);
        assert_eq!((v.zoom, v.goto_point), (64.0, None));
        v.go_to_dest(0, DestView::Fit, &info);
        assert_eq!((v.fit, v.goto_point), (Fit::Page, None));
        v.go_to_dest(0, DestView::FitH { top: Some(100.0) }, &info);
        assert_eq!((v.fit, v.goto_point), (Fit::Width, Some((0, None, Some(0.75), vec2(SIDE, 0.0)))));
        v.go_to_dest(0, DestView::FitV { left: Some(75.0) }, &info);
        assert_eq!((v.fit, v.goto_point), (Fit::Height, Some((0, Some(0.25), None, vec2(SIDE, 0.0)))));
        // /FitR: the zoom that fits a 100×50 pt rectangle in an 800×600 window, centred.
        (v.viewport_w, v.viewport_h) = (800.0, 600.0);
        v.go_to_dest(0, DestView::FitR { rect: [0.0, 300.0, 100.0, 350.0] }, &info);
        assert_eq!(v.fit, Fit::None);
        assert!((v.zoom - 8.0 / PT).abs() < 1e-4, "{}", v.zoom);
        let (page, fx, fy, rel) = v.goto_point.expect("a point to show");
        assert_eq!((page, rel), (0, vec2(400.0, 300.0)));
        assert!((fx.unwrap_or(-1.0) - 50.0 / 300.0).abs() < 1e-4 && (fy.unwrap_or(-1.0) - 75.0 / 400.0).abs() < 1e-4, "{fx:?} {fy:?}");
        // A page past the end goes to the last page; a page the document info lacks, to its top.
        v.go_to_dest(usize::MAX, DestView::FitH { top: Some(100.0) }, &info);
        assert_eq!((v.current, v.goto_point.map(|g| g.0)), (2, Some(2)));
        let empty = pdfcraft_render::DocInfo::default();
        v.go_to_dest(1, DestView::FitH { top: Some(100.0) }, &empty);
        assert_eq!((v.current, v.goto_point), (1, None));
    }
}
