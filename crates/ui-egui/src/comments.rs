//! Commenting in the document view (execution plan M5.3–M5.4; Acrobat's "Add comments").
//!
//! - **Tools** ([`CommentTool`]): sticky note, text box, highlight / underline / strikethrough
//!   (driven by text selection), freehand, line, arrow, rectangle, oval. They live in the quick
//!   bar in three flyout groups, as in Acrobat's comment toolbar (audit `a14-*`).
//! - **On the page:** drawing gestures with live previews; with the Select tool, comments can be
//!   hovered, selected (blue frame with 8 round handles), moved, resized (rectangles, ovals and
//!   text boxes), edited (double-click) and deleted (⌫ / Delete); a context menu offers Edit,
//!   Reply, Set status, Colour and Delete.
//! - **Composer:** the floating "Add a comment" card used for new notes, text boxes and edits.
//!
//! Everything is turned into `pdfcraft_engine::Edit`s, queued on the view as `pending_edit` and
//! applied by the app, so each change is one undo step.

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, pos2, vec2};
use pdfcraft_engine::{Edit, LineEnding, Markup, NewAnnotation, NoteIcon, ReviewState, Rgb, Shape, Style};
use pdfcraft_render::{Annotation, DocInfo};

use crate::canvas::{DocView, PageXform};
use crate::theme::{self, Tokens};
use crate::{QuickTool, icons};

/// Acrobat's selection blue for annotation frames and handles (11-visual-spec §5).
const SELECT_BLUE: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommentTool {
    Note,
    TextBox,
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
    Ink,
    Line,
    Arrow,
    Rectangle,
    Oval,
    Polygon,
    PolyLine,
    Cloud,
    Callout,
    Caret,
    ReplaceText,
    Attach,
    Eraser,
}

/// The quick-bar flyout groups, in Acrobat's order: Comment ▸, Highlight ▸, Draw ▸.
pub const GROUPS: [&[CommentTool]; 3] = [
    &[CommentTool::Note, CommentTool::TextBox, CommentTool::Callout, CommentTool::Attach],
    &[CommentTool::Highlight, CommentTool::Underline, CommentTool::StrikeOut, CommentTool::Squiggly, CommentTool::Caret, CommentTool::ReplaceText],
    &[
        CommentTool::Ink,
        CommentTool::Line,
        CommentTool::Arrow,
        CommentTool::Rectangle,
        CommentTool::Oval,
        CommentTool::PolyLine,
        CommentTool::Polygon,
        CommentTool::Cloud,
        CommentTool::Eraser,
    ],
];

pub const ALL: [CommentTool; 19] = [
    CommentTool::Eraser,
    CommentTool::ReplaceText,
    CommentTool::Attach,
    CommentTool::Polygon,
    CommentTool::PolyLine,
    CommentTool::Cloud,
    CommentTool::Callout,
    CommentTool::Caret,
    CommentTool::Note,
    CommentTool::TextBox,
    CommentTool::Highlight,
    CommentTool::Underline,
    CommentTool::StrikeOut,
    CommentTool::Squiggly,
    CommentTool::Ink,
    CommentTool::Line,
    CommentTool::Arrow,
    CommentTool::Rectangle,
    CommentTool::Oval,
];

impl CommentTool {
    pub fn command(self) -> &'static str {
        match self {
            Self::Note => "comment.note",
            Self::TextBox => "comment.freetext",
            Self::Highlight => "comment.highlight",
            Self::Underline => "comment.underline",
            Self::StrikeOut => "comment.strikeout",
            Self::Squiggly => "comment.squiggly",
            Self::Ink => "comment.ink",
            Self::Line => "comment.line",
            Self::Arrow => "comment.arrow",
            Self::Rectangle => "comment.square",
            Self::Oval => "comment.circle",
            Self::Polygon => "comment.polygon",
            Self::PolyLine => "comment.polyline",
            Self::Cloud => "comment.cloud",
            Self::Callout => "comment.callout",
            Self::Caret => "comment.caret",
            Self::ReplaceText => "comment.replace",
            Self::Attach => "comment.attach",
            Self::Eraser => "comment.eraser",
        }
    }

    pub fn from_command(id: &str) -> Option<Self> {
        ALL.into_iter().find(|t| t.command() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Note => "Add a sticky note",
            Self::TextBox => "Add a text box",
            Self::Highlight => "Highlight",
            Self::Underline => "Underline",
            Self::StrikeOut => "Strikethrough",
            Self::Squiggly => "Squiggly underline",
            Self::Ink => "Draw",
            Self::Line => "Line",
            Self::Arrow => "Arrow",
            Self::Rectangle => "Rectangle",
            Self::Oval => "Oval",
            Self::Polygon => "Polygon",
            Self::PolyLine => "Connected lines",
            Self::Cloud => "Cloud",
            Self::Callout => "Add a callout",
            Self::Caret => "Insert text",
            Self::ReplaceText => "Replace text",
            Self::Attach => "Attach file",
            Self::Eraser => "Eraser",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Note => "message-square-plus",
            Self::TextBox => "type",
            Self::Highlight => "highlighter",
            Self::Underline => "underline",
            Self::StrikeOut => "strikethrough",
            Self::Squiggly => "spline",
            Self::Ink => "pencil",
            Self::Line => "minus",
            Self::Arrow => "move-right",
            Self::Rectangle => "square",
            Self::Oval => "circle",
            Self::Polygon => "pentagon",
            Self::PolyLine => "spline",
            Self::Cloud => "cloud",
            Self::Callout => "message-square-quote",
            Self::Caret => "text-cursor-input",
            Self::ReplaceText => "replace",
            Self::Attach => "paperclip",
            Self::Eraser => "eraser",
        }
    }

    pub fn group(self) -> usize {
        GROUPS.iter().position(|g| g.contains(&self)).unwrap_or(0)
    }

    pub fn markup(self) -> Option<Markup> {
        match self {
            Self::Highlight => Some(Markup::Highlight),
            Self::Underline => Some(Markup::Underline),
            Self::StrikeOut => Some(Markup::StrikeOut),
            Self::Squiggly => Some(Markup::Squiggly),
            _ => None,
        }
    }

    /// Tools that draw with a drag gesture.
    pub fn draws(self) -> bool {
        matches!(self, Self::Ink | Self::Line | Self::Arrow | Self::Rectangle | Self::Oval | Self::Eraser)
    }

    /// Tools that place points one click at a time (double-click or Enter finishes).
    pub fn clicks_points(self) -> bool {
        matches!(self, Self::Polygon | Self::PolyLine | Self::Cloud)
    }

    /// Whether the line-thickness control applies.
    pub fn has_width(self) -> bool {
        self.draws() || self.clicks_points()
    }

    /// Whether the fill-colour control applies: shapes with an interior (#686).
    pub fn has_fill(self) -> bool {
        matches!(self, Self::Rectangle | Self::Oval | Self::Polygon | Self::Cloud)
    }

    /// A placeholder shape of this kind (for per-tool default styles).
    fn sample(self) -> Shape {
        match self {
            Self::Note => Shape::Note { at: [0.0; 2], icon: NoteIcon::Comment },
            Self::TextBox => Shape::TextBox { rect: [0.0; 4], font_size: 12.0 },
            Self::Highlight | Self::Underline | Self::StrikeOut | Self::Squiggly => {
                Shape::TextMarkup { kind: self.markup().unwrap_or(Markup::Highlight), quads: Vec::new() }
            }
            Self::Ink => Shape::Ink { strokes: Vec::new() },
            Self::Line => Shape::Line { from: [0.0; 2], to: [0.0; 2], start: LineEnding::None, end: LineEnding::None },
            Self::Arrow => Shape::Line { from: [0.0; 2], to: [0.0; 2], start: LineEnding::None, end: LineEnding::OpenArrow },
            Self::Rectangle => Shape::Rectangle { rect: [0.0; 4] },
            Self::Oval => Shape::Oval { rect: [0.0; 4] },
            Self::Polygon => Shape::Polygon { vertices: Vec::new(), cloud: false },
            Self::Cloud => Shape::Polygon { vertices: Vec::new(), cloud: true },
            Self::PolyLine => Shape::PolyLine { vertices: Vec::new(), start: LineEnding::None, end: LineEnding::None },
            Self::Callout => Shape::Callout { rect: [0.0; 4], knee: [0.0; 2], point: [0.0; 2], font_size: 10.0, ending: LineEnding::OpenArrow },
            Self::Caret => Shape::Caret { rect: [0.0; 4] },
            Self::ReplaceText => Shape::TextMarkup { kind: Markup::StrikeOut, quads: Vec::new() },
            Self::Eraser => Shape::Ink { strokes: Vec::new() },
            Self::Attach => Shape::Attachment { at: [0.0; 2], icon: pdfcraft_engine::AttachIcon::PushPin, file: String::new(), data: Vec::new() },
        }
    }
}

