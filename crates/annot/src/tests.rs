use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef, Object, SaveOptions, write_incremental};

use super::*;

/// Two pages. Page 1 has an inline Square annotation in a direct /Annots array; page 2 has an
/// indirect /Annots array holding a Stamp (a type we cannot redraw).
fn fixture() -> Document {
    let objs: Vec<&[u8]> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>",                                       // 1
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 612 792] >>", // 2
        b"<< /Type /Page /Parent 2 0 R /Annots [<< /Type /Annot /Subtype /Square /Rect [10 10 50 50] /C [1 0 0] >>] >>", // 3
        b"<< /Type /Page /Parent 2 0 R /Annots 5 0 R >>",                           // 4
        b"[6 0 R]",                                                                 // 5
        b"<< /Type /Annot /Subtype /Stamp /Rect [100 100 200 150] /Name /Approved /Foo (kept) >>", // 6
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).expect("opens")
}

fn meta(id: &str) -> Meta {
    Meta { date: Some("D:20261001120000Z".into()), id: id.into() }
}

fn new(page: usize, shape: Shape) -> NewAnnotation {
    NewAnnotation {
        page,
        shape,
        style: Style { color: [0.0, 0.4, 1.0], opacity: 1.0, width: 2.0, fill: None },
        contents: "hello".into(),
        author: "Ada".into(),
    }
}

/// Save incrementally and reopen, so every check runs against what a reader would see.
fn reopen(doc: &Document) -> Document {
    let bytes = write_incremental(doc, &SaveOptions::default()).expect("writes");
    // A second, independent parser must accept the result too.
    hayro_syntax::Pdf::new(bytes.clone()).expect("hayro-syntax parses the output");
    Document::open(Arc::new(bytes)).expect("reopens")
}

fn list(doc: &Document, page: usize) -> Vec<Dict> {
    let p = page_refs(doc).unwrap()[page];
    annots(doc, p).iter().map(|o| doc.resolve(o).as_dict().cloned().unwrap()).collect()
}

fn ap_content(doc: &Document, d: &Dict) -> String {
    let n = d.get(b"AP").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().and_then(|a| a.reference(b"N"))).expect("has /AP /N");
    let obj = doc.get(n);
    let Object::Stream(s) = &*obj else { panic!("AP is not a stream") };
    String::from_utf8_lossy(&s.decoded().unwrap()).into_owned()
}

fn text(d: &Dict, k: &[u8]) -> String {
    d.get(k).and_then(|o| o.as_string()).map(|s| s.to_text()).unwrap_or_default()
}

fn rect(d: &Dict) -> Vec<f64> {
    d.get(b"Rect").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect()
}

#[test]
fn every_tool_creates_a_drawable_comment() {
    let shapes = [
        Shape::Note { at: [100.0, 700.0], icon: NoteIcon::Comment },
        Shape::TextMarkup { kind: Markup::Highlight, quads: vec![[72.0, 712.0, 200.0, 712.0, 72.0, 700.0, 200.0, 700.0]] },
        Shape::TextMarkup { kind: Markup::Underline, quads: vec![[72.0, 612.0, 200.0, 612.0, 72.0, 600.0, 200.0, 600.0]] },
        Shape::TextMarkup { kind: Markup::StrikeOut, quads: vec![[72.0, 512.0, 200.0, 512.0, 72.0, 500.0, 200.0, 500.0]] },
        Shape::TextMarkup { kind: Markup::Squiggly, quads: vec![[72.0, 412.0, 200.0, 412.0, 72.0, 400.0, 200.0, 400.0]] },
        Shape::Rectangle { rect: [300.0, 300.0, 200.0, 200.0] },
        Shape::Oval { rect: [300.0, 300.0, 400.0, 350.0] },
        Shape::Line { from: [50.0, 50.0], to: [150.0, 100.0], start: LineEnding::None, end: LineEnding::OpenArrow },
        Shape::Ink { strokes: vec![vec![[10.0, 10.0], [20.0, 30.0], [30.0, 10.0]], vec![[40.0, 40.0]]] },
        Shape::TextBox { rect: [300.0, 600.0, 500.0, 650.0], font_size: 14.0 },
    ];
    let mut doc = fixture();
    for (i, s) in shapes.iter().enumerate() {
        let idx = add_annotation(&mut doc, &new(0, s.clone()), &meta(&format!("id-{i}"))).expect("adds");
        assert!(idx >= 1, "appended after the existing inline annotation");
    }
    let doc = reopen(&doc);
    let all = list(&doc, 0);
    // The inline square, 10 comments and the sticky note's pop-up.
    assert_eq!(all.len(), 12);
    let subtypes: Vec<&[u8]> = all.iter().map(|d| d.name(b"Subtype").unwrap()).collect();
    assert_eq!(
        subtypes,
        [
            &b"Square"[..],
            b"Text",
            b"Popup",
            b"Highlight",
            b"Underline",
            b"StrikeOut",
            b"Squiggly",
            b"Square",
            b"Circle",
            b"Line",
            b"Ink",
            b"FreeText"
        ]
    );
    for d in all.iter().filter(|d| d.contains(b"NM")) {
        assert_eq!(text(d, b"T"), "Ada");
        assert_eq!(text(d, b"Contents"), "hello");
        assert!(d.reference(b"P").is_some(), "/P points at the page");
        assert!(!ap_content(&doc, d).is_empty(), "{:?} has an appearance", d.name(b"Subtype"));
    }
    // Highlight multiplies; the rectangle was normalized.
    let hl = &all[3];
    let ap = hl.get(b"AP").unwrap().as_dict().unwrap().reference(b"N").unwrap();
    let res = doc.get(ap).as_dict().unwrap().get(b"Resources").cloned().unwrap();
    let gs = res.as_dict().unwrap().get(b"ExtGState").unwrap().as_dict().unwrap().get(b"GS0").unwrap().as_dict().unwrap().clone();
    assert_eq!(gs.name(b"BM"), Some(&b"Multiply"[..]));
    assert_eq!(rect(&all[7]), [200.0, 200.0, 300.0, 300.0]);
    // The arrow's rectangle includes the arrowhead and the stroke.
    let r = rect(&all[9]);
    assert!(r[0] < 50.0 && r[2] > 150.0 + 1.0);
    // The text box draws its contents in Helvetica.
    assert!(ap_content(&doc, &all[11]).contains("(hello) Tj"));
    assert!(ap_content(&doc, &all[11]).contains("/Helv 14 Tf"));
    // The note is a fixed-size icon with a closed pop-up.
    assert_eq!(all[1].int(b"F"), Some(28));
    let entries = annots(&doc, page_refs(&doc).unwrap()[0]);
    assert_eq!(all[2].reference(b"Parent"), entries[1].as_ref());
    assert_eq!(all[1].reference(b"Popup"), entries[2].as_ref());
}

/// #260 (page 7): Acrobat draws an Ink annotation as a smooth curve through its `/InkList`
/// points, where PdfCraft drew straight segments between them. Three or more points now make a
/// Catmull-Rom spline through every point (as cubic Béziers); two points stay a line, one a dot.
#[test]
fn ink_strokes_are_smooth_curves_through_their_points() {
    let mut doc = fixture();
    let strokes = vec![vec![[0.0, 0.0], [10.0, 10.0], [20.0, 0.0]], vec![[30.0, 0.0], [40.0, 10.0]], vec![[50.0, 50.0]]];
    let i = add_annotation(&mut doc, &new(0, Shape::Ink { strokes }), &meta("ink")).unwrap();
    let ap = ap_content(&doc, &list(&doc, 0)[i]);
    // Tangents at each point run parallel to the chord between its neighbours (the ends use
    // their own point): (0,0)→(10,10) leaves along (10,10) and arrives along (20,0).
    assert!(ap.contains("0 0 m\n1.667 1.667 6.667 10 10 10 c\n13.333 10 18.333 1.667 20 0 c\nS\n"), "{ap}");
    assert!(ap.contains("30 0 m\n40 10 l\nS\n"), "{ap}");
    assert!(ap.contains("50 50 m\n50.01 50 l\nS\n"), "{ap}");
    // /Rect holds the whole curve: rising to (20, 20) and dropping to (21, 0), it swings above
    // y 20 (its control point is at y 21.667), beyond the points' own bounds plus the margin.
    let rise = Shape::Ink { strokes: vec![vec![[0.0, 0.0], [10.0, 10.0], [20.0, 20.0], [21.0, 0.0]]] };
    let j = add_annotation(&mut doc, &new(0, rise), &meta("rise")).unwrap();
    let top = rect(&list(&doc, 0)[j])[3];
    assert!(top >= 21.667 + 2.0 - 0.01, "/Rect top {top}");
}

