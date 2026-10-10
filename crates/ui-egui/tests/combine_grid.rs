//! Headless UI tests for the Combine files grid: a thumbnail card per file, selected, moved and
//! undone like the list's rows; thumbnails rendered lazily, a few at a time, and never stale.

use std::time::Duration;

use egui::{Key, Modifiers, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_render::{PageRenderer, RenderRequest, RequestKind};
use pdfcraft_ui_egui::{CombineThumb, CombineView, FilePurpose, PdfCraftApp};

/// An `n`-page document with a proper xref table; page `i` shows "Page i+1".
fn fixture(n: usize) -> Vec<u8> {
    fixture_sized(n, 200.0, 300.0)
}

/// The same, with pages of `w` × `h` points.
fn fixture_sized(n: usize, w: f32, h: f32) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 {w} {h}] >>", kids.join(" ")));
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

/// A one-page document whose page takes a while to draw (tens of thousands of shapes), so a
/// thumbnail render is still under way when a test acts.
fn heavy() -> Vec<u8> {
    let mut body = String::from("0.2 0.4 0.8 rg\n");
    for i in 0..60_000 {
        body.push_str(&format!("{} {} 1 1 re f\n", i % 200, (i / 200) % 300));
    }
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_string(),
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

/// A two-page fixture that opens with `user` and allows everything with `owner`.
fn protected(user: &str, owner: &str) -> Vec<u8> {
    protected_with(pdfcraft_cos::Algorithm::Aes256, user, owner)
}

fn protected_with(algorithm: pdfcraft_cos::Algorithm, user: &str, owner: &str) -> Vec<u8> {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(fixture(2))).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm,
        user_password: user,
        owner_password: owner,
        permissions: -1,
        encrypt_metadata: true,
        seed: [4; 32],
    })
    .unwrap();
    pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()
}

/// The Combine tab with these files (name, bytes), in its default (grid) view.
fn grid_of(files: Vec<(&str, Vec<u8>)>, size: egui::Vec2) -> Harness<'static, PdfCraftApp> {
    grid_after(files, size, 4)
}

/// The same, after only `steps` frames (1: the first thumbnails have only just been asked for).
fn grid_after(files: Vec<(&str, Vec<u8>)>, size: egui::Vec2, steps: usize) -> Harness<'static, PdfCraftApp> {
    let files: Vec<(String, Vec<u8>)> = files.into_iter().map(|(n, b)| (n.to_string(), b)).collect();
    let mut h = Harness::builder().with_size(size).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.use_files(FilePurpose::Combine, files);
        app
    });
    h.run_steps(steps);
    h
}

/// The grid on the frame its first thumbnails start rendering (a render started in one frame
/// is collected in a later one, so there always is such a frame).
fn grid_after_one(files: Vec<(&str, Vec<u8>)>, size: egui::Vec2) -> Harness<'static, PdfCraftApp> {
    let mut h = grid_after(files, size, 0);
    for _ in 0..20 {
        h.run_steps(1);
        if h.state().combine_thumbnails_rendering() > 0 {
            break;
        }
    }
    h
}

fn grid_of_pages(names: &[(&str, usize)]) -> Harness<'static, PdfCraftApp> {
    grid_of(names.iter().map(|(n, p)| (*n, fixture(*p))).collect(), vec2(1400.0, 900.0))
}

fn names(h: &Harness<'static, PdfCraftApp>) -> Vec<String> {
    h.state().combine_draft.iter().map(|f| f.name.clone()).collect()
}

/// Run frames until `done`, giving render threads time; whether it happened.
fn settle(h: &mut Harness<'static, PdfCraftApp>, done: impl Fn(&PdfCraftApp) -> bool) -> bool {
    for _ in 0..400 {
        h.run_steps(1);
        if done(h.state()) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn ready(t: &CombineThumb) -> bool {
    matches!(t, CombineThumb::Ready { .. })
}

/// Press, move in steps and release, as a user drags.
fn drag(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=6 {
        h.hover_at(from + (to - from) * (k as f32 / 6.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(4);
}

/// Where a file's card is.
fn card(h: &Harness<'static, PdfCraftApp>, name: &str) -> egui::Rect {
    h.get_by_label(name).rect()
}

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
fn combine_shows_a_grid_by_default_and_the_view_is_kept_in_the_settings() {
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 2)]);
    assert_eq!(h.state().combine_view, CombineView::Grid);
    h.get_by_label_contains("Files are combined in the order shown");
    assert!(h.query_by_label("File name").is_none(), "no table headings in the grid");
    // The totals stay on screen below the grid.
    let totals = h.get_by_label_contains("2 files · 3 pages").rect();
    assert!(totals.bottom() <= 900.0, "the totals line is visible: {totals:?}");
    h.get_by_label("List view").click();
    h.run_steps(2);
    assert_eq!(h.state().combine_view, CombineView::List);
    h.get_by_label("File name");
    // Kept in the settings, and read back.
    let saved = h.state().persist();
    let mut fresh = PdfCraftApp::new();
    assert_eq!(fresh.combine_view, CombineView::Grid);
    fresh.restore(&saved);
    assert_eq!(fresh.combine_view, CombineView::List);
    // A damaged setting keeps the default.
    let mut damaged = PdfCraftApp::new();
    damaged.restore(r#"{"combine_view": "tiles"}"#);
    assert_eq!(damaged.combine_view, CombineView::Grid);
    // Scriptable, with a clear refusal for anything else.
    let app = h.state_mut();
    app.set_option("combine-view", "grid").unwrap();
    assert_eq!(app.combine_view, CombineView::Grid);
    assert_eq!(app.set_option("combine-view", "tiles"), Err("combine-view must be grid or list".into()));
}

#[test]
fn cards_select_like_the_list_and_the_selection_survives_switching_views() {
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 1), ("c.pdf", 1), ("d.pdf", 1), ("e.pdf", 1)]);
    h.get_by_label("b.pdf").click();
    h.run_steps(1);
    h.get_by_label("d.pdf").click_modifiers(Modifiers::SHIFT);
    h.run_steps(1);
    assert_eq!(h.state().combine_selection(), [1, 2, 3]);
    h.get_by_label("c.pdf").click_modifiers(Modifiers::COMMAND);
    h.run_steps(1);
    assert_eq!(h.state().combine_selection(), [1, 3], "Ctrl/⌘ toggles one card");
    h.get_by_label("2 selected");
    assert!(h.get_by_label("b.pdf").accesskit_node().is_selected() == Some(true), "cards say they are selected");
    h.get_by_label("List view").click();
    h.run_steps(2);
    assert_eq!(h.state().combine_selection(), [1, 3]);
    h.get_by_label("Grid view").click();
    h.run_steps(2);
    assert_eq!(h.state().combine_selection(), [1, 3]);
    // The toolbar acts on it in the grid too.
    h.get_by_label("Remove 2 files").click();
    h.run_steps(2);
    assert_eq!(names(&h), ["a.pdf", "c.pdf", "e.pdf"]);
}