/// The comment swatches (our palette, close to Acrobat's quick colours).
pub const SWATCHES: [(&str, Rgb); 10] = [
    ("Yellow", [1.0, 0.94, 0.0]),
    ("Orange", [1.0, 0.54, 0.0]),
    ("Red", [0.89, 0.13, 0.13]),
    ("Pink", [1.0, 0.37, 0.64]),
    ("Purple", [0.54, 0.25, 0.82]),
    ("Blue", [0.0, 0.47, 0.84]),
    ("Light blue", [0.36, 0.75, 0.98]),
    ("Green", [0.18, 0.62, 0.36]),
    ("Gray", [0.5, 0.5, 0.5]),
    ("Black", [0.0, 0.0, 0.0]),
];

pub fn color32(c: Rgb) -> Color32 {
    let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(b(c[0]), b(c[1]), b(c[2]))
}

/// Commenting preferences shared by every document (author, per-tool styles, pin).
#[derive(Clone, Debug)]
pub struct CommentPrefs {
    /// `/T` of new comments (Acrobat: Preferences ▸ Identity, default the login name).
    pub author: String,
    styles: Vec<(CommentTool, Style)>,
    /// Keep the tool selected after use (the pin).
    pub pinned: bool,
    /// The tool each flyout group shows (the last one used).
    pub group_tool: [CommentTool; 3],
}

impl Default for CommentPrefs {
    fn default() -> Self {
        Self {
            author: login_name(),
            styles: ALL.iter().map(|t| (*t, Style::default_for(&t.sample()))).collect(),
            pinned: false,
            group_tool: [CommentTool::Note, CommentTool::Highlight, CommentTool::Ink],
        }
    }
}

impl CommentPrefs {
    pub fn style(&self, tool: CommentTool) -> Style {
        self.styles.iter().find(|(t, _)| *t == tool).map(|(_, s)| s.clone()).unwrap_or_default()
    }

    /// Every tool with its current default style (the persisted settings read this).
    pub fn styles(&self) -> impl Iterator<Item = (CommentTool, &Style)> {
        self.styles.iter().map(|(t, s)| (*t, s))
    }

    pub fn set_color(&mut self, tool: CommentTool, c: Rgb) {
        if let Some((_, s)) = self.styles.iter_mut().find(|(t, _)| *t == tool) {
            s.color = c;
        }
    }

    pub fn set_width(&mut self, tool: CommentTool, w: f64) {
        if let Some((_, s)) = self.styles.iter_mut().find(|(t, _)| *t == tool) {
            s.width = w.clamp(0.5, 12.0);
        }
    }

    /// The tool's opacity, 10–100 % (an invisible comment can't be found again).
    pub fn set_opacity(&mut self, tool: CommentTool, o: f64) {
        if let Some((_, s)) = self.styles.iter_mut().find(|(t, _)| *t == tool) {
            s.opacity = if o.is_finite() { o.clamp(0.1, 1.0) } else { 1.0 };
        }
    }

    /// The tool's fill (`None`: no fill). Only shapes with an interior keep one.
    pub fn set_fill(&mut self, tool: CommentTool, fill: Option<Rgb>) {
        if let Some((_, s)) = self.styles.iter_mut().find(|(t, _)| *t == tool) {
            s.fill = if tool.has_fill() { fill.filter(|c| c.iter().all(|x| x.is_finite())).map(|c| c.map(|x| x.clamp(0.0, 1.0))) } else { None };
        }
    }

    pub fn set_style(&mut self, tool: CommentTool, style: Style) {
        if let Some((_, s)) = self.styles.iter_mut().find(|(t, _)| *t == tool) {
            *s = style;
        }
    }
}

/// The tool that makes comments like `a` (for Make Current Properties Default).
pub fn tool_for(a: &Annotation) -> Option<CommentTool> {
    Some(match (a.subtype.as_str(), a.intent.as_deref()) {
        ("Text", _) => CommentTool::Note,
        ("FreeText", Some("FreeTextCallout")) => CommentTool::Callout,
        ("FreeText", Some("FreeTextTypeWriter")) => return None,
        ("FreeText", _) => CommentTool::TextBox,
        ("Highlight", _) => CommentTool::Highlight,
        ("Underline", _) => CommentTool::Underline,
        ("StrikeOut", _) => CommentTool::StrikeOut,
        ("Squiggly", _) => CommentTool::Squiggly,
        ("Ink", _) => CommentTool::Ink,
        ("Line", _) => CommentTool::Line,
        ("Square", _) => CommentTool::Rectangle,
        ("Circle", _) => CommentTool::Oval,
        ("Polygon", Some("PolygonCloud")) => CommentTool::Cloud,
        ("Polygon", _) => CommentTool::Polygon,
        ("PolyLine", _) => CommentTool::PolyLine,
        ("Caret", _) => CommentTool::Caret,
        ("FileAttachment", _) => CommentTool::Attach,
        _ => return None,
    })
}

/// The user's login name, which Acrobat uses as the default comment author.
fn login_name() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    for var in ["USER", "USERNAME", "LOGNAME"] {
        if let Ok(v) = std::env::var(var)
            && !v.trim().is_empty()
        {
            return v;
        }
    }
    "Guest".into()
}

/// A drag in progress on a page.
#[derive(Clone, Debug, PartialEq)]
pub enum Gesture {
    /// Drawing with a tool: points in user space (ink: the stroke; polygons: the vertices
    /// clicked so far; others: start and end).
    Draw { page: usize, tool: CommentTool, points: Vec<[f64; 2]> },
    /// Moving a comment, from the press position on screen.
    Move { page: usize, index: usize, from: Pos2 },
    /// Resizing a comment by one of its handles: (dx, dy) ∈ {-1, 0, 1}² says which sides move.
    Resize { page: usize, index: usize, handle: (i8, i8), from: Pos2, aspect_ratio: Option<f32> },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ComposerKind {
    Note,
    TextBox,
    /// A callout whose arrow touches `point`; the composer's anchor is the text box's top-left.
    Callout {
        point: [f64; 2],
    },
    /// Insert text at the anchor (the caret's tip).
    Caret,
    /// Replace the selected text (`CommentView::replace` holds the quads).
    Replace,
    /// Edit the text of the comment at this index on the composer's page.
    Edit(usize),
}

/// The floating "Add a comment" card.
#[derive(Clone, Debug, PartialEq)]
pub struct Composer {
    pub page: usize,
    /// Anchor in user space (the note's top-left, the text box's top-left, the comment's corner).
    pub at: [f64; 2],
    pub kind: ComposerKind,
    pub text: String,
    pub focus: bool,
}

/// Per-document commenting state.
#[derive(Clone, Debug, Default)]
pub struct CommentView {
    /// The selected comment: (page, index in `/Annots`).
    pub selected: Option<(usize, usize)>,
    pub gesture: Option<Gesture>,
    /// Escape ends egui's drag too; ignore its synthetic release until the mouse is up.
    cancelled_drag: bool,
    pub composer: Option<Composer>,
    /// Reply being typed under the selected card.
    pub reply: String,
    /// Inline edit of a card's text in the Comments panel: (page, index, text).
    pub editing: Option<(usize, usize, String)>,
    /// The panel should scroll the selected card into view.
    pub reveal: bool,
    /// The "Add a comment" box at the top of the Comments panel.
    pub add_box: String,
    /// The Comments panel's search query (`None`: search closed).
    pub search: Option<String>,
    /// Comment types, authors and statuses hidden by the panel's filter.
    pub hidden_types: Vec<String>,
    pub hidden_authors: Vec<String>,
    pub hidden_statuses: Vec<String>,
    /// Colours (`RRGGBB`, empty for none) and checkmark states ("Checked", "Unchecked") hidden.
    pub hidden_colors: Vec<String>,
    pub hidden_checks: Vec<String>,
    /// How the panel orders comments.
    pub sort: SortBy,
    /// Comment Properties was asked for from the panel: (page, index).
    pub props_request: Option<(usize, usize)>,
    /// Make Current Properties Default was asked for: (page, index).
    pub default_request: Option<(usize, usize)>,
    /// Replace Text: the struck-out text's quads while the replacement is typed.
    pub replace: Option<Vec<[f64; 8]>>,
    /// Attach file: where the icon goes (page, top-left in user space), for the app to pick a file.
    pub attach_at: Option<(usize, [f64; 2])>,
    pub search_focus: bool,
    /// Where the canvas context menu was opened: (page, user-space point).
    pub context_at: Option<(usize, [f64; 2])>,
    /// A one-shot tool just finished; the app returns to the Select tool unless pinned.
    pub tool_done: bool,
}

/// Comments panel order (Acrobat: Sort by page, author, date, type, colour).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortBy {
    #[default]
    Page,
    Author,
    Date,
    Type,
    Color,
}

