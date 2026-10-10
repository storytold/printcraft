//! Viewer conveniences: close all, revert, fit height, view history, select all, find options,
//! cover page, a scroll position per tab.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

/// `n` pages of 200×300; page i says "Page i+1" plus "page" and "Pages" for find tests.
fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i));
        let body = format!("BT /F1 18 Tf 20 150 Td (Page {} pages PAGE) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("a.pdf", None, fixture(5)).unwrap();
        app.open_bytes("b.pdf", None, fixture(2)).unwrap();
        app
    });
    for _ in 0..40 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h
}

#[test]
fn close_all_asks_only_for_changed_documents() {
    let mut h = harness();
    // Change the active document (b.pdf).
    h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
    h.state_mut().execute("file.close_all");
    h.run_steps(2);
    assert_eq!(h.state().views.len(), 1, "the clean document closed at once");
    assert_eq!(h.state().close_request, Some(pdfcraft_ui_egui::CloseRequest::All), "b.pdf asks to be saved");
    h.get_by_label_contains("Save changes");
}

#[test]
fn revert_after_confirming() {
    let mut h = harness();
    h.state_mut().apply_edit(pdfcraft_engine::Edit::DeletePages { pages: vec![0] });
    assert!(h.state_mut().execute("file.revert"));
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::Revert));
    h.get_all_by_label("Revert").last().expect("the button").click();
    h.run_steps(3);
    let s = h.state();
    let d = s.session.get(s.views[1].id).unwrap();
    assert_eq!((d.info.pages.len(), d.dirty), (2, false));
}

#[test]
fn view_history_select_all_and_find_options() {
    let mut h = harness();
    h.state_mut().active = Some(0);
    h.run_steps(2);
    h.state_mut().views[0].go_to_page(3);
    h.run_steps(2);
    h.state_mut().views[0].go_to_page(1);
    h.run_steps(2);
    assert!(h.state_mut().views[0].view_history(false));
    assert_eq!(h.state().views[0].current, 3, "previous view");
    assert!(h.state_mut().views[0].view_history(false));
    assert_eq!(h.state().views[0].current, 0);
    assert!(h.state_mut().views[0].view_history(true));
    assert_eq!(h.state().views[0].current, 3, "next view");
    // Select all on the current page (once its text is known).
    for _ in 0..40 {
        h.run_steps(2);
        if h.state_mut().views[0].select_all() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("Page 4 pages PAGE"));
    h.state_mut().views[0].select_text(3, 0, 0);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(2);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("Page 4 pages PAGE"), "the viewer shortcut still selects text");
    assert!(h.state().views[0].selected.is_empty(), "the viewer shortcut doesn't select pages");
    // Find: "page" matches three times a page; whole words, case-sensitive narrows it.
    h.state_mut().views[0].open_find();
    h.state_mut().views[0].find.as_mut().unwrap().query = "page".into();
    h.state_mut().views[0].rerun_find();
    // Find runs in the background: wait for it by a deadline, not a frame count, which a loaded
    // machine (the whole workspace testing at once) can use up before the last page is searched.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline && h.state().views[0].find.as_ref().unwrap().matches.len() < 15 {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(h.state().views[0].find.as_ref().unwrap().matches.len(), 15);
    {
        let f = h.state_mut().views[0].find.as_mut().unwrap();
        f.whole_words = true;
        f.case_sensitive = true;
    }
    h.state_mut().views[0].rerun_find();
    h.run_steps(2);
    assert_eq!(h.state().views[0].find.as_ref().unwrap().matches.len(), 0, "\"page\" alone, lower case, appears nowhere");
    h.state_mut().views[0].find.as_mut().unwrap().query = "PAGE".into();
    h.state_mut().views[0].rerun_find();
    h.run_steps(2);
    assert_eq!(h.state().views[0].find.as_ref().unwrap().matches.len(), 5);
}

