use std::sync::Arc;

use super::*;
use crate::spool::{Duplex, Job, Printer, lp_args, parse_lpstat, parse_windows_printers, windows_env_vars};

/// `n` pages of 200×300 (page 3 is landscape 300×200 when n ≥ 3); page i shows "(Page i+1)".
/// Page 1 carries a printable square comment, a non-printing note, and a stamp.
fn fixture(n: usize) -> Document {
    let mut objs: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 6 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")).into_bytes());
    objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    // 4: an appearance stream; 5: unused.
    let ap = b"0 0 1 rg 0 0 10 10 re f";
    objs.push(
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Length {} >>\nstream\n{}\nendstream", ap.len(), String::from_utf8_lossy(ap))
            .into_bytes(),
    );
    objs.push(b"null".to_vec());
    for i in 0..n {
        let annots = if i == 0 {
            " /Annots [<< /Type /Annot /Subtype /Square /F 4 /Rect [10 10 50 50] /AP << /N 4 0 R >> >> << /Type /Annot /Subtype /Text /F 0 /Rect [60 10 80 30] /AP << /N 4 0 R >> >> << /Type /Annot /Subtype /Stamp /F 4 /Rect [100 10 140 50] /AP << /N 4 0 R >> >>]"
        } else {
            ""
        };
        let mb = if i == 2 { " /MediaBox [0 0 300 200]" } else { "" };
        objs.push(
            format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >>{mb}{annots} >>", 7 + 2 * i).into_bytes(),
        );
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes());
    }
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

fn settings(pages: Vec<usize>, layout: Layout) -> Settings {
    Settings { pages, paper: (612.0, 792.0), layout, ..Settings::default() }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.01
}

#[test]
fn page_selection() {
    let labels: Vec<String> = ["i", "ii", "1", "2", "A-1"].iter().map(|s| s.to_string()).collect();
    assert_eq!(select_pages(5, None, &labels, Subset::All, false).unwrap(), [0, 1, 2, 3, 4]);
    assert_eq!(select_pages(5, Some("1-2, 5"), &[], Subset::All, false).unwrap(), [0, 1, 4]);
    assert_eq!(select_pages(5, Some("4-"), &[], Subset::All, false).unwrap(), [3, 4]);
    assert_eq!(select_pages(5, Some("-2"), &[], Subset::All, false).unwrap(), [0, 1]);
    assert_eq!(select_pages(5, Some("ii-2, A-1"), &labels, Subset::All, false).unwrap(), [1, 2, 3, 4], "labels, even with a dash");
    assert_eq!(select_pages(5, None, &[], Subset::Even, true).unwrap(), [3, 1]);
    assert_eq!(select_pages(5, None, &[], Subset::Odd, false).unwrap(), [0, 2, 4]);
    assert!(matches!(select_pages(5, Some("7"), &[], Subset::All, false), Err(PrintError::Invalid(_))));
    assert!(matches!(select_pages(5, Some("x"), &[], Subset::All, false), Err(PrintError::Invalid(_))));
    assert_eq!(select_pages(1, None, &[], Subset::Even, false), Err(PrintError::NoPages));
    // An explicit list (selected thumbnails): positions, never labels, in the order given.
    assert_eq!(select_listed(5, &[1, 3], Subset::All, false).unwrap(), [1, 3]);
    assert_eq!(select_listed(5, &[0, 2, 4], Subset::Even, false).unwrap(), [2]);
    assert_eq!(select_listed(5, &[0, 2, 4], Subset::Odd, true).unwrap(), [4, 0]);
    assert!(matches!(select_listed(5, &[1, 5], Subset::All, false), Err(PrintError::Invalid(_))));
    assert!(matches!(select_listed(0, &[usize::MAX], Subset::All, false), Err(PrintError::Invalid(_))));
    assert_eq!(select_listed(5, &[], Subset::All, false), Err(PrintError::NoPages));
}