#[test]
fn dragging_a_card_moves_it_and_undo_puts_it_back() {
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 1), ("c.pdf", 1), ("d.pdf", 1)]);
    // Sorted first: a move by hand ends the sort, as in the list.
    h.state_mut().combine_tab.sort = Some((pdfcraft_ui_egui::SortKey::Name, true));
    let (a, c) = (card(&h, "a.pdf"), card(&h, "c.pdf"));
    drag(&mut h, a.center(), c.center() + vec2(c.width() * 0.3, 0.0));
    assert_eq!(names(&h), ["b.pdf", "c.pdf", "a.pdf", "d.pdf"]);
    assert_eq!(h.state().combine_selection(), [2], "the moved card is selected");
    assert_eq!(h.state().combine_tab.sort, None);
    // The list shows the same order.
    h.get_by_label("List view").click();
    h.run_steps(2);
    let rows: Vec<f32> = ["b.pdf", "c.pdf", "a.pdf", "d.pdf"].iter().map(|n| h.get_by_label(n).rect().top()).collect();
    assert!(rows.windows(2).all(|w| w[0] < w[1]), "the table lists them in the same order: {rows:?}");
    h.get_by_label("Grid view").click();
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert_eq!(names(&h), ["a.pdf", "b.pdf", "c.pdf", "d.pdf"]);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run_steps(2);
    assert_eq!(names(&h), ["b.pdf", "c.pdf", "a.pdf", "d.pdf"]);
}

#[test]
fn a_dragged_selection_moves_together_to_the_first_and_last_gaps() {
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 1), ("c.pdf", 1), ("d.pdf", 1), ("e.pdf", 1)]);
    // A selection with a gap in it.
    h.state_mut().select_combine_rows(&[1, 3]);
    h.run_steps(1);
    let (b, e) = (card(&h, "b.pdf"), card(&h, "e.pdf"));
    drag(&mut h, b.center(), e.center() + vec2(e.width() * 0.3, 0.0));
    assert_eq!(names(&h), ["a.pdf", "c.pdf", "e.pdf", "b.pdf", "d.pdf"], "to the end, in their order");
    assert_eq!(h.state().combine_selection(), [3, 4]);
    // And back to the very start.
    let (d, a) = (card(&h, "d.pdf"), card(&h, "a.pdf"));
    drag(&mut h, d.center(), a.center() - vec2(a.width() * 0.3, 0.0));
    assert_eq!(names(&h), ["b.pdf", "d.pdf", "a.pdf", "c.pdf", "e.pdf"]);
    // A file that isn't selected moves alone, and becomes the selection.
    let (c, b) = (card(&h, "c.pdf"), card(&h, "b.pdf"));
    drag(&mut h, c.center(), b.center() - vec2(b.width() * 0.3, 0.0));
    assert_eq!(names(&h), ["c.pdf", "b.pdf", "d.pdf", "a.pdf", "e.pdf"]);
    assert_eq!(h.state().combine_selection(), [0]);
}

#[test]
fn a_drop_that_changes_nothing_or_lands_outside_the_grid_leaves_no_undo_step() {
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 1), ("c.pdf", 1)]);
    // The same drag gesture, to a real gap, does move (and is undone).
    let (b, c) = (card(&h, "b.pdf"), card(&h, "c.pdf"));
    drag(&mut h, b.center(), c.center() + vec2(c.width() * 0.3, 0.0));
    assert_eq!(names(&h), ["a.pdf", "c.pdf", "b.pdf"]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert_eq!(names(&h), ["a.pdf", "b.pdf", "c.pdf"]);
    h.state_mut().combine_tab.sort = Some((pdfcraft_ui_egui::SortKey::Name, true));
    let b = card(&h, "b.pdf");
    // Onto its own place.
    drag(&mut h, b.center(), b.center() + vec2(b.width() * 0.3, 0.0));
    assert_eq!(names(&h), ["a.pdf", "b.pdf", "c.pdf"]);
    // Onto the toolbar, outside the grid.
    let toolbar = h.get_by_label("Grid view").rect().center();
    drag(&mut h, b.center(), toolbar);
    assert_eq!(names(&h), ["a.pdf", "b.pdf", "c.pdf"]);
    assert!(h.state().combine_tab.sort.is_some(), "a move that didn't happen keeps the sort");
    // The only step is still the adding of the files: one undo empties the list.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert!(names(&h).is_empty());
    assert!(h.get_by_label("Undo").accesskit_node().is_disabled());
}

#[test]
fn arrow_keys_move_by_card_and_by_row_and_shift_extends() {
    let files: Vec<(String, usize)> = (0..9).map(|i| (format!("f{i}.pdf"), 1)).collect();
    let named: Vec<(&str, usize)> = files.iter().map(|(n, p)| (n.as_str(), *p)).collect();
    // Narrow enough for several rows.
    let mut h = grid_of(named.iter().map(|(n, p)| (*n, fixture(*p))).collect(), vec2(760.0, 1200.0));
    let top = card(&h, "f0.pdf").top();
    let cols = (0..9).filter(|i| (card(&h, &format!("f{i}.pdf")).top() - top).abs() < 1.0).count();
    assert!((2..9).contains(&cols), "several cards per row and several rows ({cols} per row)");
    h.get_by_label("f0.pdf").click();
    h.run_steps(1);
    h.key_press(Key::ArrowRight);
    h.run_steps(1);
    assert_eq!(h.state().combine_selection(), [1]);
    h.key_press(Key::ArrowDown);
    h.run_steps(1);
    assert_eq!(h.state().combine_selection(), [1 + cols], "down one row, same column");
    h.key_press(Key::ArrowUp);
    h.run_steps(1);
    assert_eq!(h.state().combine_selection(), [1]);
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowDown);
    h.run_steps(1);
    assert_eq!(h.state().combine_selection(), (1..=1 + cols).collect::<Vec<_>>(), "Shift extends from the anchor");
    // On the first row, Up stays.
    h.get_by_label("f0.pdf").click();
    h.run_steps(1);
    h.key_press(Key::ArrowUp);
    h.run_steps(1);
    assert_eq!(h.state().combine_selection(), [0]);
    // Alt+arrows still move the selection.
    h.key_press_modifiers(Modifiers::ALT, Key::ArrowDown);
    h.run_steps(1);
    assert_eq!(names(&h).first().map(String::as_str), Some("f1.pdf"));
}

#[test]
fn shift_arrows_extend_the_selection_in_the_list_too() {
    // Regression: a plain arrow matched with Shift held, so Shift+Up/Down only stepped.
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 1), ("c.pdf", 1), ("d.pdf", 1)]);
    h.state_mut().set_option("combine-view", "list").unwrap();
    h.run_steps(2);
    h.get_by_label("b.pdf").click();
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowDown);
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowDown);
    h.run_steps(1);
    assert_eq!(h.state().combine_selection(), [1, 2, 3]);
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowUp);
    h.run_steps(1);
    assert_eq!(h.state().combine_selection(), [1, 2]);
    h.key_press(Key::ArrowUp);
    h.run_steps(1);
    assert_eq!(h.state().combine_selection(), [1], "a plain arrow still steps");
}

#[test]
fn thumbnails_show_the_first_page_each_file_takes() {
    let mut h = grid_of_pages(&[("three.pdf", 3), ("one.pdf", 1)]);
    h.state_mut().combine_draft[0].range = "3, 1".into();
    assert!(settle(&mut h, |app| app.combine_thumbnails().iter().all(ready)), "{:?}", h.state().combine_thumbnails());
    let thumbs = h.state().combine_thumbnails();
    assert!(matches!(thumbs[0], CombineThumb::Ready { page: 2, .. }), "\"3, 1\" starts with page 3: {:?}", thumbs[0]);
    assert!(matches!(thumbs[1], CombineThumb::Ready { page: 0, .. }));
    let CombineThumb::Ready { width, height, .. } = thumbs[1] else { unreachable!() };
    assert!(width > 0 && height > 0 && width <= 768 && height <= 768, "{width}×{height}");
    // A new range is a new thumbnail, never the old page.
    h.state_mut().combine_draft[0].range = "2".into();
    assert!(settle(&mut h, |app| matches!(app.combine_thumbnails()[0], CombineThumb::Ready { page: 1, .. })));
    // A range that can't be used shows no page at all.
    h.state_mut().combine_draft[0].range = "9".into();
    h.run_steps(2);
    assert_eq!(h.state().combine_thumbnails()[0], CombineThumb::None);
}