#[test]
fn find_shortcuts_advance_and_retreat_matches() {
    let mut h = harness();
    h.state_mut().active = Some(0);
    h.run_steps(2);
    h.state_mut().views[0].open_find();
    h.state_mut().views[0].find.as_mut().unwrap().query = "page".into();
    h.state_mut().views[0].rerun_find();
    // Find runs in the background: wait by a deadline, not a frame count, which a loaded machine
    // can use up before the last page is searched.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
        if h.state().views[0].find.as_ref().unwrap().matches.len() >= 15 {
            break;
        }
    }
    let n = h.state().views[0].find.as_ref().unwrap().matches.len();
    assert_eq!(n, 15);
    // Surrender text edit focus so canvas shortcut handler receives key events.
    h.state_mut().views[0].find.as_mut().unwrap().focus = false;
    h.ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("find-input")));
    h.run_steps(2);
    let start = h.state().views[0].find.as_ref().unwrap().current.unwrap();
    // ⌘G advances to next match.
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::G);
    h.run_steps(2);
    assert_eq!(h.state().views[0].find.as_ref().unwrap().current, Some((start + 1) % n));
    // ⇧⌘G retreats to previous match.
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::G);
    h.run_steps(2);
    assert_eq!(h.state().views[0].find.as_ref().unwrap().current, Some(start));
    // ⇧⌘G again wraps backwards.
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::G);
    h.run_steps(2);
    assert_eq!(h.state().views[0].find.as_ref().unwrap().current, Some((start + n - 1) % n));
}

#[test]
fn layouts_fit_height_labels_and_system_theme() {
    use pdfcraft_ui_egui::canvas::{Fit, PageLayout};
    let mut h = harness();
    h.state_mut().active = Some(0);
    // Fit height: the page's height fills the view (less the margins).
    h.state_mut().views[0].fit = Fit::Height;
    h.run_steps(4);
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let vp = h.state().views[0].viewport_rect();
    assert!(r.height() > vp.height() * 0.85 && r.top() >= vp.top() && r.bottom() <= vp.bottom() + 1.0, "page {r:?} in {vp:?}");
    // Two-page view scrolls continuously; a cover puts page 1 alone on the right.
    h.state_mut().views[0].fit = Fit::Width;
    h.state_mut().views[0].layout = PageLayout::TwoUp;
    h.run_steps(4);
    let (a, b) = (h.state().views[0].page_screen_rect(0).unwrap(), h.state().views[0].page_screen_rect(1).unwrap());
    assert!((a.top() - b.top()).abs() < 1.0 && b.left() > a.right(), "side by side");
    h.state_mut().views[0].cover = true;
    h.run_steps(4);
    let (a, b, c) = (
        h.state().views[0].page_screen_rect(0).unwrap(),
        h.state().views[0].page_screen_rect(1).unwrap(),
        h.state().views[0].page_screen_rect(2).unwrap(),
    );
    assert!(a.left() > vp.center().x - 1.0, "the cover sits on the right");
    assert!(b.top() > a.bottom() && (b.top() - c.top()).abs() < 1.0, "then pairs 2–3");
    // Page labels in the page box.
    h.state_mut().apply_edit(pdfcraft_engine::Edit::NumberPages {
        from: 0,
        to: 1,
        style: pdfcraft_engine::LabelStyle::LowerRoman,
        prefix: String::new(),
        first: 1,
    });
    h.run_steps(2);
    let labels: Vec<String> = {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().info.pages.iter().map(|p| p.label.clone()).collect()
    };
    assert_eq!(labels[1], "ii");
    assert!(h.state_mut().views[0].go_to_typed("ii", &labels));
    assert_eq!(h.state().views[0].current, 1);
    assert!(!h.state_mut().views[0].go_to_typed("xx", &labels));
    // Follow the system theme.
    h.state_mut().set_option("theme", "system").unwrap();
    assert_eq!(h.state().theme_preference, pdfcraft_ui_egui::theme::ThemePreference::System);
}