#[test]
fn size_modes() {
    let sizes = [(200.0, 300.0), (1000.0, 1500.0)];
    let fit = layout(&sizes, &settings(vec![0, 1], Layout::Size(SizeMode::Fit))).unwrap();
    let s0 = fit[0].placed[0].matrix.0[0];
    assert!(close(s0, 2.52), "min(576 / 200, 756 / 300): {s0}");
    let actual = layout(&sizes, &settings(vec![0], Layout::Size(SizeMode::Actual))).unwrap();
    assert_eq!(actual[0].placed[0].matrix.0, [1.0, 0.0, 0.0, 1.0, 206.0, 246.0], "centred at 100%");
    let shrink = layout(&sizes, &settings(vec![0, 1], Layout::Size(SizeMode::Shrink))).unwrap();
    assert_eq!(shrink[0].placed[0].matrix.0[0], 1.0, "small pages stay at 100%");
    assert!(shrink[1].placed[0].matrix.0[0] < 1.0, "big pages shrink");
    let custom = layout(&sizes, &settings(vec![0], Layout::Size(SizeMode::Custom(50.0)))).unwrap();
    assert_eq!(custom[0].placed[0].matrix.0[0], 0.5);
    // Auto orientation turns the sheet for landscape pages.
    let land = layout(&[(300.0, 200.0)], &settings(vec![0], Layout::Size(SizeMode::Fit))).unwrap();
    assert_eq!(land[0].size, (792.0, 612.0));
}

#[test]
fn multiple_pages_per_sheet() {
    let sizes = vec![(200.0, 300.0); 5];
    let sheets = layout(&sizes, &settings((0..5).collect(), Layout::multiple(4))).unwrap();
    assert_eq!(sheet_pages(&sheets), [vec![0, 1, 2, 3], vec![4]]);
    // Horizontal order: 0 top-left, 1 top-right, 2 bottom-left.
    let o = |k: usize| (sheets[0].placed[k].matrix.0[4], sheets[0].placed[k].matrix.0[5]);
    assert!(o(1).0 > o(0).0 && close(o(1).1, o(0).1) && o(2).1 < o(0).1 && close(o(2).0, o(0).0));
    // Two per sheet prints side by side on landscape paper.
    let two = layout(&sizes, &settings(vec![0, 1], Layout::multiple(2))).unwrap();
    assert_eq!(two[0].size, (792.0, 612.0));
    assert!(two[0].placed[1].matrix.0[4] > two[0].placed[0].matrix.0[4]);
    // Vertical order and borders.
    let v =
        layout(&sizes, &settings(vec![0, 1, 2], Layout::Multiple { cols: 2, rows: 2, order: PageOrder::Vertical, border: true, auto_rotate: false }))
            .unwrap();
    assert!(close(v[0].placed[1].matrix.0[4], v[0].placed[0].matrix.0[4]) && v[0].placed[1].matrix.0[5] < v[0].placed[0].matrix.0[5]);
    assert_eq!(v[0].borders.len(), 3);
    // Auto-rotate turns a landscape page in a portrait cell.
    let r = layout(&[(300.0, 200.0), (200.0, 300.0)], &settings(vec![0, 1], Layout::multiple(4))).unwrap();
    // The first (landscape) page sets a landscape sheet; the portrait page is turned to fit.
    assert_eq!(r[0].size, (792.0, 612.0));
    assert!(r[0].placed[0].matrix.0[0] > 0.0);
    assert_eq!(r[0].placed[1].matrix.0[0], 0.0, "rotated");
}

#[test]
fn booklets_pair_pages_for_folding() {
    let sizes = vec![(200.0, 300.0); 6];
    let sheets = layout(&sizes, &settings((0..6).collect(), Layout::Booklet { subset: BookletSubset::BothSides, binding: Binding::Left })).unwrap();
    // 6 pages pad to 8: sheet 1 front [8,1] → [blank, 0], back [1, 6]; sheet 2 front [5, 2], back [3, 4].
    assert_eq!(sheet_pages(&sheets), [vec![0], vec![1], vec![5, 2], vec![3, 4]]);
    assert!(sheets[0].placed[0].matrix.0[4] >= 395.9, "page 1 on the right half");
    assert_eq!(sheets[0].size, (792.0, 612.0));
    let front = layout(&sizes, &settings((0..6).collect(), Layout::Booklet { subset: BookletSubset::FrontOnly, binding: Binding::Right })).unwrap();
    assert_eq!(sheet_pages(&front), [vec![0], vec![2, 5]], "right binding mirrors the spread");
}

#[test]
fn posters_tile_with_overlap() {
    // A 200×300 page at 400% = 800×1200 on Letter with 18 pt margins (576×756 tiles), overlap 36.
    let sheets = layout(&[(200.0, 300.0)], &settings(vec![0], Layout::Poster { scale: 400.0, overlap: 36.0, cut_marks: true })).unwrap();
    assert_eq!(sheets.len(), 4, "2 × 2 tiles");
    assert_eq!(sheets[0].lines.len(), 8, "cut marks");
    let c0 = sheets[0].placed[0].clip;
    let c1 = sheets[1].placed[0].clip;
    assert!(close(c0[2] - c1[0], 36.0 / 4.0), "neighbouring tiles share the overlap");
    assert!(layout(&[(200.0, 300.0)], &settings(vec![0], Layout::Poster { scale: 400.0, overlap: 400.0, cut_marks: false })).is_err());
}