/// What a page needs to know to handle comment input.
pub(crate) struct PageCx<'a> {
    pub page: usize,
    pub xf: &'a PageXform,
    pub info: &'a DocInfo,
    pub tool: QuickTool,
    pub prefs: &'a CommentPrefs,
    /// The document allows commenting.
    pub allowed: bool,
    /// Comments are hidden (Hide all comments): nothing to select.
    pub hidden: bool,
}

impl PageCx<'_> {
    fn to_user(&self, p: Pos2) -> [f64; 2] {
        let (vx, vy) = self.xf.screen_to_view(p);
        let u = self.info.pages[self.page].view_to_user(vx, vy);
        [u[0] as f64, u[1] as f64]
    }

    fn to_screen(&self, p: [f64; 2]) -> Pos2 {
        let v = self.info.pages[self.page].user_to_view(p[0] as f32, p[1] as f32);
        self.xf.norm_to_screen(v[0] / self.xf.pw, v[1] / self.xf.ph)
    }

    fn screen_rect(&self, a: &Annotation) -> Rect {
        self.xf.user_rect(self.info, self.page, a.rect)
    }

    /// Top-level comments on this page, in paint order.
    fn comments(&self) -> impl Iterator<Item = &Annotation> {
        self.info.annotations.iter().filter(move |a| !self.hidden && a.page == self.page && a.in_reply_to.is_none() && a.subtype != "Popup")
    }

    /// Where a comment is on screen: one rectangle, or one per marked line for text markup.
    fn screen_rects(&self, a: &Annotation) -> Vec<Rect> {
        if is_markup(&a.subtype) && !a.quads.is_empty() {
            a.quads
                .iter()
                .map(|q| {
                    let xs = [q[0], q[2], q[4], q[6]];
                    let ys = [q[1], q[3], q[5], q[7]];
                    let (x0, x1) = (xs.iter().copied().fold(f32::MAX, f32::min), xs.iter().copied().fold(f32::MIN, f32::max));
                    let (y0, y1) = (ys.iter().copied().fold(f32::MAX, f32::min), ys.iter().copied().fold(f32::MIN, f32::max));
                    self.xf.user_rect(self.info, self.page, [x0, y0, x1, y1])
                })
                .collect()
        } else {
            vec![self.screen_rect(a)]
        }
    }

    /// The topmost comment under `p`.
    fn hit(&self, p: Pos2) -> Option<&Annotation> {
        self.comments().filter(|a| self.screen_rects(a).iter().any(|r| r.expand(3.0).contains(p))).last()
    }

    pub(crate) fn get(&self, index: usize) -> Option<&Annotation> {
        self.comments().find(|a| a.index == index)
    }

    pub(crate) fn rect_to_user(&self, r: Rect) -> [f64; 4] {
        let (a, b) = (self.to_user(r.left_top()), self.to_user(r.right_bottom()));
        [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
    }

    /// The same geometry drives the live image, its handles, and the edit on release.
    pub(crate) fn adjusted_rect(&self, a: &Annotation, gesture: Option<&Gesture>, pointer: Option<Pos2>, pending: Option<&Edit>) -> Rect {
        let r = self.screen_rect(a);
        if let Some(Edit::ResizeAnnotation { page, index, rect }) = pending
            && (*page, *index) == (self.page, a.index)
        {
            return self.xf.user_rect(self.info, self.page, rect.map(|v| v as f32));
        }
        if let Some(Edit::MoveAnnotation { page, index, dx, dy }) = pending
            && (*page, *index) == (self.page, a.index)
        {
            return self.xf.user_rect(
                self.info,
                self.page,
                [a.rect[0] + *dx as f32, a.rect[1] + *dy as f32, a.rect[2] + *dx as f32, a.rect[3] + *dy as f32],
            );
        }
        match (gesture, pointer) {
            (Some(Gesture::Move { page, index, from }), Some(p)) if (*page, *index) == (self.page, a.index) => r.translate(p - *from),
            (Some(Gesture::Resize { page, index, handle, from, aspect_ratio }), Some(p)) if (*page, *index) == (self.page, a.index) => {
                resized(r, *handle, p - *from, *aspect_ratio)
            }
            _ => r,
        }
    }
}

fn is_markup(subtype: &str) -> bool {
    matches!(subtype, "Highlight" | "Underline" | "StrikeOut" | "Squiggly")
}

/// Rectangles, ovals, text boxes, stamps and drawings (drawn signatures among them). A callout's
/// `/Rect` also holds its leader line, so it only moves.
fn resizable(a: &Annotation) -> bool {
    matches!(a.subtype.as_str(), "Square" | "Circle" | "FreeText" | "Stamp" | "Ink") && a.intent.as_deref() != Some("FreeTextCallout")
}

const HANDLES: [(i8, i8); 8] = [(-1, -1), (0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0)];

fn handle_pos(r: Rect, (hx, hy): (i8, i8)) -> Pos2 {
    let x = match hx {
        -1 => r.left(),
        0 => r.center().x,
        _ => r.right(),
    };
    let y = match hy {
        -1 => r.top(),
        0 => r.center().y,
        _ => r.bottom(),
    };
    pos2(x, y)
}

fn resized(r: Rect, (hx, hy): (i8, i8), d: egui::Vec2, aspect_ratio: Option<f32>) -> Rect {
    if hx != 0
        && hy != 0
        && let Some(ratio) = aspect_ratio.filter(|v| v.is_finite() && *v > 0.0)
    {
        // As in content_ui, the larger requested dimension drives proportional corner resizing.
        // Keep the opposite corner fixed and prevent crossing it from flipping the image.
        let width = (r.width() + f32::from(hx) * d.x).max((r.height() + f32::from(hy) * d.y) * ratio).max(4.0).max(4.0 * ratio);
        let anchor = handle_pos(r, (-hx, -hy));
        return Rect::from_two_pos(anchor, anchor + vec2(f32::from(hx) * width, f32::from(hy) * width / ratio));
    }
    let mut r = r;
    match hx {
        -1 => r.min.x += d.x,
        1 => r.max.x += d.x,
        _ => {}
    }
    match hy {
        -1 => r.min.y += d.y,
        1 => r.max.y += d.y,
        _ => {}
    }
    Rect::from_two_pos(r.min, r.max)
}