#[test]
fn indirect_annots_array_is_updated_in_place() {
    let mut doc = fixture();
    add_annotation(&mut doc, &new(1, Shape::Rectangle { rect: [0.0, 0.0, 10.0, 10.0] }), &meta("x")).unwrap();
    let doc = reopen(&doc);
    let page = doc.get(page_refs(&doc).unwrap()[1]);
    assert_eq!(page.as_dict().unwrap().reference(b"Annots"), Some(ObjRef::new(5, 0)));
    assert_eq!(doc.get(ObjRef::new(5, 0)).as_array().unwrap().len(), 2);
}

#[test]
fn invalid_geometry_is_rejected() {
    let mut doc = fixture();
    let bad = [
        Shape::TextMarkup { kind: Markup::Highlight, quads: vec![] },
        Shape::Rectangle { rect: [0.0, 0.0, 0.5, 10.0] },
        Shape::Line { from: [1.0, 1.0], to: [1.0, 1.0], start: LineEnding::None, end: LineEnding::None },
        Shape::Ink { strokes: vec![vec![]] },
        Shape::Note { at: [f64::NAN, 0.0], icon: NoteIcon::Note },
    ];
    for s in bad {
        assert!(matches!(add_annotation(&mut doc, &new(0, s.clone()), &meta("x")), Err(AnnotError::Invalid(_))), "{s:?}");
    }
    assert_eq!(add_annotation(&mut doc, &new(5, Shape::Rectangle { rect: [0.0, 0.0, 10.0, 10.0] }), &meta("x")), Err(AnnotError::NoSuchPage(5)));
    assert!(!doc.is_modified(), "failed adds change nothing");
}

#[test]
fn replies_and_status_thread_onto_the_parent() {
    let mut doc = fixture();
    // Replying to the inline square promotes it to an indirect object.
    let r = add_reply(&mut doc, 0, 0, "Agreed", "Bob", &meta("r1")).unwrap();
    let s = set_review_state(&mut doc, 0, 0, ReviewState::Accepted, "Bob", &meta("r2")).unwrap();
    assert_eq!((r, s), (1, 2));
    let doc = reopen(&doc);
    let p = page_refs(&doc).unwrap()[0];
    let entries = annots(&doc, p);
    let parent = entries[0].as_ref().expect("promoted to an indirect object");
    let all = list(&doc, 0);
    assert_eq!(all[1].reference(b"IRT"), Some(parent));
    assert_eq!(text(&all[1], b"Contents"), "Agreed");
    assert_eq!(rect(&all[1]), [10.0, 10.0, 50.0, 50.0]);
    assert_eq!(ap_content(&doc, &all[1]), "", "replies draw nothing");
    assert_eq!(text(&all[2], b"State"), "Accepted");
    assert_eq!(text(&all[2], b"StateModel"), "Review");
    assert_eq!(text(&all[2], b"Contents"), "Accepted set by Bob");
}

#[test]
fn delete_takes_popup_and_reply_chain() {
    let mut doc = fixture();
    let note = add_annotation(&mut doc, &new(0, Shape::Note { at: [100.0, 100.0], icon: NoteIcon::Comment }), &meta("n")).unwrap();
    let reply = add_reply(&mut doc, 0, note, "first", "Bob", &meta("r1")).unwrap();
    add_reply(&mut doc, 0, reply, "reply to reply", "Ada", &meta("r2")).unwrap();
    add_reply(&mut doc, 0, 0, "on the square", "Ada", &meta("r3")).unwrap();
    assert_eq!(list(&doc, 0).len(), 6);
    delete_annotation(&mut doc, 0, note).unwrap();
    let doc = reopen(&doc);
    let left: Vec<String> =
        list(&doc, 0).iter().map(|d| format!("{}:{}", String::from_utf8_lossy(d.name(b"Subtype").unwrap()), text(d, b"Contents"))).collect();
    assert_eq!(left, ["Square:", "Text:on the square"]);
    let mut doc = doc;
    assert_eq!(delete_annotation(&mut doc, 0, 9), Err(AnnotError::NoSuchAnnotation { page: 0, index: 9 }));
}

#[test]
fn deleting_the_last_comment_removes_annots() {
    let mut doc = fixture();
    delete_annotation(&mut doc, 0, 0).unwrap();
    let doc = reopen(&doc);
    assert!(!doc.get(page_refs(&doc).unwrap()[0]).as_dict().unwrap().contains(b"Annots"));
}

#[test]
fn move_shifts_all_geometry_and_the_popup() {
    let mut doc = fixture();
    let q = [72.0, 712.0, 200.0, 712.0, 72.0, 700.0, 200.0, 700.0];
    let h = add_annotation(&mut doc, &new(0, Shape::TextMarkup { kind: Markup::Highlight, quads: vec![q] }), &meta("h")).unwrap();
    let i = add_annotation(&mut doc, &new(0, Shape::Ink { strokes: vec![vec![[10.0, 10.0], [20.0, 20.0]]] }), &meta("i")).unwrap();
    let n = add_annotation(&mut doc, &new(0, Shape::Note { at: [100.0, 100.0], icon: NoteIcon::Note }), &meta("n")).unwrap();
    for idx in [h, i, n] {
        move_annotation(&mut doc, 0, idx, 10.0, -5.0, &meta("")).unwrap();
    }
    let doc = reopen(&doc);
    let all = list(&doc, 0);
    let qp: Vec<f64> = all[h].get(b"QuadPoints").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect();
    assert_eq!(qp, [82.0, 707.0, 210.0, 707.0, 82.0, 695.0, 210.0, 695.0]);
    let ink = all[i].get(b"InkList").unwrap().as_array().unwrap()[0].as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect::<Vec<_>>();
    assert_eq!(ink, [20.0, 5.0, 30.0, 15.0]);
    assert_eq!(rect(&all[n]), [110.0, 75.0, 130.0, 95.0]);
    let popup = &all[n + 1];
    assert_eq!(rect(popup)[0], 140.0);
}

#[test]
fn restyle_and_resize_redraw_the_appearance() {
    let mut doc = fixture();
    let r = add_annotation(&mut doc, &new(0, Shape::Rectangle { rect: [10.0, 10.0, 110.0, 60.0] }), &meta("r")).unwrap();
    set_style(&mut doc, 0, r, Some([1.0, 0.0, 0.0]), Some(0.5), Some(4.0), None, &meta("")).unwrap();
    set_rect(&mut doc, 0, r, [0.0, 0.0, 200.0, 100.0], &meta("")).unwrap();
    let doc2 = reopen(&doc);
    let d = &list(&doc2, 0)[r];
    let ap = ap_content(&doc2, d);
    assert!(ap.contains("1 0 0 RG") && ap.contains("4 w") && ap.contains("/GS0 gs"), "{ap}");
    assert!(ap.contains("2 2 196 96 re"), "inset by half the border: {ap}");
    assert_eq!(d.get(b"CA").and_then(|o| o.as_f64()), Some(0.5));
    // A stamp's appearance can't be regenerated, so restyling it changes nothing.
    let before = list(&doc, 1)[0].clone();
    assert_eq!(set_style(&mut doc, 1, 0, Some([0.0; 3]), None, None, None, &meta("")), Err(AnnotError::Unsupported("Stamp".into())));
    assert_eq!(list(&doc, 1)[0], before);
    // Lines can't be resized as rectangles.
    let l = add_annotation(
        &mut doc,
        &new(0, Shape::Line { from: [0.0, 0.0], to: [10.0, 10.0], start: LineEnding::None, end: LineEnding::None }),
        &meta("l"),
    )
    .unwrap();
    assert!(matches!(set_rect(&mut doc, 0, l, [0.0, 0.0, 5.0, 5.0], &meta("")), Err(AnnotError::Invalid(_))));
}