#[test]
fn imposed_pdf_has_the_sheets_and_honours_comments_and_forms() {
    let doc = fixture(3);
    let out = impose(&doc, &settings(vec![0, 1, 2], Layout::multiple(2))).unwrap();
    let printed = Document::open(Arc::new(out.clone())).unwrap();
    let pages = pdfcraft_model::pages(&printed);
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].crop(&printed), [0.0, 0.0, 792.0, 612.0]);
    // The source pages are form XObjects holding their content.
    let streams = |d: &Document| -> Vec<String> {
        d.object_numbers()
            .into_iter()
            .filter_map(|n| match &*d.get(ObjRef::new(n, d.generation(n))) {
                Object::Stream(s) => s.decoded().ok().map(|b| String::from_utf8_lossy(&b).into_owned()),
                _ => None,
            })
            .collect()
    };
    let all = streams(&printed).join("\n");
    assert!(all.contains("(Page 1)") && all.contains("(Page 3)"));
    // Markups: the printable square and the stamp, not the note without the Print flag.
    let page1 = streams(&printed).into_iter().find(|s| s.contains("(Page 1)")).unwrap();
    assert_eq!(page1.matches(" Do Q").count(), 2, "{page1}");
    let doc_only = impose(&doc, &Settings { content: Content::Document, ..settings(vec![0], Layout::Size(SizeMode::Fit)) }).unwrap();
    let p = streams(&Document::open(Arc::new(doc_only)).unwrap()).into_iter().find(|s| s.contains("(Page 1)")).unwrap();
    assert_eq!(p.matches(" Do Q").count(), 0);
    let stamps = impose(&doc, &Settings { content: Content::DocumentAndStamps, ..settings(vec![0], Layout::Size(SizeMode::Fit)) }).unwrap();
    let p = streams(&Document::open(Arc::new(stamps)).unwrap()).into_iter().find(|s| s.contains("(Page 1)")).unwrap();
    assert_eq!(p.matches(" Do Q").count(), 1);
    let fields = impose(&doc, &Settings { content: Content::FormFieldsOnly, ..settings(vec![1], Layout::Size(SizeMode::Fit)) }).unwrap();
    assert!(!streams(&Document::open(Arc::new(fields)).unwrap()).join("").contains("(Page 2)"));
    // The output is a fresh file: the unused source pages are gone.
    let one = impose(&doc, &settings(vec![1], Layout::Size(SizeMode::Fit))).unwrap();
    let s = streams(&Document::open(Arc::new(one)).unwrap()).join("\n");
    assert!(s.contains("(Page 2)") && !s.contains("(Page 1)"));
}

#[test]
fn spooler_arguments_and_printer_list() {
    let out = "printer Office_Laser is idle.  enabled since Thu Oct  1 09:00:00 2026\nprinter Label_Writer disabled since …\nsystem default destination: Office_Laser\n";
    assert_eq!(
        parse_lpstat(out),
        [spool::Printer { name: "Office_Laser".into(), default: true }, spool::Printer { name: "Label_Writer".into(), default: false }]
    );
    assert!(parse_lpstat("lpstat: No destinations added.\nno system default destination\n").is_empty());
    let job = Job {
        printer: Some("Office_Laser".into()),
        copies: 3,
        collate: false,
        duplex: Duplex::LongEdge,
        grayscale: true,
        title: "memo.pdf".into(),
        options: Vec::new(),
    };
    assert_eq!(
        lp_args(&job).join(" "),
        "-d Office_Laser -n 3 -t memo.pdf -o collate=false -o sides=two-sided-long-edge -o print-color-mode=monochrome -o fit-to-page=false"
    );
    assert_eq!(lp_args(&Job::default())[0], "-n", "no -d: the default printer");
}

#[test]
fn lpstat_output_is_untranslated() {
    // A localized lpstat (here Polish) is unreadable to parse_lpstat...
    assert!(parse_lpstat("drukarka Office_Laser jest bezczynna.\ndomyślny cel systemowy: Office_Laser\n").is_empty());
    // ...so the command must force the C locale, including the SOFTWARE switch macOS CUPS needs.
    let cmd = spool::lpstat_command();
    let envs: Vec<_> = cmd.get_envs().map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()))).collect();
    for key in ["LC_ALL", "LANG"] {
        assert!(envs.contains(&(key.into(), Some("C".into()))), "{key}=C missing: {envs:?}");
    }
    assert!(envs.iter().any(|(k, v)| k == "SOFTWARE" && v.as_deref().is_some_and(|v| !v.is_empty())), "SOFTWARE missing: {envs:?}");
}

