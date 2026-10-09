//! Sanitizing inside apply, direct annotations, replies, appearance coverage and structure tags.

use std::collections::HashMap;
use std::sync::Arc;

use pdfcraft_annot::{Meta, NewAnnotation, Shape, Style, add_annotation, rect_quad};
use pdfcraft_cos::{Document, SaveOptions, write_full};

use super::*;
use crate::sanitize::{Hidden, LayerPolicy, Sanitize};

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut v = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    v.extend_from_slice(data);
    v.extend_from_slice(b"\nendstream");
    v
}

fn pdf(objs: Vec<Vec<u8>>) -> Document {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

const FONT: &str =
    "<< /Type /Font /Subtype /TrueType /BaseFont /Arial /FirstChar 32 /LastChar 126 /Widths 6 0 R /FontDescriptor << /Ascent 800 /Descent -200 >> >>";

/// Objects 1 to 6: catalog (with `cat` added), pages, one 300×300 page (with `page` added),
/// content, font, widths. `extra` starts at object 7.
fn build(cat: &str, page: &str, resources: &str, content: &[u8], extra: Vec<Vec<u8>>) -> Document {
    let widths = format!("[{}]", vec!["500"; 95].join(" ")).into_bytes();
    let mut objs: Vec<Vec<u8>> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {cat} >>").into_bytes(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> {resources} >> {page} >>")
            .into_bytes(),
        stream("", content),
        FONT.as_bytes().to_vec(),
        widths,
    ];
    objs.extend(extra);
    pdf(objs)
}

fn mark(doc: &mut Document, rect: [f64; 4]) {
    let shape = Shape::Redact { quads: vec![rect_quad(rect)], overlay: String::new(), look: Default::default() };
    let style = Style::default_for(&shape);
    add_annotation(doc, &NewAnnotation { page: 0, shape, style, contents: String::new(), author: "T".into() }, &Meta::default()).unwrap();
}

fn page_content(doc: &Document) -> String {
    let p = &pdfcraft_model::pages(doc)[0];
    let (_, data) = page_streams(doc, &p.dict, 0).unwrap();
    data.iter().map(|d| String::from_utf8_lossy(d).into_owned()).collect::<Vec<_>>().join("\n")
}

fn page_annots(doc: &Document) -> Vec<Object> {
    annots_of(doc, &pdfcraft_model::pages(doc)[0].dict)
}

/// Every string, name and decoded stream of the objects the saved file holds (the writer packs
/// objects into compressed streams, so the file's bytes can't be searched directly).
fn saved(doc: &Document) -> String {
    fn text(o: &Object, out: &mut String, depth: usize) {
        if depth > 16 {
            return;
        }
        match o {
            Object::String(s) => out.push_str(&format!("{}\n", String::from_utf8_lossy(&s.bytes))),
            Object::Name(n) => out.push_str(&format!("/{}\n", String::from_utf8_lossy(n))),
            Object::Array(a) => a.iter().for_each(|x| text(x, out, depth + 1)),
            Object::Dict(d) => d.iter().for_each(|(k, v)| {
                out.push_str(&format!("/{}\n", String::from_utf8_lossy(k)));
                text(v, out, depth + 1);
            }),
            Object::Stream(s) => {
                text(&Object::Dict(s.dict.clone()), out, depth + 1);
                out.push_str(&String::from_utf8_lossy(&s.decoded().unwrap_or_default()));
                out.push('\n');
            }
            _ => {}
        }
    }
    let bytes = write_full(doc, &SaveOptions::default()).unwrap();
    let reopened = Document::open(Arc::new(bytes)).unwrap();
    let mut out = String::new();
    for n in reopened.object_numbers() {
        text(&reopened.get(ObjRef::new(n, reopened.generation(n))), &mut out, 0);
    }
    out
}

fn contains(hay: &str, needle: &str) -> bool {
    hay.contains(needle)
}

fn counts(r: &Report) -> HashMap<Hidden, usize> {
    r.sanitized.iter().copied().collect()
}

const SECRET_TEXT: &[u8] = b"BT /F1 10 Tf 10 200 Td (SECRET) Tj ET";
const OVER_SECRET: [f64; 4] = [5.0, 195.0, 60.0, 212.0];

// ── Direct annotations, replies, appearances ────────────────────────────────────────────────

