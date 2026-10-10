//! Commenting in the real shell (egui_kittest): tools, gestures, selection, the composer and
//! the Comments panel.

use egui::{Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_render::Annotation;
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool};

/// Two 300×200 pages of Helvetica text.
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

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("text.pdf", None, TEXT_FIXTURE.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("author", "Tester").unwrap();
        setup(&mut app);
        app
    });
    h.run_steps(4);
    settle(&mut h);
    h
}

fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    for _ in 0..200 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// A point on page 1 in PDF user space → screen (no rotation; MediaBox 300×200).
fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("page 1 on screen");
    pos2(r.left() + x / 300.0 * r.width(), r.top() + (200.0 - y) / 200.0 * r.height())
}

fn drag(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(from + (to - from) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(3);
}

fn click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
}

/// Drag between two page points (user space).
fn drag_pt(h: &mut Harness<'static, PdfCraftApp>, a: (f32, f32), b: (f32, f32)) {
    let (a, b) = (at(h, a.0, a.1), at(h, b.0, b.1));
    drag(h, a, b);
}

/// Click a page point (user space).
fn click_pt(h: &mut Harness<'static, PdfCraftApp>, p: (f32, f32)) {
    let p = at(h, p.0, p.1);
    click(h, p);
}

/// A text field by its placeholder ("Add a comment", "Add a reply").
fn field<'a>(h: &'a Harness<'static, PdfCraftApp>, placeholder: &'a str) -> egui_kittest::Node<'a> {
    h.query_all_by(|n| n.placeholder() == Some(placeholder)).next().unwrap_or_else(|| panic!("no field {placeholder:?}"))
}

fn comments(h: &Harness<'static, PdfCraftApp>) -> Vec<Annotation> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().info.annotations.clone()
}

#[test]
fn dragging_over_text_with_the_highlighter_highlights_it() {
    let mut h = harness(|app| app.set_option("quick", "highlight").unwrap());
    let (a, b) = {
        let v = &h.state().views[0];
        (v.glyph_screen_pos(0, 4).expect("text layer"), v.glyph_screen_pos(0, 14).expect("glyph"))
    };
    drag(&mut h, a, b);
    let c = comments(&h);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!((c[0].subtype.as_str(), c[0].author.as_deref(), c[0].quads.len()), ("Highlight", Some("Tester"), 1));
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "the new comment is selected");
    assert_eq!(h.state().quick_tool, QuickTool::Comment(pdfcraft_ui_egui::comments::CommentTool::Highlight), "the highlighter stays on");
    // The Comments panel opened with the tool and lists it.
    h.get_by_label_contains("Comments");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Add highlight"));
}

#[test]
fn a_new_highlight_opens_its_note_for_typing() {
    let mut h = harness(|app| app.set_option("quick", "highlight").unwrap());
    assert_eq!(h.state().views[0].comments.editing, None);
    let (a, b) = {
        let v = &h.state().views[0];
        (v.glyph_screen_pos(0, 4).expect("text layer"), v.glyph_screen_pos(0, 14).expect("glyph"))
    };
    drag(&mut h, a, b);
    assert_eq!(comments(&h).len(), 1);
    assert_eq!(h.state().views[0].comments.editing, Some((0, 0, String::new())), "the Comments panel edits the new highlight");
    // An area highlight opens its note too, replacing the empty one.
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    assert_eq!(comments(&h).len(), 2);
    assert_eq!(h.state().views[0].comments.editing, Some((0, 1, String::new())));
    // A note being typed is kept: the next highlight is only selected.
    h.state_mut().views[0].comments.editing = Some((0, 1, "typed".into()));
    drag_pt(&mut h, (160.0, 100.0), (260.0, 40.0));
    assert_eq!(comments(&h).len(), 3);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 2)));
    assert_eq!(h.state().views[0].comments.editing, Some((0, 1, "typed".into())));
}

#[test]
fn a_selection_made_first_is_marked_when_a_tool_is_picked() {
    let mut h = harness(|_| {});
    h.state_mut().views[0].select_text(0, 4, 8);
    assert!(h.state_mut().execute("comment.strikeout"));
    h.run_steps(2);
    let c = comments(&h);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].subtype, "StrikeOut");
    assert!(h.state().views[0].selected_text().is_none(), "the selection is consumed");
}

