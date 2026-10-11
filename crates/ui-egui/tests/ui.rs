//! Headless UI tests (egui_kittest + AccessKit). They drive the real app shell without a window.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

/// A tiny PDF with two pages, two bookmarks and one sticky note.
const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /Outlines 6 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Annots [9 0 R] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >> endobj
6 0 obj << /Type /Outlines /First 7 0 R /Last 8 0 R /Count 2 >> endobj
7 0 obj << /Title (Alpha section) /Parent 6 0 R /Next 8 0 R /Dest [3 0 R /Fit] >> endobj
8 0 obj << /Title (Beta section) /Parent 6 0 R /Prev 7 0 R /Dest [4 0 R /Fit] >> endobj
9 0 obj << /Type /Annot /Subtype /Text /Rect [10 370 30 390] /T (Tester) /Contents (Check the numbers) >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        setup(&mut app);
        app
    });
    // Fonts install on frame 1 and apply on frame 2.
    h.run_steps(4);
    h
}

#[test]
fn home_shows_welcome_and_tools() {
    let h = harness(|_| {});
    h.get_by_label_contains("Welcome to PdfCraft");
    assert!(h.query_all_by_label("Organize pages").count() >= 2, "tool list + home card");
    h.get_by_label("Open file");
}

/// Every piece of text painted on the last frame: its whole text, whether it was cut with "…",
/// and where it was drawn.
fn painted_text(h: &Harness<'static, PdfCraftApp>) -> Vec<(String, bool, egui::Rect)> {
    h.output()
        .shapes
        .iter()
        .filter_map(|c| match &c.shape {
            egui::Shape::Text(t) => Some((t.galley.text().to_owned(), t.galley.elided, t.visual_bounding_rect())),
            _ => None,
        })
        .collect()
}

#[test]
fn long_recent_names_and_paths_are_cut_before_the_page_count() {
    let name = format!("{}.pdf", "Quarterly report final version ".repeat(10));
    let path = format!("/{}/{name}", "A folder with a long name".repeat(8));
    let h = harness({
        let (name, path) = (name.clone(), path.clone());
        move |app| app.recent.push(pdfcraft_ui_egui::RecentFile { name, path, pages: 7, size: 2048 })
    });
    let text = painted_text(&h);
    let detail = text.iter().find(|(s, ..)| s.starts_with("7 pages")).expect("the row shows its page count").2;
    for whole in [&name, &path] {
        let (_, cut, rect) = text.iter().find(|(s, ..)| s == whole).expect("the row shows the name and the path");
        assert!(*cut, "{whole:?} is cut short");
        assert!(rect.right() < detail.left(), "{whole:?} ends before the page count");
    }
}

#[test]
fn home_removes_one_recent_file_or_clears_them_all() {
    // Tall enough that the Recent list is on screen without scrolling.
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1600.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        for name in ["first.pdf", "second.pdf", "third.pdf"] {
            app.recent.push(pdfcraft_ui_egui::RecentFile { name: name.into(), path: format!("/nowhere/{name}"), pages: 1, size: 0 });
        }
        app
    });
    h.run_steps(4);
    assert!(h.query_by_label("Remove second.pdf from Recent").is_none(), "the remove button shows only over its row");
    h.get_by_label("second.pdf").hover();
    h.run_steps(2);
    h.get_by_label("Remove second.pdf from Recent").click();
    h.run_steps(2);
    let names: Vec<String> = h.state().recent.iter().map(|r| r.name.clone()).collect();
    assert_eq!(names, ["first.pdf", "third.pdf"], "only that file left the list");
    h.get_by_label("Clear Recent Files").click();
    h.run_steps(2);
    assert!(h.state().recent.is_empty(), "Clear empties the list");
    h.get_by_label_contains("Files you open in PdfCraft appear here");
    assert!(h.query_by_label("Clear Recent Files").is_none(), "and goes away with nothing left to clear");
}

#[test]
fn every_catalog_tool_is_listed_after_view_more() {
    let mut h = harness(|_| {});
    h.get_by_label("View more").click();
    h.run_steps(3);
    for g in pdfcraft_engine::catalog::TOOL_GROUPS {
        assert!(h.query_all_by_label(g.label).count() >= 1, "tool {} missing from All tools", g.label);
    }
}

