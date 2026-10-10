//! pdfcraft-ui-egui — the first PdfCraft shell (L7).
//!
//! Layout grammar follows plan/acrobat/02-ui-ux.md §1: tab strip, mode bar, left tool panel,
//! floating quick-action bar, document area, right panel + right rail with page navigation.
//! Everything here is presentation: documents, rendering and the tool catalogue live in
//! `pdfcraft-engine`.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// Translate an English UI string into the current language (see [`i18n`]).
macro_rules! tl {
    ($s:expr) => {
        $crate::i18n::t($s)
    };
}

/// [`tl!`] for a label whose meaning depends on where it appears ("Type" is a column and a
/// button): a catalog can translate it under `context`, otherwise the plain translation is used.
macro_rules! tl_ctx {
    ($context:expr, $s:expr) => {
        $crate::i18n::tr_ctx($crate::i18n::current(), $context, $s)
    };
}

mod a11y_ui;
mod actions_ui;
pub mod canvas;
mod chrome;
mod combine_grid;
pub use combine_grid::ThumbState as CombineThumb;
mod combine_ui;
pub use combine_ui::{Columns as CombineColumns, CombineView, Lock as CombineLock, SortKey};
mod commands;
mod comment_props;
pub mod comments;
mod comments_panel;
mod compare_ui;
pub mod control;
mod create_multiple_ui;
mod create_ui;
mod credits;
mod crop;
mod drag_pointer;
mod export_ui;
mod js_ui;
mod marks_ui;
mod measure_ui;
mod ocr_ui;
mod optimize_ui;
mod search_ui;
mod sign_ui;
mod stamps_ui;
mod standards_ui;
mod zoom_snap;
/// Header & footer / watermark / background dialog types (tests and automation).
pub mod marks {
    pub use crate::marks_ui::{MarksDraft, PageRange, Subset};
}
mod content_ui;
mod link_ui;
mod object_ui;
pub use create_ui::Clip;
pub use link_ui::LinkDraft;
pub use optimize_ui::{OptimizeDraft, OptimizeTab};
pub use sign_ui::{DigitalIdEntry, SignDraft, SignGraphic, SignStep};
mod autoscroll;
mod bidi;
pub mod bulk_fields;
mod dialogs;
mod edit_text_ui;
mod editing;
mod files;
pub mod fill_sign;
pub mod folders_ui;
pub mod forms_ui;
mod home;
mod icon_data;
pub mod icons;
pub mod last_session;
mod pageboxes;
mod palette;
mod panels;
#[cfg(not(target_arch = "wasm32"))]
mod pickers;
pub mod prepare;
mod print_ui;
mod signature_drag;
pub use print_ui::{Handling as PrintHandling, PrintDraft, Which as PrintWhich};
mod redact_ui;
pub use redact_ui::{HiddenDraft, PagesDraft as RedactPagesDraft, RedactPrefs, SearchDraft as RedactSearchDraft, SearchMode as RedactSearchMode};
pub mod i18n;

/// [`PdfCraftApp::date_text`] for callers that already borrow other parts of the app.
pub(crate) fn date_text(session: &Session, fmt: Option<&str>) -> Result<String, String> {
    let lang = session.date_language().unwrap_or(i18n::current().code());
    session.today_text(fmt, Some(lang))
}

/// Today in Preferences ▸ Date format for stamping into a PDF; refused when it has characters
/// Fill & Sign can't write yet (see [`pdfcraft_engine::dates::unwritable`]).
pub(crate) fn date_text_for_pdf(session: &Session) -> Result<String, String> {
    let lang = session.date_language().unwrap_or(i18n::current().code());
    session.today_text_for_pdf(None, Some(lang))
}

/// The longest author name kept (Preferences ▸ Identity, restored settings).
pub(crate) const MAX_AUTHOR_CHARS: usize = 200;

pub mod portable;
mod protect;
mod recovery;
#[cfg(not(target_arch = "wasm32"))]
mod system_fonts;
pub mod theme;
pub mod ui_scale;
pub mod updates;
mod wheel_pager;
mod widgets;

use pdfcraft_engine::{DocId, Session};

pub use canvas::{DocView, RasterMemory};
pub use editing::{CloseRequest, SaveTarget};
pub use files::{ExtractDraft, FilePurpose, FileRequest, RotateDraft, SplitDraft, SplitMode, SplitPlan};
pub use recovery::{AUTOSAVE_SECS, RecoveryMeta, RecoveryStore};
use theme::{ThemeKind, ThemePreference};

/// Top-level workspace modes (Acrobat's mode bar).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[serde(rename = "all")]
    AllTools,
    Read,
    Edit,
    Convert,
    Sign,
}

impl Mode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "all" => Some(Self::AllTools),
            "read" => Some(Self::Read),
            "edit" => Some(Self::Edit),
            "convert" => Some(Self::Convert),
            "sign" => Some(Self::Sign),
            _ => None,
        }
    }
}

/// What the left panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftPanel {
    AllTools,
    Tool(&'static str),
}

/// Right-hand panels, opened from the rail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RightPanel {
    Comments,
    Bookmarks,
    Pages,
    Fields,
    Layers,
    Attachments,
    Signatures,
    /// Accessibility Checker results.
    Accessibility,
    /// Advanced Search results.
    Search,
    /// Compare files: the differences.
    Compare,
}

/// Quick-action bar tools (the vertical floating strip).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickTool {
    Select,
    Hand,
    /// A commenting tool (Add comments).
    Comment(comments::CommentTool),
    Measure(measure_ui::Tool),
    /// Crop pages by dragging a rectangle.
    Crop,
    /// A Fill & Sign tool.
    Fill(fill_sign::FillTool),
    /// A Prepare a form field tool.
    Field(prepare::FieldTool),
    /// Redact text and images (drag across text or draw a box).
    Redact,
    /// Edit a PDF ▸ Add content ▸ Text.
    AddText,
    /// Edit a PDF ▸ Edit text: click a line of existing text to edit it.
    EditText,
    /// Add a stamp: click to place this stamp.
    Stamp(pdfcraft_engine::StampKind),
    /// A custom stamp from the library (its index).
    CustomStamp(usize),
    /// Edit a PDF ▸ Link: draw link areas, select and edit links.
    Link,
    /// Use a certificate ▸ Digitally sign / Certify (visible): drag the signature's rectangle.
    SignArea {
        certify: bool,
    },
    /// View ▸ Zoom ▸ Marquee Zoom.
    MarqueeZoom,
    /// Edit ▸ Take a Snapshot.
    Snapshot,
    /// Edit ▸ Column Select: drag a rectangle to select only the text inside it (#740). The
    /// Select tool does the same while Alt/Option is held at the start of a drag.
    ColumnSelect,
}

/// Files dropped on a document's page grid.
struct GridDrop {
    doc: pdfcraft_engine::DocId,
    files: Vec<(String, Vec<u8>)>,
    /// When to stop waiting for the pointer (egui time, seconds).
    deadline: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialog {
    CreateImages,
    Properties(PropsTab),
    About,
    Shortcuts,
    Split,
    /// Pages ▸ Number pages… (page labels).
    NumberPages,
    /// Protect Using Password.
    Protect,
    /// Set Page Boxes (crop, trim, bleed, art, media).
    PageBoxes,
    /// Add / Update Header and Footer, Watermark, Background.
    Marks(pdfcraft_engine::MarkKind),
    /// Export a PDF ▸ Image / Text.
    Export(export_ui::ExportKind),
    /// Fill & Sign ▸ Create signature (the drawing pad).
    Signature,
    /// Comment Properties.
    CommentProps,
    /// Replace Pages (after choosing the file).
    ReplacePages,
    /// Prepare a form ▸ Field Properties.
    FieldProps,
    /// Shared properties of several selected form fields.
    BulkFieldProps,
    /// Redact a PDF ▸ Redact pages, Find text and redact, Set properties, apply confirmation.
    RedactPages,
    RedactSearch,
    RedactProps,
    RedactApply,
    /// File ▸ Print.
    Print,
    /// File ▸ Revert confirmation.
    Revert,
    /// Link Properties.
    LinkProps,
    /// Organize ▸ Extract (options), Rotate Pages.
    Extract,
    RotatePages,
    /// Remove Hidden Information and Sanitize Document.
    RemoveHidden,
    Sanitize,
    /// Documents from a session that ended unexpectedly.
    Recovery,
    /// Comments ▸ Summarize comments (options).
    SummarizeComments,
    /// Sign with a Digital ID ▸ Configure ▸ Sign as.
    Sign,
    /// Optimize PDF ▸ Advanced optimization.
    Optimize,
    /// Prepare a form ▸ right-click a field ▸ Duplicate.
    DuplicateField,
    /// Check for accessibility ▸ Accessibility Checker Options.
    AccessibilityOptions,
    /// Scan & OCR ▸ Recognize text.
    RecognizeText,
    /// The JavaScript console (⌘J).
    JsConsole,
    /// Document JavaScripts.
    DocumentJs,
    /// Preferences.
    Preferences,
    /// Compare files: choose the older version.
    CompareFiles,
    /// Action Wizard.
    ActionWizard,
    /// Standards ▸ PDF/A.
    PdfA,
    /// Custom stamps ▸ Create.
    CreateStamp,
    /// Prepare for accessibility ▸ Add alternate text.
    AltText,
    /// PDF Optimizer ▸ Audit space usage (then back to the optimizer).
    AuditSpace,
    /// Signatures ▸ Show certificate.
    CertificateViewer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropsTab {
    Description,
    InitialView,
    Security,
    Fonts,
    Advanced,
}

/// Duplicate Field: the field and the pages (all, or a range, 1-based).
#[derive(Clone, Debug, PartialEq)]
pub struct DuplicateDraft {
    pub name: String,
    pub all: bool,
    pub from: usize,
    pub to: usize,
}

/// Copied pages: their document's name and bytes (as it was when copied) and the pages.
#[derive(Clone, Debug)]
pub struct PageClip {
    pub name: String,
    pub bytes: std::sync::Arc<Vec<u8>>,
    pub pages: Vec<usize>,
}

/// Files delivered asynchronously: (name, bytes).
pub type Inbox = std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<u8>)>>>;

/// Files that failed to arrive asynchronously (a browser `?file=` URL that couldn't be fetched):
/// `(name, error)`, reported to the user on the next frame (#173).
pub type FailedInbox = std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>;

/// A request the operating system sends the running app, outside its window: on macOS, Finder
/// double-clicks, Open With and drops on the Dock icon arrive as Apple events, not arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OsEvent {
    /// Open these files.
    Open(Vec<String>),
    /// Quit (the Dock's Quit, logging out), asking about unsaved changes first.
    Quit,
}

