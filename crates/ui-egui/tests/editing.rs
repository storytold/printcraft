//! Headless UI tests for editing and saving: the organize toolbar, selection, undo/redo keys,
//! save/save-as, the unsaved-changes prompt and editable document properties.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_render::{PageRenderer, RenderRequest, RequestKind};
use pdfcraft_ui_egui::{CloseRequest, Dialog, PdfCraftApp};

/// An `n`-page document with a proper xref table; page `i` shows "Page i+1".
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

fn harness(pages: usize, setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, fixture(pages)).expect("fixture opens");
        setup(&mut app);
        app
    });
    h.run_steps(4);
    h
}

fn organize(pages: usize) -> Harness<'static, PdfCraftApp> {
    harness(pages, |app| app.set_option("organize", "on").unwrap())
}

/// Page labels of the active document, read back from its current bytes.
fn page_texts(app: &PdfCraftApp) -> Vec<String> {
    let doc = app.session.get(app.views[0].id).unwrap();
    let mut r = PageRenderer::new(doc.bytes.clone(), Default::default());
    (0..r.page_count())
        .map(|p| {
            let out = r.render(RenderRequest { page: p, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
            out.text.map(|t| t.plain_text().trim().to_string()).unwrap_or_default()
        })
        .collect()
}

fn dirty(h: &Harness<'static, PdfCraftApp>) -> bool {
    let app = h.state();
    app.session.get(app.views[0].id).is_some_and(|d| d.dirty)
}

fn temp_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-ui-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn organize_delete_then_undo_and_redo_with_keys() {
    let mut h = organize(3);
    h.get_by_label("Page 2").click();
    h.run_steps(2);
    h.get_by_label_contains("1 page selected");
    h.get_by_label("Delete pages (Delete)").click();
    h.run_steps(3);
    assert_eq!(page_texts(h.state()), ["Page 1", "Page 3"]);
    assert!(dirty(&h));
    h.get_by_label("doc.pdf (edited)"); // the tab shows unsaved changes

    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(page_texts(h.state()), ["Page 1", "Page 2", "Page 3"]);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run_steps(3);
    assert_eq!(page_texts(h.state()), ["Page 1", "Page 3"]);
}

#[test]
fn shift_click_selects_a_range_and_moves_it() {
    let mut h = organize(4);
    h.get_by_label("Page 1").click();
    h.run_steps(1);
    h.get_by_label("Page 2").click_modifiers(Modifiers::SHIFT);
    h.run_steps(2);
    h.get_by_label_contains("2 pages selected");
    h.get_by_label("Move later").click();
    h.run_steps(3);
    assert_eq!(page_texts(h.state()), ["Page 3", "Page 1", "Page 2", "Page 4"]);
    // The moved pages stay selected at their new position, so repeated clicks keep moving them.
    assert_eq!(h.state().views[0].selected.iter().copied().collect::<Vec<_>>(), [1, 2]);
    h.get_by_label("Move later").click();
    h.run_steps(3);
    assert_eq!(page_texts(h.state()), ["Page 3", "Page 4", "Page 1", "Page 2"]);
}

#[test]
fn command_click_toggles_and_rotation_applies_to_selection() {
    let mut h = organize(3);
    h.get_by_label("Page 1").click();
    h.run_steps(1);
    h.get_by_label("Page 3").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(3);
    let rot: Vec<u16> = {
        let app = h.state();
        app.session.get(app.views[0].id).unwrap().info.pages.iter().map(|p| p.rotation).collect()
    };
    assert_eq!(rot, [90, 0, 90]);
}

#[test]
fn organize_select_all_shortcut_preserves_current_page_and_sets_range_anchor() {
    let mut h = organize(4);
    h.get_by_label("Page 3").click();
    h.run_steps(1);
    // Automation may move the current selection without a click's range anchor.
    h.state_mut().views[0].select_pages(&[1]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(2);
    h.get_by_label_contains("4 pages selected");
    assert_eq!(h.state().views[0].target_pages(), [0, 1, 2, 3]);
    assert_eq!(h.state().views[0].current, 1, "selecting all keeps the reader's place");
    assert!(!dirty(&h), "selection doesn't edit the document");

    // Repeating Select all is harmless; deleting all pages still keeps the document intact.
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    h.key_press(Key::Delete);
    h.run_steps(2);
    assert_eq!(page_texts(h.state()).len(), 4);
    assert!(!dirty(&h));
    h.get_by_label("Page 4").click_modifiers(Modifiers::SHIFT);
    h.run_steps(2);
    assert_eq!(h.state().views[0].target_pages(), [1, 2, 3], "Shift-click extends from the current page");
    h.get_by_label("Page 2").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(h.state().views[0].target_pages(), [2, 3], "command-click still toggles a page");
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state().views[0].selected.is_empty());
}

#[test]
fn organize_select_all_applies_operations_to_every_page() {
    let mut h = organize(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(2);
    assert_eq!(h.state().views[0].target_pages(), [0, 1, 2]);
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(3);
    let app = h.state();
    let rotations: Vec<_> = app.session.get(app.views[0].id).unwrap().info.pages.iter().map(|p| p.rotation).collect();
    assert_eq!(rotations, [90, 90, 90]);
}

#[test]
fn organize_select_all_works_without_page_editing_permission() {
    let mut h = harness(3, |app| {
        app.apply_edit(pdfcraft_engine::Edit::Protect(pdfcraft_engine::Protection {
            permissions_password: Some("owner".into()),
            changes: pdfcraft_engine::Changes::None,
            ..Default::default()
        }));
        let bytes = app.session.save_bytes(app.views[0].id).unwrap();
        app.open_bytes("restricted.pdf", None, bytes.as_ref().clone()).unwrap();
        app.set_option("organize", "on").unwrap();
    });
    let index = h.state().active.unwrap();
    assert!(!h.state().session.get(h.state().views[index].id).unwrap().allows_assembly());
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(2);
    assert_eq!(h.state().views[index].target_pages(), [0, 1, 2]);
    assert!(!h.state().session.get(h.state().views[index].id).unwrap().dirty);

    let mut h = organize(1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(2);
    assert_eq!(h.state().views[0].target_pages(), [0]);
    assert_eq!(h.state().views[0].selected.len(), 1);

    // The opener refuses zero-page PDFs, but the view method also handles empty geometry.
    let mut empty = pdfcraft_ui_egui::canvas::DocView::new(pdfcraft_engine::DocId(0), &Default::default(), Default::default());
    empty.organize = true;
    assert!(!empty.select_all());
    assert!(empty.selected.is_empty());
}

#[test]
fn organize_select_all_leaves_focused_text_input_and_dialogs_alone() {
    let mut h = organize(3);
    h.query_all_by_value("1").next().expect("current page input").focus();
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    h.query_all_by_value("1").next().expect("current page input").type_text("2");
    h.run_steps(2);
    assert_eq!(h.state().views[0].page_input, "2", "Ctrl/Cmd+A selected the field's text");
    assert!(h.state().views[0].selected.is_empty());
    h.key_press(Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().views[0].current, 1);

    h.state_mut().execute("help.about");
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(2);
    assert!(h.state().views[0].selected.is_empty(), "a modal dialog isolates the page selection");
    h.state_mut().dialog = None;
    h.state_mut().execute("view.palette");
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(2);
    assert!(h.state().views[0].selected.is_empty(), "the palette keeps its own keyboard input");
}

#[test]
fn delete_is_refused_when_it_would_remove_every_page() {
    let mut h = organize(2);
    h.state_mut().views[0].select_pages(&[0, 1]);
    h.run_steps(2);
    h.key_press(Key::Delete);
    h.run_steps(3);
    assert_eq!(page_texts(h.state()).len(), 2, "a document keeps at least one page");
    assert!(!dirty(&h));
}

#[test]
fn insert_blank_page_after_selection() {
    let mut h = organize(2);
    h.get_by_label("Page 1").click();
    h.run_steps(1);
    h.get_by_label("Insert a blank page after the selection").click();
    h.run_steps(3);
    assert_eq!(page_texts(h.state()), ["Page 1", "", "Page 2"]);
    let app = h.state();
    let info = &app.session.get(app.views[0].id).unwrap().info;
    assert_eq!((info.pages[1].width, info.pages[1].height), (200.0, 300.0), "matches the neighbouring page");
}

#[test]
fn save_writes_an_incremental_update_and_clears_dirty() {
    let out = temp_path("saved.pdf");
    let original = fixture(3);
    let target = out.to_string_lossy().into_owned();
    let mut h = organize(3);
    h.state_mut().save_override = Some(target.clone());
    h.get_by_label("Page 3").click();
    h.run_steps(1);
    h.get_by_label("Rotate counterclockwise").click();
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run_steps(3);
    let saved = std::fs::read(&out).expect("file written");
    assert_eq!(&saved[..original.len()], &original[..], "original bytes untouched");
    assert!(!dirty(&h));
    h.get_by_label("saved.pdf"); // the tab takes the new name, no edited marker
    // What was written is what the app now shows.
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().bytes.as_slice(), saved.as_slice());
    let reopened = pdfcraft_render::inspect(std::sync::Arc::new(saved), None).unwrap();
    assert_eq!(reopened.pages[2].rotation, 270);
    let _ = std::fs::remove_file(out);
}

#[test]
fn closing_a_dirty_tab_asks_and_cancel_keeps_it() {
    let mut h = organize(2);
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    h.run_steps(3);
    h.get_by_label_contains("Save changes to “doc.pdf”");
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 1, "cancel keeps the document open");
    assert!(dirty(&h));
    assert!(h.state().close_request.is_none());

    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    h.run_steps(3);
    h.get_by_label("Don't save").click();
    h.run_steps(3);
    assert!(h.state().views.is_empty(), "discarding closes the tab");
}

#[test]
fn closing_a_dirty_tab_can_save_first() {
    let out = temp_path("closed.pdf");
    let mut h = organize(2);
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(3);
    h.state_mut().request_close_tab(0);
    h.run_steps(2);
    h.get_by_label("Save").click();
    h.run_steps(3);
    assert!(h.state().views.is_empty());
    let saved = pdfcraft_render::inspect(std::sync::Arc::new(std::fs::read(&out).unwrap()), None).unwrap();
    assert_eq!(saved.pages[0].rotation, 90);
    let _ = std::fs::remove_file(out);
}

#[test]
fn the_save_prompt_answers_to_the_keyboard() {
    // Issue #8: Enter saves; ⌘D / Ctrl+D, Alt+D and Alt+N don't save; Escape cancels. While the
    // prompt is open, ⌘D is not Document properties.
    for (m, key) in [(Modifiers::COMMAND, Key::D), (Modifiers::ALT, Key::D), (Modifiers::ALT, Key::N)] {
        let mut h = organize(2);
        h.get_by_label("Rotate clockwise").click();
        h.run_steps(3);
        h.key_press_modifiers(Modifiers::COMMAND, Key::W);
        h.run_steps(3);
        h.get_by_label_contains("Save changes to “doc.pdf”");
        h.key_press_modifiers(m, key);
        h.run_steps(3);
        assert!(h.state().views.is_empty(), "{m:?}+{key:?} doesn't save and closes");
        assert!(h.state().dialog.is_none(), "{m:?}+{key:?} ran no command underneath");
    }
    let mut h = organize(2);
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    h.run_steps(3);
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 1, "Escape cancels");
    assert!(h.state().close_request.is_none());

    let out = temp_path("enter-saves.pdf");
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    h.run_steps(3);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert!(h.state().views.is_empty(), "Enter saves and closes");
    let saved = pdfcraft_render::inspect(std::sync::Arc::new(std::fs::read(&out).unwrap()), None).unwrap();
    assert_eq!(saved.pages[0].rotation, 90);
    let _ = std::fs::remove_file(out);
}

#[test]
fn keys_leave_the_document_under_a_dialog_alone() {
    // Issue #870: ⌘W with Preferences open closed the file underneath it, and in the page grid
    // Delete deleted the selected page. While a dialog is open its keys are its own.
    let mut h = organize(3);
    h.get_by_label("Page 2").click();
    h.run_steps(2);
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(3);
    assert!(h.state_mut().execute("app.preferences"));
    h.run_steps(3);
    let shift = Modifiers::COMMAND | Modifiers::SHIFT;
    for (m, key) in [
        (Modifiers::COMMAND, Key::W),
        (shift, Key::W),
        (Modifiers::COMMAND, Key::Z),
        (Modifiers::COMMAND, Key::D),
        (Modifiers::COMMAND, Key::F),
        (Modifiers::NONE, Key::Delete),
        (Modifiers::NONE, Key::Backspace),
    ] {
        h.key_press_modifiers(m, key);
        h.run_steps(3);
        let app = h.state();
        assert_eq!(app.views.len(), 1, "{m:?}+{key:?} closed the file");
        assert!(app.close_request.is_none(), "{m:?}+{key:?} asked to close the file");
        assert!(app.session.get(app.views[0].id).unwrap().can_undo().is_some(), "{m:?}+{key:?} undid the rotation");
        assert!(app.views[0].find.is_none(), "{m:?}+{key:?} opened Find");
        assert_eq!(app.dialog, Some(Dialog::Preferences), "{m:?}+{key:?} replaced Preferences");
    }
    assert_eq!(page_texts(h.state()), ["Page 1", "Page 2", "Page 3"], "a page was deleted under the dialog");
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(h.state().dialog.is_none(), "Escape closes Preferences");
    assert_eq!(h.state().views[0].target_pages(), [1], "Escape went to the dialog, not the page selection");
    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    h.run_steps(3);
    assert!(h.state().close_request.is_some(), "with the dialog closed, ⌘W closes the file again");
}

#[test]
fn clean_tabs_close_without_asking() {
    let mut h = harness(1, |_| {});
    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    h.run_steps(3);
    assert!(h.state().views.is_empty());
    assert!(h.query_by_label("Don't save").is_none());
}

#[test]
fn quitting_with_unsaved_changes_asks_for_each_document() {
    let mut h = harness(2, |app| {
        app.open_bytes("second.pdf", None, fixture(1)).unwrap();
    });
    // Edit both documents.
    for tab in 0..2 {
        h.state_mut().active = Some(tab);
        h.state_mut().views[tab].select_pages(&[0]);
        h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
    }
    h.state_mut().close_request = Some(CloseRequest::Quit);
    h.run_steps(3);
    h.get_by_label_contains("Save changes to “doc.pdf”");
    h.get_by_label("Don't save").click();
    h.run_steps(3);
    h.get_by_label_contains("Save changes to “second.pdf”");
    h.get_by_label("Don't save").click();
    h.run_steps(3);
    assert!(h.state().views.is_empty());
    assert!(h.state().close_request.is_none());
}

/// PDFs written to a fresh temp folder, removed when the test ends.
struct TempPdfs(std::path::PathBuf);

impl TempPdfs {
    fn new(test: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pdfcraft-session-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn file(&self, name: &str, pages: usize) -> String {
        let path = self.0.join(name);
        std::fs::write(&path, fixture(pages)).unwrap();
        path.to_string_lossy().into_owned()
    }
}

impl Drop for TempPdfs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The app with Preferences ▸ Reopen the files that were open when PdfCraft last closed on.
fn session_harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.reopen_last_session = true;
        setup(&mut app);
        app
    });
    h.run_steps(4);
    h
}

