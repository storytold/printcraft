//! Combine files, in its own tab: the files to combine, in order, each with the pages to take
//! (all, or a range such as "1-3, 6"), its size, when it was modified and anything that stops it
//! being combined. Rows are selected like files in a file manager (click, Ctrl/⌘+click,
//! Shift+click), moved with the toolbar, the keyboard or by dragging, and sorted by clicking a
//! column heading; PDFs dropped on the tab join the list. Every change to the list can be undone.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::SystemTime;

use egui::{Align, Color32, CornerRadius, Layout, Modifiers, Rect, Sense, Stroke, vec2};
use egui_extras::{Column, TableBuilder};
use pdfcraft_engine::SourceProblem;

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, icons, widgets};

/// Same reds and ambers as the signature status (`sign_ui`).
pub(crate) const ERROR: Color32 = Color32::from_rgb(0xD7, 0x37, 0x3F);
pub(crate) const WARNING: Color32 = Color32::from_rgb(0xE6, 0x86, 0x19);
/// Undo steps kept for the list (each holds the list itself; the files' bytes are shared).
const HISTORY: usize = 100;

/// Files found in a folder are added up to this many at a time.
#[cfg(not(target_arch = "wasm32"))]
const MAX_FOLDER_FILES: usize = 1000;
/// How deep "Add folder and subfolders" looks.
#[cfg(not(target_arch = "wasm32"))]
const MAX_FOLDER_DEPTH: usize = 16;

/// A column of the list: what it shows and what it sorts by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortKey {
    Name,
    Pages,
    Size,
    Modified,
    Warnings,
}

impl SortKey {
    pub const ALL: [SortKey; 5] = [SortKey::Name, SortKey::Pages, SortKey::Size, SortKey::Modified, SortKey::Warnings];

    fn index(self) -> usize {
        self as usize
    }

    fn label(self) -> &'static str {
        match self {
            SortKey::Name => tl!("File name"),
            SortKey::Pages => tl!("Pages"),
            SortKey::Size => tl!("Size"),
            SortKey::Modified => tl!("Modified"),
            SortKey::Warnings => tl!("Warnings"),
        }
    }

    /// The narrowest the column may be dragged.
    fn min_width(self) -> f32 {
        match self {
            SortKey::Name => 120.0,
            SortKey::Pages => 110.0,
            SortKey::Size => 60.0,
            SortKey::Modified => 80.0,
            SortKey::Warnings => 100.0,
        }
    }
}

/// The columns' order and widths, as the user arranged them (kept in the settings).
#[derive(Clone, Debug, PartialEq)]
pub struct Columns {
    pub order: [SortKey; 5],
    /// By [`SortKey::index`]; `None` until a column is resized: fitted to the window until then.
    pub widths: Option<[f32; 5]>,
}

impl Default for Columns {
    fn default() -> Self {
        Columns { order: SortKey::ALL, widths: None }
    }
}

impl Columns {
    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "order": self.order, "widths": self.widths })
    }

    /// Settings written by [`Self::to_json`]; anything malformed gives the default layout.
    pub(crate) fn from_json(v: &serde_json::Value) -> Self {
        let order = serde_json::from_value::<Vec<SortKey>>(v["order"].clone())
            .ok()
            .filter(|o| SortKey::ALL.iter().all(|k| o.contains(k)))
            .and_then(|o| <[SortKey; 5]>::try_from(o).ok())
            .unwrap_or(SortKey::ALL);
        let widths = serde_json::from_value::<[f32; 5]>(v["widths"].clone())
            .ok()
            .filter(|w| w.iter().all(|x| x.is_finite()))
            .map(|w| std::array::from_fn(|i| w[i].clamp(SortKey::ALL[i].min_width(), 4000.0)));
        Columns { order, widths }
    }

    /// The widths to show: the user's, or the window's width shared out.
    fn resolved(&self, available: f32) -> [f32; 5] {
        if let Some(w) = self.widths {
            return w;
        }
        let fixed = 170.0 + 84.0 + 110.0;
        let flexible = (available - fixed).max(260.0);
        [(flexible * 0.55).max(140.0), 170.0, 84.0, 110.0, (flexible * 0.45).max(120.0)]
    }
}

/// How the files are shown: a thumbnail of each (the default, to see what goes where) or the
/// table (kept in the settings).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CombineView {
    #[default]
    Grid,
    List,
}

impl CombineView {
    pub fn as_str(self) -> &'static str {
        match self {
            CombineView::Grid => "grid",
            CombineView::List => "list",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "grid" => Some(CombineView::Grid),
            "list" => Some(CombineView::List),
            _ => None,
        }
    }
}

/// A column heading being dragged to a new place.
#[derive(Clone, Copy)]
struct HeadingDrag(SortKey);

enum HeadingEvent {
    Sort(SortKey),
    /// Put column `kind` before (or after) column `to`.
    Move {
        kind: SortKey,
        to: SortKey,
        after: bool,
    },
    Reset,
}

/// The Combine files tab. It shows in the Home slot (`active == None`) while `focused`.
#[derive(Clone, Debug, Default)]
pub struct CombineTab {
    pub open: bool,
    pub focused: bool,
    /// The selected rows, by [`CombineFile::id`], so a selection survives sorting and moving.
    pub selected: BTreeSet<u64>,
    /// Where a Shift+click or Shift+arrow range starts, and where it ends.
    anchor: Option<u64>,
    cursor: Option<u64>,
    /// The column the list is sorted by and whether ascending; cleared when rows are moved.
    pub sort: Option<(SortKey, bool)>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// The list as it was when a Pages field took the keyboard, recorded on its first change.
    range_edit: Option<Snapshot>,
    next_id: u64,
    /// Bumped to make the table take its column widths afresh (columns moved or reset, or the
    /// window resized while the widths follow it).
    layout: u64,
    /// The width the columns were last fitted to.
    fitted: f32,
    /// The password box of Unlock…, while it shows.
    unlock: Option<UnlockPrompt>,
    /// Bumped on every change to the list, so a grid drag that began before one is dropped.
    pub(crate) revision: u64,
    /// The file the keyboard moved to, for the grid to scroll into view.
    pub(crate) reveal: Option<u64>,
    /// Files per grid row as last drawn (Up/Down move a row in the grid).
    pub(crate) grid_columns: usize,
    /// A grid zoom asked for this frame (toolbar, keys, pinch); the grid applies it once it has
    /// laid out, so it can keep the same cards in view.
    pub(crate) zoom_request: Option<f32>,
    /// The card size slider, which takes the arrow keys while it has the keyboard.
    pub(crate) zoom_slider: Option<egui::Id>,
    /// The card kept in place through a zoom gesture: its file, its place among the file's pages
    /// (0 for the file's own card), how far below the top of the grid it was, and when the grid
    /// last zoomed (seconds).
    pub(crate) zoom_anchor: Option<(u64, usize, f32, f64)>,
    /// Files shown as their pages in the grid (a view, not an edit: no undo step).
    pub(crate) expanded: BTreeSet<u64>,
    /// The pages each expanded file shows, worked out again only when its range changes.
    pub(crate) page_lists: HashMap<u64, crate::combine_grid::PageList>,
    /// The file (by place in the list) at each place of the grid as last drawn: Up and Down step
    /// a row of cards, page cards included.
    pub(crate) grid_places: Vec<usize>,
    /// A file shown large (a card's magnifier), while it shows.
    pub(crate) preview: Option<Preview>,
    /// The card that had the keyboard last frame: Space shows its file large; on any other focused
    /// widget Space is that widget's.
    pub(crate) card_focus: Option<egui::Id>,
    /// That card's file and its place among the file's pages (0 for the file's own card): Space
    /// on a page card shows that page.
    pub(crate) card_focus_at: Option<(u64, usize)>,
    /// Single pages selected in the grid (an expanded file's page cards), by entry id and place
    /// among the entry's pages; the files' selection is empty while any is.
    pub(crate) selected_pages: BTreeSet<(u64, usize)>,
    /// The page a Shift-click selects from.
    pub(crate) page_anchor: Option<(u64, usize)>,
}

/// The pages a range takes, and the range and page count they were worked out for.
pub(crate) type PagesTaken = ((String, usize), Arc<[usize]>);

/// A file shown large: which, the place among the pages it adds, and those pages (worked out
/// again only when its page range or page count changes).
#[derive(Clone, Debug)]
pub(crate) struct Preview {
    pub(crate) file: u64,
    pub(crate) at: usize,
    /// The page (0-based, in the file) on screen, for the thumbnails to render.
    pub(crate) page: Option<usize>,
    pub(crate) pages: Option<PagesTaken>,
}

/// A password: kept in memory only, never printed (debug output ends up in logs).
#[derive(Clone, Default)]
pub struct Secret(String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_empty() { "\"\"" } else { "…" })
    }
}

/// Which password a protected file still needs before it can be combined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lock {
    /// The password that opens it.
    Open,
    /// It opens, but only its permissions (owner) password allows copying its pages.
    Permissions,
}

/// Unlock…'s password box: the rows it unlocks and what has been typed.
#[derive(Clone, Debug)]
struct UnlockPrompt {
    rows: Vec<u64>,
    input: Secret,
    show: bool,
    error: Option<String>,
    /// Where the box opens (below the link or button that asked for it).
    at: egui::Pos2,
    focused: bool,
}

/// What can be known about a file up front, opened with a password or not.
#[derive(Clone)]
struct Assessed {
    pages: usize,
    /// Each page's displayed size in points (empty when the inspector couldn't read it).
    sizes: Arc<[(f32, f32)]>,
    problem: Option<String>,
    lock: Option<Lock>,
    notes: Vec<String>,
}

enum AssessError {
    WrongPassword,
    Unreadable(String),
}

/// Read `bytes` (with `password`, if given) for the list: pages, what stops it being combined
/// and which password would help, and warnings.
fn assess(bytes: &Arc<Vec<u8>>, password: Option<&str>) -> Result<Assessed, AssessError> {
    // The engine that combines decides whether the password works; the inspector (which may
    // not accept every password the engine does, e.g. an RC4 owner password) adds the details.
    let check = pdfcraft_engine::combine_source_check(bytes, password);
    if check == Err(SourceProblem::WrongPassword) {
        return Err(AssessError::WrongPassword);
    }
    let info = pdfcraft_render::inspect(bytes.clone(), password);
    if let (Err(SourceProblem::Unreadable(e)), Err(_)) = (&check, &info) {
        return Err(AssessError::Unreadable(e.clone()));
    }
    let info = info.ok();
    let pages = match &info {
        Some(i) => i.pages.len(),
        None => pdfcraft_engine::source_page_count(bytes, password).unwrap_or(0),
    };
    let sizes: Arc<[(f32, f32)]> = match &info {
        Some(i) => i.pages.iter().map(|p| (p.width, p.height)).collect(),
        None => Arc::from(Vec::new()),
    };
    let encrypted = password.is_some() || info.as_ref().is_some_and(|i| i.encrypted);
    let warnings = info.map(|i| i.warnings).unwrap_or_default();
    let (problem, lock) = match check {
        Ok(()) => (None, None),
        Err(SourceProblem::Password) => (Some(tl!("Password-protected").to_string()), Some(Lock::Open)),
        Err(SourceProblem::WrongPassword) => return Err(AssessError::WrongPassword),
        Err(SourceProblem::NotPermitted) if password.is_some() => (
            Some(tl!("Opened, but its security settings don't allow combining. Enter the permissions password.").to_string()),
            Some(Lock::Permissions),
        ),
        Err(SourceProblem::NotPermitted) => (Some(tl!("Its security settings don't allow copying pages").to_string()), Some(Lock::Permissions)),
        Err(SourceProblem::Unreadable(e)) => (Some(crate::i18n::fmt(tl!("Can't be read: {e}"), &[("e", &e)])), None),
    };
    let mut notes = Vec::new();
    // The combined file is a new, unprotected document: protection doesn't carry over.
    if encrypted && problem.is_none() {
        notes.push(if password.is_some() {
            tl!("Unlocked: the combined file won't be password-protected").to_string()
        } else {
            tl!("Protected: the combined file won't keep its security settings").to_string()
        });
    }
    notes.extend(warnings.into_iter().take(5));
    Ok(Assessed { pages, sizes, problem, lock, notes })
}

#[derive(Clone, Debug)]
struct Snapshot {
    files: Vec<CombineFile>,
    selected: BTreeSet<u64>,
    sort: Option<(SortKey, bool)>,
}

/// A file arriving in the list (picked, dropped or already open).
pub(crate) struct Incoming {
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    pub modified: Option<SystemTime>,
    /// Something to tell the user about where it came from.
    pub note: Option<String>,
}

/// A new value for [`CombineFile::auth`]: counted for the whole run, outside the undo history.
fn next_auth() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// A page range checked: the pages it takes and the first of them (0-based), or why it's wrong.
type RangeCheck = Result<(usize, usize), String>;