#[test]
fn a_locked_file_shows_no_page_until_unlocked_and_undo_locks_it_again() {
    let mut h = grid_of(vec![("one.pdf", fixture(1)), ("secret.pdf", protected("pw", "owner"))], vec2(1400.0, 900.0));
    assert!(settle(&mut h, |app| ready(&app.combine_thumbnails()[0])));
    assert_eq!(h.state().combine_thumbnails()[1], CombineThumb::None, "nothing is drawn of a locked file");
    assert!(h.state_mut().combine_unlock_rows(&[1], "pw"));
    assert!(settle(&mut h, |app| ready(&app.combine_thumbnails()[1])), "{:?}", h.state().combine_thumbnails());
    // Undo brings back the locked file: its unlocked thumbnail goes with the password.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(3);
    assert!(h.state().combine_draft[1].lock.is_some());
    assert_eq!(h.state().combine_thumbnails()[1], CombineThumb::None);
    // Unlocked again (a new branch of the history): drawn again, from this unlock.
    assert!(h.state_mut().combine_unlock_rows(&[1], "pw"));
    assert!(settle(&mut h, |app| ready(&app.combine_thumbnails()[1])), "{:?}", h.state().combine_thumbnails());
}

#[test]
fn many_files_render_only_what_is_in_view_a_few_at_a_time() {
    // The first few are slow to draw, so renders are still under way while the frames are watched
    // (small pages can all be drawn before the first look).
    let files: Vec<(String, Vec<u8>)> = (0..200).map(|i| (format!("scan{i:03}.pdf"), if i < 6 { heavy() } else { fixture(1) })).collect();
    let mut h = grid_after(files.iter().map(|(n, b)| (n.as_str(), b.clone())).collect(), vec2(1200.0, 800.0), 0);
    let mut most = 0;
    let mut done = false;
    for _ in 0..800 {
        h.run_steps(1);
        // Every render thread counts, including those of finished renders not yet exited.
        most = most.max(h.state_mut().combine_thumbnail_threads());
        let app = h.state();
        if app.combine_thumbnails().first().is_some_and(ready) && app.combine_thumbnails_rendering() == 0 {
            done = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(done, "the first cards render");
    assert!(most > 0, "renders were seen under way");
    assert!(most <= 3, "never more than three render threads at once ({most})");
    let thumbs = h.state().combine_thumbnails();
    let drawn = thumbs.iter().filter(|t| ready(t)).count();
    assert!(drawn > 0 && drawn < 60, "only the cards in view (and a row either side) are rendered: {drawn} of 200");
    assert_eq!(thumbs.last(), Some(&CombineThumb::None), "the last card, far out of view, isn't rendered");
}

#[test]
fn removing_a_file_while_it_renders_and_closing_the_tab_leave_nothing_behind() {
    let files: Vec<(String, Vec<u8>)> = (0..12).map(|i| (format!("f{i}.pdf"), fixture(2))).collect();
    let mut h = grid_of(files.iter().map(|(n, b)| (n.as_str(), b.clone())).collect(), vec2(1400.0, 900.0));
    // Remove files straight away, while their thumbnails may still be rendering.
    h.state_mut().select_combine_rows(&[0, 1, 2]);
    h.key_press(Key::Delete);
    assert!(settle(&mut h, |app| app.combine_thumbnails_rendering() == 0 && app.combine_thumbnails().iter().take(4).all(ready)));
    assert_eq!(h.state().combine_thumbnails().len(), 9);
    // Undo brings them back, and their thumbnails follow.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    assert!(settle(&mut h, |app| app.combine_draft.len() == 12 && app.combine_thumbnails().iter().take(3).all(ready)));
    // Closing the tab forgets everything.
    h.state_mut().select_combine_rows(&[0]);
    h.key_press(Key::Delete);
    h.run_steps(1);
    h.state_mut().close_combine_tab();
    h.run_steps(2);
    assert!(h.state().combine_thumbnails().is_empty());
    assert_eq!(h.state().combine_thumbnails_rendering(), 0);
    assert_eq!(h.state().combine_thumbnail_textures(), 0, "no thumbnail texture is kept");
    assert!(settle(&mut h, |app| app.combine_thumbnail_threads_exited()), "every render thread exits");
}

#[test]
fn switching_to_the_list_lets_renders_under_way_finish() {
    let files: Vec<(String, Vec<u8>)> = (0..6).map(|i| (format!("f{i}.pdf"), heavy())).collect();
    let mut h = grid_after_one(files.iter().map(|(n, b)| (n.as_str(), b.clone())).collect(), vec2(1400.0, 900.0));
    assert!(h.state().combine_thumbnails_rendering() > 0, "renders are under way");
    // Straight to the list, while the grid's first thumbnails are still rendering.
    h.state_mut().set_option("combine-view", "list").unwrap();
    assert!(settle(&mut h, |app| app.combine_thumbnails_rendering() == 0), "renders finish and let go of their threads");
    for _ in 0..20 {
        h.run_steps(1);
        assert_eq!(h.state().combine_thumbnails_rendering(), 0, "nothing new starts in the list");
    }
    assert!(h.state().combine_thumbnails().contains(&CombineThumb::None), "the rest wait for the grid");
    assert!(settle(&mut h, |app| app.combine_thumbnail_threads_exited()), "and their threads exit");
    // Back in the grid, the rest follow.
    h.get_by_label("Grid view").click();
    assert!(settle(&mut h, |app| app.combine_thumbnails().iter().all(ready)));
}

#[test]
fn combine_follows_the_order_shown_in_the_grid() {
    let mut h = grid_of_pages(&[("one.pdf", 2), ("two.pdf", 1), ("three.pdf", 1)]);
    let (three, one) = (card(&h, "three.pdf"), card(&h, "one.pdf"));
    drag(&mut h, three.center(), one.center() - vec2(one.width() * 0.3, 0.0));
    assert_eq!(names(&h), ["three.pdf", "one.pdf", "two.pdf"]);
    h.get_by_label("Combine").click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.views.len(), 1, "the result opens");
    assert_eq!(texts_of(app, 0), ["Page 1", "Page 1", "Page 2", "Page 1"]);
    let doc = app.session.get(app.views[0].id).unwrap();
    assert_eq!(doc.info.outline.iter().map(|o| o.title.as_str()).collect::<Vec<_>>(), ["three", "one", "two"]);
}

#[test]
fn long_and_unusual_names_are_shortened_on_the_card_but_kept_whole() {
    let long = "Überweisungsbestätigung für das dritte Quartal 2026 – Endfassung (unterschrieben).pdf";
    let mut h = grid_of(vec![(long, fixture(1)), ("日本語のファイル名.pdf", fixture(1)), ("a&b <c> 'd'.pdf", fixture(1))], vec2(1400.0, 900.0));
    // The card's label is the whole name; only the drawing is shortened.
    h.get_by_label(long);
    h.get_by_label("日本語のファイル名.pdf");
    h.get_by_label("a&b <c> 'd'.pdf");
    assert!(settle(&mut h, |app| app.combine_thumbnails().iter().all(ready)));
}

#[test]
fn long_thin_pages_get_thumbnails_within_the_size_limits() {
    // Regression: the scale came from the rounded width alone, so a strip 1 pt wide and 999 pt
    // long asked for a 9×8192 raster, over the GPU texture limit (a panic in load_texture).
    let files = vec![
        ("strip.pdf", fixture_sized(1, 1.0, 999.0)),
        ("tall.pdf", fixture_sized(1, 10.0, 3000.0)),
        ("wide.pdf", fixture_sized(1, 3000.0, 10.0)),
        ("big.pdf", fixture_sized(1, 14400.0, 14400.0)),
    ];
    let mut h = grid_of(files, vec2(1400.0, 900.0));
    assert!(settle(&mut h, |app| app.combine_thumbnails().iter().all(|t| *t != CombineThumb::None && *t != CombineThumb::Pending)));
    for t in h.state().combine_thumbnails() {
        match t {
            CombineThumb::Ready { width, height, .. } => assert!(width <= 768 && height <= 768, "{width}×{height}"),
            other => panic!("each strip gets a thumbnail: {other:?}"),
        }
    }
}

#[test]
fn a_bad_page_range_on_a_card_says_where_to_fix_it() {
    let mut h = grid_of_pages(&[("a.pdf", 3), ("b.pdf", 1)]);
    h.state_mut().combine_draft[0].range = "9".into();
    h.run_steps(2);
    let a = card(&h, "a.pdf");
    h.hover_at(a.center());
    for _ in 0..60 {
        h.run_steps(1);
        if h.query_by_label_contains("Switch to List view to change the pages it takes").is_some() {
            break;
        }
    }
    h.get_by_label_contains("Switch to List view to change the pages it takes");
}

#[test]
fn a_file_whose_pages_cant_be_measured_says_there_is_no_preview() {
    // An RC4 owner password opens the file for combining but not for reading it here, so the
    // page's size is unknown and nothing is drawn. Regression: the card was a blank white page.
    // The fixture must keep the size unknown: if the inspector learns RC4 owner passwords, this
    // needs another file it can't measure (the check below says so rather than pass vacuously).
    let mut h =
        grid_of(vec![("one.pdf", fixture(1)), ("old.pdf", protected_with(pdfcraft_cos::Algorithm::Rc4_128, "pw", "owner"))], vec2(1400.0, 900.0));
    assert!(h.state_mut().combine_unlock_rows(&[1], "owner"));
    assert!(settle(&mut h, |app| ready(&app.combine_thumbnails()[0])));
    assert!(h.state().combine_draft[1].lock.is_none(), "unlocked for combining");
    assert_eq!(h.state().combine_thumbnails()[1], CombineThumb::None, "nothing rendered (is the page size still unknown?)");
    assert_eq!(h.state().combine_thumbnails_rendering(), 0);
    // Screen readers hear it too, not only in the tooltip.
    let description = |h: &Harness<'static, PdfCraftApp>, name: &str| h.get_by_label(name).accesskit_node().description().unwrap_or_default();
    assert!(description(&h, "old.pdf").contains("No preview"), "{}", description(&h, "old.pdf"));
    assert_eq!(description(&h, "one.pdf"), "1 page");
    let tip_of = |h: &mut Harness<'static, PdfCraftApp>, name: &str, text: &str| {
        let at = card(h, name).center();
        h.hover_at(at);
        (0..60).any(|_| {
            h.run_steps(1);
            h.query_by_label_contains(text).is_some()
        })
    };
    assert!(tip_of(&mut h, "old.pdf", "No preview"), "the card says why it shows no page");
    h.hover_at(egui::pos2(5.0, 890.0));
    h.run_steps(3);
    assert!(!tip_of(&mut h, "one.pdf", "No preview"), "a card with its page doesn't");
}

