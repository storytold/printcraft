//! Create a PDF and Reduce File Size in the real shell.

use pdfcraft_ui_egui::PdfCraftApp;

fn png() -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, 8, 4);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[120u8; 8 * 4 * 3]).unwrap();
    }
    out
}

#[test]
fn image_import_dialog_chooses_dpi_and_cancels() {
    use egui_kittest::{Harness, kittest::Queryable};
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.begin_image_import(vec![("scan.png".into(), png())]);
        app
    });
    h.run_steps(3);
    if let Ok(path) = std::env::var("PDFCRAFT_IMAGE_IMPORT_SHOT") {
        h.render().unwrap().save(path).unwrap();
    }
    h.get_by_label("Use 72 DPI (one point per pixel)").click();
    h.run_steps(1);
    h.get_by_label("Create").click();
    h.run_steps(3);
    assert!(h.state().image_import.is_none());
    let page = &h.state().session.docs()[0].info.pages[0];
    assert_eq!((page.width, page.height), (8.0, 4.0));
    h.state_mut().begin_image_import(vec![("scan.png".into(), png())]);
    h.state_mut().image_import.as_mut().unwrap().dpi = 144.0;
    h.run_steps(2);
    h.get_by_label("Use custom DPI").click();
    h.run_steps(1);
    h.get_by_label("Create").click();
    h.run_steps(3);
    let page = &h.state().session.docs()[1].info.pages[0];
    assert_eq!((page.width, page.height), (4.0, 2.0));
    h.state_mut().begin_image_import(vec![("scan.png".into(), png())]);
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state().image_import.is_none());
    assert_eq!(h.state().session.docs().len(), 2);
}

#[test]
fn opening_images_and_text_converts_them_to_new_pdfs() {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.open_bytes("photo.png", Some("/tmp/photo.png".into()), png()).unwrap();
    app.open_bytes("notes.txt", None, b"first line\nsecond line".to_vec()).unwrap();
    app.create_from_images(vec![("a.png".into(), png()), ("b.png".into(), png())]);
    assert!(app.execute("create.blank"));
    let docs = app.session.docs();
    let summary: Vec<(String, usize, bool)> = docs.iter().map(|d| (d.name.clone(), d.info.pages.len(), d.dirty)).collect();
    assert_eq!(
        summary,
        [("photo.pdf".to_string(), 1, true), ("notes.pdf".into(), 1, true), ("Images.pdf".into(), 2, true), ("Untitled.pdf".into(), 1, true)]
    );
    assert!(docs[0].path.is_none(), "a converted file has no PDF path yet: Save asks where");
    assert!(app.open_bytes("junk.png", None, b"\x89PNG\r\n\x1a\nnot really".to_vec()).is_err());
}

/// Open `name` and run Reduce File Size on it, saving to `out` without a dialog.
fn reduce(name: &'static str, bytes: Vec<u8>, out: &std::path::Path) -> egui_kittest::Harness<'static, PdfCraftApp> {
    let out2 = out.to_path_buf();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes(name, None, bytes).unwrap();
        app.save_override = Some(out2.to_string_lossy().into_owned());
        app
    });
    h.run_steps(2);
    assert!(h.state_mut().execute("optimize.reduce"));
    // It runs on a worker thread like the PDF Optimizer, with the progress card meanwhile.
    let start = std::time::Instant::now();
    while h.state().optimize_run.is_some() && start.elapsed() < std::time::Duration::from_secs(30) {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    h.run_steps(2);
    assert!(h.state().optimize_run.is_none() && h.state().progress_notice.is_none(), "the run finished");
    h
}