#[test]
fn shapes_and_ink_are_drawn_by_dragging() {
    let mut h = harness(|app| app.set_option("quick", "square").unwrap());
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.state_mut().set_option("quick", "ink").unwrap();
    drag_pt(&mut h, (160.0, 100.0), (260.0, 40.0));
    h.state_mut().set_option("quick", "arrow").unwrap();
    drag_pt(&mut h, (20.0, 20.0), (120.0, 30.0));
    // A click with a drawing tool draws nothing.
    h.state_mut().set_option("quick", "circle").unwrap();
    click_pt(&mut h, (200.0, 180.0));
    let c = comments(&h);
    let mut c = c;
    c.sort_by_key(|a| a.index);
    let kinds: Vec<&str> = c.iter().map(|a| a.subtype.as_str()).collect();
    assert_eq!(kinds, ["Square", "Ink", "Line"], "{c:?}");
    let r = c[0].rect;
    assert!((r[0] - 40.0).abs() < 3.0 && (r[1] - 40.0).abs() < 3.0 && (r[2] - 140.0).abs() < 3.0 && (r[3] - 100.0).abs() < 3.0, "{r:?}");
}

#[test]
fn pen_strokes_stay_unselected_so_writing_continues() {
    let mut h = harness(|app| app.set_option("quick", "ink").unwrap());
    // No Comments panel: the author's name should only show in a hover popup.
    h.state_mut().set_option("panel", "none").unwrap();
    h.run_steps(2);
    drag_pt(&mut h, (40.0, 100.0), (140.0, 60.0));
    assert_eq!(h.state().views[0].comments.selected, None, "the first stroke isn't selected");
    // The pointer still rests on the stroke: no author popup covers it.
    h.hover_at(at(&h, 140.0, 60.0));
    h.run_steps(3);
    assert!(h.query_by_label_contains("Tester").is_none(), "no author popup while drawing");
    drag_pt(&mut h, (60.0, 100.0), (160.0, 60.0));
    let c = comments(&h);
    let kinds: Vec<&str> = c.iter().map(|a| a.subtype.as_str()).collect();
    assert_eq!(kinds, ["Ink", "Ink"], "{c:?}");
    assert_eq!(h.state().views[0].comments.selected, None, "nothing is selected after drawing");
    assert_eq!(h.state().quick_tool, QuickTool::Comment(pdfcraft_ui_egui::comments::CommentTool::Ink), "the pen stays on");
    // Other drawn shapes are still selected when made.
    h.state_mut().set_option("quick", "square").unwrap();
    drag_pt(&mut h, (180.0, 100.0), (260.0, 40.0));
    assert_eq!(h.state().views[0].comments.selected, Some((0, 2)));
}

#[test]
fn sticky_note_composer_posts_and_returns_to_select() {
    let mut h = harness(|app| app.set_option("quick", "note").unwrap());
    click_pt(&mut h, (250.0, 180.0));
    assert!(h.state().views[0].comments.composer.is_some(), "the composer opened");
    field(&h, "Add a comment").type_text("Please check");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!(c.len(), 1);
    assert_eq!((c[0].subtype.as_str(), c[0].contents.as_deref()), ("Text", Some("Please check")));
    assert!(h.state().views[0].comments.composer.is_none());
    assert_eq!(h.state().quick_tool, QuickTool::Select, "one-shot tools return to Select");
    // Cancelling another one creates nothing.
    h.state_mut().set_option("quick", "freetext").unwrap();
    click_pt(&mut h, (50.0, 60.0));
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(comments(&h).len(), 1);
}

#[test]
fn comments_are_selected_moved_and_deleted_with_the_select_tool() {
    let mut h = harness(|app| app.set_option("quick", "square").unwrap());
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.state_mut().set_option("quick", "select").unwrap();
    // Click away, then on the rectangle's border area.
    click_pt(&mut h, (250.0, 20.0));
    assert_eq!(h.state().views[0].comments.selected, None);
    click_pt(&mut h, (90.0, 70.0));
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)));
    drag_pt(&mut h, (90.0, 70.0), (140.0, 70.0));
    let r = comments(&h)[0].rect;
    assert!((r[0] - 90.0).abs() < 3.0 && (r[2] - 190.0).abs() < 3.0, "moved right by 50 pt: {r:?}");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Move comment"));
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert!(comments(&h).is_empty());
    h.state_mut().undo();
    h.run_steps(2);
    assert_eq!(comments(&h).len(), 1);
}

#[test]
fn the_panel_posts_comments_and_replies() {
    let mut h = harness(|app| app.set_option("panel", "comments").unwrap());
    field(&h, "Add a comment").click();
    h.run_steps(2);
    field(&h, "Add a comment").type_text("General remark");
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!((c.len(), c[0].subtype.as_str(), c[0].contents.as_deref()), (1, "Text", Some("General remark")));
    // The new comment is selected: reply to it.
    field(&h, "Add a reply").click();
    h.run_steps(2);
    field(&h, "Add a reply").type_text("Agreed");
    h.run_steps(1);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!(c.len(), 2);
    let reply = c.iter().find(|a| a.in_reply_to.is_some()).expect("a reply");
    assert_eq!((reply.contents.as_deref(), reply.author.as_deref()), (Some("Agreed"), Some("Tester")));
    h.get_by_label_contains("Agreed");
}