/// Returns the [`OsEvent`]s that arrived since it was last called (set by the desktop app).
pub type OsEventsFn = Box<dyn FnMut() -> Vec<OsEvent>>;

/// Hands a saved PDF to the page embedding PdfCraft (web): `(name, bytes, save_as)`. `save_as`
/// is `true` for Save As. Set by the web app when a host page (such as a Nextcloud app) asked
/// for saves; without it the browser downloads the file.
pub type HostSaveFn = Box<dyn Fn(&str, &[u8], bool) -> Result<(), String>>;

/// Told every frame whether any tab has unsaved work, so a host page can warn before leaving.
pub type HostDirtyFn = Box<dyn FnMut(bool)>;

pub struct PasswordPrompt {
    pub name: String,
    pub path: Option<String>,
    pub bytes: std::sync::Arc<Vec<u8>>,
    pub input: String,
    pub error: Option<String>,
    /// A late startup file must stay in the background even after it is unlocked.
    activate: bool,
    /// The bytes are a recovery snapshot: once the document is open it takes the snapshot's
    /// path and recovery entry. Cancelling the prompt drops the association with it (#813).
    recovered: Option<RecoveryMeta>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct RecentFile {
    pub name: String,
    pub path: String,
    pub pages: usize,
    pub size: usize,
}

pub struct PdfCraftApp {
    pub session: Session,
    pub views: Vec<DocView>,
    /// `None` shows the Home tab.
    pub active: Option<usize>,
    pub mode: Mode,
    /// Workspace used for newly opened PDFs; independent of PDF Initial View metadata.
    pub default_mode: Mode,
    /// Page display and zoom for newly opened PDFs that don't ask for their own (Preferences ▸
    /// Documents and view). Continuous scrolling at fit width by default, which never snaps
    /// between pages.
    pub view_defaults: canvas::ViewDefaults,
    /// The Preferences ▸ Date format box while it's being edited, or while it holds an invalid
    /// pattern (the session keeps the last valid one).
    pub date_format_draft: Option<String>,
    /// Explicit CLI/control mode lasts for this session and is never persisted.
    mode_override: Option<Mode>,
    pub left: LeftPanel,
    pub left_open: bool,
    pub right: Option<RightPanel>,
    /// The user closed the Comments panel, so picking a comment tool leaves it closed until they
    /// open it again (#225). Remembered across restarts.
    pub comments_panel_closed: bool,
    pub quick_tool: QuickTool,
    /// The tool to go back to when Space, held for a temporary Hand, is released.
    space_hand: Option<QuickTool>,
    /// Comment author, per-tool colours and widths, pin.
    pub comment_prefs: comments::CommentPrefs,
    /// Resolved colours, including the current OS theme when following the system.
    pub theme: ThemeKind,
    pub theme_preference: ThemePreference,
    /// The desktop's light/dark choice, watched while the app runs (Linux has no winit answer).
    desktop_theme: pdfcraft_platform::desktop_theme::DesktopTheme,
    /// The last answer [`desktop_theme`] gave, so a frame that asks costs nothing.
    desktop_dark: Option<bool>,
    /// Interface language preference: `auto` (follow the system) or a code from [`i18n::LANGUAGES`].
    pub language: String,
    /// Preferences ▸ Interface size as a factor, or `None` to follow [`Self::system_text_scale`].
    pub ui_scale: Option<f32>,
    /// The desktop's text scaling (GNOME's text scaling factor), which the desktop app reads at
    /// launch. 1.0 elsewhere, so tests and screenshots don't depend on the machine.
    pub system_text_scale: f32,
    /// The interface size last given to egui ([`ui_scale::sync`]).
    ui_scale_applied: Option<f32>,
    /// Preferences: bake Fill & Sign marks into the page when saving. Off, so a normal save stays editable.
    pub flatten_fill_sign_on_save: bool,
    pub dialog: Option<Dialog>,
    /// How to ask for the latest release (the desktop app sets it; see `updates`).
    pub update_source: Option<updates::UpdateSource>,
    pub(crate) updates: updates::Updates,
    pub palette_open: bool,
    pub palette_query: String,
    pub all_tools_expanded: bool,
    pub recent: Vec<RecentFile>,
    /// Preferences: reopen the files that were open when PdfCraft last closed (#442).
    pub reopen_last_session: bool,
    /// The files open when PdfCraft last closed, read from the settings for
    /// [`PdfCraftApp::reopen_last_files`].
    pub last_session: last_session::LastSession,
    /// Quitting closes unsaved tabs one by one: what was open when the quit began.
    quit_session: Option<last_session::LastSession>,
    /// Folders pinned to Home, and what they held when last listed.
    pub pinned: folders_ui::PinnedFolders,
    pub toast: Option<(String, f64)>,
    /// Whether the macOS title bar is drawn by us (traffic lights over our tab strip).
    pub integrated_titlebar: bool,
    pub password_prompt: Option<PasswordPrompt>,
    pub full_screen: bool,
    /// Files delivered asynchronously (web drag-and-drop, web file picker).
    pub inbox: Inbox,
    /// The browser's startup `?file=` download. Unlike a deliberate Open or drop, a late
    /// arrival opens in the background once the user has started working (#171).
    pub startup_inbox: Inbox,
    startup_superseded: bool,
    /// Asynchronous opens that failed (web `?file=` fetches), shown as a notice.
    pub failed_inbox: FailedInbox,
    /// Mirrors "some document has unsaved work" each frame, so the web page's `beforeunload` handler can read it (#812).
    pub unsaved_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Requests from the operating system, polled every frame (macOS Apple events).
    pub os_events: Option<OsEventsFn>,
    /// Save delivers to the embedding page instead of downloading (web, see [`HostSaveFn`]).
    pub host_save: Option<HostSaveFn>,
    /// Reports unsaved work to the embedding page (web, see [`HostDirtyFn`]).
    pub host_dirty: Option<HostDirtyFn>,
    /// A pending "save changes?" question (closing a dirty tab or quitting).
    pub close_request: Option<CloseRequest>,
    /// Save to this path instead of asking (tests and automation).
    pub save_override: Option<String>,
    /// Document Properties ▸ Description fields being edited: (document, Title/Author/Subject/Keywords).
    pub props_draft: Option<(DocId, [String; 4])>,
    /// Document Properties ▸ Initial View (and reading options) being edited.
    pub view_draft: Option<(DocId, pdfcraft_engine::InitialView)>,
    /// Files picked asynchronously for combine / insert (web).
    pub requests: files::Requests,
    /// Native file pickers in flight (they never block the frame; see `pickers`).
    #[cfg(not(target_arch = "wasm32"))]
    pickers: pickers::Pickers,
    /// Pick these files instead of showing a picker (tests and automation).
    #[cfg(not(target_arch = "wasm32"))]
    pub pick_override: Option<Vec<String>>,
    /// Write exported files (split) here instead of asking (tests and automation).
    pub export_dir_override: Option<String>,
    /// Split dialog settings.
    pub split_draft: SplitDraft,
    pub extract_draft: ExtractDraft,
    pub rotate_draft: RotateDraft,
    /// Summarize Comments: sort order.
    pub summary_sort: pdfcraft_engine::SummarySort,
    /// The signing dialogs' state.
    pub sign_draft: Option<SignDraft>,
    /// Digital ID files the user has created or added.
    pub digital_ids: Vec<DigitalIdEntry>,
    /// Signatures panel: expanded entries (field names).
    pub sig_expanded: Vec<String>,
    /// Accessibility Checker: options, the last check, and rules skipped by hand.
    pub a11y_options: a11y_ui::A11yOptions,
    /// Scan & OCR: the Recognize Text choices, the running job, and (tests) run it inline.
    pub ocr_draft: ocr_ui::OcrDraft,
    pub ocr_run: Option<ocr_ui::OcrRun>,
    pub ocr_batch: Option<std::sync::Arc<std::sync::Mutex<ocr_ui::BatchProgress>>>,
    /// Background jobs (OCR, actions, listing pinned folders) run inline instead (tests).
    pub run_inline: bool,
    /// Action Wizard: the user's actions, the dialog state, the running action and (tests) the
    /// files to use instead of a picker.
    pub custom_actions: Vec<pdfcraft_engine::actions::Action>,
    pub wizard: actions_ui::Wizard,
    pub action_run: Option<std::sync::Arc<std::sync::Mutex<actions_ui::RunProgress>>>,
    pub action_files_override: Option<Vec<String>>,
    /// Standards ▸ PDF/A: level and last result.
    pub pdfa: standards_ui::PdfaState,
    /// Compare files: the chosen older document and the last result.
    pub compare_old: Option<DocId>,
    pub compare: Option<compare_ui::CompareState>,
    /// The JavaScript console and the Document JavaScripts draft.
    pub js_console: js_ui::JsConsole,
    pub doc_js: js_ui::DocJsDraft,
    pub a11y: a11y_ui::A11yState,
    pub a11y_skipped: std::collections::BTreeSet<pdfcraft_engine::a11y::Rule>,
    pub alt_draft: a11y_ui::AltDraft,
    /// List the OS key store's signing identities among the digital IDs (the desktop app).
    pub os_key_store_ids: bool,
    pub cert_viewer: Option<sign_ui::CertViewer>,
    /// The last space audit.
    pub space_audit: Vec<pdfcraft_engine::optimize::SpaceUse>,
    /// Combine files: the files staged so far.
    pub combine_draft: Vec<combine_ui::CombineFile>,
    /// The Combine files tab: whether it is open, shown, its selection and undo history.
    pub combine_tab: combine_ui::CombineTab,
    /// The Combine files table's column order and widths (kept in the settings).
    pub combine_columns: combine_ui::Columns,
    /// Combine files as thumbnails or as the table (kept in the settings).
    pub combine_view: combine_ui::CombineView,
    /// How large the Combine grid draws its cards (1.0 = the usual; kept in the settings). Set
    /// with [`PdfCraftApp::set_combine_zoom`].
    pub combine_zoom: f32,
    /// The Combine grid's thumbnails: rendered off the UI thread, a few at a time.
    pub(crate) combine_thumbs: combine_grid::Thumbs,
    /// Images waiting for the resolution choice (released on cancel).
    pub image_import: Option<create_ui::ImageImport>,
    /// The custom stamp library, and the stamp being created.
    pub custom_stamps: Vec<stamps_ui::CustomStamp>,
    pub stamp_draft: stamps_ui::StampDraft,
    /// PDF Optimizer choices.
    pub optimize_draft: OptimizeDraft,
    /// The running optimization (Optimize PDF ▸ Advanced optimization).
    pub optimize_run: Option<optimize_ui::OptimizeRun>,
    /// A background job's progress card (see [`widgets::progress_notice`]).
    pub progress_notice: Option<widgets::ProgressNotice>,
    /// Pages copied or cut in Organize Pages, ready to paste (into any document).
    pub page_clipboard: Option<PageClip>,
    /// Files dropped on the page grid, waiting for the pointer to say which gap they go to.
    grid_drop: Option<GridDrop>,
    /// The last snapshot (width, height, RGBA); `system_clipboard` also puts it on the
    /// system clipboard (tests turn that off).
    pub last_snapshot: Option<(u32, u32, pdfcraft_render::Pixels)>,
    pub system_clipboard: bool,
    /// Attach file: the file to attach instead of asking (tests, automation).
    pub attach_override: Option<(String, Vec<u8>)>,
    /// Duplicate Field: which field and onto which pages.
    pub duplicate_draft: Option<DuplicateDraft>,
    /// Prepare a form ▸ Preview: fill the form instead of editing its fields.
    pub form_preview: bool,
    /// Where autosaves go (`None`: autosave off, e.g. on the web and in tests).
    pub recovery: Option<RecoveryStore>,
    /// Entries left by a previous session, offered in the Recovery dialog.
    pub recoverable: Vec<RecoveryMeta>,
    recovery_keys: std::collections::HashMap<DocId, String>,
    last_autosave: f64,
    allow_quit: bool,
    /// Shortcuts pressed while a text field had the keyboard, run on the next frame (see
    /// `registry_shortcuts`).
    deferred_commands: Vec<(&'static str, Option<DocId>)>,
    /// The dialog seen at the last check, and a counter bumped whenever it changes (see
    /// [`Self::dialog_epoch`]).
    dialog_seen: Option<Dialog>,
    dialog_epoch: u64,
    /// The egui context, for commands that change window or theme state.
    ctx: Option<egui::Context>,
    styled: bool,
    fonts_ready: bool,
    /// The installed fonts put the Simplified Chinese faces first (see `theme::font_definitions_for`).
    fonts_hans: bool,
    /// The window title last sent to the platform.
    pub window_title: String,
    /// The UI control channel, when enabled (`--control`; off by default).
    control: Option<control::Control>,
    /// A bookmark being renamed in the Bookmarks panel: (path, text so far).
    pub bookmark_rename: Option<(Vec<usize>, String)>,
    /// Number pages dialog settings (1-based pages).
    pub number_draft: NumberDraft,
    /// Protect Using Password dialog state.
    pub protect_draft: protect::ProtectDraft,
    /// Set Page Boxes dialog state.
    pub boxes_draft: pageboxes::BoxesDraft,
    /// Header & footer / watermark / background dialog state.
    pub marks_draft: marks_ui::MarksDraft,
    /// Export dialog settings.
    pub export_draft: export_ui::ExportDraft,
    /// A running export's progress.
    export_status: Option<export_ui::ExportStatus>,
    /// The saved Fill & Sign signature and initials (drawn or typed).
    pub signature: Option<fill_sign::SavedSig>,
    pub initials: Option<fill_sign::SavedSig>,
    /// The Create signature / initials dialog, and its typed preview.
    pub signature_draft: fill_sign::SigDraft,
    pub(crate) signature_preview: Option<(fill_sign::SavedSig, egui::TextureHandle)>,
    pub(crate) saved_signature_previews: [Option<(fill_sign::SavedSig, egui::TextureHandle)>; 2],
    /// What visible digital signatures show on their left, and its preview in Sign as.
    pub sign_graphic: sign_ui::SignGraphic,
    pub(crate) sign_graphic_preview: Option<(fill_sign::SavedSig, egui::TextureHandle)>,
    #[cfg(target_arch = "wasm32")]
    pub(crate) signature_images: fill_sign::ImageInbox,
    /// The Comment Properties dialog's state.
    pub comment_props: Option<comment_props::PropsDraft>,
    pub field_props: Option<prepare::FieldDraft>,
    pub bulk_field_props: Option<bulk_fields::Draft>,
    pub redact_prefs: RedactPrefs,
    pub redact_pages_draft: RedactPagesDraft,
    pub redact_search: RedactSearchDraft,
    pub hidden_draft: HiddenDraft,
    pub print_draft: PrintDraft,
    pub link_draft: Option<LinkDraft>,
    /// The style new text gets (Edit a PDF ▸ Format text).
    pub text_style: pdfcraft_engine::AddedText,
    /// The Replace Pages dialog's state.
    pub replace_draft: Option<files::ReplaceDraft>,
    /// The last web link the app asked the system to open (tests and automation).
    pub last_opened_url: Option<String>,
    /// A document asked to open this address; the user hasn't answered yet (#90, #91).
    pub pending_link: Option<PendingLink>,
}

/// Where in a document a request to open an address came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkOrigin {
    /// A link on the page.
    Link,
    /// A push button's URI action.
    Button,
    /// A script (`app.launchURL`).
    Script,
}

impl LinkOrigin {
    fn noun(self) -> &'static str {
        match self {
            Self::Link => "A link",
            Self::Button => "A button",
            Self::Script => "A script",
        }
    }
}