#[test]
fn opening_a_pdf_shows_comments_and_bookmarks() {
    let mut h = harness(|app| app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens"));
    // Documents with comments open on the Comments panel, like Acrobat.
    h.get_by_label_contains("Check the numbers");
    h.get_by_label("Bookmarks").click();
    h.run_steps(3);
    h.get_by_label("Alpha section");
    h.get_by_label("Beta section");
}

/// A right-to-left file name opens, lays out and paints; the tab's accessible name keeps the
/// logical text (only the painted label is put in visual order).
#[test]
fn a_tab_with_an_arabic_file_name_keeps_its_logical_accessible_name() {
    let name = "واحد اثنين.pdf";
    let mut h = harness(move |app| app.open_bytes(name, None, FIXTURE.to_vec()).expect("fixture opens"));
    let tab = h.get_by_label(name).rect();
    assert!(tab.width() > 64.0 && tab.height() > 0.0, "{tab:?}");
    // A name longer than the tab's 28-character limit is cut on a character boundary.
    let long = "واحد اثنين ثلاثة أربعة خمسة ستة سبعة ثمانية.pdf";
    h.state_mut().open_bytes(long, None, FIXTURE.to_vec()).expect("fixture opens");
    h.run_steps(3);
    h.get_by_label(long);
}

#[test]
fn overflowing_tabs_can_be_reached_by_scrolling() {
    let mut h = harness(|app| {
        for i in 0..20 {
            app.open_bytes(&format!("document-{i:02}.pdf"), None, FIXTURE.to_vec()).unwrap();
        }
    });
    let last = h.get_by_label("document-19.pdf").rect();
    assert!(last.center().x < 1200.0, "newly opened active tab must be visible: {last:?}");
    h.state_mut().active = Some(0);
    h.run_steps(4);
    let first = h.get_by_label("document-00.pdf").rect();
    assert!(first.center().x < 1200.0, "changing active tab must reveal it");
    h.event(egui::Event::PointerMoved(first.center()));
    for _ in 0..8 {
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(-500.0, 0.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        h.run_steps(8);
    }
    let last = h.get_by_label("document-19.pdf").rect();
    assert!(last.center().x > 0.0 && last.center().x < 1200.0, "last tab must be reachable: {last:?}");
    h.get_by_label("document-19.pdf").click();
    h.run_steps(3);
    assert_eq!(h.state().active, Some(19));
    assert!(h.get_by_label_contains("Display theme:").rect().center().x < 1400.0);
    let close = last.right_center() - egui::vec2(16.0, 0.0);
    h.event(egui::Event::PointerMoved(close));
    h.event(egui::Event::PointerButton { pos: close, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.step();
    h.event(egui::Event::PointerButton { pos: close, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(4);
    assert_eq!(h.state().views.len(), 19);
    assert!(h.state().active.is_some_and(|i| i < 19));
    if let Ok(dir) = std::env::var("PDFCRAFT_TAB_SHOTS") {
        settle(&mut h);
        h.render().unwrap().save(format!("{dir}/overflow-tabs.png")).unwrap();
    }
}

/// Whether a node labelled `label` is there and enabled (`None`: not there).
fn enabled(h: &Harness<'static, PdfCraftApp>, label: &str) -> Option<bool> {
    h.query_by_label(label)?;
    Some(h.query_by(|n| n.label().as_deref() == Some(label) && n.is_disabled()).is_none())
}

#[test]
fn tabs_shrink_to_fit_then_arrows_step_through_them_and_open_stays_in_sight() {
    // Five long names are wider than the strip: the tabs shrink instead of running under the
    // buttons on the right, Open sits after them, and no arrows show.
    let name = |i: usize| format!("Statement {i} for the office records, scanned.pdf");
    let mut h = harness(move |app| {
        for i in 0..5 {
            app.open_bytes(&name(i), None, FIXTURE.to_vec()).unwrap();
        }
    });
    let open = h.get_by_label("Open").rect();
    let discord = h.get_by_label("Discord").rect();
    assert!(open.right() <= discord.left(), "Open {open:?} must not run under Discord {discord:?}");
    let mut previous_right = f32::MIN;
    for i in 0..5 {
        let tab = h.get_by_label(&name(i)).rect();
        assert!(tab.left() >= previous_right - 0.5, "tabs must not overlap: {i} at {tab:?}");
        assert!(tab.right() <= open.left() + 0.5, "every tab fits before Open: {i} at {tab:?}, Open at {open:?}");
        assert!(tab.width() >= 179.0, "tabs keep most of their name: {i} is {tab:?}");
        previous_right = tab.right();
    }
    assert_eq!(enabled(&h, "Scroll tabs left"), None, "no arrows while every tab fits");
    // Seven don't fit even at their narrowest: arrows show. The newest tab is active and in
    // sight at the end, so only the left arrow can go further.
    for i in 5..7 {
        h.state_mut().open_bytes(&name(i), None, FIXTURE.to_vec()).unwrap();
    }
    h.run_steps(6);
    assert_eq!((enabled(&h, "Scroll tabs left"), enabled(&h, "Scroll tabs right")), (Some(true), Some(false)));
    let first = h.get_by_label(&name(0)).rect();
    h.get_by_label("Scroll tabs left").click();
    h.run_steps(4);
    assert!(h.get_by_label(&name(0)).rect().left() > first.left(), "the left arrow scrolls the tabs back");
    assert_eq!(enabled(&h, "Scroll tabs right"), Some(true), "and then there is more to the right");
    let open = h.get_by_label("Open").rect();
    let right = h.get_by_label("Scroll tabs right").rect();
    assert!(right.right() <= open.left() + 0.5 && open.right() <= h.get_by_label("Discord").rect().left());
    // Twenty: Open stays in sight left of Discord.
    for i in 0..13 {
        h.state_mut().open_bytes(&format!("more-{i:02}.pdf"), None, FIXTURE.to_vec()).unwrap();
    }
    h.run_steps(4);
    let open = h.get_by_label("Open").rect();
    let discord = h.get_by_label("Discord").rect();
    assert!(open.left() > 0.0 && open.right() <= discord.left(), "Open {open:?} stays in sight left of Discord {discord:?}");
}

#[test]
fn narrow_tab_strips_accept_vertical_wheel_scrolling() {
    for width in [600.0, 900.0] {
        let mut h = Harness::builder().with_size(egui::vec2(width, 700.0)).build_eframe(|_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            for i in 0..12 {
                app.open_bytes(&format!("narrow-{i:02}.pdf"), None, FIXTURE.to_vec()).unwrap();
            }
            app
        });
        h.run_steps(4);
        let last = h.get_by_label("narrow-11.pdf").rect();
        assert!(last.center().x > 28.0 && last.right() < width - 100.0, "active tab: {last:?}");
        h.state_mut().active = Some(0);
        h.run_steps(4);
        let first = h.get_by_label("narrow-00.pdf").rect();
        assert!(first.center().x > 28.0 && first.right() < width - 100.0);
        h.event(egui::Event::PointerMoved(first.center()));
        for _ in 0..8 {
            h.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -500.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            });
            h.run_steps(8);
        }
        let last = h.get_by_label("narrow-11.pdf").rect();
        assert!(last.center().x > 28.0 && last.right() < width - 100.0, "scrolled tab: {last:?}");
        h.get_by_label("narrow-11.pdf").click();
        h.run_steps(4);
        assert_eq!(h.state().active, Some(11));
        assert!(h.get_by_label_contains("Display theme:").rect().right() <= width);
        if let Ok(dir) = std::env::var("PDFCRAFT_TAB_SHOTS") {
            settle(&mut h);
            h.render().unwrap().save(format!("{dir}/tabs-{width}.png")).unwrap();
        }
    }
}

#[test]
fn garbage_input_is_rejected_without_panicking() {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    assert!(app.open_bytes("junk.pdf", None, b"this is not a pdf".to_vec()).is_err());
    assert!(app.open_bytes("empty.pdf", None, Vec::new()).is_err());
    let mut truncated = FIXTURE.to_vec();
    truncated.truncate(FIXTURE.len() / 3);
    // A truncated file either repairs or fails cleanly; it must not panic.
    let _ = app.open_bytes("truncated.pdf", None, truncated);
}

#[test]
fn theme_and_view_options_apply() {
    let mut h = harness(|app| {
        app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).expect("fixture opens");
        app.set_option("theme", "dark").unwrap();
        app.set_option("panel", "pages").unwrap();
        app.set_option("page", "2").unwrap();
    });
    h.run_steps(3);
    h.get_by_label("Page 2");
    assert!(h.state_mut().set_option("panel", "nope").is_err());
}

/// Two pages of real text.
const TEXT_FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 5 0 R /Resources << /Font << /F1 7 0 R >> >> >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> >> >> endobj
5 0 obj << /Length 55 >> stream
BT /F1 14 Tf 20 150 Td (The quick brown fox) Tj ET
endstream endobj
6 0 obj << /Length 58 >> stream
BT /F1 14 Tf 20 150 Td (A second brown animal) Tj ET
endstream endobj
7 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    for _ in 0..200 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn find_counts_matches_across_pages() {
    let mut h = harness(|app| {
        app.open_bytes("text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
        app.set_option("find", "brown").unwrap();
    });
    settle(&mut h);
    h.get_by_label_contains("of 2");
    let v = &h.state().views[0];
    let f = v.find.as_ref().expect("find open");
    assert_eq!(f.matches.iter().map(|(p, _)| *p).collect::<Vec<_>>(), vec![0, 1]);
}

#[test]
fn find_reports_no_matches() {
    let mut h = harness(|app| {
        app.open_bytes("text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
        app.set_option("find", "zebra").unwrap();
    });
    settle(&mut h);
    h.get_by_label("No matches");
}

#[test]
fn drag_selects_text_and_copy_returns_it() {
    let mut h = harness(|app| {
        app.open_bytes("text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
    });
    settle(&mut h);
    // "The quick brown fox": glyph 4 = 'q', glyph 14 = 'n' of "brown".
    let (a, b) = {
        let v = &h.state().views[0];
        (v.glyph_screen_pos(0, 4).expect("text layer loaded"), v.glyph_screen_pos(0, 14).expect("glyph"))
    };
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    h.hover_at(a + (b - a) * 0.5);
    h.run_steps(1);
    h.hover_at(b);
    h.run_steps(2);
    h.drop_at(b);
    h.run_steps(2);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick brown"));
}

#[test]
fn persistence_round_trips_and_tolerates_garbage() {
    let mut a = PdfCraftApp::new();
    a.set_option("language", "en").unwrap();
    a.set_option("theme", "dark").unwrap();
    let json = a.persist();
    let mut b = PdfCraftApp::new();
    b.set_option("language", "en").unwrap();
    b.restore(&json);
    assert_eq!(b.theme, pdfcraft_ui_egui::theme::ThemeKind::Dark);
    b.restore("{not json");
    b.restore("{\"recent\": 5, \"theme\": \"Purple\"}");
    assert_eq!(b.theme, pdfcraft_ui_egui::theme::ThemeKind::Dark);
}

/// A tool's saved "no fill" (`"fill": null`) clears a fill it had as its default; a missing or
/// malformed fill keeps it (#340).
#[test]
fn a_saved_no_fill_clears_a_comment_tool_default_fill() {
    use pdfcraft_ui_egui::comments::CommentTool;
    let filled = |a: &mut PdfCraftApp| {
        let mut s = a.comment_prefs.style(CommentTool::Rectangle);
        s.fill = Some([0.2, 0.4, 0.6]);
        a.comment_prefs.set_style(CommentTool::Rectangle, s);
    };
    let mut a = PdfCraftApp::new();
    filled(&mut a);
    a.restore(r#"{"comment_styles":[{"tool":"comment.square","fill":null}]}"#);
    assert_eq!(a.comment_prefs.style(CommentTool::Rectangle).fill, None, "null clears the fill");
    let mut b = PdfCraftApp::new();
    filled(&mut b);
    b.restore(r#"{"comment_styles":[{"tool":"comment.square","color":[0,0,1]},{"tool":"comment.square","fill":"red"}]}"#);
    assert_eq!(b.comment_prefs.style(CommentTool::Rectangle).fill, Some([0.2, 0.4, 0.6]), "missing or malformed keeps it");
    // A round trip keeps a cleared fill cleared.
    let mut c = PdfCraftApp::new();
    c.restore(&a.persist());
    assert_eq!(c.comment_prefs.style(CommentTool::Rectangle).fill, None);
}

/// Persisted per-tool comment styles go through their clamping setters: hostile or malformed
/// values are clamped or ignored, never trusted (#340).
#[test]
fn comment_style_settings_clamp_and_refuse_hostile_values() {
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut a = PdfCraftApp::new();
    a.restore(
        r#"{"comment_styles":[
            {"tool":"comment.highlight","color":[2,-1,0.5],"opacity":99,"width":-4},
            {"tool":"comment.note","color":"yellow","opacity":"none"},
            {"tool":"comment.note","color":[0.1,null,0.3]},
            {"tool":"bogus.command","color":[0,0,0]}
        ]}"#,
    );
    // Out-of-range numbers are clamped into range.
    let st = a.comment_prefs.style(CommentTool::Highlight);
    assert_eq!((st.color, st.opacity, st.width), ([1.0, 0.0, 0.5], 1.0, 0.5));
    // A malformed colour is refused whole: the tool keeps its default.
    let note = a.comment_prefs.style(CommentTool::Note);
    assert_eq!(note.color, [1.0, 0.82, 0.0], "the default colour stays");
}

#[test]
fn zoom_keeps_the_point_under_the_cursor_still() {
    let mut h = harness(|app| {
        app.open_bytes("text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
    });
    settle(&mut h);
    let before = h.state().views[0].glyph_screen_pos(0, 4).expect("glyph on screen");
    let z = h.state().views[0].zoom;
    h.state_mut().views[0].zoom_at(z * 2.0, before);
    h.run_steps(4);
    let after = h.state().views[0].glyph_screen_pos(0, 4).expect("glyph still on screen");
    assert!((after - before).length() < 2.0, "moved from {before:?} to {after:?}");
}

#[test]
fn selection_works_in_every_view_rotation() {
    for deg in ["90", "180", "270"] {
        let mut h = harness(move |app| {
            app.open_bytes("text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
            app.set_option("left", "closed").unwrap();
            app.set_option("rotate", deg).unwrap();
            // Keep the whole rotated page on screen.
            app.set_option("zoom", "60").unwrap();
        });
        settle(&mut h);
        let (a, b) = {
            let v = &h.state().views[0];
            (v.glyph_screen_pos(0, 4).expect("glyph"), v.glyph_screen_pos(0, 14).expect("glyph"))
        };
        h.hover_at(a);
        h.run_steps(1);
        h.drag_at(a);
        h.run_steps(1);
        h.hover_at(a + (b - a) * 0.5);
        h.run_steps(1);
        h.hover_at(b);
        h.run_steps(2);
        h.drop_at(b);
        h.run_steps(2);
        assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick brown"), "rotation {deg}");
    }
}

/// A file dropped on the window, as the windowing layer hands it over.
#[derive(Debug)]
struct Dropped {
    path: std::path::PathBuf,
    bytes: Vec<u8>,
}

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.path
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        Ok(self.bytes.clone())
    }
}

#[test]
fn dropping_a_pdf_on_the_window_opens_it() {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    h.run_steps(3);
    assert!(h.state().views.is_empty());
    let file: egui::DroppedFileHandle = std::sync::Arc::new(Dropped { path: "dropped.pdf".into(), bytes: FIXTURE.to_vec() });
    h.input_mut().dropped_files.push(file);
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 1, "the dropped PDF opens in a tab");
    h.get_by_label_contains("dropped.pdf");
}

#[test]
fn pdfs_dropped_on_the_combine_tab_join_its_list() {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    h.run_steps(3);
    h.state_mut().set_option("combine-view", "list").unwrap();
    h.state_mut().execute("page.combine");
    h.run_steps(2);
    // An absolute path is read from disk, with its modified time.
    let path = std::env::temp_dir().join(format!("pdfcraft-combine-drop-{}.pdf", std::process::id()));
    std::fs::write(&path, FIXTURE).unwrap();
    let on_disk: egui::DroppedFileHandle = std::sync::Arc::new(Dropped { path: path.clone(), bytes: Vec::new() });
    let in_memory: egui::DroppedFileHandle = std::sync::Arc::new(Dropped { path: "second.pdf".into(), bytes: FIXTURE.to_vec() });
    h.input_mut().dropped_files.extend([on_disk, in_memory]);
    h.run_steps(3);
    std::fs::remove_file(&path).ok();
    let app = h.state();
    assert!(app.views.is_empty(), "nothing opens");
    assert_eq!(app.combine_draft.len(), 2);
    assert!(app.combine_draft[0].modified.is_some() && app.combine_draft[1].modified.is_none());
    h.get_by_label("just now");
}

#[test]
fn a_folder_dropped_on_the_combine_tab_adds_its_pdfs() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-combine-drop-dir-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("inner")).unwrap();
    std::fs::write(dir.join("one.pdf"), FIXTURE).unwrap();
    std::fs::write(dir.join("inner").join("two.pdf"), FIXTURE).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app
    });
    h.run_steps(3);
    h.state_mut().execute("page.combine");
    h.run_steps(2);
    let folder: egui::DroppedFileHandle = std::sync::Arc::new(Dropped { path: dir.clone(), bytes: Vec::new() });
    h.input_mut().dropped_files.push(folder);
    h.run_steps(3);
    let _ = std::fs::remove_dir_all(&dir);
    let names: Vec<_> = h.state().combine_draft.iter().map(|f| f.name.clone()).collect();
    assert_eq!(names, ["two.pdf", "one.pdf"], "subfolders included, in path order");
}