#[test]
fn two_page_view_steps_a_spread_at_a_time() {
    // #70: ⌘→ went to the right-hand page of the same spread, so the view never moved.
    use egui::{Key, Modifiers};
    use pdfcraft_ui_egui::canvas::{Fit, PageLayout};
    let mut h = harness();
    h.state_mut().active = Some(0);
    h.state_mut().views[0].fit = Fit::Width;
    h.state_mut().views[0].layout = PageLayout::TwoUp;
    h.run_steps(4);
    let step = |h: &mut Harness<'static, PdfCraftApp>, key| {
        h.key_press_modifiers(Modifiers::COMMAND, key);
        h.run_steps(4);
        h.state().views[0].current
    };
    let pages: Vec<usize> = [Key::ArrowRight, Key::ArrowRight, Key::ArrowRight, Key::ArrowLeft, Key::ArrowLeft].map(|k| step(&mut h, k)).into();
    assert_eq!(pages, [2, 4, 4, 2, 0], "spreads 1–2, 3–4, 5");
    // With a cover page the spreads are 1, 2–3, 4–5.
    h.state_mut().views[0].cover = true;
    h.run_steps(4);
    let pages: Vec<usize> = [Key::ArrowRight, Key::ArrowRight, Key::ArrowLeft, Key::ArrowLeft].map(|k| step(&mut h, k)).into();
    assert_eq!(pages, [1, 3, 1, 0]);
}

#[test]
fn damaged_files_say_they_were_repaired() {
    // No cross-reference table: the file is reconstructed.
    let damaged = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
trailer << /Root 1 0 R >>
%%EOF"
        .to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("damaged.pdf", None, damaged.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h.get_by_label_contains("This file was damaged and has been repaired.");
    h.get_by_label("Details").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::Properties(pdfcraft_ui_egui::PropsTab::Advanced)));
    h.get_by_label("Repair log");
}

#[test]
fn initial_view_is_edited_and_honoured_on_open() {
    use pdfcraft_engine::{InitialLayout, Magnification, Navigation};
    let mut h = harness();
    h.state_mut().dialog = Some(Dialog::Properties(pdfcraft_ui_egui::PropsTab::InitialView));
    h.run_steps(2);
    h.get_by_label("Open to page");
    {
        let (_, v) = h.state_mut().view_draft.as_mut().expect("seeded");
        v.navigation = Navigation::Bookmarks;
        v.layout = InitialLayout::TwoUpCoverPage;
        v.magnification = Magnification::FitWidth;
        v.page = 1;
        v.language = Some("de-DE".into());
    }
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let id = s.views[s.active.unwrap()].id;
    let v = s.session.get(id).unwrap().initial_view();
    assert_eq!((v.navigation, v.layout, v.page, v.language.as_deref()), (Navigation::Bookmarks, InitialLayout::TwoUpCoverPage, 1, Some("de-DE")));
    assert_eq!(s.session.get(id).unwrap().can_undo(), Some("Change document properties"));
    // Opening the saved file follows it.
    let bytes = s.session.save_bytes(id).unwrap();
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.open_bytes("again.pdf", None, bytes.to_vec()).unwrap();
    let view = &app.views[0];
    assert_eq!((view.current, view.cover, app.right), (1, true, Some(pdfcraft_ui_egui::RightPanel::Bookmarks)));
}