#[test]
fn driverless_printers_join_the_list_without_a_queue() {
    // A driverless network printer (IPP, discovered on the LAN) has no queue, so `lpstat -p`
    // never lists it — yet GTK's print dialog shows it and `lp` prints to it through a temporary
    // queue. Only `lpstat -e` names it, and the Print dialog used to show Save as PDF alone.
    let queues = "printer Office_Laser is idle.  enabled since Thu Oct  1 09:00:00 2026\nsystem default destination: Office_Laser\n";
    assert_eq!(
        spool::printers_parsed(queues, Some("Basement_Color\nOffice_Laser\n")),
        [spool::Printer { name: "Office_Laser".into(), default: true }, spool::Printer { name: "Basement_Color".into(), default: false },],
        "queues first, then the driverless destination; the queue listed twice is not doubled"
    );
    // An old spooler without `-e` (CUPS < 1.7) keeps its queues; only the driverless names are lost.
    assert_eq!(spool::printers_parsed(queues, None), [spool::Printer { name: "Office_Laser".into(), default: true }]);
    // The default can itself be driverless (`lpoptions -d` names a destination no queue exists
    // for): it keeps its marker, so the dialog preselects the user's default, not the first queue.
    assert_eq!(
        spool::printers_parsed("system default destination: Basement_Color\n", Some("Basement_Color\n")),
        [spool::Printer { name: "Basement_Color".into(), default: true }],
        "a default destination without a queue is still the default"
    );
    assert!(spool::parse_lpstat_e("\n \n").is_empty(), "no destinations is empty, not [\"\"]");
    assert_eq!(spool::parse_lpstat_e("A\r\nB \n"), ["A".to_string(), "B".to_string()], "CRLF and stray spaces are trimmed");
}

fn cut_stack(cols: usize, rows: usize) -> Layout {
    Layout::Multiple { cols, rows, order: PageOrder::CutStack, border: false, auto_rotate: false }
}

#[test]
fn cut_stack_keeps_piles_in_order_and_blanks_in_place() {
    let sizes = vec![(200.0, 300.0); 10];
    let sheets = layout(&sizes, &settings((0..10).collect(), cut_stack(2, 2))).unwrap();
    assert_eq!(sheet_pages(&sheets), [vec![0, 3, 6, 9], vec![1, 4, 7], vec![2, 5, 8]]);
    for s in &sheets {
        assert_eq!(s.lines, sheets[0].lines, "identical cuts even with blank cells");
        assert_eq!(s.lines.len(), 4);
        for (pl, first) in s.placed.iter().zip(&sheets[0].placed) {
            assert_eq!(pl.matrix, first.matrix, "every pile stays in its cell");
        }
    }
    assert!(sheets[0].placed[1].matrix.0[4] > sheets[0].placed[0].matrix.0[4]);
    assert!(sheets[0].placed[2].matrix.0[5] < sheets[0].placed[0].matrix.0[5]);
    // Selection and reverse order are preserved, including repeated source pages.
    let selected = vec![9, 5, 5, 2, 0];
    let sheets = layout(&sizes, &settings(selected, cut_stack(2, 2))).unwrap();
    assert_eq!(sheet_pages(&sheets), [vec![9, 5, 0], vec![5, 2]]);
    assert_eq!(sheets[1].placed[1].matrix, sheets[0].placed[1].matrix);
}

#[test]
fn cutting_and_stacking_recovers_every_selected_page() {
    // Simulate the actual operation using cell positions, not the imposition formula.
    for (cols, rows) in [(1, 1), (1, 2), (2, 2), (2, 3), (3, 3), (4, 4)] {
        for orientation in [Orientation::Auto, Orientation::Portrait, Orientation::Landscape] {
            for n in 1..=37 {
                let sizes = vec![(200.0, 300.0); n];
                let selected: Vec<usize> = (0..n).rev().collect();
                let s = Settings { orientation, ..settings(selected.clone(), cut_stack(cols, rows)) };
                let sheets = layout(&sizes, &s).unwrap();
                let mut piles: std::collections::BTreeMap<(i64, i64), Vec<usize>> = Default::default();
                for sheet in &sheets {
                    for pl in &sheet.placed {
                        // Equal source sizes, so the page origins identify row/column.
                        let [_, _, _, _, x, y] = pl.matrix.0;
                        piles.entry((-(y * 100.0).round() as i64, (x * 100.0).round() as i64)).or_default().push(pl.page);
                    }
                }
                let restacked: Vec<usize> = piles.into_values().flatten().collect();
                assert_eq!(restacked, selected, "{cols}x{rows}, {n} pages, {orientation:?}");
            }
        }
    }
}