fn open_paths(app: &PdfCraftApp) -> Vec<String> {
    app.views.iter().map(|v| app.session.get(v.id).and_then(|d| d.path.clone()).unwrap_or_default()).collect()
}

#[test]
fn the_last_session_reopens_its_files_pages_and_active_tab() {
    let dir = TempPdfs::new("reopen");
    let files = [dir.file("a.pdf", 1), dir.file("b.pdf", 5), dir.file("c.pdf", 2)];
    let opened = files.clone();
    let mut h = session_harness(move |app| opened.iter().for_each(|f| app.open_path(f)));
    h.state_mut().views[1].go_to_page(3);
    h.state_mut().active = Some(1);
    h.run_steps(4);
    assert_eq!(h.state().views[1].current, 3, "b.pdf shows page 4");
    let settings = h.state().persist();
    let mut h = session_harness(move |app| {
        app.restore(&settings);
        app.reopen_last_files(&[]);
    });
    h.run_steps(4);
    let app = h.state();
    assert_eq!(open_paths(app), files, "the same files, in the same order");
    assert_eq!(app.active, Some(1), "b.pdf is in front again");
    assert_eq!(app.views[1].current, 3, "at the page it showed");
}

#[test]
fn quitting_with_unsaved_changes_still_remembers_every_file() {
    let dir = TempPdfs::new("quit");
    let files = [dir.file("a.pdf", 1), dir.file("b.pdf", 1)];
    let opened = files.clone();
    let mut h = session_harness(move |app| opened.iter().for_each(|f| app.open_path(f)));
    for tab in 0..2 {
        h.state_mut().active = Some(tab);
        h.state_mut().views[tab].select_pages(&[0]);
        h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
    }
    h.state_mut().active = Some(1);
    h.state_mut().close_request = Some(CloseRequest::Quit);
    h.run_steps(3);
    // Each unsaved tab closes as it is answered, before PdfCraft quits.
    for _ in 0..2 {
        h.get_by_label("Don't save").click();
        h.run_steps(3);
    }
    assert!(h.state().views.is_empty());
    let mut app = PdfCraftApp::new();
    app.restore(&h.state().persist());
    app.reopen_last_files(&[]);
    assert_eq!(open_paths(&app), files, "both files come back");
    assert_eq!(app.active, Some(1), "with b.pdf in front, as when the quit began");
}

#[test]
fn the_last_session_is_off_by_default_and_skips_missing_and_duplicate_files() {
    let dir = TempPdfs::new("skip");
    let (a, b) = (dir.file("a.pdf", 1), dir.file("b.pdf", 1));
    let mut app = PdfCraftApp::new();
    app.open_path(&a);
    app.open_path(&b);
    let off: serde_json::Value = serde_json::from_str(&app.persist()).unwrap();
    assert!(off["last_session"].is_null(), "nothing is kept while the preference is off");
    app.reopen_last_session = true;
    let settings = app.persist();
    let mut later = PdfCraftApp::new();
    later.restore(&settings);
    later.reopen_last_session = false;
    later.reopen_last_files(&[]);
    assert!(later.views.is_empty(), "turned off again: nothing reopens");
    // b.pdf is gone, and a.pdf is about to open from the command line.
    std::fs::remove_file(&b).unwrap();
    let mut later = PdfCraftApp::new();
    later.restore(&settings);
    later.reopen_last_files(&[a]);
    assert!(later.views.is_empty(), "neither opens from the session");
}

#[test]
fn save_prompt_stays_inside_the_screen_for_a_long_filename() {
    // Issue #161: an unwrapped title carrying a long filename widened the centered modal past
    // the viewport, clipping the message and pushing the Save/Cancel buttons off-screen.
    let name = "Psychology_ The Science of Mind and Behaviour, -- Nigel Holt, Andy Bremner, Michael \
                Vliek, Ed Sutherland, -- 5, 2024 -- McGraw-Hill Education (UK) Ltd -- isbn13 97815268.pdf";
    let mut h = Harness::builder().with_size(egui::vec2(1365.0, 719.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes(name, None, fixture(1)).expect("fixture opens");
        app.close_request = Some(CloseRequest::Tab(app.views[0].id));
        app
    });
    h.run_steps(4);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1365.0, 719.0));
    let inside = |r: egui::Rect| screen.contains(r.min) && screen.contains(r.max);
    let title = h.get_by_label_contains("Save changes to");
    let title_rect = title.rect();
    assert!(inside(title_rect), "the title rect {title_rect:?} leaves the screen");
    for button in ["Save", "Cancel", "Don't save"] {
        let rect = h.get_by_label(button).rect();
        assert!(inside(rect), "the {button} button rect {rect:?} leaves the screen");
    }
}

#[test]
fn save_prompt_fits_the_smallest_window_whatever_the_name() {
    // Issue #236: wrapping (#161) kept the prompt narrow, but a long enough name still grew it
    // taller than the window and pushed the buttons off-screen. On the web a `?file=` URL names
    // the document, so the name has no length limit. Long names now give way in the middle.
    let names = [
        format!("{}.pdf", "a".repeat(400)),
        format!("{}.pdf", "Quarterly_Report_FY2026_Final_v3_".repeat(12)),
        format!("{}.pdf", "รายงานประจำปีงบประมาณ".repeat(15)),
        format!("{}.pdf", "年".repeat(251)),
        format!("{}.pdf", "W".repeat(2000)),
    ];
    // The desktop window's minimum inner size (apps/pdfcraft/src/main.rs).
    let size = egui::vec2(820.0, 520.0);
    for name in names {
        let start: String = name.chars().take(10).collect();
        let mut h = Harness::builder().with_size(size).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes(&name, None, fixture(1)).expect("fixture opens");
            app.close_request = Some(CloseRequest::Tab(app.views[0].id));
            app
        });
        h.run_steps(4);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let inside = |r: egui::Rect| screen.contains(r.min) && screen.contains(r.max);
        let title = h.get_by_label_contains("Save changes to");
        let title_rect = title.rect();
        assert!(inside(title_rect), "{start}…: the title rect {title_rect:?} leaves the screen");
        let opening = format!("Save changes to “{start}");
        assert!(
            h.query_by(|n| n.value().is_some_and(|l| l.starts_with(&opening) && l.contains('…') && l.contains(".pdf” before closing?"))).is_some(),
            "{start}…: the title keeps the start and the extension"
        );
        for button in ["Save", "Cancel", "Don't save"] {
            let rect = h.get_by_label(button).rect();
            assert!(inside(rect), "{start}…: the {button} button rect {rect:?} leaves the screen");
        }
    }
}