fn drag(h: &mut Harness<'static, PdfCraftApp>, a: egui::Pos2, b: egui::Pos2) {
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(a + (b - a) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(3);
}

#[test]
fn marquee_zoom_and_snapshot() {
    let mut h = harness();
    let i = h.state().active.unwrap();
    // Text "Page 2 pages PAGE" sits at y 150 (of 300) from x 20.
    let r = h.state().views[i].page_screen_rect(0).expect("on screen");
    let at = |x: f32, y: f32| egui::pos2(r.left() + x / 200.0 * r.width(), r.top() + (300.0 - y) / 300.0 * r.height());
    // Snapshot of the text line.
    h.state_mut().system_clipboard = false;
    assert!(h.state_mut().execute("edit.snapshot"));
    drag(&mut h, at(15.0, 175.0), at(180.0, 140.0));
    let (w, hgt, px) = h.state().last_snapshot.clone().expect("a snapshot");
    assert!(w > 50 && hgt > 10, "{w} × {hgt}");
    assert!(px.as_chunks::<4>().0.iter().any(|p| p[0] < 100), "the text is in it");
    // Marquee zoom on a small area zooms in, centred on it.
    let before = h.state().views[i].zoom;
    h.state_mut().set_option("quick", "marquee-zoom").unwrap();
    drag(&mut h, at(20.0, 160.0), at(60.0, 140.0));
    h.run_steps(4);
    let after = h.state().views[i].zoom;
    assert!(after > before * 2.0, "{before} → {after}");
}

#[test]
fn fit_visible_zooms_to_the_content_width() {
    let mut h = harness();
    let i = h.state().active.unwrap();
    let before = h.state().views[i].zoom;
    assert!(h.state_mut().execute("view.fit_visible"));
    for _ in 0..4 {
        h.run_steps(2);
    }
    let v = &h.state().views[i];
    assert!(v.zoom > before * 1.2, "{before} → {}", v.zoom);
    // The text starts 20 pt in from the page's left; that edge is now near the window's left.
    let r = v.page_screen_rect(v.current).expect("on screen");
    let ink_left = r.left() + 20.0 / 200.0 * r.width();
    let vp = v.viewport_rect();
    assert!((ink_left - vp.left() - 16.0).abs() < 12.0, "ink at {ink_left}, viewport {vp:?}");
}

#[test]
fn tab_and_window_show_the_document_title_when_asked() {
    use pdfcraft_engine::Edit;
    let mut h = harness();
    h.run_steps(2);
    assert_eq!(h.state().window_title, "b.pdf — PdfCraft");
    {
        let s = h.state_mut();
        let id = s.views[s.active.unwrap()].id;
        s.session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Quarterly report".into() }).unwrap();
        let mut v = s.session.get(id).unwrap().initial_view();
        v.display_title = true;
        s.session.apply(id, Edit::SetInitialView(Box::new(v))).unwrap();
    }
    h.run_steps(2);
    assert_eq!(h.state().window_title, "Quarterly report — PdfCraft");
    h.get_by_label_contains("Quarterly report");
}

#[test]
fn a_placeholder_title_leaves_the_file_name_on_the_tab_and_window() {
    // A browser's "Save as PDF" of a blank page or pop-up titles the document "about:blank"
    // and asks viewers to show the title: the file name says more.
    use pdfcraft_engine::Edit;
    let mut h = harness();
    {
        let s = h.state_mut();
        let id = s.views[s.active.unwrap()].id;
        s.session.apply(id, Edit::SetInfo { key: "Title".into(), value: "about:blank".into() }).unwrap();
        let mut v = s.session.get(id).unwrap().initial_view();
        v.display_title = true;
        s.session.apply(id, Edit::SetInitialView(Box::new(v))).unwrap();
    }
    h.run_steps(2);
    assert_eq!(h.state().window_title, "b.pdf — PdfCraft");
    assert!(h.query_by_label_contains("about:blank").is_none());
}

#[test]
fn an_earlier_revision_opens_from_document_properties() {
    use pdfcraft_engine::Edit;
    let mut h = harness();
    {
        let s = h.state_mut();
        let id = s.views[s.active.unwrap()].id;
        s.session.apply(id, Edit::SetInfo { key: "Title".into(), value: "Second".into() }).unwrap();
        let bytes = s.session.save_bytes(id).unwrap();
        s.open_bytes("updated.pdf", None, bytes.to_vec()).unwrap();
    }
    h.run_steps(2);
    h.state_mut().dialog = Some(Dialog::Properties(pdfcraft_ui_egui::PropsTab::Advanced));
    h.run_steps(2);
    h.run_steps(2);
    h.get_by_label("View revision 1").click();
    h.run_steps(3);
    let s = h.state();
    assert!(s.dialog.is_none(), "{:?} {:?}", s.dialog, s.session.get(s.views[s.active.unwrap()].id).unwrap().name);
    let doc = s.session.get(s.views[s.active.unwrap()].id).unwrap();
    assert_eq!(doc.name, "updated (revision 1).pdf");
    assert_eq!(doc.info_value("Title"), None, "as it was before the update");
}

#[test]
fn advanced_search_lists_results_with_context() {
    let mut h = harness();
    let i = h.state().active.unwrap();
    assert!(h.state_mut().execute("edit.advanced_search"));
    h.run_steps(2);
    h.get_by_label("What word or phrase would you like to search for?");
    h.state_mut().views[i].find.as_mut().unwrap().query = "pages".into();
    for _ in 0..40 {
        h.run_steps(2);
        if h.state().views[i].find.as_ref().is_some_and(|f| f.matches.len() == 2) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(2);
    h.get_by_label("2 instances");
    h.get_by_label("Page 2");
    // A result shows its context; clicking it goes there.
    h.get_by_label_contains("Page 2 pages PAGE").click();
    h.run_steps(3);
    let v = &h.state().views[i];
    assert_eq!((v.find.as_ref().unwrap().current, v.current), (Some(1), 1));
    // Closing the panel moves the search to the find bar.
    h.state_mut().right = None;
    h.run_steps(2);
    assert!(!h.state().views[i].find.as_ref().unwrap().in_panel);
}

#[test]
fn optimizer_audits_space_usage() {
    let mut h = harness();
    assert!(h.state_mut().execute("optimize.advanced"));
    h.run_steps(2);
    h.get_by_label("Audit space usage…").click();
    h.run_steps(3);
    h.get_by_label("Space Audit");
    h.get_by_label("Content Streams");
    h.get_by_label("Total");
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert_eq!(h.state().dialog, Some(Dialog::Optimize), "back to the optimizer");
}

#[test]
fn each_document_keeps_its_own_scroll_position() {
    // #189: every document's page view shared one scroll position, so switching tabs showed a
    // document at wherever the other one had been scrolled to (usually back at the top).
    use pdfcraft_ui_egui::canvas::Fit;
    let mut h = harness();
    for v in &mut h.state_mut().views {
        v.fit = Fit::Width;
    }
    let show = |h: &mut Harness<'static, PdfCraftApp>, i: usize| {
        h.state_mut().active = Some(i);
        h.run_steps(4);
        h.state().views[i].current
    };
    show(&mut h, 0);
    h.state_mut().views[0].goto = Some((3, 0.0));
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 3, "a.pdf scrolled to page 4");
    // b.pdf opens where it was (the top), and is scrolled to its own last page.
    assert_eq!(show(&mut h, 1), 0);
    h.state_mut().views[1].goto = Some((1, 0.0));
    h.run_steps(4);
    assert_eq!(h.state().views[1].current, 1);
    // Back to a.pdf: still on page 4, and b.pdf still on page 2.
    assert_eq!(show(&mut h, 0), 3);
    assert_eq!(show(&mut h, 1), 1);
}