/// Comment input on one page, before text selection runs. Returns `true` when the pointer
/// gesture belongs to commenting (text selection must ignore it).
pub(crate) fn page_input(ui: &egui::Ui, resp: &egui::Response, cx: &PageCx<'_>, view: &mut DocView) -> bool {
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let origin = ui.input(|i| i.pointer.press_origin());
    let page_rect = cx.xf.rect;
    let pressed_here = origin.is_some_and(|o| page_rect.contains(o));
    let over_page = pointer.is_some_and(|p| page_rect.contains(p));
    let fill_grab = fill_grabs(ui, cx, view);
    let cv = &mut view.comments;
    if cv.gesture.is_some() && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        cv.gesture = None;
        cv.cancelled_drag = true;
        return true;
    }
    if cv.cancelled_drag {
        return true;
    }
    if resp.secondary_clicked()
        && let Some(p) = pointer.filter(|p| page_rect.contains(*p))
    {
        cv.context_at = Some((cx.page, cx.to_user(p)));
        cv.selected = cx.hit(p).map(|a| (cx.page, a.index));
    }
    match cx.tool {
        QuickTool::Comment(tool) if tool.draws() => {
            if !cx.allowed {
                return false;
            }
            if over_page {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            if resp.drag_started()
                && pressed_here
                && let Some(o) = origin
            {
                cv.gesture = Some(Gesture::Draw { page: cx.page, tool, points: vec![cx.to_user(o)] });
                cv.selected = None;
            }
            if let Some(Gesture::Draw { page, tool, points }) = cv.gesture.as_mut()
                && *page == cx.page
                && let Some(p) = pointer
            {
                let p = clamp_to(page_rect, p);
                let u = cx.to_user(p);
                if matches!(*tool, CommentTool::Ink | CommentTool::Eraser) {
                    let last = points.last().map(|l| cx.to_screen(*l)).unwrap_or(p);
                    if last.distance(p) >= 1.5 {
                        points.push(u);
                    }
                } else {
                    points.truncate(1);
                    points.push(u);
                }
            }
            if resp.drag_stopped()
                && let Some(Gesture::Draw { page, tool, points }) = cv.gesture.clone()
                && page == cx.page
            {
                cv.gesture = None;
                if tool == CommentTool::Eraser {
                    view.pending_edit = erase(cx, &points);
                } else if let Some(shape) = drawn_shape(tool, &points) {
                    view.pending_edit = Some(new_comment(cx, tool, shape, String::new()));
                }
            }
            true
        }
        QuickTool::Comment(tool) if tool.clicks_points() => {
            if !cx.allowed {
                return false;
            }
            if over_page {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            let finish = resp.double_clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter));
            if resp.clicked()
                && over_page
                && let Some(p) = pointer
            {
                let u = cx.to_user(p);
                match cv.gesture.as_mut() {
                    Some(Gesture::Draw { page, tool: t, points }) if *page == cx.page && *t == tool => {
                        // Clicking the first point again closes a polygon.
                        let closes = points.len() >= 3 && tool != CommentTool::PolyLine && cx.to_screen(points[0]).distance(p) <= 6.0;
                        let repeat = points.last().is_some_and(|l| cx.to_screen(*l).distance(p) < 2.0);
                        if closes {
                            let pts = std::mem::take(points);
                            cv.gesture = None;
                            if let Some(shape) = drawn_shape(tool, &pts) {
                                view.pending_edit = Some(new_comment(cx, tool, shape, String::new()));
                            }
                            return true;
                        }
                        if !repeat {
                            points.push(u);
                        }
                    }
                    _ => {
                        cv.gesture = Some(Gesture::Draw { page: cx.page, tool, points: vec![u] });
                        cv.selected = None;
                    }
                }
            }
            if finish
                && let Some(Gesture::Draw { page, tool: t, points }) = cv.gesture.clone()
                && page == cx.page
                && t == tool
            {
                cv.gesture = None;
                if let Some(shape) = drawn_shape(tool, &points) {
                    view.pending_edit = Some(new_comment(cx, tool, shape, String::new()));
                }
            }
            true
        }
        QuickTool::Comment(CommentTool::Callout) => {
            // Drag from what the callout points at to where its text box goes.
            if !cx.allowed {
                return false;
            }
            if over_page {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            if resp.drag_started()
                && pressed_here
                && let Some(o) = origin
            {
                cv.gesture = Some(Gesture::Draw { page: cx.page, tool: CommentTool::Callout, points: vec![cx.to_user(o)] });
                cv.selected = None;
            }
            if let Some(Gesture::Draw { page, points, .. }) = cv.gesture.as_mut()
                && *page == cx.page
                && let Some(p) = pointer
            {
                points.truncate(1);
                points.push(cx.to_user(clamp_to(page_rect, p)));
            }
            if resp.drag_stopped()
                && let Some(Gesture::Draw { page, points, .. }) = cv.gesture.clone()
                && page == cx.page
            {
                cv.gesture = None;
                if let [point, at] = points[..]
                    && (point[0] - at[0]).hypot(point[1] - at[1]) >= 8.0
                {
                    cv.composer = Some(Composer { page, at, kind: ComposerKind::Callout { point }, text: String::new(), focus: true });
                }
            }
            true
        }
        QuickTool::Comment(CommentTool::Attach) => {
            if !cx.allowed {
                return false;
            }
            if over_page {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            if resp.clicked()
                && over_page
                && let Some(p) = pointer
            {
                cv.attach_at = Some((cx.page, cx.to_user(p)));
                cv.selected = None;
            }
            true
        }
        QuickTool::Comment(CommentTool::Caret) => {
            if !cx.allowed {
                return false;
            }
            if over_page {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
            }
            if resp.clicked()
                && over_page
                && let Some(p) = pointer
            {
                cv.composer = Some(Composer { page: cx.page, at: cx.to_user(p), kind: ComposerKind::Caret, text: String::new(), focus: true });
                cv.selected = None;
            }
            true
        }
        QuickTool::Comment(tool @ (CommentTool::Note | CommentTool::TextBox)) => {
            if !cx.allowed {
                return false;
            }
            if over_page {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            if resp.clicked()
                && over_page
                && let Some(p) = pointer
            {
                let kind = if tool == CommentTool::Note { ComposerKind::Note } else { ComposerKind::TextBox };
                cv.composer = Some(Composer { page: cx.page, at: cx.to_user(p), kind, text: String::new(), focus: true });
                cv.selected = None;
            }
            true
        }
        QuickTool::Select => select_input(ui, resp, cx, view, pointer, origin, pressed_here),
        QuickTool::Fill(_) if fill_grab => select_input(ui, resp, cx, view, pointer, origin, pressed_here),
        _ => false,
    }
}

/// With a Fill & Sign tool, the Fill & Sign marks already on the page are picked up as with the
/// Select tool, as in Acrobat: hovering one shows the move cursor, a click selects it, a drag
/// moves it and the selected one's handles resize it. Elsewhere the tool places its mark. While
/// the button is down the press decides; otherwise the pointer (egui clears the press origin on
/// release, so a click is located by the pointer).
pub(crate) fn fill_grabs(ui: &egui::Ui, cx: &PageCx<'_>, view: &DocView) -> bool {
    matches!(cx.tool, QuickTool::Fill(_)) && grabs(ui, cx, view, |a| a.fill_sign)
}

/// With the Select tool, a comment that would move (not text markup) is picked up before a form
/// field under it, so a signature placed over a field can still be dragged off it.
pub(crate) fn select_grabs(ui: &egui::Ui, cx: &PageCx<'_>, view: &DocView) -> bool {
    cx.tool == QuickTool::Select && grabs(ui, cx, view, |a| !is_markup(&a.subtype))
}

/// Whether the press (or, with no button down, the pointer) is on a `pick` comment or on the
/// selected one's resize handles.
fn grabs(ui: &egui::Ui, cx: &PageCx<'_>, view: &DocView, pick: impl Fn(&Annotation) -> bool) -> bool {
    if cx.hidden {
        return false;
    }
    let cv = &view.comments;
    if matches!(cv.gesture, Some(Gesture::Move { page, .. } | Gesture::Resize { page, .. }) if page == cx.page) {
        return true;
    }
    let (pointer, origin, down) = ui.input(|i| (i.pointer.hover_pos(), i.pointer.press_origin(), i.pointer.any_down()));
    let Some(p) = (if down { origin } else { pointer }).filter(|p| cx.xf.rect.contains(*p)) else { return false };
    let on_handle = cv
        .selected
        .filter(|(page, _)| *page == cx.page)
        .and_then(|(_, i)| cx.get(i))
        .filter(|a| pick(a) && cx.allowed && resizable(a))
        .is_some_and(|a| HANDLES.into_iter().any(|h| handle_pos(cx.screen_rect(a), h).distance(p) <= 7.0));
    on_handle || cx.hit(p).is_some_and(pick)
}

fn clamp_to(r: Rect, p: Pos2) -> Pos2 {
    pos2(p.x.clamp(r.left(), r.right()), p.y.clamp(r.top(), r.bottom()))
}

/// The Select tool: hover, select, move, resize, edit, delete.
fn select_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    cx: &PageCx<'_>,
    view: &mut DocView,
    pointer: Option<Pos2>,
    origin: Option<Pos2>,
    pressed_here: bool,
) -> bool {
    let cv = &mut view.comments;
    let selected = cv.selected.filter(|(p, _)| *p == cx.page).and_then(|(_, i)| cx.get(i));
    // Handles of a selected, resizable comment.
    let handle_at = |p: Pos2| -> Option<(i8, i8)> {
        let a = selected.filter(|a| cx.allowed && resizable(a))?;
        let r = cx.screen_rect(a);
        HANDLES.into_iter().find(|h| handle_pos(r, *h).distance(p) <= 7.0)
    };
    let mut consumed = false;
    if let Some(p) = pointer {
        if let Some(h) = handle_at(p) {
            ui.ctx().set_cursor_icon(match h {
                (0, _) => egui::CursorIcon::ResizeVertical,
                (_, 0) => egui::CursorIcon::ResizeHorizontal,
                (-1, -1) | (1, 1) => egui::CursorIcon::ResizeNwSe,
                _ => egui::CursorIcon::ResizeNeSw,
            });
        } else if let Some(a) = cx.hit(p) {
            let movable = cx.allowed && !is_markup(&a.subtype);
            ui.ctx().set_cursor_icon(if movable { egui::CursorIcon::Move } else { egui::CursorIcon::PointingHand });
        }
    }
    if resp.drag_started()
        && pressed_here
        && let Some(o) = origin
    {
        if let (Some(h), Some(a)) = (handle_at(o), selected) {
            // On screen, the image is also turned by the view's rotation.
            let turned = !cx.xf.rot.is_multiple_of(180);
            let aspect_ratio = view.signature_drag.aspect_ratio(cx.page, a.index).map(|ratio| if turned { ratio.recip() } else { ratio });
            // A drawn signature keeps its shape from the corners, as image signatures do.
            let aspect_ratio = aspect_ratio.or_else(|| {
                let r = cx.screen_rect(a);
                (a.fill_sign && a.subtype == "Ink" && r.height() > 0.0).then(|| r.width() / r.height())
            });
            cv.gesture = Some(Gesture::Resize { page: cx.page, index: a.index, handle: h, from: o, aspect_ratio });
            consumed = true;
        } else if let Some(a) = cx.hit(o)
            && !is_markup(&a.subtype)
        {
            cv.selected = Some((cx.page, a.index));
            cv.reveal = true;
            if cx.allowed {
                cv.gesture = Some(Gesture::Move { page: cx.page, index: a.index, from: o });
            }
            consumed = true;
        }
    }
    if matches!(cv.gesture, Some(Gesture::Move { page, .. } | Gesture::Resize { page, .. }) if page == cx.page) {
        consumed = true;
    }
    if resp.drag_stopped()
        && let Some(p) = pointer
    {
        match cv.gesture.clone() {
            Some(Gesture::Move { page, index, from }) if page == cx.page => {
                cv.gesture = None;
                let (a, b) = (cx.to_user(from), cx.to_user(p));
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                if from.distance(p) >= 2.0 {
                    view.pending_edit = Some(Edit::MoveAnnotation { page, index, dx, dy });
                }
            }
            Some(Gesture::Resize { page, index, handle, from, aspect_ratio }) if page == cx.page => {
                cv.gesture = None;
                if let Some(a) = cx.get(index) {
                    let r = resized(cx.screen_rect(a), handle, p - from, aspect_ratio);
                    let rect = cx.rect_to_user(r);
                    if r.width() >= 4.0 && r.height() >= 4.0 {
                        view.pending_edit = Some(Edit::ResizeAnnotation { page, index, rect });
                    }
                }
            }
            _ => {}
        }
    }
    // (egui clears the press origin on release, so clicks are located by the pointer.)
    let on_page = pointer.is_some_and(|p| cx.xf.rect.contains(p));
    if resp.clicked() && on_page {
        match pointer.and_then(|p| cx.hit(p)) {
            Some(a) => {
                cv.selected = Some((cx.page, a.index));
                cv.reveal = true;
                consumed = true;
            }
            None => cv.selected = None,
        }
    }
    if resp.double_clicked()
        && on_page
        && cx.allowed
        && let Some(a) = pointer.and_then(|p| cx.hit(p))
    {
        cv.selected = Some((cx.page, a.index));
        cv.composer = Some(Composer {
            page: cx.page,
            at: [a.rect[2] as f64, a.rect[3] as f64],
            kind: ComposerKind::Edit(a.index),
            text: a.contents.clone().unwrap_or_default(),
            focus: true,
        });
        consumed = true;
    }
    consumed
}

/// After text selection ran: a markup tool turns a finished selection into a comment; Replace
/// Text asks for the replacement.
pub(crate) fn page_after_text(resp: &egui::Response, cx: &PageCx<'_>, view: &mut DocView) {
    let QuickTool::Comment(tool) = cx.tool else { return };
    if tool == CommentTool::ReplaceText && cx.allowed && (resp.drag_stopped() || resp.double_clicked()) {
        if let Some((page, quads)) = view.selection_quads(cx.info).filter(|(p, _)| *p == cx.page) {
            view.clear_selection();
            let last = quads.last().copied().unwrap_or_default();
            let at = [last[2].max(last[6]), last[1].max(last[3])];
            view.comments.replace = Some(quads);
            view.comments.composer = Some(Composer { page, at, kind: ComposerKind::Replace, text: String::new(), focus: true });
        }
        return;
    }
    if tool.markup().is_none() || !cx.allowed || !(resp.drag_stopped() || resp.double_clicked()) {
        return;
    }
    if let Some(kind) = tool.markup()
        && let Some((page, quads)) = view.selection_quads(cx.info).filter(|(p, _)| *p == cx.page)
    {
        view.clear_selection();
        view.pending_edit = Some(new_comment(cx, tool, Shape::TextMarkup { kind, quads }, String::new()));
        let _ = page;
    }
}

/// Paint comment selection, hover and gesture previews on a page.
pub(crate) fn paint_page(ui: &egui::Ui, painter: &egui::Painter, cx: &PageCx<'_>, view: &DocView) {
    let cv = &view.comments;
    let pointer = ui.input(|i| i.pointer.hover_pos());
    if cv.gesture.is_none()
        && let Some(a) = pointer.and_then(|p| cx.hit(p))
        && (cx.tool == QuickTool::Select || matches!(cx.tool, QuickTool::Fill(_)) && a.fill_sign)
        && cv.selected != Some((cx.page, a.index))
    {
        for r in cx.screen_rects(a) {
            painter.rect_stroke(r.expand(2.0), CornerRadius::same(2), Stroke::new(1.0, SELECT_BLUE.gamma_multiply(0.7)), egui::StrokeKind::Outside);
        }
    }
    if let Some((page, index)) = cv.selected
        && page == cx.page
        && let Some(a) = cx.get(index)
    {
        if is_markup(&a.subtype) && !a.quads.is_empty() {
            for r in cx.screen_rects(a) {
                painter.rect_stroke(r.expand(2.0), CornerRadius::ZERO, Stroke::new(1.0, SELECT_BLUE), egui::StrokeKind::Middle);
            }
            return paint_gesture(painter, cx, view);
        }
        if view.signature_drag.contains(page, index) && matches!(cv.gesture, Some(Gesture::Move { page: p, index: i, .. }) if (p, i) == (page, index))
        {
            return paint_gesture(painter, cx, view);
        }
        let r = cx.adjusted_rect(a, cv.gesture.as_ref(), pointer, view.pending_edit.as_ref()).expand(2.0);
        painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, SELECT_BLUE), egui::StrokeKind::Middle);
        if cx.allowed && resizable(a) {
            for h in HANDLES {
                let c = handle_pos(r, h);
                painter.circle(c, 4.5, Color32::WHITE, Stroke::new(1.0, SELECT_BLUE));
            }
        }
    }
    paint_gesture(painter, cx, view);
}