#[test]
fn shapes_can_be_filled_and_unfilled() {
    let mut doc = fixture();
    let r = add_annotation(&mut doc, &new(0, Shape::Rectangle { rect: [10.0, 10.0, 110.0, 60.0] }), &meta("r")).unwrap();
    let p = props(&doc, 0, r).unwrap();
    assert!(p.fillable);
    assert_eq!(p.fill, None);
    set_fill(&mut doc, 0, r, Some([0.0, 0.5, 1.0]), &meta("")).unwrap();
    let doc2 = reopen(&doc);
    let d = &list(&doc2, 0)[r];
    let ap = ap_content(&doc2, d);
    assert!(ap.contains("0 0.5 1 rg") && ap.contains("\nB\n"), "filled and stroked: {ap}");
    assert_eq!(props(&doc2, 0, r).unwrap().fill, Some([0.0, 0.5, 1.0]));
    // No fill again: /IC goes and the appearance only strokes.
    set_fill(&mut doc, 0, r, None, &meta("")).unwrap();
    let doc2 = reopen(&doc);
    let d = &list(&doc2, 0)[r];
    assert!(d.get(b"IC").is_none());
    let ap = ap_content(&doc2, d);
    assert!(!ap.contains(" rg") && ap.contains("\nS\n"), "stroked only: {ap}");
    assert_eq!(props(&doc2, 0, r).unwrap().fill, None);
    // Ovals and polygons fill too.
    let o = add_annotation(&mut doc, &new(0, Shape::Oval { rect: [10.0, 10.0, 60.0, 60.0] }), &meta("o")).unwrap();
    set_fill(&mut doc, 0, o, Some([1.0, 1.0, 0.0]), &meta("")).unwrap();
    assert!(ap_content(&doc, &list(&doc, 0)[o]).contains("1 1 0 rg"));
    let vertices = vec![[0.0, 0.0], [50.0, 0.0], [25.0, 40.0]];
    let g = add_annotation(&mut doc, &new(0, Shape::Polygon { vertices, cloud: false }), &meta("g")).unwrap();
    set_fill(&mut doc, 0, g, Some([1.0, 0.0, 0.0]), &meta("")).unwrap();
    assert!(ap_content(&doc, &list(&doc, 0)[g]).contains("1 0 0 rg"));
    // Lines have no interior, and bad colours change nothing.
    let l = add_annotation(
        &mut doc,
        &new(0, Shape::Line { from: [0.0, 0.0], to: [10.0, 10.0], start: LineEnding::None, end: LineEnding::None }),
        &meta("l"),
    )
    .unwrap();
    assert!(!props(&doc, 0, l).unwrap().fillable);
    assert!(matches!(set_fill(&mut doc, 0, l, Some([1.0; 3]), &meta("")), Err(AnnotError::Invalid(_))));
    let before = list(&doc, 0)[r].clone();
    assert!(matches!(set_fill(&mut doc, 0, r, Some([f64::NAN, 0.0, 0.0]), &meta("")), Err(AnnotError::Invalid(_))));
    assert_eq!(list(&doc, 0)[r], before);
    assert!(set_fill(&mut doc, 0, 99, None, &meta("")).is_err());
}

#[test]
fn text_box_text_and_colour_changes_are_drawn() {
    let mut doc = fixture();
    let t = add_annotation(&mut doc, &new(0, Shape::TextBox { rect: [0.0, 0.0, 300.0, 100.0], font_size: 12.0 }), &meta("t")).unwrap();
    set_contents(&mut doc, 0, t, "Caf\u{e9} (draft) \u{2014} 100%", &meta("")).unwrap();
    set_style(&mut doc, 0, t, Some([1.0, 0.0, 0.0]), None, None, None, &meta("")).unwrap();
    let doc = reopen(&doc);
    let d = &list(&doc, 0)[t];
    assert_eq!(text(d, b"Contents"), "Caf\u{e9} (draft) \u{2014} 100%");
    assert_eq!(text(d, b"DA"), "1 0 0 rg /Helv 12 Tf");
    let n = d.get(b"AP").unwrap().as_dict().unwrap().reference(b"N").unwrap();
    let Object::Stream(s) = &*doc.get(n) else { panic!() };
    let raw = s.decoded().unwrap();
    // WinAnsi bytes, with the parentheses escaped.
    let needle = b"(Caf\xe9 \\(draft\\) \x97 100%) Tj";
    assert!(raw.windows(needle.len()).any(|w| w == needle), "{}", String::from_utf8_lossy(&raw));
    assert!(String::from_utf8_lossy(&raw).contains("1 0 0 rg"));
    // Rich text follows the plain text and the style.
    assert_eq!(text(d, b"DS"), "font: Helvetica 12pt; text-align:left; color:#FF0000");
    let rc = text(d, b"RC");
    assert!(rc.starts_with("<?xml") && rc.contains("<p dir=\"ltr\">Caf\u{e9} (draft) \u{2014} 100%</p>") && rc.contains("color:#FF0000"), "{rc}");
}

/// The rectangle a text box needs for `text` (the appearance's padding plus the 2 pt slack):
/// the wrap width and top edge stay, the height fits the wrapped lines.
fn expected_box(top: f64, w: f64, text: &str, size: f64, border: f64) -> [f64; 4] {
    let pad = 2.0 + border;
    let lines = appearance::wrap(text, size, w - 2.0 * pad).len().max(1) as f64;
    [0.0, top - lines * size * 1.2 - 2.0 * pad - 2.0, w, 0.0]
}

#[test]
fn editing_a_text_box_refits_its_rectangle() {
    let mut doc = fixture();
    let t = add_annotation(&mut doc, &new(0, Shape::TextBox { rect: [100.0, 680.0, 304.0, 692.0], font_size: 12.0 }), &meta("t")).unwrap();
    // The style's border is 2 pt wide, so the text is padded by 2 + 2.
    let long = "the quick brown fox jumps over the lazy dog and keeps going";
    set_contents(&mut doc, 0, t, long, &meta("")).unwrap();
    let d = &list(&doc, 0)[t];
    let r = rect(d);
    assert_eq!((r[0], r[2], r[3]), (100.0, 304.0, 692.0), "width and top edge stay");
    let want = expected_box(692.0, 204.0, long, 12.0, 2.0);
    assert_eq!(r[1], want[1], "the height fits the wrapped lines");
    // The appearance covers the new rectangle, and every wrapped line is drawn.
    let n = d.get(b"AP").unwrap().as_dict().unwrap().reference(b"N").unwrap();
    let bbox: Vec<f64> = doc.get(n).as_dict().unwrap().get(b"BBox").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect();
    assert_eq!(bbox, r);
    assert_eq!(ap_content(&doc, d).matches(" Tj").count(), appearance::wrap(long, 12.0, 204.0 - 2.0 * 4.0).len(), "nothing is clipped");
    // Shorter text shrinks the box back to one line.
    set_contents(&mut doc, 0, t, "Hi", &meta("")).unwrap();
    let r = rect(&list(&doc, 0)[t]);
    assert_eq!(r, [100.0, expected_box(692.0, 204.0, "Hi", 12.0, 2.0)[1], 304.0, 692.0]);
}

#[test]
fn editing_a_callout_refits_the_box_and_keeps_the_leader() {
    let mut doc = fixture();
    let c = add_annotation(
        &mut doc,
        &new(
            0,
            Shape::Callout {
                rect: [300.0, 600.0, 500.0, 612.0],
                knee: [260.0, 560.0],
                point: [200.0, 520.0],
                font_size: 12.0,
                ending: LineEnding::OpenArrow,
            },
        ),
        &meta("c"),
    )
    .unwrap();
    let long = "the quick brown fox jumps over the lazy dog and keeps going";
    set_contents(&mut doc, 0, c, long, &meta("")).unwrap();
    let d = &list(&doc, 0)[c];
    let outer = rect(d);
    let f = |k: &[u8]| -> Vec<f64> { d.get(k).unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect() };
    let cl = f(b"CL");
    // The leader line keeps its start and knee; the attach point follows the re-fitted box.
    assert_eq!(&cl[..4], &[200.0, 520.0, 260.0, 560.0]);
    let want = expected_box(612.0, 200.0, long, 12.0, 2.0);
    assert_eq!((cl[4], cl[5]), callout_attach([300.0, want[1], 500.0, 612.0], [260.0, 560.0]).into());
    // The text box (recovered from /Rect and /RD) keeps its width and top edge.
    let tb = [outer[0] + f(b"RD")[0], outer[1] + f(b"RD")[1], outer[2] - f(b"RD")[2], outer[3] - f(b"RD")[3]];
    assert_eq!((tb[0], tb[2], tb[3]), (300.0, 500.0, 612.0));
    assert_eq!(tb[1], want[1]);
    // /Rect still grows to hold the leader line: its padded bounds are [186, 506, 274, 574].
    assert_eq!((outer[0], outer[2], outer[3]), (186.0, 500.0, 612.0));
    assert_eq!(ap_content(&doc, d).matches(" Tj").count(), appearance::wrap(long, 12.0, 200.0 - 2.0 * 4.0).len(), "nothing is clipped");
}

