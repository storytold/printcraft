//! The command registry drives menus, shortcuts and the palette: every registered command must
//! be implemented, disabled commands must say why, and every surface must reach the same action.

use egui::os::OperatingSystem;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::commands::COMMANDS;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i));
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
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
    out
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, fixture(3)).unwrap();
        app
    });
    h.run_steps(4);
    h
}

/// Commands that open a native file picker can't run headless.
/// (Create ▸ Clipboard reads the system clipboard: tests use `create_from_clip`.)
const PICKERS: &[&str] = &[
    "file.open",
    "page.combine",
    "page.insert",
    "file.save_as",
    "create.file",
    "create.images",
    "create.multiple",
    "page.replace",
    "create.clipboard",
    "a11y.report",
    "ocr.recognize_batch",
    "form.merge_data",
    "export.docx",
    "export.html",
    "export.rtf",
    "measure.export",
];

#[test]
fn every_registered_command_is_implemented() {
    for spec in COMMANDS {
        if PICKERS.contains(&spec.id) {
            continue;
        }
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        if spec.id.starts_with("form.") || spec.id == "comment.flatten" {
            app.open_bytes("form.pdf", None, include_bytes!("data/form.pdf").to_vec()).unwrap();
        } else {
            app.open_bytes("doc.pdf", None, fixture(3)).unwrap();
        }
        // Give undo/redo something to do.
        app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
        if spec.id == "edit.redo" {
            app.undo();
        }
        if spec.id.starts_with("edit.") && (spec.id.ends_with(".update") || spec.id.ends_with(".remove")) {
            use pdfcraft_engine::{Background, Edit, HeaderFooter, Watermark};
            let mut hf = HeaderFooter::default();
            hf.text[1] = "x".into();
            app.apply_edit(Edit::AddHeaderFooter { pages: vec![0], settings: hf, replace: false });
            app.apply_edit(Edit::AddWatermark {
                pages: vec![0],
                settings: Watermark { text: "x".into(), ..Watermark::default() },
                replace: false,
                file: None,
            });
            app.apply_edit(Edit::AddBackground {
                pages: vec![0],
                settings: Background { color: [1.0; 3], opacity: 1.0, ..Background::default() },
                replace: false,
                file: None,
            });
        }
        if matches!(spec.id, "comment.flatten" | "comment.export" | "comment.summarize") {
            use pdfcraft_engine::{Edit, NewAnnotation, Shape, Style};
            let shape = Shape::Rectangle { rect: [10.0, 10.0, 50.0, 50.0] };
            app.apply_edit(Edit::AddAnnotation(NewAnnotation {
                page: 0,
                style: Style::default_for(&shape),
                shape,
                contents: String::new(),
                author: String::new(),
            }));
        }
        if spec.id.starts_with("redact.") {
            use pdfcraft_engine::{Edit, NewAnnotation, Shape, Style};
            let shape =
                Shape::Redact { quads: vec![pdfcraft_engine::rect_quad([10.0, 10.0, 50.0, 50.0])], overlay: String::new(), look: Default::default() };
            app.apply_edit(Edit::AddAnnotation(NewAnnotation {
                page: 0,
                style: Style::default_for(&shape),
                shape,
                contents: String::new(),
                author: String::new(),
            }));
        }
        if spec.id == "protect.remove" {
            let p = pdfcraft_engine::Protection { open_password: Some("pw".into()), ..Default::default() };
            app.apply_edit(pdfcraft_engine::Edit::Protect(p));
        }
        // The cover toggle needs two-page view first (it is disabled elsewhere).
        if spec.id == "view.layout.cover" {
            app.set_option("layout", "two-up").unwrap();
        }
        let dir = std::env::temp_dir().join(format!("pdfcraft-cmd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        app.save_override = Some(dir.join("out.pdf").to_string_lossy().into_owned());
        assert!(app.execute(spec.id), "{} is registered but not implemented (or wrongly disabled)", spec.id);
    }
}

#[test]
fn disabled_commands_explain_themselves() {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    assert!(!app.execute("file.save"));
    assert_eq!(app.toast.as_ref().map(|t| t.0.as_str()), Some("Open a document first"));
    app.open_bytes("doc.pdf", None, fixture(2)).unwrap();
    assert!(!app.execute("edit.undo"));
    assert_eq!(app.toast.as_ref().map(|t| t.0.as_str()), Some("Nothing to undo"));
    // View state counts too: the cover page exists only in two-page view.
    assert!(!app.execute("view.layout.cover"));
    assert_eq!(app.toast.as_ref().map(|t| t.0.as_str()), Some("Switch to two-page view first to show the cover page"));
    assert!(!app.execute("no.such.command"));
}

#[test]
fn shortcuts_run_registered_commands() {
    let mut h = harness();
    h.key_press_modifiers(Modifiers::COMMAND, Key::D);
    h.run_steps(2);
    assert!(matches!(h.state().dialog, Some(Dialog::Properties(_))));
    h.state_mut().dialog = None;
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run_steps(2);
    assert!(h.state().palette_open);
    h.key_press(Key::Escape);
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::CTRL, Key::H);
    h.run_steps(2);
    assert_eq!(h.state().mode, pdfcraft_ui_egui::Mode::Read);
}

#[test]
fn the_palette_lists_commands_with_shortcuts_and_runs_them() {
    let mut h = harness();
    h.state_mut().set_option("palette", "rotate pages clockwise").unwrap();
    h.run_steps(3);
    h.get_by_label("Rotate pages clockwise");
    h.key_press(Key::Enter);
    h.run_steps(3);
    let app = h.state();
    assert!(!app.palette_open, "Enter runs the top hit and closes the palette");
    let doc = app.session.get(app.views[0].id).unwrap();
    assert_eq!(doc.info.pages[0].rotation, 90);
    assert_eq!(doc.can_undo(), Some("Rotate page"));
}

#[test]
fn the_pages_menu_comes_from_the_registry() {
    let mut h = harness();
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("Pages ⏵").hover();
    h.run_steps(3);
    h.get_by_label_contains("Delete pages").click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.session.get(app.views[0].id).unwrap().info.pages.len(), 2);
}