fn paint_gesture(painter: &egui::Painter, cx: &PageCx<'_>, view: &DocView) {
    if let Some(Gesture::Draw { page, tool, points }) = &view.comments.gesture
        && *page == cx.page
    {
        let style = cx.prefs.style(*tool);
        let zoom = cx.xf.rect.width() / cx.xf.pw.max(1.0);
        let stroke = Stroke::new((style.width as f32 * zoom).max(1.0), color32(style.color));
        let pts: Vec<Pos2> = points.iter().map(|p| cx.to_screen(*p)).collect();
        match tool {
            CommentTool::Ink => {
                painter.add(egui::Shape::line(pts, stroke));
            }
            CommentTool::Eraser => {
                painter.add(egui::Shape::line(pts, Stroke::new(12.0, Color32::from_gray(128).gamma_multiply(0.35))));
            }
            CommentTool::Line | CommentTool::Arrow if pts.len() == 2 => {
                painter.line_segment([pts[0], pts[1]], stroke);
                if *tool == CommentTool::Arrow {
                    let d = (pts[0] - pts[1]).normalized();
                    let s = (6.0 + 3.0 * style.width as f32) * zoom;
                    let rot = |v: egui::Vec2, a: f32| vec2(v.x * a.cos() - v.y * a.sin(), v.x * a.sin() + v.y * a.cos());
                    for a in [0.52f32, -0.52] {
                        painter.line_segment([pts[1], pts[1] + rot(d, a) * s], stroke);
                    }
                }
            }
            CommentTool::Rectangle if pts.len() == 2 => {
                painter.rect_stroke(Rect::from_two_pos(pts[0], pts[1]), CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
            }
            CommentTool::Oval if pts.len() == 2 => {
                let r = Rect::from_two_pos(pts[0], pts[1]);
                painter.add(egui::Shape::ellipse_stroke(r.center(), r.size() / 2.0, stroke));
            }
            CommentTool::Polygon | CommentTool::PolyLine | CommentTool::Cloud => {
                // The vertices so far, and a rubber band to the pointer.
                let mut line = pts.clone();
                if let Some(p) = painter.ctx().input(|i| i.pointer.hover_pos()) {
                    line.push(p);
                }
                painter.add(egui::Shape::line(line, stroke));
                if let Some(first) = pts.first().filter(|_| *tool != CommentTool::PolyLine && pts.len() >= 3) {
                    painter.circle_stroke(*first, 5.0, Stroke::new(1.0, SELECT_BLUE));
                }
            }
            CommentTool::Callout if pts.len() == 2 => {
                painter.line_segment([pts[0], pts[1]], stroke);
                painter.rect_stroke(Rect::from_min_size(pts[1], vec2(160.0, 40.0) * zoom), CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
            }
            _ => {}
        }
    }
}

/// The shape a finished drawing gesture makes (`None` if it is too small to mean anything).
fn drawn_shape(tool: CommentTool, points: &[[f64; 2]]) -> Option<Shape> {
    let (first, last) = (*points.first()?, *points.last()?);
    let far = (first[0] - last[0]).hypot(first[1] - last[1]) >= 2.0;
    let rect = [first[0].min(last[0]), first[1].min(last[1]), first[0].max(last[0]), first[1].max(last[1])];
    let big = rect[2] - rect[0] >= 2.0 && rect[3] - rect[1] >= 2.0;
    match tool {
        CommentTool::Ink if points.len() >= 2 => Some(Shape::Ink { strokes: vec![points.to_vec()] }),
        CommentTool::Line | CommentTool::Arrow if far => Some(Shape::Line {
            from: first,
            to: last,
            start: LineEnding::None,
            end: if tool == CommentTool::Arrow { LineEnding::OpenArrow } else { LineEnding::None },
        }),
        CommentTool::Rectangle if big => Some(Shape::Rectangle { rect }),
        CommentTool::Oval if big => Some(Shape::Oval { rect }),
        CommentTool::Polygon | CommentTool::Cloud if points.len() >= 3 => {
            Some(Shape::Polygon { vertices: points.to_vec(), cloud: tool == CommentTool::Cloud })
        }
        CommentTool::PolyLine if points.len() >= 2 => {
            Some(Shape::PolyLine { vertices: points.to_vec(), start: LineEnding::None, end: LineEnding::None })
        }
        _ => None,
    }
}

/// Eraser: rub out drawings along `path` (user space); one undo step for all of them.
fn erase(cx: &PageCx<'_>, path: &[[f64; 2]]) -> Option<Edit> {
    let zoom = (cx.xf.rect.width() / cx.xf.pw.max(1.0)) as f64;
    let radius = (6.0 / zoom).max(1.0);
    let b = path.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |r, p| [r[0].min(p[0]), r[1].min(p[1]), r[2].max(p[0]), r[3].max(p[1])]);
    let mut hits: Vec<usize> = cx
        .comments()
        .filter(|a| a.subtype == "Ink" && !a.locked)
        .filter(|a| {
            let r = a.rect;
            (r[0] as f64) <= b[2] + radius && (r[2] as f64) >= b[0] - radius && (r[1] as f64) <= b[3] + radius && (r[3] as f64) >= b[1] - radius
        })
        .map(|a| a.index)
        .collect();
    if hits.is_empty() {
        return None;
    }
    // Later indices first: deleting an emptied drawing shifts the ones after it.
    hits.sort_unstable_by(|a, b| b.cmp(a));
    let edits = hits.into_iter().map(|index| Edit::EraseInk { page: cx.page, index, path: path.to_vec(), radius }).collect();
    Some(Edit::Batch { label: "Erase".into(), edits })
}

fn new_comment(cx: &PageCx<'_>, tool: CommentTool, shape: Shape, contents: String) -> Edit {
    Edit::AddAnnotation(NewAnnotation { page: cx.page, shape, style: cx.prefs.style(tool), contents, author: cx.prefs.author.clone() })
}

/// The floating composer card (new note, new text box, edit). Returns the edit to apply.
pub(crate) fn composer(ctx: &egui::Context, view: &mut DocView, info: &DocInfo, prefs: &CommentPrefs) -> Option<Edit> {
    let c = view.comments.composer.as_ref()?;
    let page = c.page;
    let Some(xf) = view.page_xform(page) else {
        // Scrolled away: keep the draft until the page is back.
        return None;
    };
    let cx = PageCx { page, xf: &xf, info, tool: QuickTool::Select, prefs, allowed: true, hidden: false };
    let anchor = cx.to_screen(c.at);
    let t = Tokens::get(ctx);
    let mut post = false;
    let mut cancel = false;
    let c = view.comments.composer.as_mut()?;
    let title = match c.kind {
        ComposerKind::Note => "Sticky note",
        ComposerKind::TextBox => "Text box",
        ComposerKind::Callout { .. } => "Callout",
        ComposerKind::Caret => "Inserted text",
        ComposerKind::Replace => "Replacement text",
        ComposerKind::Edit(_) => "Edit comment",
    };
    let pos = pos2(anchor.x + 12.0, anchor.y);
    egui::Area::new(egui::Id::new(("comment-composer", view.id.0))).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(12)).corner_radius(CornerRadius::same(8)).show(ui, |ui| {
            ui.set_width(260.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&prefs.author).font(theme::semibold(13.0)));
                ui.label(egui::RichText::new(tl!(title)).font(theme::regular(11.5)).color(t.text_faint));
            });
            ui.add_space(6.0);
            let hint = match c.kind {
                ComposerKind::TextBox | ComposerKind::Callout { .. } => tl!("Type text"),
                ComposerKind::Caret => tl!("Text to insert"),
                ComposerKind::Replace => tl!("Replacement text"),
                _ => tl!("Add a comment"),
            };
            let edit =
                ui.add(egui::TextEdit::multiline(&mut c.text).hint_text(hint).desired_rows(3).desired_width(f32::INFINITY).id_salt("composer-text"));
            if c.focus {
                edit.request_focus();
                c.focus = false;
            }
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_post = !c.text.trim().is_empty() || matches!(c.kind, ComposerKind::Edit(_));
                    if ui
                        .add_enabled(
                            can_post,
                            egui::Button::new(egui::RichText::new(tl!("Post")).color(Color32::WHITE)).fill(t.accent).corner_radius(14),
                        )
                        .clicked()
                        || (enter && can_post)
                    {
                        post = true;
                    }
                    if ui.add(egui::Button::new(tl!("Cancel")).corner_radius(14)).clicked() {
                        cancel = true;
                    }
                });
            });
        });
    });
    if cancel {
        view.comments.composer = None;
        return None;
    }
    if !post {
        return None;
    }
    let c = view.comments.composer.take()?;
    let text = c.text.trim_end().to_string();
    match c.kind {
        ComposerKind::Edit(index) => Some(Edit::SetAnnotationContents { page, index, text }),
        ComposerKind::Note => {
            view.comments.tool_done = true;
            Some(new_comment(&cx, CommentTool::Note, Shape::Note { at: c.at, icon: NoteIcon::Comment }, text))
        }
        ComposerKind::TextBox => {
            view.comments.tool_done = true;
            Some(new_comment(&cx, CommentTool::TextBox, Shape::TextBox { rect: text_box_rect(c.at, &text, 12.0), font_size: 12.0 }, text))
        }
        ComposerKind::Callout { point } => {
            view.comments.tool_done = true;
            let rect = text_box_rect(c.at, &text, 10.0);
            let side = if point[0] < rect[0] {
                rect[0]
            } else if point[0] > rect[2] {
                rect[2]
            } else {
                (rect[0] + rect[2]) / 2.0
            };
            let knee = [(point[0] + side) / 2.0, (rect[1] + rect[3]) / 2.0];
            Some(new_comment(&cx, CommentTool::Callout, Shape::Callout { rect, knee, point, font_size: 10.0, ending: LineEnding::OpenArrow }, text))
        }
        ComposerKind::Replace => {
            view.comments.tool_done = true;
            let quads = view.comments.replace.take()?;
            Some(Edit::ReplaceText {
                page,
                quads,
                text,
                author: prefs.author.clone(),
                strike: prefs.style(CommentTool::ReplaceText),
                caret: prefs.style(CommentTool::Caret),
            })
        }
        ComposerKind::Caret => {
            view.comments.tool_done = true;
            let [x, y] = c.at;
            Some(new_comment(&cx, CommentTool::Caret, Shape::Caret { rect: [x - 4.0, y - 8.0, x + 4.0, y] }, text))
        }
    }
}