#[derive(Clone, Debug)]
pub struct CombineFile {
    /// Stable while the file is in the list (selection, undo).
    pub id: u64,
    /// The file it is (part of): the entries a file is split into (a page moved between another
    /// file's pages) share it. Combined as one file, unlocked together, counted once.
    pub(crate) group: u64,
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    pub pages: usize,
    /// Each page's displayed size in points; shared, so undo snapshots don't copy it.
    pub(crate) sizes: Arc<[(f32, f32)]>,
    /// How the file was read (with which password): a new value every time it is read, never
    /// reused, not even after undo, so a thumbnail made another way is never taken for this one.
    pub(crate) auth: u64,
    /// The pages to take ("" = all).
    pub range: String,
    /// When the file was last modified (desktop picks and drops only).
    pub modified: Option<SystemTime>,
    /// Why the file can't be combined (password, security settings), if so.
    pub problem: Option<String>,
    /// The password that would let it be combined, while one is needed.
    pub lock: Option<Lock>,
    /// The password it was unlocked with.
    password: Option<Secret>,
    /// Warnings that don't stop it being combined.
    pub notes: Vec<String>,
    /// Where it came from, shown among the warnings (e.g. an open document's unsaved changes).
    origin_note: Option<String>,
    /// The range last checked and the result: the number of pages it takes and the first of
    /// them (0-based), or why it is wrong.
    checked: Option<(String, RangeCheck)>,
}

impl CombineFile {
    fn take(&mut self, a: Assessed, password: Option<Secret>) {
        self.pages = a.pages;
        self.sizes = a.sizes;
        self.problem = a.problem;
        self.lock = a.lock;
        self.password = password;
        self.auth = next_auth();
        self.notes = self.origin_note.iter().cloned().chain(a.notes).collect();
        self.checked = None;
    }

    /// The password the file was unlocked with, if any.
    pub(crate) fn password(&self) -> Option<&str> {
        self.password.as_ref().map(|p| p.0.as_str())
    }

    /// The number of pages `range` takes, or why it can't be used. Checked again only when the
    /// range changes.
    pub fn selection(&mut self) -> Result<usize, String> {
        self.checked_range().map(|(count, _)| count)
    }

    /// The first page (0-based) the range takes, in its order ("3, 1" starts with page 3): the
    /// page the grid shows for the file.
    pub fn first_page(&mut self) -> Result<usize, String> {
        self.checked_range().map(|(_, first)| first)
    }

    /// The first page the range takes, without updating the cache (read-only callers).
    pub(crate) fn first_taken(&self) -> Option<usize> {
        match &self.checked {
            Some((range, result)) if *range == self.range => result.as_ref().ok().map(|(_, first)| *first),
            _ => self.pages_taken().ok()?.first().copied(),
        }
    }

    /// Every page (0-based) the range takes, in its order: what the preview flips through.
    pub(crate) fn pages_taken(&self) -> Result<Vec<usize>, String> {
        if self.range.trim().is_empty() {
            return Ok((0..self.pages).collect());
        }
        match pdfcraft_engine::print::select_pages(self.pages, Some(&self.range), &[], pdfcraft_engine::print::Subset::All, false) {
            Ok(p) if p.is_empty() => Err(tl!("No pages selected").to_string()),
            Ok(p) => Ok(p),
            Err(e) => Err(e.to_string()),
        }
    }

    fn checked_range(&mut self) -> RangeCheck {
        if let Some((range, result)) = &self.checked
            && *range == self.range
        {
            return result.clone();
        }
        let result = if self.range.trim().is_empty() {
            // (A file whose pages couldn't be counted is left to the engine, as before; the grid
            // checks the first page against the page count.)
            Ok((self.pages, 0))
        } else {
            match pdfcraft_engine::print::select_pages(self.pages, Some(&self.range), &[], pdfcraft_engine::print::Subset::All, false) {
                Ok(p) => match p.first() {
                    Some(first) => Ok((p.len(), *first)),
                    None => Err(tl!("No pages selected").to_string()),
                },
                Err(e) => Err(e.to_string()),
            }
        };
        self.checked = Some((self.range.clone(), result.clone()));
        result
    }

    /// 0 = can't be combined, 1 = has warnings, 2 = fine: the Warnings column's sort order.
    fn severity(&mut self) -> u8 {
        if self.problem.is_some() || self.selection().is_err() {
            0
        } else if !self.notes.is_empty() {
            1
        } else {
            2
        }
    }
}

#[derive(Clone)]
pub(crate) enum RowAction {
    /// A click on row `i` (Ctrl/⌘ toggles it, Shift selects up to it).
    Click(usize, Modifiers),
    SelectAll,
    /// An arrow key: `by` files on (1 in the list, or along a grid row; a grid row's length for
    /// Up/Down in the grid). With Shift the selection grows.
    Step {
        down: bool,
        by: usize,
        extend: bool,
    },
    MoveUp,
    MoveDown,
    Remove,
    /// One file's card trash: that file only, whatever else is selected.
    RemoveFile(u64),
    /// One file's magnifier: show it large.
    Preview(u64),
    /// Space: show the file the keyboard is on (or the first selected) large.
    PreviewSelected,
    /// A page card's magnifier: show its file large, from that page (its place among the pages
    /// the file adds).
    PreviewAt(u64, usize),
    /// Show one file as its pages in the grid (`true`), or as its card again.
    Expand(u64, bool),
    /// Every file that adds more than one page, as its pages (`true`), or all as cards again.
    ExpandAll(bool),
    /// A click on a page card: that page (by entry id and place among its pages) selected,
    /// Ctrl/⌘ toggling it, Shift selecting up to it.
    ClickPage(u64, usize, Modifiers),
    /// A page card (and the rest of the selected pages, if it is selected) dropped before page
    /// `gap` of the list (see [`moved_pages`]), if the list hasn't changed since the drag began.
    MovePages {
        page: (u64, usize),
        revision: u64,
        gap: usize,
    },
    /// One page's trash (`Some`), or the selected pages: out of the list.
    RemovePages(Option<(u64, usize)>),
    /// Row `from` (and the rest of the selection, if it is selected) dropped on row `to`.
    Drop {
        from: usize,
        to: usize,
    },
    /// A grid card (and the rest of the selection, if it is selected) dropped at `gap`
    /// (0 = before the first file, n = after the last), if the list hasn't changed since the
    /// drag began.
    DropAt {
        file: u64,
        revision: u64,
        gap: usize,
    },
    Sort(SortKey),
    /// Unlock… on row `i` (and the rest of the selection, if it is selected), or on the
    /// selection (`None`); the password box opens at the position.
    Unlock(Option<usize>, egui::Pos2),
}

/// What the table reports besides row actions.
#[derive(Default)]
struct TableEvents {
    action: Option<RowAction>,
    range_focused: bool,
    range_changed: bool,
}

/// The Combine files page.
pub(crate) fn page(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let files_hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
    // Ranges are checked before anything below reads them.
    let checks: Vec<Result<usize, String>> = app.combine_draft.iter_mut().map(CombineFile::selection).collect();
    let n = app.combine_draft.len();
    let ids: Vec<u64> = app.combine_draft.iter().map(|f| f.id).collect();
    app.combine_tab.selected.retain(|id| ids.contains(id));
    // Pages stay picked only while they show (a grid card of an expanded file), so the trash or
    // Delete never takes a page the user can't see.
    {
        let tab = &mut app.combine_tab;
        if app.combine_view == CombineView::Grid {
            let shown: BTreeSet<(u64, usize)> = app
                .combine_draft
                .iter_mut()
                .filter(|f| tab.expanded.contains(&f.id))
                .flat_map(|f| {
                    let id = f.id;
                    (0..f.selection().unwrap_or(0)).map(move |nth| (id, nth))
                })
                .collect();
            tab.selected_pages.retain(|p| shown.contains(p));
        } else {
            tab.selected_pages.clear();
        }
        if tab.selected_pages.is_empty() {
            tab.page_anchor = None;
        }
    }
    // A file stays expanded only while it can show as its pages (not locked again by undo, nor
    // edited down to one page or a bad range): it doesn't come back expanded by itself.
    {
        let tab = &mut app.combine_tab;
        let draft = &app.combine_draft;
        let lists = &mut tab.page_lists;
        tab.expanded.retain(|id| draft.iter().find(|f| f.id == *id).is_some_and(|f| crate::combine_grid::pages_shown(lists, f).is_some()));
        let expanded = &tab.expanded;
        tab.page_lists.retain(|id, _| expanded.contains(id));
    }
    let selected: Vec<bool> = ids.iter().map(|id| app.combine_tab.selected.contains(id)).collect();
    let count = selected.iter().filter(|s| **s).count();
    let blocker = blocker(&app.combine_draft, &checks);
    let grid = app.combine_view == CombineView::Grid;
    let columns = grid.then_some(app.combine_tab.grid_columns);
    // The keyboard is the list's unless a text field, the password box or the size slider has it.
    let slider_focused = grid && app.combine_tab.zoom_slider.is_some_and(|id| ui.memory(|m| m.has_focus(id)));
    // (Nor while a file is shown large: its window may only open or close this frame.)
    let covered = app.combine_tab.unlock.is_some() || app.combine_tab.preview.is_some();
    let focus = ui.memory(|m| m.focused());
    let space = focus.is_none() || focus == app.combine_tab.card_focus;
    let any_picked = count > 0 || !app.combine_tab.selected_pages.is_empty();
    let keys = if covered || slider_focused { None } else { keyboard(ui, n, any_picked, columns, space) };
    let mut events = TableEvents { action: keys, ..Default::default() };
    // ⌘+ / ⌘− / ⌘0 size the grid's cards (instead of the whole window's text).
    if grid && n > 0 && !covered {
        app.combine_tab.zoom_request = zoom_keys(ui, app.combine_zoom).or(app.combine_tab.zoom_request);
    }
    // An arrow pressed or a zoom asked for in the list has nothing for the grid to do later.
    if !grid {
        app.combine_tab.reveal = None;
        app.combine_tab.zoom_request = None;
    }
    let mut combine = false;

    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(28, 20)).show(ui, |ui| {
        // Title and Combine.
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tl!("Combine files")).font(theme::semibold(20.0)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let response = ui.add_enabled_ui(blocker.is_none(), |ui| widgets::icon_pill(ui, "files", tl!("Combine"), true)).inner;
                if let Some(why) = &blocker {
                    response.clone().on_disabled_hover_text(why);
                }
                if response.clicked() {
                    combine = true;
                }
            });
        });
        let hint = if grid {
            tl!("Files are combined in the order shown. Drag them to change it.")
        } else {
            tl!("Files are combined from top to bottom. Leave Pages empty to take every page.")
        };
        ui.label(egui::RichText::new(hint).color(t.text_faint));
        ui.add_space(12.0);

        // Toolbar: on two lines when the window is narrow.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            // Where the line ends (an overflowing line would move it).
            let edge = ui.max_rect().right();
            add_menu(app, ui);
            let open_docs: Vec<(usize, String)> =
                app.views.iter().enumerate().filter_map(|(i, v)| app.session.get(v.id).map(|d| (i, d.display_name()))).collect();
            let response = ui.add_enabled_ui(!open_docs.is_empty(), |ui| widgets::icon_pill(ui, "file-plus", tl!("Add open documents"), false)).inner;
            response.clone().on_disabled_hover_text(tl!("No documents are open"));
            egui::Popup::menu(&response).show(|ui| {
                if open_docs.len() > 1 && ui.button(tl!("All open documents")).clicked() {
                    app.add_open_documents(&open_docs.iter().map(|(i, _)| *i).collect::<Vec<_>>());
                    ui.close();
                }
                for (i, name) in &open_docs {
                    if ui.button(crate::bidi::visual(name).as_ref()).clicked() {
                        app.add_open_documents(&[*i]);
                        ui.close();
                    }
                }
            });
            group(ui, &t, edge, 4.0 * 30.0 + 3.0 * 6.0);
            // Moves need a selected row with room to move; the buttons stay put either way.
            let can_up = selected.iter().skip_while(|s| **s).any(|s| *s);
            let can_down = selected.iter().rev().skip_while(|s| **s).any(|s| *s);
            let why = tl!("Select a file first");
            let pages_picked = app.combine_tab.selected_pages.len();
            let remove = match (pages_picked, count) {
                (1, _) => tl!("Remove page").to_string(),
                (k, _) if k > 1 => crate::i18n::fmt(tl!("Remove {n} pages"), &[("n", &k.to_string())]),
                (_, 1) => {
                    let name = app.combine_draft.iter().find(|f| app.combine_tab.selected.contains(&f.id)).map_or("", |f| f.name.as_str());
                    crate::i18n::fmt(tl!("Remove {name}"), &[("name", name)])
                }
                (_, 0) => tl!("Remove").to_string(),
                (_, k) => crate::i18n::fmt(tl!("Remove {n} files"), &[("n", &k.to_string())]),
            };
            if tool(ui, can_up, "chevron-up", tl!("Move up"), why).clicked() {
                events.action = Some(RowAction::MoveUp);
            }
            if tool(ui, can_down, "chevron-down", tl!("Move down"), why).clicked() {
                events.action = Some(RowAction::MoveDown);
            }
            if tool(ui, count > 0 || pages_picked > 0, "trash-2", &remove, why).clicked() {
                events.action = Some(RowAction::Remove);
            }
            let locked = app.combine_draft.iter().any(|f| f.lock.is_some() && app.combine_tab.selected.contains(&f.id));
            let unlock = tool(ui, locked, "lock-open", tl!("Unlock…"), tl!("Select a password-protected file first"));
            if unlock.clicked() {
                events.action = Some(RowAction::Unlock(None, unlock.rect.left_bottom() + vec2(0.0, 6.0)));
            }
            group(ui, &t, edge, 2.0 * 30.0 + 6.0);
            let (undo, redo) = (!app.combine_tab.undo.is_empty(), !app.combine_tab.redo.is_empty());
            if tool(ui, undo, "undo-2", tl!("Undo"), tl!("Nothing to undo")).clicked() {
                app.execute("edit.undo");
            }
            if tool(ui, redo, "redo-2", tl!("Redo"), tl!("Nothing to redo")).clicked() {
                app.execute("edit.redo");
            }
            group(ui, &t, edge, 2.0 * 30.0 + 6.0);
            // Thumbnails or the table; the choice is kept in the settings.
            for (view, icon, label) in [(CombineView::Grid, "layout-grid", tl!("Grid view")), (CombineView::List, "list", tl!("List view"))] {
                if icons::button(ui, icon, 30.0, app.combine_view == view, label).clicked() {
                    app.combine_view = view;
                }
            }
            if count > 1 {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(crate::i18n::fmt(tl!("{n} selected"), &[("n", &count.to_string())])).color(t.text_muted));
            }
            if grid && n > 0 {
                // Every file as its pages, or all as cards again.
                group(ui, &t, edge, 2.0 * 30.0 + 6.0);
                let can_expand = app.combine_draft.iter_mut().any(|f| !app.combine_tab.expanded.contains(&f.id) && expandable(f));
                if ui.add_enabled_ui(can_expand, |ui| icons::button(ui, "maximize-2", 30.0, false, tl!("Expand all"))).inner.clicked() {
                    events.action = Some(RowAction::ExpandAll(true));
                }
                let can_collapse = !app.combine_tab.expanded.is_empty();
                if ui.add_enabled_ui(can_collapse, |ui| icons::button(ui, "minimize-2", 30.0, false, tl!("Collapse all"))).inner.clicked() {
                    events.action = Some(RowAction::ExpandAll(false));
                }
                zoom_controls(app, ui, &t, edge);
            }
        });
        ui.add_space(10.0);

        // The list, in a card that highlights while files are dragged over the window.
        let footer_h = 34.0;
        let card_h = (ui.available_height() - footer_h).max(120.0);
        let stroke = if files_hovering { Stroke::new(2.0, t.accent) } else { Stroke::new(1.0, t.border) };
        egui::Frame::NONE.fill(t.card).stroke(stroke).corner_radius(CornerRadius::same(t.radius)).inner_margin(egui::Margin::symmetric(8, 6)).show(
            ui,
            |ui| {
                ui.set_min_height(card_h - 12.0);
                // The grid scrolls inside the card, leaving room for the totals below it.
                if grid {
                    ui.set_max_height(card_h - 12.0);
                }
                ui.set_width(ui.available_width());
                if n == 0 {
                    empty_state(app, ui, &t, files_hovering);
                // The view as the toolbar left it: a click on List view this frame starts no
                // thumbnail renders.
                } else if app.combine_view == CombineView::Grid {
                    if let Some(action) = crate::combine_grid::grid(app, ui, &t, &checks, &selected) {
                        events.action = Some(action);
                    }
                } else {
                    let table_events = table(app, ui, &t, &checks, &selected);
                    events.range_focused |= table_events.range_focused;
                    events.range_changed |= table_events.range_changed;
                    if table_events.action.is_some() {
                        events.action = table_events.action;
                    }
                }
            },
        );

        // A file shown large, over everything.
        crate::combine_grid::preview(app, ui.ctx(), &t);

        // Totals.
        ui.add_space(8.0);
        if n > 0 {
            ui.horizontal(|ui| {
                let pages: usize = checks.iter().filter_map(|c| c.as_ref().ok()).sum();
                // Each file's size once, however many parts it is in.
                let mut counted = BTreeSet::new();
                let size: usize = app.combine_draft.iter().filter(|f| counted.insert(f.group)).map(|f| f.bytes.len()).sum();
                // A file split into parts counts once.
                let n = app.combine_draft.iter().map(|f| f.group).collect::<BTreeSet<u64>>().len();
                let files = if n == 1 { tl!("1 file").to_string() } else { crate::i18n::fmt(tl!("{n} files"), &[("n", &n.to_string())]) };
                let pages = if pages == 1 { tl!("1 page").to_string() } else { crate::i18n::fmt(tl!("{n} pages"), &[("n", &pages.to_string())]) };
                ui.label(egui::RichText::new(format!("{files} · {pages} · {}", crate::panels::human_size(size))).color(t.text_muted));
                let bad = app
                    .combine_draft
                    .iter()
                    .zip(&checks)
                    .filter(|(f, c)| f.problem.is_some() || c.is_err())
                    .map(|(f, _)| f.group)
                    .collect::<BTreeSet<u64>>()
                    .len();
                if bad > 0 {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let text = if bad == 1 {
                            tl!("1 file needs attention").to_string()
                        } else {
                            crate::i18n::fmt(tl!("{n} files need attention"), &[("n", &bad.to_string())])
                        };
                        ui.label(egui::RichText::new(text).color(ERROR));
                    });
                }
            });
        }
    });

    // A Pages field taking the keyboard remembers the list; its first change makes the undo step.
    if events.range_focused {
        app.combine_tab.range_edit = Some(app.combine_snapshot());
    }
    if events.range_changed
        && let Some(before) = app.combine_tab.range_edit.take()
    {
        app.combine_push_undo(before);
    }
    unlock_prompt(app, ui.ctx());
    if let Some(action) = events.action {
        app.combine_apply(action);
    }
    if combine && blocker.is_none() {
        app.combine_staged();
    }
}