#[test]
fn document_properties_edit_is_one_undoable_step() {
    let mut h = harness(1, |app| app.set_option("dialog", "properties").unwrap());
    let title = h.get_by_role_and_label(Role::TextInput, "Title");
    title.focus();
    title.type_text("Annual report");
    h.run_steps(2);
    let author = h.get_by_role_and_label(Role::TextInput, "Author");
    author.focus();
    author.type_text("Finance team");
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    assert_eq!(doc.info.title.as_deref(), Some("Annual report"), "inspection sees the new title");
    assert_eq!(doc.info_value("Author").as_deref(), Some("Finance team"));
    assert_eq!(doc.can_undo(), Some("Change document properties"));
    assert!(app.dialog.is_none());

    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    assert_eq!(doc.info.title, None);
    assert_eq!(doc.info_value("Author"), None);
}

#[test]
fn cancelling_properties_discards_the_draft() {
    let mut h = harness(1, |app| app.set_option("dialog", "properties").unwrap());
    let title = h.get_by_role_and_label(Role::TextInput, "Title");
    title.focus();
    title.type_text("Draft");
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(!dirty(&h));
    assert!(h.state().props_draft.is_none());
}

#[test]
fn edit_menu_names_the_step_to_undo() {
    let mut h = harness(2, |app| {
        app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
    });
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("Edit ⏵").hover(); // submenus open on hover; their labels carry the arrow
    h.run_steps(3);
    h.get_by_label_contains("Undo Rotate page").click(); // the label includes the shortcut
    h.run_steps(3);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    assert_eq!(doc.can_redo(), Some("Rotate page"));
    assert_eq!(doc.info.pages[0].rotation, 0);
}

// ── Combine / insert from file / extract / split ──────────────────────────────────────────────

fn texts_of(app: &PdfCraftApp, tab: usize) -> Vec<String> {
    let doc = app.session.get(app.views[tab].id).unwrap();
    let mut r = PageRenderer::new(doc.bytes.clone(), Default::default());
    (0..r.page_count())
        .map(|p| {
            let out = r.render(RenderRequest { page: p, kind: RequestKind::Text, scale: 1.0, ..Default::default() });
            out.text.map(|t| t.plain_text().trim().to_string()).unwrap_or_default()
        })
        .collect()
}

#[test]
fn combining_files_opens_a_new_unsaved_tab() {
    let mut h = harness(1, |app| {
        app.use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("one.pdf".into(), fixture(2)), ("two.pdf".into(), fixture(1))]);
    });
    h.run_steps(3);
    h.get_by_label("Combine").click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.views.len(), 2);
    assert_eq!(app.active, Some(1));
    assert_eq!(texts_of(app, 1), ["Page 1", "Page 2", "Page 1"]);
    let doc = app.session.get(app.views[1].id).unwrap();
    assert_eq!(doc.name, "Combined.pdf");
    assert!(doc.dirty && doc.path.is_none(), "unsaved until the user saves it");
    assert_eq!(doc.info.outline.iter().map(|o| o.title.as_str()).collect::<Vec<_>>(), ["one", "two"]);
    h.get_by_label("Combined.pdf (edited)");
}

#[test]
fn combine_files_takes_chosen_pages_in_the_order_listed() {
    let mut h = harness(1, |app| {
        app.set_option("combine-view", "list").unwrap();
        app.use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("one.pdf".into(), fixture(3)), ("two.pdf".into(), fixture(2))]);
    });
    h.run_steps(3);
    h.get_by_label_contains("Files are combined from top to bottom");
    // Nothing is selected yet: the toolbar's Move up is there but disabled.
    assert!(h.get_by_label("Move up").accesskit_node().is_disabled());
    // Select two.pdf with the keyboard and move it up: two.pdf first; one.pdf's pages 3 and 1.
    h.key_press(Key::ArrowUp);
    h.run_steps(2);
    assert_eq!(h.state().combine_selection(), [1]);
    h.get_by_label("Move up").click();
    h.run_steps(2);
    assert_eq!(h.state().combine_draft[0].name, "two.pdf");
    assert_eq!(h.state().combine_selection(), [0], "the selection follows the moved file");
    h.state_mut().combine_draft[1].range = "3, 1".into();
    h.run_steps(2);
    h.get_by_label("2 of 3");
    h.get_by_label("Combine").click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(texts_of(app, 1), ["Page 1", "Page 2", "Page 3", "Page 1"]);
    let doc = app.session.get(app.views[1].id).unwrap();
    assert_eq!(doc.info.outline.iter().map(|o| o.title.as_str()).collect::<Vec<_>>(), ["two", "one"]);
    assert!(app.combine_draft.is_empty());
    assert!(!app.combine_tab.open, "the tab makes way for the result");
}

#[test]
fn combine_files_opens_in_its_own_tab() {
    let mut h = harness(1, |_| {});
    h.state_mut().execute("page.combine");
    h.run_steps(3);
    assert!(h.state().combine_showing() && h.state().active.is_none());
    h.get_by_label("Add the PDFs to combine");
    // Back to the document, then to Combine through its tab.
    h.get_by_label("doc.pdf").click();
    h.run_steps(3);
    assert_eq!(h.state().active, Some(0));
    assert!(!h.state().combine_showing() && h.state().combine_tab.open);
    // The tab, in the tab strip (All tools lists Combine files too).
    h.get_all_by_label("Combine files").find(|n| n.rect().top() < 40.0).unwrap().click();
    h.run_steps(3);
    assert!(h.state().combine_showing());
    // Home hides it too.
    h.get_by_label("Home").click();
    h.run_steps(3);
    assert!(!h.state().combine_showing() && h.state().combine_tab.open);
}

#[test]
fn closing_the_combine_tab_forgets_its_list() {
    let mut h = harness(1, |app| {
        app.use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("one.pdf".into(), fixture(1))]);
    });
    h.run_steps(3);
    assert!(h.state().combine_showing());
    h.state_mut().close_combine_tab();
    h.run_steps(3);
    let app = h.state();
    assert!(app.combine_draft.is_empty() && !app.combine_tab.open);
    assert_eq!(app.active, Some(0), "the document next to it shows");
}

#[test]
fn combine_lists_size_and_warns_before_combining() {
    let mut h = harness(1, |app| {
        app.set_option("combine-view", "list").unwrap();
        app.use_files(
            pdfcraft_ui_egui::FilePurpose::Combine,
            vec![
                ("one.pdf".into(), fixture(3)),
                ("locked.pdf".into(), protected("", "owner", -1 ^ 1024 ^ 8)),
                ("secret.pdf".into(), protected("pw", "owner", -1)),
            ],
        );
    });
    h.run_steps(3);
    h.get_by_label("File name");
    h.get_by_label("Warnings");
    h.get_by_label("Its security settings don't allow copying pages");
    h.get_by_label("Password-protected");
    h.get_by_label("2 files need attention");
    assert!(h.get_by_label("Combine").accesskit_node().is_disabled());
    // Removing them (Delete on the selection) lets Combine run.
    for _ in 0..2 {
        h.state_mut().select_combine_rows(&[1]);
        h.key_press(Key::Delete);
        h.run_steps(2);
    }
    assert_eq!(h.state().combine_draft.len(), 1);
    h.get_by_label_contains("1 file · 3 pages");
}

#[test]
fn a_bad_range_stops_combine_until_fixed() {
    let mut h = harness(1, |app| {
        app.use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("one.pdf".into(), fixture(3)), ("two.pdf".into(), fixture(2))]);
    });
    h.run_steps(3);
    h.get_by_label_contains("2 files · 5 pages");
    h.state_mut().combine_draft[0].range = "1-99".into();
    h.run_steps(2);
    h.get_by_label("1 file needs attention");
    assert!(h.get_by_label("Combine").accesskit_node().is_disabled());
    h.state_mut().combine_draft[0].range = "2-3".into();
    h.run_steps(2);
    h.get_by_label_contains("2 files · 4 pages");
    assert!(!h.get_by_label("Combine").accesskit_node().is_disabled());
}

#[test]
fn alt_arrows_move_the_selected_file() {
    let mut h = harness(1, |app| {
        app.use_files(
            pdfcraft_ui_egui::FilePurpose::Combine,
            vec![("a.pdf".into(), fixture(1)), ("b.pdf".into(), fixture(1)), ("c.pdf".into(), fixture(1))],
        );
    });
    h.run_steps(3);
    h.key_press(Key::ArrowDown);
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::ALT, Key::ArrowDown);
    h.run_steps(2);
    let names: Vec<_> = h.state().combine_draft.iter().map(|f| f.name.clone()).collect();
    assert_eq!(names, ["b.pdf", "a.pdf", "c.pdf"]);
    assert_eq!(h.state().combine_selection(), [1]);
}

fn combine_names(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    h.state().combine_draft.iter().map(|f| f.name.clone()).collect()
}

/// Files in the Combine tab's list view (the table: headings, page ranges, row links).
fn combine_of(names: &[(&str, usize)]) -> Harness<'static, PdfCraftApp> {
    let files: Vec<(String, Vec<u8>)> = names.iter().map(|(n, p)| (n.to_string(), fixture(*p))).collect();
    let mut h = harness(1, move |app| {
        app.set_option("combine-view", "list").unwrap();
        app.use_files(pdfcraft_ui_egui::FilePurpose::Combine, files);
    });
    h.run_steps(3);
    h
}

#[test]
fn column_headings_sort_the_list_and_flip_on_the_second_click() {
    let mut h = combine_of(&[("scan10.pdf", 1), ("Scan2.pdf", 3), ("a.pdf", 2)]);
    h.get_by_label("File name").click();
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["a.pdf", "Scan2.pdf", "scan10.pdf"], "case-insensitive, numbers by value");
    h.get_by_label("File name, sorted ascending").click();
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["scan10.pdf", "Scan2.pdf", "a.pdf"]);
    h.get_by_label("Pages").click();
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["scan10.pdf", "a.pdf", "Scan2.pdf"]);
    // Moving a file by hand ends the sorted order.
    h.state_mut().select_combine_rows(&[2]);
    h.get_by_label("Move up").click();
    h.run_steps(2);
    assert_eq!(h.state().combine_tab.sort, None);
    h.get_by_label("Pages");
}

#[test]
fn ctrl_and_shift_clicks_select_several_files_that_move_and_go_together() {
    let mut h = combine_of(&[("a.pdf", 1), ("b.pdf", 1), ("c.pdf", 1), ("d.pdf", 1), ("e.pdf", 1)]);
    h.get_by_label("b.pdf").click();
    h.run_steps(1);
    h.get_by_label("d.pdf").click_modifiers(Modifiers::SHIFT);
    h.run_steps(1);
    assert_eq!(h.state().combine_selection(), [1, 2, 3]);
    h.get_by_label("c.pdf").click_modifiers(Modifiers::COMMAND);
    h.run_steps(1);
    assert_eq!(h.state().combine_selection(), [1, 3], "Ctrl/⌘ toggles one file");
    h.get_by_label("2 selected");
    h.get_by_label("Move up").click();
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["b.pdf", "a.pdf", "d.pdf", "c.pdf", "e.pdf"]);
    h.get_by_label("Remove 2 files").click();
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["a.pdf", "c.pdf", "e.pdf"]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    assert_eq!(h.state().combine_selection(), [0, 1, 2]);
}

