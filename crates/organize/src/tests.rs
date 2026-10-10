use std::sync::Arc;

use pdfcraft_cos::{Document, Object, SaveOptions, write_full, write_incremental};

use super::*;

#[test]
fn page_rotation_resolves_inheritance_overrides_and_missing_pages() {
    let mut doc = Document::open(Arc::new(fixture())).unwrap();
    for page in 0..3 {
        assert_eq!(page_rotation(&doc, page).unwrap(), 90);
    }
    rotate_pages(&mut doc, &[1], -180).unwrap();
    assert_eq!(page_rotation(&doc, 1).unwrap(), 270);
    assert_eq!(page_rotation(&doc, 0).unwrap(), 90);
    assert_eq!(page_rotation(&doc, 3), Err(OrganizeError::NoSuchPage(3)));
}

#[test]
fn rotate_rejects_angles_that_are_not_multiples_of_90() {
    // The page tree only turns in quarter steps, so anything else used to be silently truncated
    // by the integer division. It has to be an error, and the document must stay as it was.
    let mut doc = Document::open(Arc::new(fixture())).unwrap();
    assert_eq!(rotate_pages(&mut doc, &[0], 45), Err(OrganizeError::Invalid("rotation must be a multiple of 90 degrees".into())));
    assert_eq!(rotate_pages(&mut doc, &[0], 91), Err(OrganizeError::Invalid("rotation must be a multiple of 90 degrees".into())));
    assert_eq!(page_rotation(&doc, 0).unwrap(), 90);
    rotate_pages(&mut doc, &[0], -90).unwrap();
    assert_eq!(page_rotation(&doc, 0).unwrap(), 0);
}

/// A 3-page document with a nested page tree. MediaBox and Rotate are inherited from the root,
/// Resources from an intermediate node; each page's content says which page it is.
fn fixture() -> Vec<u8> {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),                                                  // 1
        b"<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 3 /MediaBox [0 0 300 400] /Rotate 90 >>".to_vec(), // 2
        b"<< /Type /Pages /Parent 2 0 R /Kids [4 0 R 5 0 R] /Count 2 /Resources << /Font << /F1 10 0 R >> >> >>".to_vec(), // 3
        b"<< /Type /Page /Parent 3 0 R /Contents 7 0 R >>".to_vec(),                                    // 4
        b"<< /Type /Page /Parent 3 0 R /Contents 8 0 R >>".to_vec(),                                    // 5
        b"<< /Type /Page /Parent 2 0 R /Contents 9 0 R /Resources << /Font << /F1 10 0 R >> >> /MediaBox [0 0 500 500] >>".to_vec(), // 6
        stream(b"BT /F1 12 Tf 20 20 Td (Page 1) Tj ET"),                                                // 7
        stream(b"BT /F1 12 Tf 20 20 Td (Page 2) Tj ET"),                                                // 8
        stream(b"BT /F1 12 Tf 20 20 Td (Page 3) Tj ET"),                                                // 9
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),                             // 10
        b"<< /Title (Original) /Producer (fixture) >>".to_vec(),                                        // 11
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
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R /Info 11 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn stream(body: &[u8]) -> Vec<u8> {
    let mut v = format!("<< /Length {} >>\nstream\n", body.len()).into_bytes();
    v.extend_from_slice(body);
    v.extend_from_slice(b"\nendstream");
    v
}

fn open(bytes: Vec<u8>) -> Document {
    Document::open(Arc::new(bytes)).expect("opens")
}

/// The text label of each page, via its content stream, in order.
fn labels(doc: &Document) -> Vec<String> {
    pages(doc)
        .unwrap()
        .iter()
        .map(|p| {
            let page = doc.get(p.obj);
            let c = page.as_dict().and_then(|d| d.get(b"Contents").cloned());
            match c.map(|c| doc.resolve(&c)).as_deref() {
                Some(Object::Stream(s)) => {
                    let data = s.decoded().unwrap();
                    let t = String::from_utf8_lossy(&data);
                    t.split('(').nth(1).and_then(|x| x.split(')').next()).unwrap_or("").to_string()
                }
                _ => "blank".into(),
            }
        })
        .collect()
}

/// Save incrementally, verify the prefix is untouched, and reopen — both with our reader and with
/// an independent parser (hayro-syntax) that must agree on the page count.
fn save_and_reopen(doc: &Document) -> Document {
    let original = doc.bytes().clone();
    let bytes = write_incremental(doc, &SaveOptions::default()).expect("saves");
    assert_eq!(&bytes[..original.len()], &original[..], "incremental save must not modify existing bytes");
    let reopened = open(bytes.clone());
    let independent = hayro_syntax::Pdf::new(bytes).expect("hayro opens our output");
    assert_eq!(independent.pages().len(), page_count(&reopened).unwrap(), "readers agree on page count");
    reopened
}

#[test]
fn walks_nested_tree_in_order() {
    let doc = open(fixture());
    assert_eq!(labels(&doc), ["Page 1", "Page 2", "Page 3"]);
}

#[test]
fn rotate_uses_inherited_rotation_and_round_trips() {
    let mut doc = open(fixture());
    rotate_pages(&mut doc, &[0, 2], 90).unwrap();
    rotate_pages(&mut doc, &[2], -270).unwrap(); // page 3: inherited 90 + 90 - 270 = -90 ≡ 270
    let doc = save_and_reopen(&doc);
    let rot = |i: usize| {
        let p = pages(&doc).unwrap()[i].obj;
        doc.get(p).as_dict().and_then(|d| d.int(b"Rotate"))
    };
    assert_eq!(rot(0), Some(180), "inherited 90 + 90");
    assert_eq!(rot(1), None, "untouched page keeps inheriting");
    assert_eq!(rot(2), Some(270), "normalised into 0..360");
}

#[test]
fn delete_keeps_inherited_attributes_on_survivors() {
    let mut doc = open(fixture());
    delete_pages(&mut doc, &[0]).unwrap();
    let doc = save_and_reopen(&doc);
    assert_eq!(labels(&doc), ["Page 2", "Page 3"]);
    // Page 2 inherited Resources (from the intermediate node) and MediaBox/Rotate (from the root);
    // the tree was flattened, so they must now be on the page itself.
    let p = doc.get(pages(&doc).unwrap()[0].obj);
    let d = p.as_dict().unwrap();
    assert!(d.contains(b"Resources") && d.contains(b"MediaBox"));
    assert_eq!(d.int(b"Rotate"), Some(90));
    let root = doc.get(doc.get(doc.root().unwrap()).as_dict().unwrap().reference(b"Pages").unwrap());
    assert_eq!(root.as_dict().unwrap().int(b"Count"), Some(2));
}

#[test]
fn deleted_pages_are_not_kept_by_what_points_at_them() {
    // A1 links to A3, a bookmark goes to A3, and A2 holds the form's only field. After deleting
    // A2 and A3 neither page, its content nor the field may stay in the saved file.
    let mut doc = doc_a();
    add_bookmark(&mut doc, &[], 0, "To A3", 2).unwrap();
    delete_pages(&mut doc, &[1, 2]).unwrap();
    let out = full_roundtrip(&doc);
    assert_eq!(labels(&out), ["A1"]);
    // The bookmark and A1's link now go nowhere instead of to a missing page.
    let mark = out.get(bookmarks(&out)[0].obj).as_dict().cloned().unwrap();
    assert!(!mark.contains(b"Dest") && !mark.contains(b"A"), "{mark:?}");
    let link = &annots(&out, 0)[0];
    assert!(!link.contains(b"A") && !link.contains(b"Dest"), "{link:?}");
    let mut page_objects = 0;
    for n in out.object_numbers() {
        let o = out.get(ObjRef::new(n, out.generation(n)));
        if let Object::Stream(s) = &*o {
            let data = String::from_utf8_lossy(&s.decoded().unwrap()).into_owned();
            assert!(!data.contains("(A2)") && !data.contains("(A3)"), "object {n} still holds a deleted page's content");
        }
        page_objects += usize::from(o.as_dict().is_some_and(|d| d.name(b"Type") == Some(b"Page")));
    }
    assert_eq!(page_objects, 1);
    let cat = out.get(out.root().unwrap()).as_dict().cloned().unwrap();
    let form = out.resolve(cat.get(b"AcroForm").unwrap());
    assert_eq!(form.as_dict().and_then(|f| f.get(b"Fields")).and_then(Object::as_array).map(Vec::len), Some(0), "the field went with its page");
}

#[test]
fn deleting_the_opening_page_drops_the_open_action() {
    let open_action = |doc: &Document| doc.get(doc.root().unwrap()).as_dict().unwrap().get(b"OpenAction").cloned();
    let set_open = |doc: &mut Document, value: Object| {
        let root = doc.root().unwrap();
        doc.update_dict(root, |c| c.set(b"OpenAction".to_vec(), value)).unwrap();
    };
    let page = |doc: &Document, i: usize| pages(doc).unwrap()[i].obj;
    for as_action in [false, true] {
        let target = |doc: &Document, i: usize| {
            let dest = Object::Array(vec![Object::Ref(page(doc, i)), Object::Name(b"Fit".to_vec())]);
            if as_action {
                let mut a = Dict::new();
                a.set(b"S".to_vec(), Object::Name(b"GoTo".to_vec()));
                a.set(b"D".to_vec(), dest);
                Object::Dict(a)
            } else {
                dest
            }
        };
        // The opening page goes: no OpenAction is left to point at nothing.
        let mut doc = doc_a();
        let open = target(&doc, 0);
        set_open(&mut doc, open);
        delete_pages(&mut doc, &[0]).unwrap();
        let out = full_roundtrip(&doc);
        assert_eq!(labels(&out), ["A2", "A3"]);
        assert_eq!(open_action(&out), None);
        // Another page goes: the opening page keeps its OpenAction.
        let mut doc = doc_a();
        let open = target(&doc, 0);
        set_open(&mut doc, open);
        delete_pages(&mut doc, &[2]).unwrap();
        let out = full_roundtrip(&doc);
        assert_eq!(labels(&out), ["A1", "A2"]);
        assert!(open_action(&out).is_some());
    }
}

#[test]
fn cannot_delete_every_page_or_missing_pages() {
    let mut doc = open(fixture());
    assert_eq!(delete_pages(&mut doc, &[0, 1, 2]), Err(OrganizeError::WouldRemoveAllPages));
    assert_eq!(delete_pages(&mut doc, &[7]), Err(OrganizeError::NoSuchPage(7)));
    assert_eq!(rotate_pages(&mut doc, &[3], 90), Err(OrganizeError::NoSuchPage(3)));
    assert!(!doc.is_modified(), "failed operations leave the document untouched");
}

#[test]
fn move_pages_reorders() {
    let mut doc = open(fixture());
    move_pages(&mut doc, &[2], 0).unwrap();
    assert_eq!(labels(&doc), ["Page 3", "Page 1", "Page 2"]);
    move_pages(&mut doc, &[0, 1], 3).unwrap();
    assert_eq!(labels(&doc), ["Page 2", "Page 3", "Page 1"]);
    let doc = save_and_reopen(&doc);
    assert_eq!(labels(&doc), ["Page 2", "Page 3", "Page 1"]);
}

#[test]
fn inserts_blank_page() {
    let mut doc = open(fixture());
    insert_blank_page(&mut doc, 1, 612.0, 792.0).unwrap();
    let doc = save_and_reopen(&doc);
    assert_eq!(labels(&doc), ["Page 1", "blank", "Page 2", "Page 3"]);
    let hay = hayro_syntax::Pdf::new(doc.bytes().clone()).unwrap();
    let (w, h) = hay.pages().get(1).unwrap().render_dimensions();
    assert_eq!((w, h), (612.0, 792.0));
}