#[test]
fn thumbnails_under_way_finish_on_home_and_in_a_document_tab() {
    // Regression: they were only collected while the Combine page was drawn.
    let files: Vec<(String, Vec<u8>)> = (0..6).map(|i| (format!("f{i}.pdf"), heavy())).collect();
    let mut h = grid_after_one(files.iter().map(|(n, b)| (n.as_str(), b.clone())).collect(), vec2(1400.0, 900.0));
    assert!(h.state().combine_thumbnails_rendering() > 0, "renders are under way");
    // Home.
    h.state_mut().combine_tab.focused = false;
    h.run_steps(1);
    assert!(!h.state().combine_showing());
    assert!(settle(&mut h, |app| app.combine_thumbnails_rendering() == 0 && app.combine_thumbnail_threads_exited()));
    // A document tab: the same.
    h.state_mut().open_combine_tab();
    h.run_steps(1);
    h.state_mut().open_bytes("doc.pdf", None, fixture(1)).unwrap();
    h.run_steps(1);
    assert!(h.state().active.is_some());
    assert!(settle(&mut h, |app| app.combine_thumbnails_rendering() == 0 && app.combine_thumbnail_threads_exited()));
}

#[test]
fn a_focused_card_still_takes_the_arrow_keys() {
    // Regression: the keys were ignored whenever any widget, a card included, had focus.
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 1), ("c.pdf", 1)]);
    h.get_by_label("a.pdf").click();
    h.run_steps(1);
    h.get_by_label("a.pdf").focus();
    h.run_steps(1);
    h.key_press(Key::ArrowRight);
    h.run_steps(1);
    assert_eq!(h.state().combine_selection(), [1]);
}

#[test]
fn a_drag_is_dropped_when_the_list_changes_under_it() {
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 1), ("c.pdf", 1), ("d.pdf", 1)]);
    h.state_mut().select_combine_rows(&[3]);
    h.key_press(Key::Delete);
    h.run_steps(2);
    assert_eq!(names(&h), ["a.pdf", "b.pdf", "c.pdf"]);
    // A drag begins; undo brings d.pdf back while it is under way; then the card is dropped.
    let (a, c) = (card(&h, "a.pdf"), card(&h, "c.pdf"));
    let to = c.center() + vec2(c.width() * 0.3, 0.0);
    h.hover_at(a.center());
    h.run_steps(1);
    h.drag_at(a.center());
    h.run_steps(1);
    for k in 1..=3 {
        h.hover_at(a.center() + (to - a.center()) * (k as f32 / 6.0));
        h.run_steps(1);
    }
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(1);
    assert_eq!(names(&h), ["a.pdf", "b.pdf", "c.pdf", "d.pdf"]);
    h.hover_at(to);
    h.run_steps(1);
    h.drop_at(to);
    h.run_steps(3);
    assert_eq!(names(&h), ["a.pdf", "b.pdf", "c.pdf", "d.pdf"], "the gap it was aimed at no longer means the same: nothing moves");
}

