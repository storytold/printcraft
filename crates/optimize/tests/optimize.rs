//! The optimizer on documents with images drawn at known sizes.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};
use pdfcraft_optimize::{Compression, ImageSettings, Settings, SpaceCategory, audit_space, effective_resolutions, optimize};

fn image(doc: &mut Document, w: u32, h: u32, n: usize, jpeg: bool, smask: Option<ObjRef>) -> ObjRef {
    // A smooth gradient with a little texture (photo-like).
    let mut px = Vec::with_capacity((w * h) as usize * n);
    for y in 0..h {
        for x in 0..w {
            let v = ((x * 255 / w.max(1)) as u8).wrapping_add(((x ^ y) & 7) as u8);
            px.push(v);
            if n == 3 {
                px.push((y * 255 / h.max(1)) as u8);
                px.push(128);
            }
        }
    }
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Image"));
    d.set(b"Width".to_vec(), Object::Int(w as i64));
    d.set(b"Height".to_vec(), Object::Int(h as i64));
    d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
    d.set(b"ColorSpace".to_vec(), Object::name(if n == 1 { "DeviceGray" } else { "DeviceRGB" }));
    if let Some(m) = smask {
        d.set(b"SMask".to_vec(), Object::Ref(m));
    }
    let s = if jpeg {
        let mut out = Vec::new();
        let ty = if n == 1 { image::ExtendedColorType::L8 } else { image::ExtendedColorType::Rgb8 };
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 98).encode(&px, w, h, ty).unwrap();
        d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
        Stream::from_raw(d, out)
    } else {
        Stream::flate(d, &px)
    };
    doc.add(Object::Stream(s))
}

fn page(doc: &mut Document, xobjects: &[(&str, ObjRef)], content: &str) -> ObjRef {
    let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
    let mut x = Dict::new();
    for (n, r) in xobjects {
        x.set(n.as_bytes().to_vec(), Object::Ref(*r));
    }
    let mut res = Dict::new();
    res.set(b"XObject".to_vec(), Object::Dict(x));
    let c = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content.as_bytes().to_vec())));
    let mut p = Dict::new();
    p.set(b"Type".to_vec(), Object::name("Page"));
    p.set(b"Parent".to_vec(), Object::Ref(pages));
    p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]));
    p.set(b"Resources".to_vec(), Object::Dict(res));
    p.set(b"Contents".to_vec(), Object::Ref(c));
    p.set(b"Thumb".to_vec(), Object::Ref(c));
    let r = doc.add(Object::Dict(p));
    doc.update_dict(pages, |d| {
        let mut kids = d.get(b"Kids").and_then(|k| k.as_array().cloned()).unwrap_or_default();
        kids.push(Object::Ref(r));
        d.set(b"Count".to_vec(), Object::Int(kids.len() as i64));
        d.set(b"Kids".to_vec(), Object::Array(kids));
    })
    .unwrap();
    r
}

fn stream(doc: &Document, r: ObjRef) -> Stream {
    match &*doc.get(r) {
        Object::Stream(s) => s.clone(),
        _ => panic!("not a stream"),
    }
}

