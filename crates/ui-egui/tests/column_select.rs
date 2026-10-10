//! Issue #740: column select, as in Acrobat. Alt-drag (Option-drag on macOS) with the Select tool,
//! or a plain drag with Edit ▸ Column select, selects only the text inside the rectangle drawn.

use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

/// One 300×200 page with a three-row table, "Name Qty / Apple 12 / Pear 7". Its columns sit far
/// enough apart that reading order takes one column, then the other: glyphs 0–3 "Name", 4–8
/// "Apple", 9–12 "Pear", 13–15 "Qty", 16–17 "12", 18 "7".
fn table() -> Vec<u8> {
    let content =
        "BT /F1 14 Tf 20 150 Td (Name) Tj 130 0 Td (Qty) Tj -130 -24 Td (Apple) Tj 130 0 Td (12) Tj -130 -24 Td (Pear) Tj 130 0 Td (7) Tj ET";
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

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("table.pdf", None, table()).expect("opens");
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
    assert!(h.state().views[0].glyph_screen_pos(0, 0).is_some(), "text layer loaded");
    h
}

fn glyph(h: &Harness<'static, PdfCraftApp>, g: usize) -> Pos2 {
    h.state().views[0].glyph_screen_pos(0, g).expect("glyph on screen")
}

fn text(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
    h.state().views[0].selected_text()
}

/// Press at `from`, move to `to` and release there, with `modifiers` held throughout.
fn drag(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2, modifiers: Modifiers) {
    h.event(Event::ModifiersChanged(modifiers));
    h.hover_at(from);
    h.run_steps(1);
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers });
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from + (to - from) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers });
    h.run_steps(1);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
}

/// Below-right of the top two rows: past the "y" of "Qty" (the wider cell), level with "12".
fn below_right_of_row_two(h: &Harness<'static, PdfCraftApp>) -> Pos2 {
    egui::pos2(glyph(h, 15).x.max(glyph(h, 17).x) + 12.0, glyph(h, 17).y + 8.0)
}

/// From above-left of "Name" to below-right of "12": the top two rows of both columns.
fn top_two_rows(h: &Harness<'static, PdfCraftApp>) -> (Pos2, Pos2) {
    (glyph(h, 0) - vec2(12.0, 12.0), below_right_of_row_two(h))
}

#[test]
fn alt_drag_selects_only_the_text_inside_the_rectangle() {
    let mut h = harness(|_| {});
    let (from, to) = top_two_rows(&h);
    // A plain drag selects in reading order: everything from "Name" to "12".
    drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!(text(&h).as_deref(), Some("Name\nApple\nPear\nQty\n12"));
    h.run_steps(60);
    // Alt (Option on macOS) at the start of the drag: only the rectangle, row by row.
    drag(&mut h, from, to, Modifiers::ALT);
    assert_eq!(text(&h).as_deref(), Some("Name\tQty\nApple\t12"));
    // One column of numbers, which is what the issue asked for.
    h.run_steps(60);
    let (from, to) = (glyph(&h, 13) - vec2(8.0, 12.0), egui::pos2(glyph(&h, 15).x + 12.0, glyph(&h, 18).y + 12.0));
    drag(&mut h, from, to, Modifiers::ALT);
    assert_eq!(text(&h).as_deref(), Some("Qty\n12\n7"));
}

#[test]
fn the_column_select_tool_needs_no_modifier() {
    let mut h = harness(|app| assert!(app.execute("edit.column_select")));
    assert_eq!(h.state().quick_tool, pdfcraft_ui_egui::QuickTool::ColumnSelect);
    let (from, to) = top_two_rows(&h);
    drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!(text(&h).as_deref(), Some("Name\tQty\nApple\t12"));
    // A rectangle around no text selects nothing.
    h.run_steps(60);
    let below = glyph(&h, 12) + vec2(0.0, 60.0);
    drag(&mut h, below, below + vec2(60.0, 30.0), Modifiers::NONE);
    assert_eq!(text(&h), None);
}

/// ⇧-click extends a reading-order selection; a column selection has no anchor glyph, so a
/// ⇧-click clears it like a plain click.
#[test]
fn shift_click_clears_a_column_selection() {
    let mut h = harness(|_| {});
    let (from, to) = top_two_rows(&h);
    drag(&mut h, from, to, Modifiers::ALT);
    assert!(text(&h).is_some());
    h.run_steps(60);
    let at = glyph(&h, 10);
    h.event(Event::ModifiersChanged(Modifiers::SHIFT));
    h.hover_at(at);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::SHIFT });
    }
    h.run_steps(1);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
    assert_eq!(text(&h), None);
}

/// Alt-drag with the highlighter marks only the cells inside the rectangle: one quad per piece
/// of a line. The drag starts on text, since off text the highlighter draws an area highlight.
#[test]
fn alt_drag_with_the_highlighter_marks_only_the_column() {
    let mut h = harness(|app| assert!(app.execute("comment.highlight")));
    // Just above-left of the centre of "N", still on the glyph.
    let (from, to) = (glyph(&h, 0) - vec2(2.0, 2.0), below_right_of_row_two(&h));
    drag(&mut h, from, to, Modifiers::ALT);
    h.run_steps(3);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    let marks: Vec<(&str, usize)> = doc.info.annotations.iter().map(|a| (a.subtype.as_str(), a.quads.len())).collect();
    assert_eq!(marks, [("Highlight", 4)], "Name, Qty, Apple and 12, each its own quad");
}