#[test]
fn multiple_rejects_hostile_grids_without_panicking() {
    let sizes = [(200.0, 300.0)];
    for (cols, rows) in [(usize::MAX, 2), (2, usize::MAX), (0, 2), (1, 0), (257, 1)] {
        let mode = Layout::Multiple { cols, rows, order: PageOrder::Horizontal, border: false, auto_rotate: false };
        assert!(matches!(layout(&sizes, &settings(vec![0], mode)), Err(PrintError::Invalid(_))));
    }
    let tiny = Settings { paper: (72.0, 72.0), ..settings(vec![0], cut_stack(16, 16)) };
    assert!(matches!(layout(&sizes, &tiny), Err(PrintError::Invalid(_))));
    for size in [(0.0, 300.0), (200.0, f64::NAN), (f64::INFINITY, 300.0), (f64::from_bits(1), f64::from_bits(1))] {
        assert!(matches!(layout(&[size], &settings(vec![0], cut_stack(2, 2))), Err(PrintError::Invalid(_))));
    }
}

/// Set on the child process when a test runs this test binary as a stand-in for `lp`.
const STAND_IN_LP: &str = "PDFCRAFT_STAND_IN_LP";

/// Not a test of its own: [`stand_in_lp`] re-runs this binary with only this test selected, and it
/// then plays `lp`. It records its arguments and stdin in the folder named by [`STAND_IN_LP`], or,
/// when the folder is `refuse`, exits without reading stdin, the way `lp` refuses an unknown
/// printer.
#[test]
fn stand_in_for_lp() {
    let Some(record) = std::env::var_os(STAND_IN_LP) else { return };
    if record == "refuse" {
        eprintln!("lp: The printer or class does not exist.");
        std::process::exit(1);
    }
    let record = std::path::PathBuf::from(record);
    let args: Vec<String> = std::env::args().skip_while(|a| a != "--").skip(1).collect();
    let mut stdin = Vec::new();
    std::io::Read::read_to_end(&mut std::io::stdin(), &mut stdin).unwrap();
    std::fs::write(record.join("args"), args.join(" ")).unwrap();
    std::fs::write(record.join("stdin"), stdin).unwrap();
}

/// A spooler command that runs [`stand_in_for_lp`] with `record` (see there).
fn stand_in_lp(record: &std::ffi::OsStr) -> std::process::Command {
    let mut c = std::process::Command::new(std::env::current_exe().unwrap());
    c.args(["--exact", "tests::stand_in_for_lp", "--nocapture", "--"]).env(STAND_IN_LP, record);
    c
}