/// Unlock…'s password box, floating below what opened it. Enter unlocks, Escape or a click
/// elsewhere closes it.
fn unlock_prompt(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(mut prompt) = app.combine_tab.unlock.take() else { return };
    let t = Tokens::get(ctx);
    let files: Vec<&CombineFile> = app.combine_draft.iter().filter(|f| prompt.rows.contains(&f.id)).collect();
    let title = match files.as_slice() {
        [one] => crate::i18n::fmt(tl!("Unlock {name}"), &[("name", &one.name)]),
        many => crate::i18n::fmt(tl!("Unlock {n} files"), &[("n", &many.len().to_string())]),
    };
    let explain = if files.iter().all(|f| f.lock == Some(Lock::Permissions)) {
        tl!("Enter the permissions password to allow combining")
    } else {
        tl!("Enter the password that opens it")
    };
    let batch = files.len() > 1;
    let (mut submit, mut cancel) = (false, false);
    let area = egui::Area::new(egui::Id::new("combine-unlock")).order(egui::Order::Foreground).fixed_pos(prompt.at).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
            ui.set_width(340.0);
            ui.horizontal(|ui| {
                ui.add(icons::image("lock", 16.0, t.icon));
                ui.add(egui::Label::new(egui::RichText::new(&title).font(theme::semibold(14.0))).truncate());
            });
            ui.add_space(4.0);
            ui.label(egui::RichText::new(explain).color(t.text_muted));
            if batch {
                ui.label(egui::RichText::new(tl!("The same password is tried on each file.")).small().color(t.text_faint));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let field =
                    ui.add(egui::TextEdit::singleline(&mut prompt.input.0).password(!prompt.show).hint_text(tl!("Password")).desired_width(290.0));
                if !prompt.focused {
                    field.request_focus();
                    prompt.focused = true;
                }
                if field.changed() {
                    prompt.error = None;
                }
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
                let (icon, tip) = if prompt.show { ("eye-off", tl!("Hide password")) } else { ("eye", tl!("Show password")) };
                if icons::button(ui, icon, 26.0, false, tip).clicked() {
                    prompt.show = !prompt.show;
                }
            });
            if let Some(e) = &prompt.error {
                ui.label(egui::RichText::new(e).color(ERROR));
            }
            ui.add_space(6.0);
            ui.label(egui::RichText::new(tl!("The password is only used to read the file; it isn't saved.")).small().color(t.text_faint));
            ui.add_space(8.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add_enabled_ui(!prompt.input.0.is_empty(), |ui| widgets::pill_button(ui, tl!("Unlock"), true)).inner.clicked() {
                    submit = true;
                }
                if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                    cancel = true;
                }
            });
        });
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || area.response.clicked_elsewhere() {
        cancel = true;
    }
    if submit && !prompt.input.0.is_empty() {
        let rows = prompt.rows.clone();
        if app.combine_unlock(&rows, &prompt.input.0) {
            return;
        }
        prompt.error = Some(tl!("Wrong password").to_string());
        prompt.focused = false;
    }
    if !cancel {
        app.combine_tab.unlock = Some(prompt);
    }
}

fn separator(ui: &mut egui::Ui, t: &Tokens) {
    ui.add_space(4.0);
    // As tall as the buttons on this line (the toolbar can take two).
    let top = ui.cursor().top();
    ui.painter().vline(ui.cursor().left(), (top + 4.0)..=(top + 26.0), Stroke::new(1.0, t.divider));
    ui.add_space(6.0);
}

/// Before a group of toolbar buttons `width` wide: a divider, or a new line when the group
/// doesn't fit before `edge` (the toolbar wraps between groups, never inside one).
fn group(ui: &mut egui::Ui, t: &Tokens, edge: f32, width: f32) {
    if edge - ui.cursor().min.x < width + 10.0 {
        ui.end_row();
    } else {
        separator(ui, t);
    }
}

/// A toolbar icon button, disabled with a reason.
fn tool(ui: &mut egui::Ui, enabled: bool, icon: &str, label: &str, disabled_why: &str) -> egui::Response {
    ui.add_enabled_ui(enabled, |ui| icons::button(ui, icon, 30.0, false, label)).inner.on_disabled_hover_text(disabled_why)
}

/// Whether a file can be shown as its pages: readable, unlocked, and adding more than one.
fn expandable(f: &mut CombineFile) -> bool {
    f.lock.is_none() && f.problem.is_none() && f.selection().is_ok_and(|k| k > 1 && k <= crate::combine_grid::MAX_EXPAND)
}

/// Why Combine can't run yet, if so.
fn blocker(files: &[CombineFile], checks: &[Result<usize, String>]) -> Option<String> {
    // (A file split into parts is still one file.)
    if files.iter().map(|f| f.group).collect::<BTreeSet<u64>>().len() < 2 {
        return Some(tl!("Add at least two files").to_string());
    }
    for (f, check) in files.iter().zip(checks) {
        if let Some(p) = &f.problem {
            return Some(crate::i18n::fmt(tl!("{name} can't be combined: {why}"), &[("name", &f.name), ("why", p)]));
        }
        if let Err(e) = check {
            return Some(crate::i18n::fmt(tl!("Check the pages of {name}: {why}"), &[("name", &f.name), ("why", e)]));
        }
    }
    None
}

/// While no text field has the keyboard: Up/Down select (Shift extends), Alt+Up/Down move the
/// selection, Delete removes it, Ctrl/⌘+A selects every file. In the grid (`columns` per row)
/// Left/Right step one file and Up/Down one row.
pub(crate) fn keyboard(ui: &egui::Ui, n: usize, any_selected: bool, grid_columns: Option<usize>, space: bool) -> Option<RowAction> {
    // A focused card or button still leaves the arrows to the list; a text field, an open menu
    // or a dialog doesn't.
    let ctx = ui.ctx();
    if n == 0 || ctx.text_edit_focused() || egui::Popup::is_any_open(ctx) || ctx.memory(|m| m.top_modal_layer().is_some()) {
        return None;
    }
    use egui::Key;
    ui.ctx().input_mut(|i| {
        if i.consume_key(Modifiers::COMMAND, Key::A) {
            return Some(RowAction::SelectAll);
        }
        // As Quick Look: the file the keyboard is on, large (the way there without a pointer). A
        // focused button keeps its Space.
        if any_selected && space && i.consume_key(Modifiers::NONE, Key::Space) {
            return Some(RowAction::PreviewSelected);
        }
        if any_selected {
            if i.consume_key(Modifiers::ALT, Key::ArrowUp) {
                return Some(RowAction::MoveUp);
            }
            if i.consume_key(Modifiers::ALT, Key::ArrowDown) {
                return Some(RowAction::MoveDown);
            }
            if i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace) {
                return Some(RowAction::Remove);
            }
        }
        let row = grid_columns.unwrap_or(1).max(1);
        let mut keys = vec![(false, row, Key::ArrowUp), (true, row, Key::ArrowDown)];
        if grid_columns.is_some() {
            keys.extend([(false, 1, Key::ArrowLeft), (true, 1, Key::ArrowRight)]);
        }
        for (down, by, key) in keys {
            // Shift first: a plain arrow also matches with Shift held (egui ignores an extra
            // Shift), which would step instead of extending.
            if i.consume_key(Modifiers::SHIFT, key) {
                return Some(RowAction::Step { down, by, extend: true });
            }
            if i.consume_key(Modifiers::NONE, key) {
                return Some(RowAction::Step { down, by, extend: false });
            }
        }
        None
    })
}