/// The side panel's width as egui keeps it from one frame to the next.
fn panel_width(h: &Harness<'static, PdfCraftApp>) -> f32 {
    egui::containers::panel::PanelState::load(&h.ctx, egui::Id::new("right_panel")).expect("the side panel is shown").size().x
}

#[test]
fn the_panel_keeps_its_width_with_a_reply_box_open() {
    // The reply row reserved 48 px for "Post"; Russian's wider button overflowed it, and a
    // resizable panel keeps the width its content used, so every repaint widened the panel.
    let mut h = harness(|app| app.set_option("panel", "comments").unwrap());
    field(&h, "Add a comment").click();
    h.run_steps(2);
    field(&h, "Add a comment").type_text("General remark");
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    field(&h, "Add a reply").click(); // the new comment is selected
    h.run_steps(2);
    field(&h, "Add a reply").type_text("Agreed");
    h.run_steps(1);
    h.state_mut().set_option("language", "ru").unwrap();
    h.run_steps(3);
    let before = panel_width(&h);
    h.get_by_label("Опубликовать").hover();
    for _ in 0..30 {
        h.run_steps(1);
    }
    assert_eq!(panel_width(&h), before, "the Comments panel drifted wider");
}

#[test]
fn line_ending_properties_change_and_undo() {
    let mut h = harness(|app| app.set_option("quick", "line").unwrap());
    drag_pt(&mut h, (40.0, 100.0), (160.0, 40.0));
    h.state_mut().open_comment_props(0, 0);
    h.run_steps(2);
    h.get_by_label("Line ending");
    h.query_all_by_value("None").next().expect("start ending").click();
    h.run_steps(2);
    for label in ["Open arrow", "Closed arrow", "Butt", "Reverse open arrow", "Reverse closed arrow", "Slash", "Square", "Circle", "Diamond"] {
        h.get_by_label(label);
    }
    assert_eq!(h.query_all_by_label("ROpenArrow").count(), 0);
    assert_eq!(h.query_all_by_label("OpenArrow").count(), 0);
    h.get_by_label("None").click();
    h.run_steps(1);
    {
        let d = h.state_mut().comment_props.as_mut().expect("open");
        assert_eq!(d.edited.endings.as_deref(), Some(&[pdfcraft_engine::LineEnding::None, pdfcraft_engine::LineEnding::None][..]));
        d.edited.endings = Some(vec![pdfcraft_engine::LineEnding::None, pdfcraft_engine::LineEnding::Diamond]);
    }
    h.get_by_label("OK").click();
    h.run_steps(3);
    let endings = {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        assert_eq!(doc.can_undo(), Some("Change comment properties"));
        doc.comment_props(0, 0).unwrap().endings
    };
    assert_eq!(endings.as_deref(), Some(&[pdfcraft_engine::LineEnding::None, pdfcraft_engine::LineEnding::Diamond][..]));
    h.state_mut().execute("edit.undo");
    h.run_steps(2);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(
        doc.comment_props(0, 0).unwrap().endings.as_deref(),
        Some(&[pdfcraft_engine::LineEnding::None, pdfcraft_engine::LineEnding::None][..])
    );
}

#[test]
fn comment_properties_change_appearance_and_author() {
    let mut h = harness(|app| app.set_option("quick", "square").unwrap());
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.state_mut().open_comment_props(0, 0);
    h.run_steps(2);
    h.get_by_label("Rectangle Properties");
    {
        let d = h.state_mut().comment_props.as_mut().expect("open");
        assert_eq!(d.original.width, Some(2.0));
        d.edited.color = Some([0.0, 0.0, 1.0]);
        d.edited.width = Some(5.0);
        d.edited.author = "Grace".into();
    }
    h.get_by_label("General").click();
    h.run_steps(2);
    h.get_by_label("Author");
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    let p = doc.comment_props(0, 0).unwrap();
    assert_eq!((p.color, p.width, p.author.as_str()), (Some([0.0, 0.0, 1.0]), Some(5.0), "Grace"));
    assert_eq!(doc.can_undo(), Some("Change comment properties"), "one undo step");
}