#[test]
fn document_info_edits_round_trip_with_unicode() {
    let mut doc = open(fixture());
    set_info(&mut doc, "Title", "Résumé — 履歴書").unwrap();
    set_info(&mut doc, "Author", "PdfCraft").unwrap();
    set_info(&mut doc, "Producer", "").unwrap(); // clearing removes the key
    let doc = save_and_reopen(&doc);
    assert_eq!(info(&doc, "Title").as_deref(), Some("Résumé — 履歴書"));
    assert_eq!(info(&doc, "Author").as_deref(), Some("PdfCraft"));
    assert_eq!(info(&doc, "Producer"), None);
}

#[test]
fn successive_incremental_saves_stack_revisions() {
    let mut doc = open(fixture());
    rotate_pages(&mut doc, &[0], 90).unwrap();
    let doc = save_and_reopen(&doc);
    assert_eq!(doc.revisions().len(), 2);
    let mut doc = doc;
    delete_pages(&mut doc, &[1]).unwrap();
    let doc = save_and_reopen(&doc);
    assert_eq!(doc.revisions().len(), 3);
    assert_eq!(labels(&doc), ["Page 1", "Page 3"]);
}

#[test]
fn full_save_drops_unreachable_objects() {
    let mut doc = open(fixture());
    delete_pages(&mut doc, &[0, 1]).unwrap();
    let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    let full = open(bytes.clone());
    assert_eq!(labels(&full), ["Page 3"]);
    // Deleted pages, their content streams and the orphaned intermediate node are gone.
    assert!(full.object_numbers().len() < doc.object_numbers().len());
    assert_eq!(info(&full, "Title").as_deref(), Some("Original"));
    assert_eq!(hayro_syntax::Pdf::new(bytes).unwrap().pages().len(), 1);
}

#[test]
fn deleting_a_page_drops_the_named_destinations_to_it() {
    // Four names (two in the /Names tree, two in the legacy /Dests dictionary) go to P1 or P3;
    // P2 has a link for each. Deleting P1 must drop its names and their links, keep P3's.
    let b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Names << /Dests 6 0 R >> /Dests 7 0 R >>".into(), // 1
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >>".into(), // 2
        "<< /Type /Page /Parent 2 0 R >>".into(),                                          // 3
        "<< /Type /Page /Parent 2 0 R /Annots [8 0 R 9 0 R 10 0 R 11 0 R] >>".into(),      // 4
        "<< /Type /Page /Parent 2 0 R >>".into(),                                          // 5
        "<< /Names [(treeFirst) [3 0 R /Fit] (treeLast) [5 0 R /Fit]] >>".into(),          // 6
        "<< /legacyFirst [3 0 R /Fit] /legacyLast << /D [5 0 R /Fit] >> >>".into(),        // 7
        "<< /Type /Annot /Subtype /Link /Rect [0 0 9 9] /Dest (treeFirst) >>".into(),      // 8
        "<< /Type /Annot /Subtype /Link /Rect [0 0 9 9] /Dest (treeLast) >>".into(),       // 9
        "<< /Type /Annot /Subtype /Link /Rect [0 0 9 9] /Dest /legacyFirst >>".into(),     // 10
        "<< /Type /Annot /Subtype /Link /Rect [0 0 9 9] /A << /S /GoTo /D /legacyLast >> >>".into(), // 11
    ];
    let mut doc = open(build(&b, "/Root 1 0 R"));
    delete_pages(&mut doc, &[0]).unwrap();
    let out = full_roundtrip(&doc);
    let cat = out.get(out.root().unwrap()).as_dict().cloned().unwrap();
    let tree = out.resolve(out.resolve(cat.get(b"Names").unwrap()).as_dict().unwrap().get(b"Dests").unwrap());
    let pairs = tree.as_dict().unwrap().get(b"Names").and_then(Object::as_array).cloned().unwrap();
    assert_eq!(pairs.len(), 2, "{pairs:?}");
    assert_eq!(pairs[0].as_string().map(|s| s.bytes.clone()), Some(b"treeLast".to_vec()));
    let page3 = pages(&out).unwrap()[1].obj;
    assert_eq!(out.resolve(&pairs[1]).as_array().and_then(|a| a.first()).and_then(Object::as_ref), Some(page3));
    let legacy = out.resolve(cat.get(b"Dests").unwrap()).as_dict().cloned().unwrap();
    assert!(!legacy.contains(b"legacyFirst") && legacy.contains(b"legacyLast"), "{legacy:?}");
    let links = annots(&out, 0);
    assert!(!links[0].contains(b"Dest"), "{:?}", links[0]);
    assert!(links[1].contains(b"Dest"), "{:?}", links[1]);
    assert!(!links[2].contains(b"Dest"), "{:?}", links[2]);
    assert!(links[3].contains(b"A"), "{:?}", links[3]);
}

// ── Combine / extract / split ──────────────────────────────────────────────────────────────────

/// Build a classic-xref PDF from object bodies (object i+1 = bodies[i]).
fn build(bodies: &[String], trailer: &str) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, b) in bodies.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{b}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} {trailer} >>\nstartxref\n{xref}\n%%EOF\n", bodies.len() + 1).as_bytes());
    out
}

fn body(text: &str) -> String {
    let s = format!("BT /F1 12 Tf 20 20 Td ({text}) Tj ET");
    format!("<< /Length {} >>\nstream\n{s}\nendstream", s.len())
}

/// Document A: pages A1–A3 sharing one font. A1 links to A3 (GoTo action), A2 links to A1
/// (/Dest) and carries a text field widget; A3 has a note with a popup (a reference cycle).
fn doc_a() -> Document {
    let b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [12 0 R] >> >>".into(), // 1
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >>".into(), // 2
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F1 9 0 R >> >> /Annots [10 0 R] >>".into(), // 3
        "<< /Type /Page /Parent 2 0 R /Contents 7 0 R /Resources << /Font << /F1 9 0 R >> >> /Annots [11 0 R 12 0 R] >>".into(), // 4
        "<< /Type /Page /Parent 2 0 R /Contents 8 0 R /Resources << /Font << /F1 9 0 R >> >> /Annots [13 0 R] /StructParents 4 >>".into(), // 5
        body("A1"),                                                                  // 6
        body("A2"),                                                                  // 7
        body("A3"),                                                                  // 8
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),             // 9
        "<< /Type /Annot /Subtype /Link /Rect [0 0 50 50] /P 3 0 R /A << /S /GoTo /D [5 0 R /Fit] >> >>".into(), // 10
        "<< /Type /Annot /Subtype /Link /Rect [0 0 50 50] /P 4 0 R /Dest [3 0 R /XYZ 0 400 0] >>".into(), // 11
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Ada) /Rect [60 60 200 80] /P 4 0 R >>".into(), // 12
        "<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /Contents (note) /Popup 14 0 R /P 5 0 R /StructParent 7 >>".into(), // 13
        "<< /Type /Annot /Subtype /Popup /Rect [40 40 200 120] /Parent 13 0 R >>".into(), // 14
        "<< /Title (Doc A) /Author (Alice) >>".into(),                               // 15
    ];
    open(build(&b, "/Root 1 0 R /Info 15 0 R"))
}

/// Document B: pages B1, B2 in a nested tree with MediaBox and Rotate inherited from the root.
fn doc_b() -> Document {
    let b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 2 /MediaBox [0 0 500 250] /Rotate 90 >>".into(),
        "<< /Type /Pages /Parent 2 0 R /Kids [4 0 R 5 0 R] /Count 2 >>".into(),
        "<< /Type /Page /Parent 3 0 R /Contents 6 0 R /Resources << /Font << /F1 8 0 R >> >> >>".into(),
        "<< /Type /Page /Parent 3 0 R /Contents 7 0 R /Resources << /Font << /F1 8 0 R >> >> >>".into(),
        body("B1"),
        body("B2"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >>".into(),
    ];
    open(build(&b, "/Root 1 0 R"))
}

/// Write in full, reopen with our reader, and require hayro to agree on the page count.
fn full_roundtrip(doc: &Document) -> Document {
    let bytes = write_full(doc, &SaveOptions::default()).expect("writes");
    let reopened = open(bytes.clone());
    let hay = hayro_syntax::Pdf::new(bytes).expect("hayro opens");
    assert_eq!(hay.pages().len(), page_count(&reopened).unwrap());
    reopened
}

fn page_dict(doc: &Document, i: usize) -> Dict {
    doc.get(pages(doc).unwrap()[i].obj).as_dict().cloned().unwrap()
}

fn annots(doc: &Document, i: usize) -> Vec<Dict> {
    let p = page_dict(doc, i);
    let list = p.get(b"Annots").map(|a| doc.resolve(a).as_array().cloned().unwrap_or_default()).unwrap_or_default();
    list.iter().map(|a| doc.resolve(a).as_dict().cloned().unwrap()).collect()
}

fn count_fonts(doc: &Document) -> usize {
    doc.object_numbers().iter().filter(|n| doc.get(ObjRef::new(**n, 0)).as_dict().is_some_and(|d| d.name(b"Type") == Some(b"Font"))).count()
}

#[test]
fn combine_concatenates_with_a_bookmark_per_file() {
    let out = full_roundtrip(&combine(&[("Report A", &doc_a()), ("Report B", &doc_b())]).unwrap());
    assert_eq!(labels(&out), ["A1", "A2", "A3", "B1", "B2"]);
    // Shared resources are copied once per source: one Helvetica, one Times.
    assert_eq!(count_fonts(&out), 2);
    // Bookmarks: one per source file, pointing at its first page.
    let cat = out.get(out.root().unwrap()).as_dict().cloned().unwrap();
    let outlines = out.get(cat.reference(b"Outlines").unwrap()).as_dict().cloned().unwrap();
    let first = out.get(outlines.reference(b"First").unwrap()).as_dict().cloned().unwrap();
    assert_eq!(first.get(b"Title").and_then(|t| t.as_string()).map(|s| s.to_text()).as_deref(), Some("Report A"));
    let second = out.get(first.reference(b"Next").unwrap()).as_dict().cloned().unwrap();
    assert_eq!(second.get(b"Title").and_then(|t| t.as_string()).map(|s| s.to_text()).as_deref(), Some("Report B"));
    let dest_page = second.get(b"Dest").and_then(|d| d.as_array()).and_then(|a| a[0].as_ref()).unwrap();
    assert_eq!(dest_page, pages(&out).unwrap()[3].obj);
}

#[test]
fn inherited_attributes_travel_with_copied_pages() {
    let out = full_roundtrip(&extract_pages(&doc_b(), &[1]).unwrap());
    let p = page_dict(&out, 0);
    assert_eq!(p.int(b"Rotate"), Some(90));
    assert!(p.contains(b"MediaBox") && p.contains(b"Resources"));
    let hay = hayro_syntax::Pdf::new(write_full(&out, &SaveOptions::default()).unwrap()).unwrap();
    assert_eq!(hay.pages().first().unwrap().render_dimensions(), (250.0, 500.0), "rotated 90°");
}

#[test]
fn links_are_rewired_to_copied_pages_or_dropped() {
    let a = doc_a();
    // Extract A3, A1 (reordered): A1's GoTo A3 must now point at the copy of A3 (index 0).
    let out = full_roundtrip(&extract_pages(&a, &[2, 0]).unwrap());
    assert_eq!(labels(&out), ["A3", "A1"]);
    let link = &annots(&out, 1)[0];
    let action = out.resolve(link.get(b"A").unwrap());
    let target = action.as_dict().unwrap().get(b"D").and_then(|d| d.as_array()).and_then(|a| a[0].as_ref()).unwrap();
    assert_eq!(target, pages(&out).unwrap()[0].obj);
    assert_eq!(link.reference(b"P"), Some(pages(&out).unwrap()[1].obj), "/P points at the new page");
    // Extract only A2: its link to A1 would dangle, so the destination is dropped.
    let out = full_roundtrip(&extract_pages(&a, &[1]).unwrap());
    let link = annots(&out, 0).into_iter().find(|d| d.name(b"Subtype") == Some(b"Link")).unwrap();
    assert!(!link.contains(b"Dest") && !link.contains(b"A"));
}

/// A widget annotation that is its own form field (no `/Parent`).
fn own_field(doc: &Document, r: ObjRef) -> bool {
    doc.get(r).as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Widget") && !d.contains(b"Parent"))
}