/// An address a document asked to open, waiting for the user to allow or cancel it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingLink {
    pub url: String,
    pub origin: LinkOrigin,
}

/// Settings for the Number pages dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct NumberDraft {
    pub from: usize,
    pub to: usize,
    pub style: pdfcraft_engine::LabelStyle,
    pub prefix: String,
    pub start: u32,
}

impl Default for PdfCraftApp {
    fn default() -> Self {
        Self::new()
    }
}

/// What the operating system says about light and dark, as "draw dark". `None` means nobody
/// answered, not a preference, so the caller keeps what it chose.
fn system_theme(ctx: &egui::Context, desktop_dark: Option<bool>) -> Option<bool> {
    ctx.system_theme().map(|theme| theme == egui::Theme::Dark).or(desktop_dark)
}

/// A restored colour: an `[r, g, b]` array of finite numbers, each clamped to 0–1 (restored
/// settings are untrusted). Anything else (a string, a wrong length, a null or a non-finite
/// number) is refused whole, so the caller keeps its default.
fn rgb_triple(v: &serde_json::Value) -> Option<[f64; 3]> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    let mut out = [0.0f64; 3];
    for (slot, x) in out.iter_mut().zip(a.iter()) {
        let n = x.as_f64()?;
        if !n.is_finite() {
            return None;
        }
        *slot = n.clamp(0.0, 1.0);
    }
    Some(out)
}

impl PdfCraftApp {
    pub fn new() -> Self {
        Self {
            session: Session::new(),
            views: Vec::new(),
            active: None,
            mode: Mode::AllTools,
            default_mode: Mode::AllTools,
            view_defaults: Default::default(),
            date_format_draft: None,
            mode_override: None,
            left: LeftPanel::AllTools,
            left_open: true,
            right: None,
            comments_panel_closed: false,
            quick_tool: QuickTool::Select,
            space_hand: None,
            comment_prefs: Default::default(),
            theme: ThemeKind::Light,
            theme_preference: ThemePreference::Light,
            desktop_theme: pdfcraft_platform::desktop_theme::DesktopTheme::start(),
            desktop_dark: None,
            language: i18n::AUTO.to_string(),
            ui_scale: None,
            system_text_scale: 1.0,
            ui_scale_applied: None,
            flatten_fill_sign_on_save: false,
            dialog: None,
            update_source: None,
            updates: updates::Updates::default(),
            palette_open: false,
            palette_query: String::new(),
            all_tools_expanded: false,
            recent: Vec::new(),
            reopen_last_session: false,
            last_session: Default::default(),
            quit_session: None,
            pinned: Default::default(),
            toast: None,
            integrated_titlebar: false,
            password_prompt: None,
            full_screen: false,
            inbox: Default::default(),
            startup_inbox: Default::default(),
            startup_superseded: false,
            failed_inbox: Default::default(),
            unsaved_flag: Default::default(),
            os_events: None,
            host_save: None,
            host_dirty: None,
            close_request: None,
            save_override: None,
            props_draft: None,
            view_draft: None,
            requests: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            pickers: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            pick_override: None,
            export_dir_override: None,
            split_draft: SplitDraft::default(),
            extract_draft: ExtractDraft::default(),
            rotate_draft: RotateDraft::default(),
            summary_sort: Default::default(),
            sign_draft: None,
            digital_ids: Vec::new(),
            sig_expanded: Vec::new(),
            a11y_options: a11y_ui::A11yOptions::default(),
            ocr_draft: ocr_ui::OcrDraft::default(),
            ocr_run: None,
            ocr_batch: None,
            run_inline: false,
            custom_actions: Vec::new(),
            wizard: Default::default(),
            action_run: None,
            action_files_override: None,
            pdfa: Default::default(),
            compare_old: None,
            compare: None,
            js_console: Default::default(),
            doc_js: Default::default(),
            a11y: a11y_ui::A11yState::default(),
            a11y_skipped: Default::default(),
            alt_draft: Default::default(),
            os_key_store_ids: false,
            cert_viewer: None,
            space_audit: Vec::new(),
            combine_draft: Vec::new(),
            combine_tab: Default::default(),
            combine_columns: Default::default(),
            combine_view: Default::default(),
            combine_zoom: 1.0,
            combine_thumbs: Default::default(),
            image_import: None,
            custom_stamps: Vec::new(),
            stamp_draft: Default::default(),
            optimize_draft: OptimizeDraft::default(),
            optimize_run: None,
            progress_notice: None,
            page_clipboard: None,
            grid_drop: None,
            last_snapshot: None,
            system_clipboard: true,
            attach_override: None,
            duplicate_draft: None,
            form_preview: false,
            window_title: String::new(),
            recovery: None,
            recoverable: Vec::new(),
            recovery_keys: Default::default(),
            last_autosave: 0.0,
            allow_quit: false,
            deferred_commands: Vec::new(),
            dialog_seen: None,
            dialog_epoch: 0,
            ctx: None,
            styled: false,
            fonts_ready: false,
            fonts_hans: false,
            control: None,
            bookmark_rename: None,
            last_opened_url: None,
            pending_link: None,
            protect_draft: Default::default(),
            boxes_draft: Default::default(),
            marks_draft: Default::default(),
            export_draft: Default::default(),
            export_status: None,
            signature: None,
            initials: None,
            signature_draft: Default::default(),
            signature_preview: None,
            saved_signature_previews: [None, None],
            sign_graphic: sign_ui::SignGraphic::Name,
            sign_graphic_preview: None,
            #[cfg(target_arch = "wasm32")]
            signature_images: Default::default(),
            comment_props: None,
            field_props: None,
            bulk_field_props: None,
            redact_prefs: RedactPrefs::default(),
            redact_pages_draft: RedactPagesDraft::default(),
            redact_search: RedactSearchDraft::default(),
            hidden_draft: HiddenDraft::default(),
            print_draft: PrintDraft::default(),
            link_draft: None,
            text_style: content_ui::default_style(),
            replace_draft: None,
            number_draft: NumberDraft { from: 1, to: 1, style: pdfcraft_engine::LabelStyle::Decimal, prefix: String::new(), start: 1 },
        }
    }