#[test]
fn the_open_recent_menu_lists_files_and_opens_one() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-recent-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("recent.pdf");
    std::fs::write(&path, fixture(2)).unwrap();
    let path = path.to_string_lossy().into_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe({
        let path = path.clone();
        move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("doc.pdf", None, fixture(3)).unwrap();
            app.recent.push(pdfcraft_ui_egui::RecentFile { name: "recent.pdf".into(), path, pages: 2, size: 0 });
            app
        }
    });
    h.run_steps(4);
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("File ⏵").hover();
    h.run_steps(3);
    h.get_by_label("Open Recent ⏵").hover();
    h.run_steps(3);
    h.get_by_label_contains("recent.pdf").click();
    h.run_steps(4);
    let app = h.state();
    assert_eq!(app.views.len(), 2, "the recent file opened in a new tab");
    let active = app.active.unwrap();
    assert_eq!(app.session.get(app.views[active].id).and_then(|d| d.path.as_deref()), Some(path.as_str()), "the active tab is the recent file");
}

#[test]
fn the_open_recent_menu_is_disabled_while_the_list_is_empty() {
    let mut h = harness();
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("File ⏵").hover();
    h.run_steps(3);
    assert!(
        h.query_by(|n| n.label().as_deref() == Some("Open Recent") && n.is_disabled()).is_some(),
        "Open Recent is disabled while no file has been opened"
    );
}

fn recent_file(name: &str) -> pdfcraft_ui_egui::RecentFile {
    pdfcraft_ui_egui::RecentFile { name: name.into(), path: format!("/nowhere/{name}"), pages: 1, size: 0 }
}

#[test]
fn clear_recent_files_at_the_foot_of_open_recent_empties_the_list() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, fixture(3)).unwrap();
        app.recent.extend([recent_file("first.pdf"), recent_file("second.pdf")]);
        app
    });
    h.run_steps(4);
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("File ⏵").hover();
    h.run_steps(3);
    h.get_by_label("Open Recent ⏵").hover();
    h.run_steps(3);
    h.get_by_label("Clear Recent Files").click();
    h.run_steps(3);
    assert!(h.state().recent.is_empty(), "the list is empty");
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("File ⏵").hover();
    h.run_steps(3);
    assert!(h.query_by(|n| n.label().as_deref() == Some("Open Recent") && n.is_disabled()).is_some(), "and Open Recent is disabled");
}

#[test]
fn clear_recent_files_runs_from_the_palette_and_is_saved() {
    let mut app = PdfCraftApp::new();
    app.recent.extend([recent_file("first.pdf"), recent_file("second.pdf")]);
    assert!(app.execute("file.clear_recent"));
    assert!(app.recent.is_empty());
    let mut restarted = PdfCraftApp::new();
    restarted.recent.push(recent_file("stale.pdf"));
    restarted.restore(&app.persist());
    assert!(restarted.recent.is_empty(), "the empty list is what's saved");
    app.execute("file.clear_recent");
    assert!(app.toast.as_ref().is_some_and(|(m, _)| m.contains("No recent files")), "nothing to clear is said, not silent");
}