#[test]
fn repeated_page_gets_independent_annotations() {
    // A1 has a link, A2 a link and a form field, A3 a note with a popup; each is listed twice.
    let out = full_roundtrip(&extract_pages(&doc_a(), &[0, 0, 1, 1, 2, 2]).unwrap());
    let ps = pages(&out).unwrap();
    let refs: Vec<Vec<ObjRef>> = (0..6)
        .map(|i| page_dict(&out, i).get(b"Annots").map(|a| out.resolve(a).as_array().cloned().unwrap_or_default()).unwrap_or_default())
        .map(|list| list.iter().filter_map(|a| a.as_ref()).collect())
        .collect();
    for i in (0..6).step_by(2) {
        assert!(!refs[i].is_empty());
        assert_eq!(refs[i].len(), refs[i + 1].len());
        for (a, b) in refs[i].iter().zip(&refs[i + 1]) {
            // A widget that is its own field stays one object on both pages, so the field stays
            // one (#790); every other annotation is copied.
            if !own_field(&out, *a) {
                assert_ne!(a, b, "each copy of the page has its own annotation objects");
            }
        }
    }
    for (i, list) in refs.iter().enumerate() {
        for a in list.iter().filter(|a| !own_field(&out, **a)) {
            assert_eq!(out.get(*a).as_dict().unwrap().reference(b"P"), Some(ps[i].obj), "/P names the page holding the copy");
        }
    }
    // Each note keeps its own popup, pointing back at that note.
    let popups: Vec<ObjRef> = [4, 5].iter().map(|&i| out.get(refs[i][0]).as_dict().unwrap().reference(b"Popup").unwrap()).collect();
    assert_ne!(popups[0], popups[1]);
    for (k, p) in popups.iter().enumerate() {
        assert_eq!(out.get(*p).as_dict().unwrap().reference(b"Parent"), Some(refs[4 + k][0]));
    }
}

#[test]
fn form_fields_come_along_and_are_registered() {
    let out = full_roundtrip(&extract_pages(&doc_a(), &[1]).unwrap());
    let cat = out.get(out.root().unwrap()).as_dict().cloned().unwrap();
    let form = out.resolve(cat.get(b"AcroForm").unwrap());
    let fields = out.resolve(form.as_dict().unwrap().get(b"Fields").unwrap()).as_array().cloned().unwrap();
    assert_eq!(fields.len(), 1);
    let field = out.resolve(&fields[0]).as_dict().cloned().unwrap();
    assert_eq!(field.get(b"V").and_then(|v| v.as_string()).map(|s| s.to_text()).as_deref(), Some("Ada"));
    assert_eq!(field.reference(b"P"), Some(pages(&out).unwrap()[0].obj));
}

#[test]
fn popup_cycles_copy_once_and_structure_links_are_removed() {
    let out = full_roundtrip(&extract_pages(&doc_a(), &[2]).unwrap());
    let p = page_dict(&out, 0);
    assert!(!p.contains(b"StructParents"));
    let note = &annots(&out, 0)[0];
    assert!(!note.contains(b"StructParent"));
    let popup = out.get(note.reference(b"Popup").unwrap()).as_dict().cloned().unwrap();
    assert_eq!(
        out.get(popup.reference(b"Parent").unwrap()).as_dict().unwrap().get(b"Contents").and_then(|c| c.as_string()).map(|s| s.to_text()).as_deref(),
        Some("note")
    );
    // Nothing unrelated was dragged in: no pages A1/A2, no other content streams.
    assert_eq!(labels(&out), ["A3"]);
}

#[test]
fn inserting_pages_from_another_file_is_an_incremental_edit() {
    let mut a = doc_a();
    import_pages(&mut a, &doc_b(), &[1, 0], 1).unwrap();
    let a = save_and_reopen(&a); // checks the prefix invariant and hayro agreement
    assert_eq!(labels(&a), ["A1", "B2", "B1", "A2", "A3"]);
    assert_eq!(info(&a, "Title").as_deref(), Some("Doc A"));
    assert_eq!(import_pages(&mut a.clone(), &doc_b(), &[5], 0), Err(OrganizeError::NoSuchPage(5)));
}

#[test]
fn split_ranges_cover_every_page_once() {
    assert_eq!(split_ranges(5, &SplitBy::PageCount(2)), [0..2, 2..4, 4..5]);
    assert_eq!(split_ranges(5, &SplitBy::PageCount(0)), [0..1, 1..2, 2..3, 3..4, 4..5]);
    assert_eq!(split_ranges(5, &SplitBy::Before(vec![3, 1, 1, 9, 0])), [0..1, 1..3, 3..5]);
    assert_eq!(split_ranges(3, &SplitBy::PageCount(10)), vec![0..3]);
}

#[test]
fn split_produces_standalone_documents_with_metadata() {
    let parts = split(&doc_a(), &SplitBy::PageCount(1)).unwrap();
    assert_eq!(parts.len(), 3);
    for (i, part) in parts.iter().enumerate() {
        let part = full_roundtrip(part);
        assert_eq!(labels(&part), [format!("A{}", i + 1)]);
        assert_eq!(info(&part, "Author").as_deref(), Some("Alice"));
    }
}

// ── PDF/X (#263) ───────────────────────────────────────────────────────────────────────────────

/// PDF/X-4 identification in XMP, as an attribute, next to a PDF/UA claim.
const X4_XMP: &str = "<?xpacket begin=\"\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?><x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                      xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" \
                      xmlns:pdfxid=\"http://www.npes.org/pdfx/ns/id/\" xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\" \
                      pdfxid:GTS_PDFXVersion=\"PDF/X-4\" pdfuaid:part=\"1\"/></rdf:RDF></x:xmpmeta><?xpacket end=\"r\"?>";

/// A two-page print file whose output intent prints to `condition` with `profile` as its ICC
/// profile, with `info` as its document information and `xmp` as its XMP metadata.
fn doc_print(condition: &str, profile: &str, info: &str, xmp: Option<&str>) -> Document {
    let metadata = if xmp.is_some() { " /Metadata 10 0 R" } else { "" };
    let mut b: Vec<String> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /OutputIntents [7 0 R]{metadata} >>"), // 1
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 252 144] >>".into(), // 2
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /TrimBox [9 9 243 135] >>".into(), // 3
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /TrimBox [9 9 243 135] >>".into(), // 4
        body("X1"),                                                                    // 5
        body("X2"),                                                                    // 6
        format!("<< /Type /OutputIntent /S /GTS_PDFX /OutputConditionIdentifier ({condition}) /DestOutputProfile 8 0 R >>"), // 7
        format!("<< /N 4 /Length {} >>\nstream\n{profile}\nendstream", profile.len()), // 8
        format!("<< {info} >>"),                                                       // 9
    ];
    if let Some(x) = xmp {
        b.push(format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{x}\nendstream", x.len())); // 10
    }
    open(build(&b, "/Root 1 0 R /Info 9 0 R"))
}

/// A PDF/X-4 business card printing to `condition` with `profile`.
fn doc_x4(condition: &str, profile: &str) -> Document {
    doc_print(condition, profile, "/Title (Card) /GTS_PDFXVersion (PDF/X-4) /Trapped /False", Some(X4_XMP))
}

/// What a document says about PDF/X.
#[derive(Debug, Default, PartialEq)]
struct Pdfx {
    /// Each output intent's printing condition and decoded profile.
    intents: Vec<(String, Vec<u8>)>,
    version: Option<String>,
    trapped: Option<Vec<u8>>,
    xmp: Option<String>,
}

fn pdfx_of(doc: &Document) -> Pdfx {
    let cat = catalog(doc);
    let list = cat.get(b"OutputIntents").map(|o| doc.resolve(o).as_array().cloned().unwrap_or_default()).unwrap_or_default();
    let intents = list
        .iter()
        .filter_map(|oi| {
            let d = doc.resolve(oi).as_dict().cloned()?;
            let id = d.get(b"OutputConditionIdentifier").and_then(|v| doc.resolve(v).as_string().map(|s| s.to_text())).unwrap_or_default();
            let profile = match d.get(b"DestOutputProfile").map(|p| doc.resolve(p)).as_deref() {
                Some(Object::Stream(s)) => s.decoded().unwrap(),
                _ => Vec::new(),
            };
            Some((id, profile))
        })
        .collect();
    let trapped = doc.trailer().get(b"Info").map(|i| doc.resolve(i)).and_then(|i| i.as_dict().and_then(|d| d.name(b"Trapped")).map(<[u8]>::to_vec));
    let xmp = match cat.get(b"Metadata").map(|m| doc.resolve(m)).as_deref() {
        Some(Object::Stream(s)) => Some(String::from_utf8(s.decoded().unwrap()).unwrap()),
        _ => None,
    };
    Pdfx { intents, version: info(doc, "GTS_PDFXVersion"), trapped, xmp }
}

#[test]
fn extract_and_split_keep_the_pdfx_output_intent_and_identification() {
    // #263: the parts had no /OutputIntents, no GTS_PDFXVersion and no XMP pdfxid, so a PDF/X
    // file stopped being one.
    let src = doc_x4("FOGRA39", "icc-fogra39");
    let mut outs = vec![extract_pages(&src, &[1]).unwrap()];
    outs.extend(split(&src, &SplitBy::PageCount(1)).unwrap());
    for out in &outs {
        let x = pdfx_of(&full_roundtrip(out));
        assert_eq!(x.intents, [("FOGRA39".to_string(), b"icc-fogra39".to_vec())]);
        assert_eq!(x.version.as_deref(), Some("PDF/X-4"));
        assert_eq!(x.trapped.as_deref(), Some(&b"False"[..]));
        let xmp = x.xmp.expect("XMP metadata");
        assert!(xmp.contains("<pdfxid:GTS_PDFXVersion>PDF/X-4</pdfxid:GTS_PDFXVersion>"), "{xmp}");
        assert!(xmp.contains(">Card</rdf:li>") && xmp.contains("<pdf:Trapped>False</pdf:Trapped>"), "{xmp}");
        // The structure tree is not copied, so the source's PDF/UA claim must not be either.
        assert!(!xmp.contains("pdfuaid"), "{xmp}");
    }
}

#[test]
fn combine_keeps_pdfx_when_every_source_agrees() {
    // #263: a PDF/X-4 card combined with itself.
    let (a, b) = (doc_x4("FOGRA39", "icc-fogra39"), doc_x4("FOGRA39", "icc-fogra39"));
    let out = full_roundtrip(&combine(&[("a", &a), ("b", &b)]).unwrap());
    assert_eq!(labels(&out), ["X1", "X2", "X1", "X2"]);
    let x = pdfx_of(&out);
    assert_eq!(x.intents, [("FOGRA39".to_string(), b"icc-fogra39".to_vec())], "one output intent, not one per source");
    assert_eq!(x.version.as_deref(), Some("PDF/X-4"));
    assert!(x.xmp.is_some_and(|x| x.contains("<pdfxid:GTS_PDFXVersion>PDF/X-4</pdfxid:GTS_PDFXVersion>")));
}

#[test]
fn combine_claims_no_pdfx_when_the_sources_disagree() {
    // Another printing condition, the same name with another profile, or a source that is not
    // PDF/X: no one standard describes the result, so it claims none.
    let x4 = doc_x4("FOGRA39", "icc-fogra39");
    for other in [doc_x4("GRACoL2013", "icc-gracol"), doc_x4("FOGRA39", "icc-other"), doc_b()] {
        let out = full_roundtrip(&combine(&[("a", &x4), ("b", &other)]).unwrap());
        assert_eq!(pdfx_of(&out), Pdfx::default());
    }
}

#[test]
fn documents_without_a_print_standard_gain_none() {
    assert_eq!(pdfx_of(&full_roundtrip(&extract_pages(&doc_a(), &[0]).unwrap())), Pdfx::default());
    assert_eq!(pdfx_of(&full_roundtrip(&combine(&[("a", &doc_a()), ("b", &doc_b())]).unwrap())), Pdfx::default());
}