#[test]
fn print_jobs_reach_lp_on_stdin_never_through_a_shared_temp_folder() {
    // The job used to be written to `<temp>/pdfcraft-print-<pid>/job-<time>.pdf` and handed to `lp`
    // by name. In a shared /tmp another local user can predict that folder, create it first (or
    // plant a symlink there) and so read every printed document or swap the file before `lp`
    // reads it. Piping the job to `lp` leaves nothing on disk.
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let record =
        std::env::temp_dir().join(format!("pdfcraft-print-test-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    std::fs::create_dir(&record).unwrap();
    // Bigger than any pipe buffer, so the spooler must read while the job is still being written.
    let pdf: Vec<u8> = b"%PDF-1.7 synthetic print job\n".iter().copied().cycle().take(3 << 20).collect();
    let job = Job { title: "memo.pdf".into(), ..Job::default() };
    let sent = spool::submit_via(stand_in_lp(record.as_os_str()), &pdf, &job);
    let args = std::fs::read_to_string(record.join("args"));
    let stdin = std::fs::read(record.join("stdin"));
    let _ = std::fs::remove_dir_all(&record);
    assert!(sent.is_ok(), "{sent:?}");
    assert_eq!(
        args.unwrap(),
        "-n 1 -t memo.pdf -o collate=true -o sides=one-sided -o fit-to-page=false",
        "no file argument: lp reads the job from stdin"
    );
    assert!(stdin.unwrap() == pdf, "lp receives the whole job on stdin");
    let predictable = std::env::temp_dir().join(format!("pdfcraft-print-{}", std::process::id()));
    assert!(!predictable.exists(), "nothing is created at {predictable:?}, a name other local users can predict");
}

#[test]
fn a_refused_print_job_reports_the_spoolers_message() {
    // `lp` refuses an unknown printer without reading the job; the user sees why, not a broken pipe.
    let pdf = vec![b'%'; 3 << 20];
    let err = spool::submit_via(stand_in_lp("refuse".as_ref()), &pdf, &Job::default());
    assert_eq!(err, Err(PrintError::Spool("lp: The printer or class does not exist.".into())));
}

#[test]
fn the_spoolers_reply_drops_the_file_count_of_a_job_sent_on_stdin() {
    assert_eq!(
        spool::job_message(
            b"request id is Office_Laser-12 (0 file(s))
"
        ),
        "request id is Office_Laser-12"
    );
    assert_eq!(
        spool::job_message(
            b"request id is Office_Laser-13 (1 file(s))
"
        ),
        "request id is Office_Laser-13 (1 file(s))"
    );
    assert_eq!(spool::job_message(b""), "");
}

#[test]
fn windows_printers_carry_one_default_and_keep_their_names() {
    // Windows marks the system default with a `*` no queue name may contain; names hold spaces,
    // parentheses and non-ASCII letters, and the lines end with CRLF.
    let printers = parse_windows_printers("*Microsoft Print to PDF\r\nHP LaserJet 400 (Copy 2)\r\nКопир Canon iR\r\n\r\n*XPS via USB\r\n");
    assert_eq!(printers[0], Printer { name: "Microsoft Print to PDF".into(), default: true });
    assert_eq!(printers[1], Printer { name: "HP LaserJet 400 (Copy 2)".into(), default: false });
    assert_eq!(printers[2], Printer { name: "Копир Canon iR".into(), default: false });
    // A spooler marking two defaults: the first still wins, so the dialog has one to preselect.
    assert_eq!(printers[3], Printer { name: "XPS via USB".into(), default: false });
    assert_eq!(parse_windows_printers("\r\n"), Vec::new());
}

/// The print script's progress lines, and only those, are read as progress: anything else it
/// says is the spooler's message.
#[test]
fn the_print_scripts_progress_lines_are_read_and_nothing_else() {
    assert_eq!(spool::parse_progress("progress 0 3"), Some((0, 3)));
    assert_eq!(spool::parse_progress("progress 3 3\r"), Some((3, 3)));
    for other in ["spooled 3 sheet(s)", "progress", "progress 1", "progress 4 3", "progress 1 3 x", "progress a 3", "Progress 1 3", ""] {
        assert_eq!(spool::parse_progress(other), None, "{other:?}");
    }
}

#[test]
fn the_windows_job_is_the_environment_of_the_print_script() {
    let path = std::path::Path::new("C:\\Users\\me\\AppData\\Local\\Temp\\pdfcraft-print-42.pdf");
    let job = Job {
        printer: Some("HP \"Laser\" Jet".into()),
        copies: 3,
        duplex: Duplex::ShortEdge,
        grayscale: true,
        title: "Angebot.pdf".into(),
        ..Job::default()
    };
    let vars = windows_env_vars(path, &job);
    let env = |key: &str| vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).unwrap();
    assert_eq!(env("PDFCRAFT_PRINT_FILE"), "C:\\Users\\me\\AppData\\Local\\Temp\\pdfcraft-print-42.pdf");
    // Quotes and spaces in a printer name survive because nothing goes on the command line.
    assert_eq!(env("PDFCRAFT_PRINT_PRINTER"), "HP \"Laser\" Jet");
    assert_eq!(env("PDFCRAFT_PRINT_COPIES"), "3");
    assert_eq!(env("PDFCRAFT_PRINT_COLLATE"), "1");
    assert_eq!(env("PDFCRAFT_PRINT_DUPLEX"), "short-edge");
    assert_eq!(env("PDFCRAFT_PRINT_GRAYSCALE"), "1");
    assert_eq!(env("PDFCRAFT_PRINT_TITLE"), "Angebot.pdf");
    // No printer chosen means the system default: an empty name, not the word "default".
    let one = |j: &Job, key: &str| windows_env_vars(path, j).into_iter().find(|(k, _)| k == key).unwrap().1;
    assert_eq!(one(&Job::default(), "PDFCRAFT_PRINT_PRINTER"), "");
    assert_eq!(one(&Job { duplex: Duplex::Off, ..Job::default() }, "PDFCRAFT_PRINT_DUPLEX"), "off");
    assert_eq!(one(&Job { duplex: Duplex::LongEdge, ..Job::default() }, "PDFCRAFT_PRINT_DUPLEX"), "long-edge");
}

/// Run a PowerShell script for the Windows-only tests: its stdout, or everything it said when
/// it failed — the tests print that as the skip reason.
#[cfg(windows)]
fn windows_powershell(script: &str, envs: &[(&str, &str)]) -> Result<String, String> {
    let mut c = std::process::Command::new("powershell");
    c.args(["-NoProfile", "-NonInteractive", "-Command", script]);
    for (k, v) in envs {
        c.env(k, v);
    }
    match c.output() {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
        Ok(o) => Err(format!("{} {}", String::from_utf8_lossy(&o.stderr).trim(), String::from_utf8_lossy(&o.stdout).trim())),
        Err(e) => Err(e.to_string()),
    }
}

/// Windows: the printer list comes from the spooler. A bare machine may have no printer; the
/// list that exists has real names and at most one default.
#[cfg(windows)]
#[test]
fn windows_lists_the_printers_the_system_knows() {
    let printers = spool::printers();
    assert!(printers.iter().all(|p| !p.name.trim().is_empty()), "{printers:?}");
    assert!(printers.iter().filter(|p| p.default).count() <= 1, "{printers:?}");
}

/// Windows: the embedded print script is valid PowerShell, checked on every machine — including
/// runners that have no printer to print to.
#[cfg(windows)]
#[test]
fn the_windows_print_script_parses() {
    let path = std::env::temp_dir().join(format!("pdfcraft-print-script-{}.ps1", std::process::id()));
    std::fs::write(&path, spool::WINDOWS_PRINT_SCRIPT).unwrap();
    let errors = windows_powershell(
        "$tokens = $null; $errors = $null; [System.Management.Automation.Language.Parser]::ParseFile($env:PDFCRAFT_TEST_SCRIPT, [ref]$tokens, [ref]$errors) > $null; $errors.Count",
        &[("PDFCRAFT_TEST_SCRIPT", path.to_str().unwrap())],
    );
    let _ = std::fs::remove_file(&path);
    assert_eq!(errors.as_deref().map(str::trim), Ok("0"), "parse errors: {errors:?}");
}

/// Windows, end to end: the script renders the job's sheets with Windows.Data.Pdf on any
/// machine — CI included, no printer needed (the dry run writes the PNGs to the temporary
/// folder) — and, where a paused test printer can be added, a real job lands in its queue.
/// Its driver is the inbox "Generic / Text Only"; a machine that cannot add one (no rights, no
/// driver) skips that half, as the CUPS tests skip where CUPS isn't running.
#[cfg(windows)]
#[test]
fn the_print_script_renders_the_job_and_reaches_the_spooler() {
    let pdf = impose(&fixture(2), &settings(vec![0, 1], Layout::Size(SizeMode::Fit))).unwrap();
    // The printer name is deliberately wrong: the dry run stops before the driver is touched.
    let job = Job { printer: Some("no such printer".into()), title: "test.pdf".into(), ..Job::default() };
    let dry = spool::windows_submit(&pdf, &job, &[("PDFCRAFT_PRINT_DRYRUN", "1")], &mut |_, _| {});
    assert_eq!(dry.as_deref(), Ok("rendered 2 sheet(s)"), "{dry:?}");

    const NAME: &str = "PdfCraft Test Print";
    let added = windows_powershell(
        &format!(
            "if (-not (Get-Printer -Name '{NAME}' -ErrorAction SilentlyContinue)) {{ Add-Printer -Name '{NAME}' -DriverName 'Generic / Text Only' -PortName 'FILE:' }}; Set-Printer -Name '{NAME}' -IsPaused $true"
        ),
        &[],
    );
    if let Err(why) = added {
        eprintln!("skipping the real print: the test printer could not be prepared: {why}");
        return;
    }
    let job = Job { printer: Some(NAME.into()), title: "test.pdf".into(), ..Job::default() };
    let mut reported = Vec::new();
    let sent = spool::submit_with_progress(&pdf, &job, &mut |sent, sheets| reported.push((sent, sheets)));
    let queued = windows_powershell(
        "if (Get-PrintJob -Name 'PdfCraft Test Print') { (Get-PrintJob -Name 'PdfCraft Test Print' | Measure-Object).Count } else { 0 }",
        &[],
    );
    let cleaned = windows_powershell("Get-PrintJob -Name 'PdfCraft Test Print' | Remove-PrintJob; Remove-Printer -Name 'PdfCraft Test Print'", &[]);
    assert!(sent.is_ok(), "{sent:?}");
    assert_eq!(reported, [(0, 2), (1, 2), (2, 2)], "each sheet is reported as it reaches the spooler");
    assert_eq!(queued.as_deref().map(str::trim), Ok("1"), "the job is in the paused printer's queue: {queued:?}");
    assert!(cleaned.is_ok(), "the test printer is removed again: {cleaned:?}");
}

/// A PPD shaped like a Fiery's: installable options, multi-line PostScript in the choices, Latin-1
/// labels, options the Print dialog sets itself.
const FIERY_PPD: &[u8] = b"*PPD-Adobe: \"4.3\"\n\
*LanguageEncoding: ISOLatin1\n\
*OpenGroup: InstallableOptions/Installable Options\n\
*OpenUI *EFFinisher/Finisher option: PickOne\n\
*DefaultEFFinisher: False\n\
*EFFinisher False/Not installed: \"\"\n\
*EFFinisher SingleStapler/Single stapler: \"\"\n\
*CloseUI: *EFFinisher\n\
*CloseGroup: InstallableOptions\n\
*OpenGroup: FPPaperSource/Media\n\
*OpenUI *InputSlot/Paper tray: PickOne\n\
*OrderDependency: 20.0 AnySetup *InputSlot\n\
*DefaultInputSlot: AutoSelect\n\
*InputSlot AutoSelect/Auto tray select: \"\n\
userdict /XJXEFIsetpageproperties known\n\
{ << /XJXsettrayselV2 [ 7 ] >> XJXEFIsetpageproperties } if\"\n\
*End\n\
*InputSlot ManualFeed/Bypass tray: \"\n\
{ pop 2 XJXsettrayselV2 } if\"\n\
*End\n\
*InputSlot Tray2/Tray 2: \"\"\n\
*fr.InputSlot Tray2/Bac 2: \"\"\n\
*CloseUI: *InputSlot\n\
*OpenUI *EFMediaType/Paper type: PickOne\n\
*DefaultEFMediaType: Plain\n\
*EFMediaType Plain/Plain: \"\"\n\
*EFMediaType Heavy1/Thick 1 (106\xad163 g/m\xb2): \"\"\n\
*CloseUI: *EFMediaType\n\
*CloseGroup: FPPaperSource\n\
*OpenUI *PageSize/Page size: PickOne\n\
*DefaultPageSize: A4\n\
*PageSize A4/A4: \"\"\n\
*CloseUI: *PageSize\n\
*OpenUI *EFRaster/Print queue action: PickOne\n\
*DefaultEFRaster: Bogus\n\
*EFRaster False/Print: \"\"\n\
*EFRaster Hold: \"\"\n\
*CloseUI: *EFRaster\n";

#[test]
fn printer_options_come_from_the_ppd() {
    let options = spool::parse_ppd(&spool::ppd_text(FIERY_PPD));
    let keys: Vec<&str> = options.iter().map(|o| o.key.as_str()).collect();
    assert_eq!(keys, ["InputSlot", "EFMediaType", "EFRaster"], "no installable options, no PageSize");
    let tray = &options[0];
    assert_eq!((tray.label.as_str(), tray.group.as_str(), tray.default.as_str()), ("Paper tray", "Media", "AutoSelect"));
    assert_eq!(
        tray.choices,
        [("AutoSelect".into(), "Auto tray select".into()), ("ManualFeed".into(), "Bypass tray".into()), ("Tray2".into(), "Tray 2".into())],
        "PostScript lines and translations (*fr.InputSlot) are not choices"
    );
    assert_eq!(options[1].choices[1].1, "Thick 1 (106\u{ad}163 g/m²)", "Latin-1 labels");
    // A default that isn't one of the choices falls back to the first; a choice without a label shows its keyword.
    assert_eq!((options[2].default.as_str(), options[2].choices[1].1.as_str()), ("False", "Hold"));
    assert!(spool::parse_ppd("*OpenUI *Broken\n*Broken A/B\n").is_empty(), "unclosed or malformed blocks are skipped");
}

#[test]
fn printer_option_defaults_and_job_arguments() {
    let current =
        spool::parse_lpoptions("InputSlot/Paper tray: AutoSelect *Tray2 ManualFeed\nEFRaster/Print queue action: *False Hold\nbroken line\n");
    assert_eq!(current, [("InputSlot".to_string(), "Tray2".to_string()), ("EFRaster".to_string(), "False".to_string())]);
    let job = Job {
        options: vec![
            ("InputSlot".into(), "Tray2".into()),
            ("EFMediaType".into(), "Heavy1".into()),
            ("Duplex".into(), "DuplexTumble".into()),
            ("Bad Key".into(), "x".into()),
            ("EFRaster".into(), "a=b".into()),
        ],
        ..Job::default()
    };
    let args = lp_args(&job).join(" ");
    assert!(args.ends_with("-o fit-to-page=false -o InputSlot=Tray2 -o EFMediaType=Heavy1"), "{args}");
    assert!(spool::printer_options("../../etc/passwd").is_empty(), "not a queue name");
}