#[test]
fn images_are_measured_where_drawn_and_downsampled() {
    let mut doc = Document::new_empty();
    // 1200 px drawn 144 pt (2 in) wide → 600 ppi.
    let big = image(&mut doc, 1200, 1200, 3, false, None);
    // 100 px at 144 pt → 50 ppi: left at its size.
    let small = image(&mut doc, 100, 100, 1, false, None);
    // With a soft mask, 800 px at 72 pt → 800 ppi.
    let mask = image(&mut doc, 800, 800, 1, false, None);
    let masked = image(&mut doc, 800, 800, 3, false, Some(mask));
    // A high-quality JPEG at 72 ppi: recompressed, not resized.
    let photo = image(&mut doc, 600, 400, 3, true, None);
    // Drawn twice: 300 ppi and 75 ppi → effective 75 (never downsampled below what a use needs).
    let twice = image(&mut doc, 300, 300, 3, false, None);
    page(&mut doc, &[("Im1", big), ("Im2", small)], "q 144 0 0 144 36 600 cm /Im1 Do Q q 144 0 0 144 300 600 cm /Im2 Do Q");
    page(
        &mut doc,
        &[("A", masked), ("B", photo), ("C", twice)],
        "q 72 0 0 72 36 36 cm /A Do Q q 1 0 0 1 100 100 cm 600 0 0 400 0 0 cm /B Do Q q 72 0 0 72 0 0 cm /C Do Q q 288 0 0 288 300 300 cm /C Do Q",
    );
    let pages = pdfcraft_annot::page_refs(&doc).unwrap();
    let ppi = effective_resolutions(&doc, &pages);
    assert_eq!(ppi[&big].round(), 600.0);
    assert_eq!(ppi[&small].round(), 50.0);
    assert_eq!(ppi[&masked].round(), 800.0);
    assert_eq!(ppi[&photo].round(), 72.0);
    assert_eq!(ppi[&twice].round(), 75.0, "the largest use decides");

    let before = write_full(&doc, &SaveOptions::default()).unwrap().len();
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(report.images, 5, "{report:?}");
    assert_eq!(report.images_resampled, 2, "{report:?}");
    assert!(report.thumbnails == 2);
    let b = stream(&doc, big);
    assert_eq!((b.dict.int(b"Width"), b.dict.int(b"Height"), b.dict.name(b"Filter")), (Some(300), Some(300), Some(&b"DCTDecode"[..])));
    let m = stream(&doc, masked);
    let sm = stream(&doc, mask);
    assert_eq!((m.dict.int(b"Width"), sm.dict.int(b"Width")), (Some(150), Some(150)), "the soft mask follows its image");
    assert_eq!(sm.dict.name(b"Filter"), Some(&b"FlateDecode"[..]));
    assert_eq!(stream(&doc, small).dict.int(b"Width"), Some(100));
    assert_eq!(stream(&doc, twice).dict.int(b"Width"), Some(300), "75 ppi is below the threshold");
    let p = stream(&doc, photo);
    assert_eq!(p.dict.int(b"Width"), Some(600));
    let after_bytes = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(after_bytes.len() * 3 < before, "{} → {}", before, after_bytes.len());

    // The optimized file opens, and the page still shows the picture where it was.
    let reopened = Document::open(Arc::new(after_bytes.clone())).unwrap();
    assert_eq!(pdfcraft_annot::page_refs(&reopened).unwrap().len(), 2);
    let mut r = pdfcraft_render::PageRenderer::new(Arc::new(after_bytes), pdfcraft_render::RenderConfig::default());
    let out = r.render(pdfcraft_render::RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    // Top-left image area (36..180 x 600..744 user → y from the top 48..192): drawn, not white.
    let i = ((100 * out.width + 100) * 4) as usize;
    assert_ne!(&out.rgba[i..i + 3], &[255, 255, 255]);
}

#[test]
fn an_identity_decode_array_does_not_stop_reduce() {
    // Office scanners write /Decode [0 1 0 1 0 1] (no change) on their page images (#490).
    let with_decode = |doc: &mut Document, r: ObjRef, decode: Vec<Object>| {
        let mut s = stream(doc, r);
        s.dict.set(b"Decode".to_vec(), Object::Array(decode));
        doc.set(r, Object::Stream(s));
    };
    let mut doc = Document::new_empty();
    // 900 px drawn 216 pt (3 in) wide → 300 ppi, like a scanned page.
    let scan = image(&mut doc, 900, 900, 3, true, None);
    with_decode(&mut doc, scan, [0, 1, 0, 1, 0, 1].map(Object::Int).to_vec());
    let gray = image(&mut doc, 900, 900, 1, false, None);
    with_decode(&mut doc, gray, vec![Object::Real(0.0), Object::Real(1.0)]);
    // A decode array that inverts the colours, and one of the wrong length: left alone.
    let inverted = image(&mut doc, 900, 900, 3, true, None);
    with_decode(&mut doc, inverted, [1, 0, 1, 0, 1, 0].map(Object::Int).to_vec());
    let odd = image(&mut doc, 900, 900, 3, true, None);
    with_decode(&mut doc, odd, [0, 1].map(Object::Int).to_vec());
    page(
        &mut doc,
        &[("S", scan), ("G", gray), ("I", inverted), ("O", odd)],
        "q 216 0 0 216 0 0 cm /S Do Q q 216 0 0 216 216 0 cm /G Do Q q 216 0 0 216 0 300 cm /I Do Q q 216 0 0 216 216 300 cm /O Do Q",
    );
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(report.images_resampled, 2, "{report:?}");
    assert_eq!(stream(&doc, scan).dict.int(b"Width"), Some(450));
    assert_eq!(stream(&doc, gray).dict.int(b"Width"), Some(450));
    assert!(!stream(&doc, scan).dict.contains(b"Decode"), "the default decode array isn't carried over");
    assert_eq!(stream(&doc, inverted).dict.int(b"Width"), Some(900));
    assert_eq!(stream(&doc, odd).dict.int(b"Width"), Some(900));
}

#[test]
fn settings_choose_what_happens() {
    let mut doc = Document::new_empty();
    let big = image(&mut doc, 1200, 600, 1, false, None);
    page(&mut doc, &[("Im1", big)], "q 144 0 0 72 36 600 cm /Im1 Do Q");
    let mut keep = doc.clone();
    // Off: nothing changes (but the thumbnail goes).
    let off = ImageSettings { downsample: false, target_ppi: 150.0, above_ppi: 225.0, compression: Compression::Retain };
    let r = optimize(&mut keep, &Settings { color: off, gray: off, ..Settings::default() }).unwrap();
    assert_eq!((r.images_resampled, r.images_recompressed), (0, 0));
    // Lossless: resampled to 300 ppi, Flate.
    let lossless = ImageSettings { downsample: true, target_ppi: 300.0, above_ppi: 450.0, compression: Compression::Flate };
    let r = optimize(&mut doc, &Settings { gray: lossless, ..Settings::default() }).unwrap();
    assert_eq!(r.images_resampled, 1);
    let s = stream(&doc, big);
    assert_eq!((s.dict.int(b"Width"), s.dict.int(b"Height"), s.dict.name(b"Filter")), (Some(600), Some(300), Some(&b"FlateDecode"[..])));
}

#[test]
fn discards_and_clean_up() {
    let mut doc = Document::new_empty();
    let pages = page(&mut doc, &[], "BT /F1 12 Tf 72 720 Td (Hello world, hello world, hello world, hello world) Tj ET");
    let root = doc.root().unwrap();
    let mut vp = Dict::new();
    vp.set(b"PrintScaling".to_vec(), Object::name("None"));
    vp.set(b"Duplex".to_vec(), Object::name("Simplex"));
    vp.set(b"HideToolbar".to_vec(), Object::Bool(true));
    let st = doc.add(Object::Dict(Dict::new()));
    doc.update_dict(root, |c| {
        c.set(b"ViewerPreferences".to_vec(), Object::Dict(vp));
        c.set(b"StructTreeRoot".to_vec(), Object::Ref(st));
        c.set(b"MarkInfo".to_vec(), Object::Dict(Dict::new()));
    })
    .unwrap();
    doc.update_dict(pages, |p| p.set(b"StructParents".to_vec(), Object::Int(0))).unwrap();
    let settings = Settings { discard_tags: true, discard_print_settings: true, ..Settings::default() };
    let r = optimize(&mut doc, &settings).unwrap();
    assert!(r.tags_removed);
    assert_eq!((r.print_settings, r.thumbnails, r.streams_compressed), (2, 1, 1));
    let c = doc.get(root).as_dict().cloned().unwrap();
    assert!(!c.contains(b"StructTreeRoot") && !c.contains(b"MarkInfo"));
    let vp = c.get(b"ViewerPreferences").and_then(|v| v.as_dict().cloned()).unwrap();
    assert!(vp.contains(b"HideToolbar") && !vp.contains(b"Duplex"), "other preferences stay");
}

#[test]
fn space_audit_shares_the_file_out_by_kind() {
    let mut doc = Document::new_empty();
    let img = image(&mut doc, 300, 300, 3, false, None);
    page(&mut doc, &[("Im0", img)], "q 300 0 0 300 0 0 cm /Im0 Do Q BT ET");
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    let doc = Document::open(Arc::new(bytes.clone())).unwrap();
    let audit = audit_space(&doc, bytes.len() as u64);
    assert_eq!(audit.len(), SpaceCategory::ALL.len());
    let get = |c: SpaceCategory| audit.iter().find(|u| u.category == c).unwrap().clone();
    let (images, content, overhead) = (get(SpaceCategory::Images), get(SpaceCategory::ContentStreams), get(SpaceCategory::DocumentOverhead));
    assert!(images.percent > 80.0, "{audit:?}");
    assert!(content.bytes > 0 && overhead.bytes > 0, "{audit:?}");
    assert_eq!(audit.iter().map(|u| u.bytes).sum::<u64>(), bytes.len() as u64);
    assert!((audit.iter().map(|u| u.percent).sum::<f64>() - 100.0).abs() < 1e-6);
}

#[test]
fn space_audit_counts_images_a_content_stream_dictionary_points_at_as_images() {
    // Content added with Edit ▸ Add content records its image in the content stream's
    // dictionary (/PCAdded); the image is still an image, not part of the content stream.
    let mut doc = Document::new_empty();
    let img = image(&mut doc, 300, 300, 3, false, None);
    let p = page(&mut doc, &[("Im0", img)], "q 300 0 0 300 0 0 cm /Im0 Do Q");
    let c = doc.get(p).as_dict().and_then(|d| d.reference(b"Contents")).unwrap();
    let mut added = Dict::new();
    added.set(b"Kind".to_vec(), Object::name("Image"));
    added.set(b"Image".to_vec(), Object::Ref(img));
    doc.update_dict(c, |d| d.set(b"PCAdded".to_vec(), Object::Dict(added))).unwrap();
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    let doc = Document::open(Arc::new(bytes.clone())).unwrap();
    let audit = audit_space(&doc, bytes.len() as u64);
    let images = audit.iter().find(|u| u.category == SpaceCategory::Images).unwrap();
    assert!(images.percent > 80.0, "{audit:?}");
}

#[test]
fn invalid_links_and_unreferenced_destinations_go() {
    let mut doc = Document::new_empty();
    let p = page(&mut doc, &[], "BT ET");
    let link = |doc: &mut Document, dest: Object| {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Annot"));
        d.set(b"Subtype".to_vec(), Object::name("Link"));
        d.set(b"Rect".to_vec(), Object::Array(vec![0.into(), 0.into(), 10.into(), 10.into()]));
        d.set(b"Dest".to_vec(), dest);
        Object::Ref(doc.add(Object::Dict(d)))
    };
    let s = |t: &str| Object::String(pdfcraft_cos::PdfString::literal(t.as_bytes().to_vec()));
    let annots = vec![
        link(&mut doc, Object::Array(vec![Object::Ref(p), Object::name("Fit")])),
        link(&mut doc, Object::Array(vec![Object::Ref(ObjRef::new(999, 0)), Object::name("Fit")])),
        link(&mut doc, s("gone")),
        link(&mut doc, s("kept")),
    ];
    doc.update_dict(p, |d| d.set(b"Annots".to_vec(), Object::Array(annots))).unwrap();
    // Named destinations "kept" (used) and "unused"; a bookmark to a missing name.
    let fit = Object::Array(vec![Object::Ref(p), Object::name("Fit")]);
    let mut tree = Dict::new();
    tree.set(b"Names".to_vec(), Object::Array(vec![s("kept"), fit.clone(), s("unused"), fit]));
    let tree = doc.add(Object::Dict(tree));
    let mut names = Dict::new();
    names.set(b"Dests".to_vec(), Object::Ref(tree));
    let outlines = doc.add(Object::Null);
    let mut item = Dict::new();
    item.set(b"Title".to_vec(), s("Lost"));
    item.set(b"Parent".to_vec(), Object::Ref(outlines));
    item.set(b"Dest".to_vec(), s("nowhere"));
    let item = doc.add(Object::Dict(item));
    let mut o = Dict::new();
    o.set(b"First".to_vec(), Object::Ref(item));
    o.set(b"Last".to_vec(), Object::Ref(item));
    doc.set(outlines, Object::Dict(o));
    let root = doc.root().unwrap();
    doc.update_dict(root, |c| {
        c.set(b"Names".to_vec(), Object::Dict(names));
        c.set(b"Outlines".to_vec(), Object::Ref(outlines));
    })
    .unwrap();
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!((report.invalid_links, report.invalid_bookmarks, report.unreferenced_dests), (2, 1, 1));
    let left = doc.get(p).as_dict().unwrap().get(b"Annots").unwrap().as_array().unwrap().len();
    assert_eq!(left, 2);
    assert!(doc.get(item).as_dict().unwrap().get(b"Dest").is_none(), "the bookmark stays, without its broken destination");
    // Off: nothing changes.
    let mut again = doc.clone();
    let off = Settings { remove_invalid_links: false, remove_unreferenced_dests: false, ..Settings::default() };
    assert_eq!(optimize(&mut again, &off).unwrap().invalid_links, 0);
}

#[test]
fn progress_is_reported_per_image_and_a_refusal_cancels() {
    use pdfcraft_optimize::{OptimizeError, Stage, optimize_with_progress};
    let build = || {
        let mut doc = Document::new_empty();
        let a = image(&mut doc, 300, 300, 3, false, None);
        let b = image(&mut doc, 300, 300, 1, false, None);
        let c = image(&mut doc, 300, 300, 3, true, None);
        page(&mut doc, &[("A", a), ("B", b), ("C", c)], "q 72 0 0 72 0 0 cm /A Do Q q 72 0 0 72 100 0 cm /B Do Q q 72 0 0 72 200 0 cm /C Do Q");
        doc
    };
    let mut seen = Vec::new();
    let report = optimize_with_progress(&mut build(), &Settings::default(), &mut |s| {
        seen.push(s);
        true
    })
    .unwrap();
    assert_eq!(report.images, 3);
    assert_eq!(
        seen,
        [
            Stage::Images { done: 0, total: 3 },
            Stage::Images { done: 1, total: 3 },
            Stage::Images { done: 2, total: 3 },
            Stage::Images { done: 3, total: 3 },
            Stage::CleanUp
        ]
    );

    // Stopping after the first image.
    let mut calls = 0;
    let r = optimize_with_progress(&mut build(), &Settings::default(), &mut |_| {
        calls += 1;
        calls < 2
    });
    assert!(matches!(r, Err(OptimizeError::Cancelled)), "{r:?}");
    assert_eq!(calls, 2, "nothing runs after the refusal");
}

/// A page with `resources` and, if given, `contents`.
fn raw_page(doc: &mut Document, resources: Object, contents: Option<ObjRef>) -> ObjRef {
    let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
    let mut p = Dict::new();
    p.set(b"Type".to_vec(), Object::name("Page"));
    p.set(b"Parent".to_vec(), Object::Ref(pages));
    p.set(b"MediaBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]));
    p.set(b"Resources".to_vec(), resources);
    if let Some(c) = contents {
        p.set(b"Contents".to_vec(), Object::Ref(c));
    }
    let r = doc.add(Object::Dict(p));
    doc.update_dict(pages, |d| {
        let mut kids = d.get(b"Kids").and_then(|k| k.as_array().cloned()).unwrap_or_default();
        kids.push(Object::Ref(r));
        d.set(b"Count".to_vec(), Object::Int(kids.len() as i64));
        d.set(b"Kids".to_vec(), Object::Array(kids));
    })
    .unwrap();
    r
}

/// Builders for the tests below.
fn xobjects(pairs: &[(&str, ObjRef)]) -> Dict {
    let mut x = Dict::new();
    for (n, r) in pairs {
        x.set(n.as_bytes().to_vec(), Object::Ref(*r));
    }
    x
}

fn resources(x: Dict) -> Dict {
    let mut res = Dict::new();
    res.set(b"XObject".to_vec(), Object::Dict(x));
    res
}

fn content(doc: &mut Document, c: &str) -> ObjRef {
    doc.add(Object::Stream(Stream::flate(Dict::new(), c.as_bytes())))
}

fn form(doc: &mut Document, resources: Option<Object>, c: &str) -> ObjRef {
    let mut d = Dict::new();
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 1.into(), 1.into()]));
    if let Some(r) = resources {
        d.set(b"Resources".to_vec(), r);
    }
    doc.add(Object::Stream(Stream::flate(d, c.as_bytes())))
}