#[test]
fn real_annotation_flags_do_not_lock_comments() {
    let mut doc = fixture();
    let page = page_refs(&doc).unwrap()[0];
    let mut entries = annots(&doc, page);
    let Object::Dict(annotation) = &mut entries[0] else { panic!("fixture annotation is inline") };
    annotation.set(b"F".to_vec(), Object::Real(128.5));
    set_annots(&mut doc, page, entries).unwrap();

    assert!(!summaries(&doc)[0].locked, "a non-integer /F is outside the annotation flag domain");
    assert!(!props(&doc, 0, 0).unwrap().locked);
    assert!(set_info(&mut doc, 0, 0, Some("Ada"), None, None, &meta("")).is_ok());
}

#[test]
fn a_locked_text_box_still_refits_its_text() {
    let mut doc = fixture();
    let t = add_annotation(&mut doc, &new(0, Shape::TextBox { rect: [100.0, 680.0, 304.0, 692.0], font_size: 12.0 }), &meta("t")).unwrap();
    set_locked(&mut doc, 0, t, true).unwrap();
    // Resizing is refused, but the text (and its re-fit) stays editable, as in Acrobat.
    assert!(matches!(set_rect(&mut doc, 0, t, [0.0, 0.0, 50.0, 50.0], &meta("")), Err(AnnotError::Invalid(_))));
    let long = "the quick brown fox jumps over the lazy dog";
    set_contents(&mut doc, 0, t, long, &meta("")).unwrap();
    let r = rect(&list(&doc, 0)[t]);
    assert_eq!(r[3], 692.0);
    let want = expected_box(692.0, 204.0, long, 12.0, 2.0);
    assert_eq!(r[1], want[1], "the box grew to fit the wrapped lines");
}

#[test]
fn unknown_keys_survive_edits() {
    let mut doc = fixture();
    move_annotation(&mut doc, 1, 0, 1.0, 1.0, &meta("")).unwrap();
    set_contents(&mut doc, 1, 0, "note", &meta("")).unwrap();
    let doc = reopen(&doc);
    let d = &list(&doc, 1)[0];
    assert_eq!(text(d, b"Foo"), "kept");
    assert_eq!(d.name(b"Name"), Some(&b"Approved"[..]));
    assert_eq!(rect(d), [101.0, 101.0, 201.0, 151.0]);
}

#[test]
fn wrapping_and_widths() {
    assert!(appearance::text_width("MMMM", 10.0) > appearance::text_width("iiii", 10.0) * 2.0);
    let lines = appearance::wrap("the quick brown fox jumps over the lazy dog", 12.0, 80.0);
    assert!(lines.len() > 2);
    assert!(lines.iter().all(|l| appearance::text_width(l, 12.0) <= 80.0));
    assert_eq!(appearance::wrap("a\nb", 12.0, 100.0), ["a", "b"]);
    // A word longer than the line is broken.
    let long = appearance::wrap("Supercalifragilisticexpialidocious", 12.0, 40.0);
    assert!(long.len() > 3 && long.concat() == "Supercalifragilisticexpialidocious");
    assert_eq!(n(1.0), "1");
    assert_eq!(n(-0.0001), "0");
    assert_eq!(n(2.50), "2.5");
}

#[test]
fn fill_and_sign_items_are_drawn() {
    let mut doc = fixture();
    let add = |doc: &mut Document, shape: Shape, contents: &str| {
        let style = Style::default_for(&shape);
        add_annotation(doc, &NewAnnotation { page: 1, shape, style, contents: contents.into(), author: "Ada".into() }, &meta("x")).unwrap()
    };
    let t = add(&mut doc, Shape::Typewriter { rect: [100.0, 700.0, 200.0, 716.0], font_size: 11.0 }, "Ada Lovelace");
    let marks: Vec<usize> = [FillMark::Check, FillMark::Cross, FillMark::Dot, FillMark::Line]
        .into_iter()
        .enumerate()
        .map(|(k, m)| add(&mut doc, Shape::Mark { rect: [100.0 + 20.0 * k as f64, 600.0, 114.0 + 20.0 * k as f64, 614.0], mark: m }, ""))
        .collect();
    let s = add(&mut doc, Shape::Signature { strokes: vec![vec![[10.0, 10.0], [30.0, 20.0], [50.0, 10.0]]] }, "");
    let doc = reopen(&doc);
    let all = list(&doc, 1);
    assert_eq!(all[t].name(b"IT"), Some(&b"FreeTextTypeWriter"[..]));
    assert!(ap_content(&doc, &all[t]).contains("(Ada Lovelace) Tj"));
    for (k, m) in marks.iter().enumerate() {
        assert_eq!(all[*m].name(b"Subtype"), Some(&b"Stamp"[..]));
        let ap = ap_content(&doc, &all[*m]);
        assert!(ap.contains(if k == 2 { " c" } else { " l" }), "mark {k}: {ap}");
    }
    assert_eq!(text(&all[marks[0]], b"Subj"), "Checkmark");
    assert_eq!(text(&all[s], b"Subj"), "Signature");
    // Other stamps still can't be restyled; ours can.
    let mut doc = doc;
    set_style(&mut doc, 1, marks[0], Some([0.0, 0.0, 1.0]), None, None, None, &meta("")).unwrap();
    assert!(set_style(&mut doc, 1, 0, Some([0.0; 3]), None, None, None, &meta("")).is_err());
}

#[test]
fn author_subject_and_note_icon_change() {
    let mut doc = fixture();
    let n = add_annotation(&mut doc, &new(0, Shape::Note { at: [10.0, 100.0], icon: NoteIcon::Comment }), &meta("n")).unwrap();
    let before = ap_content(&doc, &list(&doc, 0)[n]);
    set_info(&mut doc, 0, n, Some("Grace"), Some("Question"), Some(NoteIcon::Help), &meta("")).unwrap();
    let d = &list(&doc, 0)[n];
    assert_eq!((text(d, b"T").as_str(), text(d, b"Subj").as_str()), ("Grace", "Question"));
    assert_eq!(d.name(b"Name"), Some(&b"Help"[..]));
    assert_ne!(ap_content(&doc, d), before, "the icon was redrawn");
    assert!(matches!(set_info(&mut doc, 0, 0, None, None, Some(NoteIcon::Key), &meta("")), Err(AnnotError::Invalid(_))));
}

#[test]
fn stamps_draw_their_label_and_by_line() {
    let mut doc = fixture();
    let shape =
        Shape::Stamp { rect: [100.0, 100.0, 260.0, 142.0], stamp: StampKind::DynApproved, by: Some("By Ada at 2:14 pm, Oct 02, 2026".into()) };
    let i = add_annotation(
        &mut doc,
        &NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: String::new(), author: "Ada".into() },
        &Meta::default(),
    )
    .unwrap();
    let s = &summaries(&doc)[0];
    assert_eq!((s.subtype.as_str(), s.index), ("Stamp", i));
    let ap = ap_content(&doc, &list(&doc, 0)[i]);
    assert!(ap.contains("(APPROVED) Tj") && ap.contains("(By Ada at 2:14 pm, Oct 02, 2026) Tj") && ap.contains("/HelvB"), "{ap}");
    let sign = Shape::Stamp { rect: [100.0, 300.0, 220.0, 330.0], stamp: StampKind::SignHere, by: None };
    let j = add_annotation(
        &mut doc,
        &NewAnnotation { page: 0, style: Style::default_for(&sign), shape: sign, contents: String::new(), author: "Ada".into() },
        &Meta::default(),
    )
    .unwrap();
    assert!(ap_content(&doc, &list(&doc, 0)[j]).contains("(SIGN HERE) Tj"));
    assert_eq!(StampKind::from_name(b"Approved"), Some(StampKind::Approved), "standard names");
    assert!(StampKind::ALL.iter().all(|k| k.size().0 >= 80.0));
}