#[test]
fn a_mark_held_directly_in_annots_is_applied() {
    let mut doc = build(
        "",
        "/Annots [ << /Type /Annot /Subtype /Redact /Rect [5 195 60 212] /IC [0 0 0] >> << /Type /Annot /Subtype /Text /Rect [10 196 20 206] /Contents (about SECRET) >> ]",
        "",
        SECRET_TEXT,
        vec![],
    );
    let report = apply(&mut doc, None).unwrap();
    assert_eq!((report.marks, report.annotations), (1, 1), "the mark, and the direct comment under it");
    let c = page_content(&doc);
    assert!(!c.contains("SECRET"), "{c}");
    assert!(page_annots(&doc).is_empty());
    assert!(!contains(&saved(&doc), "about SECRET"));
}

#[test]
fn a_direct_mark_can_be_cleared() {
    let mut doc = build("", "/Annots [ << /Type /Annot /Subtype /Redact /Rect [5 195 60 212] >> ]", "", SECRET_TEXT, vec![]);
    assert_eq!(clear_marks(&mut doc, None), Ok(1));
    assert!(page_annots(&doc).is_empty());
    assert!(page_content(&doc).contains("SECRET"));
}

#[test]
fn replies_and_pop_ups_of_removed_annotations_go() {
    let mut doc = build(
        "",
        "/Annots [7 0 R 8 0 R 9 0 R 10 0 R 11 0 R]",
        "",
        SECRET_TEXT,
        vec![
            b"<< /Type /Annot /Subtype /Text /Rect [10 196 20 206] /Contents (note) /Popup 8 0 R >>".to_vec(),
            b"<< /Type /Annot /Subtype /Popup /Rect [100 100 200 150] /Parent 7 0 R >>".to_vec(),
            // A reply far from the mark, and a reply to the reply, each with text of its own.
            b"<< /Type /Annot /Subtype /Text /Rect [250 10 260 20] /IRT 7 0 R /Contents (reply SECRET1) /Popup 10 0 R >>".to_vec(),
            b"<< /Type /Annot /Subtype /Popup /Rect [100 10 200 60] /Parent 9 0 R >>".to_vec(),
            b"<< /Type /Annot /Subtype /Text /Rect [250 40 260 50] /IRT 9 0 R /Contents (nested SECRET2) >>".to_vec(),
        ],
    );
    mark(&mut doc, OVER_SECRET);
    let report = apply(&mut doc, None).unwrap();
    assert_eq!(report.annotations, 3, "the comment and its two replies (pop-ups are not counted)");
    assert!(page_annots(&doc).is_empty(), "{:?}", page_annots(&doc));
    let out = saved(&doc);
    assert!(!contains(&out, "SECRET1") && !contains(&out, "SECRET2"));
}

#[test]
fn an_annotation_whose_appearance_reaches_under_a_mark_goes() {
    // The rectangle is far away; the appearance (as written, shifted by /Matrix) is not.
    let mut doc = build(
        "",
        "/Rotate 90 /Annots [7 0 R 8 0 R 9 0 R]",
        "",
        SECRET_TEXT,
        vec![
            b"<< /Type /Annot /Subtype /Stamp /Rect [250 250 260 260] /AP << /N 10 0 R >> >>".to_vec(),
            b"<< /Type /Annot /Subtype /Square /Rect [250 10 260 20] >>".to_vec(),
            b"<< /Type /Annot /Subtype /Square /Rect [250 40 260 50] /AP << /N 11 0 R >> >>".to_vec(),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Matrix [1 0 0 1 -50 150]", b"0 0 100 100 re f"),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", b"0 0 10 10 re f"),
        ],
    );
    mark(&mut doc, OVER_SECRET);
    let report = apply(&mut doc, None).unwrap();
    assert_eq!(report.annotations, 1);
    let left: Vec<ObjRef> = page_annots(&doc).iter().filter_map(Object::as_ref).collect();
    assert_eq!(left, [ObjRef::new(8, 0), ObjRef::new(9, 0)], "only the stamp reaching under the mark went");

    // Touching is not overlapping.
    let mut doc = build(
        "",
        "/Annots [7 0 R]",
        "",
        SECRET_TEXT,
        vec![
            b"<< /Type /Annot /Subtype /Stamp /Rect [250 250 260 260] /AP << /N 8 0 R >> >>".to_vec(),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Matrix [1 0 0 1 60 100]", b"0 0 100 100 re f"),
        ],
    );
    mark(&mut doc, OVER_SECRET);
    assert_eq!(apply(&mut doc, None).unwrap().annotations, 0);
}

// ── Sanitizing as part of apply ─────────────────────────────────────────────────────────────