#[test]
fn the_pdfx_version_comes_from_xmp_or_the_document_information() {
    // A PDF/X-4 file may name its version only in XMP (here as an element); a PDF/X-1a:2001 file
    // (PDF 1.3) only in its document information, with no XMP, and gets no XMP added.
    let xmp = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description \
               rdf:about=\"\" xmlns:pdfxid=\"http://www.npes.org/pdfx/ns/id/\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\">\
               <pdfxid:GTS_PDFXVersion> PDF/X-4 </pdfxid:GTS_PDFXVersion><pdf:Trapped>True</pdf:Trapped></rdf:Description></rdf:RDF></x:xmpmeta>";
    let x = pdfx_of(&full_roundtrip(&extract_pages(&doc_print("FOGRA51", "icc", "/Title (Flyer)", Some(xmp)), &[0]).unwrap()));
    assert_eq!((x.version.as_deref(), x.trapped.as_deref()), (Some("PDF/X-4"), Some(&b"True"[..])));
    assert!(x.xmp.is_some_and(|x| x.contains("<pdfxid:GTS_PDFXVersion>PDF/X-4</pdfxid:GTS_PDFXVersion>")));

    let x1a = doc_print("CGATS TR 001", "icc", "/GTS_PDFXVersion (PDF/X-1:2001) /GTS_PDFXConformance (PDF/X-1a:2001) /Trapped (False)", None);
    let out = full_roundtrip(&extract_pages(&x1a, &[0]).unwrap());
    let x = pdfx_of(&out);
    assert_eq!((x.version.as_deref(), x.trapped.as_deref()), (Some("PDF/X-1:2001"), Some(&b"False"[..])));
    assert_eq!(info(&out, "GTS_PDFXConformance").as_deref(), Some("PDF/X-1a:2001"));
    assert_eq!(x.xmp, None);
}

#[test]
fn malformed_output_intents_and_identification_do_not_fail() {
    // Untrusted input: intents that are not dictionaries, a missing profile, an intent that is
    // its own profile, an XMP packet that never closes, and a version full of markup and NULs.
    let b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /OutputIntents [5 0 R 42 (junk) 6 0 R] /Metadata 7 0 R >>".into(), // 1
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>".into(),                        // 2
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".into(),                                          // 3
        body("M1"),                                                                                        // 4
        "<< /Type /OutputIntent /S /GTS_PDFX /DestOutputProfile 99 0 R >>".into(),                         // 5
        "<< /Type /OutputIntent /S /GTS_PDFX /OutputConditionIdentifier (x) /DestOutputProfile 6 0 R >>".into(), // 6
        "<< /Length 30 >>\nstream\n<pdfxid:GTS_PDFXConformance>X\nendstream".into(),                       // 7
        "<< /GTS_PDFXVersion (<PDF/X-4 & \"more\">\\000) /Trapped /Maybe >>".into(),                       // 8
    ];
    let src = open(build(&b, "/Root 1 0 R /Info 8 0 R"));
    let out = full_roundtrip(&extract_pages(&src, &[0]).unwrap());
    assert_eq!(labels(&out), ["M1"]);
    let list = catalog(&out).get(b"OutputIntents").map(|o| out.resolve(o).as_array().cloned().unwrap_or_default()).unwrap_or_default();
    assert_eq!(list.len(), 4, "every entry is kept as written");
    let x = pdfx_of(&out);
    assert_eq!(x.trapped, None, "/Maybe is not a trapping state");
    let xmp = x.xmp.expect("XMP metadata");
    assert!(xmp.contains("<pdfxid:GTS_PDFXVersion>&lt;PDF/X-4 &amp; &quot;more&quot;&gt;</pdfxid:GTS_PDFXVersion>"), "{xmp}");
    assert!(!xmp.contains('\0') && !xmp.contains("GTS_PDFXConformance"), "{xmp}");
}

#[test]
fn duplicate_pages_become_independent_copies() {
    let out = full_roundtrip(&extract_pages(&doc_b(), &[0, 0]).unwrap());
    assert_eq!(labels(&out), ["B1", "B1"]);
    let ps = pages(&out).unwrap();
    assert_ne!(ps[0].obj, ps[1].obj, "a page object may appear only once in the tree");
}

/// Document C: two pages. Page 1 has a link to the named destination `chap2` (page 2); page 2's
/// content uses a layer that is off by default. It has a bookmark tree and one attachment.
fn doc_c() -> Document {
    let b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Names << /Dests 9 0 R /EmbeddedFiles 13 0 R >> /OCProperties << /OCGs [8 0 R] /D << /OFF [8 0 R] >> >> /Outlines 10 0 R >>".into(), // 1
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 200] >>".into(), // 2
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 7 0 R >> >> /Annots [<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /Dest (chap2) >>] >>".into(), // 3
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> /Properties << /L1 8 0 R >> >> >>".into(), // 4
        body("C1"),                                                                   // 5
        body("C2"),                                                                   // 6
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),             // 7
        "<< /Type /OCG /Name (Draft marks) >>".into(),                                // 8
        "<< /Names [(chap2) [4 0 R /Fit]] >>".into(),                                 // 9
        "<< /Type /Outlines /First 11 0 R /Last 11 0 R /Count 1 >>".into(),          // 10
        "<< /Title (Chapter one) /Parent 10 0 R /Dest [3 0 R /Fit] /First 12 0 R /Last 12 0 R /Count 1 >>".into(), // 11
        "<< /Title (Section) /Parent 11 0 R /A << /S /GoTo /D (chap2) >> >>".into(),  // 12
        "<< /Names [(notes.txt) 14 0 R] >>".into(),                                   // 13
        "<< /Type /Filespec /F (notes.txt) /EF << /F 15 0 R >> >>".into(),            // 14
        "<< /Type /EmbeddedFile /Length 5 >>\nstream\nhello\nendstream".into(),       // 15
    ];
    open(build(&b, "/Root 1 0 R"))
}

fn catalog(doc: &Document) -> Dict {
    doc.get(doc.root().unwrap()).as_dict().cloned().unwrap()
}

#[test]
fn named_destinations_are_resolved_to_the_copies() {
    let out = full_roundtrip(&extract_pages(&doc_c(), &[0, 1]).unwrap());
    let link = &annots(&out, 0)[0];
    let dest = link.get(b"Dest").and_then(|d| d.as_array()).expect("explicit destination");
    assert_eq!(dest[0].as_ref(), Some(pages(&out).unwrap()[1].obj));
    // Without the target page, the link loses its destination instead of dangling.
    let alone = full_roundtrip(&extract_pages(&doc_c(), &[0]).unwrap());
    assert!(!annots(&alone, 0)[0].contains(b"Dest"));
}

#[test]
fn layers_are_registered_with_their_default_state() {
    let out = full_roundtrip(&extract_pages(&doc_c(), &[1]).unwrap());
    let props = out.resolve(catalog(&out).get(b"OCProperties").unwrap()).as_dict().cloned().unwrap();
    let ocgs = props.get(b"OCGs").and_then(|o| o.as_array()).unwrap().clone();
    assert_eq!(ocgs.len(), 1);
    let name = out.resolve(&ocgs[0]).as_dict().unwrap().get(b"Name").and_then(|n| n.as_string()).map(|s| s.to_text());
    assert_eq!(name.as_deref(), Some("Draft marks"));
    let d = props.get(b"D").and_then(|d| d.as_dict()).unwrap();
    assert_eq!(d.get(b"OFF").and_then(|o| o.as_array()).unwrap(), &ocgs, "stays off by default");
    // Page 1 uses no layer, so extracting it alone adds no OCProperties.
    let out = extract_pages(&doc_c(), &[0]).unwrap();
    assert!(!catalog(&out).contains(b"OCProperties"));
}

/// The top-level bookmarks' titles, in order.
fn top_titles(doc: &Document) -> Vec<String> {
    let Some(outlines) = catalog(doc).reference(b"Outlines") else { return Vec::new() };
    let mut item = doc.get(outlines).as_dict().and_then(|o| o.reference(b"First"));
    let mut titles = Vec::new();
    while let Some(r) = item {
        let d = doc.get(r).as_dict().cloned().unwrap();
        titles.push(d.get(b"Title").and_then(|t| t.as_string()).map(|s| s.to_text()).unwrap());
        item = d.reference(b"Next");
    }
    titles
}

#[test]
fn a_file_split_around_another_is_copied_once_with_one_bookmark() {
    // C's first page, then all of A, then C's second page.
    let (a, c) = (doc_a(), doc_c());
    let out = full_roundtrip(&crate::combine_grouped(&[(0, "C", &c, Some(&[0])), (1, "A", &a, None), (0, "C", &c, Some(&[1]))]).unwrap());
    assert_eq!(labels(&out), ["C1", "A1", "A2", "A3", "C2"]);
    assert_eq!(top_titles(&out), ["C", "A"], "one bookmark per file, not per run");
    let ps = pages(&out).unwrap();
    let outlines = out.get(catalog(&out).reference(b"Outlines").unwrap()).as_dict().cloned().unwrap();
    let file = out.get(outlines.reference(b"First").unwrap()).as_dict().cloned().unwrap();
    assert_eq!(file.get(b"Dest").and_then(|d| d.as_array()).unwrap()[0].as_ref(), Some(ps[0].obj), "C's bookmark: its first page shown");
    // Its own bookmarks follow its pages wherever they went: the section is on C2, now last.
    let chapter = out.get(file.reference(b"First").unwrap()).as_dict().cloned().unwrap();
    let section = out.get(chapter.reference(b"First").unwrap()).as_dict().cloned().unwrap();
    assert_eq!(section.get(b"Dest").and_then(|d| d.as_array()).unwrap()[0].as_ref(), Some(ps[4].obj));
    // The link from C1 to C2 still works across A's pages between them.
    let link = &annots(&out, 0)[0];
    assert_eq!(link.get(b"Dest").and_then(|d| d.as_array()).expect("kept").first().and_then(|o| o.as_ref()), Some(ps[4].obj));
    // Its attachment once, not once per run.
    let names = out.resolve(catalog(&out).get(b"Names").unwrap()).as_dict().cloned().unwrap();
    let tree = out.resolve(names.get(b"EmbeddedFiles").unwrap()).as_dict().cloned().unwrap();
    let keys: Vec<String> = tree.get(b"Names").and_then(|n| n.as_array()).unwrap().chunks(2).map(|p| p[0].as_string().unwrap().to_text()).collect();
    assert_eq!(keys, ["notes.txt"]);
}

#[test]
fn a_page_shown_twice_gets_its_own_annotations() {
    // One page with an indirect comment (and its popup), and a second page.
    let b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),                                                          // 1
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 200] >>".into(),                    // 2
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Annots [6 0 R] >>".into(),                            // 3
        "<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>".into(),                                            // 4
        body("D1"),                                                                                          // 5
        "<< /Type /Annot /Subtype /Text /Rect [0 0 10 10] /Contents (note) /P 3 0 R /Popup 8 0 R >>".into(), // 6
        body("D2"),                                                                                          // 7
        "<< /Type /Annot /Subtype /Popup /Rect [10 10 50 50] /Parent 6 0 R >>".into(),                       // 8
    ];
    let d = open(build(&b, "/Root 1 0 R"));
    // Page 1 twice, from one file split around its page 2 (and the same through combine_selected).
    for out in [
        crate::combine_grouped(&[(0, "D", &d, Some(&[0, 1])), (0, "D", &d, Some(&[0]))]).unwrap(),
        combine_selected(&[("D", &d, Some(&[0, 1, 0]))]).unwrap(),
    ] {
        let out = full_roundtrip(&out);
        assert_eq!(labels(&out), ["D1", "D2", "D1"]);
        let ps = pages(&out).unwrap();
        let annot_of = |i: usize| page_dict(&out, i).get(b"Annots").and_then(|a| a.as_array()).and_then(|a| a[0].as_ref()).unwrap();
        let (first, again) = (annot_of(0), annot_of(2));
        assert_ne!(first, again, "two pages, two comments");
        for (annot, page) in [(first, ps[0].obj), (again, ps[2].obj)] {
            let a = out.get(annot).as_dict().cloned().unwrap();
            assert_eq!(a.reference(b"P"), Some(page), "each comment belongs to its own page");
            let popup = out.get(a.reference(b"Popup").unwrap()).as_dict().cloned().unwrap();
            assert_eq!(popup.reference(b"Parent"), Some(annot), "and its popup to it");
        }
    }
}