#[test]
fn reduce_file_size_writes_a_compact_copy() {
    use egui_kittest::kittest::Queryable;
    // A photo-like picture, stored losslessly: Reduce recompresses it as JPEG.
    let (w, h) = (600u32, 400u32);
    let px: Vec<u8> = (0..w * h).flat_map(|i| [(i % w * 255 / w) as u8 ^ (i & 7) as u8, (i / w * 255 / h) as u8, 128]).collect();
    let mut photo = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut photo, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&px).unwrap();
    }
    let dir = std::env::temp_dir().join(format!("pdfcraft-reduce-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("reduced.pdf");
    let h = reduce("photo.png", photo, &out);
    let bytes = std::fs::read(&out).unwrap();
    assert!(bytes.starts_with(b"%PDF-"));
    assert!(bytes.len() < h.state().session.docs()[0].bytes.len(), "the copy is smaller");
    assert!(h.state().session.docs()[0].dirty, "the open document is unchanged");
    h.get_by_label_contains("% smaller");
}

#[test]
fn reduce_file_size_saves_nothing_when_the_file_is_already_small() {
    use egui_kittest::kittest::Queryable;
    let dir = std::env::temp_dir().join(format!("pdfcraft-reduce-small-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("reduced.pdf");
    // A new text PDF is already compact: a copy would be no smaller (#490).
    let h = reduce("notes.txt", "lorem ipsum ".repeat(500).into_bytes(), &out);
    assert!(!out.exists(), "no copy was written");
    h.get_by_label_contains("This file is already as small as it can be made");
}

#[test]
fn clipboard_images_and_text_become_new_pdfs() {
    use pdfcraft_ui_egui::Clip;
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.create_from_clip(Clip::Image { width: 40, height: 20, rgba: [10u8, 20, 30, 255].repeat(40 * 20) }).unwrap();
    app.create_from_clip(Clip::Text("Pasted\nlines".into())).unwrap();
    let docs = app.session.docs();
    assert_eq!(docs.len(), 2);
    assert_eq!(docs[0].name, "Clipboard.pdf");
    // The image page takes the image's size (one pixel per point at 72 dpi).
    let p = &docs[0].info.pages[0];
    assert!((p.width / p.height - 2.0).abs() < 0.01, "{} × {}", p.width, p.height);
    assert!(app.create_from_clip(Clip::Image { width: 4, height: 4, rgba: vec![0; 3] }).is_err(), "malformed images are refused");
}

#[test]
fn the_pdf_optimizer_dialog_saves_an_optimized_copy() {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    let dir = std::env::temp_dir().join(format!("pdfcraft-optimizer-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("optimized.pdf");
    let out2 = out.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("notes.txt", None, "lorem ipsum ".repeat(500).into_bytes()).unwrap();
        app.save_override = Some(out2.to_string_lossy().into_owned());
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("optimize.advanced"));
    h.run_steps(2);
    h.get_by_label("PDF Optimizer");
    h.get_by_label("Color Images");
    h.get_by_label("Discard Objects").click();
    h.run_steps(2);
    h.get_by_label("Discard document tags").click();
    h.get_by_label("Discard User Data").click();
    h.run_steps(2);
    h.get_by_label("Discard document information and metadata").click();
    h.run_steps(1);
    {
        let d = &h.state().optimize_draft;
        assert!(d.settings.discard_tags && d.discard == vec![pdfcraft_engine::Hidden::Metadata]);
    }
    h.get_by_label("OK").click();
    // The click is handled next frame. The optimization then runs on a worker thread and the
    // copy is saved when it is done.
    h.run_steps(1);
    let start = std::time::Instant::now();
    while (h.state().optimize_run.is_some() || !out.exists()) && start.elapsed() < std::time::Duration::from_secs(30) {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    h.run_steps(2);
    assert!(h.state().progress_notice.is_none(), "the progress card is gone");
    assert!(std::fs::read(&out).unwrap().starts_with(b"%PDF-"));
    assert_eq!(h.state().dialog, None);
}

fn mixed_files(app: &PdfCraftApp) -> Vec<(String, Vec<u8>)> {
    let pdf = app.session.create_from_text("a", "from a pdf").unwrap();
    vec![
        ("notes.txt".into(), b"from text".to_vec()),
        ("report.docx".into(), b"PK\x03\x04".to_vec()),
        ("a.pdf".into(), pdf.to_vec()),
        ("photo.png".into(), png()),
    ]
}

#[test]
fn multiple_files_open_as_one_document_in_the_page_grid() {
    use egui_kittest::{Harness, kittest::Queryable};
    let path = std::env::temp_dir().join(format!("pdfcraft-create-multiple-{}.pdf", std::process::id()));
    let mut h = Harness::builder().with_size(egui::vec2(1000.0, 720.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let files = mixed_files(&app);
        app.use_files(pdfcraft_ui_egui::FilePurpose::CreateMultiple, files);
        app
    });
    h.run_steps(4);
    {
        let app = h.state();
        assert_eq!(app.views.len(), 1);
        assert!(app.views[0].organize, "the pages are shown as a grid");
        let doc = app.session.get(app.views[0].id).unwrap();
        assert_eq!(doc.name, "Combined.pdf");
        assert!(doc.dirty && doc.path.is_none(), "unsaved until the user saves it");
        assert_eq!(doc.info.pages.len(), 3, "the Word file can't be converted and is left out");
        assert_eq!(doc.info.outline.iter().map(|o| o.title.as_str()).collect::<Vec<_>>(), ["notes", "a", "photo"]);
    }
    h.get_by_label_contains("left out: report.docx");
    h.get_by_label("Insert a file before page 2");
    if let Ok(path) = std::env::var("PDFCRAFT_CREATE_MULTIPLE_SHOT") {
        h.run_steps(20);
        h.render().unwrap().save(path).unwrap();
    }
    // Remove the middle page and save what is left.
    h.get_by_label("Page 2").click();
    h.run_steps(2);
    h.get_by_label("Delete pages (Delete)").click();
    h.run_steps(3);
    h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
    h.get_by_label("Save pages").click();
    h.run_steps(4);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    assert!(!doc.dirty && doc.info.pages.len() == 2);
    assert!(std::fs::read(&path).unwrap().starts_with(b"%PDF-"));
}

#[test]
fn multiple_files_that_cannot_be_converted_open_nothing() {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.use_files(pdfcraft_ui_egui::FilePurpose::CreateMultiple, vec![("report.docx".into(), b"PK\x03\x04".to_vec())]);
    assert!(app.views.is_empty());
    assert!(app.toast.is_some());
}

#[test]
fn multiple_files_past_a_thousand_are_all_used() {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    let pdf = app.session.create_from_text("a", "from a pdf").unwrap();
    let files: Vec<(String, Vec<u8>)> = (0..1001).map(|i| (format!("f{i}.pdf"), pdf.to_vec())).collect();
    app.use_files(pdfcraft_ui_egui::FilePurpose::CreateMultiple, files);
    assert_eq!(app.views.len(), 1);
    let doc = app.session.get(app.views[0].id).unwrap();
    assert_eq!(doc.info.pages.len(), 1001, "every picked file is used, in order");
}
