//! Multi-object editing through the real shell. Synthetic artwork and standard-font text only.
use egui::{Key, Modifiers, Pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::{AddedText, Edit, EditableObject};
use pdfcraft_ui_egui::PdfCraftApp;

fn fixture(clipping: bool) -> Vec<u8> {
    fixture_with_background(clipping, false)
}

fn fixture_with_background(clipping: bool, background: bool) -> Vec<u8> {
    let stream = |body: &str, dict: &str| format!("<< {dict} /Length {} >>\nstream\n{body}\nendstream", body.len());
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 500] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Figure 6 0 R >> >> >>"
            .into(),
        stream(
            &format!(
                "{}q BT {} Tr /F1 18 Tf 40 430 Td (Group headline) Tj ET Q q 1 0 0 1 40 340 cm /Figure Do Q BT /F1 12 Tf 240 90 Td (Keep here) Tj ET",
                if background { "q /Background Do Q " } else { "" },
                if clipping { 4 } else { 0 }
            ),
            "",
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        stream("1 0.2 0.1 rg 0 0 60 30 re f", "/Type /XObject /Subtype /Form /BBox [0 0 60 30]"),
    ];
    if background {
        objects[2] = objects[2].replace("/Figure 6 0 R", "/Figure 6 0 R /Background 7 0 R");
        objects.push(stream("1 1 1 rg 0 0 400 500 re f", "/Type /XObject /Subtype /Form /BBox [0 0 400 500]"));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

fn harness() -> Harness<'static, PdfCraftApp> {
    harness_with(false)
}

fn harness_with(clipping: bool) -> Harness<'static, PdfCraftApp> {
    harness_document(clipping, false)
}

fn harness_document(clipping: bool, background: bool) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.session = pdfcraft_engine::Session::new().with_clock(|| 1_700_000_000);
        app.set_option("language", "en").unwrap();
        app.open_bytes("group.pdf", None, fixture_with_background(clipping, background)).unwrap();
        let id = app.views[0].id;
        app.session
            .apply(
                id,
                Edit::AddText { page: 0, text: AddedText { text: "Added note".into(), rect: [130.0, 340.0, 230.0, 370.0], ..Default::default() } },
            )
            .unwrap();
        let saved = app.session.save_bytes(id).unwrap();
        app.session.mark_saved(id, saved, None).unwrap();
        app.set_option("zoom", "100").unwrap();
        assert!(app.execute("edit.edit_text"));
        app
    });
    for _ in 0..60 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h
}

fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    r.min + egui::vec2(x, y) * (r.width() / 400.0)
}
fn objects(h: &Harness<'static, PdfCraftApp>) -> Vec<EditableObject> {
    let app = h.state();
    app.session.get(app.views[0].id).unwrap().editable_objects(0).unwrap()
}
fn click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2, mods: Modifiers) {
    h.hover_at(p);
    h.run_steps(2);
    for pressed in [true, false] {
        h.event_modifiers(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: mods }, mods);
    }
    h.run_steps(4);
}
fn marquee(h: &mut Harness<'static, PdfCraftApp>) {
    let (a, b) = (at(h, 25.0, 45.0), at(h, 235.0, 175.0));
    h.hover_at(a);
    h.run_steps(2);
    h.drag_at(a);
    h.run_steps(2);
    h.hover_at(b);
    h.run_steps(2);
    h.drop_at(b);
    h.run_steps(4);
    h.get_by_label("3 objects selected");
}
fn drag(h: &mut Harness<'static, PdfCraftApp>, a: Pos2, b: Pos2) {
    h.hover_at(a);
    h.run_steps(2);
    h.drag_at(a);
    h.run_steps(2);
    h.hover_at(b);
    h.run_steps(2);
    h.drop_at(b);
    h.run_steps(4);
}

#[test]
fn marquee_and_mixed_group_drag_move_once_and_undo_together() {
    let mut h = harness();
    let before = objects(&h);
    marquee(&mut h);
    let (a, b) = (at(&h, 55.0, 70.0), at(&h, 80.0, 100.0));
    drag(&mut h, a, b);
    let after = objects(&h);
    for old in &before {
        let new = after.iter().find(|o| o.target == old.target).unwrap();
        let shift = if old.text.as_deref() == Some("Keep here") { [0.0, 0.0] } else { [25.0, -30.0] };
        for i in 0..4 {
            assert!((new.rect[i] - old.rect[i] - shift[i % 2]).abs() < 0.01, "{old:?} -> {new:?}");
        }
    }
    h.get_by_label("3 objects selected");
    let app = h.state();
    assert_eq!(app.session.get(app.views[0].id).unwrap().can_undo(), Some("Move objects"));
    if let Ok(path) = std::env::var("PDFCRAFT_GROUP_SHOT") {
        h.render().unwrap().save(path).unwrap();
    }
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(4);
    assert_eq!(objects(&h), before);
    h.get_by_label("Shift-click to select several objects. Drag empty space to select an area.");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run_steps(4);
    assert_eq!(objects(&h), after);
}

