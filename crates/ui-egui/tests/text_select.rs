//! Issue #295: quick clicks on text widen the selection, as in Acrobat: two clicks select a
//! word, three the line, four all the text on the page.

use egui::{Event, Modifiers, PointerButton, Pos2};
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

/// One 300×200 page with two lines of Helvetica text.
fn two_lines() -> Vec<u8> {
    let content = "BT /F1 14 Tf 20 150 Td (The quick brown fox) Tj 0 -24 Td (jumps over the dog) Tj ET";
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF",
        content.len()
    )
    .into_bytes()
}

/// The page on screen and the pointer over the "u" of "quick". Frames are 1/60 s: a click takes
/// two frames (press, release), and egui counts a double-click only within 0.3 s.
fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> (Harness<'static, PdfCraftApp>, Pos2) {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("lines.pdf", None, two_lines()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("author", "Tester").unwrap();
        setup(&mut app);
        app
    });
    for _ in 0..200 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let at = h.state().views[0].glyph_screen_pos(0, 5).expect("text layer loaded");
    h.hover_at(at);
    h.run_steps(1);
    (h, at)
}

/// Press and release without moving or leaving the page.
fn click(h: &mut Harness<'static, PdfCraftApp>, pos: Pos2) {
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(1);
}

#[test]
fn quick_clicks_select_a_word_then_the_line_then_the_page() {
    let (mut h, at) = harness(|_| {});
    let page = "The quick brown fox\njumps over the dog";
    for (n, want) in [None, Some("quick"), Some("The quick brown fox"), Some(page), Some(page)].into_iter().enumerate() {
        click(&mut h, at);
        assert_eq!(h.state().views[0].selected_text().as_deref(), want, "after {} clicks", n + 1);
    }
    // A pause ends the run: the next double-click selects a word again.
    h.run_steps(60);
    click(&mut h, at);
    click(&mut h, at);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick"));
}

#[test]
fn quick_clicks_with_the_highlighter_mark_only_the_word() {
    let (mut h, at) = harness(|app| assert!(app.execute("comment.highlight")));
    for _ in 0..4 {
        click(&mut h, at);
    }
    h.run_steps(3);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    let marks: Vec<(&str, usize)> = doc.info.annotations.iter().map(|a| (a.subtype.as_str(), a.quads.len())).collect();
    assert_eq!(marks, [("Highlight", 1)], "one highlight, on the double-clicked word");
    assert!(app.views[0].selected_text().is_none());
}

/// Press at `from`, move to `to` and release there.
fn drag(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2) {
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from + (to - from) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(1);
}

/// Click at `pos` with Shift held.
fn shift_click(h: &mut Harness<'static, PdfCraftApp>, pos: Pos2) {
    h.event(Event::ModifiersChanged(Modifiers::SHIFT));
    h.hover_at(pos);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::SHIFT });
    }
    h.run_steps(1);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
}

/// Issue #527: ⇧-click extends the text selection to the click, keeping its anchor.
#[test]
fn shift_click_extends_the_selection_from_its_anchor() {
    let (mut h, _) = harness(|_| {});
    let glyph = |h: &Harness<'static, PdfCraftApp>, g: usize| h.state().views[0].glyph_screen_pos(0, g).expect("glyph on screen");
    // Drag over "quick" (glyphs 4..=8).
    let (q, k) = (glyph(&h, 4), glyph(&h, 8));
    drag(&mut h, q, k);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick"));
    h.run_steps(60);
    // ⇧-click on the "n" of "brown": the selection runs from the anchor to it.
    let n = glyph(&h, 14);
    shift_click(&mut h, n);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("quick brown"));
    h.run_steps(60);
    // ⇧-click before the anchor: the anchor stays, the selection now runs back to the click.
    let t = glyph(&h, 0);
    shift_click(&mut h, t);
    assert_eq!(h.state().views[0].selected_text().as_deref(), Some("The q"));
    h.run_steps(60);
    // A plain click still clears it.
    click(&mut h, n);
    assert_eq!(h.state().views[0].selected_text(), None);
    h.run_steps(60);
    // ⇧-click with nothing selected selects nothing.
    shift_click(&mut h, n);
    assert_eq!(h.state().views[0].selected_text(), None);
}

/// One tall 300×1200 page with 40 lines of text, so the view can scroll through text.
fn long_page() -> Vec<u8> {
    let lines: Vec<String> = (1..=40).map(|n| format!("(Line {n:02} of a long page) Tj 0 -28 Td")).collect();
    let content = format!("BT /F1 14 Tf 20 1170 Td {} ET", lines.join(" "));
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 1200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF",
        content.len()
    )
    .into_bytes()
}

/// The long page at 300 % with auto-scroll latched by a middle click at the viewport's centre,
/// which is returned.
fn latched_on_long_page(tool: &'static str) -> (Harness<'static, PdfCraftApp>, Pos2) {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("long.pdf", None, long_page()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("author", "Tester").unwrap();
        app.set_option("zoom", "300").unwrap();
        app.set_option("quick", tool).unwrap();
        app
    });
    for _ in 0..200 {
        h.run_steps(2);
        if !h.state().render_pending() && h.state().views[0].glyph_screen_pos(0, 0).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let anchor = h.state().views[0].viewport_rect().center();
    h.hover_at(anchor);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: anchor, button: PointerButton::Middle, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(1);
    assert!(h.state().views[0].auto_scrolling(), "a middle click latches auto-scroll");
    (h, anchor)
}

/// Below the anchor, press the left button and move past egui's drag threshold, so a selection
/// starts while the page scrolls down.
fn start_selecting(h: &mut Harness<'static, PdfCraftApp>, anchor: Pos2) -> Pos2 {
    let at = anchor + egui::vec2(-200.0, 70.0);
    h.hover_at(at);
    h.run_steps(1);
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    let held = at + egui::vec2(40.0, 0.0);
    h.hover_at(held);
    h.run_steps(1);
    held
}

#[test]
fn a_left_drag_selects_text_while_auto_scroll_runs() {
    let (mut h, anchor) = latched_on_long_page("select");
    let held = start_selecting(&mut h, anchor);
    let early = h.state().views[0].selected_text().unwrap_or_default();
    // Hold the button still: the page scrolls under the pointer and the selection follows it.
    h.run_steps(40);
    let later = h.state().views[0].selected_text().expect("a selection");
    assert!(later.lines().count() > early.lines().count() + 1, "the selection grows as the page scrolls: {early:?} -> {later:?}");
    assert!(h.state().views[0].auto_scrolling(), "selecting does not stop scrolling");
    h.event(Event::PointerButton { pos: held, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
    assert!(h.state().views[0].auto_scrolling(), "the end of a selection drag leaves scrolling on");
    assert!(h.state().views[0].selected_text().is_some_and(|t| t.contains("Line")), "the selection stays");
    // A plain left click stops scrolling, and is an ordinary click on the page.
    click(&mut h, held);
    assert!(!h.state().views[0].auto_scrolling(), "a left click stops scrolling");
}

#[test]
fn the_highlighter_marks_text_while_auto_scroll_runs() {
    let (mut h, anchor) = latched_on_long_page("highlight");
    let held = start_selecting(&mut h, anchor);
    h.run_steps(20);
    h.event(Event::PointerButton { pos: held, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    let s = h.state();
    let kinds: Vec<String> = s.session.get(s.views[0].id).unwrap().info.annotations.iter().map(|a| a.subtype.clone()).collect();
    assert_eq!(kinds, ["Highlight"], "one highlight over the text dragged across");
    assert!(s.views[0].auto_scrolling(), "highlighting does not stop scrolling");
}