#[test]
fn files_and_quit_from_the_operating_system() {
    // #73: macOS hands Finder double-clicks, Open With and Dock drops over as Apple events.
    use pdfcraft_ui_egui::OsEvent;
    let name = format!("pdfcraft-os-open-{}.pdf", std::process::id());
    let path = std::env::temp_dir().join(&name);
    std::fs::write(&path, FIXTURE).unwrap();
    let queue = std::rc::Rc::new(std::cell::RefCell::new(vec![OsEvent::Open(vec![path.to_string_lossy().into_owned()])]));
    let q = queue.clone();
    let mut h = harness(move |app| app.os_events = Some(Box::new(move || q.borrow_mut().drain(..).collect())));
    std::fs::remove_file(&path).ok();
    assert_eq!(h.state().views.len(), 1, "the file opens in a tab");
    h.get_by_label_contains(&name);
    // Quit closes the window like the close button, so unsaved changes are asked about.
    queue.borrow_mut().push(OsEvent::Quit);
    h.step();
    let closing = h.output().viewport_output.values().any(|v| v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::Close)));
    assert!(closing);
}

#[test]
fn default_workspace_mode_persists_and_tolerates_invalid_settings() {
    use pdfcraft_ui_egui::Mode;
    for mode in [Mode::AllTools, Mode::Read, Mode::Edit, Mode::Convert, Mode::Sign] {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.default_mode = mode;
        app.set_option("mode", "sign").unwrap();
        let mut restored = PdfCraftApp::new();
        restored.set_option("language", "en").unwrap();
        restored.restore(&app.persist());
        assert_eq!(restored.default_mode, mode);
        restored.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).unwrap();
        assert_eq!(restored.mode, mode, "session override must not be persisted");
    }
    for json in ["{}", "{not json", r#"{"default_mode": null}"#, r#"{"default_mode": 5}"#, r#"{"default_mode": "unknown"}"#] {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.restore(json);
        assert_eq!(app.default_mode, Mode::AllTools);
        app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).unwrap();
        assert_eq!(app.mode, Mode::AllTools);
    }
}