/// A text box sized to its text (at most 300 pt wide), hanging from its top-left corner.
pub fn text_box_rect(at: [f64; 2], text: &str, size: f64) -> [f64; 4] {
    use pdfcraft_engine::annot_text::{text_measure, text_wrap};
    let pad = 2.0;
    let longest = text_measure(text, size);
    let w = (longest + 2.0 * pad + 4.0).clamp(40.0, 300.0);
    let lines = text_wrap(text, size, w - 2.0 * pad).len().max(1);
    let h = lines as f64 * size * 1.2 + 2.0 * pad + 2.0;
    [at[0], at[1] - h, at[0] + w, at[1]]
}

/// Delete / Escape handling for comments (only while no text field has focus).
pub(crate) fn keys(ctx: &egui::Context, view: &mut DocView, tool: &mut QuickTool, allowed: bool) {
    if !ctx.input(|i| i.pointer.any_down()) {
        view.comments.cancelled_drag = false;
    }
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    let cv = &mut view.comments;
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        // One step back per press: cancel the gesture, then deselect, then drop the tool.
        let cancelled = cv.gesture.take().is_some() || cv.selected.take().is_some();
        if !cancelled && matches!(tool, QuickTool::Comment(_)) {
            *tool = QuickTool::Select;
        }
    }
    if allowed
        && let Some((page, index)) = cv.selected
        && ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
    {
        cv.selected = None;
        view.pending_edit = Some(Edit::DeleteAnnotation { page, index });
    }
}