fn sanitize_fixture() -> Document {
    let cat = "/Metadata 7 0 R /AF [8 0 R] /Collection << /Schema << >> >> \
        /Names << /EmbeddedFiles << /Names [(a.txt) 8 0 R] >> /JavaScript << /Names [(init) 9 0 R] >> /Dests << /Names [(chap1) [3 0 R /Fit]] >> >> \
        /Dests << /old [3 0 R /Fit] >> /PageLabels << /Nums [0 << /S /D /P (SecretPrefix) >>] >> /Threads [13 0 R] \
        /OpenAction 9 0 R /AA << /WC 9 0 R >> /Outlines 10 0 R /PageMode /UseOutlines \
        /OCProperties << /OCGs [16 0 R 17 0 R] /D << /OFF [16 0 R] /AS [<< /Event /Print /Category [/Print] /OCGs [16 0 R] >>] >> >> \
        /AcroForm << /Fields [] /XFA [(template) 18 0 R] >>";
    let page = "/Annots [19 0 R 20 0 R 21 0 R 22 0 R 23 0 R] /Thumb 24 0 R /PieceInfo << /App << /P (page private) >> >> /AA << /O 9 0 R >>";
    let mut doc = build(
        cat,
        page,
        "/XObject << /Fm1 25 0 R >>",
        b"BT /F1 10 Tf 10 200 Td (SECRET) Tj ET /Fm1 Do",
        vec![
            stream("/Type /Metadata /Subtype /XML", b"<x:xmpmeta>TopSecretXmp</x:xmpmeta>"),
            b"<< /Type /Filespec /F (a.txt) /EF << /F 13 0 R >> >>".to_vec(),
            b"<< /S /JavaScript /JS (app.alert(1)) >>".to_vec(),
            b"<< /Type /Outlines /First 11 0 R /Last 12 0 R /Count 2 >>".to_vec(),
            b"<< /Title (OutlineSecret) /Parent 10 0 R /Next 12 0 R >>".to_vec(),
            b"<< /Title (Two) /Parent 10 0 R /Prev 11 0 R >>".to_vec(),
            stream("", b"attached"),
            b"<< /F 15 0 R >>".to_vec(),
            stream("", b"unused"),
            b"<< /Type /OCG /Name (Secret layer) >>".to_vec(),
            b"<< /Type /OCG /Name (Shown layer) >>".to_vec(),
            stream("", b"<xfa>XfaSecret</xfa>"),
            // 19 comment, 20 link with a web address, 21 link inside the document, 22 movie, 23 attachment
            b"<< /Type /Annot /Subtype /Text /Rect [100 100 110 110] /Contents (CommentSecret) /RC (<p>RichSecret</p>) >>".to_vec(),
            b"<< /Type /Annot /Subtype /Link /Rect [100 130 150 140] /A << /S /URI /URI (https://example.org) >> >>".to_vec(),
            b"<< /Type /Annot /Subtype /Link /Rect [100 150 150 160] /A << /S /GoTo /D [3 0 R /Fit] >> >>".to_vec(),
            b"<< /Type /Annot /Subtype /Movie /Rect [100 170 150 180] /Movie << /F (m.mov) >> >>".to_vec(),
            b"<< /Type /Annot /Subtype /FileAttachment /Rect [100 190 110 200] /FS 8 0 R >>".to_vec(),
            stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"x"),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Metadata 26 0 R /PieceInfo << /App << /P (form private) >> >>", b"0 0 1 1 re f"),
            stream("/Type /Metadata /Subtype /XML", b"<x:xmpmeta>FormXmp</x:xmpmeta>"),
        ],
    );
    let info = doc.add(Object::Dict({
        let mut d = Dict::new();
        d.set(b"Title".to_vec(), pdfcraft_cos::PdfString::text("Plan"));
        d.set(b"Author".to_vec(), pdfcraft_cos::PdfString::text("TopSecretAuthor"));
        d
    }));
    doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(info));
    mark(&mut doc, OVER_SECRET);
    doc
}