#[test]
fn undo_and_redo_restore_the_combine_list() {
    let mut h = combine_of(&[("a.pdf", 1), ("b.pdf", 1), ("c.pdf", 1)]);
    h.state_mut().select_combine_rows(&[0, 2]);
    h.key_press(Key::Delete);
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["b.pdf"]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["a.pdf", "b.pdf", "c.pdf"], "⌘Z brings the removed files back");
    assert_eq!(h.state().combine_selection(), [0, 2], "…selected as they were");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["b.pdf"]);
    // The toolbar's buttons do the same; undo also reverses a sort and the files' addition.
    h.get_by_label("Undo").click();
    h.run_steps(2);
    h.get_by_label("File name").click();
    h.run_steps(2);
    h.get_by_label("File name, sorted ascending").click();
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["c.pdf", "b.pdf", "a.pdf"]);
    h.get_by_label("Undo").click();
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["a.pdf", "b.pdf", "c.pdf"]);
    // A page range typed in is one step.
    h.state_mut().combine_draft[0].range.clear();
    let field = h.get_all_by_role(Role::TextInput).next().unwrap();
    field.focus();
    h.run_steps(1);
    h.get_all_by_role(Role::TextInput).next().unwrap().type_text("1");
    h.run_steps(2);
    assert_eq!(h.state().combine_draft[0].range, "1");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert_eq!(h.state().combine_draft[0].range, "");
    // The document's own history is untouched: it has nothing to undo.
    h.get_by_label("doc.pdf").click();
    h.run_steps(2);
    assert!(h.state().session.get(h.state().views[0].id).unwrap().can_undo().is_none());
}

#[test]
fn protected_files_that_can_be_combined_are_flagged() {
    let mut h = harness(1, |app| {
        app.set_option("combine-view", "list").unwrap();
        app.use_files(
            pdfcraft_ui_egui::FilePurpose::Combine,
            vec![("one.pdf".into(), fixture(1)), ("no-copy.pdf".into(), protected("", "owner", -1 ^ 16))],
        );
    });
    h.run_steps(3);
    let app = h.state();
    assert!(app.combine_draft[1].problem.is_none(), "copying text is withheld, assembling pages isn't");
    h.get_by_label("Protected: the combined file won't keep its security settings");
    assert!(!h.get_by_label("Combine").accesskit_node().is_disabled(), "a warning, not a blocker");
}

#[test]
fn the_whole_heading_sorts_and_dragging_it_moves_the_column() {
    let mut h = combine_of(&[("b.pdf", 1), ("a.pdf", 3)]);
    // A click in the empty right part of the heading cell, away from its text.
    let cell = h.get_by_label("File name").rect();
    let spot = egui::pos2(cell.right() - 30.0, cell.center().y);
    h.hover_at(spot);
    h.run_steps(1);
    h.drag_at(spot);
    h.run_steps(1);
    h.drop_at(spot);
    h.run_steps(2);
    assert_eq!(combine_names(&h), ["a.pdf", "b.pdf"]);
    // Drag Size before File name.
    let size = h.get_by_label("Size").rect().center();
    let name = h.get_by_label("File name, sorted ascending").rect();
    drag(&mut h, size, egui::pos2(name.left() + 10.0, name.center().y));
    use pdfcraft_ui_egui::SortKey::*;
    assert_eq!(h.state().combine_columns.order, [Size, Name, Pages, Modified, Warnings]);
    assert_eq!(combine_names(&h), ["a.pdf", "b.pdf"], "moving a column doesn't sort");
    let (size, name) = (h.get_by_label("Size").rect(), h.get_by_label("File name, sorted ascending").rect());
    assert!(size.left() < name.left(), "Size now shows first");
}

#[test]
fn columns_resize_and_the_layout_is_kept_in_the_settings() {
    let mut h = combine_of(&[("a.pdf", 1), ("b.pdf", 1)]);
    let size = h.get_by_label("Size").rect();
    // The separator after Size: drag it right by 60 points.
    let edge = egui::pos2(size.right() + 2.0, size.center().y);
    drag(&mut h, edge, edge + egui::vec2(60.0, 0.0));
    let widths = h.state().combine_columns.widths.expect("a resized column keeps its width");
    let size_index = pdfcraft_ui_egui::SortKey::Size as usize;
    assert!(widths[size_index] > 120.0, "{widths:?}");
    let saved = h.state().persist();
    let mut fresh = PdfCraftApp::new();
    fresh.set_option("language", "en").unwrap();
    fresh.restore(&saved);
    assert_eq!(fresh.combine_columns, h.state().combine_columns);
    // Malformed settings give the default layout.
    fresh.restore(r#"{"combine_columns":{"order":["size","size"],"widths":[1,2]}}"#);
    assert_eq!(fresh.combine_columns, Default::default());
}

#[test]
fn a_folder_adds_its_pdfs_in_order_and_optionally_its_subfolders() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-combine-folder-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    for (name, pages) in [("page10.pdf", 1), ("page2.PDF", 2), ("sub/inner.pdf", 3)] {
        std::fs::write(dir.join(name), fixture(pages)).unwrap();
    }
    std::fs::write(dir.join("notes.txt"), b"not a pdf").unwrap();
    let mut h = harness(1, |_| {});
    h.state_mut().execute("page.combine");
    h.run_steps(2);
    h.state_mut().pick_override = Some(vec![dir.to_string_lossy().into_owned()]);
    h.get_by_label("Add folder…").click();
    h.run_steps(4);
    assert_eq!(combine_names(&h), ["page2.PDF", "page10.pdf"], "PDFs only, numbers by value");
    // The toolbar's menu: the folder and its subfolders, as one undo step.
    h.get_by_label("More ways to add files").click();
    h.run_steps(2);
    h.get_by_label("Add folder and subfolders…").click();
    h.run_steps(4);
    assert_eq!(combine_names(&h), ["page2.PDF", "page10.pdf", "page2.PDF", "page10.pdf", "inner.pdf"]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert_eq!(combine_names(&h).len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

fn type_password(h: &mut Harness<'static, PdfCraftApp>, password: &str) {
    let field = h.get_by_role(Role::PasswordInput);
    field.focus();
    field.type_text(password);
    h.run_steps(1);
    h.key_press(Key::Enter);
    h.run_steps(3);
}

#[test]
fn a_password_protected_file_is_unlocked_with_its_password_and_combined() {
    let mut h = combine_of(&[("one.pdf", 1)]);
    h.state_mut().use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("secret.pdf".into(), protected("pw", "owner", -1))]);
    h.run_steps(3);
    h.get_by_label("Password-protected");
    assert!(h.get_by_label("Combine").accesskit_node().is_disabled());
    // The row's link (the toolbar's Unlock… comes first).
    h.get_all_by_label("Unlock…").last().unwrap().click();
    h.run_steps(3);
    h.get_by_label("Unlock secret.pdf");
    type_password(&mut h, "nope");
    h.get_by_label("Wrong password");
    assert!(h.state().combine_draft[1].lock.is_some());
    // The box stays open for another try.
    let field = h.get_by_role(Role::PasswordInput);
    field.focus();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    type_password(&mut h, "pw");
    let f = &h.state().combine_draft[1];
    assert!(f.lock.is_none() && f.problem.is_none());
    assert_eq!(f.pages, 2);
    h.get_by_label("Unlocked: the combined file won't be password-protected");
    // Never printed: debug output ends up in logs.
    assert!(!format!("{:?} {:?}", h.state().combine_draft, h.state().combine_tab).contains("pw\""));
    h.get_by_label("Combine").click();
    h.run_steps(3);
    assert_eq!(texts_of(h.state(), 1), ["Page 1", "Page 1", "Page 2"]);
}

#[test]
fn one_password_unlocks_several_selected_files_and_undo_locks_them_again() {
    let mut h = combine_of(&[("one.pdf", 1)]);
    h.state_mut().use_files(
        pdfcraft_ui_egui::FilePurpose::Combine,
        vec![
            ("a.pdf".into(), protected("same", "o1", -1)),
            ("b.pdf".into(), protected("same", "o2", -1)),
            ("c.pdf".into(), protected("other", "o3", -1)),
        ],
    );
    h.run_steps(3);
    h.state_mut().select_combine_rows(&[1, 2, 3]);
    h.run_steps(1);
    h.get_all_by_label("Unlock…").next().unwrap().click(); // the toolbar's (the rows' links come after it)
    h.run_steps(3);
    h.get_by_label("Unlock 3 files");
    type_password(&mut h, "same");
    let locked: Vec<bool> = h.state().combine_draft.iter().map(|f| f.lock.is_some()).collect();
    assert_eq!(locked, [false, false, false, true]);
    h.get_by_label_contains("Unlocked 2 of 3 files");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert!(h.state().combine_draft[1..].iter().all(|f| f.lock.is_some()), "undo locks them again");
}

#[test]
fn a_file_that_opens_but_forbids_combining_takes_the_permissions_password() {
    let mut h = combine_of(&[("one.pdf", 1)]);
    // Opens with "pw"; only the owner may assemble pages.
    h.state_mut().use_files(pdfcraft_ui_egui::FilePurpose::Combine, vec![("both.pdf".into(), protected("pw", "owner", -1 ^ 1024 ^ 8))]);
    h.run_steps(3);
    assert!(h.state_mut().combine_unlock_rows(&[1], "pw"));
    h.run_steps(2);
    assert_eq!(h.state().combine_draft[1].lock, Some(pdfcraft_ui_egui::CombineLock::Permissions));
    h.get_by_label_contains("Enter the permissions password");
    h.get_all_by_label("Unlock…").last().unwrap().click();
    h.run_steps(3);
    h.get_by_label("Enter the permissions password to allow combining");
    type_password(&mut h, "owner");
    assert!(h.state().combine_draft[1].lock.is_none());
    assert!(!h.get_by_label("Combine").accesskit_node().is_disabled());
}

#[test]
fn an_rc4_file_unlocks_with_its_owner_password_too() {
    // The engine reads RC4 files with their owner password; the inspector may not, and the
    // engine's answer is the one that counts.
    let mut h = combine_of(&[("one.pdf", 1)]);
    h.state_mut().use_files(
        pdfcraft_ui_egui::FilePurpose::Combine,
        vec![("rc4.pdf".into(), protected_with(pdfcraft_cos::Algorithm::Rc4_128, "user", "boss", -1))],
    );
    h.run_steps(3);
    assert!(h.state_mut().combine_unlock_rows(&[1], "boss"));
    let f = &h.state().combine_draft[1];
    assert!(f.lock.is_none());
    assert_eq!(f.pages, 2);
}

#[test]
fn open_documents_can_be_added_to_combine() {
    let mut h = harness(2, |_| {});
    h.state_mut().execute("page.combine");
    h.run_steps(3);
    h.get_by_label("Add open documents").click();
    h.run_steps(2);
    // The menu's entry, after the tab of the same name.
    h.get_all_by_label("doc.pdf").last().unwrap().click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.combine_draft.len(), 1);
    assert_eq!(app.combine_draft[0].pages, 2);
    assert!(app.combine_showing());
}