#[test]
fn a_new_document_opens_at_the_top_after_a_relaunch() {
    // eframe saves egui's memory, scroll offsets included, with the settings and restores it at
    // the next launch, where document ids start again at 1: the first document opened then
    // started at the offset the previous session's first document was left at.
    use pdfcraft_ui_egui::canvas::Fit;
    let mut h = harness();
    for v in &mut h.state_mut().views {
        v.fit = Fit::Width;
    }
    h.state_mut().active = Some(0);
    h.run_steps(4);
    h.state_mut().views[0].goto = Some((3, 0.0));
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 3, "a.pdf scrolled to page 4");
    // Quit: eframe writes egui's memory to app.ron, as below, and reads it back at the next launch.
    #[derive(Default)]
    struct Settings(std::collections::HashMap<String, String>);
    impl eframe::Storage for Settings {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.into(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }
    let mut settings = Settings::default();
    h.ctx.memory(|m| eframe::set_value(&mut settings, "egui", m));
    let memory: egui::Memory = eframe::get_value(&settings, "egui").expect("egui's memory round-trips");
    // Next launch: a document never seen before, which gets a.pdf's old id.
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|cc| {
        cc.egui_ctx.memory_mut(|m| *m = memory);
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("c.pdf", None, fixture(5)).unwrap();
        app.views[0].fit = Fit::Width;
        app
    });
    h.run_steps(4);
    assert_eq!(h.state().views[0].id, pdfcraft_engine::DocId(1));
    assert_eq!(h.state().views[0].current, 0, "c.pdf opens on page 1");
    // It is still a scroll position of its own from then on.
    h.state_mut().views[0].goto = Some((2, 0.0));
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 2);
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 2, "c.pdf stays on page 3");
}