#[test]
fn modifier_clicks_extend_an_ordinary_selection_and_toggle_objects() {
    let mut h = harness();
    let p = at(&h, 55.0, 70.0);
    click(&mut h, p, Modifiers::NONE);
    assert!(h.state().views[0].line_editor.is_some());
    let p = at(&h, 60.0, 145.0);
    click(&mut h, p, Modifiers::SHIFT);
    h.get_by_label("2 objects selected");
    assert!(h.state().views[0].line_editor.is_none());
    let p = at(&h, 155.0, 140.0);
    click(&mut h, p, Modifiers::COMMAND);
    h.get_by_label("3 objects selected");
    let p = at(&h, 60.0, 145.0);
    click(&mut h, p, Modifiers::CTRL);
    h.get_by_label("2 objects selected");
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
    h.key_press(Key::Escape);
    h.run_steps(3);
    h.get_by_label("Shift-click to select several objects. Drag empty space to select an area.");
}

#[test]
fn escaping_a_group_drag_and_switching_tools_do_not_edit() {
    let mut h = harness();
    let before = objects(&h);
    marquee(&mut h);
    let (a, b) = (at(&h, 55.0, 70.0), at(&h, 90.0, 110.0));
    h.hover_at(a);
    h.run_steps(2);
    h.drag_at(a);
    h.run_steps(2);
    h.hover_at(b);
    h.run_steps(2);
    assert_eq!(objects(&h), before, "only a preview while held");
    h.key_press(Key::Escape);
    h.run_steps(2);
    h.drop_at(b);
    h.run_steps(4);
    assert_eq!(objects(&h), before);
    marquee(&mut h);
    assert!(h.state_mut().execute("edit.text"));
    h.run_steps(4);
    assert_eq!(objects(&h), before);
}

#[test]
fn a_changed_text_editor_is_kept_when_group_selection_is_requested() {
    let mut h = harness();
    let before = objects(&h);
    let p = at(&h, 55.0, 70.0);
    click(&mut h, p, Modifiers::NONE);
    h.state_mut().views[0].line_editor.as_mut().unwrap().text = "A draft still being edited".into();
    let p = at(&h, 60.0, 145.0);
    click(&mut h, p, Modifiers::SHIFT);
    assert_eq!(h.state().views[0].line_editor.as_ref().unwrap().text, "A draft still being edited");
    assert_eq!(objects(&h), before);
}

#[test]
fn a_document_change_invalidates_old_selection_before_the_next_drag() {
    let mut h = harness();
    marquee(&mut h);
    let id = h.state().views[0].id;
    h.state_mut()
        .session
        .apply(id, Edit::AddText { page: 0, text: AddedText { text: "A new item".into(), rect: [40.0, 210.0, 150.0, 240.0], ..Default::default() } })
        .unwrap();
    h.run_steps(4);
    h.get_by_label("Shift-click to select several objects. Drag empty space to select an area.");
    assert_eq!(objects(&h).len(), 5);
}

#[test]
fn refusing_one_object_keeps_the_whole_group_and_its_selection() {
    let mut h = harness_with(true);
    let before = objects(&h);
    marquee(&mut h);
    let (a, b) = (at(&h, 55.0, 70.0), at(&h, 80.0, 100.0));
    drag(&mut h, a, b);
    assert_eq!(objects(&h), before);
    h.get_by_label("3 objects selected");
    assert!(h.state().toast.as_ref().unwrap().0.contains("Text used as a clipping path cannot be moved with this selection."));
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
}

#[test]
fn a_group_drag_released_beyond_the_page_finishes_once() {
    let mut h = harness();
    let before = objects(&h);
    marquee(&mut h);
    let (a, b) = (at(&h, 55.0, 70.0), at(&h, 415.0, 100.0));
    drag(&mut h, a, b);
    let after = objects(&h);
    for (old, new) in before.iter().zip(&after) {
        let delta = if old.text.as_deref() == Some("Keep here") { 0.0 } else { 360.0 };
        assert!((new.rect[0] - old.rect[0] - delta).abs() < 0.01);
    }
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(4);
    assert_eq!(objects(&h), before);
}

#[test]
fn group_selection_can_be_extended_by_an_additive_marquee() {
    let mut h = harness();
    let p = at(&h, 55.0, 70.0);
    click(&mut h, p, Modifiers::SHIFT);
    h.get_by_label("1 object selected");
    let (a, b) = (at(&h, 25.0, 120.0), at(&h, 235.0, 175.0));
    h.hover_at(a);
    h.run_steps(2);
    h.event_modifiers(
        egui::Event::PointerButton { pos: a, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::SHIFT },
        Modifiers::SHIFT,
    );
    h.run_steps(2);
    h.hover_at(b);
    h.run_steps(2);
    h.drop_at(b);
    h.run_steps(4);
    h.get_by_label("3 objects selected");
    assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty);
}