#[test]
fn links_are_added_edited_listed_and_removed() {
    use crate::links::{self, Highlight, LinkAction, LinkStyle};
    let mut doc = fixture();
    let before = links::list(&doc).len();
    let i = links::add(&mut doc, 0, [10.0, 10.0, 110.0, 30.0], &LinkAction::Uri("https://example.org".into()), &LinkStyle::default()).unwrap();
    let style = LinkStyle { visible: true, color: [1.0, 0.0, 0.0], width: 2.0, dashed: true, underline: false, highlight: Highlight::Outline };
    links::add(&mut doc, 1, [10.0, 10.0, 60.0, 30.0], &LinkAction::Page(0), &style).unwrap();
    let doc = reopen(&doc);
    let all = links::list(&doc);
    assert_eq!(all.len(), before + 2);
    let web = all.iter().find(|l| l.page == 0 && l.index == i).unwrap();
    assert_eq!(
        (web.action.clone(), web.style.visible, web.style.highlight),
        (LinkAction::Uri("https://example.org".into()), false, Highlight::Invert)
    );
    let page = all.iter().find(|l| l.page == 1 && l.action == LinkAction::Page(0)).unwrap().clone();
    assert_eq!(page.style, style);
    let mut doc = doc;
    links::set(&mut doc, 1, page.index, Some([0.0, 0.0, 50.0, 20.0]), Some(&LinkAction::Uri("mailto:ada@example.org".into())), None).unwrap();
    let changed = links::list(&doc).into_iter().find(|l| l.page == 1 && l.index == page.index).unwrap();
    assert_eq!((changed.rect, changed.action), ([0.0, 0.0, 50.0, 20.0], LinkAction::Uri("mailto:ada@example.org".into())));
    links::delete(&mut doc, 1, page.index).unwrap();
    assert!(links::delete(&mut doc, 0, 999).is_err(), "no such annotation");
    let n = links::remove_all(&mut doc, None).unwrap();
    assert!(n >= 1 && links::list(&doc).is_empty());
    assert!(links::add(&mut doc, 0, [0.0, 0.0, 1.0, 1.0], &LinkAction::Page(0), &LinkStyle::default()).is_err(), "too small");
}

#[test]
fn urls_are_found_in_text() {
    let t: Vec<char> = "See https://example.org/a?b=1, or www.rust-lang.org. Not a.b or http:/x.".chars().collect();
    let found: Vec<String> = crate::links::find_urls(&t).into_iter().map(|(_, u)| u).collect();
    assert_eq!(found, ["https://example.org/a?b=1", "http://www.rust-lang.org"]);
}

#[test]
fn common_email_addresses_become_mailto_links_with_source_character_ranges() {
    let source = "İ Contact ada.lovelace+forms@example.co.uk, browse HTTPS://EXAMPLE.ORG; or ask admin@example.org.";
    let chars: Vec<char> = source.chars().collect();
    let links = crate::links::find_urls(&chars);
    let found: Vec<(String, String)> = links
        .into_iter()
        .map(|(range, url)| (chars.get(range).unwrap().iter().collect(), url))
        .collect();
    assert_eq!(
        found,
        [
            ("ada.lovelace+forms@example.co.uk".into(), "mailto:ada.lovelace+forms@example.co.uk".into()),
            ("HTTPS://EXAMPLE.ORG".into(), "HTTPS://EXAMPLE.ORG".into()),
            ("admin@example.org".into(), "mailto:admin@example.org".into()),
        ]
    );
}

#[test]
fn malformed_email_addresses_are_not_autolinked() {
    for invalid in ["a@b", "broken@localhost", "a..b@example.org", "foo@.example.org", "foo@example..org", "foo@example.com@x"] {
        let input: Vec<char> = invalid.chars().collect();
        assert!(crate::links::find_urls(&input).is_empty(), "{invalid:?} produced a link");
    }
}

#[test]
fn urls_are_found_after_length_changing_lowercase() {
    // U+0130 (İ) lowercases to "i" plus a combining dot above, so a case-folded copy is longer
    // than the source. The old matcher took the address offset from that copy and sliced the
    // source with it, panicking at `chars[i..j]` (#816).
    let mut text = "\u{130}".repeat(20);
    text.push_str(" http://example.com");
    let chars: Vec<char> = text.chars().collect();
    let found = crate::links::find_urls(&chars);
    assert_eq!(found.len(), 1, "{found:?}");
    let (range, uri) = found.into_iter().next().unwrap();
    assert_eq!(uri, "http://example.com");
    assert_eq!(range, 21..39, "range is in source-character indices");
}

#[test]
fn url_scheme_and_www_match_case_insensitively() {
    let t: Vec<char> = "HTTP://Example.COM".chars().collect();
    let found: Vec<String> = crate::links::find_urls(&t).into_iter().map(|(_, u)| u).collect();
    assert_eq!(found, ["HTTP://Example.COM"]);
    let t: Vec<char> = "\u{130} www.Example.org".chars().collect();
    let found: Vec<String> = crate::links::find_urls(&t).into_iter().map(|(_, u)| u).collect();
    assert_eq!(found, ["http://www.Example.org"]);
}

#[test]
fn polygons_clouds_connected_lines_callouts_and_carets_are_drawn() {
    let mut doc = fixture();
    let tri = vec![[100.0, 100.0], [200.0, 100.0], [150.0, 180.0]];
    let i = add_annotation(&mut doc, &new(0, Shape::Polygon { vertices: tri.clone(), cloud: false }), &meta("p")).unwrap();
    let c = add_annotation(&mut doc, &new(0, Shape::Polygon { vertices: tri.clone(), cloud: true }), &meta("c")).unwrap();
    let l = add_annotation(
        &mut doc,
        &new(0, Shape::PolyLine { vertices: vec![[300.0, 300.0], [350.0, 320.0], [400.0, 300.0]], start: LineEnding::None, end: LineEnding::None }),
        &meta("l"),
    )
    .unwrap();
    let co = add_annotation(
        &mut doc,
        &new(
            0,
            Shape::Callout {
                rect: [300.0, 500.0, 420.0, 540.0],
                knee: [260.0, 520.0],
                point: [220.0, 460.0],
                font_size: 10.0,
                ending: LineEnding::OpenArrow,
            },
        ),
        &meta("co"),
    )
    .unwrap();
    let k = add_annotation(&mut doc, &new(0, Shape::Caret { rect: [50.0, 600.0, 60.0, 612.0] }), &meta("k")).unwrap();
    let doc = reopen(&doc);
    let a = list(&doc, 0);

    let p = &a[i];
    assert_eq!(p.name(b"Subtype"), Some(&b"Polygon"[..]));
    assert_eq!(text(p, b"Subj"), "Polygon");
    let ap = ap_content(&doc, p);
    assert!(ap.contains("100 100 m") && ap.contains("150 180 l") && ap.contains("h\nS"), "{ap}");
    let r = rect(p);
    assert!(r[0] <= 99.0 && r[3] >= 181.0, "{r:?}");

    let cl = &a[c];
    assert_eq!((text(cl, b"Subj").as_str(), cl.name(b"IT")), ("Cloud", Some(&b"PolygonCloud"[..])));
    let ap = ap_content(&doc, cl);
    assert!(ap.matches(" c\n").count() >= 6, "bumps along every edge: {ap}");
    // Bumps bulge outwards: the outline reaches below the bottom edge.
    let r = rect(cl);
    assert!(r[1] < 100.0 - appearance::cloud_radius(2.0), "{r:?}");

    let pl = &a[l];
    assert_eq!((pl.name(b"Subtype"), text(pl, b"Subj").as_str()), (Some(&b"PolyLine"[..]), "Polygonal Line"));
    let ap = ap_content(&doc, pl);
    assert!(ap.contains("400 300 l") && !ap.contains("h\n"), "open path: {ap}");

    let callout = &a[co];
    assert_eq!(callout.name(b"IT"), Some(&b"FreeTextCallout"[..]));
    let line: Vec<f64> = callout.get(b"CL").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect();
    assert_eq!(line, vec![220.0, 460.0, 260.0, 520.0, 300.0, 520.0], "attached to the box's left side");
    let r = rect(callout);
    assert!(r[0] < 220.0 && r[1] < 460.0 && r[2] >= 420.0 && r[3] >= 540.0, "{r:?}");
    let ap = ap_content(&doc, callout);
    assert!(ap.contains("220 460 m") && ap.contains("(hello) Tj"), "{ap}");

    let caret = &a[k];
    assert_eq!((caret.name(b"Subtype"), text(caret, b"Subj").as_str()), (Some(&b"Caret"[..]), "Inserted Text"));
    assert!(ap_content(&doc, caret).contains(" c h f"));

    // Resizing a callout moves its text box; the leader re-attaches.
    let mut doc = doc;
    set_rect(&mut doc, 0, co, [100.0, 500.0, 200.0, 540.0], &meta("x")).unwrap();
    let callout = &list(&doc, 0)[co];
    let line: Vec<f64> = callout.get(b"CL").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect();
    assert_eq!(&line[4..], &[200.0, 520.0], "now the right side");
    assert!(ap_content(&doc, callout).contains("(hello) Tj"));

    // Moving shifts vertices.
    move_annotation(&mut doc, 0, i, 10.0, 0.0, &meta("m")).unwrap();
    let v: Vec<f64> = list(&doc, 0)[i].get(b"Vertices").unwrap().as_array().unwrap().iter().map(|o| o.as_f64().unwrap()).collect();
    assert_eq!(v[0], 110.0);

    for bad in [
        Shape::Polygon { vertices: vec![[0.0, 0.0], [10.0, 10.0]], cloud: false },
        Shape::PolyLine { vertices: vec![[0.0, 0.0]], start: LineEnding::None, end: LineEnding::None },
        Shape::Caret { rect: [0.0, 0.0, 0.0, 10.0] },
    ] {
        assert!(add_annotation(&mut doc, &new(0, bad), &meta("b")).is_err());
    }
}