#[test]
fn extract_button_copies_selected_pages_to_a_new_tab() {
    let mut h = organize(3);
    h.get_by_label("Page 2").click();
    h.run_steps(1);
    h.get_by_label("Page 3").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    h.get_by_label("Extract pages to a new document").click();
    h.run_steps(3);
    h.get_by_label("2 pages selected.");
    h.get_all_by_label("Extract").last().unwrap().click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.views.len(), 2);
    assert_eq!(texts_of(app, 1), ["Page 2", "Page 3"]);
    assert_eq!(texts_of(app, 0).len(), 3, "the original is unchanged");
    assert!(!app.session.get(app.views[0].id).unwrap().dirty);
}

#[test]
fn inserting_a_file_goes_after_the_selection_and_undoes() {
    let mut h = organize(2);
    h.get_by_label("Page 1").click();
    h.run_steps(2);
    h.state_mut().use_files(pdfcraft_ui_egui::FilePurpose::InsertPages, vec![("extra.pdf".into(), fixture(2))]);
    h.run_steps(3);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 1", "Page 2", "Page 2"]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 2"]);
}

#[test]
fn plus_between_pages_inserts_picked_files_there_in_order() {
    let dir = temp_path("insert-gap");
    std::fs::create_dir_all(&dir).unwrap();
    let (extra, note) = (dir.join("extra.pdf"), dir.join("note.txt"));
    std::fs::write(&extra, fixture(2)).unwrap();
    std::fs::write(&note, "a note").unwrap();
    let mut h = organize(3);
    // Page 3 is selected, yet the files go where the "+" is: between pages 1 and 2.
    h.get_by_label("Page 3").click();
    h.run_steps(2);
    h.state_mut().pick_override = Some(vec![extra.to_string_lossy().into_owned(), note.to_string_lossy().into_owned()]);
    h.get_by_label("Insert a file before page 2").click();
    h.run_steps(4);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 1", "Page 2", "a note", "Page 2", "Page 3"]);
    assert_eq!(picked(&h), [1, 2, 3], "the inserted pages are selected");
    // Each file is one undo step.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 2", "Page 3"]);
    // Both ends.
    h.state_mut().pick_override = Some(vec![note.to_string_lossy().into_owned()]);
    h.get_by_label("Insert a file before page 1").click();
    h.run_steps(4);
    h.get_by_label("Insert a file at the end").click();
    h.run_steps(4);
    assert_eq!(texts_of(h.state(), 0), ["a note", "Page 1", "Page 2", "Page 3", "a note"]);
    // The toolbar button still inserts after the selection, not at the last "+" used.
    h.get_by_label("Page 2").click();
    h.run_steps(2);
    h.get_by_label("Insert pages from a file…").click();
    h.run_steps(4);
    assert_eq!(texts_of(h.state(), 0), ["a note", "Page 1", "a note", "Page 2", "Page 3", "a note"]);
    // A file that can't be converted changes nothing.
    let before = texts_of(h.state(), 0);
    h.state_mut().use_files(pdfcraft_ui_egui::FilePurpose::InsertPages, vec![("report.docx".into(), b"PK\x03\x04".to_vec())]);
    h.run_steps(2);
    assert_eq!(texts_of(h.state(), 0), before);
}

/// A file dropped on the window, as the windowing layer hands it over.
#[derive(Debug)]
struct Dropped(&'static str, Vec<u8>);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        std::path::Path::new(self.0)
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        Ok(self.1.clone())
    }
}

fn drop_files(h: &mut Harness<'static, PdfCraftApp>, files: Vec<Dropped>) {
    for f in files {
        h.input_mut().dropped_files.push(std::sync::Arc::new(f));
    }
}

#[test]
fn files_dropped_between_pages_are_inserted_there() {
    let mut h = organize(3);
    // Over the gap between pages 1 and 2.
    let gap = h.get_by_label("Insert a file before page 2").rect().center();
    h.hover_at(gap + egui::vec2(0.0, 40.0));
    h.run_steps(2);
    drop_files(&mut h, vec![Dropped("extra.pdf", fixture(2)), Dropped("note.txt", b"a note".to_vec())]);
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 1, "dropped files are not opened as documents");
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 1", "Page 2", "a note", "Page 2", "Page 3"]);
    assert_eq!(picked(&h), [1, 2, 3], "the inserted pages are selected");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 2", "Page 3"]);
    // Right of the last page: the end.
    let last = h.get_by_label("Page 3").rect();
    h.hover_at(last.center() + egui::vec2(last.width() * 0.4, 0.0));
    h.run_steps(2);
    drop_files(&mut h, vec![Dropped("note.txt", b"a note".to_vec())]);
    h.run_steps(4);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 2", "Page 3", "a note"]);
}

#[test]
fn files_dropped_before_the_pointer_is_known_wait_for_it() {
    // Some platforms don't report the pointer while files are dragged over the window.
    let mut h = organize(2);
    drop_files(&mut h, vec![Dropped("note.txt", b"a note".to_vec())]);
    h.run_steps(1);
    assert_eq!(texts_of(h.state(), 0).len(), 2, "not placed yet");
    let gap = h.get_by_label("Insert a file before page 2").rect().center();
    h.hover_at(gap + egui::vec2(0.0, 40.0));
    h.run_steps(3);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "a note", "Page 2"]);
}

#[test]
fn files_dropped_outside_the_page_grid_still_open_as_documents() {
    let mut h = harness(2, |_| {});
    drop_files(&mut h, vec![Dropped("other.pdf", fixture(1))]);
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 2);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 2"]);
}

fn grid_zoom(h: &Harness<'static, PdfCraftApp>) -> f32 {
    h.state().views[0].grid_zoom()
}

#[test]
fn page_grid_zooms_with_its_buttons_keys_and_pinch() {
    let mut h = organize(12);
    let width = |h: &Harness<'static, PdfCraftApp>| h.get_by_label("Page 2").rect().left() - h.get_by_label("Page 1").rect().left();
    let usual = width(&h);
    h.get_by_label("Larger pages").click();
    h.run_steps(3);
    assert_eq!(grid_zoom(&h), 1.25);
    assert!(width(&h) > usual + 20.0, "the pages are drawn larger");
    h.get_by_label("Reset page size").click();
    h.run_steps(3);
    assert_eq!((grid_zoom(&h), width(&h)), (1.0, usual));
    h.get_by_label("Smaller pages").click();
    h.run_steps(3);
    assert_eq!(grid_zoom(&h), 0.8);
    assert!(width(&h) < usual - 20.0);
    // Keys.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num0);
    h.run_steps(2);
    assert_eq!(grid_zoom(&h), 1.0);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Equals);
    h.run_steps(2);
    assert_eq!(grid_zoom(&h), 1.25);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Minus);
    h.run_steps(2);
    assert_eq!(grid_zoom(&h), 1.0);
    // Pinch (or Ctrl/⌘ + wheel) over the grid; elsewhere it leaves the grid alone.
    let page = h.get_by_label("Page 1").rect().center();
    h.hover_at(page);
    h.run_steps(1);
    h.event(egui::Event::Zoom(1.5));
    h.run_steps(2);
    assert_eq!(grid_zoom(&h), 1.5);
    h.hover_at(h.get_by_label("Larger pages").rect().center());
    h.run_steps(1);
    h.event(egui::Event::Zoom(1.5));
    h.run_steps(2);
    assert_eq!(grid_zoom(&h), 1.5, "the pointer was on the toolbar");
    // It stops at its limits, where the buttons switch off.
    for _ in 0..12 {
        h.key_press_modifiers(Modifiers::COMMAND, Key::Equals);
        h.run_steps(1);
    }
    assert_eq!(grid_zoom(&h), 3.0);
    h.get_by_label("Larger pages").click();
    h.run_steps(2);
    assert_eq!(grid_zoom(&h), 3.0);
    for _ in 0..20 {
        h.key_press_modifiers(Modifiers::COMMAND, Key::Minus);
        h.run_steps(1);
    }
    assert_eq!(grid_zoom(&h), 0.5);
    assert!(!dirty(&h), "zooming is not an edit");
}

#[test]
fn zoomed_pages_in_view_are_rendered_at_the_size_drawn() {
    // A long document: sharpness must not depend on how many pages there are.
    let mut h = harness(400, |app| {
        app.set_option("organize", "on").unwrap();
        app.set_option("grid-zoom", "300").unwrap();
    });
    let drawn = 146.0 * 3.0 * h.ctx.pixels_per_point();
    let sharp = |h: &Harness<'static, PdfCraftApp>, page| h.state().views[0].grid_page_pixels(page);
    for _ in 0..200 {
        if sharp(&h, 0).is_some_and(|w| w as f32 >= drawn * 0.9) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        h.run_steps(1);
    }
    let width = sharp(&h, 0).expect("the first page has a sharp render");
    assert!((drawn * 0.9..=drawn * 1.6).contains(&(width as f32)), "{width} for {drawn}");
    assert!(sharp(&h, 399).is_none(), "pages out of view have none");
    // Zoomed out again, thumbnails are enough and the sharp renders are dropped.
    h.state_mut().set_option("grid-zoom", "50").unwrap();
    h.run_steps(3);
    assert!(sharp(&h, 0).is_none(), "page 0 still has a sharp render: {:?}", sharp(&h, 0));
}

#[test]
fn a_zoomed_page_grid_still_inserts_and_moves_at_the_right_gap() {
    let dir = temp_path("zoomed-grid");
    std::fs::create_dir_all(&dir).unwrap();
    let note = dir.join("note.txt");
    std::fs::write(&note, "a note").unwrap();
    let mut h = harness(3, |app| {
        app.set_option("organize", "on").unwrap();
        app.set_option("grid-zoom", "200").unwrap();
        assert!(
            app.set_option("grid-zoom", "20").is_err() && app.set_option("grid-zoom", "NaN").is_err() && app.set_option("grid-zoom", "big").is_err()
        );
    });
    assert_eq!(grid_zoom(&h), 2.0);
    h.state_mut().pick_override = Some(vec![note.to_string_lossy().into_owned()]);
    h.get_by_label("Insert a file before page 2").click();
    h.run_steps(4);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "a note", "Page 2", "Page 3"]);
    // Files dropped over a gap go there too.
    let gap = h.get_by_label("Insert a file before page 2").rect().center();
    h.hover_at(gap + egui::vec2(0.0, 60.0));
    h.run_steps(2);
    drop_files(&mut h, vec![Dropped("more.txt", b"more".to_vec())]);
    h.run_steps(4);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "more", "a note", "Page 2", "Page 3"]);
}