/// #686: rectangles, ovals, polygons and clouds can be filled, from the quick bar for new ones
/// and from Properties for existing ones; "No fill" takes it away again.
#[test]
fn shapes_take_a_fill_colour_from_the_quick_bar_and_properties() {
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut h = harness(|app| app.set_option("quick", "square").unwrap());
    h.run_steps(2);
    // No fill by default; the quick bar says so.
    h.get_by_label("Fill colour: none");
    h.state_mut().comment_prefs.set_fill(CommentTool::Rectangle, Some([1.0, 0.5, 0.0]));
    // Tools without an interior keep no fill.
    h.state_mut().comment_prefs.set_fill(CommentTool::Line, Some([1.0, 0.5, 0.0]));
    assert_eq!(h.state().comment_prefs.style(CommentTool::Line).fill, None);
    h.run_steps(2);
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    let fill_of = |h: &Harness<'static, PdfCraftApp>, index: usize| {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        let p = doc.comment_props(0, index).unwrap();
        assert!(p.fillable);
        p.fill
    };
    assert_eq!(fill_of(&h, 0), Some([1.0, 0.5, 0.0]), "the new rectangle is filled");
    // Quick bar ▸ Fill colour ▸ No fill: the next rectangle has none.
    h.state_mut().set_option("quick", "square").unwrap();
    h.run_steps(2);
    h.get_by_label("Fill colour").click();
    h.run_steps(2);
    h.get_by_label("No fill").click();
    h.run_steps(2);
    assert_eq!(h.state().comment_prefs.style(CommentTool::Rectangle).fill, None);
    drag_pt(&mut h, (160.0, 100.0), (260.0, 40.0));
    assert_eq!(fill_of(&h, 1), None);

    // Properties ▸ Fill colour changes an existing one, as one undo step.
    h.state_mut().open_comment_props(0, 0);
    h.run_steps(2);
    h.get_by_label("Rectangle Properties");
    h.get_by_label("Fill colour");
    assert_eq!(h.state().comment_props.as_ref().unwrap().original.fill, Some([1.0, 0.5, 0.0]));
    h.get_by_label("No fill").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(fill_of(&h, 0), None);
    let undo = h.state().session.get(h.state().views[0].id).unwrap().can_undo().map(str::to_owned);
    assert_eq!(undo.as_deref(), Some("Change comment properties"));
    h.state_mut().open_comment_props(0, 1);
    h.run_steps(2);
    h.state_mut().comment_props.as_mut().expect("open").edited.fill = Some([0.0, 0.0, 1.0]);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(fill_of(&h, 1), Some([0.0, 0.0, 1.0]));
    // Make Current Properties Default carries the fill into the tool.
    h.state_mut().make_comment_default(0, 1);
    assert_eq!(h.state().comment_prefs.style(CommentTool::Rectangle).fill, Some([0.0, 0.0, 1.0]));
}

#[test]
fn the_panel_filters_and_sorts() {
    let mut h = harness(|app| {
        app.set_option("panel", "comments").unwrap();
        app.set_option("quick", "square").unwrap();
    });
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.state_mut().set_option("quick", "circle").unwrap();
    drag_pt(&mut h, (160.0, 100.0), (260.0, 40.0));
    h.state_mut().set_option("quick", "select").unwrap();
    h.run_steps(2);
    let ovals = h.query_all_by_label("Oval").count();
    assert!(ovals >= 1);
    h.state_mut().views[0].comments.hidden_types.push("Oval".into());
    h.run_steps(2);
    assert_eq!(h.query_all_by_label("Oval").count(), ovals - 1, "the oval's card is filtered out");
    assert!(h.query_all_by_label("Rectangle").count() >= 1);
    h.state_mut().views[0].comments.hidden_types.clear();
    h.state_mut().views[0].comments.sort = pdfcraft_ui_egui::comments::SortBy::Type;
    h.run_steps(2);
    // Grouped by type: a group header names each type.
    assert_eq!(h.query_all_by_label("Oval").count(), ovals + 1, "a group header was added");
}