fn xobject_names(doc: &Document, resources: &Object) -> Vec<String> {
    let res = doc.resolve(resources);
    let mut n: Vec<String> =
        res.as_dict().unwrap().get(b"XObject").unwrap().as_dict().unwrap().iter().map(|(k, _)| String::from_utf8_lossy(k).into_owned()).collect();
    n.sort();
    n
}

fn page_xobjects(doc: &Document, page: ObjRef) -> Vec<String> {
    let res = doc.get(page).as_dict().unwrap().get(b"Resources").unwrap().clone();
    xobject_names(doc, &res)
}

fn count_images(bytes: &[u8]) -> usize {
    let doc = Document::open(Arc::new(bytes.to_vec())).unwrap();
    doc.object_numbers()
        .into_iter()
        .filter(|n| matches!(&*doc.get(ObjRef::new(*n, doc.generation(*n))), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image")))
        .count()
}

#[test]
fn images_and_forms_nothing_draws_are_dropped() {
    let mut doc = Document::new_empty();
    let img = |doc: &mut Document| image(doc, 64, 64, 3, false, None);

    // Edit a PDF deleting an image: the page gets a new content stream without the `Do`; the
    // old one, which still names the image, is no longer used.
    let t1 = img(&mut doc);
    let old = content(&mut doc, "q 72 0 0 72 0 0 cm /ImT Do Q");
    let t = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("ImT", t1)]))), Some(old));
    let new = content(&mut doc, "");
    doc.update_dict(t, |p| p.set(b"Contents".to_vec(), Object::Ref(new))).unwrap();
    // Dropped: a JPEG a page no longer draws, beside one it does.
    let a1 = img(&mut doc);
    let a2 = image(&mut doc, 64, 64, 3, true, None);
    let c = content(&mut doc, "q 72 0 0 72 0 0 cm /Im1 Do Q");
    let a = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("Im1", a1), ("Im2", a2)]))), Some(c));
    // Dropped from resources the page alone refers to, written as an object of their own.
    let s1 = img(&mut doc);
    let own = doc.add(Object::Dict(resources(xobjects(&[("Unused", s1)]))));
    let c = content(&mut doc, "");
    let s = raw_page(&mut doc, Object::Ref(own), Some(c));
    // An image drawn only inside a form stays; one beside it goes.
    let (d3, d4) = (img(&mut doc), img(&mut doc));
    let f = form(&mut doc, None, "/Im3 Do");
    let c = content(&mut doc, "/F Do");
    let d = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("F", f), ("Im3", d3), ("Im4", d4)]))), Some(c));

    // Kept, though the page's own content doesn't name them:
    // - drawn by a form with resources of its own that lack the name (renderers fall back to
    //   the page's);
    let k1 = img(&mut doc);
    let mut procset = Dict::new();
    procset.set(b"ProcSet".to_vec(), Object::Array(vec![Object::name("PDF")]));
    let fk = form(&mut doc, Some(Object::Dict(procset)), "/ImK Do");
    let c = content(&mut doc, "/Fk Do");
    let k = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("Fk", fk), ("ImK", k1)]))), Some(c));
    // - drawn by a Type 3 glyph named like an inert key;
    let g6 = img(&mut doc);
    let glyph = content(&mut doc, "/Im6 Do");
    let mut procs = Dict::new();
    procs.set(b"P".to_vec(), Object::Ref(glyph));
    let mut t3 = Dict::new();
    t3.set(b"Type".to_vec(), Object::name("Font"));
    t3.set(b"Subtype".to_vec(), Object::name("Type3"));
    t3.set(b"CharProcs".to_vec(), Object::Dict(procs));
    let mut fonts = Dict::new();
    fonts.set(b"T3".to_vec(), Object::Dict(t3));
    let mut res = resources(xobjects(&[("Im6", g6)]));
    res.set(b"Font".to_vec(), Object::Dict(fonts));
    let c = content(&mut doc, "BT /T3 1 Tf (P) Tj ET");
    let g = raw_page(&mut doc, Object::Dict(res), Some(c));
    // - after an inline image whose data runs into the operators (no space before `EI`), or
    //   given as the second of two operands;
    let (l1, l2) = (img(&mut doc), img(&mut doc));
    let c = content(&mut doc, "BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 00>EI Q q /ImL Do Q /Other /ImL2 Do");
    let l = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("ImL", l1), ("ImL2", l2)]))), Some(c));
    // - drawn by a form field that is on no page;
    let z1 = img(&mut doc);
    let c = content(&mut doc, "");
    let z = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("Z1", z1)]))), Some(c));
    let wap = form(&mut doc, None, "/Z1 Do");
    let mut appearance = Dict::new();
    appearance.set(b"N".to_vec(), Object::Ref(wap));
    let mut widget = Dict::new();
    widget.set(b"Subtype".to_vec(), Object::name("Widget"));
    widget.set(b"AP".to_vec(), Object::Dict(appearance));
    let widget = doc.add(Object::Dict(widget));
    let mut acroform = Dict::new();
    acroform.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(widget)]));
    let root = doc.root().unwrap();
    doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(acroform))).unwrap();
    // - in resources shared by pages (or with an annotation's appearance), or inherited from the
    //   page tree: left alone;
    let (b1, b2) = (img(&mut doc), img(&mut doc));
    let shared = doc.add(Object::Dict(resources(xobjects(&[("X1", b1), ("X2", b2)]))));
    let c = content(&mut doc, "0 0 m");
    raw_page(&mut doc, Object::Ref(shared), Some(c));
    let c = content(&mut doc, "/X1 Do");
    raw_page(&mut doc, Object::Ref(shared), Some(c));
    let (inh, unused_inh) = (img(&mut doc), img(&mut doc));
    let tree = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
    doc.update_dict(tree, |d| d.set(b"Resources".to_vec(), Object::Dict(resources(xobjects(&[("Inh", inh), ("Old", unused_inh)]))))).unwrap();
    // - named by another page for its own image.
    let (r1, r2) = (img(&mut doc), img(&mut doc));
    let c = content(&mut doc, "");
    let r = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("Im9", r1)]))), Some(c));
    let c = content(&mut doc, "/Im9 Do");
    raw_page(&mut doc, Object::Dict(resources(xobjects(&[("Im9", r2)]))), Some(c));

    let before = write_full(&doc, &SaveOptions::default()).unwrap();
    let report = optimize(&mut doc, &Settings::default()).unwrap();
    assert_eq!(report.unused_xobjects, 4, "{report:?}");
    assert!(page_xobjects(&doc, t).is_empty(), "the deleted image goes");
    assert_eq!(page_xobjects(&doc, a), ["Im1"]);
    assert!(xobject_names(&doc, &Object::Ref(own)).is_empty());
    assert!(page_xobjects(&doc, s).is_empty());
    assert_eq!(page_xobjects(&doc, d), ["F", "Im3"]);
    assert_eq!(page_xobjects(&doc, k), ["Fk", "ImK"]);
    assert_eq!(page_xobjects(&doc, g), ["Im6"]);
    assert_eq!(page_xobjects(&doc, l), ["ImL", "ImL2"]);
    assert_eq!(page_xobjects(&doc, z), ["Z1"]);
    assert_eq!(xobject_names(&doc, &Object::Ref(shared)), ["X1", "X2"]);
    assert_eq!(page_xobjects(&doc, tree), ["Inh", "Old"]);
    assert_eq!(page_xobjects(&doc, r), ["Im9"]);
    let after = write_full(&doc, &SaveOptions::default()).unwrap();
    assert_eq!((count_images(&before), count_images(&after)), (17, 13), "the dropped images are gone from the file");
}