#[test]
fn a_card_dragged_to_the_bottom_edge_scrolls_the_grid_and_still_drops() {
    let files: Vec<(String, Vec<u8>)> = (0..30).map(|i| (format!("f{i:02}.pdf"), fixture(1))).collect();
    let mut h = grid_of(files.iter().map(|(n, b)| (n.as_str(), b.clone())).collect(), vec2(820.0, 860.0));
    let from = card(&h, "f00.pdf").center();
    let totals = h.get_by_label_contains("30 files").rect();
    let edge = egui::pos2(from.x, totals.top() - 30.0);
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    h.hover_at(edge);
    // Held at the edge, the grid scrolls; the card dragged leaves the rows drawn.
    for _ in 0..80 {
        h.run_steps(1);
    }
    assert!(h.query_by_label("f00.pdf").is_none(), "scrolled away from the first row");
    // A card wholly in view (rows just outside it are drawn too, to scroll smoothly).
    let top = h.get_by_label("Grid view").rect().bottom() + 40.0;
    let (target_name, target) = (1..30)
        .map(|i| format!("f{i:02}.pdf"))
        .find_map(|n| h.query_by_label(&n).map(|c| c.rect()).filter(|r| r.top() > top && r.bottom() < totals.top() - 20.0).map(|r| (n, r)))
        .expect("a card in view");
    let to = target.center() - vec2(target.width() * 0.3, 0.0);
    h.hover_at(to);
    h.run_steps(1);
    h.drop_at(to);
    h.run_steps(3);
    let order = names(&h);
    let at = order.iter().position(|n| n == "f00.pdf").unwrap();
    assert!(at > 0, "moved from the start: {order:?}");
    assert_eq!(order.get(at + 1), Some(&target_name), "dropped before {target_name}: {order:?}");
}

#[test]
fn a_locked_card_offers_unlock_on_a_right_click() {
    let mut h = grid_of(vec![("one.pdf", fixture(1)), ("secret.pdf", protected("pw", "owner"))], vec2(1400.0, 900.0));
    h.get_by_label("secret.pdf").click_secondary();
    h.run_steps(2);
    // The menu's item comes after the toolbar's button of the same name.
    h.get_all_by_label("Unlock…").last().unwrap().click();
    h.run_steps(2);
    h.get_by_label("Unlock secret.pdf");
    assert_eq!(h.state().combine_selection(), [1], "the card right-clicked is the one unlocked");
}

fn disabled(h: &Harness<'static, PdfCraftApp>, label: &str) -> bool {
    h.get_by_label(label).accesskit_node().is_disabled()
}

/// Ctrl/⌘ with the wheel at `at` (a positive `dy` zooms in).
fn ctrl_wheel(h: &mut Harness<'static, PdfCraftApp>, at: Pos2, dy: f32) {
    h.hover_at(at);
    h.run_steps(1);
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, dy),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::COMMAND,
    });
    h.run_steps(3);
}

#[test]
fn the_toolbar_sizes_the_cards_within_limits_and_the_size_is_kept_in_the_settings() {
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 2)]);
    assert_eq!(h.state().combine_zoom, 1.0);
    let before = card(&h, "a.pdf");
    h.get_by_label("Larger pages").click();
    h.run_steps(3);
    assert_eq!(h.state().combine_zoom, 1.25);
    let after = card(&h, "a.pdf");
    assert!(after.width() > before.width() && after.height() > before.height(), "{before:?} → {after:?}");
    // Larger until it can't go further: 200%, and the button says so.
    for _ in 0..10 {
        if disabled(&h, "Larger pages") {
            break;
        }
        h.get_by_label("Larger pages").click();
        h.run_steps(2);
    }
    assert_eq!(h.state().combine_zoom, 2.0);
    assert!(disabled(&h, "Larger pages"));
    // The percentage resets it.
    h.get_by_label("Reset page size").click();
    h.run_steps(2);
    assert_eq!(h.state().combine_zoom, 1.0);
    for _ in 0..10 {
        if disabled(&h, "Smaller pages") {
            break;
        }
        h.get_by_label("Smaller pages").click();
        h.run_steps(2);
    }
    assert_eq!(h.state().combine_zoom, 0.6);
    let smallest = card(&h, "a.pdf");
    assert!(smallest.width() < before.width() && smallest.height() < before.height(), "{smallest:?}");
    // Kept in the settings, and read back; a damaged setting is clamped or ignored.
    let saved = h.state().persist();
    let mut fresh = PdfCraftApp::new();
    fresh.restore(&saved);
    assert_eq!(fresh.combine_zoom, 0.6);
    fresh.restore(r#"{"combine_zoom": 99}"#);
    assert_eq!(fresh.combine_zoom, 2.0);
    fresh.restore(r#"{"combine_zoom": "big"}"#);
    assert_eq!(fresh.combine_zoom, 2.0);
    // Scriptable in percent, with a clear refusal outside the range.
    let app = h.state_mut();
    app.set_option("combine-zoom", "150%").unwrap();
    assert_eq!(app.combine_zoom, 1.5);
    for bad in ["59", "201", "big", "NaN", "inf", ""] {
        assert!(app.set_option("combine-zoom", bad).is_err(), "{bad:?}");
    }
    assert_eq!(app.combine_zoom, 1.5);
    // The list has no card size.
    h.get_by_label("List view").click();
    h.run_steps(2);
    assert!(h.query_by_label("Larger pages").is_none() && h.query_by_label("Page size").is_none());
}

#[test]
fn the_slider_sizes_the_cards_from_smallest_to_largest() {
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 1)]);
    let slider = h.get_by_label("Page size").rect();
    drag(&mut h, slider.center(), slider.right_center() + vec2(40.0, 0.0));
    assert_eq!(h.state().combine_zoom, 2.0);
    let slider = h.get_by_label("Page size").rect();
    drag(&mut h, slider.center(), slider.left_center() - vec2(40.0, 0.0));
    assert_eq!(h.state().combine_zoom, 0.6);
    // The cards are still there and in order.
    assert_eq!(names(&h), ["a.pdf", "b.pdf"]);
    assert!(card(&h, "a.pdf").left() < card(&h, "b.pdf").left());
}

#[test]
fn ctrl_keys_and_ctrl_wheel_size_the_cards_and_leave_the_window_alone() {
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 1), ("c.pdf", 1)]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Equals);
    h.run_steps(3);
    assert_eq!(h.state().combine_zoom, 1.25);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Plus);
    h.run_steps(3);
    assert_eq!(h.state().combine_zoom, 1.5);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Minus);
    h.run_steps(3);
    assert_eq!(h.state().combine_zoom, 1.25);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num0);
    h.run_steps(3);
    assert_eq!(h.state().combine_zoom, 1.0);
    assert_eq!(h.ctx.zoom_factor(), 1.0, "the window's text keeps its size");
    // Ctrl/⌘ with the wheel over the grid: in, then out.
    let at = card(&h, "b.pdf").center();
    ctrl_wheel(&mut h, at, 120.0);
    let zoomed = h.state().combine_zoom;
    assert!(zoomed > 1.0, "{zoomed}");
    let at = card(&h, "b.pdf").center();
    ctrl_wheel(&mut h, at, -240.0);
    assert!(h.state().combine_zoom < zoomed, "{}", h.state().combine_zoom);
    // Not over the grid (the toolbar): nothing.
    let now = h.state().combine_zoom;
    let toolbar = h.get_by_label("Grid view").rect().center();
    ctrl_wheel(&mut h, toolbar, 120.0);
    assert_eq!(h.state().combine_zoom, now);
    // Nor in the list.
    h.get_by_label("List view").click();
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Equals);
    h.run_steps(2);
    assert_eq!(h.state().combine_zoom, now);
}