/// ⌘+ (or ⌘=), ⌘− and ⌘0, while no text field, menu or dialog has the keyboard: the grid zoom
/// they ask for, from `zoom`.
fn zoom_keys(ui: &egui::Ui, zoom: f32) -> Option<f32> {
    use crate::combine_grid::zoom_step;
    use egui::{Key, KeyboardShortcut};
    let ctx = ui.ctx();
    if ctx.text_edit_focused() || egui::Popup::is_any_open(ctx) || ctx.memory(|m| m.top_modal_layer().is_some()) {
        return None;
    }
    // All of them are taken (several can arrive in one slow frame), so none is left for egui to
    // zoom the whole window with.
    let (larger, smaller, reset) = ctx.input_mut(|i| {
        let mut pressed = |k| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, k));
        (pressed(Key::Plus) | pressed(Key::Equals), pressed(Key::Minus), pressed(Key::Num0))
    });
    match (larger, smaller, reset) {
        (_, _, true) => Some(1.0),
        (true, false, _) => Some(zoom_step(zoom, true)),
        (false, true, _) => Some(zoom_step(zoom, false)),
        _ => None,
    }
}

/// The grid's card size, at the right of the toolbar (right to left: larger, the percentage, the
/// slider, smaller), as the page grid's.
fn zoom_controls(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens, edge: f32) {
    use crate::combine_grid::{ZOOM_RANGE, zoom_step};
    let zoom = app.combine_zoom;
    // Two buttons, the percentage, the slider and the spaces between: on a line of their own when
    // they don't fit beside the rest.
    let needed = 30.0 + 44.0 + 96.0 + 30.0 + 4.0 * ui.spacing().item_spacing.x + 8.0;
    if edge - ui.cursor().min.x < needed {
        ui.end_row();
    }
    // Right-aligned on the line, up to its end.
    let line = Rect::from_min_max(ui.cursor().min, egui::pos2(edge, ui.cursor().min.y + 30.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(line).layout(Layout::right_to_left(Align::Center)), |ui| {
        let larger = ui.add_enabled_ui(zoom < *ZOOM_RANGE.end(), |ui| icons::button(ui, "zoom-in", 30.0, false, tl!("Larger pages"))).inner;
        if larger.clicked() {
            app.combine_tab.zoom_request = Some(zoom_step(zoom, true));
        }
        let percent =
            egui::Button::new(egui::RichText::new(format!("{:.0}%", zoom * 100.0)).font(theme::medium(12.0)).color(t.text_muted)).frame(false);
        let percent = ui.add_sized([44.0, 30.0], percent);
        percent.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tl!("Reset page size")));
        if percent.on_hover_text(tl!("Reset page size")).clicked() {
            app.combine_tab.zoom_request = Some(1.0);
        }
        let mut value = zoom;
        ui.spacing_mut().slider_width = 96.0;
        let slider = ui.add(egui::Slider::new(&mut value, ZOOM_RANGE).show_value(false));
        // Read out as a percentage, as shown beside it.
        slider.widget_info(|| egui::WidgetInfo::slider(true, f64::from(value * 100.0).round(), tl!("Page size")));
        app.combine_tab.zoom_slider = Some(slider.id);
        if slider.on_hover_text(tl!("Page size")).changed() {
            app.combine_tab.zoom_request = Some(value);
        }
        let smaller = ui.add_enabled_ui(zoom > *ZOOM_RANGE.start(), |ui| icons::button(ui, "zoom-out", 30.0, false, tl!("Smaller pages"))).inner;
        if smaller.clicked() {
            app.combine_tab.zoom_request = Some(zoom_step(zoom, false));
        }
    });
}

fn empty_state(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens, files_hovering: bool) {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() / 2.0 - 70.0).max(16.0));
        ui.add(icons::image("files", 40.0, if files_hovering { t.accent } else { t.text_faint }));
        ui.add_space(10.0);
        ui.label(egui::RichText::new(tl!("Add the PDFs to combine")).font(theme::medium(15.0)));
        ui.add_space(4.0);
        ui.label(egui::RichText::new(tl!("Drop PDFs or folders here, or choose them.")).color(t.text_faint));
        ui.add_space(12.0);
        let folders = cfg!(not(target_arch = "wasm32"));
        // Centre the buttons: measure them first.
        let font = theme::medium(12.5);
        let width = |s: &str| ui.fonts_mut(|f| f.layout_no_wrap(s.to_owned(), font.clone(), t.text).size().x);
        let total = width(tl!("Add files…")) + 26.0 + if folders { width(tl!("Add folder…")) + 46.0 + 8.0 } else { 0.0 };
        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
            ui.spacing_mut().item_spacing.x = 8.0;
            if widgets::pill_button(ui, tl!("Add files…"), true).clicked() {
                app.combine_dialog();
            }
            if folders && widgets::icon_pill(ui, "folder", tl!("Add folder…"), false).clicked() {
                app.combine_add_folder(false);
            }
        });
    });
}

/// "Add files…" with a menu of the other ways to add: folders (desktop).
fn add_menu(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    if !cfg!(not(target_arch = "wasm32")) {
        if widgets::icon_pill(ui, "plus", tl!("Add files…"), false).clicked() {
            app.combine_dialog();
        }
        return;
    }
    let (main, more) = widgets::split_pill(ui, "plus", tl!("Add files…"), tl!("More ways to add files"));
    if main.clicked() {
        app.combine_dialog();
    }
    egui::Popup::menu(&more).show(|ui| {
        ui.set_min_width(220.0);
        let item = |ui: &mut egui::Ui, icon: &str, label: &str| {
            ui.add(egui::Button::image_and_text(icons::image(icon, 15.0, ui.visuals().text_color()), label))
        };
        if item(ui, "file-plus", tl!("Add files…")).clicked() {
            app.combine_dialog();
            ui.close();
        }
        if item(ui, "folder", tl!("Add folder…")).on_hover_text(tl!("Every PDF in the folder")).clicked() {
            app.combine_add_folder(false);
            ui.close();
        }
        if item(ui, "folder-open", tl!("Add folder and subfolders…"))
            .on_hover_text(tl!("Every PDF in the folder and the folders inside it"))
            .clicked()
        {
            app.combine_add_folder(true);
            ui.close();
        }
    });
}

/// The file table.
fn table(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens, checks: &[Result<usize, String>], selected: &[bool]) -> TableEvents {
    let mut events = TableEvents::default();
    // The same file twice is allowed (e.g. a cover sheet), but probably a mistake.
    // (A file's parts are the one file, not copies.)
    let twice: Vec<bool> = app
        .combine_draft
        .iter()
        .map(|f| app.combine_draft.iter().filter(|g| g.group != f.group && g.name == f.name && g.bytes.len() == f.bytes.len()).count() > 0)
        .collect();
    let dragging = egui::DragAndDrop::has_payload_of_type::<usize>(ui.ctx());
    // Until the user resizes a column, the widths follow the window.
    let gap = ui.spacing().item_spacing.x;
    let available = ui.available_width() - 18.0 - gap * 6.0;
    if app.combine_columns.widths.is_none() && (available - app.combine_tab.fitted).abs() > 1.0 {
        app.combine_tab.fitted = available;
        app.combine_tab.layout = app.combine_tab.layout.wrapping_add(1);
    }
    let widths = app.combine_columns.resolved(available);
    let rows = Rows { checks, selected, twice: &twice, dragging, sort: app.combine_tab.sort };
    let mut heading_event = None;
    let mut shown = None;
    egui::ScrollArea::horizontal()
        .id_salt("combine-files-h")
        .show(ui, |ui| table_body(app, ui, t, &rows, widths, &mut events, &mut heading_event, &mut shown));
    // A column resized by hand keeps its width from now on.
    if let Some(shown) = shown
        && shown.iter().zip(&widths).any(|(a, b)| (a - b).abs() > 0.5)
    {
        app.combine_columns.widths = Some(shown);
    }
    match heading_event {
        Some(HeadingEvent::Sort(key)) => events.action = Some(RowAction::Sort(key)),
        Some(HeadingEvent::Move { kind, to, after }) => {
            let mut order: Vec<SortKey> = app.combine_columns.order.iter().copied().filter(|k| *k != kind).collect();
            let at = order.iter().position(|k| *k == to).map_or(order.len(), |p| if after { p + 1 } else { p });
            order.insert(at, kind);
            if let Ok(order) = <[SortKey; 5]>::try_from(order) {
                app.combine_columns.order = order;
                app.combine_tab.layout = app.combine_tab.layout.wrapping_add(1);
            }
        }
        Some(HeadingEvent::Reset) => {
            app.combine_columns = Columns::default();
            app.combine_tab.fitted = 0.0;
        }
        None => {}
    }
    events
}

/// What every row needs to know.
struct Rows<'a> {
    checks: &'a [Result<usize, String>],
    selected: &'a [bool],
    twice: &'a [bool],
    dragging: bool,
    sort: Option<(SortKey, bool)>,
}

/// A column heading: the whole cell sorts by its column when clicked, moves the column when
/// dragged, and offers Reset columns on a right-click.
fn heading(ui: &mut egui::Ui, t: &Tokens, key: SortKey, sort: Option<(SortKey, bool)>) -> Option<HeadingEvent> {
    let text = key.label();
    let rect = ui.max_rect();
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    let arrow = sort.filter(|(k, _)| *k == key).map(|(_, ascending)| ascending);
    let state = match arrow {
        Some(true) => tl!("sorted ascending"),
        Some(false) => tl!("sorted descending"),
        None => "",
    };
    let a11y = if state.is_empty() { text.to_owned() } else { format!("{text}, {state}") };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &a11y));
    let being_dragged = egui::DragAndDrop::payload::<HeadingDrag>(ui.ctx()).is_some_and(|p| p.0 == key);
    if being_dragged || response.is_pointer_button_down_on() {
        ui.painter().rect_filled(rect, CornerRadius::same(4), t.pressed);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(4), t.hover);
    }
    let colour = if arrow.is_some() { t.text } else { t.text_muted };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), theme::medium(12.5), colour);
    let text_pos = egui::pos2(rect.left() + 4.0, rect.center().y - galley.size().y / 2.0);
    let text_w = galley.size().x;
    ui.painter().with_clip_rect(rect).galley(text_pos, galley, colour);
    if let Some(ascending) = arrow {
        let r = Rect::from_center_size(egui::pos2(rect.left() + text_w + 14.0, rect.center().y), vec2(12.0, 12.0));
        icons::paint(ui, r, if ascending { "chevron-up" } else { "chevron-down" }, 12.0, t.text);
    }
    if response.drag_started() {
        egui::DragAndDrop::set_payload(ui.ctx(), HeadingDrag(key));
    }
    // Another heading dragged over this one: a line where it would go.
    let after = |r: &egui::Response| r.ctx.pointer_latest_pos().is_some_and(|p| p.x > rect.center().x);
    if let Some(dragged) = response.dnd_hover_payload::<HeadingDrag>()
        && dragged.0 != key
    {
        let x = if after(&response) { rect.right() } else { rect.left() };
        ui.painter().with_clip_rect(rect.expand(3.0)).vline(x, rect.y_range(), Stroke::new(2.5, t.accent));
    }
    let mut event = None;
    if let Some(dragged) = response.dnd_release_payload::<HeadingDrag>()
        && dragged.0 != key
    {
        event = Some(HeadingEvent::Move { kind: dragged.0, to: key, after: after(&response) });
    }
    let response = response
        .on_hover_cursor(if being_dragged { egui::CursorIcon::Grabbing } else { egui::CursorIcon::PointingHand })
        .on_hover_text(tl!("Click to sort, drag to move the column. Drag its edge to resize it."));
    response.context_menu(|ui| {
        if ui.button(tl!("Reset columns")).clicked() {
            event = Some(HeadingEvent::Reset);
            ui.close();
        }
    });
    if response.clicked() {
        event = Some(HeadingEvent::Sort(key));
    }
    event
}