#[test]
fn grouped_runs_keep_their_order_repeats_and_separate_files() {
    let (a, b) = (doc_a(), doc_b());
    // B's page 2, A's page 1, B's page 1, A's page 1 again (a repeat is a page of its own).
    let out = full_roundtrip(
        &crate::combine_grouped(&[(0, "B", &b, Some(&[1])), (1, "A", &a, Some(&[0])), (0, "B", &b, Some(&[0])), (1, "A", &a, Some(&[0]))]).unwrap(),
    );
    assert_eq!(labels(&out), ["B2", "A1", "B1", "A1"]);
    assert_eq!(top_titles(&out), ["B", "A"]);
    let ps = pages(&out).unwrap();
    assert_ne!(ps[1].obj, ps[3].obj, "a page shown twice is two pages");
    // The same file added twice on purpose (two groups) keeps two bookmarks, as combine does.
    let twice = full_roundtrip(&crate::combine_grouped(&[(0, "A", &a, None), (1, "A again", &a, None)]).unwrap());
    assert_eq!(top_titles(&twice), ["A", "A again"]);
    // A page that isn't there is refused, as combine_selected does.
    assert!(matches!(crate::combine_grouped(&[(0, "A", &a, Some(&[9]))]), Err(crate::OrganizeError::NoSuchPage(9))));
}

#[test]
fn combine_nests_source_bookmarks_and_keeps_attachments() {
    let out = full_roundtrip(&combine(&[("C", &doc_c()), ("C again", &doc_c())]).unwrap());
    assert_eq!(labels(&out), ["C1", "C2", "C1", "C2"]);
    let outlines = out.get(catalog(&out).reference(b"Outlines").unwrap()).as_dict().cloned().unwrap();
    let file = out.get(outlines.reference(b"First").unwrap()).as_dict().cloned().unwrap();
    assert!(file.int(b"Count").unwrap() < 0, "source bookmarks start collapsed");
    let chapter = out.get(file.reference(b"First").unwrap()).as_dict().cloned().unwrap();
    assert_eq!(chapter.get(b"Title").and_then(|t| t.as_string()).map(|s| s.to_text()).as_deref(), Some("Chapter one"));
    let section = out.get(chapter.reference(b"First").unwrap()).as_dict().cloned().unwrap();
    let dest = section.get(b"Dest").and_then(|d| d.as_array()).expect("GoTo (named) became an explicit destination");
    assert_eq!(dest[0].as_ref(), Some(pages(&out).unwrap()[1].obj));
    // The second copy's bookmarks point into the second copy.
    let file2 = out.get(file.reference(b"Next").unwrap()).as_dict().cloned().unwrap();
    let chapter2 = out.get(file2.reference(b"First").unwrap()).as_dict().cloned().unwrap();
    assert_eq!(chapter2.get(b"Dest").and_then(|d| d.as_array()).unwrap()[0].as_ref(), Some(pages(&out).unwrap()[2].obj));
    // Attachments from both sources, the duplicate name disambiguated.
    let names = out.resolve(catalog(&out).get(b"Names").unwrap()).as_dict().cloned().unwrap();
    let tree = out.resolve(names.get(b"EmbeddedFiles").unwrap()).as_dict().cloned().unwrap();
    let keys: Vec<String> = tree.get(b"Names").and_then(|n| n.as_array()).unwrap().chunks(2).map(|p| p[0].as_string().unwrap().to_text()).collect();
    assert_eq!(keys, ["notes.txt", "notes.txt (2)"]);
}

#[test]
fn combine_and_insert_share_identical_resources() {
    let a = doc_a();
    let fonts = |d: &Document| {
        d.object_numbers()
            .into_iter()
            .filter(|n| d.get(ObjRef::new(*n, d.generation(*n))).as_dict().is_some_and(|x| x.name(b"Type") == Some(b"Font")))
            .count()
    };
    assert_eq!(fonts(&a), 1);
    // The same document three times: one font, and every page still renders its own text.
    let out = full_roundtrip(&crate::combine(&[("one", &a), ("two", &a), ("three", &a)]).unwrap());
    assert_eq!(fonts(&out), 1);
    assert_eq!(labels(&out), ["A1", "A2", "A3", "A1", "A2", "A3", "A1", "A2", "A3"]);
    // Inserting pages that use the same font reuses the font already there, without touching it.
    let mut d = doc_a();
    let font_before =
        d.object_numbers().into_iter().find(|n| d.get(ObjRef::new(*n, 0)).as_dict().is_some_and(|x| x.name(b"Type") == Some(b"Font"))).unwrap();
    crate::import_pages(&mut d, &doc_a(), &[0, 1], 3).unwrap();
    assert_eq!(fonts(&d), 1);
    assert!(!d.modified_objects().contains(&font_before));
    let d = full_roundtrip(&d);
    assert_eq!(labels(&d), ["A1", "A2", "A3", "A1", "A2"]);
}

/// A one-page document drawing one image, written the way ImageMagick writes it: the colour
/// space is an indirect `/DeviceRGB` object of its own (#878).
fn indirect_colour_space_doc() -> Document {
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
        stream(b"q 100 0 0 100 0 0 cm /Im0 Do Q"),
        [
            b"<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace 6 0 R /BitsPerComponent 8 /Length 6 >>\nstream\n".as_slice(),
            &[255, 0, 0, 0, 0, 255],
            b"\nendstream",
        ]
        .concat(),
        b"/DeviceRGB".to_vec(),
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
    open(out)
}

#[test]
fn combine_stores_an_image_once_when_its_colour_space_is_an_indirect_name() {
    let images = |d: &Document| {
        d.object_numbers()
            .into_iter()
            .filter(|n| matches!(&*d.get(ObjRef::new(*n, d.generation(*n))), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image")))
            .count()
    };
    let src = indirect_colour_space_doc();
    let out = full_roundtrip(&crate::combine(&[("one", &src), ("two", &src), ("three", &src)]).unwrap());
    assert_eq!(images(&out), 1);
    assert_eq!(pages(&out).unwrap().len(), 3);
    // Inserting the same pages again reuses the image already there.
    let mut d = indirect_colour_space_doc();
    crate::import_pages(&mut d, &indirect_colour_space_doc(), &[0], 1).unwrap();
    assert_eq!(images(&full_roundtrip(&d)), 1);
}

// ---- bookmarks (M4.6) --------------------------------------------------------------------------

/// The titles with children in brackets (`Parent[Child,Child]`), one per top-level bookmark.
fn titles(b: &[crate::Bookmark]) -> Vec<String> {
    fn level(b: &[crate::Bookmark], at: &mut usize, depth: usize) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(x) = b.get(*at).filter(|x| x.path.len() == depth + 1) {
            *at += 1;
            let kids = level(b, at, depth + 1);
            out.push(if kids.is_empty() { x.title.clone() } else { format!("{}[{}]", x.title, kids.join(",")) });
        }
        out
    }
    let mut at = 0;
    level(b, &mut at, 0)
}

fn count_of(doc: &Document, r: ObjRef) -> Option<i64> {
    doc.get(r).as_dict().and_then(|d| d.int(b"Count"))
}

#[test]
fn bookmarks_add_rename_move_delete() {
    let mut d = doc_a();
    assert!(crate::bookmarks(&d).is_empty());
    assert_eq!(crate::add_bookmark(&mut d, &[], 0, "Intro", 0).unwrap(), vec![0]);
    assert_eq!(crate::add_bookmark(&mut d, &[], 9, "Results", 2).unwrap(), vec![1]);
    assert_eq!(crate::add_bookmark(&mut d, &[1], 0, "Table", 1).unwrap(), vec![1, 0]);
    crate::add_bookmark(&mut d, &[1], 1, "Chart", 2).unwrap();
    assert_eq!(titles(&crate::bookmarks(&d)), ["Intro", "Results[Table,Chart]"]);

    crate::rename_bookmark(&mut d, &[1, 1], "Figure 1").unwrap();
    assert_eq!(crate::move_bookmark(&mut d, &[1, 1], &[], 0).unwrap(), vec![0]);
    assert_eq!(titles(&crate::bookmarks(&d)), ["Figure 1", "Intro", "Results[Table]"]);
    // Into a later sibling: the target path is adjusted for the removal.
    assert_eq!(crate::move_bookmark(&mut d, &[0], &[2], 9).unwrap(), vec![1, 1]);
    assert_eq!(titles(&crate::bookmarks(&d)), ["Intro", "Results[Table,Figure 1]"]);
    assert_eq!(crate::move_bookmark(&mut d, &[1], &[1, 0], 0), Err(crate::OutlineError::IntoItself));
    crate::delete_bookmark(&mut d, &[1, 0]).unwrap();
    assert_eq!(titles(&crate::bookmarks(&d)), ["Intro", "Results[Figure 1]"]);

    // Counts: the root counts visible items; open items count their visible descendants.
    let b = crate::bookmarks(&d);
    let root = d.get(d.root().unwrap()).as_dict().unwrap().reference(b"Outlines").unwrap();
    assert_eq!(count_of(&d, root), Some(3));
    assert_eq!(count_of(&d, b[1].obj), Some(1));
    crate::set_bookmark_open(&mut d, &[1], false).unwrap();
    assert_eq!((count_of(&d, b[1].obj), count_of(&d, root)), (Some(-1), Some(2)));

    // Destinations point at the chosen page; errors are specific.
    crate::set_bookmark_page(&mut d, &[0], 2).unwrap();
    let dest = d.get(b[0].obj).as_dict().unwrap().get(b"Dest").cloned().unwrap();
    let pages = crate::walk(&d).unwrap();
    assert_eq!(dest.as_array().unwrap()[0], Object::Ref(pages[2].0));
    assert_eq!(crate::rename_bookmark(&mut d, &[5], "x"), Err(crate::OutlineError::NoSuchBookmark(vec![5])));
    assert_eq!(crate::rename_bookmark(&mut d, &[0], "  "), Err(crate::OutlineError::EmptyTitle));
    assert!(crate::add_bookmark(&mut d, &[], 0, "Nowhere", 7).is_err());

    // Everything survives a save and reopen.
    let back = full_roundtrip(&d);
    assert_eq!(titles(&crate::bookmarks(&back)), ["Intro", "Results[Figure 1]"]);
}

#[test]
fn a_bookmark_tree_nests_entries_by_level_under_a_new_first_bookmark() {
    let mut d = doc_a();
    crate::add_bookmark(&mut d, &[], 0, "Existing", 0).unwrap();
    let e = |level, title: &str, page, element| crate::OutlineEntry { level, title: title.into(), page, element };
    let se = ObjRef::new(4242, 0);
    let entries = [e(1, "One", 0, Some(se)), e(3, "Deep", 1, None), e(2, "Two", 1, None), e(1, "Next", 2, None)];
    assert_eq!(crate::add_bookmark_tree(&mut d, "Untitled", &entries).unwrap(), vec![0]);
    assert_eq!(titles(&crate::bookmarks(&d)), ["Untitled[One[Deep,Two],Next]", "Existing"]);

    let b = crate::bookmarks(&d);
    let root = d.get(d.root().unwrap()).as_dict().unwrap().reference(b"Outlines").unwrap();
    // Listing order: Untitled, One, Deep, Two, Next, Existing.
    assert_eq!((count_of(&d, root), count_of(&d, b[0].obj), count_of(&d, b[1].obj)), (Some(6), Some(4), Some(2)));
    let one = d.get(b[1].obj).as_dict().cloned().unwrap();
    assert_eq!(one.get(b"SE"), Some(&Object::Ref(se)));
    let pages = crate::walk(&d).unwrap();
    let next = d.get(b[4].obj).as_dict().cloned().unwrap();
    assert_eq!(next.get(b"Dest").and_then(|x| x.as_array()).map(|a| a[0].clone()), Some(Object::Ref(pages[2].0)));

    assert_eq!(crate::add_bookmark_tree(&mut d, "Untitled", &[]), Err(crate::OutlineError::NoEntries));
    assert!(crate::add_bookmark_tree(&mut d, "Untitled", &[e(1, "Nowhere", 9, None)]).is_err());
    assert_eq!(titles(&crate::bookmarks(&full_roundtrip(&d))), ["Untitled[One[Deep,Two],Next]", "Existing"]);
}

#[test]
fn bookmark_edits_keep_unknown_keys_and_touch_few_objects() {
    let mut d = doc_a();
    crate::add_bookmark(&mut d, &[], 0, "One", 0).unwrap();
    crate::add_bookmark(&mut d, &[], 1, "Two", 1).unwrap();
    let first = crate::bookmarks(&d)[0].obj;
    d.update_dict(first, |x| x.set(b"C".to_vec(), Object::Array(vec![Object::Real(1.0), Object::Int(0), Object::Int(0)]))).unwrap();
    let bytes = std::sync::Arc::new(write_full(&d, &SaveOptions::default()).unwrap());
    let mut d = Document::open(bytes).unwrap();
    crate::rename_bookmark(&mut d, &[0], "Uno").unwrap();
    let b = crate::bookmarks(&d);
    assert_eq!(b[0].title, "Uno");
    assert!(d.get(b[0].obj).as_dict().unwrap().get(b"C").is_some(), "colour kept");
    assert_eq!(d.modified_objects(), vec![b[0].obj.num], "a rename rewrites only that item");
}

/// A flat outline of `n` top-level bookmarks, `Item 0` to `Item n-1`, all going to one page.
fn flat_outline(n: usize) -> Vec<u8> {
    let mut b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Outlines 3 0 R >>".into(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        format!("<< /Type /Outlines /First 5 0 R /Last {} 0 R /Count {n} >>", 4 + n),
        "<< /Type /Page /Parent 2 0 R >>".into(),
    ];
    for k in 0..n {
        let prev = if k == 0 { String::new() } else { format!(" /Prev {} 0 R", 4 + k) };
        let next = if k + 1 == n { String::new() } else { format!(" /Next {} 0 R", 6 + k) };
        b.push(format!("<< /Title (Item {k}) /Parent 3 0 R{prev}{next} /Dest [4 0 R /Fit] >>"));
    }
    build(&b, "/Root 1 0 R")
}