#[test]
fn save_pages_writes_what_the_grid_shows() {
    let path = temp_path("grid-save.pdf");
    let mut h = organize(3);
    h.get_by_label("Page 2").click();
    h.run_steps(2);
    h.key_press(Key::Delete);
    h.run_steps(3);
    h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
    h.get_by_label("Save pages").click();
    h.run_steps(4);
    assert!(!dirty(&h));
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.open_bytes("saved.pdf", None, std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(texts_of(&app, 0), ["Page 1", "Page 3"]);
}

#[test]
fn split_dialog_writes_one_file_per_part() {
    let dir = temp_path("split-count");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.to_string_lossy().into_owned();
    let mut h = organize(3);
    h.state_mut().export_dir_override = Some(d);
    h.get_by_label("Split into files…").click();
    h.run_steps(3);
    h.get_by_label_contains("Creates 3 files from 3 pages");
    h.get_by_label("Split").click();
    h.run_steps(3);
    let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["doc (page 1).pdf", "doc (page 2).pdf", "doc (page 3).pdf"]);
    for name in names {
        let info = pdfcraft_render::inspect(std::sync::Arc::new(std::fs::read(dir.join(name)).unwrap()), None).unwrap();
        assert_eq!(info.pages.len(), 1);
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn split_at_bookmarks_with_equal_titles_keeps_every_part() {
    let dir = temp_path("split-dup-titles");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = organize(3);
    h.state_mut().export_dir_override = Some(dir.to_string_lossy().into_owned());
    for (index, title) in [(0, "Same"), (1, "Same"), (2, "same")] {
        h.state_mut().apply_edit(pdfcraft_engine::Edit::AddBookmark { parent: vec![], index, title: title.into(), page: index });
    }
    h.state_mut().split_draft.mode = pdfcraft_ui_egui::SplitMode::Bookmarks;
    h.state_mut().run_command("page.split");
    h.run_steps(3);
    h.get_by_label_contains("Creates 3 files from 3 pages");
    h.get_by_label("Split").click();
    h.run_steps(3);
    let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["doc - Same (2).pdf", "doc - Same.pdf", "doc - same (3).pdf"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn split_before_selected_pages() {
    let dir = temp_path("split-sel");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = organize(4);
    h.state_mut().export_dir_override = Some(dir.to_string_lossy().into_owned());
    h.state_mut().views[0].select_pages(&[2]);
    h.state_mut().split_draft.mode = pdfcraft_ui_egui::SplitMode::Selection;
    h.state_mut().run_command("page.split");
    h.run_steps(3);
    h.get_by_label_contains("Creates 2 files from 4 pages");
    h.get_by_label("Split").click();
    h.run_steps(3);
    let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["doc (pages 1-2).pdf", "doc (pages 3-4).pdf"]);
    let _ = std::fs::remove_dir_all(dir);
}

// ── Encrypted documents ───────────────────────────────────────────────────────────────────────

fn protected(user: &str, owner: &str, permissions: i32) -> Vec<u8> {
    protected_with(pdfcraft_cos::Algorithm::Aes256, user, owner, permissions)
}

fn protected_with(algorithm: pdfcraft_cos::Algorithm, user: &str, owner: &str, permissions: i32) -> Vec<u8> {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(fixture(2))).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm,
        user_password: user,
        owner_password: owner,
        permissions,
        encrypt_metadata: true,
        seed: [4; 32],
    })
    .unwrap();
    pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()
}

#[test]
fn password_prompt_opens_and_security_tab_reports_the_details() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("secret.pdf", None, protected("pw", "owner", -1)).unwrap();
        app
    });
    h.run_steps(4);
    h.get_by_label_contains("is protected");
    let field = h.get_by_role(Role::PasswordInput);
    field.focus();
    field.type_text("pw");
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 1);
    h.state_mut().set_option("dialog", "properties").unwrap();
    h.run_steps(2);
    h.get_by_label("Security").click();
    h.run_steps(3);
    h.get_by_label("AES, 256-bit");
    h.get_by_label("User password");
}

/// #785: the notice's message used to be laid out before its buttons, so in a narrow document
/// area (a side panel open) it ran under "Security settings" and on into the panel, and it sat
/// off the buttons' centre line.
#[test]
fn the_security_notice_wraps_beside_its_buttons_and_lines_up_with_them() {
    // Wide: one line. Narrower: wrapped. Narrowest: cut short (the full text is on hover).
    for (width, wraps) in [(1700.0, false), (1250.0, true), (900.0, false)] {
        let mut h = Harness::builder().with_size(egui::vec2(width, 800.0)).build_eframe(|_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("locked.pdf", None, protected("", "owner", 0b0100)).unwrap();
            app.set_option("panel", "bookmarks").unwrap();
            app
        });
        h.run_steps(4);
        let msg = h.get_by_label_contains("This document is secured").rect();
        let button = h.get_by_label("Security settings").rect();
        let dismiss = h.get_by_label("Dismiss").rect();
        assert!(msg.right() <= button.left() + 0.5, "{width}: the message stops before the buttons: {msg:?} vs {button:?}");
        assert!(button.right() <= dismiss.left() + 0.5, "{width}: {button:?} vs {dismiss:?}");
        assert!((button.center().y - dismiss.center().y).abs() <= 0.5, "{width}: the buttons share a centre line");
        assert!(msg.top() >= button.top(), "{width}: the message stays inside the bar: {msg:?} vs {button:?}");
        let lines = (msg.height() / 14.0).floor().max(1.0);
        let first_line = msg.top() + msg.height() / lines / 2.0;
        assert!((first_line - button.center().y).abs() <= 1.5, "{width}: first line centred on the buttons: {msg:?} vs {button:?}");
        assert_eq!(msg.height() > button.height(), wraps, "{width}: {msg:?}");
    }
}

#[test]
fn restricted_documents_show_a_notice_and_block_page_changes() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("locked.pdf", None, protected("", "owner", 0b0100)).unwrap(); // opens without a password
        app.set_option("organize", "on").unwrap();
        app
    });
    h.run_steps(4);
    h.get_by_label_contains("This document is secured");
    h.get_by_label("Page 1").click();
    h.run_steps(1);
    h.get_by_label("Delete pages (Delete)").click();
    h.key_press(Key::Delete);
    h.run_steps(3);
    assert_eq!(texts_of(h.state(), 0).len(), 2, "page changes are blocked");
    assert_eq!(h.query_all_by_label_contains("Insert a file").count(), 0, "nothing to insert into");
    // Looking closer is not a page change.
    h.get_by_label("Larger pages").click();
    h.run_steps(2);
    assert_eq!(grid_zoom(&h), 1.25);
    assert!(!dirty(&h));
    h.get_by_label("Security settings").click();
    h.run_steps(3);
    assert!(h.query_all_by_label("Not allowed").count() >= 4);
}

#[test]
fn replace_pages_dialog_swaps_page_content() {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.open_bytes("doc.pdf", None, fixture(3)).unwrap();
    app.views[0].select_pages(&[1]);
    app.start_replace("other.pdf".into(), fixture(5));
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| app);
    h.run_steps(3);
    egui_kittest::kittest::Queryable::get_by_label(&h, "Replace Pages");
    h.state_mut().replace_draft.as_mut().unwrap().src_from = 5;
    h.run_steps(1);
    egui_kittest::kittest::Queryable::get_by_label(&h, "OK").click();
    h.run_steps(3);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.info.pages.len(), 3);
    assert_eq!(doc.can_undo(), Some("Replace page"));
}

#[test]
fn extract_options_and_rotate_pages_dialog() {
    let dir = temp_path("extract-sep");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = organize(4);
    h.state_mut().export_dir_override = Some(dir.to_string_lossy().into_owned());
    h.state_mut().views[0].select_pages(&[1, 2]);
    h.state_mut().run_command("page.extract");
    h.run_steps(2);
    h.state_mut().extract_draft = pdfcraft_ui_egui::ExtractDraft { separate: true, delete: true, ..Default::default() };
    h.get_all_by_label("Extract").last().unwrap().click();
    h.run_steps(3);
    let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["doc (page 2).pdf", "doc (page 3).pdf"]);
    assert_eq!(texts_of(h.state(), 0), ["Page 1", "Page 4"], "deleted after extracting");
    let _ = std::fs::remove_dir_all(dir);
    // Rotate Pages: odd page numbers only.
    h.state_mut().run_command("page.rotate_dialog");
    h.run_steps(2);
    h.state_mut().rotate_draft.which = 0;
    h.state_mut().rotate_draft.parity = pdfcraft_engine::PageParity::Odd;
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let d = s.session.get(s.views[0].id).unwrap();
    assert_eq!(d.info.pages.iter().map(|p| p.rotation).collect::<Vec<_>>(), [90, 0]);
}

#[test]
fn dragging_thumbnails_reorders_pages() {
    let mut h = organize(4);
    let before = page_texts(h.state());
    let grab = |h: &Harness<'static, PdfCraftApp>, label: &str| h.get_by_label(label).rect();
    let (from, to) = (grab(&h, "Page 1").center(), grab(&h, "Page 3").right_center() - egui::vec2(10.0, 0.0));
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=5 {
        h.hover_at(from + (to - from) * (k as f32 / 5.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(4);
    // Page 1 now sits after page 3.
    let after = page_texts(h.state());
    assert_eq!(after, vec![before[1].clone(), before[2].clone(), before[0].clone(), before[3].clone()], "{after:?}");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Move page"));
    let sel: Vec<usize> = h.state().views[0].selected.iter().copied().collect();
    assert_eq!(sel, vec![2], "the moved page stays selected");
}

#[test]
fn copying_cutting_and_pasting_pages() {
    let mut h = organize(4);
    let before = page_texts(h.state());
    h.state_mut().views[0].select_pages(&[0]);
    assert!(h.state_mut().execute("page.copy"));
    h.state_mut().views[0].select_pages(&[2]);
    assert!(h.state_mut().execute("page.paste"));
    h.run_steps(2);
    let after = page_texts(h.state());
    assert_eq!(after, vec![before[0].clone(), before[1].clone(), before[2].clone(), before[0].clone(), before[3].clone()], "pasted after page 3");
    let sel: Vec<usize> = h.state().views[0].selected.iter().copied().collect();
    assert_eq!(sel, vec![3], "the pasted page is selected");
    // Cut page 2 and paste it at the end.
    h.state_mut().views[0].select_pages(&[1]);
    assert!(h.state_mut().execute("page.cut"));
    h.state_mut().views[0].select_pages(&[3]);
    assert!(h.state_mut().execute("page.paste"));
    let after = page_texts(h.state());
    assert_eq!(after, vec![before[0].clone(), before[2].clone(), before[0].clone(), before[3].clone(), before[1].clone()]);
    // Every page can be copied but not cut.
    h.state_mut().views[0].select_pages(&[0, 1, 2, 3, 4]);
    h.state_mut().execute("page.cut");
    assert_eq!(page_texts(h.state()).len(), 5);
}

fn source_font_fixture() -> Vec<u8> {
    let body = "BT /F1 18 Tf 20 220 Td (Serif) Tj ET BT /F2 18 Tf 20 120 Td (Mono) Tj ET";
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 300 300] >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Times-BoldItalic >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 7 0 R /Resources << /Font << /F1 3 0 R /F2 5 0 R >> >> >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier-Oblique >>".into(),
        "<< /Producer (PdfCraft) >>".into(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
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
    out
}

fn open_source_font_fixture() -> Harness<'static, PdfCraftApp> {
    Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("fonts.pdf", None, source_font_fixture()).expect("font fixture opens");
        app
    })
}