#[allow(clippy::too_many_arguments)]
fn table_body(
    app: &mut PdfCraftApp,
    ui: &mut egui::Ui,
    t: &Tokens,
    rows: &Rows,
    widths: [f32; 5],
    events: &mut TableEvents,
    heading_event: &mut Option<HeadingEvent>,
    shown: &mut Option<[f32; 5]>,
) {
    let n = app.combine_draft.len();
    let order = app.combine_columns.order;
    // The heading being dragged follows the pointer.
    if let (Some(dragged), Some(pos)) = (egui::DragAndDrop::payload::<HeadingDrag>(ui.ctx()), ui.ctx().pointer_latest_pos()) {
        let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("combine-heading-drag")));
        let galley = painter.layout_no_wrap(dragged.0.label().to_owned(), theme::medium(12.5), t.text);
        let r = Rect::from_min_size(pos + vec2(10.0, 8.0), galley.size() + vec2(16.0, 8.0));
        painter.rect(r, CornerRadius::same(4), t.card, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
        painter.galley(r.min + vec2(8.0, 4.0), galley, t.text);
    }
    let mut builder = TableBuilder::new(ui)
        // Columns moved, reset or refitted: a new table state, built from `widths`.
        .id_salt(("combine-files", app.combine_tab.layout))
        .striped(true)
        .sense(Sense::click())
        .auto_shrink([false, true])
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::exact(18.0));
    for kind in order {
        builder = builder.column(Column::initial(widths[kind.index()]).at_least(kind.min_width()).resizable(true).clip(true));
    }
    builder
        .header(26.0, |mut row| {
            row.col(|_| {});
            for kind in order {
                row.col(|ui| {
                    if let Some(e) = heading(ui, t, kind, rows.sort) {
                        *heading_event = Some(e);
                    }
                });
            }
        })
        .body(|mut body| {
            if !body.ui_mut().is_sizing_pass() {
                let w = body.widths();
                let mut out = widths;
                for (kind, width) in order.iter().zip(w.iter().skip(1)) {
                    out[kind.index()] = *width;
                }
                *shown = Some(out);
            }
            for i in 0..n {
                let Some(f) = app.combine_draft.get_mut(i) else { break };
                let check = rows.checks.get(i).cloned().unwrap_or(Ok(f.pages));
                body.row(32.0, |mut row| {
                    row.set_selected(rows.selected.get(i).copied().unwrap_or(false));
                    // Drag handle.
                    row.col(|ui| {
                        let id = ui.id().with(("combine-drag", f.id));
                        ui.dnd_drag_source(id, i, |ui| {
                            let (rect, resp) = ui.allocate_exact_size(vec2(12.0, 18.0), Sense::hover());
                            grip(ui, rect, if resp.hovered() || rows.dragging { t.text_muted } else { t.text_faint });
                        })
                        .response
                        .on_hover_cursor(egui::CursorIcon::Grab)
                        .on_hover_text(tl!("Drag to reorder"));
                    });
                    for kind in order {
                        row.col(|ui| match kind {
                            SortKey::Name => {
                                ui.add(icons::image("file-text", 16.0, t.icon));
                                ui.add(egui::Label::new(crate::bidi::visual(&f.name).as_ref()).truncate().selectable(false));
                            }
                            // The range field and how many pages it takes.
                            SortKey::Pages => {
                                let edit = ui.add_enabled(
                                    f.problem.is_none(),
                                    egui::TextEdit::singleline(&mut f.range).hint_text(tl!("All pages")).desired_width(88.0),
                                );
                                // A visible field on plain and striped rows alike; red when the range is wrong.
                                let border = if check.is_err() { Stroke::new(1.5, ERROR) } else { Stroke::new(1.0, t.border) };
                                ui.painter().rect_stroke(edit.rect, CornerRadius::same(4), border, egui::StrokeKind::Inside);
                                if let Err(e) = &check {
                                    edit.clone().on_hover_text(e);
                                } else {
                                    edit.clone()
                                        .on_hover_text(crate::i18n::fmt(tl!("Pages of {name} to combine, e.g. 1-3, 6"), &[("name", &f.name)]));
                                }
                                if edit.gained_focus() {
                                    events.range_focused = true;
                                    events.action = Some(RowAction::Click(i, Modifiers::NONE));
                                }
                                if edit.changed() {
                                    events.range_changed = true;
                                }
                                let count = match (&check, f.problem.is_some()) {
                                    (_, true) => "—".to_string(),
                                    (Ok(k), _) => crate::i18n::fmt(tl!("{k} of {n}"), &[("k", &k.to_string()), ("n", &f.pages.to_string())]),
                                    (Err(_), _) => crate::i18n::fmt(tl!("? of {n}"), &[("n", &f.pages.to_string())]),
                                };
                                ui.label(egui::RichText::new(count).small().color(if check.is_err() { ERROR } else { t.text_faint }));
                            }
                            SortKey::Size => {
                                ui.label(egui::RichText::new(crate::panels::human_size(f.bytes.len())).color(t.text_muted));
                            }
                            SortKey::Modified => match f.modified {
                                Some(m) => {
                                    ui.label(egui::RichText::new(ago(m)).color(t.text_muted)).on_hover_text(utc_stamp(m));
                                }
                                None => {
                                    ui.label(egui::RichText::new("—").color(t.text_faint));
                                }
                            },
                            // Blocking problems first, in red.
                            SortKey::Warnings => {
                                let mut errors: Vec<String> = f.problem.iter().cloned().collect();
                                if let Err(e) = &check {
                                    errors.push(e.clone());
                                }
                                let mut warnings = f.notes.clone();
                                if rows.twice.get(i).copied().unwrap_or(false) {
                                    warnings.push(tl!("Added more than once").to_string());
                                }
                                let (icon, colour, first) = match (errors.first(), warnings.first()) {
                                    (Some(e), _) => ("circle-x", ERROR, e.clone()),
                                    (None, Some(w)) => ("triangle-alert", WARNING, w.clone()),
                                    (None, None) => return,
                                };
                                let all = errors.iter().chain(&warnings).cloned().collect::<Vec<_>>().join("\n");
                                ui.add(icons::image(icon, 15.0, colour));
                                if f.lock.is_some() {
                                    let link =
                                        ui.add(egui::Link::new(egui::RichText::new(tl!("Unlock…")).color(t.accent))).on_hover_text(match f.lock {
                                            Some(Lock::Permissions) => tl!("Enter the permissions password to allow combining"),
                                            _ => tl!("Enter the password that opens it"),
                                        });
                                    if link.clicked() {
                                        events.action = Some(RowAction::Unlock(Some(i), link.rect.left_bottom() + vec2(0.0, 6.0)));
                                    }
                                }
                                ui.add(
                                    egui::Label::new(egui::RichText::new(first).color(if colour == ERROR { ERROR } else { t.text }))
                                        .truncate()
                                        .selectable(false),
                                )
                                .on_hover_text(all);
                            }
                        });
                    }
                    let response = row.response();
                    if f.lock.is_some() {
                        let at = response.rect.left_bottom();
                        response.context_menu(|ui| {
                            if ui
                                .add(egui::Button::image_and_text(icons::image("lock-open", 15.0, ui.visuals().text_color()), tl!("Unlock…")))
                                .clicked()
                            {
                                events.action = Some(RowAction::Unlock(Some(i), at));
                                ui.close();
                            }
                        });
                    }
                    if response.clicked() {
                        let modifiers = response.ctx.input(|i| i.modifiers);
                        events.action = Some(RowAction::Click(i, modifiers));
                    }
                    // A row dragged over this one goes where the line is drawn.
                    if let Some(from) = response.dnd_hover_payload::<usize>().map(|p| *p) {
                        let r = response.rect;
                        let y = if from > i { r.top() } else { r.bottom() };
                        if from != i {
                            response.ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("combine-drop-line"))).hline(
                                r.x_range(),
                                y,
                                Stroke::new(2.0, t.accent),
                            );
                        }
                    }
                    if let Some(from) = response.dnd_release_payload::<usize>().map(|p| *p) {
                        events.action = Some(RowAction::Drop { from, to: i });
                    }
                });
            }
        });
}

/// The PDFs in `dir` (and, if `recursive`, the folders inside it, not following links), in
/// natural order, at most [`MAX_FOLDER_FILES`]; and whether there were more.
#[cfg(not(target_arch = "wasm32"))]
fn pdfs_in(dir: &std::path::Path, recursive: bool) -> (Vec<std::path::PathBuf>, bool) {
    let mut found = Vec::new();
    let mut more = false;
    let mut folders = vec![(dir.to_path_buf(), 0usize)];
    'walk: while let Some((folder, depth)) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() {
                let hidden = entry.file_name().to_string_lossy().starts_with('.');
                if recursive && !hidden && depth < MAX_FOLDER_DEPTH {
                    folders.push((path, depth + 1));
                }
            } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) && path.is_file() {
                if found.len() >= MAX_FOLDER_FILES {
                    more = true;
                    break 'walk;
                }
                found.push(path);
            }
        }
    }
    found.sort_by(|a, b| natural(&a.to_string_lossy(), &b.to_string_lossy()));
    (found, more)
}

/// File names in the order people expect: case-insensitive, with numbers by value
/// ("scan2" before "scan10").
fn natural(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        it.next();
                    }
                    digits.trim_start_matches('0').to_owned()
                };
                let (x, y) = (take(&mut a), take(&mut b));
                // Without leading zeros, a longer run of digits is a larger number.
                match x.len().cmp(&y.len()).then_with(|| x.cmp(&y)) {
                    Ordering::Equal => {}
                    other => return other,
                }
            }
            (Some(x), Some(y)) => {
                match x.to_lowercase().cmp(y.to_lowercase()) {
                    Ordering::Equal => {}
                    other => return other,
                }
                a.next();
                b.next();
            }
        }
    }
}

/// Where an arrow key goes from `at` among `n` files, `by` at a time (1 in the list; a grid
/// row's length for Up/Down in the grid). It stays put on the first row going up and on the last
/// row going down; a short last row is reached from the row above at its last file.
pub(crate) fn step_to(at: usize, n: usize, by: usize, down: bool) -> usize {
    let by = by.max(1);
    let last = n.saturating_sub(1);
    let at = at.min(last);
    if down {
        if at >= last / by * by { at } else { at.saturating_add(by).min(last) }
    } else if at < by {
        at
    } else {
        at - by
    }
}

/// The order after moving the files `moving` (keeping their order) to `gap` in `ids` as it is
/// (0 = before the first, `ids.len()` = after the last); `None` when nothing would change.
pub(crate) fn moved_order(ids: &[u64], moving: &BTreeSet<u64>, gap: usize) -> Option<Vec<u64>> {
    let gap = gap.min(ids.len());
    // The gap counted among the files that stay.
    let before = ids.get(..gap).map_or(0, |s| s.iter().filter(|id| moving.contains(id)).count());
    let (taken, mut rest): (Vec<u64>, Vec<u64>) = ids.iter().partition(|id| moving.contains(id));
    if taken.is_empty() {
        return None;
    }
    let at = gap.saturating_sub(before).min(rest.len());
    rest.splice(at..at, taken);
    (rest != ids).then_some(rest)
}

/// A page range (1-based: "1-3, 6, 9-7") that takes exactly `pages` (0-based, in that order)
/// from a file of `count` pages; empty (every page) when that is all of them in order.
pub(crate) fn range_of(pages: &[usize], count: usize) -> String {
    if pages.len() == count && pages.iter().enumerate().all(|(i, p)| i == *p) {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while let Some(&start) = pages.get(i) {
        // The longest run going up (or down) one page at a time from here.
        let step = |a: usize, b: usize| {
            if b == a.wrapping_add(1) {
                1
            } else if a == b.wrapping_add(1) {
                -1
            } else {
                0
            }
        };
        let dir = pages.get(i + 1).map_or(0, |&next| step(start, next));
        let mut end = i;
        while dir != 0 && pages.get(end + 1).is_some_and(|&next| pages.get(end).is_some_and(|&cur| step(cur, next) == dir)) {
            end += 1;
        }
        let last = pages.get(end).copied().unwrap_or(start);
        parts.push(if end == i { format!("{}", start.saturating_add(1)) } else { format!("{}-{}", start.saturating_add(1), last.saturating_add(1)) });
        i = end + 1;
    }
    parts.join(", ")
}

/// One list entry, for moving and removing single pages: its file (`group`: the parts of one
/// file split around share it) and the pages it takes, in order; `None` when they can't be
/// listed (a locked file, a bad range): such an entry only ever moves whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Part {
    pub(crate) id: u64,
    pub(crate) group: u64,
    pub(crate) pages: Option<Vec<usize>>,
}

/// One page in the list's order, or an entry that moves whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Page { group: u64, from: u64, nth: usize, page: usize },
    Whole(u64),
}

/// Every page in the list's order (and the entries that can't be split, whole).
fn items_of(parts: &[Part]) -> Vec<Item> {
    let mut items = Vec::new();
    for p in parts {
        match &p.pages {
            Some(pages) => items.extend(pages.iter().enumerate().map(|(nth, &page)| Item::Page { group: p.group, from: p.id, nth, page })),
            None => items.push(Item::Whole(p.id)),
        }
    }
    items
}

/// Entries again from `items`: a file's pages side by side are one entry (its first unused id
/// among them, else a new one from `new_id`); entries that move whole keep theirs.
fn parts_of(items: &[Item], parts: &[Part], new_id: &mut dyn FnMut() -> u64) -> Vec<Part> {
    let mut out: Vec<Part> = Vec::new();
    let mut used: BTreeSet<u64> = BTreeSet::new();
    let mut run: Vec<Item> = Vec::new();
    let mut close = |run: &mut Vec<Item>, out: &mut Vec<Part>, used: &mut BTreeSet<u64>| {
        let Some(Item::Page { group, .. }) = run.first().copied() else { return };
        let id = run.iter().find_map(|i| match i {
            Item::Page { from, .. } if !used.contains(from) => Some(*from),
            _ => None,
        });
        let id = id.unwrap_or_else(&mut *new_id);
        used.insert(id);
        let pages = run.iter().filter_map(|i| if let Item::Page { page, .. } = i { Some(*page) } else { None }).collect();
        out.push(Part { id, group, pages: Some(pages) });
        run.clear();
    };
    for item in items {
        match item {
            Item::Whole(id) => {
                close(&mut run, &mut out, &mut used);
                if let Some(p) = parts.iter().find(|p| p.id == *id) {
                    used.insert(*id);
                    out.push(p.clone());
                }
            }
            Item::Page { group, .. } => {
                if run.first().is_some_and(|f| !matches!(f, Item::Page { group: g, .. } if g == group)) {
                    close(&mut run, &mut out, &mut used);
                }
                run.push(*item);
            }
        }
    }
    close(&mut run, &mut out, &mut used);
    out
}