#[test]
fn cloud_paths_bulge_outwards_either_way_round() {
    let ccw = [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)];
    let cw: Vec<(f64, f64)> = ccw.iter().rev().copied().collect();
    for pts in [ccw.to_vec(), cw] {
        let p = cloud_ys(&appearance::cloud_path(&pts, 5.0));
        assert!(p.iter().any(|y| *y < -3.0) && p.iter().any(|y| *y > 103.0), "{p:?}");
        assert!(p.iter().all(|y| *y > -10.0 && *y < 110.0));
    }
}

/// Every y coordinate of the curve control points in a cloud path.
fn cloud_ys(path: &str) -> Vec<f64> {
    path.lines()
        .filter(|l| l.ends_with(" c"))
        .flat_map(|l| {
            let v: Vec<f64> = l.split_whitespace().filter_map(|t| t.parse().ok()).collect();
            [v[1], v[3]]
        })
        .collect()
}

#[test]
fn replace_text_groups_a_strikeout_and_a_caret() {
    let mut doc = fixture();
    let quads = [[100.0, 720.0, 200.0, 720.0, 100.0, 706.0, 200.0, 706.0]];
    let red = Style { color: [0.89, 0.13, 0.13], opacity: 1.0, width: 1.0, fill: None };
    let blue = Style { color: [0.0, 0.47, 0.84], opacity: 1.0, width: 1.0, fill: None };
    let i = add_text_replacement(&mut doc, 0, &quads, "new words", "Ada", &red, &blue, &meta("r")).unwrap();
    let doc = reopen(&doc);
    let a = list(&doc, 0);
    let strike = &a[i];
    assert_eq!((strike.name(b"Subtype"), strike.name(b"IT")), (Some(&b"StrikeOut"[..]), Some(&b"StrikeOutTextEdit"[..])));
    let caret = a.iter().find(|d| d.name(b"Subtype") == Some(b"Caret")).unwrap();
    assert_eq!((text(caret, b"Contents").as_str(), caret.name(b"RT")), ("new words", Some(&b"Group"[..])));
    assert!(caret.reference(b"IRT").is_some());
    let r = rect(caret);
    assert!(r[0] < 200.0 && r[2] > 200.0 && r[1] < 706.0, "at the end of the line, below its baseline: {r:?}");
    // Deleting the strikeout takes the caret with it (the group).
    let mut doc = doc;
    delete_annotation(&mut doc, 0, i).unwrap();
    assert!(!list(&doc, 0).iter().any(|d| d.name(b"Subtype") == Some(b"Caret")));
    assert!(add_text_replacement(&mut doc, 0, &[], "x", "Ada", &red, &blue, &meta("r")).is_err());
}

#[test]
fn files_attach_as_comments() {
    let mut doc = fixture();
    let data = b"col1,col2\n1,2\n".to_vec();
    let shape = Shape::Attachment { at: [300.0, 500.0], icon: AttachIcon::Paperclip, file: "data.csv".into(), data: data.clone() };
    let i = add_annotation(
        &mut doc,
        &NewAnnotation { page: 0, shape, style: Style::default(), contents: String::new(), author: "Ada".into() },
        &meta("f"),
    )
    .unwrap();
    let doc = reopen(&doc);
    let a = &list(&doc, 0)[i];
    assert_eq!((a.name(b"Subtype"), a.name(b"Name")), (Some(&b"FileAttachment"[..]), Some(&b"Paperclip"[..])));
    assert_eq!(text(a, b"Contents"), "data.csv", "the file name describes it");
    let fs = doc.resolve(a.get(b"FS").unwrap()).as_dict().cloned().unwrap();
    assert_eq!(text(&fs, b"UF"), "data.csv");
    let ef = fs.get(b"EF").and_then(|e| e.as_dict()).and_then(|e| e.reference(b"F")).unwrap();
    let Object::Stream(s) = &*doc.get(ef) else { panic!() };
    assert_eq!(s.decoded().unwrap(), data);
    assert!(ap_content(&doc, a).contains(" c\n") || ap_content(&doc, a).contains(" c "), "a drawn paperclip");
    let mut doc = doc;
    let bad = Shape::Attachment { at: [0.0, 0.0], icon: AttachIcon::Tag, file: " ".into(), data: Vec::new() };
    assert!(
        add_annotation(
            &mut doc,
            &NewAnnotation { page: 0, shape: bad, style: Style::default(), contents: String::new(), author: String::new() },
            &meta("x")
        )
        .is_err()
    );
}

#[test]
fn the_eraser_cuts_strokes_and_removes_empty_drawings() {
    let mut doc = fixture();
    let line = |y: f64| (0..=10).map(|k| [100.0 + k as f64 * 20.0, y]).collect::<Vec<_>>();
    let shape = Shape::Ink { strokes: vec![line(500.0), line(400.0)] };
    let i = add_annotation(&mut doc, &new(0, shape), &meta("i")).unwrap();
    // A vertical swipe through the middle of both lines.
    assert!(erase_ink(&mut doc, 0, i, &[[200.0, 550.0], [200.0, 350.0]], 5.0, &meta("e")).unwrap());
    let d = &list(&doc, 0)[i];
    let strokes = d.get(b"InkList").unwrap().as_array().unwrap().len();
    assert_eq!(strokes, 4, "each line cut in two");
    assert!(!erase_ink(&mut doc, 0, i, &[[600.0, 100.0]], 5.0, &meta("e")).unwrap(), "nothing under the eraser");
    // Rubbing out everything deletes the drawing.
    let all: Vec<[f64; 2]> = (0..=20).map(|k| [100.0 + k as f64 * 10.0, 500.0]).chain((0..=20).map(|k| [100.0 + k as f64 * 10.0, 400.0])).collect();
    erase_ink(&mut doc, 0, i, &all[..21], 8.0, &meta("e")).unwrap();
    erase_ink(&mut doc, 0, i, &all[21..], 8.0, &meta("e")).unwrap();
    assert!(!list(&doc, 0).iter().any(|d| d.name(b"Subtype") == Some(b"Ink")));
}

/// A one-page document from object bodies (object 1 is the catalog, 3 the page).
fn document_of(objs: &[&[u8]]) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).expect("opens")
}

#[test]
fn comments_without_appearances_get_one_for_display_only() {
    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R 7 0 R 8 0 R 9 0 R 10 0 R] >>",
        b"<< /Type /Annot /Subtype /FreeText /Rect [48 582 285 632] /Contents (FreeText annotation with valid /DA string.) /DA (/Helv 10 Tf 0 0 1 rg) /Border [0 0 1] /C [.85 .47 .02] >>",
        b"<< /Type /Annot /Subtype /Ink /Rect [70 392 275 472] /InkList [[80 402 120 452 160 412 210 462 260 417]] /Border [0 0 2] /C [.49 .18 .07] >>",
        b"<< /Type /Annot /Subtype /Text /Rect [400 717 420 737] /C [1 1 0] /Contents (A note) >>",
        b"<< /Type /Annot /Subtype /Square /Rect [10 10 50 50] /C [1 0 0] /AP << /N 11 0 R >> >>",
        b"<< /Type /Annot /Subtype /Link /Rect [48 717 308 741] /Border [0 0 1] /C [0 0 1] >>",
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (f) /Rect [10 100 100 120] >>",
        b"<< /Type /Annot /Subtype /Sound /Rect [10 200 30 220] >>",
        b"<< /Type /XObject /Subtype /Form /BBox [10 10 50 50] /Length 0 >>\nstream\n\nendstream",
    ]);
    let display = with_missing_appearances(&doc).expect("three comments need an appearance");
    let has_ap = |doc: &Document| list(doc, 0).iter().map(|d| d.get(b"AP").is_some()).collect::<Vec<_>>();
    assert_eq!(has_ap(&doc), [false, false, false, true, false, false, false], "the document itself is untouched");
    assert_eq!(has_ap(&display), [true, true, true, true, false, false, false], "FreeText, Ink and Text get one; the rest don't");
    let shown = list(&display, 0);
    let free_text = ap_content(&display, &shown[0]);
    assert!(free_text.contains("re f") && free_text.contains("Tj"), "background box and text: {free_text}");
    assert_eq!(shown[3].get(b"AP"), list(&doc, 0)[3].get(b"AP"), "an existing appearance is kept");
    // The display copy survives a save and reopen (what the renderer reads).
    assert_eq!(has_ap(&reopen(&display)), [true, true, true, true, false, false, false]);
    // Nothing to add: no copy.
    let complete = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R] >>",
        b"<< /Type /Annot /Subtype /Link /Rect [48 717 308 741] >>",
        b"<< /Type /Annot /Subtype /Square /Rect [10 10 50 50] /C [1 0 0] /AP << /N 6 0 R >> >>",
        b"<< /Type /XObject /Subtype /Form /BBox [10 10 50 50] /Length 0 >>\nstream\n\nendstream",
    ]);
    assert!(with_missing_appearances(&complete).is_none());
}

