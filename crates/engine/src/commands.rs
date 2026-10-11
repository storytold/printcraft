//! The command registry: every user-facing action, by id (architecture §4, "everything is a
//! command").
//!
//! Menus, keyboard shortcuts, the ⌘K palette and automation (CLI `run`, the control channel and
//! MCP later) all go through this table. A frontend implements *what* each id does. The registry
//! says what it is called, where it appears, which key runs it, and when it is enabled, so that
//! every surface agrees.
//!
//! View-local keys (zoom, page navigation, find-next) stay with the document view. They act on
//! view state, not on the document.

use crate::{DocId, Session};

/// When a command can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Needs {
    /// Always.
    Nothing,
    /// A document is open.
    Document,
    /// The open document's security allows page changes (insert, delete, rotate, extract…).
    Assembly,
    /// The open document's security allows content and metadata changes.
    Modification,
    /// The open document's security allows adding and changing comments.
    Annotate,
    /// The document has form fields and its security allows filling them in.
    FillForms,
    /// The document has comments and allows changes.
    HasComments,
    /// The document has form fields and allows changes.
    HasFields,
    /// The document has redaction marks and allows changes.
    HasRedactions,
    /// The document has page marks of this kind and allows changes.
    Marks(crate::MarkKind),
    /// The document's security may be changed (owner, or nothing restricted).
    Security,
    /// The document is protected and its security may be removed.
    ProtectedSecurity,
    /// There is something to undo.
    Undo,
    /// There is something to redo.
    Redo,
    /// A document is open in two-page view. The engine has no views, so it checks the
    /// document and the frontend checks the view.
    TwoPageView,
}

/// A keyboard shortcut. `command` is ⌘ on macOS and Ctrl elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub command: bool,
    pub shift: bool,
    /// ⌃ on macOS (in addition to ⌘); unused elsewhere.
    pub mac_ctrl: bool,
    /// Key name: a letter, a digit, punctuation such as `,`, or `Delete`.
    pub key: &'static str,
}

impl Shortcut {
    /// ⌘ (Ctrl elsewhere) and `key`.
    pub const fn cmd(key: &'static str) -> Self {
        Self { command: true, shift: false, mac_ctrl: false, key }
    }

    /// ⇧⌘ (Ctrl+Shift elsewhere) and `key`.
    pub const fn cmd_shift(key: &'static str) -> Self {
        Self { command: true, shift: true, mac_ctrl: false, key }
    }

    /// The command modifier on its own, for text such as "⌘-scroll" / "Ctrl-scroll".
    pub fn command_name(mac: bool) -> &'static str {
        if mac { "⌘" } else { "Ctrl" }
    }

    /// How the shortcut is written in menus: `⇧⌘S` on macOS, `Ctrl+Shift+S` elsewhere.
    pub fn label(&self, mac: bool) -> String {
        if mac {
            let mut s = String::new();
            if self.mac_ctrl {
                s.push('⌃');
            }
            if self.shift {
                s.push('⇧');
            }
            if self.command {
                s.push('⌘');
            }
            s.push_str(self.key);
            s
        } else {
            let mut parts = Vec::new();
            if self.command || self.mac_ctrl {
                parts.push("Ctrl");
            }
            if self.shift {
                parts.push("Shift");
            }
            parts.push(self.key);
            parts.join("+")
        }
    }

    /// Number of modifiers (more specific shortcuts are matched first).
    pub fn modifier_count(&self) -> usize {
        usize::from(self.command) + usize::from(self.shift) + usize::from(self.mac_ctrl)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// Top-level menu it appears in (`File`, `Edit`, `View`, `Pages`, `Help`), if any.
    pub menu: Option<&'static str>,
    pub shortcut: Option<Shortcut>,
    pub needs: Needs,
    /// The shortcut also works while a text field has focus.
    pub in_text: bool,
    /// Lucide icon name for palettes and toolbars.
    pub icon: &'static str,
}