/// The entries after moving the pages `moving` (by entry id and place among its pages), in
/// their order, to before page `gap` of the list (counted over every page, an entry that can't be
/// split counting as one; 0 = before the first): a page moved between another file's pages
/// splits its file into parts, and parts of one file brought side by side join. `None` when
/// nothing would change.
pub(crate) fn moved_pages(parts: &[Part], moving: &BTreeSet<(u64, usize)>, gap: usize, new_id: &mut dyn FnMut() -> u64) -> Option<Vec<Part>> {
    let items = items_of(parts);
    let gap = gap.min(items.len());
    let picked = |i: &Item| matches!(i, Item::Page { from, nth, .. } if moving.contains(&(*from, *nth)));
    let before = items.get(..gap).map_or(0, |s| s.iter().filter(|i| picked(i)).count());
    let (taken, mut rest): (Vec<Item>, Vec<Item>) = items.iter().partition(|i| picked(i));
    if taken.is_empty() {
        return None;
    }
    let at = gap.saturating_sub(before).min(rest.len());
    rest.splice(at..at, taken);
    let out = parts_of(&rest, parts, new_id);
    (out != parts).then_some(out)
}

/// The entries after taking out the pages `removing` (by entry id and place among its pages);
/// an entry left with none goes, and parts of one file left side by side join. `None` when
/// nothing would change.
pub(crate) fn removed_pages(parts: &[Part], removing: &BTreeSet<(u64, usize)>, new_id: &mut dyn FnMut() -> u64) -> Option<Vec<Part>> {
    let items: Vec<Item> =
        items_of(parts).into_iter().filter(|i| !matches!(i, Item::Page { from, nth, .. } if removing.contains(&(*from, *nth)))).collect();
    let out = parts_of(&items, parts, new_id);
    (out != parts).then_some(out)
}

/// Six dots: the drag handle.
fn grip(ui: &egui::Ui, rect: Rect, colour: Color32) {
    for dx in [-2.5, 2.5] {
        for dy in [-5.0, 0.0, 5.0] {
            ui.painter().circle_filled(rect.center() + vec2(dx, dy), 1.4, colour);
        }
    }
}

/// How long ago `time` was ("5 min ago"); a date after a week. Relative times need no time zone.
pub(crate) fn ago(time: SystemTime) -> String {
    let secs = SystemTime::now().duration_since(time).map(|d| d.as_secs()).unwrap_or(0);
    let (mins, hours, days) = (secs / 60, secs / 3600, secs / 86_400);
    match () {
        _ if mins == 0 => tl!("just now").to_string(),
        _ if hours == 0 => crate::i18n::fmt(tl!("{n} min ago"), &[("n", &mins.to_string())]),
        _ if days == 0 => crate::i18n::fmt(tl!("{n} h ago"), &[("n", &hours.to_string())]),
        _ if days == 1 => tl!("yesterday").to_string(),
        _ if days < 7 => crate::i18n::fmt(tl!("{n} days ago"), &[("n", &days.to_string())]),
        _ => utc_stamp(time).get(..10).unwrap_or_default().to_string(),
    }
}

/// `time` as "2026-10-08 14:05 UTC".
fn utc_stamp(time: SystemTime) -> String {
    let secs = time.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", rem / 3600, rem % 3600 / 60)
}

impl PdfCraftApp {
    /// Show the Combine files tab (File ▸ Combine files…), opening it if needed.
    pub fn open_combine_tab(&mut self) {
        self.combine_tab.open = true;
        self.combine_tab.focused = true;
        self.active = None;
    }

    /// Whether the Combine files tab is the one showing.
    pub fn combine_showing(&self) -> bool {
        self.active.is_none() && self.combine_tab.open && self.combine_tab.focused
    }

    /// The selected rows, top to bottom (tests and automation).
    pub fn combine_selection(&self) -> Vec<usize> {
        self.combine_draft.iter().enumerate().filter(|(_, f)| self.combine_tab.selected.contains(&f.id)).map(|(i, _)| i).collect()
    }

    /// Select these rows (tests and automation).
    pub fn select_combine_rows(&mut self, rows: &[usize]) {
        // Files picked: no page stays picked (the trash and Delete act on these files).
        self.combine_tab.selected_pages.clear();
        self.combine_tab.page_anchor = None;
        self.combine_tab.selected = rows.iter().filter_map(|i| self.combine_draft.get(*i)).map(|f| f.id).collect();
        let last = rows.last().and_then(|i| self.combine_draft.get(*i)).map(|f| f.id);
        (self.combine_tab.anchor, self.combine_tab.cursor) = (last, last);
    }

    fn combine_snapshot(&self) -> Snapshot {
        Snapshot { files: self.combine_draft.clone(), selected: self.combine_tab.selected.clone(), sort: self.combine_tab.sort }
    }

    fn combine_restore(&mut self, s: Snapshot) {
        self.combine_draft = s.files;
        self.combine_tab.selected = s.selected;
        self.combine_tab.selected_pages.clear();
        self.combine_tab.page_anchor = None;
        self.combine_tab.sort = s.sort;
        self.combine_tab.range_edit = None;
        self.combine_tab.revision = self.combine_tab.revision.wrapping_add(1);
        // A Pages field with the keyboard would otherwise keep typing into the restored list
        // without an undo step.
        if let Some(ctx) = &self.ctx {
            ctx.memory_mut(|m| m.stop_text_input());
        }
    }

    fn combine_push_undo(&mut self, before: Snapshot) {
        self.combine_tab.revision = self.combine_tab.revision.wrapping_add(1);
        let undo = &mut self.combine_tab.undo;
        undo.push(before);
        if undo.len() > HISTORY {
            undo.remove(0);
        }
        self.combine_tab.redo.clear();
    }

    /// Remember the list before a change, for Undo.
    fn combine_record(&mut self) {
        let now = self.combine_snapshot();
        self.combine_push_undo(now);
        self.combine_tab.range_edit = None;
    }

    pub(crate) fn combine_can_undo(&self, undo: bool) -> bool {
        if undo { !self.combine_tab.undo.is_empty() } else { !self.combine_tab.redo.is_empty() }
    }

    /// Edit ▸ Undo / Redo (⌘Z, ⇧⌘Z) while the Combine files tab shows.
    pub(crate) fn combine_history_step(&mut self, undo: bool) {
        // ⌘Z in the password box is not about the list.
        if self.combine_tab.unlock.is_some() {
            return;
        }
        let (from, to) =
            if undo { (&mut self.combine_tab.undo, &mut self.combine_tab.redo) } else { (&mut self.combine_tab.redo, &mut self.combine_tab.undo) };
        let Some(state) = from.pop() else { return };
        let now = Snapshot { files: self.combine_draft.clone(), selected: self.combine_tab.selected.clone(), sort: self.combine_tab.sort };
        to.push(now);
        self.combine_restore(state);
    }

    fn combine_sort(&mut self, key: SortKey, ascending: bool) {
        let list = &mut self.combine_draft;
        let mut keyed: Vec<(u8, CombineFile)> = list.drain(..).map(|mut f| (f.severity(), f)).collect();
        keyed.sort_by(|(sa, a), (sb, b)| {
            let order = match key {
                SortKey::Name => natural(&a.name, &b.name),
                SortKey::Pages => a.pages.cmp(&b.pages),
                SortKey::Size => a.bytes.len().cmp(&b.bytes.len()),
                SortKey::Modified => match (a.modified, b.modified) {
                    (Some(x), Some(y)) => x.cmp(&y),
                    // Unknown times last, whichever the direction.
                    (x, y) => return x.is_none().cmp(&y.is_none()).then_with(|| natural(&a.name, &b.name)),
                },
                SortKey::Warnings => sa.cmp(sb),
            };
            let order = if ascending { order } else { order.reverse() };
            order.then_with(|| natural(&a.name, &b.name))
        });
        *list = keyed.into_iter().map(|(_, f)| f).collect();
        self.combine_tab.sort = Some((key, ascending));
    }

    /// The list as parts, for page edits: each entry's pages, or `None` for one that can't be
    /// split (locked, unreadable, a bad range).
    fn combine_parts(&self) -> Vec<Part> {
        self.combine_draft
            .iter()
            .map(|f| Part {
                id: f.id,
                group: f.group,
                // (No page listed, e.g. its pages couldn't be counted: it moves whole, never lost.)
                pages: (f.lock.is_none() && f.problem.is_none()).then(|| f.pages_taken().ok()).flatten().filter(|p| !p.is_empty()),
            })
            .collect()
    }

    /// Make the list `parts`, as one undo step: entries kept by id, new parts made from their
    /// file's other entries, each taking exactly its pages (its range rewritten). The parts of a
    /// file shown as its pages stay shown so; the page selection is let go of.
    fn combine_set_parts(&mut self, parts: Vec<Part>) {
        self.combine_record();
        self.combine_put_parts(parts);
        let tab = &mut self.combine_tab;
        tab.selected_pages.clear();
        tab.page_anchor = None;
        // An order made by hand isn't sorted any more.
        tab.sort = None;
    }

    /// Join the parts of a file that have come to be side by side (no undo step of its own: it
    /// belongs to the change that brought them together).
    fn combine_join_parts(&mut self) {
        let parts = self.combine_parts();
        // Nothing to join unless some file is in more than one part.
        let groups: BTreeSet<u64> = parts.iter().map(|p| p.group).collect();
        if groups.len() == parts.len() {
            return;
        }
        let next = &mut self.combine_tab.next_id;
        let mut new_id = || {
            let id = *next;
            *next = next.wrapping_add(1);
            id
        };
        let joined = parts_of(&items_of(&parts), &parts, &mut new_id);
        if joined != parts {
            self.combine_put_parts(joined);
        }
    }

    /// Make the list `parts`: entries kept by id, new parts made from their file's other entries,
    /// each taking exactly its pages (its range rewritten only when they differ).
    fn combine_put_parts(&mut self, parts: Vec<Part>) {
        let old = std::mem::take(&mut self.combine_draft);
        let expanded: BTreeSet<u64> = old.iter().filter(|f| self.combine_tab.expanded.contains(&f.id)).map(|f| f.group).collect();
        for p in parts {
            let Some(base) = old.iter().find(|f| f.id == p.id).or_else(|| old.iter().find(|f| f.group == p.group)) else { continue };
            let mut f = base.clone();
            f.id = p.id;
            // (A range that already takes these pages stays as the user typed it.)
            if let Some(pages) = &p.pages
                && f.pages_taken().ok().as_ref() != Some(pages)
            {
                f.range = range_of(pages, f.pages);
                f.checked = None;
            }
            if expanded.contains(&f.group) {
                self.combine_tab.expanded.insert(f.id);
            }
            self.combine_draft.push(f);
        }
        let ids: BTreeSet<u64> = self.combine_draft.iter().map(|f| f.id).collect();
        self.combine_tab.selected.retain(|id| ids.contains(id));
    }

    /// A page card clicked: that page selected (Ctrl/⌘ toggles it, Shift selects every page from
    /// the last one clicked, in the list's order); the files' selection is let go of.
    fn combine_click_page(&mut self, id: u64, nth: usize, m: Modifiers) {
        let page = (id, nth);
        let tab = &mut self.combine_tab;
        tab.selected.clear();
        tab.cursor = Some(id);
        if m.shift
            && let Some(from) = tab.page_anchor
        {
            // Every page between the two, as the grid shows them.
            let order: Vec<(u64, usize)> = self
                .combine_draft
                .iter()
                .filter(|f| tab.expanded.contains(&f.id))
                .flat_map(|f| (0..f.pages_taken().map_or(0, |p| p.len())).map(move |n| (f.id, n)))
                .collect();
            let (a, b) = (order.iter().position(|p| *p == from), order.iter().position(|p| *p == page));
            if let (Some(a), Some(b)) = (a, b) {
                if !m.command {
                    tab.selected_pages.clear();
                }
                tab.selected_pages.extend(order.get(a.min(b)..=a.max(b)).unwrap_or_default().iter().copied());
                return;
            }
        }
        if m.command {
            if !tab.selected_pages.remove(&page) {
                tab.selected_pages.insert(page);
            }
        } else {
            tab.selected_pages = BTreeSet::from([page]);
        }
        tab.page_anchor = Some(page);
    }

    /// Move `moving` to `gap`, as one undo step; a move that changes nothing leaves the list,
    /// its sort and the history alone.
    fn combine_move_to_gap(&mut self, moving: BTreeSet<u64>, gap: usize) {
        let ids: Vec<u64> = self.combine_draft.iter().map(|f| f.id).collect();
        let Some(order) = moved_order(&ids, &moving, gap) else { return };
        self.combine_record();
        let rank: std::collections::HashMap<u64, usize> = order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        self.combine_draft.sort_by_key(|f| rank.get(&f.id).copied().unwrap_or(usize::MAX));
        self.combine_tab.selected = moving.into_iter().filter(|id| rank.contains_key(id)).collect();
        self.combine_tab.sort = None;
    }