#[test]
fn recommended_sanitizing_runs_inside_apply_with_exact_counts() {
    let mut doc = sanitize_fixture();
    let report = apply_with(&mut doc, None, &ApplyOptions::default()).unwrap();
    let c = counts(&report);
    // Info entries, the catalog's and a form's XMP stream.
    assert_eq!(c[&Hidden::Metadata], 2 + 1 + 1);
    // The embedded file, the associated file of the catalog and the attachment annotation.
    assert_eq!(c[&Hidden::Attachments], 3);
    // The portfolio and the private data of a form (the page's thumbnail and private data are
    // dropped with the redacted page, before the sanitize pass, and not counted here).
    assert_eq!(c[&Hidden::PrivateData], 2);
    // Outline items, old and named destinations, page labels, threads.
    assert_eq!(c[&Hidden::Bookmarks], 2 + 1 + 1 + 1 + 1);
    // Open action, document trigger, script, page trigger, and the link to the web.
    assert_eq!(c[&Hidden::LinksActionsScripts], 5);
    // The comment's text, the movie.
    assert_eq!(c[&Hidden::Comments], 2);
    assert_eq!(c[&Hidden::FormFields], 1, "the XFA data");
    assert_eq!(c[&Hidden::HiddenLayers], 1);
    assert_eq!(report.layers, ["Secret layer"]);
    assert!(doc.full_save_required());

    let out = saved(&doc);
    for s in [
        "TopSecretAuthor",
        "TopSecretXmp",
        "FormXmp",
        "OutlineSecret",
        "SecretPrefix",
        "CommentSecret",
        "RichSecret",
        "XfaSecret",
        "app.alert",
        "page private",
        "form private",
        "attached",
        "SECRET",
        "example.org",
    ] {
        assert!(!contains(&out, s), "{s} is still in the file");
    }
    // What the reader sees stays: the comment (without its text), the link inside the document.
    let kept: Vec<Vec<u8>> =
        page_annots(&doc).iter().filter_map(|a| doc.resolve(a).as_dict().and_then(|d| d.name(b"Subtype").map(<[u8]>::to_vec))).collect();
    assert_eq!(kept, [b"Text".to_vec(), b"Link".to_vec(), b"Link".to_vec()]);
    let links: Vec<Dict> = page_annots(&doc).iter().filter_map(|a| doc.dict(a)).filter(|d| d.name(b"Subtype") == Some(b"Link")).collect();
    assert!(!links[0].contains(b"A") && links[1].contains(b"A"), "only plain navigation stays");
    // Layers: the hidden one stays hidden, and no usage rule can show it.
    let cat = doc.get(doc.root().unwrap()).as_dict().cloned().unwrap();
    let oc = doc.dict(cat.get(b"OCProperties").unwrap()).unwrap();
    let d = doc.dict(oc.get(b"D").unwrap()).unwrap();
    assert_eq!(d.get(b"OFF"), Some(&Object::Array(vec![Object::Ref(ObjRef::new(16, 0))])));
    assert!(!d.contains(b"AS"));
    for k in [&b"Outlines"[..], b"OpenAction", b"AA", b"Metadata", b"AF", b"Collection", b"Dests", b"PageLabels", b"Threads"] {
        assert!(!cat.contains(k), "{}", String::from_utf8_lossy(k));
    }
    assert!(doc.trailer().get(b"Info").is_none());
}

#[test]
fn apply_without_sanitizing_leaves_the_rest_alone() {
    let mut doc = sanitize_fixture();
    // XFA data holds values redaction can't reach: refused, and nothing changed.
    let err = apply(&mut doc, None).unwrap_err();
    assert_eq!(err, RedactError::Xfa);
    assert!(!err.to_string().contains("SECRET"));
    assert!(page_content(&doc).contains("SECRET"));
    // Remove just that.
    let opts = ApplyOptions { sanitize: Sanitize::Only(vec![Hidden::FormFields]), ..ApplyOptions::default() };
    let mut doc2 = sanitize_fixture();
    let report = apply_with(&mut doc2, None, &opts).unwrap();
    assert!(report.sanitized.iter().all(|(h, _)| *h == Hidden::FormFields), "{:?}", report.sanitized);
    let out = saved(&doc2);
    assert!(contains(&out, "CommentSecret") && !contains(&out, "XfaSecret"));
    // Without XFA, `apply` keeps working as before.
    let mut plain = build("/Metadata 7 0 R", "", "", SECRET_TEXT, vec![stream("/Type /Metadata", b"keepme")]);
    mark(&mut plain, OVER_SECRET);
    let report = apply(&mut plain, None).unwrap();
    assert!(report.sanitized.is_empty());
    assert!(contains(&saved(&plain), "keepme"));
}

#[test]
fn the_fields_of_a_removed_widget_lose_their_values_up_the_parent_chain() {
    let mut doc = build(
        "/AcroForm << /Fields [7 0 R] >>",
        "/Annots [8 0 R 9 0 R]",
        "",
        b"",
        vec![
            b"<< /T (p) /FT /Tx /V (ParentSecret) /DV (DefaultSecret) /TU (TipSecret) /Kids [8 0 R 9 0 R] >>".to_vec(),
            b"<< /Type /Annot /Subtype /Widget /Parent 7 0 R /T (a) /V (WidgetSecret) /TU (WidgetTip) /Rect [10 196 50 206] >>".to_vec(),
            b"<< /Type /Annot /Subtype /Widget /Parent 7 0 R /T (b) /V (SiblingSecret) /Rect [10 100 50 110] >>".to_vec(),
        ],
    );
    mark(&mut doc, OVER_SECRET);
    let report = apply_with(&mut doc, None, &ApplyOptions::default()).unwrap();
    assert_eq!(report.fields, 1);
    // The widget's own entries, and the parent's.
    assert_eq!(counts(&report)[&Hidden::FormFields], 2);
    let out = saved(&doc);
    for s in ["ParentSecret", "DefaultSecret", "TipSecret", "WidgetSecret", "WidgetTip"] {
        assert!(!contains(&out, s), "{s}");
    }
    assert_eq!(pdfcraft_forms::fields(&doc).len(), 1, "the other field stays");
}