#[test]
fn the_shortcuts_dialog_lists_the_real_bindings() {
    let mut h = harness();
    h.ctx.set_os(OperatingSystem::Nix);
    h.state_mut().execute("help.shortcuts");
    h.run_steps(3);
    h.get_by_label("Ctrl+Shift+S");
    h.get_by_label("Save as");
    // The keys the document view handles itself are written for the platform too, and Fit
    // visible (a registered command) is listed once.
    h.get_by_label("Ctrl+1");
    h.get_by_label("Ctrl+3");
    h.get_by_label("Ctrl+G / Ctrl+Shift+G");
    h.get_by_label("← / →, Ctrl+← / Ctrl+→");
    h.get_by_label("Zoom in / out (also pinch or Ctrl-scroll)");
    assert_eq!(h.query_all_by_label_contains("⌘").count(), 0, "no ⌘ outside macOS");
    h.ctx.set_os(OperatingSystem::Mac);
    h.run_steps(2);
    h.get_by_label("⇧⌘S");
    h.get_by_label("⌘1");
    h.get_by_label("Zoom in / out (also pinch or ⌘-scroll)");
}

#[test]
fn localized_shortcuts_update_immediately_and_keep_command_ids_and_key_bindings() {
    let mut h = harness();
    h.ctx.set_os(OperatingSystem::Windows);
    h.state_mut().set_option("language", "de").unwrap();
    h.state_mut().execute("help.shortcuts");
    h.run_steps(3);
    h.get_by_label("Strg+Umschalt+S");
    h.state_mut().set_option("language", "en").unwrap();
    h.run_steps(2);
    h.get_by_label("Ctrl+Shift+S");
    h.state_mut().set_option("language", "de").unwrap();
    h.ctx.set_os(OperatingSystem::Mac);
    h.run_steps(2);
    h.get_by_label("⇧⌘S");
    h.key_press(Key::Escape);
    h.run_steps(2);
    h.ctx.set_os(OperatingSystem::Windows);
    h.key_press_modifiers(Modifiers::CTRL | Modifiers::COMMAND, Key::K);
    h.run_steps(3);
    assert!(h.state().palette_open, "the localized Strg+K display still binds Ctrl+K");
    // Searching by the stable command id still works in a translated interface.
    h.state_mut().set_option("palette", "view.read_mode").unwrap();
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(h.state().mode, pdfcraft_ui_egui::Mode::Read);
    assert!(!h.state().palette_open);
    let doc = h.state().session.get(h.state().views[0].id).unwrap();
    assert!(!doc.dirty);
}

/// Menu ▸ View open, with egui told the app runs on `os`.
fn view_menu(os: OperatingSystem) -> Harness<'static, PdfCraftApp> {
    let mut h = harness();
    h.ctx.set_os(os);
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("View ⏵").hover();
    h.run_steps(3);
    h
}

#[test]
fn view_menu_and_tooltips_write_shortcuts_for_the_platform() {
    // On Linux, View ▸ Zoom and Page navigation showed ⌘1, ⌘[ … (hard-coded) while the
    // registered items under them showed Ctrl+K; tooltips showed ⌘ too.
    let h = view_menu(OperatingSystem::Nix);
    h.get_by_label("Actual size Ctrl+1");
    h.get_by_label("Zoom to page level Ctrl+0");
    h.get_by_label("Rotate view counterclockwise Ctrl+Shift+−");
    h.get_by_label("Previous view Ctrl+[");
    h.get_by_label("Find tools and commands… Ctrl+K");
    h.get_by_label("Zoom in (Ctrl++)");
    h.get_by_label("Print (Ctrl+P)");
    assert_eq!(h.query_all_by_label_contains("⌘").count(), 0, "no ⌘ outside macOS");
    assert_eq!(h.query_all_by_label_contains("Fit visible").count(), 1, "View lists Fit visible once");
    h.get_by_label("Fit visible Ctrl+3");

    let h = view_menu(OperatingSystem::Mac);
    h.get_by_label("Actual size ⌘1");
    h.get_by_label("Fit visible ⌘3");
    h.get_by_label("Rotate view clockwise (⇧⌘+)");
    assert_eq!(h.query_all_by_label_contains("Ctrl+").count(), 0, "no Ctrl+ on macOS");
}

#[test]
fn full_screen_toggles_by_shortcut_and_command_and_escape_leaves() {
    let mut h = harness();
    h.key_press_modifiers(Modifiers::COMMAND, Key::L);
    h.run_steps(2);
    assert!(h.state().full_screen, "⌘L / Ctrl+L enters full screen");
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(!h.state().full_screen, "Escape leaves full screen");
    h.state_mut().execute("view.full_screen");
    h.run_steps(2);
    assert!(h.state().full_screen);
    h.state_mut().execute("view.full_screen");
    h.run_steps(2);
    assert!(!h.state().full_screen, "the command toggles back");
}

#[test]
fn the_about_dialog_shows_the_running_version() {
    let mut h = harness();
    h.state_mut().execute("help.about");
    h.run_steps(3);
    h.get_by_label_contains(&format!("Version {}", env!("CARGO_PKG_VERSION")));
}