    pub(crate) fn combine_apply(&mut self, action: RowAction) {
        let page_edit = matches!(action, RowAction::ClickPage(..) | RowAction::MovePages { .. } | RowAction::RemovePages(_));
        self.combine_apply_once(action);
        // One kind of selection at a time: files picked (by a click, the keys, a drop, Unlock…)
        // let go of the pages picked, so the trash never takes pages the user no longer means.
        if !self.combine_tab.selected.is_empty() {
            self.combine_tab.selected_pages.clear();
            self.combine_tab.page_anchor = None;
        }
        // A file's parts brought side by side (the file between them removed or moved away) are
        // one entry again, as in the same undo step.
        if !page_edit {
            self.combine_join_parts();
        }
    }

    fn combine_apply_once(&mut self, action: RowAction) {
        // The trash and Delete act on the selected pages while there are any.
        let action = match action {
            // (Files picked come first: the two are never both picked, but if they ever were, the
            // files the user sees picked are what goes.)
            RowAction::Remove if self.combine_tab.selected.is_empty() && !self.combine_tab.selected_pages.is_empty() => RowAction::RemovePages(None),
            a => a,
        };
        // Page edits work on the list as parts.
        match action {
            RowAction::ClickPage(id, nth, m) => return self.combine_click_page(id, nth, m),
            RowAction::MovePages { page, revision, gap } => {
                if revision != self.combine_tab.revision {
                    return;
                }
                let moving =
                    if self.combine_tab.selected_pages.contains(&page) { self.combine_tab.selected_pages.clone() } else { BTreeSet::from([page]) };
                let parts = self.combine_parts();
                let next = &mut self.combine_tab.next_id;
                let mut new_id = || {
                    let id = *next;
                    *next = next.wrapping_add(1);
                    id
                };
                if let Some(moved) = moved_pages(&parts, &moving, gap, &mut new_id) {
                    self.combine_set_parts(moved);
                }
                return;
            }
            RowAction::RemovePages(one) => {
                let removing = match one {
                    Some(page) => BTreeSet::from([page]),
                    None => self.combine_tab.selected_pages.clone(),
                };
                if removing.is_empty() {
                    return;
                }
                let parts = self.combine_parts();
                let next = &mut self.combine_tab.next_id;
                let mut new_id = || {
                    let id = *next;
                    *next = next.wrapping_add(1);
                    id
                };
                // (Nothing to take out: no undo step, the redo history kept.)
                if let Some(left) = removed_pages(&parts, &removing, &mut new_id) {
                    self.combine_set_parts(left);
                }
                return;
            }
            _ => {}
        }
        // A file's card clicked (or the list's keys): the files are selected, no page.
        if matches!(action, RowAction::Click(..) | RowAction::SelectAll | RowAction::Step { .. }) {
            self.combine_tab.selected_pages.clear();
        }
        let ids: Vec<u64> = self.combine_draft.iter().map(|f| f.id).collect();
        let n = ids.len();
        let index_of = |id: Option<u64>| id.and_then(|id| ids.iter().position(|x| *x == id));
        let tab = &mut self.combine_tab;
        match action {
            RowAction::Click(i, m) => {
                let Some(&id) = ids.get(i) else { return };
                if m.shift {
                    let from = index_of(tab.anchor).unwrap_or(i);
                    let range = ids.get(from.min(i)..=from.max(i)).unwrap_or_default();
                    if !m.command {
                        tab.selected.clear();
                    }
                    tab.selected.extend(range.iter().copied());
                    tab.cursor = Some(id);
                } else if m.command {
                    if !tab.selected.remove(&id) {
                        tab.selected.insert(id);
                    }
                    (tab.anchor, tab.cursor) = (Some(id), Some(id));
                } else {
                    tab.selected = BTreeSet::from([id]);
                    (tab.anchor, tab.cursor) = (Some(id), Some(id));
                }
            }
            RowAction::SelectAll => {
                tab.selected = ids.iter().copied().collect();
            }
            RowAction::Step { down, by, extend } => {
                // With files shown as their pages, Up and Down go a row of cards (from the last
                // of a file's cards down, from its first up) to the file there.
                let places = &tab.grid_places;
                let rows_of_cards = by > 1 && places.len() > n;
                let next = match index_of(tab.cursor) {
                    Some(c) if rows_of_cards => {
                        let from = if down { places.iter().rposition(|f| *f == c) } else { places.iter().position(|f| *f == c) };
                        from.and_then(|at| places.get(step_to(at, places.len(), by, down)).copied()).unwrap_or(c)
                    }
                    Some(c) => step_to(c, n, by, down),
                    None if down => 0,
                    None => n.saturating_sub(1),
                };
                let Some(&id) = ids.get(next) else { return };
                if extend {
                    let from = index_of(tab.anchor).unwrap_or(next);
                    tab.selected = ids.get(from.min(next)..=from.max(next)).unwrap_or_default().iter().copied().collect();
                } else {
                    tab.selected = BTreeSet::from([id]);
                    tab.anchor = Some(id);
                }
                tab.cursor = Some(id);
                tab.reveal = Some(id);
            }
            RowAction::MoveUp | RowAction::MoveDown => {
                let up = matches!(action, RowAction::MoveUp);
                let selected: Vec<bool> = ids.iter().map(|id| tab.selected.contains(id)).collect();
                let movable =
                    if up { selected.iter().skip_while(|s| **s).any(|s| *s) } else { selected.iter().rev().skip_while(|s| **s).any(|s| *s) };
                if !movable {
                    return;
                }
                self.combine_record();
                let list = &mut self.combine_draft;
                let sel = &self.combine_tab.selected;
                // Each selected file steps past its unselected neighbour; blocks move together.
                if up {
                    for i in 1..list.len() {
                        if sel.contains(&list[i].id) && !sel.contains(&list[i - 1].id) {
                            list.swap(i, i - 1);
                        }
                    }
                } else {
                    for i in (0..list.len().saturating_sub(1)).rev() {
                        if sel.contains(&list[i].id) && !sel.contains(&list[i + 1].id) {
                            list.swap(i, i + 1);
                        }
                    }
                }
                self.combine_tab.sort = None;
            }
            RowAction::Remove => {
                let Some(first) = ids.iter().position(|id| tab.selected.contains(id)) else { return };
                self.combine_record();
                let sel = std::mem::take(&mut self.combine_tab.selected);
                self.combine_draft.retain(|f| !sel.contains(&f.id));
                // The file that took the first removed one's place is selected next.
                let next = self.combine_draft.get(first).or(self.combine_draft.last()).map(|f| f.id);
                self.combine_tab.selected = next.into_iter().collect();
                (self.combine_tab.anchor, self.combine_tab.cursor) = (next, next);
            }
            RowAction::RemoveFile(id) => {
                if !ids.contains(&id) {
                    return;
                }
                self.combine_record();
                self.combine_draft.retain(|f| f.id != id);
                // The rest of the selection stays as it was.
                self.combine_tab.selected.remove(&id);
                if self.combine_tab.anchor == Some(id) {
                    self.combine_tab.anchor = None;
                }
                if self.combine_tab.cursor == Some(id) {
                    self.combine_tab.cursor = None;
                }
            }
            RowAction::Preview(id) => {
                if ids.contains(&id) {
                    self.combine_tab.preview = Some(Preview { file: id, at: 0, page: None, pages: None });
                }
            }
            RowAction::PreviewAt(id, at) => {
                if ids.contains(&id) {
                    self.combine_tab.preview = Some(Preview { file: id, at, page: None, pages: None });
                }
            }
            RowAction::Expand(id, on) => {
                if on {
                    if self.combine_draft.iter_mut().any(|f| f.id == id && expandable(f)) {
                        self.combine_tab.expanded.insert(id);
                    }
                } else {
                    self.combine_tab.expanded.remove(&id);
                    // Its card may be far from where its pages were: brought into view.
                    self.combine_tab.reveal = Some(id);
                }
            }
            RowAction::ExpandAll(on) => {
                self.combine_tab.expanded =
                    if on { self.combine_draft.iter_mut().filter_map(|f| expandable(f).then_some(f.id)).collect() } else { BTreeSet::new() };
                if !on {
                    self.combine_tab.reveal = self.combine_tab.cursor;
                }
            }
            RowAction::PreviewSelected => {
                // The file the keyboard is on, or the first selected; with pages selected, the
                // page card that has the keyboard, or the first selected page.
                let first_page = tab.selected_pages.iter().min_by_key(|(id, nth)| (ids.iter().position(|x| x == id), *nth)).copied();
                let page = tab.card_focus_at.filter(|p| tab.selected_pages.contains(p)).or(first_page);
                let id = tab
                    .cursor
                    .filter(|c| tab.selected.contains(c))
                    .or_else(|| ids.iter().copied().find(|id| tab.selected.contains(id)))
                    .or(page.map(|(id, _)| id));
                // Only what its card would offer a magnifier for (unlocked, readable, measured).
                let shows = |id: u64| self.combine_draft.iter().any(|f| f.id == id && f.lock.is_none() && f.problem.is_none() && !f.sizes.is_empty());
                if let Some(id) = id.filter(|id| shows(*id)) {
                    // From the page card that has the keyboard (or the selected page), if it's one
                    // of this file's.
                    let at = self.combine_tab.card_focus_at.or(page).filter(|(file, _)| *file == id).map_or(0, |(_, nth)| nth);
                    self.combine_tab.preview = Some(Preview { file: id, at, page: None, pages: None });
                }
            }
            // (Page edits were done above.)
            RowAction::ClickPage(..) | RowAction::MovePages { .. } | RowAction::RemovePages(_) => {}
            RowAction::Drop { from, to } => {
                let (Some(&dragged), Some(&target)) = (ids.get(from), ids.get(to)) else { return };
                // Dragging a selected file takes the whole selection along.
                let moving: BTreeSet<u64> = if tab.selected.contains(&dragged) { tab.selected.clone() } else { BTreeSet::from([dragged]) };
                // Dropped on the selection itself: nothing to do.
                if moving.contains(&target) {
                    return;
                }
                // Below the row when dragged down, above it when dragged up.
                self.combine_move_to_gap(moving, if from < to { to + 1 } else { to });
            }
            RowAction::DropAt { file, revision, gap } => {
                // The list changed under the drag (e.g. undo): the gap no longer means the same.
                if revision != tab.revision || !ids.contains(&file) {
                    return;
                }
                let moving: BTreeSet<u64> = if tab.selected.contains(&file) { tab.selected.clone() } else { BTreeSet::from([file]) };
                self.combine_move_to_gap(moving, gap);
            }
            RowAction::Unlock(row, at) => {
                let locked: BTreeSet<u64> = self.combine_draft.iter().filter(|f| f.lock.is_some()).map(|f| f.id).collect();
                let rows: Vec<u64> = match row.and_then(|i| ids.get(i).copied()) {
                    // A selected row brings the rest of the selection.
                    Some(id) if tab.selected.contains(&id) => {
                        ids.iter().copied().filter(|x| tab.selected.contains(x) && locked.contains(x)).collect()
                    }
                    Some(id) => {
                        tab.selected = BTreeSet::from([id]);
                        (tab.anchor, tab.cursor) = (Some(id), Some(id));
                        vec![id]
                    }
                    None => ids.iter().copied().filter(|x| tab.selected.contains(x) && locked.contains(x)).collect(),
                };
                self.combine_ask_password(rows, at);
            }
            RowAction::Sort(key) => {
                let ascending = !matches!(tab.sort, Some((k, true)) if k == key);
                self.combine_record();
                self.combine_sort(key, ascending);
            }
        }
    }

    /// Close the Combine files tab and forget its list; the last document shows, if any.
    pub fn close_combine_tab(&mut self) {
        let showing = self.combine_showing();
        // (Revisions keep counting: a drag begun before can never match a list made after.)
        let revision = self.combine_tab.revision.wrapping_add(1);
        self.combine_tab = CombineTab { revision, ..CombineTab::default() };
        self.combine_draft.clear();
        self.combine_thumbs.clear();
        if showing {
            self.active = self.views.len().checked_sub(1);
        }
    }

    /// Add picked files to the Combine files list (and show it).
    pub(crate) fn stage_combine(&mut self, files: Vec<(String, Vec<u8>)>) {
        let incoming = files.into_iter().map(|(name, bytes)| Incoming { name, bytes: Arc::new(bytes), modified: None, note: None }).collect();
        self.stage_combine_with(incoming);
    }

    /// Add files to the Combine files list, with what can be known about them up front.
    pub(crate) fn stage_combine_with(&mut self, files: Vec<Incoming>) {
        let mut added = Vec::new();
        for Incoming { name, bytes, modified, note } in files {
            let assessed = match assess(&bytes, None) {
                Ok(a) => a,
                Err(AssessError::Unreadable(e)) => {
                    self.notify_fmt("Couldn't add {name}: {e}", &[("name", &name), ("e", &e)]);
                    continue;
                }
                Err(AssessError::WrongPassword) => {
                    self.notify_fmt("Couldn't add {name}: {e}", &[("name", &name), ("e", "the password is wrong")]);
                    continue;
                }
            };
            let mut f = CombineFile {
                id: 0,
                group: 0,
                name,
                bytes,
                pages: 0,
                range: String::new(),
                modified,
                problem: None,
                lock: None,
                password: None,
                sizes: Arc::from(Vec::new()),
                auth: 0,
                notes: Vec::new(),
                origin_note: note,
                checked: None,
            };
            f.take(assessed, None);
            added.push(f);
        }
        if !added.is_empty() {
            self.combine_record();
            for mut f in added {
                f.id = self.combine_tab.next_id;
                f.group = f.id;
                self.combine_tab.next_id = self.combine_tab.next_id.wrapping_add(1);
                self.combine_draft.push(f);
            }
            // A sorted list stays sorted; new files simply join the end otherwise.
            if let Some((key, ascending)) = self.combine_tab.sort {
                self.combine_sort(key, ascending);
            }
        }
        self.open_combine_tab();
    }

