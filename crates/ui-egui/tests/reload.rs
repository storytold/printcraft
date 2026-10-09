//! A document reloads when its file changes on disk (#431): a LaTeX or Typst build rewriting the
//! open PDF. Here the watcher looks once per frame on the test's thread (`run_inline`); in the
//! app a worker thread looks every quarter second.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{PdfCraftApp, SaveTarget};

/// An `n`-page document with a proper xref table; page `i` shows "Page i+1", and with `notes`
/// each page has a sticky note.
fn fixture_with(n: usize, notes: bool) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    let annots = if notes { " /Annots [<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /Contents (note) >>]" } else { "" };
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >>{annots} >>", 5 + 2 * i));
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

fn fixture(n: usize) -> Vec<u8> {
    fixture_with(n, false)
}

/// A PDF in a fresh temp folder, removed when the test ends.
struct TempPdf {
    dir: PathBuf,
    path: String,
    builds: std::cell::Cell<u64>,
}

impl TempPdf {
    fn new(test: &str, bytes: &[u8]) -> Self {
        let dir = std::env::temp_dir().join(format!("pdfcraft-reload-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("thesis.pdf").to_string_lossy().into_owned();
        std::fs::write(&path, bytes).unwrap();
        Self { dir, path, builds: std::cell::Cell::new(0) }
    }

    /// Rewrite it as a build does: new contents, and a later modification time every time.
    fn rebuild(&self, bytes: &[u8]) {
        std::fs::write(&self.path, bytes).unwrap();
        self.builds.set(self.builds.get() + 1);
        let file = std::fs::File::options().write(true).open(&self.path).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(60 * self.builds.get())).unwrap();
    }
}

impl Drop for TempPdf {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn harness(path: &str) -> Harness<'static, PdfCraftApp> {
    let path = path.to_owned();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.run_inline = true;
        app.open_path(&path);
        app
    });
    h.run_steps(4);
    h
}

fn pages(h: &Harness<'static, PdfCraftApp>) -> usize {
    let app = h.state();
    app.session.get(app.views[0].id).unwrap().info.pages.len()
}

fn dirty(h: &Harness<'static, PdfCraftApp>) -> bool {
    let app = h.state();
    app.session.get(app.views[0].id).unwrap().dirty
}

fn rotate_first_page(h: &mut Harness<'static, PdfCraftApp>) {
    h.state_mut().views[0].select_pages(&[0]);
    h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
}

#[test]
fn a_rebuilt_file_reloads_in_place_keeping_the_page_and_zoom() {
    let file = TempPdf::new("rebuild", &fixture(3));
    let mut h = harness(&file.path);
    let id = h.state().views[0].id;
    h.state_mut().views[0].fit = pdfcraft_ui_egui::canvas::Fit::None;
    h.state_mut().views[0].zoom = 1.5;
    h.state_mut().views[0].go_to_page(2);
    h.run_steps(4);
    file.rebuild(&fixture(4));
    // One look sees the change, the next sees it settled and reloads.
    h.run_steps(3);
    let app = h.state();
    assert_eq!(pages(&h), 4, "the new version shows");
    assert_eq!((app.views.len(), app.views[0].id), (1, id), "in the same tab");
    assert_eq!((app.views[0].current, app.views[0].zoom), (2, 1.5), "at the same page and zoom");
}

#[test]
fn a_half_written_file_waits_until_the_pdf_is_whole() {
    let file = TempPdf::new("partial", &fixture(2));
    let mut h = harness(&file.path);
    let whole = fixture(5);
    // A build part-way through: whole objects up to the fourth page, no cross-reference table
    // or end yet. The lenient parser would open this as a document with pages missing.
    let cut = whole.windows(9).position(|w| w == b"\n10 0 obj").unwrap();
    file.rebuild(&whole[..cut]);
    h.run_steps(4);
    assert_eq!(pages(&h), 2, "half a file is not loaded");
    file.rebuild(&whole);
    h.run_steps(3);
    assert_eq!(pages(&h), 5, "the whole one is");
}

#[test]
fn saving_does_not_reload_or_lose_undo() {
    let file = TempPdf::new("save", &fixture(2));
    let mut h = harness(&file.path);
    rotate_first_page(&mut h);
    h.state_mut().save_active(SaveTarget::InPlace);
    h.run_steps(4);
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    assert!(!doc.dirty, "saved");
    assert!(doc.can_undo().is_some(), "a reload would have cleared the undo history");
}

#[test]
fn unsaved_changes_are_kept_until_reload_is_chosen() {
    let file = TempPdf::new("dirty", &fixture(2));
    let mut h = harness(&file.path);
    rotate_first_page(&mut h);
    file.rebuild(&fixture(4));
    h.run_steps(3);
    assert_eq!((pages(&h), dirty(&h)), (2, true), "not replaced behind the user's back");
    h.get_by_label_contains("This file changed on disk");
    h.get_by_label("Reload").click();
    h.run_steps(3);
    assert_eq!((pages(&h), dirty(&h)), (4, false), "replaced when asked");
    assert!(h.query_by_label_contains("This file changed on disk").is_none());
}

#[test]
fn a_deleted_file_leaves_the_document_open() {
    let file = TempPdf::new("deleted", &fixture(2));
    let mut h = harness(&file.path);
    std::fs::remove_file(&file.path).unwrap();
    h.run_steps(4);
    assert_eq!((h.state().views.len(), pages(&h)), (1, 2), "still open, as it was");
    file.rebuild(&fixture(3));
    h.run_steps(3);
    assert_eq!(pages(&h), 3, "and the next build shows");
}

#[test]
fn nothing_reloads_with_the_preference_off() {
    assert!(PdfCraftApp::new().reload_changed_files, "on by default");
    let file = TempPdf::new("off", &fixture(2));
    let mut h = harness(&file.path);
    h.state_mut().reload_changed_files = false;
    h.run_steps(1);
    file.rebuild(&fixture(3));
    h.run_steps(4);
    assert_eq!(pages(&h), 2);
    let mut restarted = PdfCraftApp::new();
    restarted.restore(&h.state().persist());
    assert!(!restarted.reload_changed_files, "the choice is kept");
}

#[test]
fn a_shorter_new_version_drops_what_pointed_into_the_old_one() {
    let file = TempPdf::new("shorter", &fixture_with(5, true));
    let mut h = harness(&file.path);
    h.state_mut().views[0].go_to_page(4);
    h.state_mut().views[0].select_pages(&[3, 4]);
    h.state_mut().views[0].comments.selected = Some((4, 0));
    h.run_steps(2);
    file.rebuild(&fixture(1));
    h.run_steps(4);
    let view = &h.state().views[0];
    assert_eq!(pages(&h), 1);
    assert_eq!(view.current, 0, "the page is clamped");
    assert!(view.selected.is_empty() && view.comments.selected.is_none(), "no selection points past the new version");
}