const fn c(
    id: &'static str,
    label: &'static str,
    menu: Option<&'static str>,
    shortcut: Option<Shortcut>,
    needs: Needs,
    icon: &'static str,
) -> CommandSpec {
    CommandSpec { id, label, menu, shortcut, needs, in_text: true, icon }
}

/// Like `c`, but the shortcut is left to text fields while one has focus (⌘Z, ⌘A…).
const fn ct(
    id: &'static str,
    label: &'static str,
    menu: Option<&'static str>,
    shortcut: Option<Shortcut>,
    needs: Needs,
    icon: &'static str,
) -> CommandSpec {
    CommandSpec { id, label, menu, shortcut, needs, in_text: false, icon }
}

use Needs::*;

const FILE: Option<&str> = Some("File");
const EDIT: Option<&str> = Some("Edit");
const VIEW: Option<&str> = Some("View");
const PAGES: Option<&str> = Some("Pages");
const HELP: Option<&str> = Some("Help");

/// Every command, in menu order.
pub const COMMANDS: &[CommandSpec] = &[
    c("file.open", "Open…", FILE, Some(Shortcut::cmd("O")), Nothing, "folder-open"),
    c("file.open_recent", "Open Recent", FILE, None, Nothing, "clock"),
    // Shown at the foot of File ▸ Open Recent and on Home, not as its own File menu item.
    c("file.clear_recent", "Clear Recent Files", None, None, Nothing, "trash-2"),
    c("file.pin_folder", "Pin folder to Home…", FILE, None, Nothing, "folder-plus"),
    c("create.blank", "New blank PDF", FILE, None, Nothing, "file-plus-2"),
    c("measure.distance", "Measure distance", None, None, Annotate, "ruler"),
    c("measure.perimeter", "Measure perimeter", None, None, Annotate, "ruler"),
    c("measure.area", "Measure area", None, None, Annotate, "ruler"),
    c("measure.scale", "Set measurement scale", None, None, Annotate, "ruler"),
    c("measure.info", "Measurement information", None, None, Document, "ruler"),
    c("measure.snap", "Measurement snapping", None, None, Document, "ruler"),
    c("measure.export", "Export measurements as CSV", None, None, Document, "file-output"),
    c("page.copy", "Copy pages", None, None, Document, "copy"),
    c("page.cut", "Cut pages", None, None, Assembly, "scissors"),
    c("page.paste", "Paste pages", None, None, Assembly, "clipboard-paste"),
    c("create.file", "Create PDF from file…", FILE, None, Nothing, "file-input"),
    c("create.multiple", "Create PDF from multiple files…", FILE, None, Nothing, "files"),
    c("create.images", "Create PDF from images…", FILE, None, Nothing, "image"),
    c("create.clipboard", "Create PDF from clipboard", FILE, None, Nothing, "copy-plus"),
    c("page.combine", "Combine files…", FILE, None, Nothing, "files"),
    c("file.save", "Save", FILE, Some(Shortcut::cmd("S")), Document, "save"),
    c("file.save_as", "Save as…", FILE, Some(Shortcut::cmd_shift("S")), Document, "save"),
    c("file.close", "Close file", FILE, Some(Shortcut::cmd("W")), Document, "x"),
    c("file.close_all", "Close all", FILE, Some(Shortcut::cmd_shift("W")), Document, "x"),
    c("file.revert", "Revert", FILE, None, Undo, "rotate-ccw"),
    c("print.dialog", "Print…", FILE, Some(Shortcut::cmd("P")), Document, "printer"),
    c("file.properties", "Document properties…", FILE, Some(Shortcut::cmd("D")), Document, "info"),
    ct("edit.undo", "Undo", EDIT, Some(Shortcut::cmd("Z")), Undo, "undo-2"),
    ct("edit.redo", "Redo", EDIT, Some(Shortcut::cmd_shift("Z")), Redo, "redo-2"),
    c("edit.find", "Find…", EDIT, Some(Shortcut::cmd("F")), Document, "search"),
    c("edit.advanced_search", "Advanced search…", EDIT, Some(Shortcut::cmd_shift("F")), Document, "search"),
    c("view.palette", "Find tools and commands…", VIEW, Some(Shortcut::cmd("K")), Nothing, "search"),
    // Page display: View ▸ Page display and the rail's button list the base layouts as radios and
    // continuous scrolling as a checkbox, so `menu = None` keeps them out of the generated menus.
    // The palette still runs them.
    c("view.layout.single", "Single page", None, None, Document, "file-text"),
    c("view.layout.two_up", "Two-page view", None, None, Document, "columns-2"),
    c("view.continuous", "Continuous scrolling", None, None, Document, "arrow-up-down"),
    c("view.layout.cover", "Show cover page in two-page view", None, None, TwoPageView, "bookmark"),
    // Acrobat's view modes, a page display and a zoom at once (the rail's Page display menu).
    c("view.fit_width_scrolling", "Fit to width scrolling", None, None, Document, "arrow-left-right"),
    c("view.fit_one_page", "Fit one full page", None, None, Document, "maximize-2"),
    // In View ▸ Zoom with the view-local zoom keys, so `menu = None` keeps it out of the generated
    // View menu (where it would show twice). The palette and ⌘3 still run it.
    c("view.fit_visible", "Fit visible", None, Some(Shortcut::cmd("3")), Document, "scan"),
    c("view.marquee_zoom", "Marquee zoom", VIEW, None, Document, "zoom-in"),
    // Acrobat's column select: drag a rectangle and select only the text inside it. Alt/Option-
    // drag with the Select tool does the same; this is the way in where the desktop takes
    // Alt-drag for moving windows (several Linux window managers).
    c("edit.column_select", "Column select", EDIT, None, Document, "text-select"),
    c("edit.snapshot", "Take a snapshot", EDIT, None, Document, "camera"),
    c("view.full_screen", "Full screen mode", VIEW, Some(Shortcut::cmd("L")), Document, "maximize"),
    c("view.read_mode", "Read mode", VIEW, Some(Shortcut { command: true, shift: false, mac_ctrl: true, key: "H" }), Document, "book-open"),
    c("view.focus_page_input", "Go to page…", VIEW, Some(Shortcut::cmd_shift("N")), Document, "text-cursor-input"),
    c("view.theme", "Switch light / dark theme", None, None, Nothing, "moon"),
    c("view.theme.system", "Use system setting", None, None, Nothing, "settings"),
    c("view.theme.light", "Light gray", None, None, Nothing, "sun"),
    c("view.theme.dark", "Dark gray", None, None, Nothing, "moon"),
    c("comment.list", "Comments panel", VIEW, None, Document, "message-square-text"),
    c("comment.note", "Add a sticky note", None, None, Annotate, "sticky-note"),
    c("comment.freetext", "Add a text box", None, None, Annotate, "type"),
    c("comment.highlight", "Highlight text", None, None, Annotate, "highlighter"),
    c("comment.underline", "Underline text", None, None, Annotate, "underline"),
    c("comment.strikeout", "Strikethrough text", None, None, Annotate, "strikethrough"),
    c("comment.squiggly", "Squiggly underline text", None, None, Annotate, "spline"),
    c("comment.ink", "Draw freehand", None, None, Annotate, "pencil"),
    c("comment.line", "Draw a line", None, None, Annotate, "minus"),
    c("comment.arrow", "Draw an arrow", None, None, Annotate, "move-right"),
    c("comment.square", "Draw a rectangle", None, None, Annotate, "square"),
    c("comment.circle", "Draw an oval", None, None, Annotate, "circle"),
    c("comment.polygon", "Draw a polygon", None, None, Annotate, "pentagon"),
    c("comment.polyline", "Draw connected lines", None, None, Annotate, "spline"),
    c("comment.cloud", "Draw a cloud", None, None, Annotate, "cloud"),
    c("comment.callout", "Add a callout", None, None, Annotate, "message-square-quote"),
    c("comment.caret", "Insert text", None, None, Annotate, "text-cursor-input"),
    c("comment.replace", "Replace text", None, None, Annotate, "replace"),
    c("comment.attach", "Attach a file", None, None, Annotate, "paperclip"),
    c("comment.eraser", "Erase drawings", None, None, Annotate, "eraser"),
    c("form.fields", "Form fields panel", VIEW, None, Document, "list"),
    c("form.clear", "Clear form", EDIT, None, FillForms, "eraser"),
    c("comment.flatten", "Flatten comments", None, None, HasComments, "layers"),
    c("comment.import", "Import comments…", None, None, Modification, "file-input"),
    c("comment.stamp", "Add a stamp", None, None, Annotate, "stamp"),
    c("comment.export", "Export all comments to data file…", None, None, HasComments, "file-output"),
    c("comment.hide_all", "Hide all comments", None, None, Document, "eye-off"),
    c("comment.summarize", "Summarize comments", None, None, HasComments, "file-text"),
    c("form.import_data", "Import form data…", None, None, HasFields, "file-input"),
    c("form.export_data", "Export form data…", None, None, HasFields, "file-output"),
    c("standards.pdfa", "PDF/A…", None, None, Document, "file-check"),
    c("actions.wizard", "Action Wizard…", None, None, Nothing, "list-checks"),
    c("actions.distribution", "Prepare for distribution…", None, None, Nothing, "send"),
    c("actions.optimize_scans", "Optimize scanned documents…", None, None, Nothing, "scan-text"),
    c("doc.compare", "Compare files…", None, None, Document, "git-compare"),
    c("form.detect", "Detect form fields", None, None, Modification, "scan"),
    c("form.merge_data", "Merge data files into spreadsheet…", None, None, Nothing, "file-spreadsheet"),
    c("form.flatten", "Flatten form fields", None, None, HasFields, "layers"),
    c("form.prepare", "Prepare a form", None, None, Modification, "text-cursor-input"),
    c("form.tab_order.row", "Tab order: by rows", None, None, HasFields, "rows-3"),
    c("form.tab_order.column", "Tab order: by columns", None, None, HasFields, "columns-3"),
    c("form.tab_order.structure", "Tab order: by document structure", None, None, HasFields, "list"),
    c("edit.text", "Add text", None, None, Modification, "type"),
    c("edit.edit_text", "Edit text & images", None, None, Modification, "text-cursor-input"),
    c("edit.link", "Add or edit links", None, None, Modification, "link-2"),
    c("edit.links_from_urls", "Create links from URLs", None, None, Modification, "link-2"),
    c("edit.remove_links", "Remove all links", None, None, Modification, "trash-2"),
    c("edit.bates", "Add Bates numbering…", None, None, Modification, "hash"),
    c("edit.image", "Add image…", None, None, Modification, "image-plus"),
    c("redact.mark", "Redact text and images", None, None, Modification, "rectangle-horizontal"),
    c("redact.pages", "Redact pages…", None, None, Modification, "file-x"),
    c("redact.search", "Find text and redact…", None, None, Modification, "file-search"),
    c("redact.properties", "Redaction properties…", None, None, Document, "settings-2"),
    c("redact.apply", "Apply redactions…", None, None, HasRedactions, "check"),
    c("redact.clear", "Clear redaction marks", None, None, HasRedactions, "eraser"),
    c("protect.remove_hidden", "Remove hidden information…", None, None, Modification, "eye-off"),
    c("redact.sanitize", "Sanitize document…", None, None, Modification, "sparkles"),
    c("form.field.properties", "Field properties…", None, None, HasFields, "settings-2"),
    c("form.add.text", "Add a text field", None, None, Modification, "text-cursor-input"),
    c("form.add.checkbox", "Add a checkbox", None, None, Modification, "check-circle-2"),
    c("form.add.radio", "Add a radio button", None, None, Modification, "circle"),
    c("form.add.combo", "Add a drop-down list", None, None, Modification, "chevron-down"),
    c("form.add.list", "Add a list box", None, None, Modification, "list"),
    c("form.add.button", "Add a button", None, None, Modification, "square"),
    c("form.add.image", "Add an image field", None, None, Modification, "image"),
    c("form.add.date", "Add a date field", None, None, Modification, "clock-3"),
    c("form.add.signature", "Add a digital signature field", None, None, Modification, "signature"),
    c("sign.digital", "Digitally sign", None, None, Document, "signature"),
    c("sign.certify", "Certify (visible signature)", None, None, Document, "badge-check"),
    c("sign.certify_invisible", "Certify (invisible signature)", None, None, Document, "badge-check"),
    c("sign.validate", "Validate all signatures", None, None, Document, "badge-check"),
    c("sign.panel", "Signatures panel", VIEW, None, Document, "signature"),
    c("sign.fill.text", "Fill & Sign: add text", None, None, Annotate, "type"),
    c("sign.fill.check", "Fill & Sign: checkmark", None, None, Annotate, "check"),
    c("sign.fill.cross", "Fill & Sign: cross", None, None, Annotate, "x"),
    c("sign.fill.dot", "Fill & Sign: dot", None, None, Annotate, "circle-dot"),
    c("sign.fill.line", "Fill & Sign: line", None, None, Annotate, "minus"),
    c("sign.fill.date", "Fill & Sign: date", None, None, Annotate, "clock-3"),
    c("sign.fill.signature", "Fill & Sign: sign", None, None, Annotate, "signature"),
    c("sign.fill.initials", "Fill & Sign: initials", None, None, Annotate, "signature"),
    c("sign.fill.signature.change", "Fill & Sign: change signature", None, None, Nothing, "signature"),
    c("sign.fill.signature.remove", "Fill & Sign: remove saved signature", None, None, Nothing, "x"),
    c("sign.fill.initials.remove", "Fill & Sign: remove saved initials", None, None, Nothing, "x"),
    c("sign.fill.initials.change", "Fill & Sign: change initials", None, None, Nothing, "signature"),
    c("export.image", "Export to image…", FILE, None, Document, "image"),
    c("optimize.reduce", "Reduce file size…", FILE, None, Document, "file-down"),
    c("optimize.advanced", "Optimize PDF…", FILE, None, Document, "settings-2"),
    c("export.text", "Export to text…", FILE, None, Document, "type"),
    c("export.docx", "Export to Word…", FILE, None, Document, "file-text"),
    c("export.html", "Export to HTML…", FILE, None, Document, "file-symlink"),
    c("export.rtf", "Export to RTF…", FILE, None, Document, "file-text"),
    c("app.preferences", "Preferences…", EDIT, Some(Shortcut::cmd(",")), Nothing, "settings"),
    c("tools.js_console", "JavaScript console…", None, Some(Shortcut::cmd("J")), Document, "square-terminal"),
    c("tools.document_js", "Document JavaScripts…", None, None, Modification, "file-code"),
    c("ocr.recognize", "Recognize text…", None, None, Modification, "scan-text"),
    c("ocr.recognize_batch", "Recognize text in multiple files…", None, None, Nothing, "files"),
    c("a11y.check", "Check for accessibility…", None, None, Document, "accessibility"),
    c("a11y.report", "Open accessibility report", None, None, Document, "file-text"),
    c("a11y.reading_options", "Change reading options…", None, None, Document, "book-open"),
    c("a11y.alt_text", "Add alternate text…", None, None, Modification, "image"),
    c("export.all_images", "Export all images…", FILE, None, Document, "image"),
    c("edit.header_footer", "Add header & footer…", None, None, Modification, "heading"),
    c("edit.header_footer.update", "Update header & footer…", None, None, Marks(crate::MarkKind::HeaderFooter), "heading"),
    c("edit.header_footer.remove", "Remove header & footer", None, None, Marks(crate::MarkKind::HeaderFooter), "heading"),
    c("edit.watermark", "Add watermark…", None, None, Modification, "stamp"),
    c("edit.watermark.update", "Update watermark…", None, None, Marks(crate::MarkKind::Watermark), "stamp"),
    c("edit.watermark.remove", "Remove watermark", None, None, Marks(crate::MarkKind::Watermark), "stamp"),
    c("edit.background", "Add background…", None, None, Modification, "palette"),
    c("edit.background.update", "Update background…", None, None, Marks(crate::MarkKind::Background), "palette"),
    c("edit.background.remove", "Remove background", None, None, Marks(crate::MarkKind::Background), "palette"),
    c("protect.password", "Protect using password…", FILE, None, Security, "lock"),
    c("protect.remove", "Remove security", FILE, None, ProtectedSecurity, "lock-open"),
    c("protect.properties", "Security properties…", FILE, None, Document, "shield-check"),
    c("page.organize", "Organize pages", PAGES, None, Document, "layout-grid"),
    c("bookmark.add", "New bookmark", PAGES, Some(Shortcut::cmd("B")), Assembly, "bookmark-plus"),
    c("bookmark.from_structure", "New bookmarks from structure", PAGES, None, Assembly, "list"),
    c("page.rotate", "Rotate pages clockwise", PAGES, None, Assembly, "rotate-cw"),
    c("page.rotate_ccw", "Rotate pages counterclockwise", PAGES, None, Assembly, "rotate-ccw"),
    c("page.delete", "Delete pages", PAGES, None, Assembly, "trash-2"),
    c("page.insert_blank", "Insert blank page", PAGES, None, Assembly, "file-plus"),
    c("page.rotate_dialog", "Rotate pages…", PAGES, None, Assembly, "rotate-cw"),
    c("page.duplicate", "Duplicate pages", PAGES, None, Assembly, "copy-plus"),
    c("page.crop", "Crop pages", PAGES, None, Assembly, "crop"),
    c("page.boxes", "Set page boxes…", PAGES, None, Assembly, "square-dashed-mouse-pointer"),
    c("page.insert", "Insert pages from file…", PAGES, None, Assembly, "file-input"),
    c("page.replace", "Replace pages…", PAGES, None, Assembly, "replace"),
    c("page.extract", "Extract pages…", PAGES, None, Assembly, "file-output"),
    c("page.split", "Split document…", PAGES, None, Assembly, "scissors"),
    c("page.number", "Number pages…", PAGES, None, Assembly, "hash"),
    c("help.shortcuts", "Keyboard shortcuts", HELP, None, Nothing, "circle-help"),
    c("help.discord", "Join the ArtCraft Discord", HELP, None, Nothing, "messages-square"),
    c("help.app_page", "PdfCraft web page", HELP, None, Nothing, "globe"),
    c("help.github", "PdfCraft on GitHub", HELP, None, Nothing, "code-xml"),
    c("help.website", "ArtCraft website", HELP, None, Nothing, "external-link"),
    c("help.check_updates", "Check for updates…", HELP, None, Nothing, "cloud"),
    c("help.about", "About PdfCraft", HELP, None, Nothing, "info"),
];