#[test]
fn nothing_is_dropped_when_a_stream_cannot_be_checked() {
    // A page with an image nothing draws, and an appearance stream on it.
    let build = |stream: Stream| {
        let mut doc = Document::new_empty();
        let unused = image(&mut doc, 64, 64, 3, false, None);
        let c = content(&mut doc, "");
        let page = raw_page(&mut doc, Object::Dict(resources(xobjects(&[("Im1", unused)]))), Some(c));
        let other = doc.add(Object::Stream(stream));
        let mut appearance = Dict::new();
        appearance.set(b"N".to_vec(), Object::Ref(other));
        let mut annot = Dict::new();
        annot.set(b"AP".to_vec(), Object::Dict(appearance));
        doc.update_dict(page, |p| p.set(b"Annots".to_vec(), Object::Array(vec![Object::Dict(annot)]))).unwrap();
        let report = optimize(&mut doc, &Settings::default()).unwrap();
        (report.unused_xobjects, page_xobjects(&doc, page))
    };
    let (gone, kept) = ((1, Vec::<String>::new()), (0, vec!["Im1".to_string()]));
    // Valid compressed data with extra keys.
    let flate = |extra: &[(&str, Object)]| {
        let mut s = Stream::flate(Dict::new(), b"0 0 m");
        s.dict.set(b"Filter".to_vec(), Object::name("FlateDecode"));
        for (k, v) in extra {
            s.dict.set(k.as_bytes().to_vec(), v.clone());
        }
        s
    };
    let parms = |k: &str, v: Object| {
        let mut p = Dict::new();
        p.set(k.as_bytes().to_vec(), v);
        Object::Dict(p)
    };
    // Readable: the image goes.
    assert_eq!(build(flate(&[])), gone);
    assert_eq!(build(flate(&[("DecodeParms", parms("Predictor", Object::Int(1)))])), gone);
    // The name written with an escape still counts.
    assert_eq!(build(Stream::flate(Dict::new(), b"/Im#31 Do")), kept);
    // Read differently by viewers, or not at all: nothing goes.
    let dangling = Object::Ref(ObjRef::new(999, 0));
    let cases = [
        Stream::from_raw(flate(&[]).dict, b"not zlib data".to_vec()),
        flate(&[("Filter", dangling.clone())]),
        flate(&[("Filter", Object::Array(vec![Object::name("FlateDecode"), dangling.clone()]))]),
        flate(&[("Filter", Object::Array(vec![Object::name("Crypt"), Object::name("FlateDecode")]))]),
        flate(&[("DP", parms("Predictor", Object::Int(1)))]),
        flate(&[("DecodeParms", Object::Array(vec![Object::Dict(Dict::new()), Object::Dict(Dict::new())]))]),
        flate(&[("DecodeParms", parms("Predictor", dangling))]),
        flate(&[("DecodeParms", parms("Predictor", Object::Real(12.5)))]),
        flate(&[("F", Object::name("external.dat"))]),
        // An image codec on a stream drawn as an appearance: viewers decode it as content.
        Stream::from_raw(
            {
                let mut d = Dict::new();
                d.set(b"Filter".to_vec(), Object::name("CCITTFaxDecode"));
                d
            },
            vec![0; 8],
        ),
    ];
    for s in cases {
        let dict = s.dict.clone();
        assert_eq!(build(s), kept, "{dict:?}");
    }
}