/// Items of the canvas context menu for the selected comment (or the page).
pub(crate) fn context_menu(ui: &mut egui::Ui, view: &mut DocView, info: &DocInfo, prefs: &CommentPrefs, allowed: bool) -> Option<CanvasAction> {
    let mut action = None;
    let selected = view.comments.selected.and_then(|(p, i)| info.annotations.iter().find(|a| a.page == p && a.index == i && a.in_reply_to.is_none()));
    match selected {
        Some(a) => {
            let (page, index) = (a.page, a.index);
            if ui.add_enabled(allowed, egui::Button::new(tl!("Edit text…"))).clicked() {
                view.comments.composer = Some(Composer {
                    page,
                    at: [a.rect[2] as f64, a.rect[3] as f64],
                    kind: ComposerKind::Edit(index),
                    text: a.contents.clone().unwrap_or_default(),
                    focus: true,
                });
                ui.close();
            }
            if ui.add_enabled(allowed, egui::Button::new(tl!("Reply"))).clicked() {
                view.comments.reveal = true;
                action = Some(CanvasAction::OpenComments);
                ui.close();
            }
            ui.add_enabled_ui(allowed, |ui| {
                ui.menu_button(tl!("Set status"), |ui| {
                    for s in [ReviewState::None, ReviewState::Accepted, ReviewState::Cancelled, ReviewState::Completed, ReviewState::Rejected] {
                        if ui.button(tl!(s.name())).clicked() {
                            action =
                                Some(CanvasAction::Edit(Box::new(Edit::SetAnnotationStatus { page, index, state: s, author: prefs.author.clone() })));
                            ui.close();
                        }
                    }
                });
                ui.menu_button(tl!("Colour"), |ui| {
                    if let Some(c) = swatch_grid(ui, a.color.map(|c| c.map(f64::from))) {
                        action = Some(CanvasAction::Edit(Box::new(Edit::StyleAnnotation {
                            page,
                            index,
                            color: Some(c),
                            opacity: None,
                            width: None,
                            endings: None,
                        })));
                        ui.close();
                    }
                });
            });
            let thread: Vec<&pdfcraft_render::Annotation> = info.annotations.iter().filter(|r| a.name.is_some() && r.in_reply_to == a.name).collect();
            let marked = crate::comments_panel::is_marked(&thread);
            if ui.add_enabled(allowed, egui::Button::new(tl!(if marked { "Remove checkmark" } else { "Mark with checkmark" }))).clicked() {
                action = Some(CanvasAction::Edit(Box::new(Edit::MarkAnnotation { page, index, marked: !marked, author: prefs.author.clone() })));
                ui.close();
            }
            if ui.button(tl!("Copy text")).clicked() {
                ui.ctx().copy_text(a.contents.clone().unwrap_or_default());
                ui.close();
            }
            ui.separator();
            if ui.add_enabled(allowed && !a.locked, egui::Button::new(tl!("Delete"))).clicked() {
                view.comments.selected = None;
                action = Some(CanvasAction::Edit(Box::new(Edit::DeleteAnnotation { page, index })));
                ui.close();
            }
            if ui.add_enabled(allowed, egui::Button::new(tl!("Properties…"))).clicked() {
                action = Some(CanvasAction::Properties(page, index));
                ui.close();
            }
            if ui.add_enabled(tool_for(a).is_some(), egui::Button::new(tl!("Make Current Properties Default"))).clicked() {
                view.comments.default_request = Some((page, index));
                ui.close();
            }
        }
        None => {
            if let Some((page, at)) = view.comments.context_at
                && ui.add_enabled(allowed, egui::Button::new(tl!("Add a sticky note here"))).clicked()
            {
                view.comments.composer = Some(Composer { page, at, kind: ComposerKind::Note, text: String::new(), focus: true });
                ui.close();
            }
            if ui.button(tl!("Comments panel")).clicked() {
                action = Some(CanvasAction::OpenComments);
                ui.close();
            }
        }
    }
    action
}

/// What the canvas asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum CanvasAction {
    Edit(Box<Edit>),
    OpenComments,
    /// Comment Properties for (page, index).
    Properties(usize, usize),
}

/// A grid of colour swatches; returns the one clicked.
pub fn swatch_grid(ui: &mut egui::Ui, current: Option<Rgb>) -> Option<Rgb> {
    let mut picked = None;
    egui::Grid::new(ui.id().with("swatches")).spacing(vec2(6.0, 6.0)).show(ui, |ui| {
        for (i, (name, c)) in SWATCHES.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
            let on = current.is_some_and(|cur| cur.iter().zip(c).all(|(a, b)| (a - b).abs() < 0.02));
            ui.painter().circle_filled(r.center(), 9.0, color32(*c));
            ui.painter().circle_stroke(r.center(), 9.0, Stroke::new(1.0, Color32::from_black_alpha(40)));
            if on || resp.hovered() {
                ui.painter().circle_stroke(r.center(), 11.0, Stroke::new(1.5, SELECT_BLUE));
            }
            if resp.on_hover_text(tl!(name)).clicked() {
                picked = Some(*c);
            }
            if i % 5 == 4 {
                ui.end_row();
            }
        }
    });
    picked
}

/// "No fill" and the colour swatches; returns the choice clicked (`Some(None)`: no fill).
pub fn fill_picker(ui: &mut egui::Ui, current: Option<Rgb>) -> Option<Option<Rgb>> {
    let mut picked = None;
    ui.push_id("fill-picker", |ui| {
        ui.vertical(|ui| {
            if ui.selectable_label(current.is_none(), tl!("No fill")).clicked() {
                picked = Some(None);
            }
            if let Some(c) = swatch_grid(ui, current) {
                picked = Some(Some(c));
            }
        });
    });
    picked
}