#[test]
fn comments_take_checkmarks_lock_hide_and_summarize() {
    let mut h = harness(|app| {
        app.set_option("panel", "comments").unwrap();
        app.set_option("quick", "square").unwrap();
    });
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.state_mut().set_option("quick", "select").unwrap();
    h.run_steps(2);
    assert_eq!(h.state().views[0].comments.selected, Some((0, 0)), "the new comment is selected");
    // The card's checkmark.
    h.get_by_label("Mark with checkmark").click();
    h.run_steps(3);
    assert!(comments(&h).iter().any(|a| a.state.as_deref() == Some("Marked")));
    h.get_by_label("Remove checkmark");

    // "…" ▸ Copy text puts the comment's text on the clipboard.
    h.state_mut().apply_edit(pdfcraft_engine::Edit::SetAnnotationContents { page: 0, index: 0, text: "Copy me".into() });
    h.run_steps(2);
    h.get_by_label("More").click();
    h.run_steps(2);
    h.get_by_label("Copy text").click();
    let mut copied = false;
    for _ in 0..4 {
        h.step();
        copied |= h.output().platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(t) if t == "Copy me"));
    }
    assert!(copied, "the text is copied");
    h.run_steps(2);

    // Properties ▸ Locked.
    h.state_mut().open_comment_props(0, 0);
    h.run_steps(2);
    h.get_by_label("Locked").click();
    h.run_steps(1);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(comments(&h)[0].locked);
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert!(comments(&h).iter().any(|a| a.in_reply_to.is_none()), "a locked comment isn't deleted");

    // "…" ▸ Hide all comments, then Show all comments.
    h.get_by_label("More options").click();
    h.run_steps(2);
    h.get_by_label("Hide all comments").click();
    h.run_steps(2);
    let s = h.state();
    assert!(s.session.get(s.views[0].id).unwrap().comments_hidden());
    h.get_by_label("More options").click();
    h.run_steps(2);
    h.get_by_label("Show all comments").click();
    h.run_steps(2);
    let s = h.state();
    assert!(!s.session.get(s.views[0].id).unwrap().comments_hidden());

    // "…" ▸ Create PDF comment summary.
    h.get_by_label("More options").click();
    h.run_steps(2);
    h.get_by_label("Create PDF comment summary…").click();
    h.run_steps(2);
    h.get_by_label("Summarize Options");
    h.get_by_label("Create PDF Comment Summary").click();
    h.run_steps(3);
    let s = h.state();
    assert_eq!(s.views.len(), 2);
    assert!(s.session.get(s.views[1].id).unwrap().name.starts_with("Summary of comments on text"));
}

#[test]
fn polygons_clouds_connected_lines_callouts_and_inserted_text() {
    let mut h = harness(|app| app.set_option("quick", "polygon").unwrap());
    // Polygon: click the corners, Enter finishes.
    for p in [(40.0, 40.0), (120.0, 40.0), (80.0, 110.0)] {
        click_pt(&mut h, p);
    }
    assert_eq!(comments(&h).len(), 0, "still drawing");
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].subtype, "Polygon");

    // Cloud: clicking the first point again closes it.
    h.state_mut().set_option("quick", "cloud").unwrap();
    for p in [(160.0, 40.0), (260.0, 40.0), (260.0, 100.0), (160.0, 100.0), (160.0, 40.0)] {
        click_pt(&mut h, p);
    }
    assert_eq!(comments(&h).len(), 2);

    // Connected lines.
    h.state_mut().set_option("quick", "polyline").unwrap();
    for p in [(20.0, 150.0), (60.0, 130.0), (100.0, 150.0)] {
        click_pt(&mut h, p);
    }
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(comments(&h).iter().any(|a| a.subtype == "PolyLine"));

    // Callout: drag from the target to where the box goes, then type.
    h.state_mut().set_option("quick", "callout").unwrap();
    drag_pt(&mut h, (60.0, 160.0), (180.0, 190.0));
    assert!(h.state().views[0].comments.composer.is_some(), "the composer opened");
    field(&h, "Type text").type_text("Why?");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let callout = comments(&h).into_iter().find(|a| a.subtype == "FreeText").expect("a callout");
    assert_eq!(callout.contents.as_deref(), Some("Why?"));
    assert!(callout.rect[0] <= 60.0 && callout.rect[1] <= 160.0, "holds the leader: {:?}", callout.rect);

    // Insert text at a click.
    h.state_mut().set_option("quick", "caret").unwrap();
    click_pt(&mut h, (95.0, 150.0));
    field(&h, "Text to insert").type_text("very ");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let caret = comments(&h).into_iter().find(|a| a.subtype == "Caret").expect("a caret");
    assert_eq!(caret.contents.as_deref(), Some("very"));
    assert_eq!(h.state().quick_tool, QuickTool::Select);
}