    /// Fields are being edited (Prepare a form is open or a field tool is picked), unless the
    /// form is being previewed.
    pub fn is_preparing(&self) -> bool {
        ((self.left_open && self.left == LeftPanel::Tool("form")) || matches!(self.quick_tool, QuickTool::Field(_))) && !self.form_preview
    }

    /// Open a document the way it asks to be opened: navigation panel, layout, magnification,
    /// page (Document Properties ▸ Initial View).
    fn apply_initial_view(&mut self, index: usize, v: &pdfcraft_engine::InitialView) {
        use pdfcraft_engine::{InitialLayout as L, Magnification as M, Navigation as N};
        let pages = self.session.get(self.views[index].id).map_or(0, |d| d.info.pages.len());
        if self.active == Some(index) {
            match v.navigation {
                N::PageOnly => {}
                N::Bookmarks => self.right = Some(RightPanel::Bookmarks),
                N::Pages => self.right = Some(RightPanel::Pages),
                N::Attachments => self.right = Some(RightPanel::Attachments),
                N::Layers => self.right = Some(RightPanel::Layers),
            }
        }
        let view = &mut self.views[index];
        match v.layout {
            // The view opened in Default page display already.
            L::Default => {}
            L::SinglePage => view.layout = canvas::PageLayout::Single,
            L::SinglePageContinuous => view.layout = canvas::PageLayout::Continuous,
            L::TwoUp | L::TwoUpContinuous => view.layout = canvas::PageLayout::TwoUp,
            L::TwoUpCoverPage | L::TwoUpContinuousCoverPage => {
                view.layout = canvas::PageLayout::TwoUp;
                view.cover = true;
            }
        }
        match v.magnification {
            // The view opened at the default zoom already.
            M::Default => {}
            M::ActualSize => view.set_zoom(1.0),
            M::Percent(p) => view.set_zoom((p / 100.0) as f32),
            M::FitPage | M::FitVisible => view.fit = canvas::Fit::Page,
            M::FitWidth => view.fit = canvas::Fit::Width,
            M::FitHeight => view.fit = canvas::Fit::Height,
        }
        if v.page > 0 && v.page < pages {
            view.go_to_page(v.page);
        }
    }

    /// Open a document and make it the active tab. Encrypted files raise the password prompt.
    pub fn open_bytes(&mut self, name: &str, path: Option<String>, bytes: Vec<u8>) -> Result<(), String> {
        self.startup_superseded = true;
        self.open_bytes_with_focus(name, path, bytes, true)
    }

    fn open_bytes_with_focus(&mut self, name: &str, path: Option<String>, bytes: Vec<u8>, activate: bool) -> Result<(), String> {
        // Images and text files become new, unsaved PDFs (Create a PDF).
        let active = self.active;
        if let Some(r) = self.open_converted(name, &bytes) {
            if !activate {
                self.active = active;
            }
            return r;
        }
        self.try_open(name, path, std::sync::Arc::new(bytes), None, activate)
    }