#[test]
fn bookmarks_open_in_read_mode() {
    use pdfcraft_ui_egui::{Mode, RightPanel};
    // #932: a PDF opened with /PageMode /UseOutlines sets Bookmarks even in Read mode, and the
    // panel must actually render there; the rail hides the tool-only buttons in Read mode.
    let pdf = String::from_utf8(FIXTURE.to_vec()).unwrap().replace("/Outlines 6 0 R", "/Outlines 6 0 R /PageMode /UseOutlines");
    let mut h = harness(move |app| {
        app.set_option("mode", "read").unwrap();
        app.open_bytes("outlines.pdf", None, pdf.into_bytes()).unwrap();
    });
    h.run_steps(2);
    let app = h.state();
    assert_eq!(app.mode, Mode::Read);
    assert_eq!(app.right, Some(RightPanel::Bookmarks));
    // The panel is on screen: its two bookmarks are clickable rows.
    h.get_by_label("Alpha section");
    h.get_by_label("Beta section");
    // The tool-only rail buttons are hidden in Read mode.
    assert_eq!(h.query_all_by_label("Form fields").count(), 0);
    // The panel heading is there too (the rail button and the heading both read Bookmarks).
    assert!(h.query_all_by_label("Bookmarks").count() >= 2);
}

#[test]
fn newly_opened_pdfs_use_the_default_workspace() {
    use pdfcraft_ui_egui::{LeftPanel, Mode};
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.restore(r#"{"default_mode": "edit"}"#);
    app.open_bytes("first.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(app.mode, Mode::Edit);
    assert_eq!(app.left, LeftPanel::Tool("edit"));
    app.default_mode = Mode::Read;
    assert_eq!(app.mode, Mode::Edit, "changing the preference affects future opens");
    app.open_bytes("second.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(app.mode, Mode::Read);
    // PDF Initial View is independent of the workspace preference.
    let pdf =
        String::from_utf8(FIXTURE.to_vec()).unwrap().replace("/Outlines 6 0 R", "/Outlines 6 0 R /PageLayout /TwoColumnLeft /PageMode /UseOutlines");
    app.open_bytes("initial-view.pdf", None, pdf.into_bytes()).unwrap();
    assert_eq!(app.mode, Mode::Read);
    assert_eq!(app.views.last().unwrap().layout, pdfcraft_ui_egui::canvas::PageLayout::TwoUp);
    assert_eq!(app.right, Some(pdfcraft_ui_egui::RightPanel::Bookmarks));
}

#[test]
fn explicit_mode_overrides_default_before_and_after_open() {
    use pdfcraft_ui_egui::Mode;
    for before in [false, true] {
        for (value, mode) in [("all", Mode::AllTools), ("read", Mode::Read), ("edit", Mode::Edit), ("convert", Mode::Convert), ("sign", Mode::Sign)] {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.default_mode = Mode::Read;
            if before {
                app.set_option("mode", value).unwrap();
            }
            app.open_bytes("first.pdf", None, FIXTURE.to_vec()).unwrap();
            if !before {
                // Desktop startup applies CLI options after the initial files open.
                app.set_option("mode", value).unwrap();
            }
            assert_eq!(app.mode, mode);
            app.set_option("default-mode", "edit").unwrap();
            app.open_bytes("second.pdf", None, FIXTURE.to_vec()).unwrap();
            assert_eq!(app.mode, mode);
            assert_eq!(app.default_mode, Mode::Edit);
        }
    }
}

#[test]
fn preferences_selects_default_workspace_for_next_open() {
    use egui::accesskit::Role;
    use pdfcraft_ui_egui::Mode;
    let mut h = harness(|app| app.set_option("dialog", "preferences").unwrap());
    h.get_by_label("Default workspace mode");
    h.get_by_role_and_label(Role::RadioButton, "Read").click();
    h.run_steps(2);
    assert_eq!(h.state().default_mode, Mode::Read);
    assert_eq!(h.state().mode, Mode::AllTools);
    h.get_by_label("OK").click();
    h.run_steps(2);
    h.state_mut().open_bytes("fixture.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(h.state().mode, Mode::Read);
}

#[test]
fn explicit_mode_preserves_independent_tool_and_panel_options() {
    use pdfcraft_ui_egui::LeftPanel;
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.set_option("tool", "export").unwrap();
    app.set_option("left", "closed").unwrap();
    app.set_option("mode", "edit").unwrap();
    assert_eq!(app.left, LeftPanel::Tool("export"));
    assert!(!app.left_open);
    app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(app.left, LeftPanel::Tool("export"));
    assert!(!app.left_open);
}

#[test]
fn opening_a_pdf_keeps_a_closed_left_panel_and_the_chosen_tool() {
    use pdfcraft_ui_egui::{LeftPanel, Mode};
    // `--left closed` without `--mode`, default workspace Edit: the panel stays closed.
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.default_mode = Mode::Edit;
    app.set_option("left", "closed").unwrap();
    app.open_bytes("fixture.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(app.mode, Mode::Edit);
    assert!(!app.left_open);
    // Default All Tools: opening another PDF leaves the tool panel the user picked.
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.set_option("tool", "export").unwrap();
    app.open_bytes("first.pdf", None, FIXTURE.to_vec()).unwrap();
    app.open_bytes("second.pdf", None, FIXTURE.to_vec()).unwrap();
    assert_eq!(app.mode, Mode::AllTools);
    assert_eq!(app.left, LeftPanel::Tool("export"));
}

fn three_tab_harness() -> Harness<'static, PdfCraftApp> {
    harness(|app| {
        for name in ["first.pdf", "middle.pdf", "last.pdf"] {
            app.open_bytes(name, None, FIXTURE.to_vec()).unwrap();
        }
    })
}

fn close_named_tab(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    // The close button has no separate accessible label. Its centre is defined by
    // chrome::tab relative to the actual accessible tab rectangle, not a viewport guess.
    let pos = h.get_by_label(name).rect().right_center() - egui::vec2(16.0, 0.0);
    h.event(egui::Event::PointerMoved(pos));
    h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(3);
}

#[test]
fn closing_tabs_keeps_the_selected_document_or_selects_its_neighbor() {
    for removed in [0, 1, 2] {
        let mut h = three_tab_harness();
        let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();
        h.get_by_label("middle.pdf").click();
        h.run_steps(3);
        assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[1]));
        close_named_tab(&mut h, ["first.pdf", "middle.pdf", "last.pdf"][removed]);
        let expected = if removed == 1 { ids[2] } else { ids[1] };
        let app = h.state();
        assert_eq!(app.active_ids().map(|(_, id)| id), Some(expected), "removed tab {removed}");
        assert_eq!(
            app.views.iter().map(|view| view.id).collect::<Vec<_>>(),
            ids.iter().copied().enumerate().filter_map(|(i, id)| (i != removed).then_some(id)).collect::<Vec<_>>()
        );
        assert_eq!(app.session.docs().len(), 2);
        assert!(app.session.get(ids[removed]).is_none());
        assert!(app.close_request.is_none(), "clean tabs close without a prompt");
        for doc in app.session.docs() {
            assert_eq!(doc.bytes.as_slice(), FIXTURE);
            assert!(!doc.dirty);
        }
    }
}

#[test]
fn closing_the_last_active_tab_selects_the_previous_one_then_home() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();
    for (removed, name) in [(2usize, "last.pdf"), (1, "middle.pdf"), (0, "first.pdf")] {
        close_named_tab(&mut h, name);
        let app = h.state();
        assert_eq!(app.active_ids().map(|(_, id)| id), removed.checked_sub(1).map(|index| ids[index]));
        assert_eq!(app.views.len(), removed);
        assert_eq!(app.session.docs().len(), removed);
        assert!(app.session.get(ids[removed]).is_none());
    }
    assert_eq!(h.state().active, None);
    h.get_by_label_contains("Welcome to PdfCraft");
}

#[test]
fn cancelling_then_discarding_an_earlier_dirty_tab_preserves_the_selected_document() {
    let mut h = three_tab_harness();
    let ids: Vec<_> = h.state().views.iter().map(|view| view.id).collect();
    h.get_by_label("first.pdf").click();
    h.run_steps(3);
    assert!(h.state_mut().apply_edit(pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Unsaved title".into() }));
    h.run_steps(3);
    h.get_by_label("middle.pdf").click();
    h.run_steps(3);
    close_named_tab(&mut h, "first.pdf (edited)");
    assert_eq!(h.state().close_request, Some(pdfcraft_ui_egui::CloseRequest::Tab(ids[0])));
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().views.len(), 3);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[1]));
    assert!(h.state().session.get(ids[0]).unwrap().dirty);
    close_named_tab(&mut h, "first.pdf (edited)");
    h.get_by_label("Don't save").click();
    h.run_steps(3);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), Some(ids[1]));
    assert_eq!(h.state().views.iter().map(|view| view.id).collect::<Vec<_>>(), [ids[1], ids[2]]);
    assert!(h.state().session.get(ids[0]).is_none());
    assert!(h.state().close_request.is_none());
    for doc in h.state().session.docs() {
        assert_eq!(doc.bytes.as_slice(), FIXTURE);
        assert!(!doc.dirty);
    }
}