#[test]
fn group_drag_respects_document_rotation_and_view_rotation() {
    for document_rotation in [0, 90, 180, 270] {
        let mut h = harness();
        assert!(h.state_mut().apply_edit(Edit::RotatePages { pages: vec![0], degrees: document_rotation }));
        h.state_mut().views[0].rotate_view(true);
        h.run_steps(4);
        let before = objects(&h);
        let targets: Vec<_> = before.iter().filter(|o| o.text.as_deref() != Some("Keep here")).map(|o| o.target).collect();
        for target in &targets {
            let app = h.state();
            let doc = app.session.get(app.views[0].id).unwrap();
            let o = before.iter().find(|o| o.target == *target).unwrap();
            let p = pdfcraft_ui_egui::canvas::PageXform {
                rect: app.views[0].page_screen_rect(0).unwrap(),
                rot: 90,
                pw: doc.info.pages[0].width,
                ph: doc.info.pages[0].height,
            }
            .user_rect(&doc.info, 0, o.rect.map(|v| v as f32))
            .center();
            click(&mut h, p, Modifiers::SHIFT);
        }
        h.get_by_label("3 objects selected");
        let app = h.state();
        let doc = app.session.get(app.views[0].id).unwrap();
        let xf = pdfcraft_ui_egui::canvas::PageXform {
            rect: app.views[0].page_screen_rect(0).unwrap(),
            rot: 90,
            pw: doc.info.pages[0].width,
            ph: doc.info.pages[0].height,
        };
        let start =
            xf.user_rect(&doc.info, 0, before.iter().find(|o| o.text.as_deref() == Some("Group headline")).unwrap().rect.map(|v| v as f32)).center();
        let end = start + egui::vec2(21.0, 33.0);
        let from = xf.screen_to_view(start);
        let to = xf.screen_to_view(end);
        let delta = doc.object_move_offset(0, [f64::from(to.0 - from.0), f64::from(to.1 - from.1)]).unwrap();
        drag(&mut h, start, end);
        for (old, new) in before.iter().zip(objects(&h)) {
            let expected = if targets.contains(&old.target) { delta } else { [0.0, 0.0] };
            for i in 0..4 {
                assert!((new.rect[i] - old.rect[i] - expected[i % 2]).abs() < 0.01, "rotation {document_rotation}");
            }
        }
    }
}

#[test]
fn group_drags_cancel_on_focus_loss_tab_switch_home_and_modal_dialogs() {
    for case in 0..5 {
        let mut h = harness();
        let before = objects(&h);
        marquee(&mut h);
        let (a, b) = (at(&h, 55.0, 70.0), at(&h, 90.0, 110.0));
        h.hover_at(a);
        h.run_steps(2);
        h.drag_at(a);
        h.run_steps(2);
        h.hover_at(b);
        h.run_steps(2);
        match case {
            0 => h.input_mut().focused = false,
            1 => {
                h.state_mut().open_bytes("other.pdf", None, fixture(false)).unwrap();
            }
            2 => h.state_mut().active = None,
            3 => h.state_mut().dialog = Some(pdfcraft_ui_egui::Dialog::About),
            _ => h.state_mut().request_document_url("https://example.invalid/", pdfcraft_ui_egui::LinkOrigin::Link),
        }
        h.run_steps(2);
        h.drop_at(b);
        h.run_steps(2);
        h.input_mut().focused = true;
        h.state_mut().active = Some(0);
        h.state_mut().dialog = None;
        h.state_mut().pending_link = None;
        h.run_steps(4);
        assert_eq!(objects(&h), before, "cancelled gesture case {case}");
        assert!(!h.state().session.get(h.state().views[0].id).unwrap().dirty, "case {case}");
        h.get_by_label("3 objects selected");
    }
}

#[test]
fn modifier_selection_picks_foreground_text_and_added_items_over_background_artwork() {
    let mut h = harness_document(false, true);
    let before = objects(&h);
    assert_eq!(before.len(), 5);
    for (x, y) in [(55.0, 70.0), (150.0, 135.0), (60.0, 145.0)] {
        let p = at(&h, x, y);
        click(&mut h, p, Modifiers::SHIFT);
    }
    h.get_by_label("3 objects selected");
    let (a, b) = (at(&h, 55.0, 70.0), at(&h, 80.0, 100.0));
    drag(&mut h, a, b);
    let after = objects(&h);
    for old in before {
        let new = after.iter().find(|o| o.target == old.target).unwrap();
        let stationary = old.text.as_deref() == Some("Keep here") || (old.target.kind == pdfcraft_engine::ObjectKind::Image && old.target.index == 0);
        let shift = if stationary { [0.0, 0.0] } else { [25.0, -30.0] };
        assert!(new.rect.iter().enumerate().all(|(i, value)| (value - old.rect[i] - shift[i % 2]).abs() < 0.01), "{old:?} -> {new:?}");
    }
}