#[test]
fn layers_can_all_be_switched_off() {
    let cat = "/OCProperties << /OCGs [7 0 R 8 0 R] /D << /ON [7 0 R 8 0 R] >> >>";
    let layers = || vec![b"<< /Type /OCG /Name (Shown) >>".to_vec(), b"<< /Type /OCG /Name (Other) >>".to_vec()];
    let mut doc = build(cat, "", "", SECRET_TEXT, layers());
    mark(&mut doc, OVER_SECRET);
    let report = apply_with(&mut doc, None, &ApplyOptions::default()).unwrap();
    assert!(report.layers.is_empty(), "nothing was hidden, nothing changes");
    let mut doc = build(cat, "", "", SECRET_TEXT, layers());
    mark(&mut doc, OVER_SECRET);
    let report = apply_with(&mut doc, None, &ApplyOptions { layers: LayerPolicy::ForceOff, ..ApplyOptions::default() }).unwrap();
    assert_eq!(report.layers, ["Shown", "Other"]);
    assert_eq!(counts(&report)[&Hidden::HiddenLayers], 2);
    let cat = doc.get(doc.root().unwrap()).as_dict().cloned().unwrap();
    let d = doc.dict(&doc.dict(cat.get(b"OCProperties").unwrap()).unwrap().get(b"D").cloned().unwrap()).unwrap();
    assert_eq!(d.get(b"ON"), Some(&Object::Array(vec![])));
    assert_eq!(d.name(b"BaseState"), Some(&b"OFF"[..]));
    assert_eq!(d.get(b"OFF").and_then(Object::as_array).map(Vec::len), Some(2));
}

#[test]
fn what_is_counted_is_what_is_removed() {
    let doc = sanitize_fixture();
    let mut scanned: Vec<(Hidden, usize)> = sanitize::scan(&doc).into_iter().filter(|c| c.1 > 0).collect();
    let mut work = doc.clone();
    let mut removed = sanitize::sanitize(&mut work).unwrap();
    scanned.sort_by_key(|c| c.0.id());
    removed.sort_by_key(|c| c.0.id());
    assert_eq!(scanned, removed);
    // And the plan did not touch what it only looked at.
    assert!(!doc.full_save_required());
    assert!(doc.trailer().get(b"Info").is_some());
}

#[test]
fn sanitizing_a_clean_document_does_not_force_a_rewrite() {
    let mut doc = build("", "", "", b"BT ET", vec![]);
    assert_eq!(sanitize::sanitize(&mut doc), Ok(Vec::new()));
    assert!(!doc.full_save_required());
    assert!(!doc.is_modified());
}

#[test]
fn hidden_categories_are_selectable() {
    let mut doc = sanitize_fixture();
    let done = sanitize::remove_hidden(&mut doc, &[Hidden::Bookmarks, Hidden::PrivateData]).unwrap();
    assert_eq!(done, [(Hidden::Bookmarks, 6), (Hidden::PrivateData, 4)]);
    let out = saved(&doc);
    assert!(contains(&out, "CommentSecret") && !contains(&out, "OutlineSecret") && !contains(&out, "SecretPrefix"));
}

// ── Structure tags ──────────────────────────────────────────────────────────────────────────