#[test]
fn arrow_and_page_keys_move_through_a_scrolling_document() {
    // #185: in the continuous (default) and two-page views the arrow keys and Page Down / Up did
    // nothing without ⌘.
    use egui::Key;
    use pdfcraft_ui_egui::canvas::{Fit, PageLayout};
    let mut h = harness();
    h.state_mut().active = Some(0);
    h.state_mut().views[0].fit = Fit::Width;
    h.run_steps(4);
    assert_eq!(h.state().views[0].layout, PageLayout::Continuous);
    let press = |h: &mut Harness<'static, PdfCraftApp>, key, times: usize| {
        for _ in 0..times {
            h.key_press(key);
            h.run_steps(2);
        }
        h.run_steps(2);
        h.state().views[0].current
    };
    // → / ← turn pages, as in Acrobat
    assert_eq!(press(&mut h, Key::ArrowRight, 2), 2);
    assert_eq!(press(&mut h, Key::ArrowLeft, 1), 1);
    assert_eq!(press(&mut h, Key::ArrowLeft, 1), 0);
    // ↓ scrolls a line at a time, Page Down a screen: count the presses that reach page 2
    let presses = |h: &mut Harness<'static, PdfCraftApp>, key| {
        let mut n = 0;
        while h.state().views[0].current == 0 {
            press(h, key, 1);
            n += 1;
            assert!(n < 500, "{key:?} never reached page 2");
        }
        n
    };
    let lines = presses(&mut h, Key::ArrowDown);
    assert_eq!(press(&mut h, Key::ArrowUp, lines + 1), 0);
    let screens = presses(&mut h, Key::PageDown);
    assert_eq!(press(&mut h, Key::PageUp, screens + 1), 0);
    assert!(screens >= 2 && lines > 2 * screens, "a fit-width page: {lines} lines, {screens} screens");
    // two-page view: → moves a spread
    h.state_mut().views[0].layout = PageLayout::TwoUp;
    h.run_steps(4);
    assert_eq!(press(&mut h, Key::ArrowRight, 1), 2);
    // single-page view keeps Page Down for the next page
    h.state_mut().views[0].layout = PageLayout::Single;
    h.run_steps(4);
    assert_eq!(press(&mut h, Key::PageDown, 1), 3);
}