#[test]
fn the_panel_filters_by_colour_and_checkmark() {
    let mut h = harness(|app| {
        app.set_option("panel", "comments").unwrap();
        app.set_option("quick", "square").unwrap();
    });
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.state_mut().set_option("quick", "circle").unwrap();
    drag_pt(&mut h, (160.0, 100.0), (260.0, 40.0));
    h.state_mut().set_option("quick", "select").unwrap();
    h.state_mut().apply_edit(pdfcraft_engine::Edit::StyleAnnotation {
        page: 0,
        index: 1,
        color: Some([0.0, 0.47, 0.84]),
        opacity: None,
        width: None,
        endings: None,
    });
    h.state_mut().apply_edit(pdfcraft_engine::Edit::MarkAnnotation { page: 0, index: 0, marked: true, author: "Tester".into() });
    h.run_steps(2);
    let cards = |h: &Harness<'static, PdfCraftApp>| (h.query_all_by_label("Rectangle").count(), h.query_all_by_label("Oval").count());
    // The filter menu lists the colours by name (counted with it open: it names the types too).
    h.get_by_label("Filter comments").click();
    h.run_steps(2);
    let before = cards(&h);
    h.get_by_label("Blue").click();
    h.run_steps(2);
    assert_eq!(h.state().views[0].comments.hidden_colors, vec!["0077D6".to_string()]);
    assert_eq!(cards(&h).1, before.1 - 1, "the blue oval is filtered out");
    h.get_by_label("Checked").click();
    h.run_steps(2);
    assert_eq!(cards(&h).0, before.0 - 1, "the checked rectangle is filtered out");
    h.get_by_label("Show all").click();
    h.run_steps(2);
    assert_eq!(cards(&h), before);
}

#[test]
fn make_current_properties_default() {
    let mut h = harness(|app| {
        app.set_option("panel", "comments").unwrap();
        app.set_option("quick", "square").unwrap();
    });
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    h.state_mut().set_option("quick", "select").unwrap();
    h.state_mut().apply_edit(pdfcraft_engine::Edit::StyleAnnotation {
        page: 0,
        index: 0,
        color: Some([0.0, 0.47, 0.84]),
        opacity: Some(0.5),
        width: Some(5.0),
        endings: None,
    });
    h.run_steps(2);
    h.get_by_label("More").click();
    h.run_steps(2);
    h.get_by_label("Make Current Properties Default").click();
    h.run_steps(3);
    let st = h.state().comment_prefs.style(pdfcraft_ui_egui::comments::CommentTool::Rectangle);
    assert_eq!((st.color, st.opacity, st.width), ([0.0, 0.47, 0.84], 0.5, 5.0));
    // The default survives a restart (#340): a fresh app reads the persisted settings and its
    // next rectangle still takes the style.
    let json = h.state().persist();
    let mut fresh = PdfCraftApp::new();
    fresh.restore(&json);
    let st = fresh.comment_prefs.style(pdfcraft_ui_egui::comments::CommentTool::Rectangle);
    assert_eq!((st.color, st.opacity, st.width), ([0.0, 0.47, 0.84], 0.5, 5.0), "the tool default persisted");
    // The next rectangle takes it.
    h.state_mut().set_option("quick", "square").unwrap();
    drag_pt(&mut h, (160.0, 100.0), (260.0, 40.0));
    let c = comments(&h);
    let last = c.iter().rfind(|a| a.subtype == "Square").unwrap();
    assert_eq!(last.color, Some([0.0, 0.47, 0.84]));
}