#[test]
fn tags_of_rewritten_forms_removed_annotations_and_ancestors_are_cleaned() {
    let mut doc = build(
        "/StructTreeRoot 8 0 R",
        "/StructParents 0 /Annots [14 0 R]",
        "/XObject << /Fm1 7 0 R >>",
        b"/Fm1 Do",
        vec![
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /StructParents 1 /Resources << /Font << /F1 5 0 R >> >>",
                b"/P << /MCID 0 >> BDC BT /F1 10 Tf 10 200 Td (SECRET) Tj ET EMC",
            ),
            // 8 root (its kids behind a reference), 9 array, 10 section, 11 paragraph, 12 link, 13 parent tree
            b"<< /Type /StructTreeRoot /K 9 0 R /ParentTree 13 0 R >>".to_vec(),
            b"[10 0 R]".to_vec(),
            b"<< /S /Sect /Pg 3 0 R /ActualText (SECRET summary) /K [11 0 R 12 0 R] >>".to_vec(),
            b"<< /S /P /Pg 3 0 R /Alt (secret) /K << /Type /MCR /MCID 0 /Pg 3 0 R /Stm 7 0 R >> >>".to_vec(),
            b"<< /S /Link /Pg 3 0 R /Alt (link tip) /K << /Type /OBJR /Obj 14 0 R /Pg 3 0 R >> >>".to_vec(),
            b"<< /Nums [0 [11 0 R] 1 [11 0 R] 2 12 0 R] >>".to_vec(),
            b"<< /Type /Annot /Subtype /Text /Rect [10 196 20 206] /StructParent 2 /Contents (about SECRET) >>".to_vec(),
        ],
    );
    mark(&mut doc, OVER_SECRET);
    let report = apply(&mut doc, None).unwrap();
    let get = |n: u32| doc.get(ObjRef::new(n, 0)).as_dict().cloned().unwrap();
    let (sect, para, link) = (get(10), get(11), get(12));
    assert!(!sect.contains(b"ActualText"), "an ancestor's text stands for what is below");
    assert!(!para.contains(b"Alt") && !para.contains(b"K"));
    assert!(!link.contains(b"Alt") && !link.contains(b"K"));
    assert!(sect.contains(b"K"), "the structure itself stays");
    // The parent tree keeps only the page's entry.
    assert_eq!(get(13).get(b"Nums"), Some(&Object::Array(vec![Object::Int(0), Object::Array(vec![Object::Ref(ObjRef::new(11, 0))])])));
    assert_eq!(report.tags, 3, "the paragraph, the link and the section above them");
    assert!(!contains(&saved(&doc), "SECRET"));
}

#[test]
fn page_entries_of_the_parent_tree_forget_emptied_content() {
    let content = b"/P <</MCID 0>> BDC BT /F1 10 Tf 10 200 Td (SECRET) Tj ET EMC /P <</MCID 1>> BDC BT /F1 10 Tf 10 100 Td (Public) Tj ET EMC";
    let mut doc = build(
        "/StructTreeRoot 7 0 R",
        "/StructParents 0",
        "",
        content,
        vec![
            b"<< /Type /StructTreeRoot /K 8 0 R /ParentTree << /Nums [0 [8 0 R 9 0 R]] >> >>".to_vec(),
            b"<< /S /Document /K [9 0 R 10 0 R] >>".to_vec(),
            b"<< /S /P /Pg 3 0 R /K 0 >>".to_vec(),
            b"<< /S /P /Pg 3 0 R /K 1 >>".to_vec(),
        ],
    );
    mark(&mut doc, OVER_SECRET);
    apply(&mut doc, None).unwrap();
    let root = doc.get(ObjRef::new(7, 0)).as_dict().cloned().unwrap();
    let tree = root.get(b"ParentTree").and_then(Object::as_dict).cloned().unwrap();
    let nums = tree.get(b"Nums").and_then(Object::as_array).cloned().unwrap();
    assert_eq!(nums[1], Object::Array(vec![Object::Null, Object::Ref(ObjRef::new(9, 0))]));
}

#[test]
fn rewritten_forms_do_not_stay_in_the_resources_with_their_old_content() {
    let mut doc = build(
        "",
        "",
        "/XObject << /Fm1 7 0 R /Fm2 8 0 R >>",
        b"/Fm1 Do /Fm2 Do",
        vec![
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >>",
                b"BT /F1 10 Tf 10 200 Td (SECRET) Tj ET",
            ),
            stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << /Font << /F1 5 0 R >> >>",
                b"BT /F1 10 Tf 10 100 Td (Other) Tj ET",
            ),
        ],
    );
    mark(&mut doc, OVER_SECRET);
    apply(&mut doc, None).unwrap();
    let out = saved(&doc);
    assert!(!contains(&out, "SECRET"));
    assert!(contains(&out, "Other"), "a form that was not touched stays");
}

// ── Page leaks, new identifiers, fail-closed caps ───────────────────────────────────────────