#[test]
fn zooming_keeps_the_card_under_the_pointer_in_place_and_thumbnails_never_go_blank() {
    let files: Vec<(String, Vec<u8>)> = (0..40).map(|i| (format!("f{i:02}.pdf"), fixture(1))).collect();
    let mut h = grid_of(files.iter().map(|(n, b)| (n.as_str(), b.clone())).collect(), vec2(1400.0, 900.0));
    // Scroll down a little, so the grid isn't simply at its top.
    let first = card(&h, "f00.pdf");
    h.hover_at(first.center());
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, -300.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(6);
    // A card fully in view, below the toolbar, with its thumbnail drawn.
    let toolbar = h.get_by_label("Grid view").rect().bottom();
    let pick = files
        .iter()
        .map(|(n, _)| n.as_str())
        .find(|n| h.query_by_label(n).is_some_and(|c| c.rect().top() > toolbar + 80.0 && c.rect().bottom() < 820.0))
        .expect("a card in view")
        .to_string();
    let i = names(&h).iter().position(|n| *n == pick).unwrap();
    assert!(settle(&mut h, |app| ready(&app.combine_thumbnails()[i])));
    let CombineThumb::Ready { height: small, .. } = h.state().combine_thumbnails()[i] else { unreachable!() };
    let before = card(&h, &pick);
    h.hover_at(before.center());
    h.run_steps(1);
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, 120.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::COMMAND,
    });
    // The wheel zooms smoothly over several frames; in every one the card stays put.
    let mut zooms = Vec::new();
    for frame in 0..12 {
        h.run_steps(1);
        zooms.push(h.state().combine_zoom);
        let now = card(&h, &pick);
        assert!((now.top() - before.top()).abs() < 2.0, "frame {frame}: the card under the pointer moved: {before:?} → {now:?}");
    }
    assert!(h.state().combine_zoom > 1.0, "{zooms:?}");
    // Its thumbnail never went blank: the smaller one stands in until the sharper one arrives.
    for _ in 0..400 {
        h.run_steps(1);
        match h.state().combine_thumbnails()[i] {
            CombineThumb::Ready { height, .. } if height > small => break,
            CombineThumb::Ready { .. } => {}
            ref other => panic!("went blank: {other:?}"),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(matches!(h.state().combine_thumbnails()[i], CombineThumb::Ready { height, .. } if height > small), "re-rendered larger");
}

#[test]
fn dragging_and_arrow_keys_work_at_the_smallest_and_largest_sizes() {
    let mut columns = Vec::new();
    for zoom in ["60", "200"] {
        let files: Vec<(String, Vec<u8>)> = (0..24).map(|i| (format!("f{i:02}.pdf"), fixture(1))).collect();
        let mut h = grid_of(files.iter().map(|(n, b)| (n.as_str(), b.clone())).collect(), vec2(1400.0, 900.0));
        h.state_mut().set_option("combine-zoom", zoom).unwrap();
        h.run_steps(3);
        let (a, c) = (card(&h, "f00.pdf"), card(&h, "f02.pdf"));
        drag(&mut h, a.center(), c.center() + vec2(c.width() * 0.3, 0.0));
        assert_eq!(&names(&h)[..4], ["f01.pdf", "f02.pdf", "f00.pdf", "f03.pdf"], "at {zoom}%");
        // Down moves one row: as many files as fit across.
        h.get_by_label("f01.pdf").click();
        h.run_steps(1);
        h.key_press(Key::ArrowDown);
        h.run_steps(2);
        let row = h.state().combine_selection();
        assert_eq!(row.len(), 1);
        assert!(row[0] > 1, "at {zoom}%: {row:?}");
        columns.push(row[0]);
    }
    assert!(columns[0] > columns[1], "more cards fit across when they are smaller: {columns:?}");
}

#[test]
fn zooming_quickly_never_runs_more_than_three_render_threads() {
    let files: Vec<(String, Vec<u8>)> = (0..60).map(|i| (format!("f{i:02}.pdf"), if i < 6 { heavy() } else { fixture(1) })).collect();
    let mut h = grid_after(files.iter().map(|(n, b)| (n.as_str(), b.clone())).collect(), vec2(1400.0, 900.0), 0);
    let mut most = 0;
    for k in 0..80 {
        let key = if (k / 5) % 2 == 0 { Key::Equals } else { Key::Minus };
        h.key_press_modifiers(Modifiers::COMMAND, key);
        h.run_steps(1);
        let threads = h.state_mut().combine_thumbnail_threads();
        most = most.max(threads);
        assert!(threads <= 3, "frame {k}: {threads} render threads");
        std::thread::sleep(Duration::from_millis(3));
    }
    assert!(most > 0, "renders were seen under way");
    // And it settles: the cards in view all get their thumbnails.
    assert!(settle(&mut h, |app| app.combine_thumbnails_rendering() == 0 && app.combine_thumbnails().first().is_some_and(ready)));
}

/// The grid with 40 one-page files, scrolled down by `dy`; and where the top of the grid is.
fn scrolled_grid(dy: f32) -> (Harness<'static, PdfCraftApp>, f32) {
    let files: Vec<(String, Vec<u8>)> = (0..40).map(|i| (format!("f{i:02}.pdf"), fixture(1))).collect();
    let mut h = grid_of(files.iter().map(|(n, b)| (n.as_str(), b.clone())).collect(), vec2(1400.0, 900.0));
    // Unscrolled, the first card sits a fixed space (12 points) below the top of the grid.
    let top = card(&h, "f00.pdf").top() - 12.0;
    let at = card(&h, "f00.pdf").center();
    h.hover_at(at);
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, -dy),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(8);
    (h, top)
}

/// The first card (in list order) still showing below `top`.
fn first_in_view(h: &Harness<'static, PdfCraftApp>, top: f32) -> String {
    names(h).into_iter().find(|n| h.query_by_label(n).is_some_and(|c| c.rect().bottom() > top)).expect("a card in view")
}

#[test]
fn zooming_from_the_toolbar_while_scrolled_keeps_the_top_cards_in_place() {
    // Regression: with the pointer over the toolbar, a card drawn just above the view (under the
    // toolbar) was taken as the one to keep in place, and the cards in view jumped.
    for how in ["keys", "button", "slider"] {
        let (mut h, top) = scrolled_grid(700.0);
        let anchor = first_in_view(&h, top);
        let before = card(&h, &anchor).top();
        let over_toolbar = egui::pos2(card(&h, &anchor).center().x, h.get_by_label("Grid view").rect().center().y);
        h.hover_at(over_toolbar);
        h.run_steps(1);
        match how {
            "keys" => h.key_press_modifiers(Modifiers::COMMAND, Key::Equals),
            "button" => h.get_by_label("Larger pages").click(),
            _ => {
                let s = h.get_by_label("Page size").rect();
                drag(&mut h, s.center(), s.right_center() + vec2(40.0, 0.0));
            }
        }
        h.run_steps(3);
        assert!(h.state().combine_zoom > 1.0, "{how}");
        let after = card(&h, &anchor).top();
        assert!((after - before).abs() < 2.0, "{how}: {anchor} moved from {before} to {after}");
    }
}

#[test]
fn zooming_out_at_the_bottom_leaves_no_gap_below_the_last_card() {
    let (mut h, _) = scrolled_grid(20_000.0);
    h.state_mut().set_option("combine-zoom", "200").unwrap();
    h.run_steps(2);
    // Over the middle of the grid.
    h.hover_at(egui::pos2(900.0, 600.0));
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, -20_000.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(8);
    let bottom = card(&h, "f39.pdf").bottom();
    h.get_by_label("Smaller pages").click();
    for frame in 0..6 {
        h.run_steps(1);
        let now = card(&h, "f39.pdf").bottom();
        assert!((now - bottom).abs() < 2.0, "frame {frame}: the last card left a gap below it ({bottom} → {now})");
    }
    assert_eq!(h.state().combine_zoom, 1.75);
}