#[test]
fn highlighting_an_area_off_the_text() {
    let mut h = harness(|app| app.set_option("quick", "highlight").unwrap());
    // Blank space below the text line (text is at y 150).
    drag_pt(&mut h, (40.0, 100.0), (140.0, 40.0));
    let c = comments(&h);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!((c[0].subtype.as_str(), c[0].quads.len()), ("Highlight", 1));
    let q = c[0].quads[0];
    assert!((q[0] - 40.0).abs() < 3.0 && (q[1] - 100.0).abs() < 3.0, "{q:?}");
    // Dragging over text still highlights the text (once the text layer is in).
    let mut glyphs = None;
    for _ in 0..200 {
        h.run_steps(1);
        let v = &h.state().views[0];
        if let (Some(a), Some(b)) = (v.glyph_screen_pos(0, 4), v.glyph_screen_pos(0, 14)) {
            glyphs = Some((a, b));
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let (a, b) = glyphs.expect("text layer");
    drag(&mut h, a, b);
    assert_eq!(comments(&h).len(), 2);
}

#[test]
fn replacing_text_strikes_it_and_adds_a_caret() {
    let mut h = harness(|app| app.set_option("quick", "replace").unwrap());
    drag_pt(&mut h, (20.0, 155.0), (120.0, 155.0));
    assert!(h.state().views[0].comments.composer.is_some(), "the composer asks for the replacement");
    field(&h, "Replacement text").type_text("slow red");
    h.run_steps(2);
    h.get_by_label("Post").click();
    h.run_steps(3);
    let c = comments(&h);
    let strike = c.iter().find(|a| a.subtype == "StrikeOut").expect("a strikeout");
    let caret = c.iter().find(|a| a.subtype == "Caret").expect("a caret");
    assert_eq!(caret.contents.as_deref(), Some("slow red"));
    assert_eq!(caret.in_reply_to, strike.name, "grouped with the strikeout");
    assert_eq!(h.state().session.get(h.state().views[0].id).unwrap().can_undo(), Some("Replace text"));
    assert_eq!(h.state().quick_tool, QuickTool::Select);
}

#[test]
fn attaching_a_file_as_a_comment() {
    let mut h = harness(|app| app.set_option("quick", "attach").unwrap());
    h.state_mut().attach_override = Some(("notes.txt".into(), b"remember the milk".to_vec()));
    click_pt(&mut h, (250.0, 180.0));
    h.run_steps(3);
    let c = comments(&h);
    assert_eq!((c.len(), c[0].subtype.as_str(), c[0].contents.as_deref()), (1, "FileAttachment", Some("notes.txt")));
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert!(doc.info.attachments.iter().any(|a| a.name == "notes.txt"), "listed in the Attachments panel");
    assert_eq!(s.quick_tool, QuickTool::Select);
}

/// Drag with the middle button (the mouse wheel) in small steps.
fn middle_drag(h: &mut Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2) {
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Middle, pressed, modifiers: egui::Modifiers::NONE };
    h.event(egui::Event::PointerMoved(from));
    h.run_steps(1);
    h.event(button(from, true));
    h.run_steps(1);
    for k in 1..=4 {
        h.event(egui::Event::PointerMoved(from + (to - from) * (k as f32 / 4.0)));
        h.run_steps(1);
    }
    h.event(button(to, false));
    h.run_steps(3);
}

#[test]
fn a_middle_drag_with_a_drawing_tool_scrolls_instead_of_drawing() {
    // egui's drag responses accept any button, so the drawing tools used to draw with the wheel.
    let mut h = harness(|app| {
        app.set_option("zoom", "400").unwrap();
        app.set_option("quick", "ink").unwrap();
    });
    let from = at(&h, 150.0, 100.0);
    let top = h.state().views[0].page_screen_rect(0).expect("page 1 on screen").top();
    middle_drag(&mut h, from, from - egui::vec2(0.0, 120.0));
    assert!(comments(&h).is_empty(), "{:?}", comments(&h));
    let s = h.state();
    assert!(!s.session.get(s.views[0].id).unwrap().dirty, "scrolling never edits the PDF");
    assert!(!s.views[0].middle_panning(), "releasing the wheel ends the pan");
    let moved = s.views[0].page_screen_rect(0).expect("page 1 on screen").top();
    if cfg!(target_os = "linux") {
        // Linux latches auto-scroll instead of panning with the drag.
        assert!(s.views[0].auto_scrolling());
    } else {
        assert!((moved - (top - 120.0)).abs() < 1.0, "the page follows the pointer: {top} -> {moved}");
    }
    // The tool still draws with the primary button afterwards.
    if cfg!(target_os = "linux") {
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
    }
    drag_pt(&mut h, (60.0, 120.0), (200.0, 80.0));
    let kinds: Vec<String> = comments(&h).into_iter().map(|a| a.subtype).collect();
    assert_eq!(kinds, ["Ink"]);
}

#[test]
fn erasing_part_of_a_drawing() {
    let mut h = harness(|app| app.set_option("quick", "ink").unwrap());
    drag_pt(&mut h, (40.0, 60.0), (260.0, 60.0));
    assert_eq!(comments(&h).len(), 1);
    h.state_mut().set_option("quick", "eraser").unwrap();
    drag_pt(&mut h, (150.0, 90.0), (150.0, 30.0));
    let c = comments(&h);
    assert_eq!(c.len(), 1);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.can_undo(), Some("Erase"));
    // Rubbing along the rest removes the drawing.
    drag_pt(&mut h, (40.0, 60.0), (260.0, 60.0));
    assert!(comments(&h).is_empty(), "{:?}", comments(&h));
}

#[test]
fn hovering_a_comment_shows_its_author_and_text() {
    // A square comment by Ada over the first page's text.
    let pdf = String::from_utf8_lossy(TEXT_FIXTURE).replace("/Contents 5 0 R /Resources", "/Contents 5 0 R /Annots [8 0 R] /Resources").replace(
        "trailer",
        "8 0 obj << /Type /Annot /Subtype /Square /Rect [50 50 150 120] /C [1 0 0] /T (Ada) /Contents (Check this figure) >> endobj\ntrailer",
    );
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("hover.pdf", None, pdf.into_bytes()).expect("opens");
        app.set_option("left", "closed").unwrap();
        // No Comments panel: the comment's text should only appear in the hover popup.
        app.set_option("panel", "none").unwrap();
        app
    });
    h.run_steps(4);
    settle(&mut h);
    assert!(h.query_by_label_contains("Check this figure").is_none(), "no popup before hovering");
    let p = at(&h, 100.0, 85.0);
    h.hover_at(p);
    h.run_steps(3);
    h.get_by_label_contains("Check this figure");
    h.get_by_label_contains("Ada");
}