    fn try_open(
        &mut self,
        name: &str,
        path: Option<String>,
        bytes: std::sync::Arc<Vec<u8>>,
        password: Option<&str>,
        activate: bool,
    ) -> Result<(), String> {
        use pdfcraft_render::OpenError;
        let size = bytes.len();
        let id = match self.session.open(name, path.clone(), bytes.clone(), password) {
            Ok(id) => id,
            Err(e @ (OpenError::NeedsPassword | OpenError::WrongPassword)) => {
                let error = matches!(e, OpenError::WrongPassword).then(|| "Incorrect password. Try again.".to_string());
                self.password_prompt =
                    Some(PasswordPrompt { name: name.to_string(), path, bytes, input: String::new(), error, activate, recovered: None });
                return Ok(());
            }
            Err(e) => return Err(e.to_string()),
        };
        self.password_prompt = None;
        let doc = self.session.get(id).ok_or("the document could not be opened")?;
        let pages = doc.info.pages.len();
        // Acrobat opens straight to the Comments panel when a document has comments.
        if activate && self.right.is_none() {
            self.right = if !doc.info.annotations.is_empty() {
                Some(RightPanel::Comments)
            } else if !doc.info.outline.is_empty() {
                Some(RightPanel::Bookmarks)
            } else {
                None
            };
        }
        let initial = doc.initial_view();
        self.views.push(DocView::new(id, &doc.info, self.view_defaults));
        if activate {
            self.active = Some(self.views.len() - 1);
        }
        self.apply_initial_view(self.views.len() - 1, &initial);
        if activate && let Some(mode) = self.mode_override {
            // Explicit mode options do not reset independent --tool / --left choices.
            self.mode = mode;
        } else if activate && self.mode != self.default_mode {
            // Switch workspace like the mode bar, but a left panel the user (or `--left closed`)
            // closed stays closed, and an unchanged mode keeps the tool panel the user chose.
            let left_open = self.left_open;
            self.select_mode(self.default_mode);
            self.left_open = left_open;
        }
        if let Some(p) = path {
            self.recent.retain(|r| r.path != p);
            self.recent.insert(0, RecentFile { name: name.to_string(), path: p, pages, size });
            self.recent.truncate(12);
        }
        // What the form's scripts said while it opened (messages, errors) shows now, not with
        // the next edit.
        // Background documents keep their script output until their tab is selected: a print
        // or save request must never act on the current document instead.
        if activate {
            let out = self.session.take_js_output(id);
            self.handle_js(id, out);
        }
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn open_dropped(&mut self, f: egui::DroppedFileHandle, _ctx: &egui::Context) {
        let p = f.path().to_string_lossy().into_owned();
        if !p.is_empty() && f.path().is_absolute() {
            self.open_path(&p);
            return;
        }
        let name = f.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "dropped.pdf".into());
        match f.bytes() {
            Ok(bytes) => {
                if let Err(e) = self.open_bytes(&name, None, bytes) {
                    self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                }
            }
            Err(e) => self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e.to_string())]),
        }
    }

    /// The document whose page grid is showing and may take pages, if any.
    fn grid_target(&self) -> Option<pdfcraft_engine::DocId> {
        let view = self.active.and_then(|i| self.views.get(i)).filter(|v| v.organize)?;
        (self.dialog.is_none() && self.session.get(view.id)?.allows_assembly()).then_some(view.id)
    }

    /// Files dropped while the page grid shows are inserted into it rather than opened: they
    /// are kept until [`Self::finish_grid_drop`] knows the gap. Returns the files not taken.
    #[cfg(not(target_arch = "wasm32"))]
    fn drop_on_grid(&mut self, dropped: Vec<egui::DroppedFileHandle>, ctx: &egui::Context) -> Vec<egui::DroppedFileHandle> {
        let Some(id) = self.grid_target().filter(|_| !dropped.is_empty()) else { return dropped };
        let mut files = Vec::new();
        for f in dropped.iter().take(pdfcraft_engine::MAX_CREATE_FILES) {
            let name = f.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "dropped.pdf".into());
            let bytes = if f.path().is_absolute() { std::fs::read(f.path()).map_err(|e| e.to_string()) } else { f.bytes() };
            match bytes {
                Ok(b) => files.push((name, b)),
                Err(e) => self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e)]),
            }
        }
        if !files.is_empty() {
            // Where there is no telling where the pointer is during the drag, its place arrives
            // with the first move after the drop.
            ctx.request_repaint();
            self.grid_drop = Some(GridDrop { doc: id, files, deadline: ctx.input(|i| i.time) + 1.0 });
        }
        Vec::new()
    }

    /// Insert files dropped on the page grid at the gap under the pointer, or at the end when
    /// the pointer hasn't shown up in time.
    fn finish_grid_drop(&mut self, ctx: &egui::Context) {
        let Some(GridDrop { doc: id, deadline, .. }) = &self.grid_drop else { return };
        let (id, deadline) = (*id, *deadline);
        if self.grid_target() != Some(id) {
            self.grid_drop = None;
            return self.notify_tr("The document changed while you were choosing a file, so nothing was changed.");
        }
        let Some(view) = self.active.and_then(|i| self.views.get_mut(i)) else { return };
        let gap = match view.grid_gap {
            Some(gap) => gap,
            None if ctx.input(|i| i.time) >= deadline => usize::MAX,
            None => return ctx.request_repaint(),
        };
        view.insert_at = Some(gap);
        if let Some(drop) = self.grid_drop.take() {
            self.insert_files(drop.files);
        }
    }

    /// Browsers read dropped files asynchronously; the bytes land in `inbox` and open next frame.
    #[cfg(target_arch = "wasm32")]
    fn open_dropped(&mut self, f: egui::DroppedFileHandle, ctx: &egui::Context) {
        self.startup_superseded = true;
        let inbox = self.inbox.clone();
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let name = f.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "dropped.pdf".into());
            if let Ok(bytes) = f.bytes_async().await
                && let Ok(mut q) = inbox.lock()
            {
                q.push((name, bytes));
                ctx.request_repaint();
            }
        });
    }

    /// A number that changes whenever the open dialog changes (opened, closed or replaced by
    /// another), checked every frame. A picker started from a dialog's Browse… button answers
    /// only into the same showing of that dialog.
    pub(crate) fn dialog_epoch(&mut self) -> u64 {
        if self.dialog != self.dialog_seen {
            self.dialog_seen = self.dialog;
            self.dialog_epoch = self.dialog_epoch.wrapping_add(1);
        }
        self.dialog_epoch
    }

    /// Save an attachment to disk, or open a PDF attachment in a new tab.
    pub fn attachment_action(&mut self, doc: DocId, index: usize, open: bool) {
        let Some(d) = self.session.get(doc) else { return };
        let Some(att) = d.info.attachments.get(index).cloned() else { return };
        let data = pdfcraft_render::attachment_data(&d.bytes, d.password.as_deref(), &att);
        match (data, open) {
            (Err(e), _) => self.notify_fmt("Couldn't read {name}: {e}", &[("name", &att.name), ("e", &e.to_string())]),
            (Ok(bytes), true) => {
                if let Err(e) = self.open_bytes(&att.name, None, bytes) {
                    self.notify_fmt("Couldn't open {name}: {e}", &[("name", &att.name), ("e", &e.to_string())]);
                }
            }
            (Ok(bytes), false) => {
                #[cfg(not(target_arch = "wasm32"))]
                self.ask_one(pickers::Ask::Save(rfd::AsyncFileDialog::new().set_file_name(&att.name)), None, move |app, path| {
                    match crate::editing::write_atomically(&path.to_string_lossy(), &bytes) {
                        Ok(()) => app.notify_fmt("Saved {name}", &[("name", &path.display().to_string())]),
                        Err(e) => app.notify_fmt("Couldn't save: {e}", &[("e", &e.to_string())]),
                    }
                });
                #[cfg(target_arch = "wasm32")]
                self.notify_fmt("Downloading attachments on the web arrives with M3.10 ({n} bytes ready)", &[("n", &bytes.len().to_string())]);
            }
        }
    }

    /// Enter or leave full-screen reading (Acrobat: View ▸ Full Screen Mode, ⌘L).
    pub fn set_full_screen(&mut self, ctx: &egui::Context, on: bool) {
        self.full_screen = on;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
    }

    /// Answer the password prompt (`None` cancels).
    pub fn submit_password(&mut self, password: Option<String>) {
        let Some(p) = self.password_prompt.take() else { return };
        let Some(pw) = password else {
            // A cancelled recovery goes back on offer; its snapshot stays in the store and must
            // not attach itself to whatever document is unlocked next (#813).
            if let Some(meta) = p.recovered {
                self.recoverable.push(meta);
            }
            return;
        };
        match self.try_open(&p.name, p.path, p.bytes, Some(&pw), p.activate) {
            Err(e) => self.notify_fmt("Couldn't open {name}: {e}", &[("name", &p.name), ("e", &e.to_string())]),
            // A recovered encrypted document is open once its prompt is gone.
            Ok(()) if self.password_prompt.is_none() => {
                if let Some(meta) = &p.recovered {
                    self.finish_recovery(meta);
                }
            }
            // Wrong password: the new prompt is for the same bytes, so it keeps the snapshot.
            Ok(()) => {
                if let Some(prompt) = self.password_prompt.as_mut() {
                    prompt.recovered = p.recovered;
                }
            }
        }
    }

    /// Open a file from a recent list: focus the tab already showing it, else open it (File ▸
    /// Open Recent and the Home view's list share this).
    pub fn open_recent(&mut self, path: &str) {
        if let Some(i) = self.views.iter().position(|v| self.session.get(v.id).and_then(|d| d.path.as_deref()) == Some(path)) {
            self.active = Some(i);
        } else {
            #[cfg(not(target_arch = "wasm32"))]
            self.open_path(path);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_path(&mut self, path: &str) {
        let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string());
        match std::fs::read(path) {
            Ok(bytes) => {
                if let Err(e) = self.open_bytes(&name, Some(path.to_string()), bytes) {
                    self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
                }
            }
            Err(e) => self.notify_fmt("Couldn't read {name}: {e}", &[("name", &name), ("e", &e.to_string())]),
        }
    }

    pub fn open_dialog(&mut self) {
        self.startup_superseded = true;
        #[cfg(not(target_arch = "wasm32"))]
        self.pick(
            pickers::PickFor::Open,
            rfd::AsyncFileDialog::new().add_filter("PDF", &["pdf"]).add_filter(tl!("Images and text (converted to PDF)"), &create_ui::CONVERTIBLE),
            false,
        );
        // Browsers pick files asynchronously; the bytes arrive through `inbox`.
        #[cfg(target_arch = "wasm32")]
        {
            let inbox = self.inbox.clone();
            let ctx = self.ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Some(h) = rfd::AsyncFileDialog::new()
                    .add_filter("PDF", &["pdf"])
                    .add_filter(tl!("Images and text"), &create_ui::CONVERTIBLE)
                    .pick_file()
                    .await
                {
                    let bytes = h.read().await;
                    if let Ok(mut q) = inbox.lock() {
                        q.push((h.file_name(), bytes));
                    }
                    // The read may finish while the app is idle: wake it to open the file.
                    if let Some(ctx) = ctx {
                        ctx.request_repaint();
                    }
                }
            });
        }
    }

    pub fn close_tab(&mut self, index: usize) {
        if index >= self.views.len() {
            return;
        }
        let id = self.views.remove(index).id;
        self.forget_recovery(id);
        self.session.close(id);
        self.active = match self.active {
            _ if self.views.is_empty() => None,
            Some(a) if a > index => Some(a - 1),
            Some(a) if a >= self.views.len() => Some(self.views.len() - 1),
            other => other,
        };
    }

    pub fn active_ids(&self) -> Option<(usize, DocId)> {
        self.active.and_then(|i| self.views.get(i).map(|v| (i, v.id)))
    }

    /// A dialog or prompt is open over the window: the keyboard is its own, and nothing may act
    /// on the document underneath it.
    pub(crate) fn modal_open(&self) -> bool {
        self.dialog.is_some() || self.close_request.is_some() || self.password_prompt.is_some() || self.pending_link.is_some() || self.updates.open
    }

    /// Enable the UI control channel on `ctx` (opt-in; see [`control`]). Returns a client that
    /// sends requests to this app; [`control::serve`] exposes it on loopback.
    pub fn attach_control(&mut self, ctx: &egui::Context) -> control::ControlClient {
        let (control, client) = control::attach(ctx);
        self.control = Some(control);
        client
    }

    /// Open a web link in the system browser (a new tab on the web). Only for PdfCraft's own
    /// links; an address that came from a document goes through [`Self::request_document_url`].
    pub fn open_url(&mut self, url: &str) {
        if let Some(ctx) = &self.ctx {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        }
        self.last_opened_url = Some(url.to_string());
    }

    /// A document asks to open `url` (a link, a button's URI action or `app.launchURL`). Web and
    /// email addresses wait for the user to allow them; anything else is refused with a notice
    /// (#90, #91). While one request is waiting, further ones are dropped, so a script can't
    /// queue up a stream of dialogs.
    pub fn request_document_url(&mut self, url: &str, origin: LinkOrigin) {
        match pdfcraft_engine::links::document_url(url) {
            Ok(url) => {
                if self.pending_link.is_none() {
                    self.pending_link = Some(PendingLink { url, origin });
                }
            }
            Err(e) => {
                use pdfcraft_engine::links::BlockedLink;
                let template = if matches!(e, BlockedLink::MailFile | BlockedLink::MailEncodedWord) {
                    "{who} in this document tried to open an address PdfCraft won't open: {e}."
                } else {
                    "{who} in this document tried to open an address PdfCraft won't open: {e}. Only web (http, https) and email (mailto) links open from documents."
                };
                self.notify_fmt(template, &[("who", tl!(origin.noun())), ("e", &e.to_string())]);
            }
        }
    }

    /// The user's answer to [`Self::pending_link`]: open it, or drop it.
    pub fn resolve_pending_link(&mut self, open: bool) {
        if let Some(p) = self.pending_link.take()
            && open
        {
            self.open_url(&p.url);
        }
    }

    pub fn notify(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), 0.0));
    }

    /// Notify with a static message in the UI language.
    pub fn notify_tr(&mut self, text: &str) {
        self.notify(tl!(text).to_string());
    }

    /// Notify with an error's own text. Engine and OS messages are not translated: matching their
    /// English wording would break silently whenever it changes.
    pub fn notify_error(&mut self, error: impl std::fmt::Display) {
        self.notify(error.to_string());
    }

    /// Notify with a `{name}`-style template in the UI language (placeholders filled once).
    pub fn notify_fmt(&mut self, template: &str, args: &[(&str, &str)]) {
        self.notify(i18n::fmt(tl!(template), args));
    }

    pub fn set_theme_preference(&mut self, preference: ThemePreference) {
        self.theme_preference = preference;
        // Before the first frame there is no context to ask, so the last choice stands.
        let system = self.ctx.as_ref().and_then(|ctx| system_theme(ctx, self.desktop_dark));
        self.theme = preference.resolve(system, self.theme);
        if let Some(ctx) = &self.ctx {
            theme::apply(ctx, self.theme);
        }
    }

    /// Draw the running job's progress card and pass a Cancel click on to the job.
    fn show_progress(&mut self, ctx: &egui::Context) {
        if widgets::progress_notice(self, ctx) {
            self.cancel_optimize();
        }
    }

    fn sync_theme(&mut self, ctx: &egui::Context) {
        if let Some(dark) = self.desktop_theme.take_change() {
            self.desktop_dark = Some(dark);
        }
        let kind = self.theme_preference.resolve(system_theme(ctx, self.desktop_dark), self.theme);
        if kind != self.theme {
            self.theme = kind;
            theme::apply(ctx, kind);
        }
    }

    /// Run a catalogue command. Commands that aren't implemented yet say which milestone ships them.
    pub fn run_command(&mut self, command: &str) {
        if pdfcraft_engine::commands::command(command).is_some() {
            self.execute(command);
            return;
        }
        // Not implemented yet: say which milestone ships it.
        let when = pdfcraft_engine::catalog::TOOL_GROUPS
            .iter()
            .flat_map(|g| g.sections.iter().flat_map(|s| s.items.iter()))
            .find(|i| i.command == command)
            .map(|i| match i.availability {
                pdfcraft_engine::catalog::Availability::Planned(m) => crate::i18n::fmt(tl!("ships in milestone {m}"), &[("m", m)]),
                pdfcraft_engine::catalog::Availability::Provider => tl!("needs an AI provider (off by default)").to_string(),
                pdfcraft_engine::catalog::Availability::Ready => tl!("is available").to_string(),
            })
            .unwrap_or_else(|| tl!("is not available yet").to_string());
        self.notify_fmt("`{command}` {when}", &[("command", command), ("when", &when)]);
    }

    /// Today in `fmt` (or Preferences ▸ Date format), with month and weekday names in the date
    /// language, or the interface language when it follows that.
    pub fn date_text(&self, fmt: Option<&str>) -> Result<String, String> {
        date_text(&self.session, fmt)
    }

    /// Select the workspace and its matching tool panel, just like the mode bar.
    pub(crate) fn select_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.left_open = true;
        self.left = match mode {
            Mode::Edit => LeftPanel::Tool("edit"),
            Mode::Convert => LeftPanel::Tool("export"),
            Mode::Sign => LeftPanel::Tool("fill_sign"),
            _ => LeftPanel::AllTools,
        };
    }

    /// Opens a right panel, or closes it with `None`, because the user chose to. Closing Comments
    /// keeps comment tools from reopening it; opening it again lets them (#225).
    pub fn choose_right_panel(&mut self, panel: Option<RightPanel>) {
        if panel == Some(RightPanel::Comments) {
            self.comments_panel_closed = false;
        } else if panel.is_none() && self.right == Some(RightPanel::Comments) {
            self.comments_panel_closed = true;
        }
        self.right = panel;
    }

    /// Serialize the user's persistent state (recent files, theme). Local only.
    pub fn persist(&self) -> String {
        let trusted: Vec<String> = self.session.trusted_certificates().iter().map(pdfcraft_engine::sign::x509::to_pem).collect();
        serde_json::json!({
            "recent": self.recent,
            "reopen_last_session": self.reopen_last_session,
            // Kept only while the preference is on.
            "last_session": self.reopen_last_session.then(|| self.session_to_save()),
            "pinned_folders": self.pinned.folders,
            "theme": self.theme_preference,
            "default_mode": self.default_mode,
            "default_layout": self.view_defaults.layout.as_str(),
            "default_zoom": self.view_defaults.zoom_name(),
            "highlight_fields": self.view_defaults.highlight_fields,
            "language": self.language,
            // Null follows the system's text scaling.
            "ui_scale": self.ui_scale,
            "flatten_fill_sign": self.flatten_fill_sign_on_save,
            "date_format": self.session.date_format(),
            // Null follows the interface language.
            "date_language": self.session.date_language(),
            "author": self.comment_prefs.author,
            // Per-tool comment defaults (colours, opacity, widths), so Make Current
            // Properties Default survives a restart (#340).
            "comment_styles": self
                .comment_prefs
                .styles()
                .map(|(t, s)| serde_json::json!({
                    "tool": t.command(),
                    "color": s.color,
                    "opacity": s.opacity,
                    "width": s.width,
                    "fill": s.fill,
                }))
                .collect::<Vec<_>>(),
            // Drawn signatures keep their original form (older settings read the same).
            "signature": match &self.signature { Some(fill_sign::SavedSig::Drawn(s)) => Some(s), _ => None },
            "signature_text": match &self.signature { Some(fill_sign::SavedSig::Typed(t)) => Some(t), _ => None },
            "signature_image": match &self.signature { Some(s @ fill_sign::SavedSig::Image(_)) => Some(s), _ => None },
            "initials": self.initials,
            "sign_graphic": self.sign_graphic,
            // macOS Keychain and Windows store identities are read from their OS key stores each time.
            "digital_ids": self.digital_ids.iter().filter(|e| !e.path.starts_with("keychain:") && !e.path.starts_with("windows:")).collect::<Vec<_>>(),
            "trusted": trusted,
            "custom_stamps": stamps_ui::encode(&self.custom_stamps),
            "javascript": self.session.javascript(),
            "actions": actions_ui::encode(&self.custom_actions),
            "combine_columns": self.combine_columns.to_json(),
            "combine_view": self.combine_view.as_str(),
            "combine_zoom": self.combine_zoom,
            "comments_panel_closed": self.comments_panel_closed,
        })
        .to_string()
    }

    /// Restore state written by `persist`. Unknown or malformed data is ignored.
    pub fn restore(&mut self, json: &str) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return };
        self.combine_columns = combine_ui::Columns::from_json(&v["combine_columns"]);
        if let Some(view) = v["combine_view"].as_str().and_then(combine_ui::CombineView::parse) {
            self.combine_view = view;
        }
        if let Some(zoom) = v["combine_zoom"].as_f64() {
            self.set_combine_zoom(zoom as f32);
        }
        if let Ok(r) = serde_json::from_value::<Vec<RecentFile>>(v["recent"].clone()) {
            // Only keep entries whose files still exist.
            #[cfg(not(target_arch = "wasm32"))]
            let r: Vec<RecentFile> = r.into_iter().filter(|f| std::path::Path::new(&f.path).exists()).collect();
            self.recent = r;
        }
        if let Some(on) = v["reopen_last_session"].as_bool() {
            self.reopen_last_session = on;
        }
        self.last_session = last_session::LastSession::from_json(&v["last_session"]);
        self.pinned.restore(&v["pinned_folders"]);
        if let Ok(preference) = serde_json::from_value::<ThemePreference>(v["theme"].clone()) {
            self.set_theme_preference(preference);
        }
        if let Ok(mode) = serde_json::from_value::<Mode>(v["default_mode"].clone()) {
            self.default_mode = mode;
        }
        if let Some(layout) = v["default_layout"].as_str().and_then(canvas::PageLayout::try_parse) {
            self.view_defaults.layout = layout;
        }
        if let Some(on) = v["highlight_fields"].as_bool() {
            self.view_defaults.highlight_fields = on;
        }
        if let Some(closed) = v["comments_panel_closed"].as_bool() {
            self.comments_panel_closed = closed;
        }
        if let Some(defaults) = v["default_zoom"].as_str().and_then(|zoom| self.view_defaults.with_zoom(zoom)) {
            self.view_defaults = defaults;
        }
        if let Some(language) = v["language"].as_str().and_then(i18n::normalize_pref) {
            self.language = language.to_string();
        }
        if let Some(on) = v["flatten_fill_sign"].as_bool() {
            self.flatten_fill_sign_on_save = on;
        }
        // Settings are untrusted: a size out of range follows the system instead.
        self.ui_scale = v["ui_scale"].as_f64().and_then(ui_scale::valid);
        // Settings are untrusted: an unusable pattern keeps the default.
        if let Some(f) = v["date_format"].as_str() {
            let _ = self.session.set_date_format(f);
        }
        if let Some(l) = v["date_language"].as_str() {
            let _ = self.session.set_date_language(Some(l));
        }
        // An empty or missing name keeps the login-name default; settings are untrusted, so the
        // name is cut to a sane length.
        if let Some(author) = v["author"].as_str().map(str::trim).filter(|a| !a.is_empty()) {
            self.comment_prefs.author = author.chars().take(MAX_AUTHOR_CHARS).collect();
        }
        // Per-tool comment styles (#340): each field goes through its clamping setter, so a
        // malformed or hostile entry is clamped or ignored rather than trusted.
        if let Some(styles) = v["comment_styles"].as_array() {
            for e in styles {
                let Some(tool) = e["tool"].as_str().and_then(crate::comments::CommentTool::from_command) else { continue };
                if let Some(c) = rgb_triple(&e["color"]) {
                    self.comment_prefs.set_color(tool, c);
                }
                if let Some(o) = e["opacity"].as_f64() {
                    self.comment_prefs.set_opacity(tool, o);
                }
                if let Some(w) = e["width"].as_f64() {
                    self.comment_prefs.set_width(tool, w);
                }
                // `"fill": null` is a saved "no fill", which clears a tool's default fill; a
                // missing or malformed fill keeps it.
                let fill = match e.get("fill") {
                    Some(serde_json::Value::Null) => Some(None),
                    Some(v) => rgb_triple(v).map(Some),
                    None => None,
                };
                if let Some(fill) = fill {
                    let mut s = self.comment_prefs.style(tool);
                    s.fill = fill;
                    self.comment_prefs.set_style(tool, s);
                }
            }
        }
        if let Ok(s) = serde_json::from_value::<Vec<Vec<[f32; 2]>>>(v["signature"].clone())
            && s.iter().all(|st| st.iter().all(|p| p.iter().all(|x| x.is_finite())))
        {
            self.signature = Some(fill_sign::SavedSig::Drawn(s));
        }
        if let Some(t) = v["signature_text"].as_str().filter(|t| !t.trim().is_empty()) {
            self.signature = Some(fill_sign::SavedSig::Typed(t.to_string()));
        }
        if let Ok(s @ fill_sign::SavedSig::Image(_)) = serde_json::from_value::<fill_sign::SavedSig>(v["signature_image"].clone()) {
            self.signature = Some(s);
        }
        if let Ok(i) = serde_json::from_value::<fill_sign::SavedSig>(v["initials"].clone()) {
            self.initials = Some(i);
        }
        // An unreadable choice (a damaged image) falls back to the name.
        self.sign_graphic = serde_json::from_value(v["sign_graphic"].clone()).unwrap_or_default();
        if let Ok(ids) = serde_json::from_value::<Vec<DigitalIdEntry>>(v["digital_ids"].clone()) {
            self.digital_ids = ids;
        }
        self.custom_stamps = stamps_ui::decode(&v["custom_stamps"]);
        self.custom_actions = actions_ui::decode(&v["actions"]);
        if let Some(on) = v["javascript"].as_bool() {
            self.session.set_javascript(on);
        }
        if let Ok(pems) = serde_json::from_value::<Vec<String>>(v["trusted"].clone()) {
            let certs = pems.iter().filter_map(|p| pdfcraft_engine::sign::x509::load_certificates(p.as_bytes()).ok()).flatten().collect();
            self.session.set_trusted_certificates(certs);
        }
    }

    /// `true` while any open document still waits for page renders (used by headless capture).
    pub fn render_pending(&self) -> bool {
        self.views.iter().any(|v| v.render_pending())
    }

    /// Apply a named view option (`--page 3`, `--panel bookmarks`, `--theme dark`, …).
    ///
    /// This is the seed of the UI control channel (M3.9): the same verbs become `ui.set` calls.
    pub fn set_option(&mut self, key: &str, value: &str) -> Result<(), String> {
        let view = self.active.and_then(|i| self.views.get_mut(i));
        match (key, view) {
            ("language", _) => {
                let language = i18n::normalize_pref(value).ok_or_else(|| {
                    let codes: Vec<&str> = std::iter::once(i18n::AUTO).chain(i18n::Lang::all().map(i18n::Lang::code)).collect();
                    format!("language must be one of {}", codes.join(", "))
                })?;
                self.language = language.to_string();
            }
            ("theme", _) => {
                let preference = match value {
                    "system" => ThemePreference::System,
                    "light" => ThemePreference::Light,
                    "dark" => ThemePreference::Dark,
                    _ => return Err("theme must be light, dark, or system".into()),
                };
                self.set_theme_preference(preference);
            }
            ("panel", _) => {
                self.right = match value {
                    "comments" => Some(RightPanel::Comments),
                    "bookmarks" => Some(RightPanel::Bookmarks),
                    "pages" => Some(RightPanel::Pages),
                    "fields" => Some(RightPanel::Fields),
                    "layers" => Some(RightPanel::Layers),
                    "attachments" => Some(RightPanel::Attachments),
                    "signatures" => Some(RightPanel::Signatures),
                    "accessibility" => Some(RightPanel::Accessibility),
                    "search" => Some(RightPanel::Search),
                    "compare" => Some(RightPanel::Compare),
                    "none" => None,
                    other => return Err(format!("unknown panel {other}")),
                }
            }
            ("mode", _) => {
                let mode = Mode::parse(value).unwrap_or(Mode::AllTools);
                self.mode_override = Some(mode);
                self.mode = mode;
            }
            ("default-mode", _) => {
                self.default_mode = Mode::parse(value).ok_or("default-mode must be all, read, edit, convert or sign")?;
            }
            ("date-format", _) => self.session.set_date_format(value)?,
            ("date-language", _) => self.session.set_date_language(Some(value).filter(|v| *v != "auto"))?,
            ("tool", _) => {
                let g = pdfcraft_engine::catalog::group(value).ok_or_else(|| format!("unknown tool {value}"))?;
                self.left = LeftPanel::Tool(g.id);
                self.left_open = true;
            }
            ("left", _) => self.left_open = value != "closed",
            ("home", _) => self.active = None,
            ("dialog", _) => {
                self.dialog = match value {
                    "properties" => Some(Dialog::Properties(PropsTab::Description)),
                    "security" => Some(Dialog::Properties(PropsTab::Security)),
                    "fonts" => Some(Dialog::Properties(PropsTab::Fonts)),
                    "advanced" => Some(Dialog::Properties(PropsTab::Advanced)),
                    "shortcuts" => Some(Dialog::Shortcuts),
                    "split" => Some(Dialog::Split),
                    "protect" => Some(Dialog::Protect),
                    "page-boxes" => Some(Dialog::PageBoxes),
                    "header-footer" => Some(Dialog::Marks(pdfcraft_engine::MarkKind::HeaderFooter)),
                    "watermark" => Some(Dialog::Marks(pdfcraft_engine::MarkKind::Watermark)),
                    "background" => Some(Dialog::Marks(pdfcraft_engine::MarkKind::Background)),
                    "export-image" => Some(Dialog::Export(export_ui::ExportKind::Image)),
                    "export-text" => Some(Dialog::Export(export_ui::ExportKind::Text)),
                    "export-all-images" => Some(Dialog::Export(export_ui::ExportKind::AllImages)),
                    "accessibility-options" => Some(Dialog::AccessibilityOptions),
                    "recognize-text" => Some(Dialog::RecognizeText),
                    "js-console" => Some(Dialog::JsConsole),
                    "document-js" => Some(Dialog::DocumentJs),
                    "preferences" => Some(Dialog::Preferences),
                    "compare-files" => Some(Dialog::CompareFiles),
                    "action-wizard" => Some(Dialog::ActionWizard),
                    "pdfa" => Some(Dialog::PdfA),
                    "signature" => Some(Dialog::Signature),
                    "optimize" => Some(Dialog::Optimize),
                    "sign" | "certify" => {
                        // Sign with a Digital ID for an invisible signature on the current page.
                        let page = self.active.map_or(0, |i| self.views[i].current);
                        self.start_signing(page, None, None, (value == "certify").then_some(2));
                        Some(Dialog::Sign)
                    }
                    "number-pages" => {
                        // Same path as the menu, so the page range is seeded.
                        self.execute("page.number");
                        Some(Dialog::NumberPages)
                    }
                    "none" => None,
                    _ => Some(Dialog::About),
                }
            }
            ("tools", _) => self.all_tools_expanded = value != "collapsed",
            ("palette", _) => {
                self.palette_open = true;
                self.palette_query = value.to_string();
            }
            ("page", Some(v)) => v.go_to_page(value.parse::<usize>().map_err(|e| e.to_string())?.saturating_sub(1)),
            ("zoom", Some(v)) => v.set_zoom(value.trim_end_matches('%').parse::<f32>().map_err(|e| e.to_string())? / 100.0),
            ("layout", Some(v)) => v.set_layout(canvas::PageLayout::try_parse(value).ok_or("layout must be continuous, single or two-up")?),
            ("cover", Some(v)) => {
                let on = match value {
                    "on" => true,
                    "off" => false,
                    _ => return Err("cover must be on or off".into()),
                };
                if on && !v.cover_applies() {
                    return Err("switch to two-page view first to show the cover page".into());
                }
                v.set_cover(on);
            }
            ("default-layout", _) => {
                self.view_defaults.layout = canvas::PageLayout::try_parse(value).ok_or("default-layout must be continuous, single or two-up")?;
            }
            ("default-zoom", _) => {
                self.view_defaults =
                    self.view_defaults.with_zoom(value).ok_or("default-zoom must be fit-width, fit-page or a percentage from 8 to 6400")?;
            }
            ("organize", Some(v)) => v.organize = value != "off",
            // `--grid-zoom 150`: the size of the pages in the organize grid, in percent.
            ("grid-zoom", Some(v)) => {
                let percent = value.trim_end_matches('%').parse::<f32>().map_err(|e| e.to_string())?;
                if !percent.is_finite() || !canvas::GRID_ZOOM_RANGE.contains(&(percent / 100.0)) {
                    return Err("grid-zoom must be between 50 and 300".into());
                }
                v.set_grid_zoom(percent / 100.0);
            }
            ("rotate", Some(v)) => {
                let deg: u16 = value.parse().map_err(|_| "rotate: 0, 90, 180 or 270")?;
                if !deg.is_multiple_of(90) {
                    return Err("rotate: 0, 90, 180 or 270".into());
                }
                v.rotation = deg % 360;
            }
            ("layer", _) => {
                // `--layer "Name=off"` / `"Name=on"`
                let (name, state) = value.rsplit_once('=').ok_or("expected NAME=on|off")?;
                let (i, id) = self.active_ids().ok_or("`layer` needs an open document")?;
                let idx = self
                    .session
                    .get(id)
                    .and_then(|d| d.info.layers.iter().position(|l| l.name == name))
                    .ok_or_else(|| format!("no layer named {name}"))?;
                if self.session.set_layer_visible(id, idx, state != "off") {
                    self.views[i].invalidate_content();
                }
            }
            ("find", Some(v)) => {
                v.open_find();
                if let Some(f) = v.find.as_mut() {
                    f.query = value.to_string();
                }
                v.rerun_find();
            }
            ("fields", Some(v)) => v.highlight_fields = value != "off",
            ("select", Some(v)) => {
                // `--select 2,3,5` (1-based) selects pages in the organize grid.
                let pages: Result<Vec<usize>, _> = value.split(',').map(|p| p.trim().parse::<usize>().map(|n| n.saturating_sub(1))).collect();
                v.select_pages(&pages.map_err(|_| "select: comma-separated page numbers")?);
            }
            ("notice", Some(v)) => v.notice_dismissed = value == "off",
            ("quick", _) => {
                // `--quick select|hand|note|freetext|highlight|underline|strikeout|ink|line|arrow|square|circle`
                self.quick_tool = match value {
                    "select" => QuickTool::Select,
                    measure if measure.starts_with("measure-") => QuickTool::Measure(
                        measure_ui::Tool::from_name(measure.trim_start_matches("measure-")).ok_or_else(|| format!("unknown tool {measure}"))?,
                    ),
                    "hand" => QuickTool::Hand,
                    "crop" => QuickTool::Crop,
                    "redact" => QuickTool::Redact,
                    "add-text" => QuickTool::AddText,
                    "edit-text" => QuickTool::EditText,
                    "link" => QuickTool::Link,
                    "sign" => QuickTool::SignArea { certify: false },
                    "marquee-zoom" => QuickTool::MarqueeZoom,
                    "snapshot" => QuickTool::Snapshot,
                    "column-select" => QuickTool::ColumnSelect,
                    "certify" => QuickTool::SignArea { certify: true },
                    custom if custom.starts_with("custom-stamp-") => {
                        let i: usize = custom[13..].parse().map_err(|_| format!("bad stamp {custom}"))?;
                        if i >= self.custom_stamps.len() {
                            return Err(format!("there are {} custom stamps", self.custom_stamps.len()));
                        }
                        QuickTool::CustomStamp(i)
                    }
                    stamp if stamp.starts_with("stamp-") => QuickTool::Stamp(
                        pdfcraft_engine::StampKind::ALL
                            .into_iter()
                            .find(|k| k.name().trim_start_matches("PC").eq_ignore_ascii_case(&stamp[6..]))
                            .ok_or_else(|| format!("unknown stamp {stamp}"))?,
                    ),
                    field if field.starts_with("field-") => QuickTool::Field(
                        prepare::FieldTool::from_command(&format!("form.add.{}", &field[6..])).ok_or_else(|| format!("unknown tool {field}"))?,
                    ),
                    fill if fill.starts_with("fill-") => QuickTool::Fill(
                        fill_sign::FillTool::from_command(&format!("sign.fill.{}", &fill[5..])).ok_or_else(|| format!("unknown tool {fill}"))?,
                    ),
                    other => {
                        let t = comments::CommentTool::from_command(&format!("comment.{other}")).ok_or_else(|| format!("unknown tool {other}"))?;
                        self.comment_prefs.group_tool[t.group()] = t;
                        QuickTool::Comment(t)
                    }
                };
            }
            ("author", _) => self.comment_prefs.author = value.to_string(),
            ("ui-scale", _) => self.ui_scale = ui_scale::parse(value)?,
            ("flatten-fill-sign", _) => {
                self.flatten_fill_sign_on_save = match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err("flatten-fill-sign must be true or false".into()),
                };
            }
            ("comment", Some(v)) => {
                // `--comment 2:4` selects the 4th annotation of page 2 (1-based, as comment_list reports).
                let (p, i) = value.split_once(':').ok_or("comment: PAGE:INDEX")?;
                let (p, i): (usize, usize) =
                    (p.trim().parse().map_err(|_| "comment: PAGE:INDEX")?, i.trim().parse().map_err(|_| "comment: PAGE:INDEX")?);
                v.comments.selected = Some((p.saturating_sub(1), i.saturating_sub(1)));
                v.comments.reveal = true;
            }
            // `--combine-view list`: Combine files as thumbnails (grid) or as the table (list).
            ("combine-view", _) => {
                self.combine_view = combine_ui::CombineView::parse(value).ok_or("combine-view must be grid or list")?;
            }
            // `--combine-zoom 150`: the size of the cards in the Combine grid, in percent.
            ("combine-zoom", _) => {
                let percent = value.trim_end_matches('%').parse::<f32>().map_err(|e| e.to_string())?;
                if !percent.is_finite() || !combine_grid::ZOOM_RANGE.contains(&(percent / 100.0)) {
                    return Err("combine-zoom must be between 60 and 200".into());
                }
                self.set_combine_zoom(percent / 100.0);
            }
            (k, None)
                if ["page", "zoom", "layout", "cover", "organize", "grid-zoom", "fields", "find", "rotate", "select", "notice", "comment"]
                    .contains(&k) =>
            {
                return Err(format!("`{k}` needs an open document"));
            }
            (other, _) => return Err(format!("unknown option {other}")),
        }
        Ok(())
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::Key;
        // "Save changes?" is modal: its keys are its own (⌘D is Don't save there, not Document
        // properties; Escape cancels it rather than clearing a selection), and nothing may run
        // underneath it. They are read here, before the canvas can consume them.
        if self.close_request.is_some() {
            if let Some(choice) = dialogs::save_prompt_key(ctx) {
                self.resolve_close(ctx, choice);
            }
            return;
        }
        // The other dialogs and prompts are modal too (⌘W closed the file behind Preferences,
        // #870).
        if self.modal_open() {
            return;
        }
        if let Some(view) = self.active.and_then(|i| self.views.get_mut(i))
            && view.auto_scroll.escape(ctx)
        {
            return;
        }
        self.registry_shortcuts(ctx);
        // (Esc closes a Combine file shown large first.)
        if self.full_screen && self.combine_tab.preview.is_none() && ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.set_full_screen(ctx, false);
        }
        if let Some(i) = self.active {
            // Select all belongs to the document or page grid, unless a text field or
            // overlay owns the keyboard. Other canvas shortcuts keep their own handling.
            if self.dialog.is_none()
                && !self.palette_open
                && !ctx.egui_wants_keyboard_input()
                && ctx.input_mut(|input| input.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, Key::A)))
            {
                if self.is_preparing() {
                    if let Some(doc) = self.session.get(self.views[i].id) {
                        prepare::select_all(&mut self.views[i], &doc.form);
                    }
                } else {
                    self.views[i].select_all();
                }
            }
            self.tool_keys(i, ctx);
            canvas::shortcuts(&mut self.views[i], ctx);
        }
    }
}