#[test]
fn the_size_slider_takes_the_arrow_keys_when_it_has_the_keyboard() {
    // Regression: the grid's arrow keys moved the selection instead.
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 1), ("c.pdf", 1)]);
    h.get_by_label("a.pdf").click();
    h.run_steps(1);
    h.get_by_label("Page size").focus();
    h.run_steps(1);
    h.key_press(Key::ArrowRight);
    h.run_steps(2);
    assert!(h.state().combine_zoom > 1.0, "{}", h.state().combine_zoom);
    assert_eq!(h.state().combine_selection(), [0], "the selection stays");
    let node = h.get_by_label("Page size");
    assert!(node.accesskit_node().numeric_value().is_some_and(|v| v > 100.0), "read out as a percentage");
}

#[test]
fn the_toolbar_fits_the_smallest_window() {
    // 820 points is the narrowest the window goes.
    let mut h = grid_after(vec![("a.pdf", fixture(1)), ("b.pdf", fixture(1))], vec2(820.0, 700.0), 4);
    h.get_by_label("a.pdf").click();
    h.run_steps(1);
    h.get_by_label("b.pdf").click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    let controls: Vec<egui::Rect> = [
        "Add open documents",
        "Undo",
        "Redo",
        "Grid view",
        "List view",
        "2 selected",
        "Smaller pages",
        "Page size",
        "Reset page size",
        "Larger pages",
    ]
    .iter()
    .map(|l| h.get_by_label(l).rect())
    .collect();
    for (i, a) in controls.iter().enumerate() {
        assert!(a.right() <= 820.0, "{i}: {a:?} is off the window");
        for b in controls.iter().skip(i + 1) {
            assert!(!a.intersects(*b), "{a:?} overlaps {b:?}");
        }
    }
}

/// The pointer over a card (its centre, below the bar of actions at the top of its page).
fn hover_card(h: &mut Harness<'static, PdfCraftApp>, name: &str) {
    let at = card(h, name).center();
    h.hover_at(at);
    h.run_steps(2);
}

/// A primary click at a point, as a user makes one.
fn click_at(h: &mut Harness<'static, PdfCraftApp>, at: Pos2) {
    h.hover_at(at);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(1);
    }
    h.run_steps(1);
}

#[test]
fn hovering_a_card_shows_its_trash_and_magnifier_and_only_then() {
    let mut h = grid_of(vec![("a.pdf", fixture(1)), ("b.pdf", fixture(3)), ("secret.pdf", protected("pw", "owner"))], vec2(1400.0, 900.0));
    h.hover_at(egui::pos2(5.0, 890.0));
    h.run_steps(2);
    assert!(h.query_by_label("Preview").is_none() && h.query_by_label("Remove a.pdf").is_none(), "nothing until a card is hovered");
    hover_card(&mut h, "a.pdf");
    h.get_by_label("Preview");
    h.get_by_label("Remove a.pdf");
    // Over the bar itself it stays (the pointer is still on the card).
    let bar = h.get_by_label("Preview").rect().center();
    h.hover_at(bar);
    h.run_steps(2);
    h.get_by_label("Remove a.pdf");
    // Another card: its own.
    hover_card(&mut h, "b.pdf");
    h.get_by_label("Remove b.pdf");
    assert!(h.query_by_label("Remove a.pdf").is_none());
    // A locked file has nothing to show large, but can still be removed.
    hover_card(&mut h, "secret.pdf");
    h.get_by_label("Remove secret.pdf");
    assert!(h.query_by_label("Preview").is_none(), "no magnifier on a locked file");
}

#[test]
fn the_card_trash_removes_only_that_file_and_undo_brings_it_back() {
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 1), ("c.pdf", 1), ("d.pdf", 1)]);
    h.get_by_label("b.pdf").click();
    h.run_steps(1);
    h.get_by_label("c.pdf").click_modifiers(Modifiers::COMMAND);
    h.run_steps(1);
    // The trash of a card that isn't selected: that file only, the selection stays.
    hover_card(&mut h, "a.pdf");
    h.get_by_label("Remove a.pdf").click();
    h.run_steps(2);
    assert_eq!(names(&h), ["b.pdf", "c.pdf", "d.pdf"]);
    assert_eq!(h.state().combine_selection(), [0, 1], "b and c stay selected");
    // The trash of a selected card: still that file only.
    hover_card(&mut h, "b.pdf");
    h.get_all_by_label("Remove b.pdf").last().unwrap().click();
    h.run_steps(2);
    assert_eq!(names(&h), ["c.pdf", "d.pdf"]);
    assert_eq!(h.state().combine_selection(), [0], "c stays selected");
    // One undo step each.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert_eq!(names(&h), ["b.pdf", "c.pdf", "d.pdf"]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert_eq!(names(&h), ["a.pdf", "b.pdf", "c.pdf", "d.pdf"]);
}

#[test]
fn the_magnifier_shows_the_file_large_and_flips_through_the_pages_it_adds() {
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 4)]);
    h.state_mut().combine_draft[1].range = "3, 1".into();
    h.get_by_label("a.pdf").click();
    h.run_steps(1);
    assert!(settle(&mut h, |app| ready(&app.combine_thumbnails()[1])));
    let CombineThumb::Ready { width: card_width, .. } = h.state().combine_thumbnails()[1] else { unreachable!() };
    hover_card(&mut h, "b.pdf");
    h.get_by_label("Preview").click();
    h.run_steps(2);
    // The card under it says nothing over it (its tooltip, its actions).
    for _ in 0..150 {
        h.run_steps(1);
    }
    assert!(h.query_by_label_contains("4 pages ·").is_none(), "no card tooltip over the preview");
    assert!(h.query_by_label("Remove b.pdf").is_none(), "no card actions over the preview");
    // The first page it adds: page 3.
    let (name, page, _) = h.state().combine_preview().expect("shown large");
    assert_eq!((name.as_str(), page), ("b.pdf", 2));
    h.get_by_label("Page 1 of 2");
    assert!(disabled(&h, "Previous page"));
    // Rendered larger than its card.
    assert!(
        settle(&mut h, |app| matches!(app.combine_preview(), Some((_, _, CombineThumb::Ready { width, .. })) if width > card_width)),
        "{:?}",
        h.state().combine_preview()
    );
    // The arrow keys flip, and stop at the ends; the grid's selection doesn't move.
    h.key_press(Key::ArrowRight);
    h.run_steps(2);
    assert_eq!(h.state().combine_preview().map(|p| p.1), Some(0), "then page 1");
    h.get_by_label("Page 2 of 2");
    assert!(disabled(&h, "Next page"));
    h.key_press(Key::ArrowRight);
    h.run_steps(2);
    assert_eq!(h.state().combine_preview().map(|p| p.1), Some(0));
    h.get_by_label("Previous page").click();
    h.run_steps(2);
    assert_eq!(h.state().combine_preview().map(|p| p.1), Some(2));
    assert_eq!(h.state().combine_selection(), [0]);
    // Esc closes it; so do the close button and a click outside.
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state().combine_preview().is_none());
    for how in ["close", "outside"] {
        hover_card(&mut h, "b.pdf");
        h.get_by_label("Preview").click();
        h.run_steps(2);
        assert!(h.state().combine_preview().is_some(), "{how}");
        match how {
            "close" => h.get_by_label("Close").click(),
            _ => click_at(&mut h, egui::pos2(6.0, 6.0)),
        }
        h.run_steps(2);
        assert!(h.state().combine_preview().is_none(), "{how}");
    }
    assert_eq!(names(&h), ["a.pdf", "b.pdf"], "nothing else changed");
}