#[test]
fn the_squiggly_tool_marks_selected_text() {
    let mut h = harness(|_| {});
    h.state_mut().views[0].select_text(0, 4, 8);
    assert!(h.state_mut().execute("comment.squiggly"));
    h.run_steps(2);
    let c = comments(&h);
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!((c[0].subtype.as_str(), c[0].quads.len()), ("Squiggly", 1));
    // It sits with the other text markup in the Highlight ▸ flyout, and edits as itself.
    assert!(pdfcraft_ui_egui::comments::GROUPS[1].contains(&pdfcraft_ui_egui::comments::CommentTool::Squiggly));
    assert_eq!(pdfcraft_ui_egui::comments::tool_for(&c[0]), Some(pdfcraft_ui_egui::comments::CommentTool::Squiggly));
}

#[test]
fn the_opacity_set_for_a_tool_goes_into_its_new_comments() {
    use pdfcraft_ui_egui::comments::CommentTool;
    let mut h = harness(|app| app.comment_prefs.set_opacity(CommentTool::StrikeOut, 0.5));
    // Clamped: an invisible comment can't be found again.
    h.state_mut().comment_prefs.set_opacity(CommentTool::Underline, 0.0);
    assert!((h.state().comment_prefs.style(CommentTool::Underline).opacity - 0.1).abs() < 1e-9);
    h.state_mut().views[0].select_text(0, 4, 8);
    assert!(h.state_mut().execute("comment.strikeout"));
    h.run_steps(2);
    let c = comments(&h);
    assert_eq!(c.len(), 1, "{c:?}");
    // The new comment carries /CA 0.5 (read back from the document's bytes).
    let bytes = h.state().session.get(h.state().views[0].id).unwrap().bytes.as_ref().clone();
    let doc = pdfcraft_cos::Document::open(std::sync::Arc::new(bytes)).unwrap();
    let opacities: Vec<f64> = doc
        .object_numbers()
        .into_iter()
        .filter_map(|n| doc.try_get(n).ok())
        .filter_map(|o| o.as_dict().cloned())
        .filter(|d| d.name(b"Subtype") == Some(b"StrikeOut"))
        .filter_map(|d| d.get(b"CA").and_then(|v| v.as_f64()))
        .collect();
    assert_eq!(opacities, [0.5]);
    // The quick bar offers the control.
    h.state_mut().set_option("quick", "strikeout").unwrap();
    h.run_steps(2);
    h.get_by_label("Opacity");
}

/// #225: comment tools open the Comments panel until the user closes it; then it stays closed,
/// across restarts, until they open it again.
#[test]
fn a_closed_comments_panel_stays_closed_when_picking_comment_tools() {
    use pdfcraft_ui_egui::RightPanel;
    let mut h = harness(|_| {});
    assert_eq!(h.state().right, None);
    assert!(h.state_mut().execute("comment.highlight"));
    h.run_steps(2);
    assert_eq!(h.state().right, Some(RightPanel::Comments), "Acrobat opens Comments with the tools");
    h.get_by_label("Close").click();
    h.run_steps(2);
    assert_eq!(h.state().right, None);
    for tool in ["select", "comment.underline", "comment.note", "comment.highlight"] {
        if tool == "select" {
            h.state_mut().quick_tool = QuickTool::Select;
        } else {
            assert!(h.state_mut().execute(tool));
        }
        h.run_steps(2);
        assert_eq!(h.state().right, None, "{tool} reopened the closed Comments panel");
    }
    let mut again = PdfCraftApp::new();
    again.restore(&h.state().persist());
    assert!(again.comments_panel_closed, "the choice survives a restart");
    // Opening it again from the rail lets the tools open it once more.
    h.get_by_label("Comments").click();
    h.run_steps(2);
    assert_eq!(h.state().right, Some(RightPanel::Comments));
    assert!(!h.state().comments_panel_closed);
    // What the rail and View menu call; "Bookmarks" labels two controls on screen.
    h.state_mut().choose_right_panel(Some(RightPanel::Bookmarks));
    h.state_mut().choose_right_panel(None);
    h.run_steps(2);
    assert!(!h.state().comments_panel_closed, "switching to and closing another panel isn't closing Comments");
    assert!(h.state_mut().execute("comment.strikeout"));
    h.run_steps(2);
    assert_eq!(h.state().right, Some(RightPanel::Comments));
}