pub fn command(id: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|c| c.id == id)
}

/// Commands that appear in `menu`, in order.
pub fn menu(menu: &str) -> impl Iterator<Item = &'static CommandSpec> + '_ {
    COMMANDS.iter().filter(move |c| c.menu == Some(menu))
}

/// Whether `spec` can run now, for the document `active` (the focused tab).
pub fn is_enabled(spec: &CommandSpec, session: &Session, active: Option<DocId>) -> bool {
    let doc = active.and_then(|id| session.get(id));
    match spec.needs {
        Nothing => true,
        Document | TwoPageView => doc.is_some(),
        Assembly => doc.is_some_and(|d| d.allows_assembly()),
        Modification => doc.is_some_and(|d| d.allows_modification()),
        Annotate => doc.is_some_and(|d| d.allows_annotation()),
        FillForms => doc.is_some_and(|d| d.allows_form_filling() && !d.form.is_empty()),
        Marks(k) => doc.is_some_and(|d| d.allows_modification() && d.marks.contains(&k)),
        HasComments => doc.is_some_and(|d| d.allows_modification() && !d.info.annotations.is_empty()),
        HasFields => doc.is_some_and(|d| d.allows_modification() && !d.form.is_empty()),
        HasRedactions => doc.is_some_and(|d| d.allows_modification() && d.redaction_marks() > 0),
        Security => doc.is_some_and(|d| d.allows_security_change()),
        ProtectedSecurity => doc.is_some_and(|d| d.allows_security_change() && d.security_summary().is_some()),
        Undo => doc.is_some_and(|d| d.can_undo().is_some()),
        Redo => doc.is_some_and(|d| d.can_redo().is_some()),
    }
}