#[test]
fn a_right_click_on_a_card_offers_preview_and_remove() {
    let mut h = grid_of_pages(&[("a.pdf", 2), ("b.pdf", 1)]);
    h.get_by_label("a.pdf").click_secondary();
    h.run_steps(2);
    // The menu's items come after the card's own buttons of the same name.
    h.get_all_by_label("Preview").last().unwrap().click();
    h.run_steps(2);
    assert_eq!(h.state().combine_preview().map(|p| (p.0, p.1)), Some(("a.pdf".to_string(), 0)));
    h.key_press(Key::Escape);
    h.run_steps(2);
    h.get_by_label("b.pdf").click_secondary();
    h.run_steps(2);
    h.get_all_by_label("Remove b.pdf").last().unwrap().click();
    h.run_steps(2);
    assert_eq!(names(&h), ["a.pdf"]);
}

#[test]
fn the_preview_closes_when_its_file_leaves_and_keeps_to_three_render_threads() {
    let files: Vec<(String, Vec<u8>)> = (0..12).map(|i| (format!("f{i:02}.pdf"), heavy())).collect();
    let mut h = grid_after_one(files.iter().map(|(n, b)| (n.as_str(), b.clone())).collect(), vec2(1400.0, 900.0));
    hover_card(&mut h, "f03.pdf");
    h.get_by_label("Preview").click();
    let (mut most, mut rendered) = (0, false);
    for frame in 0..60 {
        h.run_steps(1);
        let threads = h.state_mut().combine_thumbnail_threads();
        most = most.max(threads);
        assert!(threads <= 3, "frame {frame}: {threads} render threads");
        // The large page itself is rendered (under way, or done) among them.
        rendered |= matches!(h.state().combine_preview(), Some((_, _, CombineThumb::Pending | CombineThumb::Ready { .. })));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(most > 0);
    assert!(rendered, "the preview's own render ran: {:?}", h.state().combine_preview());
    // The file leaves the list (as undo of its adding would): the preview closes, nothing stays.
    h.state_mut().combine_draft.retain(|f| f.name != "f03.pdf");
    h.run_steps(2);
    assert!(h.state().combine_preview().is_none());
    assert!(settle(&mut h, |app| app.combine_thumbnails_rendering() == 0 && app.combine_thumbnail_threads_exited()));
}

#[test]
fn space_shows_the_selected_file_large_in_the_grid_and_the_list() {
    // The way to the preview without a pointer (screen readers, the keyboard).
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 2), ("c.pdf", 1)]);
    h.key_press(Key::Space);
    h.run_steps(2);
    assert!(h.state().combine_preview().is_none(), "nothing selected, nothing shown");
    h.get_by_label("a.pdf").click();
    h.run_steps(1);
    h.key_press(Key::ArrowRight);
    h.run_steps(1);
    h.key_press(Key::Space);
    h.run_steps(2);
    assert_eq!(h.state().combine_preview().map(|p| p.0), Some("b.pdf".to_string()), "the file the keyboard is on");
    // While it shows, the grid's keys wait: an arrow flips its pages, Delete removes nothing.
    h.key_press(Key::ArrowRight);
    h.key_press(Key::Delete);
    h.run_steps(2);
    assert_eq!(h.state().combine_preview().map(|p| p.1), Some(1));
    assert_eq!(names(&h).len(), 3);
    assert_eq!(h.state().combine_selection(), [1]);
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state().combine_preview().is_none());
    // The list too.
    h.get_by_label("List view").click();
    h.run_steps(2);
    h.key_press(Key::Space);
    h.run_steps(2);
    assert_eq!(h.state().combine_preview().map(|p| p.0), Some("b.pdf".to_string()));
}

#[test]
fn flipping_quickly_ends_on_the_page_shown_and_leaving_the_tab_closes_the_preview() {
    let mut h = grid_of_pages(&[("a.pdf", 6)]);
    hover_card(&mut h, "a.pdf");
    h.get_by_label("Preview").click();
    h.run_steps(1);
    // A page a frame, faster than they render.
    for _ in 0..4 {
        h.key_press(Key::ArrowRight);
        h.run_steps(1);
    }
    assert_eq!(h.state().combine_preview().map(|p| p.1), Some(4));
    assert!(
        settle(&mut h, |app| matches!(app.combine_preview(), Some((_, 4, CombineThumb::Ready { page: 4, .. })))),
        "the render shown is of the page shown: {:?}",
        h.state().combine_preview()
    );
    // Home, then back: the preview is gone, not waiting there.
    h.state_mut().combine_tab.focused = false;
    h.run_steps(2);
    assert!(h.state().combine_preview().is_none());
    h.state_mut().open_combine_tab();
    h.run_steps(2);
    assert!(h.state().combine_preview().is_none());
    assert!(h.query_by_label("Page 5 of 6").is_none());
}

#[test]
fn space_on_a_focused_button_presses_it_and_previews_only_what_can_be_shown() {
    // Regression: with a file selected, Space opened the preview whatever had the keyboard.
    let mut h =
        grid_of(vec![("a.pdf", fixture(2)), ("old.pdf", protected_with(pdfcraft_cos::Algorithm::Rc4_128, "pw", "owner"))], vec2(1400.0, 900.0));
    h.get_by_label("a.pdf").click();
    h.run_steps(1);
    h.get_by_label("List view").focus();
    h.run_steps(1);
    h.key_press(Key::Space);
    h.run_steps(2);
    assert_eq!(h.state().combine_view, CombineView::List, "the focused button was pressed");
    assert!(h.state().combine_preview().is_none());
    // A file whose pages can't be measured has nothing to show large (its card has no magnifier).
    h.get_by_label("Grid view").click();
    h.run_steps(1);
    assert!(h.state_mut().combine_unlock_rows(&[1], "owner"));
    h.run_steps(2);
    h.get_by_label("old.pdf").click();
    h.run_steps(1);
    h.key_press(Key::Space);
    h.run_steps(2);
    assert!(h.state().combine_preview().is_none(), "nothing to show");
}

#[test]
fn a_drag_held_while_the_tab_closes_and_reopens_moves_nothing() {
    // Regression: closing the tab started the list's revisions over, so a drag begun before could
    // match a new list and move one of its files.
    let mut h = grid_of_pages(&[("a.pdf", 1), ("b.pdf", 1), ("c.pdf", 1)]);
    let (a, c) = (card(&h, "a.pdf"), card(&h, "c.pdf"));
    h.hover_at(a.center());
    h.run_steps(1);
    h.drag_at(a.center());
    h.run_steps(1);
    h.hover_at(a.center() + vec2(30.0, 0.0));
    h.run_steps(1);
    // While held: the tab closes, and the same files come back.
    h.state_mut().close_combine_tab();
    h.state_mut().use_files(FilePurpose::Combine, ["a.pdf", "b.pdf", "c.pdf"].iter().map(|n| (n.to_string(), fixture(1))).collect());
    h.run_steps(2);
    let to = c.center() + vec2(c.width() * 0.3, 0.0);
    h.hover_at(to);
    h.run_steps(1);
    h.drop_at(to);
    h.run_steps(3);
    assert_eq!(names(&h), ["a.pdf", "b.pdf", "c.pdf"], "the old drag moves nothing in the new list");
}