#[test]
fn up_and_down_turn_pages_in_single_page_view() {
    // #273: in single-page view ↓ / ↑ only scrolled, so on a page that fits (or once scrolled to
    // the end) they did nothing, while the wheel turned the page there.
    use egui::Key;
    use pdfcraft_ui_egui::canvas::{Fit, PageLayout};
    let mut h = harness();
    h.state_mut().active = Some(0);
    h.state_mut().views[0].layout = PageLayout::Single;
    h.state_mut().views[0].fit = Fit::Page;
    h.run_steps(4);
    let press = |h: &mut Harness<'static, PdfCraftApp>, key, times: usize| {
        for _ in 0..times {
            h.key_press(key);
            h.run_steps(2);
        }
        h.run_steps(2);
        h.state().views[0].current
    };
    // A page that fits: each press turns one page.
    assert_eq!(press(&mut h, Key::ArrowDown, 1), 1);
    assert_eq!(press(&mut h, Key::ArrowDown, 1), 2);
    assert_eq!(press(&mut h, Key::ArrowUp, 1), 1);
    assert_eq!(press(&mut h, Key::ArrowUp, 1), 0);
    assert_eq!(press(&mut h, Key::ArrowUp, 1), 0, "nothing before the first page");
    // A page taller than the window scrolls first, then turns at its bottom.
    h.state_mut().views[0].fit = Fit::Width;
    h.run_steps(4);
    let mut lines = 0;
    while press(&mut h, Key::ArrowDown, 1) == 0 {
        lines += 1;
        assert!(lines < 500, "↓ never reached page 2");
    }
    assert!(lines > 2, "↓ scrolled through page 1 before turning ({lines} lines)");
    // ↑ at the top of page 2 goes back to the bottom of page 1, then scrolls up it.
    assert_eq!(press(&mut h, Key::ArrowUp, 1), 0);
    assert_eq!(press(&mut h, Key::ArrowDown, 1), 1, "back at the bottom of page 1");
    assert_eq!(press(&mut h, Key::ArrowUp, 2), 0);
    assert_eq!(press(&mut h, Key::ArrowDown, 1), 0, "scrolled up from the bottom of page 1");
}

#[test]
fn v_h_and_space_pick_the_quick_tools() {
    // The toolbar's tooltips promise "Select (V)" and "Hand (H)", but the keys did nothing.
    use egui::{Key, Modifiers};
    use pdfcraft_ui_egui::QuickTool;
    let mut h = harness();
    h.state_mut().active = Some(0);
    h.run_steps(2);
    let press = |h: &mut Harness<'static, PdfCraftApp>, key| {
        h.key_press(key);
        h.run_steps(2);
        h.state().quick_tool
    };
    assert_eq!(press(&mut h, Key::H), QuickTool::Hand);
    assert_eq!(press(&mut h, Key::V), QuickTool::Select);
    // V leaves any other tool too, as the toolbar button does.
    h.state_mut().quick_tool = QuickTool::Crop;
    assert_eq!(press(&mut h, Key::V), QuickTool::Select);
    // Holding Space pans with the Hand for as long as it is held, then gives the tool back.
    h.state_mut().quick_tool = QuickTool::Crop;
    h.key_down(Key::Space);
    h.run_steps(4);
    assert_eq!(h.state().quick_tool, QuickTool::Hand, "Space held");
    h.key_up(Key::Space);
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, QuickTool::Crop, "Space released");
    // With a modifier the letters are someone else's shortcut.
    h.key_press_modifiers(Modifiers::SHIFT, Key::H);
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, QuickTool::Crop);
    // They are letters to a text field: typing in the find bar keeps the tool.
    h.state_mut().quick_tool = QuickTool::Select;
    h.state_mut().views[0].open_find();
    h.run_steps(4);
    assert_eq!(press(&mut h, Key::H), QuickTool::Select, "typing H in the find bar");
    h.key_down(Key::Space);
    h.run_steps(2);
    h.key_up(Key::Space);
    h.run_steps(2);
    assert_eq!(h.state().quick_tool, QuickTool::Select, "typing a space in the find bar");
}