/// The comment tools' extra quick-bar controls: pin, colour, fill, opacity and thickness.
pub(crate) fn quick_bar_controls(ui: &mut egui::Ui, tool: CommentTool, prefs: &mut CommentPrefs) {
    let style = prefs.style(tool);
    if icons::button(ui, "pin", 32.0, prefs.pinned, if prefs.pinned { tl!("Keep tool selected: on") } else { tl!("Keep tool selected") }).clicked() {
        prefs.pinned = !prefs.pinned;
    }
    let (r, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    ui.painter().circle_filled(r.center(), 9.0, color32(style.color));
    ui.painter().circle_stroke(r.center(), 9.0, Stroke::new(1.0, Color32::from_black_alpha(50)));
    let resp = resp.on_hover_text(tl!("Colour"));
    egui::Popup::menu(&resp).align(egui::RectAlign::RIGHT_START).show(|ui| {
        if let Some(c) = swatch_grid(ui, Some(style.color)) {
            prefs.set_color(tool, c);
            ui.close();
        }
    });
    if tool.has_fill() {
        let resp = fill_button(ui, style.fill);
        egui::Popup::menu(&resp).align(egui::RectAlign::RIGHT_START).show(|ui| {
            ui.label(egui::RichText::new(tl!("Fill colour")).font(theme::semibold(12.0)));
            if let Some(f) = fill_picker(ui, style.fill) {
                prefs.set_fill(tool, f);
                ui.close();
            }
        });
    }
    let resp = icons::button(ui, "blend", 32.0, false, tl!("Opacity"));
    egui::Popup::menu(&resp).align(egui::RectAlign::RIGHT_START).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_min_width(180.0);
        let mut percent = (style.opacity * 100.0).round();
        ui.label(egui::RichText::new(tl!("Opacity")).font(theme::semibold(12.0)));
        if ui.add(egui::Slider::new(&mut percent, 10.0..=100.0).step_by(5.0).suffix(" %")).changed() {
            prefs.set_opacity(tool, percent / 100.0);
        }
    });
    if tool.has_width() {
        let resp = icons::button(ui, "sliders-horizontal", 32.0, false, tl!("Line thickness"));
        egui::Popup::menu(&resp).align(egui::RectAlign::RIGHT_START).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.set_min_width(180.0);
            let mut w = style.width;
            ui.label(egui::RichText::new(tl!("Line thickness")).font(theme::semibold(12.0)));
            if ui.add(egui::Slider::new(&mut w, 0.5..=12.0).step_by(0.5).suffix(" pt")).changed() {
                prefs.set_width(tool, w);
            }
        });
    }
}

/// The quick bar's fill button: a filled square in the fill colour, or a crossed-out outline
/// for no fill.
fn fill_button(ui: &mut egui::Ui, fill: Option<Rgb>) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    let sq = Rect::from_center_size(r.center(), vec2(16.0, 16.0));
    let edge = Stroke::new(1.0, Color32::from_black_alpha(90));
    match fill {
        Some(c) => {
            ui.painter().rect_filled(sq, 2.0, color32(c));
        }
        None => {
            ui.painter().line_segment([sq.left_bottom(), sq.right_top()], Stroke::new(1.5, Color32::from_rgb(0xD3, 0x2F, 0x2F)));
        }
    }
    ui.painter().rect_stroke(sq, 2.0, edge, egui::StrokeKind::Inside);
    if resp.hovered() {
        ui.painter().rect_stroke(r.shrink(2.0), 6.0, Stroke::new(1.0, Color32::from_black_alpha(40)), egui::StrokeKind::Inside);
    }
    let label = match fill {
        Some(_) => tl!("Fill colour"),
        None => tl!("Fill colour: none"),
    };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    resp.on_hover_text(label)
}

/// Status badge icon and label for a review state name.
pub fn status_badge(state: &str) -> Option<(&'static str, &'static str, Color32)> {
    match state {
        "Accepted" => Some(("thumbs-up", "Accepted", Color32::from_rgb(0x2D, 0x9D, 0x5B))),
        "Rejected" => Some(("thumbs-down", "Rejected", Color32::from_rgb(0xD3, 0x2F, 0x2F))),
        "Cancelled" => Some(("circle-x", "Cancelled", Color32::from_rgb(0x8E, 0x8E, 0x8E))),
        "Completed" => Some(("check", "Completed", Color32::from_rgb(0x2D, 0x9D, 0x5B))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proportional_corners_keep_the_opposite_anchor_and_edges_stretch_one_axis() {
        let r = Rect::from_min_max(pos2(100.0, 100.0), pos2(196.0, 132.0));
        for (hx, hy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
            for (dx, dy) in [(24.0, 0.0), (0.0, 16.0), (-48.0, -20.0), (-200.0, -200.0)] {
                let out = resized(r, (hx, hy), vec2(f32::from(hx) * dx, f32::from(hy) * dy), Some(3.0));
                assert_eq!(handle_pos(out, (-hx, -hy)), handle_pos(r, (-hx, -hy)));
                assert!((out.width() / out.height() - 3.0).abs() < 0.001);
                assert!(out.width() >= 4.0 && out.height() >= 4.0);
            }
        }
        for handle in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
            let out = resized(r, handle, vec2(24.0, 16.0), Some(3.0));
            assert_eq!(out, resized(r, handle, vec2(24.0, 16.0), None));
            assert_eq!(if handle.0 == 0 { out.width() } else { out.height() }, if handle.0 == 0 { r.width() } else { r.height() });
        }
        // Ordinary comment corners keep their existing independent width/height behavior.
        let free = resized(r, (1, 1), vec2(24.0, 0.0), None);
        assert_eq!(free.size(), vec2(120.0, 32.0));
    }

    #[test]
    fn tools_round_trip_through_their_commands() {
        for t in ALL {
            assert_eq!(CommentTool::from_command(t.command()), Some(t));
            assert!(pdfcraft_engine::commands::command(t.command()).is_some(), "{} is registered", t.command());
            assert!(icons::exists(t.icon()), "icon {}", t.icon());
            assert!(GROUPS[t.group()].contains(&t));
        }
    }

    #[test]
    fn drawn_shapes_ignore_slips() {
        assert_eq!(drawn_shape(CommentTool::Rectangle, &[[0.0, 0.0], [1.0, 1.0]]), None);
        assert_eq!(drawn_shape(CommentTool::Rectangle, &[[10.0, 0.0], [0.0, 10.0]]), Some(Shape::Rectangle { rect: [0.0, 0.0, 10.0, 10.0] }));
        assert_eq!(drawn_shape(CommentTool::Ink, &[[0.0, 0.0]]), None);
        assert!(matches!(
            drawn_shape(CommentTool::Arrow, &[[0.0, 0.0], [5.0, 0.0]]),
            Some(Shape::Line { start: LineEnding::None, end: LineEnding::OpenArrow, .. })
        ));
    }

    #[test]
    fn text_boxes_fit_their_text() {
        let r = text_box_rect([100.0, 500.0], "Hi", 12.0);
        assert_eq!((r[0], r[3]), (100.0, 500.0));
        assert!(r[2] - r[0] < 60.0);
        let long = text_box_rect([0.0, 500.0], &"word ".repeat(80), 12.0);
        assert_eq!(long[2], 300.0);
        assert!(long[3] - long[1] > 50.0);
    }

    /// A box fitted to Japanese text uses the advances the text is drawn with (#506); Helvetica's
    /// estimate is about half of a full-width character's advance and would make it too small.
    #[test]
    fn text_boxes_fit_text_outside_winansi_with_its_real_advances() {
        use pdfcraft_engine::annot_text::{text_measure, text_width, text_wrap};
        if pdfcraft_fonts::document_japanese_font().is_none() {
            eprintln!("skipping text_boxes_fit_text_outside_winansi_with_its_real_advances: built without craft-fonts");
            return;
        }
        let long = "これは日本語".repeat(10);
        let r = text_box_rect([0.0, 500.0], &long, 12.0);
        let lines = text_wrap(&long, 12.0, 300.0 - 4.0).len();
        assert!(lines > pdfcraft_engine::annot_text::wrap(&long, 12.0, 296.0).len(), "more lines than Helvetica's estimate gives");
        assert!((r[3] - r[1] - (lines as f64 * 12.0 * 1.2 + 6.0)).abs() < 1e-9, "height {}", r[3] - r[1]);
        let short = "日本語";
        let r = text_box_rect([0.0, 500.0], short, 12.0);
        assert!((r[2] - r[0] - (text_measure(short, 12.0) + 8.0)).abs() < 1e-9, "width {}", r[2] - r[0]);
        assert!(r[2] - r[0] > text_width(short, 12.0) + 8.0 + 1.0 && r[2] - r[0] > 40.0);
    }
}