#[test]
fn clicking_existing_text_selects_its_source_font_style() {
    let mut h = open_source_font_fixture();
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let page = h.state().views[0].page_screen_rect(0).expect("on screen");
    let click = |x: f32, y: f32| egui::pos2(page.left() + x / 300.0 * page.width(), page.top() + (300.0 - y) / 300.0 * page.height());

    let serif = click(25.0, 228.0);
    h.hover_at(serif);
    h.run_steps(1);
    h.drag_at(serif);
    h.run_steps(1);
    h.drop_at(serif);
    h.run_steps(3);
    let ed = h.state().views[0].line_editor.clone().expect("serif editor opens");
    assert_eq!(ed.look.family, pdfcraft_engine::FontFamily::Times);
    assert!(ed.look.bold && ed.look.italic, "source style: {:?}", ed.look);

    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    let mono = click(25.0, 128.0);
    h.hover_at(mono);
    h.run_steps(1);
    h.drag_at(mono);
    h.run_steps(1);
    h.drop_at(mono);
    h.run_steps(3);
    let ed = h.state().views[0].line_editor.clone().expect("mono editor opens");
    assert_eq!(ed.look.family, pdfcraft_engine::FontFamily::Courier);
    assert!(!ed.look.bold && ed.look.italic, "source style: {:?}", ed.look);
}

#[test]
fn editing_existing_text_in_place() {
    let mut h = harness(1, |_| {});
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    // The line "Page 1" sits at (20, 150) on a 200 × 300 page, 24 pt.
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let at = egui::pos2(r.left() + 40.0 / 200.0 * r.width(), r.top() + (300.0 - 158.0) / 300.0 * r.height());
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(3);
    let ed = h.state().views[0].line_editor.clone().expect("the line opens for editing");
    assert_eq!((ed.page, ed.block, ed.text.as_str()), (0, 0, "Page 1"));
    // A single-line paragraph's box may grow to the page's edge as the text does.
    assert!(ed.growth().is_some(), "the editor box may grow");
    h.state_mut().views[0].line_editor.as_mut().unwrap().text = "Chapter One".into();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Enter);
    h.run_steps(4);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.can_undo(), Some("Edit text"));
    assert_eq!(doc.text_lines(0)[0].text, "Chapter One");
    assert_eq!(texts_of(s, 0), ["Chapter One"], "the page shows it");
}

/// One page whose only line is drawn twice at the same spot (fake bold, as many generated
/// documents do).
fn double_drawn() -> Vec<u8> {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 300] >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".into(),
        format!(
            "<< /Length {} >>\nstream\nBT /F1 24 Tf 20 150 Td (Page 1) Tj ET BT /F1 24 Tf 20 150 Td (Page 1) Tj ET\nendstream",
            "BT /F1 24 Tf 20 150 Td (Page 1) Tj ET BT /F1 24 Tf 20 150 Td (Page 1) Tj ET".len()
        ),
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
    out
}

#[test]
fn editing_a_double_drawn_line_replaces_every_copy() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("bold.pdf", None, double_drawn()).expect("opens");
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let at = egui::pos2(r.left() + 40.0 / 200.0 * r.width(), r.top() + (300.0 - 158.0) / 300.0 * r.height());
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(3);
    let ed = h.state().views[0].line_editor.clone().expect("the line opens for editing");
    assert_eq!(ed.text, "Page 1");
    h.state_mut().views[0].line_editor.as_mut().unwrap().text = "Replaced".into();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Enter);
    h.run_steps(4);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().text_lines(0).len(), 1, "one line");
    assert_eq!(texts_of(s, 0), ["Replaced"], "no copy of the old text shows under it");
}

#[test]
fn editing_existing_images_on_the_page() {
    // A page made from a 40 × 20 image (at 72 dpi: a 40 × 20 pt page filled by it).
    let mut png = Vec::new();
    image::RgbImage::from_pixel(80, 40, image::Rgb([200, 40, 40])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    check_existing_artwork("picture.png", png);
}

#[test]
fn editing_existing_form_artwork_on_the_page() {
    use pdfcraft_cos::{Dict, Document, Object, SaveOptions, Stream, write_incremental};
    let mut doc = Document::open(std::sync::Arc::new(fixture(1))).unwrap();
    let page = pdfcraft_model::pages(&doc)[0].clone();
    let mut form = Dict::new();
    form.set(b"Subtype".to_vec(), Object::name("Form"));
    form.set(b"BBox".to_vec(), Object::Array([0, 0, 200, 300].map(Object::Int).to_vec()));
    let artwork = doc.add(Object::Stream(Stream::flate(form, b"0.2 0.5 0.9 rg 20 20 160 260 re f")));
    let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), b"/Figure Do")));
    doc.update_dict(page.obj, |d| {
        d.set(b"Contents".to_vec(), Object::Ref(contents));
        let mut xo = Dict::new();
        xo.set(b"Figure".to_vec(), Object::Ref(artwork));
        let mut resources = Dict::new();
        resources.set(b"XObject".to_vec(), Object::Dict(xo));
        d.set(b"Resources".to_vec(), Object::Dict(resources));
    })
    .unwrap();
    check_existing_artwork("figure.pdf", write_incremental(&doc, &SaveOptions::default()).unwrap());
}

fn check_existing_artwork(name: &'static str, bytes: Vec<u8>) {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes(name, None, bytes.clone()).expect("opens");
        app
    });
    h.run_steps(4);
    let id = h.state().views[0].id;
    let before = h.state().session.get(id).unwrap().page_images(0)[0].rect;
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    // Select it, then drag it a quarter of the page to the right.
    let c = r.center();
    h.hover_at(c);
    h.run_steps(1);
    h.drag_at(c);
    h.run_steps(1);
    h.drop_at(c);
    h.run_steps(2);
    assert!(h.state().views[0].image_selection.is_some(), "selected");
    let to = c + egui::vec2(r.width() / 4.0, 0.0);
    h.hover_at(c);
    h.run_steps(1);
    h.drag_at(c);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(c + (to - c) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(4);
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.can_undo(), Some("Move image"));
    let moved = doc.page_images(0)[0].rect;
    let dx = moved[0] - before[0];
    assert!((dx - (before[2] - before[0]) / 4.0).abs() < 1.5, "moved {dx} pt: {before:?} → {moved:?}");
    // Delete with the Delete key.
    h.key_press(egui::Key::Delete);
    h.run_steps(4);
    assert!(h.state().session.get(id).unwrap().page_images(0).is_empty());
}

#[test]
fn the_format_panel_restyles_the_paragraph_being_edited() {
    let mut h = harness(1, |_| {});
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let at = egui::pos2(r.left() + 40.0 / 200.0 * r.width(), r.top() + (300.0 - 158.0) / 300.0 * r.height());
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(3);
    assert!(h.state().views[0].line_editor.is_some());
    // The Format text panel shows the paragraph's look; making it bold applies at once.
    h.get_by_label("FORMAT TEXT");
    h.get_by_label("B").click();
    h.run_steps(4);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.can_undo(), Some("Edit text"));
    assert_eq!(doc.text_blocks(0)[0].base_font, "Helvetica-Bold");
    assert!(s.views[0].line_editor.is_some(), "still editing");
    // Underline: another step, the text keeps its bold.
    h.get_by_label("Underline").click();
    h.run_steps(4);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.text_blocks(0)[0].base_font, "Helvetica-Bold");
    assert!(s.views[0].line_editor.as_ref().is_some_and(|e| e.extras.underline));
}

/// Drag with the pointer from `from` to `to` in a few steps.
fn drag(h: &mut Harness<'static, PdfCraftApp>, from: egui::Pos2, to: egui::Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=5 {
        h.hover_at(from + (to - from) * (k as f32 / 5.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(4);
}

#[test]
fn dragging_a_paragraph_moves_it_and_its_edge_rewraps_it() {
    let mut h = harness(1, |_| {});
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let block = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().text_blocks(0)[0].clone()
    };
    // User space (200 × 300 page) → screen.
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = r.width() / 200.0;
    let screen = |x: f64, y: f64| egui::pos2(r.left() + x as f32 * k, r.top() + (300.0 - y as f32) * k);
    // Move "Page 1" 30 pt right and 50 pt up.
    let before = block(&h);
    let mid = screen((before.rect[0] + before.rect[2]) / 2.0, (before.rect[1] + before.rect[3]) / 2.0);
    drag(&mut h, mid, mid + egui::vec2(30.0 * k, -50.0 * k));
    assert!(h.state().views[0].line_editor.is_none(), "dragging moves the box; it doesn't open it for typing");
    let moved = block(&h);
    assert_eq!(moved.text, "Page 1");
    let near = |a: f64, b: f64| (a - b).abs() < 1.5;
    assert!(near(moved.rect[0], before.rect[0] + 30.0) && near(moved.rect[1], before.rect[1] + 50.0), "{:?} → {:?}", before.rect, moved.rect);
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Edit text"));
    // Drag the handle on its right edge in to about 45 pt wide: "Page" and "1" rewrap onto two lines.
    let edge = screen(moved.rect[2], (moved.rect[1] + moved.rect[3]) / 2.0) + egui::vec2(2.0, 0.0);
    let narrower = (moved.rect[2] - moved.rect[0] - 45.0) as f32 * k;
    drag(&mut h, edge, edge - egui::vec2(narrower, 0.0));
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    let lines: Vec<String> = doc.text_lines(0).iter().map(|l| l.text.clone()).collect();
    assert_eq!(lines, ["Page", "1"], "rewrapped to the narrower box");
    assert!(near(doc.text_lines(0)[0].rect[0], moved.rect[0]), "it keeps its place");
    assert!(h.state().views[0].line_editor.is_none());
}

/// A quick flick: press at `from`, and the next frame already finds the pointer at `to`.
fn flick(h: &mut Harness<'static, PdfCraftApp>, from: egui::Pos2, to: egui::Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    h.hover_at(to);
    h.run_steps(1);
    h.drop_at(to);
    h.run_steps(4);
}

#[test]
fn a_quick_flick_on_either_edge_rewraps_the_paragraph() {
    let mut h = harness(1, |_| {});
    assert!(h.state_mut().execute("edit.edit_text"));
    h.run_steps(2);
    let lines = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().text_lines(0)
    };
    let block = |h: &Harness<'static, PdfCraftApp>| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().text_blocks(0)[0].clone()
    };
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = r.width() / 200.0;
    let screen = |x: f64, y: f64| egui::pos2(r.left() + x as f32 * k, r.top() + (300.0 - y as f32) * k);
    let near = |a: f64, b: f64| (a - b).abs() < 1.5;
    // Grab edges near the box's top: a rewrapped box reaches the window's bottom, where the
    // tool's toast sits over the page and takes the press.
    let grip_y = |b: &pdfcraft_engine::TextBlock| b.rect[3] - 6.0;
    // Narrow "Page 1" from its right edge to about 45 pt: it rewraps onto two lines.
    let b = block(&h);
    let edge = screen(b.rect[2], grip_y(&b)) + egui::vec2(2.0, 0.0);
    flick(&mut h, edge, edge - egui::vec2((b.rect[2] - b.rect[0] - 45.0) as f32 * k, 0.0));
    let text: Vec<String> = lines(&h).iter().map(|l| l.text.clone()).collect();
    assert_eq!(text, ["Page", "1"], "narrowed from the right edge");
    // Flick the right edge outward, past the box and its handle: back onto one line.
    let b = block(&h);
    let edge = screen(b.rect[2], grip_y(&b)) + egui::vec2(2.0, 0.0);
    flick(&mut h, edge, edge + egui::vec2(80.0 * k, 0.0));
    let text: Vec<String> = lines(&h).iter().map(|l| l.text.clone()).collect();
    assert_eq!(text, ["Page 1"], "widened by a fast drag that left the handle at once");
    // Flick the left edge inward: it rewraps and its left side follows the pointer.
    let b = block(&h);
    let edge = screen(b.rect[0], grip_y(&b)) - egui::vec2(2.0, 0.0);
    let inward = (b.rect[2] - b.rect[0] - 45.0) as f32;
    flick(&mut h, edge, edge + egui::vec2(inward * k, 0.0));
    let after = lines(&h);
    let text: Vec<String> = after.iter().map(|l| l.text.clone()).collect();
    assert_eq!(text, ["Page", "1"], "narrowed from the left edge");
    assert!(near(after[0].rect[0], b.rect[0] + inward as f64), "{:?} → {:?}", b.rect, after[0].rect);
    assert!(h.state().views[0].line_editor.is_none());
}