fn line_dict(coords: [f64; 4], ending: &str) -> pdfcraft_cos::Dict {
    let mut d = pdfcraft_cos::Dict::new();
    d.set(b"Subtype".to_vec(), Object::name("Line"));
    d.set(b"Rect".to_vec(), Object::Array([0.0, 0.0, 300.0, 300.0].into_iter().map(Object::Real).collect()));
    d.set(b"C".to_vec(), Object::Array([1.0, 0.0, 0.0].into_iter().map(Object::Real).collect()));
    d.set(b"L".to_vec(), Object::Array(coords.into_iter().map(Object::Real).collect()));
    d.set(b"LE".to_vec(), Object::Array(vec![Object::name("None"), Object::name(ending)]));
    d
}

#[test]
fn malformed_optional_interior_color_does_not_hide_shape_strokes() {
    let mut square = pdfcraft_cos::Dict::new();
    square.set(b"Subtype".to_vec(), Object::name("Square"));
    square.set(b"Rect".to_vec(), Object::Array([0.0, 0.0, 40.0, 30.0].into_iter().map(Object::Real).collect()));
    square.set(b"C".to_vec(), Object::Array([1.0, 0.0, 0.0].into_iter().map(Object::Real).collect()));
    let malformed = Object::Array(vec![Object::Real(1.0), Object::Real(0.0)]);
    square.set(b"IC".to_vec(), malformed.clone());
    let mut circle = square.clone();
    circle.set(b"Subtype".to_vec(), Object::name("Circle"));

    let mut polygon = pdfcraft_cos::Dict::new();
    polygon.set(b"Subtype".to_vec(), Object::name("Polygon"));
    polygon.set(b"Rect".to_vec(), Object::Array([0.0, 0.0, 40.0, 30.0].into_iter().map(Object::Real).collect()));
    polygon.set(b"C".to_vec(), Object::Array([1.0, 0.0, 0.0].into_iter().map(Object::Real).collect()));
    polygon.set(b"Vertices".to_vec(), Object::Array([0.0, 0.0, 40.0, 0.0, 20.0, 30.0].into_iter().map(Object::Real).collect()));
    polygon.set(b"IC".to_vec(), malformed.clone());

    let mut line = line_dict([0.0, 0.0, 40.0, 30.0], "None");
    line.set(b"IC".to_vec(), malformed.clone());
    let mut polyline = pdfcraft_cos::Dict::new();
    polyline.set(b"Subtype".to_vec(), Object::name("PolyLine"));
    polyline.set(b"Rect".to_vec(), Object::Array([0.0, 0.0, 40.0, 30.0].into_iter().map(Object::Real).collect()));
    polyline.set(b"C".to_vec(), Object::Array([1.0, 0.0, 0.0].into_iter().map(Object::Real).collect()));
    polyline.set(b"Vertices".to_vec(), Object::Array([0.0, 0.0, 40.0, 0.0, 20.0, 30.0].into_iter().map(Object::Real).collect()));
    polyline.set(b"LE".to_vec(), Object::Array(vec![Object::name("None"), Object::name("None")]));
    polyline.set(b"IC".to_vec(), malformed);

    for (name, annotation) in [("Square", square), ("Circle", circle), ("Polygon", polygon), ("Line", line), ("PolyLine", polyline)] {
        assert!(appearance::build(&annotation).is_some(), "malformed optional /IC must not hide a valid {name} stroke");
    }

    let doc = document_of(&[
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 50 40] >>",
        b"<< /Type /Page /Parent 2 0 R /Annots [4 0 R] >>",
        b"<< /Type /Annot /Subtype /Square /Rect [5 5 45 35] /C [1 0 0] /IC [1 0] >>",
    ]);
    let display = with_missing_appearances(&doc).expect("the valid square gets a fallback appearance");
    let rendered = list(&display, 0).remove(0);
    let content = ap_content(&display, &rendered);
    assert!(content.contains("1 0 0 RG") && content.contains(" re\nS\n"), "the stroke remains drawable without a fill: {content}");
    assert!(list(&doc, 0)[0].get(b"AP").is_none(), "display generation leaves the source document unchanged");
}

#[test]
fn every_line_ending_is_drawn() {
    let filled = [LineEnding::Square, LineEnding::Circle, LineEnding::Diamond, LineEnding::ClosedArrow, LineEnding::RClosedArrow];
    let (mut open, mut reverse) = (String::new(), String::new());
    for ending in LineEnding::ALL {
        let mut doc = fixture();
        add_annotation(
            &mut doc,
            &new(0, Shape::Line { from: [80.0, 100.0], to: [200.0, 160.0], start: LineEnding::None, end: ending }),
            &meta("ending"),
        )
        .unwrap();
        let doc = reopen(&doc);
        let line = list(&doc, 0).into_iter().find(|d| d.name(b"Subtype") == Some(b"Line")).expect("line");
        let content = ap_content(&doc, &line);
        assert!(!content.is_empty(), "{ending:?}");
        let paints = content.contains("h B") || content.contains("B\n");
        assert_eq!(paints, filled.contains(&ending), "{ending:?} fill: {content}");
        if ending != LineEnding::None {
            let r = rect(&line);
            assert!(r[0] < 70.0 && r[2] > 210.0, "{ending:?} rectangle includes the ending: {r:?}");
        }
        if ending == LineEnding::OpenArrow {
            open = content.clone();
        }
        if ending == LineEnding::ROpenArrow {
            reverse = content;
        }
    }
    assert_ne!(open, reverse, "a reverse arrow points the other way");

    let mut doc = fixture();
    let index = add_annotation(
        &mut doc,
        &new(0, Shape::Line { from: [40.0, 40.0], to: [140.0, 40.0], start: LineEnding::None, end: LineEnding::None }),
        &meta("plain"),
    )
    .unwrap();
    let before = rect(&list(&doc, 0).into_iter().find(|d| d.name(b"Subtype") == Some(b"Line")).expect("line"));
    set_style(&mut doc, 0, index, None, None, None, Some(&[LineEnding::None, LineEnding::Square]), &meta("style")).unwrap();
    let mut doc = reopen(&doc);
    let line = list(&doc, 0).into_iter().find(|d| d.name(b"Subtype") == Some(b"Line")).expect("line");
    let after = rect(&line);
    assert!(after[1] < before[1] - 5.0 && after[3] > before[3] + 5.0, "the square is not clipped: {before:?} → {after:?}");
    assert!(ap_content(&doc, &line).contains("re h B"), "square ending");

    add_annotation(
        &mut doc,
        &new(0, Shape::PolyLine { vertices: vec![[40.0, 200.0], [90.0, 260.0], [150.0, 200.0]], start: LineEnding::Circle, end: LineEnding::Slash }),
        &meta("poly"),
    )
    .unwrap();
    add_annotation(
        &mut doc,
        &new(
            0,
            Shape::Callout {
                rect: [300.0, 500.0, 420.0, 540.0],
                knee: [260.0, 520.0],
                point: [220.0, 460.0],
                font_size: 10.0,
                ending: LineEnding::Diamond,
            },
        ),
        &meta("call"),
    )
    .unwrap();
    let doc = reopen(&doc);
    let all = list(&doc, 0);
    let poly = all.iter().find(|d| d.name(b"Subtype") == Some(b"PolyLine")).expect("polyline");
    let poly_ap = ap_content(&doc, poly);
    assert!(poly_ap.contains("B\n") && poly_ap.contains(" l S"), "circle fills and the slash only strokes: {poly_ap}");
    let call = all.iter().find(|d| d.name(b"IT") == Some(b"FreeTextCallout")).expect("callout");
    assert_eq!(call.name(b"LE"), Some(b"Diamond".as_slice()));
    assert!(ap_content(&doc, call).contains("h B"), "diamond callout");

    assert!(appearance::build(&line_dict([f64::NAN, 0.0, 10.0, 0.0], "OpenArrow")).is_none(), "a non-finite endpoint is rejected");
    assert!(appearance::build(&line_dict([0.0, 0.0, 10.0, 0.0], "Foo")).is_none(), "an unknown ending is not replaced");
}