/// The label to show for `spec` now ("Undo Rotate page" rather than "Undo").
pub fn current_label(spec: &CommandSpec, session: &Session, active: Option<DocId>) -> String {
    let doc = active.and_then(|id| session.get(id));
    match (spec.id, doc) {
        ("edit.undo", Some(d)) => d.can_undo().map(|l| format!("Undo {l}")).unwrap_or_else(|| spec.label.into()),
        ("edit.redo", Some(d)) => d.can_redo().map(|l| format!("Redo {l}")).unwrap_or_else(|| spec.label.into()),
        _ => spec.label.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_shortcuts_are_unique() {
        let mut ids = std::collections::HashSet::new();
        let mut keys = std::collections::HashSet::new();
        for c in COMMANDS {
            assert!(ids.insert(c.id), "duplicate id {}", c.id);
            if let Some(s) = c.shortcut {
                assert!(keys.insert(s), "duplicate shortcut {} ({})", s.label(true), c.id);
            }
        }
    }

    #[test]
    fn every_ready_catalogue_item_is_a_registered_command() {
        for g in crate::catalog::TOOL_GROUPS {
            for s in g.sections {
                for i in s.items {
                    if i.availability == crate::catalog::Availability::Ready {
                        assert!(command(i.command).is_some(), "catalogue item {} ({}) is Ready but not registered", i.label, i.command);
                    }
                }
            }
        }
    }

    #[test]
    fn shortcut_labels_follow_platform_conventions() {
        let s = command("file.save_as").unwrap().shortcut.unwrap();
        assert_eq!(s.label(true), "⇧⌘S");
        assert_eq!(s.label(false), "Ctrl+Shift+S");
        assert_eq!(command("view.read_mode").unwrap().shortcut.unwrap().label(true), "⌃⌘H");
        assert_eq!(Shortcut::cmd("+").label(false), "Ctrl++");
        assert_eq!(Shortcut::cmd_shift("−").label(true), "⇧⌘−");
        assert_eq!(Shortcut::cmd_shift("−").label(false), "Ctrl+Shift+−");
        assert_eq!(Shortcut::command_name(true), "⌘");
        assert_eq!(Shortcut::command_name(false), "Ctrl");
    }

    #[test]
    fn enablement_follows_the_document_state() {
        let mut s = Session::new();
        let undo = command("edit.undo").unwrap();
        let save = command("file.save").unwrap();
        assert!(!is_enabled(save, &s, None));
        assert!(is_enabled(command("file.open").unwrap(), &s, None));
        let id = s.open("a.pdf", None, std::sync::Arc::new(crate::tests::fixture(2)), None).unwrap();
        assert!(is_enabled(save, &s, Some(id)));
        assert!(!is_enabled(undo, &s, Some(id)));
        s.apply(id, crate::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
        assert!(is_enabled(undo, &s, Some(id)));
        assert_eq!(current_label(undo, &s, Some(id)), "Undo Rotate page");
    }
}