#[test]
fn a_bookmark_past_the_hundred_thousandth_is_listed() {
    let d = open(flat_outline(100_005));
    let all = crate::bookmarks(&d);
    assert_eq!(all.len(), 100_005);
    assert_eq!(all.last().map(|b| b.title.as_str()), Some("Item 100004"));
}

/// A chain of `n` nested bookmarks: each is the first child of the one before it.
fn deep_outline(n: usize) -> Vec<u8> {
    let mut b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Outlines 3 0 R >>".into(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        "<< /Type /Outlines /First 5 0 R /Last 5 0 R /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
    ];
    for k in 0..n {
        let parent = if k == 0 { 3 } else { 4 + k };
        let first = if k + 1 == n { String::new() } else { format!(" /First {} 0 R /Count -1", 6 + k) };
        b.push(format!("<< /Title (Deep {k}) /Parent {parent} 0 R{first} /Dest [4 0 R /Fit] >>"));
    }
    build(&b, "/Root 1 0 R")
}

#[test]
fn deep_bookmark_chains_edit_without_recursion() {
    let mut d = open(deep_outline(100_000));
    // Adding at the top level recounts the whole outline, which walks the entire chain.
    crate::add_bookmark(&mut d, &[], 1, "Extra", 0).unwrap();
    // Listing enters levels 0 to 32 of the chain; the rest is reached by path only.
    let listed = crate::bookmarks(&d);
    assert_eq!(listed.len(), 34);
    assert_eq!(listed.last().map(|b| (b.title.as_str(), b.path.clone())), Some(("Extra", vec![1])));
    assert_eq!(listed[32].path.len(), 33);
    assert_eq!(crate::bookmark_page(&d, 33, 10).bookmarks.len(), 1);
}

/// A one-page document whose outline is the given bookmark bodies, as objects 5, 6, … in order.
fn outline_with(items: &[&str]) -> Document {
    let mut b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Outlines 3 0 R >>".into(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        format!("<< /Type /Outlines /First 5 0 R /Count {} >>", items.len()),
        "<< /Type /Page /Parent 2 0 R >>".into(),
    ];
    b.extend(items.iter().map(|s| s.to_string()));
    open(build(&b, "/Root 1 0 R"))
}

#[test]
fn hostile_outlines_end_their_runs_and_never_loop() {
    let listed = |items: &[&str]| titles(&crate::bookmarks(&outline_with(items)));
    // A bookmark that is its own next sibling, or its own first child.
    assert_eq!(listed(&["<< /Title (Self) /Parent 3 0 R /Next 5 0 R >>"]), ["Self"]);
    assert_eq!(listed(&["<< /Title (Self) /Parent 3 0 R /First 5 0 R >>"]), ["Self"]);
    // Two siblings that point at each other: the run ends where it would repeat.
    assert_eq!(listed(&["<< /Title (A) /Parent 3 0 R /Next 6 0 R >>", "<< /Title (B) /Parent 3 0 R /Next 5 0 R >>"]), ["A", "B"]);
    // A child that leads back to its ancestor.
    assert_eq!(listed(&["<< /Title (A) /Parent 3 0 R /First 6 0 R >>", "<< /Title (B) /Parent 5 0 R /First 5 0 R >>"]), ["A[B]"]);
    // A broken `/Next` (a missing object, then a number) and a missing first bookmark.
    assert_eq!(listed(&["<< /Title (A) /Parent 3 0 R /Next 99 0 R >>"]), ["A"]);
    assert_eq!(listed(&["<< /Title (A) /Parent 3 0 R /Next 6 0 R >>", "42"]), ["A"]);
    assert!(listed(&[]).is_empty());
}

#[test]
fn bookmark_pages_list_every_bookmark_once() {
    let d = open(flat_outline(250));
    let mut seen = Vec::new();
    let mut offset = 0;
    while let Some(next) = {
        let page = crate::bookmark_page(&d, offset, 100);
        assert!(page.bookmarks.len() <= 100);
        seen.extend(page.bookmarks.iter().map(|b| b.title.clone()));
        page.next
    } {
        assert_eq!(next, seen.len(), "a page ends where the next one starts");
        offset = next;
    }
    assert_eq!(seen.len(), 250);
    assert_eq!(seen.last().map(String::as_str), Some("Item 249"));

    let big = open(flat_outline(100_005));
    let first = crate::bookmark_page(&big, 0, 100);
    assert_eq!((first.bookmarks.len(), first.next), (100, Some(100)));
    let late = crate::bookmark_page(&big, 100_000, 100);
    assert_eq!(
        late.bookmarks.iter().map(|b| b.title.as_str()).collect::<Vec<_>>(),
        ["Item 100000", "Item 100001", "Item 100002", "Item 100003", "Item 100004"]
    );
    assert_eq!((late.bookmarks[4].path.clone(), late.next), (vec![100_004], None));
}

#[test]
fn bookmarks_name_their_pages_through_every_kind_of_destination() {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Outlines 3 0 R /Names << /Dests 9 0 R >> >>".into(),
        "<< /Type /Pages /Kids [4 0 R 5 0 R] /Count 2 /MediaBox [0 0 300 400] >>".into(),
        "<< /Type /Outlines /First 6 0 R /Last 11 0 R /Count 6 >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R >>".into(),
        "<< /Title (Named) /Parent 3 0 R /Next 7 0 R /Dest (intro) >>".into(),
        "<< /Title (GoTo) /Parent 3 0 R /Prev 6 0 R /Next 8 0 R /A << /S /GoTo /D [5 0 R /Fit] >> >>".into(),
        "<< /Title (Appendix) /Parent 3 0 R /Prev 7 0 R /Next 10 0 R /Dest /appendix >>".into(),
        "<< /Names [(appendix) 12 0 R (intro) [4 0 R /Fit]] >>".into(),
        "<< /Title <FEFF00480069> /Parent 3 0 R /Prev 8 0 R /Next 11 0 R /Dest [1 /Fit] >>".into(),
        "<< /Title (  Padded  ) /Parent 3 0 R /Prev 10 0 R /Dest [2 0 R /Fit] >>".into(),
        "<< /D [5 0 R /Fit] >>".into(),
    ];
    let d = open(build(&objs, "/Root 1 0 R"));
    let listed: Vec<String> = crate::bookmarks(&d).iter().map(|b| format!("{}@{:?}", b.title, b.page)).collect();
    assert_eq!(listed, ["Named@Some(0)", "GoTo@Some(1)", "Appendix@Some(1)", "Hi@Some(1)", "Padded@None"]);
}

// ---- page labels (M4.4) ------------------------------------------------------------------------

#[test]
fn number_pages_like_acrobat() {
    use crate::LabelStyle::*;
    let mut d = doc_a();
    crate::insert_blank_page(&mut d, 3, 300.0, 400.0).unwrap();
    crate::insert_blank_page(&mut d, 4, 300.0, 400.0).unwrap();
    crate::insert_blank_page(&mut d, 5, 300.0, 400.0).unwrap();
    assert_eq!(crate::page_labels(&d).unwrap(), ["1", "2", "3", "4", "5", "6"]);
    // Front matter in roman numerals; later pages keep their labels.
    crate::number_pages(&mut d, 0, 1, LowerRoman, "", 1).unwrap();
    assert_eq!(crate::page_labels(&d).unwrap(), ["i", "ii", "3", "4", "5", "6"]);
    // The body restarts at 1, an appendix gets a prefix.
    crate::number_pages(&mut d, 2, 5, Decimal, "", 1).unwrap();
    crate::number_pages(&mut d, 4, 5, Decimal, "A-", 1).unwrap();
    assert_eq!(crate::page_labels(&d).unwrap(), ["i", "ii", "1", "2", "A-1", "A-2"]);
    assert_eq!(crate::page_label_ranges(&d).len(), 3, "continuations are merged");
    // Relabelling the middle keeps the pages after it.
    crate::number_pages(&mut d, 3, 3, UpperAlpha, "Fig ", 3).unwrap();
    assert_eq!(crate::page_labels(&d).unwrap(), ["i", "ii", "1", "Fig C", "A-1", "A-2"]);
    assert!(crate::number_pages(&mut d, 4, 9, Decimal, "", 1).is_err());
    // Survives a save and reopen; back to plain numbers removes /PageLabels.
    let back = full_roundtrip(&d);
    assert_eq!(crate::page_labels(&back).unwrap(), ["i", "ii", "1", "Fig C", "A-1", "A-2"]);
    crate::number_pages(&mut d, 0, 5, Decimal, "", 1).unwrap();
    assert!(crate::page_label_ranges(&d).is_empty());
    assert!(d.get(d.root().unwrap()).as_dict().unwrap().get(b"PageLabels").is_none());
}

#[test]
fn page_label_limits_reject_unsupported_edits_before_mutating() {
    use crate::LabelStyle::*;
    for (style, prefix, first) in [
        (LowerRoman, String::new(), 1_025_000),
        (LowerAlpha, String::new(), 26 * 1024 + 1),
        (Decimal, "x".repeat(1025), 1),
        (Decimal, String::new(), u32::MAX),
    ] {
        let mut d = doc_a();
        let before = d.modified_objects();
        assert!(crate::number_pages(&mut d, 0, 1, style, &prefix, first).is_err());
        assert_eq!(d.modified_objects(), before, "a refused label edit must not change the tree");
        assert!(crate::page_label_ranges(&d).is_empty());
    }
}