#[test]
fn redraw_keeps_shared_annotation_appearances_and_drops_stale_alternates() {
    let alternates: [(&[u8], &[u8]); 2] = [(b"R", b"0 0 20 10 re f\n"), (b"D", b"1 1 18 8 re S\n")];
    for indirect in [false, true] {
        let mut doc = fixture();
        let first = add_annotation(&mut doc, &new(0, Shape::Rectangle { rect: [10.0, 10.0, 110.0, 60.0] }), &meta("first")).unwrap();
        let other = add_annotation(&mut doc, &new(0, Shape::Rectangle { rect: [120.0, 10.0, 220.0, 60.0] }), &meta("other")).unwrap();
        let (_, first_ref) = annot_ref(&mut doc, 0, first).unwrap();
        let (_, other_ref) = annot_ref(&mut doc, 0, other).unwrap();
        let mut entries = annot_dict(&doc, first_ref).get(b"AP").unwrap().as_dict().unwrap().clone();
        for (key, bytes) in alternates {
            let mut d = Dict::new();
            d.set(b"Type".to_vec(), Object::name("XObject"));
            d.set(b"Subtype".to_vec(), Object::name("Form"));
            d.set(b"BBox".to_vec(), num_array(&[0.0, 0.0, 20.0, 10.0]));
            let r = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(d, bytes.to_vec())));
            entries.set(key.to_vec(), Object::Ref(r));
        }
        entries.set(b"VendorState".to_vec(), Object::name("Retained"));
        let shared = if indirect { Object::Ref(doc.add(Object::Dict(entries))) } else { Object::Dict(entries) };
        for r in [first_ref, other_ref] {
            doc.update_dict(r, |d| d.set(b"AP".to_vec(), shared.clone())).unwrap();
        }
        let mut doc = reopen(&doc);
        let other_dict = list(&doc, 0)[other].clone();
        let source = other_dict.get(b"AP").unwrap();
        let before = doc.resolve(source);
        set_style(&mut doc, 0, first, Some([1.0, 0.0, 0.0]), None, None, None, &meta("")).unwrap();
        assert_eq!(doc.resolve(source), before, "a shared AP must not change");
        for full in [false, true] {
            let bytes = if full {
                pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap()
            } else {
                assert!(!doc.revisions().is_empty(), "incremental save needs an existing revision");
                assert!(!doc.encryption_changed() && !doc.full_save_required(), "this edit must not require a full rewrite");
                let original = doc.bytes();
                let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
                assert!(bytes.len() > original.len(), "incremental save appends the changed appearance");
                assert!(bytes.starts_with(original.as_slice()), "incremental save preserves the complete original prefix");
                bytes
            };
            hayro_syntax::Pdf::new(bytes.clone()).unwrap();
            let doc = Document::open(Arc::new(bytes)).unwrap();
            let all = list(&doc, 0);
            let first_ap = doc.resolve(all[first].get(b"AP").unwrap());
            let other_ap = doc.resolve(all[other].get(b"AP").unwrap());
            let first_ap = first_ap.as_dict().unwrap();
            let other_ap = other_ap.as_dict().unwrap();
            assert_ne!(first_ap.get(b"N"), other_ap.get(b"N"));
            // The restyled annotation keeps unknown entries, but not down/rollover looks in its
            // old colour.
            assert_eq!(first_ap.len(), 2);
            assert_eq!(other_ap.len(), 4);
            for (key, bytes) in alternates {
                assert!(first_ap.get(key).is_none(), "stale /{}", String::from_utf8_lossy(key));
                let object = doc.resolve(other_ap.get(key).unwrap());
                let Object::Stream(stream) = &*object else { panic!("alternate appearance") };
                assert_eq!(stream.decoded().unwrap(), bytes);
            }
            assert_eq!(first_ap.name(b"VendorState"), Some(&b"Retained"[..]));
            assert_eq!(other_ap.name(b"VendorState"), Some(&b"Retained"[..]));
            assert!(ap_content(&doc, &all[first]).contains("1 0 0 RG"));
            assert!(ap_content(&doc, &all[other]).contains("0 0.4 1 RG"));
        }
    }
}

#[test]
fn non_markup_annotations_are_not_comments() {
    // #169: a LaTeX `animate` player's Screen annotation showed up in the Comments list.
    for s in ["Link", "Widget", "Popup", "Screen", "Movie", "RichMedia", "3D", "PrinterMark", "TrapNet", "Watermark"] {
        assert!(!is_comment_subtype(s), "{s}");
    }
    for s in ["Text", "FreeText", "Highlight", "Ink", "Stamp", "FileAttachment", "Redact", "Sound"] {
        assert!(is_comment_subtype(s), "{s}");
    }
}

/// A drawing (a drawn Fill & Sign signature is one) resizes by scaling its strokes into the new
/// rectangle, keeping each side's stroke margin, and gets a fresh appearance.
#[test]
fn drawings_resize_by_scaling_their_strokes() {
    let mut doc = fixture();
    let strokes = vec![vec![[100.0, 400.0], [150.0, 460.0], [200.0, 410.0]], vec![[120.0, 420.0], [180.0, 430.0]]];
    let i = add_annotation(&mut doc, &new(0, Shape::Ink { strokes }), &meta("i")).unwrap();
    let nums = |o: &Object| -> Vec<f64> { o.as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect() };
    let points = |doc: &Document| -> Vec<[f64; 2]> {
        let d = &list(doc, 0)[i];
        d.get(b"InkList").unwrap().as_array().unwrap().iter().flat_map(|s| nums(s).as_chunks::<2>().0.to_vec()).collect()
    };
    let before = points(&doc);
    let r0 = nums(list(&doc, 0)[i].get(b"Rect").unwrap());
    let lo = |k: usize| before.iter().map(|p| p[k]).fold(f64::MAX, f64::min);
    let hi = |k: usize| before.iter().map(|p| p[k]).fold(f64::MIN, f64::max);
    let margin = [lo(0) - r0[0], lo(1) - r0[1], r0[2] - hi(0), r0[3] - hi(1)];
    assert!(margin.iter().all(|m| *m > 0.0), "{margin:?}");
    // Twice the size, bottom-left corner fixed.
    let rect = [r0[0], r0[1], r0[0] + 2.0 * (r0[2] - r0[0]), r0[1] + 2.0 * (r0[3] - r0[1])];
    set_rect(&mut doc, 0, i, rect, &meta("r")).unwrap();
    let d = &list(&doc, 0)[i];
    let r1 = nums(d.get(b"Rect").unwrap());
    assert!(r1.iter().zip(rect).all(|(a, b)| (a - b).abs() < 1e-6), "{r1:?} != {rect:?}");
    // Each point keeps its place relative to the drawing's bounds, inside the same margin.
    let inner = |r: &[f64]| [r[0] + margin[0], r[1] + margin[1], r[2] - margin[2], r[3] - margin[3]];
    let (a, b) = (inner(&r0), inner(&r1));
    for (p, q) in before.iter().zip(points(&doc)) {
        let expected = [b[0] + (p[0] - a[0]) / (a[2] - a[0]) * (b[2] - b[0]), b[1] + (p[1] - a[1]) / (a[3] - a[1]) * (b[3] - b[1])];
        assert!((q[0] - expected[0]).abs() < 1e-6 && (q[1] - expected[1]).abs() < 1e-6, "{q:?} != {expected:?}");
    }
    let ap = ap_content(&doc, d);
    let first = points(&doc)[0];
    assert!(ap.contains(&format!("{} {} m", n(first[0]), n(first[1]))), "appearance redrawn at the new size: {ap}");
    // Too small to hold the stroke margin.
    assert!(matches!(set_rect(&mut doc, 0, i, [100.0, 400.0, 101.0, 401.0], &meta("")), Err(AnnotError::Invalid(_))));
}