/// The Pages panel, in a window tall enough to show every thumbnail of a short fixture.
fn pages_panel(pages: usize) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, fixture(pages)).expect("fixture opens");
        app.set_option("panel", "pages").unwrap();
        app
    });
    h.run_steps(4);
    h
}

fn picked(h: &Harness<'static, PdfCraftApp>) -> Vec<usize> {
    h.state().views[0].selected.iter().copied().collect()
}

#[test]
fn pages_panel_command_click_picks_pages_without_moving_the_document() {
    let mut h = pages_panel(5);
    // The first ⌘-click on another page keeps the current page (1) selected too.
    h.get_by_label("Page 3").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(picked(&h), [0, 2]);
    assert_eq!(h.state().views[0].current, 0, "picking pages does not turn the page");
    h.get_by_label_contains("2 pages selected");
    h.get_by_label("Page 5").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(picked(&h), [0, 2, 4]);
    // ⌘-click again takes a page back out.
    h.get_by_label("Page 3").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(picked(&h), [0, 4]);
    // Page commands act on what is picked, as in the organize grid.
    assert_eq!(h.state().views[0].target_pages(), [0, 4]);
}

#[test]
fn pages_panel_shift_click_picks_a_range_and_a_plain_click_starts_over() {
    let mut h = pages_panel(6);
    // A plain click goes to the page and is the anchor of the next range.
    h.get_by_label("Page 2").click_modifiers(Modifiers::NONE);
    h.run_steps(2);
    assert_eq!(h.state().views[0].current, 1);
    assert!(picked(&h).is_empty());
    h.get_by_label("Page 4").click_modifiers(Modifiers::SHIFT);
    h.run_steps(2);
    assert_eq!(picked(&h), [1, 2, 3]);
    assert_eq!(h.state().views[0].current, 1);
    // A second ⇧-click ranges from the same anchor, in either direction.
    h.get_by_label("Page 1").click_modifiers(Modifiers::SHIFT);
    h.run_steps(2);
    assert_eq!(picked(&h), [0, 1]);
    // ⇧ with no earlier click ranges from the current page.
    h.get_by_label("Page 6").click_modifiers(Modifiers::NONE);
    h.run_steps(2);
    assert!(picked(&h).is_empty(), "a plain click drops the selection");
    assert_eq!(h.state().views[0].current, 5);
    h.get_by_label("Page 5").click_modifiers(Modifiers::SHIFT);
    h.run_steps(2);
    assert_eq!(picked(&h), [4, 5]);
}

#[test]
fn pages_panel_escape_clears_and_deleted_pages_leave_the_selection() {
    let mut h = pages_panel(5);
    h.get_by_label("Page 2").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(picked(&h), [0, 1]);
    // The pointer is still over the panel.
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(picked(&h).is_empty());
    // Pages that no longer exist drop out of the selection.
    h.get_by_label("Page 5").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(picked(&h), [0, 4]);
    h.state_mut().views[0].select_pages(&[3, 4]);
    assert!(h.state_mut().execute("page.delete"));
    h.run_steps(3);
    assert_eq!(page_texts(h.state()).len(), 3);
    assert!(picked(&h).iter().all(|p| *p < 3), "{:?}", picked(&h));
}

#[test]
fn print_shortcut_offers_the_pages_picked_in_the_pages_panel() {
    let mut h = pages_panel(5);
    h.get_by_label("Page 2").click_modifiers(Modifiers::NONE);
    h.run_steps(2);
    h.get_by_label("Page 4").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(picked(&h), [1, 3]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::P);
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(pdfcraft_ui_egui::Dialog::Print));
    assert_eq!(h.state().print_draft.which, pdfcraft_ui_egui::PrintWhich::Selected);
    assert_eq!(h.state().print_draft.selected, [1, 3]);
    h.get_by_label("Selected pages (2)");
    h.get_by_label("Sheet 1 of 2");
}

#[test]
fn dragging_pages_panel_thumbnails_reorders_pages() {
    let mut h = harness(4, |app| app.right = Some(pdfcraft_ui_egui::RightPanel::Pages));
    let before = page_texts(h.state());
    // The Pages panel is on the right; the document view may label its pages too.
    let thumb = |h: &Harness<'static, PdfCraftApp>, label: &str| {
        h.get_all_by_label(label).map(|n| n.rect()).max_by(|a, b| a.left().total_cmp(&b.left())).expect("the thumbnail")
    };
    // Grab page 3 (page 4 is below the window) and drop it above page 1.
    let (from, to) = (thumb(&h, "Page 3").center(), thumb(&h, "Page 1").center_top() + egui::vec2(0.0, 4.0));
    drag(&mut h, from, to);
    h.run_steps(4);
    let after = page_texts(h.state());
    assert_eq!(after, vec![before[2].clone(), before[0].clone(), before[1].clone(), before[3].clone()], "{after:?}");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Move page"));
    let view = &h.state().views[0];
    assert_eq!(view.selected.iter().copied().collect::<Vec<_>>(), vec![0], "the moved page stays selected");
    assert!(view.panel_drag.is_none());
}

#[test]
fn a_thumbnail_drag_that_ends_off_the_panel_or_outlives_the_pages_is_dropped() {
    let mut h = harness(4, |app| app.right = Some(pdfcraft_ui_egui::RightPanel::Pages));
    let before = page_texts(h.state());
    // A drag left over (the panel closed mid-drag, the button released elsewhere) moves nothing.
    h.state_mut().views[0].panel_drag = Some(vec![2]);
    h.run_steps(2);
    assert!(h.state().views[0].panel_drag.is_none());
    assert_eq!(page_texts(h.state()), before);
    // A change to the document drops page indexes taken before it.
    h.state_mut().views[0].panel_drag = Some(vec![3]);
    let id = h.state().views[0].id;
    let info = h.state().session.get(id).unwrap().info.clone();
    h.state_mut().views[0].document_changed(&info);
    assert!(h.state().views[0].panel_drag.is_none());
}

/// A Pages panel thumbnail (the rightmost node with that label).
fn panel_thumb(h: &Harness<'static, PdfCraftApp>, label: &str) -> egui::Pos2 {
    h.get_all_by_label(label).map(|n| n.rect()).max_by(|a, b| a.left().total_cmp(&b.left())).expect("the thumbnail").center()
}

fn right_click_at(h: &mut Harness<'static, PdfCraftApp>, at: egui::Pos2) {
    h.hover_at(at);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

#[test]
fn deleting_pages_from_the_pages_panel_menu() {
    let mut h = pages_panel(4);
    let at = panel_thumb(&h, "Page 2");
    right_click_at(&mut h, at);
    h.get_by_label("Delete pages").click();
    h.run_steps(4);
    assert_eq!(page_texts(h.state()), ["Page 1", "Page 3", "Page 4"]);
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Delete page"));
    assert!(h.state().views[0].selected.is_empty(), "the deleted pages leave the selection");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert_eq!(page_texts(h.state()), ["Page 1", "Page 2", "Page 3", "Page 4"]);
}

#[test]
fn the_delete_key_deletes_pages_picked_in_the_pages_panel_but_never_every_page() {
    let mut h = pages_panel(3);
    // ⌘-click picks pages 2 and 3 (and the current page, 1: the first ⌘-click keeps it).
    h.get_by_label("Page 2").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    h.get_by_label("Page 3").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    let (p1, p3) = (panel_thumb(&h, "Page 1"), panel_thumb(&h, "Page 3"));
    h.hover_at(p3);
    h.run_steps(1);
    assert_eq!(h.state().views[0].selected.iter().copied().collect::<Vec<_>>(), vec![0, 1, 2]);
    // Every page picked: Delete does nothing, and the menu item is off.
    h.key_press(Key::Delete);
    h.run_steps(3);
    assert_eq!(page_texts(h.state()).len(), 3, "a document keeps at least one page");
    right_click_at(&mut h, p3);
    assert!(h.query_by(|n| n.label().as_deref() == Some("Delete pages") && n.is_disabled()).is_some());
    h.key_press(Key::Escape);
    h.run_steps(2);
    // Pages 2 and 3 picked (page 2 current): Delete deletes them, and page 1 becomes current.
    h.state_mut().views[0].select_pages(&[1, 2]);
    h.hover_at(p1);
    h.run_steps(1);
    h.key_press(Key::Delete);
    h.run_steps(4);
    assert_eq!(page_texts(h.state()), ["Page 1"]);
    assert_eq!(h.state().views[0].current, 0);
}

#[test]
fn host_dirty_hears_when_unsaved_work_appears_and_goes() {
    let reports = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = reports.clone();
    let mut h = harness(2, move |app| {
        app.set_option("organize", "on").unwrap();
        app.host_dirty = Some(Box::new(move |dirty| sink.lock().unwrap().push(dirty)));
    });
    assert_eq!(reports.lock().unwrap().last(), Some(&false), "a freshly opened document has nothing unsaved");
    h.get_by_label("Rotate clockwise").click();
    h.run_steps(3);
    assert_eq!(reports.lock().unwrap().last(), Some(&true), "an edit is unsaved work");

    let out = temp_path("host-dirty.pdf");
    h.state_mut().save_override = Some(out.to_string_lossy().into_owned());
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run_steps(3);
    assert_eq!(reports.lock().unwrap().last(), Some(&false), "saving clears it");
    let _ = std::fs::remove_file(out);
}