#[test]
fn drop_page_leaks_removes_thumbnails_and_private_data_of_the_given_pages_only() {
    let page = "/Thumb 7 0 R /PieceInfo << /App << /P (page private) >> >>";
    let mut doc = build("", page, "", b"BT ET", vec![stream("/Width 1 /Height 1", b"x")]);
    // Nothing named: nothing touched.
    assert_eq!(sanitize::drop_page_leaks(&mut doc, &[]), Ok(0));
    assert!(pdfcraft_model::pages(&doc)[0].dict.contains(b"Thumb"));
    assert_eq!(sanitize::drop_page_leaks(&mut doc, &[0]), Ok(2));
    let dict = pdfcraft_model::pages(&doc)[0].dict.clone();
    assert!(!dict.contains(b"Thumb") && !dict.contains(b"PieceInfo"));
    assert!(dict.contains(b"Contents"), "the rest of the page stays");
    // A page that isn't there is an error, not a panic.
    assert_eq!(sanitize::drop_page_leaks(&mut doc, &[5]), Err(RedactError::Unreadable(6)));
    assert_eq!(sanitize::drop_page_leaks(&mut doc, &[usize::MAX]), Err(RedactError::Unreadable(usize::MAX)));
}

#[test]
fn apply_drops_the_thumbnail_and_private_data_of_redacted_pages() {
    let page = "/Thumb 7 0 R /PieceInfo << /App << /P (page private) >> >>";
    let mut doc = build("", page, "", SECRET_TEXT, vec![stream("/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"THUMBPIXELS")]);
    mark(&mut doc, OVER_SECRET);
    // Plain `apply` doesn't sanitize, and still leaves no thumbnail behind.
    apply(&mut doc, None).unwrap();
    let dict = pdfcraft_model::pages(&doc)[0].dict.clone();
    assert!(!dict.contains(b"Thumb") && !dict.contains(b"PieceInfo"));
    assert!(!contains(&saved(&doc), "page private"));
    assert!(!contains(&saved(&doc), "THUMBPIXELS"));
}

#[test]
fn standalone_sanitizing_gives_the_rewritten_file_a_new_identifier() {
    let mut doc = build("/Metadata 7 0 R", "", "", b"BT ET", vec![stream("/Type /Metadata", b"<x/>")]);
    assert!(!doc.new_id_required());
    sanitize::sanitize(&mut doc).unwrap();
    assert!(doc.full_save_required() && doc.new_id_required());
}

#[test]
fn nested_metadata_streams_are_found_or_the_pass_fails() {
    // /Metadata under 20 directly nested dictionaries is still removed.
    let nest = |n: usize| format!("{}/Metadata 7 0 R{}", "<< /X ".repeat(n), " >>".repeat(n));
    let mut doc = build(&format!("/Deep {}", nest(20)), "", "", b"BT ET", vec![stream("/Type /Metadata", b"DeepXmp")]);
    sanitize::sanitize(&mut doc).unwrap();
    assert!(!contains(&saved(&doc), "DeepXmp"));
    // Past the depth cap the dictionaries can't be searched: the pass fails and changes nothing.
    let mut doc = build(&format!("/Deep {}", nest(80)), "", "", b"BT ET", vec![stream("/Type /Metadata", b"DeepXmp")]);
    assert_eq!(sanitize::sanitize(&mut doc), Err(RedactError::StructureTooLarge));
    assert!(!doc.full_save_required());
}

#[test]
fn a_key_walk_that_spends_its_work_budget_fails_closed() {
    // One object holding more containers than a walk may look into — all directly nested, far
    // inside the depth cap. The work budget stops it, like the depth cap would.
    let wide = format!("[{}]", "[]".repeat(1_100_000));
    let mut doc = build("", "", "", b"BT ET", vec![wide.into_bytes()]);
    assert_eq!(sanitize::sanitize(&mut doc), Err(RedactError::StructureTooLarge));
    assert!(!doc.full_save_required(), "nothing was changed");
}

#[test]
fn a_tree_that_fans_out_through_shared_kids_fails_instead_of_hanging() {
    // 64 number-tree nodes, each /Kids naming the next node twice: counting them would take 2^63
    // paths, every one of them under the depth cap. The work budget stops the walk after a
    // bounded number of steps, and the pass fails instead of hanging.
    let n: u32 = 64;
    let nodes: Vec<Vec<u8>> = (0..n)
        .map(|i| if i + 1 < n { format!("<< /Kids [{} 0 R {} 0 R] >>", 7 + i + 1, 7 + i + 1).into_bytes() } else { b"<< /Nums [] >>".to_vec() })
        .collect();
    let mut doc = build("/PageLabels 7 0 R", "", "", b"BT ET", nodes);
    assert_eq!(sanitize::sanitize(&mut doc), Err(RedactError::StructureTooLarge));
    assert!(!doc.full_save_required(), "nothing was changed");
}

#[test]
fn a_graph_of_objects_pointing_at_each_other_is_walked_once_not_spun_on() {
    // 64 objects, each holding two indirect references to the next. A key walk visits every
    // object exactly once (references are leaves: each object is entered from the loop over the
    // objects, not through the references), so this file sanitizes fine — and the work budget
    // would turn a walker that ever follows the references into an error, not a hang.
    let objs: Vec<Vec<u8>> = (0..64)
        .map(|i| if i + 1 < 64 { format!("<< /A {} 0 R /B {} 0 R >>", 7 + i + 1, 7 + i + 1).into_bytes() } else { b"<< /End () >>".to_vec() })
        .collect();
    let mut doc = build("", "", "", b"BT ET", objs);
    assert_eq!(sanitize::sanitize(&mut doc), Ok(Vec::new()));
}

#[test]
fn structure_elements_below_the_depth_cap_are_cleaned_whole() {
    // A chain of 70 elements, each with alternate text; the last one is far below the cap.
    let n: u32 = 70;
    // Objects 7 to 76 are the elements.
    let elems = (0..n)
        .map(|i| {
            let kid = if i + 1 < n { format!("/K [{} 0 R]", 7 + i + 1) } else { "/K 0".to_string() };
            format!("<< /S /Sect /Pg 3 0 R /Alt (deep secret) {kid} >>").into_bytes()
        })
        .collect();
    let content = b"/P <</MCID 0>> BDC BT /F1 10 Tf 10 200 Td (SECRET) Tj ET EMC";
    let mut doc =
        build("/StructTreeRoot << /Type /StructTreeRoot /K 7 0 R /ParentTree << /Nums [0 [7 0 R]] >> >>", "/StructParents 0", "", content, elems);
    mark(&mut doc, OVER_SECRET);
    apply(&mut doc, None).unwrap();
    for obj in [7, 7 + n / 2, 6 + n] {
        let d = doc.get(ObjRef::new(obj, 0)).as_dict().cloned().unwrap();
        assert!(!d.contains(b"Alt"), "object {obj} kept its alternate text");
    }
    assert!(!contains(&saved(&doc), "deep secret"));
}

#[test]
fn a_direct_structure_tree_root_gets_its_parent_tree_fixed() {
    let content = b"/P <</MCID 0>> BDC BT /F1 10 Tf 10 200 Td (SECRET) Tj ET EMC /P <</MCID 1>> BDC BT /F1 10 Tf 10 100 Td (Public) Tj ET EMC";
    let mut doc = build(
        "/StructTreeRoot << /Type /StructTreeRoot /K 7 0 R /ParentTree << /Nums [0 [8 0 R 9 0 R]] >> >>",
        "/StructParents 0",
        "",
        content,
        vec![b"<< /S /Document /K [8 0 R 9 0 R] >>".to_vec(), b"<< /S /P /Pg 3 0 R /K 0 >>".to_vec(), b"<< /S /P /Pg 3 0 R /K 1 >>".to_vec()],
    );
    mark(&mut doc, OVER_SECRET);
    apply(&mut doc, None).unwrap();
    let cat = doc.root().and_then(|r| doc.get(r).as_dict().cloned()).unwrap();
    let root = cat.get(b"StructTreeRoot").and_then(Object::as_dict).cloned().unwrap();
    let nums = root.get(b"ParentTree").and_then(Object::as_dict).and_then(|t| t.get(b"Nums")).and_then(Object::as_array).cloned().unwrap();
    assert_eq!(nums.get(1), Some(&Object::Array(vec![Object::Null, Object::Ref(ObjRef::new(9, 0))])));
}

#[test]
fn a_parent_tree_nested_too_deeply_fails_instead_of_being_half_cleaned() {
    // Objects 8 to 77 are nodes of the number tree, each the only kid of the one before.
    let n: u32 = 70;
    let mut extra = vec![b"<< /S /P /Pg 3 0 R /K 0 >>".to_vec()];
    for i in 0..n {
        extra.push(if i + 1 < n { format!("<< /Kids [{} 0 R] >>", 9 + i).into_bytes() } else { b"<< /Nums [0 []] >>".to_vec() });
    }
    let content = b"/P <</MCID 0>> BDC BT /F1 10 Tf 10 200 Td (SECRET) Tj ET EMC";
    let mut doc = build("/StructTreeRoot << /Type /StructTreeRoot /K 7 0 R /ParentTree 8 0 R >>", "/StructParents 0", "", content, extra);
    mark(&mut doc, OVER_SECRET);
    let err = apply(&mut doc, None).unwrap_err();
    assert_eq!(err, RedactError::StructureTooLarge);
    assert!(!err.to_string().contains("SECRET"));
}