    /// Open Unlock…'s password box for `rows`, at `at`.
    fn combine_ask_password(&mut self, rows: Vec<u64>, at: egui::Pos2) {
        if rows.is_empty() {
            return;
        }
        self.combine_tab.unlock = Some(UnlockPrompt { rows, input: Secret::default(), show: false, error: None, at, focused: false });
    }

    /// Try `password` on the protected files `rows` (as one undo step). Returns false when it
    /// opened none of them.
    pub fn combine_unlock(&mut self, rows: &[u64], password: &str) -> bool {
        // A file split into parts is unlocked as one: each file once, all its parts alike.
        let groups: BTreeSet<u64> =
            rows.iter().filter_map(|id| self.combine_draft.iter().find(|f| f.id == *id && f.lock.is_some())).map(|f| f.group).collect();
        let mut accepted = Vec::new();
        for group in &groups {
            let Some(f) = self.combine_draft.iter().find(|f| f.group == *group) else { continue };
            if let Ok(a) = assess(&f.bytes, Some(password)) {
                accepted.push((*group, a));
            }
        }
        if accepted.is_empty() {
            return false;
        }
        self.combine_record();
        let tried = groups.len();
        let mut unlocked = 0;
        for (group, a) in accepted {
            let mut auth = None;
            for f in self.combine_draft.iter_mut().filter(|f| f.group == group) {
                f.take(a.clone(), Some(Secret(password.to_owned())));
                // Read once: one way of reading for all its parts (their thumbnails are shared).
                f.auth = *auth.get_or_insert(f.auth);
            }
            unlocked += usize::from(self.combine_draft.iter().any(|f| f.group == group && f.lock.is_none()));
        }
        if tried > 1 {
            self.notify_fmt("Unlocked {n} of {m} files", &[("n", &unlocked.to_string()), ("m", &tried.to_string())]);
        }
        true
    }

    /// Unlock… for the files the user named in tests and automation (by row).
    pub fn combine_unlock_rows(&mut self, rows: &[usize], password: &str) -> bool {
        let ids: Vec<u64> = rows.iter().filter_map(|i| self.combine_draft.get(*i)).map(|f| f.id).collect();
        self.combine_unlock(&ids, password)
    }

    /// Add every PDF in a folder the user picks (and its subfolders, if `recursive`).
    pub fn combine_add_folder(&mut self, recursive: bool) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let title = if recursive { tl!("Add folder and subfolders") } else { tl!("Add folder") };
            let dialog = rfd::AsyncFileDialog::new().set_title(title);
            self.ask_one(crate::pickers::Ask::Folder(dialog), None, move |app, dir| app.add_folder_to_combine(&dir, recursive));
        }
        #[cfg(target_arch = "wasm32")]
        let _ = recursive;
    }

    /// Add the PDFs in `dir` to the Combine files list, as one undo step.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn add_folder_to_combine(&mut self, dir: &std::path::Path, recursive: bool) {
        let (paths, more) = pdfs_in(dir, recursive);
        let folder = dir.file_name().map_or_else(|| dir.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
        if paths.is_empty() {
            self.notify_fmt("No PDFs found in {folder}", &[("folder", &folder)]);
            self.open_combine_tab();
            return;
        }
        self.use_paths(crate::files::FilePurpose::Combine, &paths);
        if more {
            self.notify_fmt("Added the first {n} PDFs in {folder}", &[("n", &MAX_FOLDER_FILES.to_string()), ("folder", &folder)]);
        }
    }

    /// Add documents open in tabs (by tab index), as they are now.
    pub(crate) fn add_open_documents(&mut self, tabs: &[usize]) {
        let mut incoming = Vec::new();
        for &i in tabs {
            let Some(doc) = self.views.get(i).and_then(|v| self.session.get(v.id)) else { continue };
            #[cfg(not(target_arch = "wasm32"))]
            let modified = if doc.dirty { None } else { doc.path.as_ref().and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok()) };
            #[cfg(target_arch = "wasm32")]
            let modified = None;
            let note = doc.dirty.then(|| tl!("Includes unsaved changes").to_string());
            let name = if doc.name.to_ascii_lowercase().ends_with(".pdf") { doc.name.clone() } else { format!("{}.pdf", doc.name) };
            incoming.push(Incoming { name, bytes: doc.bytes.clone(), modified, note });
        }
        self.stage_combine_with(incoming);
    }

    /// Combine the listed files into a new, unsaved document; the tab makes way for it.
    pub fn combine_staged(&mut self) {
        let files = &self.combine_draft;
        if files.is_empty() {
            return;
        }
        // Each file once, however many parts it is in.
        let count = files.iter().map(|f| f.group).collect::<BTreeSet<u64>>().len();
        let groups: Vec<u64> = files.iter().map(|f| f.group).collect();
        let sources: Vec<(String, Arc<Vec<u8>>, Option<String>)> = files
            .iter()
            .map(|f| (crate::files::strip_pdf(&f.name).to_string(), f.bytes.clone(), Some(f.range.clone()).filter(|r| !r.trim().is_empty())))
            .collect();
        let passwords: Vec<Option<&str>> = files.iter().map(|f| f.password.as_ref().map(|p| p.0.as_str())).collect();
        match self.session.combine_grouped(&sources, &groups, &passwords) {
            Ok(bytes) => {
                self.close_combine_tab();
                let message = crate::i18n::fmt(tl!("Combined {n} files"), &[("n", &count.to_string())]);
                self.open_created("Combined.pdf", bytes, &message)
            }
            // The list stays, so the user can fix it.
            Err(e) => self.notify_fmt("Couldn't combine files: {e}", &[("e", &e.to_string())]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page PDF that opens with "pw".
    fn locked() -> Vec<u8> {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
            "<< /Type /Page /Parent 2 0 R >>",
        ];
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
        let mut doc = pdfcraft_cos::Document::open(Arc::new(out)).unwrap();
        doc.set_encryption(&pdfcraft_cos::NewEncryption {
            algorithm: pdfcraft_cos::Algorithm::Aes256,
            user_password: "pw",
            owner_password: "owner",
            permissions: -1,
            encrypt_metadata: true,
            seed: [4; 32],
        })
        .unwrap();
        pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()
    }

    #[test]
    fn every_read_of_a_file_is_new_even_after_undo() {
        // Regression: the read counter lived in the undo history, so unlock → undo → unlock (with
        // another password) could repeat a value and keep a thumbnail of the other read.
        let mut app = PdfCraftApp::new();
        app.stage_combine(vec![("secret.pdf".into(), locked())]);
        let added = app.combine_draft[0].auth;
        assert!(app.combine_unlock_rows(&[0], "pw"));
        let first = app.combine_draft[0].auth;
        app.combine_history_step(true);
        assert_eq!(app.combine_draft[0].auth, added, "undo brings back the file as it was read");
        assert!(app.combine_unlock_rows(&[0], "owner"));
        let second = app.combine_draft[0].auth;
        assert!(second != first && second != added, "added {added}, unlocked {first}, unlocked again {second}");
    }

    #[test]
    fn moves_by_rank_never_lose_or_repeat_a_file() {
        let mut app = PdfCraftApp::new();
        app.stage_combine((0..6).map(|i| (format!("f{i}.pdf"), locked())).collect());
        let mut ids: Vec<u64> = app.combine_draft.iter().map(|f| f.id).collect();
        for (moving, gap) in [(vec![0usize, 2, 4], 6), (vec![5], 0), (vec![1, 3], 3)] {
            let set: BTreeSet<u64> = moving.iter().filter_map(|i| ids.get(*i).copied()).collect();
            app.combine_move_to_gap(set, gap);
            let mut now: Vec<u64> = app.combine_draft.iter().map(|f| f.id).collect();
            ids = now.clone();
            now.sort_unstable();
            assert_eq!(now, (0..6).collect::<Vec<u64>>(), "every file once");
        }
    }

    #[test]
    fn page_lists_become_the_shortest_range() {
        assert_eq!(range_of(&[0, 1, 2], 3), "", "every page in order: no range");
        assert_eq!(range_of(&[0, 1, 2], 5), "1-3");
        assert_eq!(range_of(&[2, 0, 3], 5), "3, 1, 4");
        assert_eq!(range_of(&[8, 7, 6, 0], 9), "9-7, 1");
        assert_eq!(range_of(&[0, 0, 1], 2), "1, 1-2");
        assert_eq!(range_of(&[4], 9), "5");
        assert_eq!(range_of(&[], 3), "");
        // Whatever comes out reads back as the same pages.
        for pages in [vec![0, 1, 2, 5, 4, 3, 3, 7], vec![9, 0, 9, 1], vec![2, 1, 0, 1, 2]] {
            let range = range_of(&pages, 10);
            let back = pdfcraft_engine::print::select_pages(10, Some(&range), &[], pdfcraft_engine::print::Subset::All, false).unwrap();
            assert_eq!(back, pages, "{range}");
        }
    }

    fn part(id: u64, group: u64, pages: &[usize]) -> Part {
        Part { id, group, pages: Some(pages.to_vec()) }
    }

    #[test]
    fn a_page_moved_between_another_files_pages_splits_its_file_and_parts_rejoin() {
        let mut next = 100;
        let mut new_id = || {
            next += 1;
            next
        };
        // A: pages 1-3, B: pages 1-2.
        let list = [part(1, 1, &[0, 1, 2]), part(2, 2, &[0, 1])];
        // A's page 3 between B's pages: A splits around B's first page.
        let moved = moved_pages(&list, &BTreeSet::from([(1, 2)]), 4, &mut new_id).unwrap();
        assert_eq!(moved, [part(1, 1, &[0, 1]), part(2, 2, &[0]), part(101, 1, &[2]), part(102, 2, &[1])]);
        // Back where it was: the parts join again, under their first ids.
        let back = moved_pages(&moved, &BTreeSet::from([(101, 0)]), 2, &mut new_id).unwrap();
        assert_eq!(back, [part(1, 1, &[0, 1, 2]), part(2, 2, &[0, 1])]);
        // Within a file: only its order changes.
        let within = moved_pages(&list, &BTreeSet::from([(1, 0)]), 3, &mut new_id).unwrap();
        assert_eq!(within, [part(1, 1, &[1, 2, 0]), part(2, 2, &[0, 1])]);
        // Several pages carried together keep their order.
        let both = moved_pages(&list, &BTreeSet::from([(1, 0), (1, 2)]), 5, &mut new_id).unwrap();
        assert_eq!(both, [part(1, 1, &[1]), part(2, 2, &[0, 1]), part(103, 1, &[0, 2])]);
        // Dropped where it is, or nothing picked: no change.
        assert_eq!(moved_pages(&list, &BTreeSet::from([(1, 1)]), 1, &mut new_id), None);
        assert_eq!(moved_pages(&list, &BTreeSet::from([(9, 9)]), 0, &mut new_id), None);
        // A file that can't be split moves whole, and pages move around it.
        let locked = [part(1, 1, &[0, 1]), Part { id: 3, group: 3, pages: None }];
        let around = moved_pages(&locked, &BTreeSet::from([(1, 0)]), 3, &mut new_id).unwrap();
        assert_eq!(around, [part(1, 1, &[1]), Part { id: 3, group: 3, pages: None }, part(104, 1, &[0])]);
    }

    #[test]
    fn removing_pages_drops_emptied_parts_and_joins_what_meets() {
        let mut next = 200;
        let mut new_id = || {
            next += 1;
            next
        };
        // A split around B: A[1], B[1], A[2].
        let list = [part(1, 1, &[0]), part(2, 2, &[0]), part(3, 1, &[1])];
        // B goes: A's two parts meet and join.
        assert_eq!(removed_pages(&list, &BTreeSet::from([(2, 0)]), &mut new_id), Some(vec![part(1, 1, &[0, 1])]));
        // One page of a part.
        let two = [part(1, 1, &[0, 1, 2])];
        assert_eq!(removed_pages(&two, &BTreeSet::from([(1, 1)]), &mut new_id), Some(vec![part(1, 1, &[0, 2])]));
        // Every page: nothing left. Repeated pages are told apart by their place.
        assert_eq!(removed_pages(&two, &BTreeSet::from([(1, 0), (1, 1), (1, 2)]), &mut new_id), Some(Vec::new()));
        let repeats = [part(1, 1, &[0, 0])];
        assert_eq!(removed_pages(&repeats, &BTreeSet::from([(1, 1)]), &mut new_id), Some(vec![part(1, 1, &[0])]));
        // A page that isn't there: nothing changes, and it says so.
        assert_eq!(removed_pages(&two, &BTreeSet::from([(1, 9)]), &mut new_id), None);
    }
}