/// Like [`fixture`], with each page's size given.
fn sized_fixture(sizes: &[(u32, u32)]) -> Vec<u8> {
    let n = sizes.len();
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for (i, (w, h)) in sizes.iter().enumerate() {
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>",
            5 + 2 * i
        ));
        let body = format!("BT /F1 18 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

#[test]
fn page_down_steps_every_spread_when_several_fit_on_screen() {
    // #188: in two-page view with a cover page, ⌘Page Down skipped spreads (or stuck) once more
    // than one spread fit on screen: the current page was whichever wholly visible page rounding
    // made "most visible", so the next step started from the wrong spread.
    use egui::{Key, Modifiers};
    use pdfcraft_ui_egui::canvas::{Fit, PageLayout};
    // a printed book: portrait cover and back, landscape two-page spreads between
    let mut sizes = vec![(200, 300)];
    sizes.extend([(400, 300); 10]);
    sizes.push((200, 300));
    for fit in [Fit::Width, Fit::Page, Fit::Height] {
        let bytes = sized_fixture(&sizes);
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("book.pdf", None, bytes).unwrap();
            app
        });
        h.run_steps(6);
        h.state_mut().active = Some(0);
        let v = &mut h.state_mut().views[0];
        (v.fit, v.layout, v.cover) = (fit, PageLayout::TwoUp, true);
        h.run_steps(6);
        let mut step = |key| {
            h.key_press_modifiers(Modifiers::COMMAND, key);
            h.run_steps(4);
            h.state().views[0].current
        };
        let down: Vec<usize> = (0..6).map(|_| step(Key::PageDown)).collect();
        assert_eq!(down, [1, 3, 5, 7, 9, 11], "{fit:?}: cover, five spreads, back cover");
        let up: Vec<usize> = (0..6).map(|_| step(Key::PageUp)).collect();
        assert_eq!(up, [9, 7, 5, 3, 1, 0], "{fit:?}");
    }
}

/// Open `sizes` in Fit width, continuous, with the Pages panel showing.
fn pages_panel_harness(sizes: &[(u32, u32)]) -> Harness<'static, PdfCraftApp> {
    let bytes = sized_fixture(sizes);
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("cheques.pdf", None, bytes).unwrap();
        app.right = Some(pdfcraft_ui_egui::RightPanel::Pages);
        app
    });
    h.run_steps(6);
    h.state_mut().active = Some(0);
    h.state_mut().views[0].fit = pdfcraft_ui_egui::canvas::Fit::Width;
    h.run_steps(6);
    h
}

/// A thumbnail in the Pages panel (the panel lists pages as "Page n" buttons).
fn thumbnail(h: &Harness<'static, PdfCraftApp>, label: &str) -> egui::Pos2 {
    h.get_all_by_label(label).find(|n| n.rect().left() > 1000.0).expect("the Pages panel thumbnail").rect().center()
}

/// Press and release `button` at `at`.
fn press(h: &mut Harness<'static, PdfCraftApp>, at: egui::Pos2, button: egui::PointerButton) {
    h.hover_at(at);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: at, button, pressed: false, modifiers: Default::default() });
}

#[test]
fn a_short_page_gone_to_from_the_pages_panel_stays_current() {
    // A short page (a cheque) above a tall one: going to it scrolls it to the top, where the tall
    // page below shows more of itself. The current page, and the Pages panel's highlight, moved
    // to the tall page.
    let mut h = pages_panel_harness(&[(600, 250), (600, 1800), (600, 250), (600, 1800)]);
    for page in [1, 0, 2] {
        let at = thumbnail(&h, &format!("Page {}", page + 1));
        press(&mut h, at, egui::PointerButton::Primary);
        h.run_steps(6);
        assert_eq!(h.state().views[0].current, page, "clicked page {}", page + 1);
    }
}

#[test]
fn pages_panel_thumbnails_have_a_context_menu() {
    let mut h = pages_panel_harness(&[(200, 300); 4]);
    let at = thumbnail(&h, "Page 3");
    press(&mut h, at, egui::PointerButton::Secondary);
    h.run_steps(2);
    // The page right-clicked becomes the selection the menu acts on.
    assert_eq!(h.state().views[0].target_pages(), vec![2]);
    for item in ["Extract pages", "Cut", "Paste after"] {
        h.get_by_label(item);
    }
    h.get_by_label("Copy").click();
    h.run_steps(3);
    assert_eq!(h.state().page_clipboard.as_ref().map(|c| c.pages.clone()), Some(vec![2]));
}