#[test]
fn page_label_limits_reject_unsupported_ranges_before_mutating() {
    let mut d = doc_a();
    let before = d.modified_objects();
    let ranges = [crate::LabelRange { start: 0, style: crate::LabelStyle::LowerRoman, prefix: String::new(), first: 1_025_000 }];
    assert!(crate::set_page_label_ranges(&mut d, &ranges).is_err());
    assert_eq!(d.modified_objects(), before);
}

#[test]
fn page_label_limits_reject_cyclic_old_trees_without_rewriting() {
    let mut d = doc_a();
    let tree = d.add(Object::Null);
    let mut dict = Dict::new();
    dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(tree)]));
    d.set(tree, Object::Dict(dict));
    d.update_dict(d.root().unwrap(), |c| c.set(b"PageLabels".to_vec(), Object::Ref(tree))).unwrap();
    let before = d.modified_objects();
    assert!(crate::page_labels(&d).is_err());
    assert!(crate::page_label_ranges(&d).is_empty(), "compatibility query has documented empty fallback");
    assert!(crate::number_pages(&mut d, 0, 1, crate::LabelStyle::Decimal, "", 1).is_err());
    assert_eq!(d.modified_objects(), before);
    assert_eq!(d.get(d.root().unwrap()).as_dict().unwrap().reference(b"PageLabels"), Some(tree));
}

#[test]
fn page_label_limits_reject_oversized_old_arrays_without_rewriting() {
    let mut d = doc_a();
    let mut dict = Dict::new();
    dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Null; pdfcraft_cos::page_labels::MAX_LABEL_TREE_WORK + 1]));
    let tree = d.add(Object::Dict(dict));
    d.update_dict(d.root().unwrap(), |c| c.set(b"PageLabels".to_vec(), Object::Ref(tree))).unwrap();
    let before = d.modified_objects();
    assert!(crate::page_labels(&d).is_err());
    assert!(crate::number_pages(&mut d, 0, 1, crate::LabelStyle::Decimal, "", 1).is_err());
    assert_eq!(d.modified_objects(), before);
}

#[test]
fn page_label_limits_keep_preview_fallback_and_valid_boundary_edits() {
    use crate::LabelStyle::*;
    assert_eq!(LowerRoman.format(u32::MAX), u32::MAX.to_string());
    assert_eq!(LowerAlpha.format(u32::MAX), u32::MAX.to_string());
    let mut d = doc_a();
    crate::number_pages(&mut d, 0, 2, None, &"x".repeat(1024), 1).unwrap();
    assert!(crate::page_labels(&d).unwrap().iter().all(|s| s.len() == 1024));
}

#[test]
fn page_label_limits_preserve_prefix_only_max_first_and_aliases() {
    let mut d = doc_a();
    crate::number_pages(&mut d, 0, 2, crate::LabelStyle::None, "prefix", u32::MAX).unwrap();
    assert_eq!(crate::page_labels(&d).unwrap(), ["prefix", "prefix", "prefix"]);
    crate::number_pages(&mut d, 0, 0, crate::LabelStyle::Decimal, "", 1).unwrap();
    assert_eq!(crate::page_labels(&d).unwrap(), ["1", "prefix", "prefix"]);

    let prefix = d.add(Object::String(PdfString::text("alias")));
    let prefix_alias = d.add(Object::Ref(prefix));
    let mut spec = Dict::new();
    spec.set(b"P".to_vec(), Object::Ref(prefix_alias));
    spec.set(b"St".to_vec(), Object::Int(i64::MAX)); // unused without /S
    let spec = d.add(Object::Dict(spec));
    let spec_alias = d.add(Object::Ref(spec));
    let nums = d.add(Object::Array(vec![Object::Int(0), Object::Ref(spec_alias)]));
    let nums_alias = d.add(Object::Ref(nums));
    let mut tree = Dict::new();
    tree.set(b"Nums".to_vec(), Object::Ref(nums_alias));
    let tree = d.add(Object::Dict(tree));
    let tree_alias = d.add(Object::Ref(tree));
    d.update_dict(d.root().unwrap(), |c| c.set(b"PageLabels".to_vec(), Object::Ref(tree_alias))).unwrap();
    assert_eq!(crate::page_labels(&d).unwrap(), ["alias", "alias", "alias"]);
    crate::number_pages(&mut d, 0, 0, crate::LabelStyle::Decimal, "", 1).unwrap();
    assert_eq!(crate::page_labels(&d).unwrap(), ["1", "alias", "alias"]);
}

#[test]
fn page_label_limits_refuse_reference_only_cycles_without_rewriting() {
    let mut d = doc_a();
    let tree = d.add(Object::Null);
    d.set(tree, Object::Ref(tree));
    d.update_dict(d.root().unwrap(), |c| c.set(b"PageLabels".to_vec(), Object::Ref(tree))).unwrap();
    let before = d.modified_objects();
    assert!(crate::page_labels(&d).is_err());
    assert!(crate::number_pages(&mut d, 0, 1, crate::LabelStyle::Decimal, "", 1).is_err());
    assert_eq!(d.modified_objects(), before);
}

#[test]
fn page_label_limits_validate_the_same_ranges_that_are_written() {
    use crate::{LabelRange, LabelStyle};
    let mut d = doc_a();
    let duplicates = [
        LabelRange { start: 0, style: LabelStyle::None, prefix: "ok".into(), first: 1 },
        LabelRange { start: 0, style: LabelStyle::None, prefix: "x".repeat(1024), first: 1 },
    ];
    let before = d.modified_objects();
    assert!(crate::set_page_label_ranges(&mut d, &duplicates).unwrap_err().to_string().contains("unique"));
    assert_eq!(d.modified_objects(), before);
    let unsorted = [
        LabelRange { start: 1, style: LabelStyle::UpperAlpha, prefix: "".into(), first: 1 },
        LabelRange { start: 0, style: LabelStyle::LowerRoman, prefix: "".into(), first: 4 },
    ];
    crate::set_page_label_ranges(&mut d, &unsorted).unwrap();
    assert_eq!(crate::page_labels(&d).unwrap(), ["iv", "A", "B"]);
    let catalog = d.get(d.root().unwrap());
    let tree = d.resolve(catalog.as_dict().unwrap().get(b"PageLabels").unwrap());
    let nums = tree.as_dict().unwrap().get(b"Nums").unwrap().as_array().unwrap();
    assert_eq!(nums[0].as_int(), Some(0));
    assert_eq!(nums[2].as_int(), Some(1));
}

#[test]
fn page_label_duplicate_starts_keep_the_later_range() {
    let mut d = doc_a();
    let mut a = Dict::new();
    a.set(b"P".to_vec(), Object::String(PdfString::text("first")));
    let mut b = Dict::new();
    b.set(b"P".to_vec(), Object::String(PdfString::text("last")));
    let mut tree = Dict::new();
    tree.set(b"Nums".to_vec(), Object::Array(vec![Object::Int(0), Object::Dict(a), Object::Int(0), Object::Dict(b)]));
    let tree = d.add(Object::Dict(tree));
    d.update_dict(d.root().unwrap(), |c| c.set(b"PageLabels".to_vec(), Object::Ref(tree))).unwrap();
    assert_eq!(crate::page_labels(&d).unwrap()[0], "last");
    crate::number_pages(&mut d, 0, 0, crate::LabelStyle::Decimal, "", 1).unwrap();
}

/// Two kids naming objects the file doesn't have both resolve to a fresh `Null`; they are not
/// the same node, so the tree isn't refused as cyclic.
#[test]
fn page_label_missing_kids_are_not_a_cycle() {
    let mut d = doc_a();
    let mut leaf = Dict::new();
    let mut spec = Dict::new();
    spec.set(b"P".to_vec(), Object::String(PdfString::text("p-")));
    spec.set(b"S".to_vec(), Object::name("D"));
    leaf.set(b"Nums".to_vec(), Object::Array(vec![Object::Int(0), Object::Dict(spec)]));
    let leaf = d.add(Object::Dict(leaf));
    let mut tree = Dict::new();
    let missing = |n| Object::Ref(pdfcraft_cos::ObjRef { num: n, generation: 0 });
    tree.set(b"Kids".to_vec(), Object::Array(vec![missing(9_000), missing(9_001), Object::Ref(leaf)]));
    let tree = d.add(Object::Dict(tree));
    d.update_dict(d.root().unwrap(), |c| c.set(b"PageLabels".to_vec(), Object::Ref(tree))).unwrap();
    assert_eq!(crate::page_labels(&d).unwrap()[0], "p-1");
}

/// A range whose labels would be too long shows physical page numbers; other ranges keep
/// their labels.
#[test]
fn page_label_too_long_range_falls_back_alone() {
    let mut d = doc_a();
    let mut ok = Dict::new();
    ok.set(b"P".to_vec(), Object::String(PdfString::text("ok-")));
    ok.set(b"S".to_vec(), Object::name("D"));
    let mut huge = Dict::new();
    huge.set(b"S".to_vec(), Object::name("r"));
    huge.set(b"St".to_vec(), Object::Int(1_025_000));
    let mut tree = Dict::new();
    tree.set(b"Nums".to_vec(), Object::Array(vec![Object::Int(0), Object::Dict(ok), Object::Int(1), Object::Dict(huge)]));
    let tree = d.add(Object::Dict(tree));
    d.update_dict(d.root().unwrap(), |c| c.set(b"PageLabels".to_vec(), Object::Ref(tree))).unwrap();
    let labels = crate::page_labels(&d).unwrap();
    assert_eq!(labels[0], "ok-1");
    assert_eq!(labels[1], "2");
}

/// A document whose page tree is one flat node over `pages` leaves. The label tests below need
/// more pages than `doc_a` has.
fn flat_doc(pages: usize) -> Document {
    let kids = (0..pages).map(|i| format!("{} 0 R", i + 3)).collect::<Vec<_>>().join(" ");
    let mut objs = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(), format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>")];
    objs.extend((0..pages).map(|_| "<< /Type /Page /Parent 2 0 R >>".to_string()));
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    open(out)
}

/// Each label is valid (1,004 bytes), but the 5,000 of them total about 5 MB, past the old 4 MiB
/// limit on the whole sequence. Every label is readable.
#[test]
fn page_labels_past_four_mib_in_total_stay_readable() {
    let mut d = flat_doc(5_000);
    let mut spec = Dict::new();
    spec.set(b"P".to_vec(), Object::String(PdfString::text(&"x".repeat(1000))));
    spec.set(b"S".to_vec(), Object::name("D"));
    let mut tree = Dict::new();
    tree.set(b"Nums".to_vec(), Object::Array(vec![Object::Int(0), Object::Dict(spec)]));
    let tree = d.add(Object::Dict(tree));
    d.update_dict(d.root().unwrap(), |c| c.set(b"PageLabels".to_vec(), Object::Ref(tree))).unwrap();
    let labels = crate::page_labels(&d).unwrap();
    assert_eq!(labels.len(), 5_000);
    assert_eq!(labels[0], format!("{}1", "x".repeat(1000)));
    assert_eq!(labels[4_999], format!("{}5000", "x".repeat(1000)));
}

/// The same 5,000 labels written by an edit: the edit is accepted and reads back.
#[test]
fn number_pages_past_four_mib_in_total_is_accepted() {
    let mut d = flat_doc(5_000);
    crate::number_pages(&mut d, 0, 4_999, crate::LabelStyle::Decimal, &"x".repeat(1000), 1).unwrap();
    assert_eq!(crate::page_labels(&d).unwrap()[4_999], format!("{}5000", "x".repeat(1000)));
}