impl PdfCraftApp {
    /// The quick tools' keys, as in Acrobat: V selects, H pans, and holding Space pans until it
    /// is released. Plain letters, so not while a text field, a form field, a dialog or the
    /// palette has the keyboard.
    fn tool_keys(&mut self, i: usize, ctx: &egui::Context) {
        use egui::Key;
        let free = self.dialog.is_none() && !self.palette_open && !ctx.egui_wants_keyboard_input() && self.views[i].forms.focus.is_none();
        let (plain, space) = ctx.input(|input| (input.modifiers.is_none(), input.key_down(Key::Space)));
        if let Some(previous) = self.space_hand
            && !space
        {
            self.space_hand = None;
            if self.quick_tool == QuickTool::Hand {
                self.quick_tool = previous;
            }
        }
        if !free || !plain {
            return;
        }
        if ctx.input(|input| input.key_pressed(Key::V)) {
            self.quick_tool = QuickTool::Select;
            self.space_hand = None;
        }
        if ctx.input(|input| input.key_pressed(Key::H)) {
            self.quick_tool = QuickTool::Hand;
            self.space_hand = None;
        }
        if space && self.space_hand.is_none() && self.quick_tool != QuickTool::Hand {
            self.space_hand = Some(self.quick_tool);
            self.quick_tool = QuickTool::Hand;
        }
    }
}