#[test]
fn page_boxes_default_inherit_and_set() {
    let mut doc = open(fixture());
    let b = page_boxes(&doc).unwrap();
    // Pages 1–2 inherit a 300×400 media box; page 3 has its own 500×500. Crop defaults to media.
    assert_eq!(b[0][0], [0.0, 0.0, 300.0, 400.0]);
    assert_eq!(b[2][1], [0.0, 0.0, 500.0, 500.0]);
    set_page_box(&mut doc, &[0, 2], PageBox::Crop, BoxSpec::Margins([10.0, 20.0, 30.0, 40.0])).unwrap();
    set_page_box(&mut doc, &[1], PageBox::Trim, BoxSpec::Rect([-50.0, 50.0, 100.0, 1000.0])).unwrap();
    let doc = open(write_incremental(&doc, &SaveOptions::default()).unwrap());
    let b = page_boxes(&doc).unwrap();
    assert_eq!(b[0][1], [10.0, 20.0, 270.0, 360.0]);
    assert_eq!(b[2][1], [10.0, 20.0, 470.0, 460.0], "margins apply to each page's own media box");
    assert_eq!(b[1][3], [0.0, 50.0, 100.0, 400.0], "clipped to the media box");
    assert_eq!(b[1][4], b[1][1], "art box defaults to the crop box");
    // Removing goes back to the default; impossible requests change nothing.
    let mut doc = doc;
    set_page_box(&mut doc, &[0], PageBox::Crop, BoxSpec::Remove).unwrap();
    assert_eq!(page_boxes(&doc).unwrap()[0][1], [0.0, 0.0, 300.0, 400.0]);
    let before = page_boxes(&doc).unwrap();
    assert!(matches!(set_page_box(&mut doc, &[0, 1], PageBox::Crop, BoxSpec::Margins([200.0, 0.0, 200.0, 0.0])), Err(OrganizeError::InvalidBox(_))));
    assert!(matches!(set_page_box(&mut doc, &[0], PageBox::Media, BoxSpec::Remove), Err(OrganizeError::InvalidBox(_))));
    assert_eq!(page_boxes(&doc).unwrap(), before);
    assert_eq!(set_page_box(&mut doc, &[7], PageBox::Crop, BoxSpec::Remove), Err(OrganizeError::NoSuchPage(7)));
}

#[test]
fn duplicate_pages_inserts_copies_after_the_last() {
    let mut doc = open(fixture());
    duplicate_pages(&mut doc, &[0, 1]).unwrap();
    let doc = open(write_full(&doc, &SaveOptions::default()).unwrap());
    assert_eq!(labels(&doc), ["Page 1", "Page 2", "Page 1", "Page 2", "Page 3"]);
}

#[test]
fn replace_pages_swaps_content_but_keeps_annotations() {
    // Page 2 of the target gets an annotation; replacing its content keeps it.
    let mut doc = open(fixture());
    let p2 = pages(&doc).unwrap()[1].obj;
    let annot = doc.add(Object::Dict({
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Annot"));
        d.set(b"Subtype".to_vec(), Object::name("Square"));
        d.set(b"Rect".to_vec(), Object::Array(vec![0.into(), 0.into(), 10.into(), 10.into()]));
        d
    }));
    doc.update_dict(p2, |d| d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(annot)]))).unwrap();
    let src = open(fixture());
    replace_pages(&mut doc, &[1], &src, &[2]).unwrap();
    let doc = open(write_full(&doc, &SaveOptions::default()).unwrap());
    assert_eq!(labels(&doc), ["Page 1", "Page 3", "Page 3"]);
    let p2 = doc.get(pages(&doc).unwrap()[1].obj);
    assert!(p2.as_dict().unwrap().contains(b"Annots"), "the original page's annotations stay");
    let mut doc = doc;
    assert!(replace_pages(&mut doc, &[0, 1], &src, &[0]).is_err(), "counts must match");
}

#[test]
fn a_page_becomes_a_form_xobject_upright() {
    let src = open(fixture());
    let mut dst = Document::new_empty();
    // Page 1 inherits a 300 × 400 media box, rotation 90 and its font resources.
    let (r, size) = crate::page_as_form(&mut dst, &src, 0).unwrap();
    assert_eq!(size, (400.0, 300.0), "displayed size of a quarter-turned page");
    let obj = dst.get(r);
    let Object::Stream(s) = &*obj else { panic!("a stream") };
    let m: Vec<f64> = s.dict.get(b"Matrix").unwrap().as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert_eq!(m, vec![0.0, -1.0, 1.0, 0.0, 0.0, 300.0]);
    assert!(String::from_utf8_lossy(&s.decoded().unwrap()).contains("(Page 1) Tj"));
    let res = s.dict.get(b"Resources").and_then(|r| dst.resolve(r).as_dict().cloned()).unwrap();
    assert!(res.get(b"Font").is_some(), "inherited resources come along");
    assert!(crate::page_as_form(&mut dst, &src, 7).is_err());
}

#[test]
fn initial_view_round_trips_and_keeps_scripts() {
    use crate::view::{Layout, Magnification, Navigation};
    let mut doc = open(fixture());
    assert_eq!(crate::initial_view(&doc), crate::InitialView::default());
    let v = crate::InitialView {
        navigation: Navigation::Bookmarks,
        layout: Layout::TwoUpContinuousCoverPage,
        magnification: Magnification::Percent(150.0),
        page: 2,
        fit_window: true,
        display_title: true,
        hide_toolbar: true,
        language: Some("fr-FR".into()),
        right_to_left: true,
        ..Default::default()
    };
    crate::set_initial_view(&mut doc, &v).unwrap();
    let doc = save_and_reopen(&doc);
    assert_eq!(crate::initial_view(&doc), v);
    // A script opening action survives a change that needs no destination.
    let mut doc = doc;
    let root = doc.root().unwrap();
    let mut js = pdfcraft_cos::Dict::new();
    js.set(b"S".to_vec(), Object::name("JavaScript"));
    js.set(b"JS".to_vec(), pdfcraft_cos::PdfString::text("app.alert('hi')"));
    doc.update_dict(root, |c| c.set(b"OpenAction".to_vec(), Object::Dict(js))).unwrap();
    crate::set_initial_view(&mut doc, &crate::InitialView { layout: Layout::SinglePage, ..Default::default() }).unwrap();
    assert!(doc.get(root).as_dict().unwrap().get(b"OpenAction").and_then(|o| o.as_dict()).is_some_and(|d| d.name(b"S") == Some(b"JavaScript")));
    assert!(crate::set_initial_view(&mut doc, &crate::InitialView { page: 9, ..Default::default() }).is_err());
}

// ── Split / Extract keep only the resources the part draws (#204) ──────────────────────────────

/// A content stream with no text operators (an image draw).
fn raw(content: &str) -> String {
    format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len())
}

fn image(width: u32, height: u32, payload: usize) -> String {
    let data = "x".repeat(payload);
    format!(
        "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceGray /BitsPerComponent 8 /Length {payload} >>\nstream\n{data}\nendstream"
    )
}

/// Document D: two pages sharing one resources dictionary with two image XObjects; each page
/// draws a different one — the shared-dictionary shape of #204.
fn doc_images() -> Document {
    let b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),                                                        // 1
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 200] /Resources 7 0 R >>".into(), // 2
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>".into(),                                          // 3
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>".into(),                                          // 4
        raw("/ImA Do"),                                                                                    // 5
        raw("/ImB Do"),                                                                                    // 6
        "<< /XObject << /ImA 8 0 R /ImB 9 0 R >> >>".into(),                                               // 7
        image(1, 1, 1),                                                                                    // 8
        image(1, 1, 2000),                                                                                 // 9
    ];
    open(build(&b, "/Root 1 0 R"))
}

fn xobject_names(doc: &Document, page: usize) -> Vec<Vec<u8>> {
    let resources = page_dict(doc, page).get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).expect("page resources");
    let xobjects = resources.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).expect("xobject dict");
    xobjects.iter().map(|(k, _)| k.clone()).collect()
}

#[test]
fn extract_keeps_only_the_images_the_part_draws() {
    let src = doc_images();
    let whole = write_full(&src, &SaveOptions::default()).expect("writes").len();
    let part = full_roundtrip(&extract_pages(&src, &[0]).unwrap());
    let names = xobject_names(&part, 0);
    assert!(names.iter().any(|n| n.as_slice() == b"ImA"), "the drawn image stays");
    assert!(!names.iter().any(|n| n.as_slice() == b"ImB"), "an image no page of the part draws must go");
    let part_bytes = write_full(&part, &SaveOptions::default()).expect("writes").len();
    assert!(part_bytes + 1000 < whole, "the part must not carry the other page's image: {part_bytes} vs {whole}");
}

#[test]
fn extract_keeps_the_union_when_the_part_has_both_pages() {
    let part = full_roundtrip(&extract_pages(&doc_images(), &[0, 1]).unwrap());
    let names = xobject_names(&part, 0);
    assert!(names.iter().any(|n| n.as_slice() == b"ImA"));
    assert!(names.iter().any(|n| n.as_slice() == b"ImB"));
}

/// Like `doc_images`, but the second page's content can't be decoded and the first page also
/// lists a font: pruning must not guess what the unreadable page draws.
fn doc_images_with(page2_content: &str, font: &str) -> Document {
    let b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 200] /Resources 7 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>".into(),
        raw("/ImA Do"),
        page2_content.into(),
        format!("<< /XObject << /ImA 8 0 R /ImB 9 0 R >> {font} >>"),
        image(1, 1, 1),
        image(1, 1, 2000),
    ];
    open(build(&b, "/Root 1 0 R"))
}

#[test]
fn extract_keeps_images_an_undecodable_page_might_draw() {
    let bogus = "<< /Length 7 /Filter /NoSuchFilter >>\nstream\n/ImB Do\nendstream";
    let part = full_roundtrip(&extract_pages(&doc_images_with(bogus, ""), &[0, 1]).unwrap());
    let names = xobject_names(&part, 0);
    assert!(names.iter().any(|n| n.as_slice() == b"ImB"), "an image the unreadable page may draw must stay");
}

#[test]
fn extract_keeps_images_type3_glyphs_may_draw() {
    // A Type 3 font without its own /Resources draws through the page's: its glyphs could `Do`.
    let font = "/Font << /T3 << /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [1 0 0 1 0 0] /CharProcs << >> /Encoding << /Differences [] >> /FirstChar 0 /LastChar 0 /Widths [0] >> >>";
    let part = full_roundtrip(&extract_pages(&doc_images_with(&raw("/ImB Do"), font), &[0]).unwrap());
    let names = xobject_names(&part, 0);
    assert!(names.iter().any(|n| n.as_slice() == b"ImB"), "pruning must not run through Type 3 resources");
}

/// A page listed twice whose widget's field keeps `/Kids` as an array object of its own: the
/// second widget joins that array, and the field keeps every widget it had.
#[test]
fn a_repeated_widget_joins_an_indirect_kids_array() {
    let b: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] >> >>".into(), // 1
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >>".into(), // 2
        "<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>".into(),                   // 3
        "<< /Type /Page /Parent 2 0 R /Annots [8 0 R] >>".into(),                   // 4
        "<< /Type /Annot /Subtype /Widget /Parent 6 0 R /Rect [10 10 90 30] /P 3 0 R >>".into(), // 5
        "<< /FT /Tx /T (name) /V (Ada) /Kids 7 0 R >>".into(),                      // 6
        "[5 0 R 8 0 R]".into(),                                                     // 7
        "<< /Type /Annot /Subtype /Widget /Parent 6 0 R /Rect [10 40 90 60] /P 4 0 R >>".into(), // 8
    ];
    let src = open(build(&b, "/Root 1 0 R"));
    let out = full_roundtrip(&extract_pages(&src, &[0, 0, 1]).unwrap());
    let form = out.resolve(catalog(&out).get(b"AcroForm").unwrap());
    let fields = form.as_dict().unwrap().get(b"Fields").and_then(Object::as_array).cloned().unwrap();
    assert_eq!(fields.len(), 1, "one field");
    let field = out.resolve(&fields[0]).as_dict().cloned().unwrap();
    let kids = out.resolve(field.get(b"Kids").unwrap()).as_array().cloned().unwrap();
    assert_eq!(kids.len(), 3, "both original widgets and the repeat's own: {kids:?}");
}