impl eframe::App for PdfCraftApp {
    /// While files are dragged over the window the system sends no pointer moves, so egui
    /// would keep the place the pointer entered at: tell it where the pointer really is, so the
    /// page grid can show (and use) the gap under it.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if raw.hovered_files.is_empty() && raw.dropped_files.is_empty() {
            return;
        }
        if let Some(pos) = drag_pointer::in_window(ctx) {
            raw.events.push(egui::Event::PointerMoved(pos));
        }
        ctx.request_repaint();
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string("pdfcraft", self.persist());
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ctx = Some(ctx.clone());
        self.dialog_epoch();
        // Notices raised outside `ui` (opened files, OS events, the control channel) translate too.
        let lang = i18n::Lang::from_pref(&self.language);
        i18n::set_current(lang);
        // Simplified Chinese wants its own faces before the Japanese ones (one baseline per line).
        let hans = lang.code() == "zh-hans";
        if !self.styled {
            egui_extras::install_image_loaders(ctx);
            theme::install_fonts_for(ctx, hans);
            self.fonts_hans = hans;
            theme::apply(ctx, self.theme);
            self.styled = true;
        } else {
            self.fonts_ready = true;
            if hans != self.fonts_hans {
                // Same family names as before, so named fonts stay valid while the new set loads.
                theme::install_fonts_for(ctx, hans);
                self.fonts_hans = hans;
            }
        }
        self.sync_theme(ctx);
        ui_scale::sync(ctx, ui_scale::factor(self.ui_scale, self.system_text_scale), &mut self.ui_scale_applied);
        // Remember user work even if its tab is subsequently closed or Home is selected.
        self.startup_superseded |= !self.views.is_empty() || self.combine_showing() || self.dialog.is_some() || self.password_prompt.is_some();
        // Before taking this frame's drop: the grid must be drawn once with the pointer where
        // the files were let go before the gap is read.
        self.finish_grid_drop(ctx);
        // Showing a document hides the Combine files tab.
        if self.active.is_some() {
            self.combine_tab.focused = false;
        }
        #[cfg(target_arch = "wasm32")]
        self.process_signature_images();
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        #[cfg(not(target_arch = "wasm32"))]
        let dropped = self.drop_on_grid(dropped, ctx);
        for f in dropped {
            // Files dropped on the Combine files tab join its list instead of opening.
            if self.combine_showing() {
                self.drop_into_combine(f, ctx);
            } else {
                self.open_dropped(f, ctx);
            }
        }
        let arrived: Vec<(String, Vec<u8>)> = self.inbox.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        for (name, bytes) in arrived {
            if let Err(e) = self.open_bytes(&name, None, bytes) {
                self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &e.to_string())]);
            }
        }
        // Don't replace a pending prompt or import. A startup file may need its own password
        // prompt, so its bytes stay queued until the current interaction finishes.
        if self.password_prompt.is_none()
            && self.image_import.is_none()
            && self.dialog.is_none()
            && self.close_request.is_none()
            && self.pending_link.is_none()
        {
            let startup = self.startup_inbox.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
            for (name, bytes) in startup {
                let activate = !self.startup_superseded;
                if let Err(e) = self.open_bytes_with_focus(&name, None, bytes, activate) {
                    self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &e)]);
                }
            }
        }
        let failed: Vec<(String, String)> = self.failed_inbox.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        for (name, e) in failed {
            self.notify_fmt("Couldn't open {name}: {e}", &[("name", &name), ("e", &e)]);
        }
        let os_events = self.os_events.as_mut().map(|poll| poll()).unwrap_or_default();
        for e in os_events {
            match e {
                #[cfg(not(target_arch = "wasm32"))]
                OsEvent::Open(paths) => paths.iter().for_each(|p| self.open_path(p)),
                #[cfg(target_arch = "wasm32")]
                OsEvent::Open(_) => {}
                // Like closing the window: `guard_quit` asks about unsaved changes.
                OsEvent::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
        if let Some((_, id)) = self.active_ids() {
            let out = self.session.take_js_output(id);
            self.handle_js(id, out);
        }
        if let Some(mut control) = self.control.take() {
            control.tick(ctx, self);
            self.control = Some(control);
        }
        self.guard_quit(ctx);
        let now = ctx.input(|i| i.time);
        self.autosave_tick(now);
        if self.host_dirty.is_some() {
            let dirty = self.first_dirty().is_some();
            if let Some(report) = self.host_dirty.as_mut() {
                report(dirty);
            }
        }
        self.poll_updates();
        // Shortcuts deferred last frame: the text field has taken that frame's typing since.
        let deferred = std::mem::take(&mut self.deferred_commands);
        self.shortcuts(ctx);
        // Scrolling and group-drag previews are transient: never resume after changing tabs,
        // opening a modal/palette, or returning to a window that lost focus.
        let blocked = self.modal_open() || self.palette_open || !ctx.input(|i| i.focused);
        for (index, view) in self.views.iter_mut().enumerate() {
            view.objects.set_input_blocked(blocked || self.active != Some(index));
            if blocked || self.active != Some(index) {
                view.auto_scroll.cancel();
            }
        }
        // A field that refused its value this frame keeps the shortcut from saving or printing
        // behind the user's back; so does a save prompt opened since.
        if self.process_pending_edits() && self.close_request.is_none() {
            for (id, doc) in deferred {
                // Only in the document the shortcut was pressed in.
                if self.active_ids().map(|(_, active)| active) == doc {
                    self.execute(id);
                }
            }
        }
        self.poll_export();
        self.poll_ocr();
        self.poll_optimize();
        self.poll_action();
        self.process_file_requests();
        #[cfg(not(target_arch = "wasm32"))]
        self.process_picked();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        i18n::set_current(i18n::Lang::from_pref(&self.language));
        // Fonts registered via set_fonts only take effect next frame; named families would panic now.
        if !self.fonts_ready {
            ctx.request_repaint();
            return;
        }
        for view in &mut self.views {
            view.begin_render_frame();
        }
        // The window shows the active document's name (or title, if it asks for that).
        let title = self
            .active
            .and_then(|i| self.session.get(self.views[i].id))
            .map(|d| d.display_name())
            .or_else(|| self.combine_showing().then(|| tl!("Combine files").to_owned()))
            .map_or_else(|| "PdfCraft".to_owned(), |name| format!("{name} — PdfCraft"));
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
        if self.full_screen && self.active.is_some() {
            // Full screen: the page, nothing else (Esc or ⌘L to leave).
            let t = theme::Tokens::get(&ctx);
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(if t.dark() { t.pasteboard } else { egui::Color32::from_gray(32) })).show(
                ui,
                |ui| {
                    if let Some(i) = self.active {
                        canvas::document_area(self, i, ui);
                    }
                },
            );
            dialogs::show(self, &ctx);
            // Notices too: a refused field value or a failed save must be seen in full screen.
            self.show_progress(&ctx);
            widgets::toast(self, &ctx);
            self.finish_render_frame(&ctx);
            return;
        }
        chrome::tab_strip(self, ui);
        chrome::mode_bar(self, ui);
        if self.active.is_some() {
            chrome::right_rail(self, ui);
            if self.right.is_some() && self.mode != Mode::Read {
                panels::right_panel(self, ui);
            }
        }
        if self.left_open && self.mode != Mode::Read {
            panels::left_panel(self, ui);
        }
        let t = theme::Tokens::get(&ctx);
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.pasteboard)).show(ui, |ui| match self.active {
            None if self.combine_showing() => combine_ui::page(self, ui),
            None => home::show(self, ui),
            Some(i) => canvas::document_area(self, i, ui),
        });
        self.process_pending_edits();
        palette::show(self, &ctx);
        dialogs::show(self, &ctx);
        self.show_progress(&ctx);
        widgets::toast(self, &ctx);
        self.finish_render_frame(&ctx);
    }
}

impl PdfCraftApp {
    fn finish_render_frame(&mut self, ctx: &egui::Context) {
        // The Combine grid's thumbnails, whichever tab shows (renders under way still finish).
        self.combine_thumbs_frame(ctx);
        // Retire hidden documents before admitting the visible document's textures, so the
        // three cache limits apply to the application, regardless of how many tabs are open.
        for (i, view) in self.views.iter_mut().enumerate() {
            if Some(i) != self.active
                && let Some(doc) = self.session.get(view.id)
            {
                view.suspend_rendering(&doc.renderer);
            }
        }
        if let Some(view) = self.active.and_then(|i| self.views.get_mut(i))
            && let Some(doc) = self.session.get(view.id)
        {
            view.finish_render_frame(ctx, &doc.info, &doc.renderer);
        }
    }
}
