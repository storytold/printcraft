//! Page layouts, zoom modes and link navigation, checked by where pages land on screen.

use egui::{Key, Modifiers, MouseWheelUnit, TouchPhase};
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

/// Three 300×400 pt pages. Page 1 links to page 3.
const PAGES: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [6 0 R] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
5 0 obj << /Type /Page /Parent 2 0 R >> endobj
6 0 obj << /Type /Annot /Subtype /Link /Rect [50 300 250 350] /Border [0 0 0] /Dest [5 0 R /Fit] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// Tests that render pixels (`h.render()`) take this first, so only one wgpu device compiles
/// shaders at a time. On machines without a GPU, wgpu falls back to Microsoft's WARP, whose ARM64
/// pixel-shader JIT crashes (access violation in `d3d10warp!PixelJitProgram::ClassifyVars`) when
/// two devices compile at once, as on GitHub's Windows 11 ARM64 runner. Hold it for the whole
/// test (declared before the harness, so it's released after the device is dropped).
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu() -> std::sync::MutexGuard<'static, ()> {
    // A test that panicked while holding it leaves nothing to clean up.
    GPU.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn harness(options: &'static [(&'static str, &'static str)]) -> Harness<'static, PdfCraftApp> {
    harness_stepping(1.0 / 4.0, options)
}

/// [`harness`] with frames `step_dt` seconds apart (a quarter second is kittest's default).
fn harness_stepping(step_dt: f32, options: &'static [(&'static str, &'static str)]) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(step_dt).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("pages.pdf", None, PAGES.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        for (k, v) in options {
            app.set_option(k, v).unwrap();
        }
        app
    });
    h.run_steps(8);
    h
}

fn rect(h: &Harness<'static, PdfCraftApp>, page: usize) -> Option<egui::Rect> {
    h.state().views[0].page_screen_rect(page)
}

/// Two pages of different sizes: 300×400 then 600×300.
const MIXED: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 600 300] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn mixed_harness(options: &'static [(&'static str, &'static str)]) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("mixed.pdf", None, MIXED.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        for (k, v) in options {
            app.set_option(k, v).unwrap();
        }
        app
    });
    h.run_steps(8);
    h
}

/// A vertical wheel event; negative `dy` scrolls down.
fn wheel(unit: MouseWheelUnit, dy: f32, phase: TouchPhase) -> egui::Event {
    egui::Event::MouseWheel { unit, delta: egui::vec2(0.0, dy), phase, modifiers: Modifiers::NONE }
}

#[cfg(target_os = "linux")]
#[test]
fn middle_button_scrolling_keeps_page_colours_in_both_themes() {
    use egui_kittest::kittest::Queryable;
    let _gpu = gpu();
    for theme in ["light", "dark"] {
        for organize in [false, true] {
            let mut h = harness(&[("zoom", "50")]);
            h.state_mut().set_option("theme", theme).unwrap();
            h.state_mut().set_option("organize", if organize { "on" } else { "off" }).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                assert!(
                    std::time::Instant::now() < deadline,
                    "render readiness timed out: theme={theme}, organize={organize}, errors={:?}",
                    h.state().views[0].page_errors()
                );
                h.run_steps(2);
                if !h.state().render_pending() {
                    // Paint one fresh frame after all requested rasters have arrived.
                    h.run_steps(1);
                    if h.state().render_pending() {
                        continue;
                    }
                    assert!(
                        h.state().views[0].page_errors().is_empty(),
                        "page render errors: theme={theme}, organize={organize}, errors={:?}",
                        h.state().views[0].page_errors()
                    );
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let page = if organize { h.get_by_label("Page 1").rect() } else { rect(&h, 0).unwrap() };
            let at = page.center();
            let ppp = h.ctx.pixels_per_point();
            let before = *h.render().unwrap().get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32);
            assert_eq!(
                before,
                image::Rgba([255, 255, 255, 255]),
                "the synthetic page is white: theme={theme}, organize={organize}, page={page:?}, ppp={ppp}"
            );
            let anchor = h.state().views[0].viewport_rect().center();
            h.event(egui::Event::PointerMoved(anchor));
            h.event(egui::Event::PointerButton { pos: anchor, button: egui::PointerButton::Middle, pressed: true, modifiers: Modifiers::NONE });
            h.run_steps(1);
            h.event(egui::Event::PointerButton { pos: anchor, button: egui::PointerButton::Middle, pressed: false, modifiers: Modifiers::NONE });
            h.run_steps(1);
            assert!(h.state().views[0].auto_scrolling());
            let during = *h.render().unwrap().get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32);
            assert_eq!(during, before, "scrolling preserves page colours: theme={theme}, organize={organize}");
        }
    }
}

#[test]
fn continuous_layout_stacks_pages_vertically() {
    let mut h = harness(&[("layout", "continuous"), ("zoom", "50")]);
    h.run_steps(4);
    let (a, b) = (rect(&h, 0).expect("page 1 on screen"), rect(&h, 1).expect("page 2 on screen"));
    assert!(b.min.y > a.max.y, "page 2 below page 1: {a:?} {b:?}");
    assert!((a.center().x - b.center().x).abs() < 1.0, "same column");
}

#[test]
fn two_up_layout_places_pages_side_by_side() {
    let mut h = harness(&[("layout", "two-up"), ("zoom", "50")]);
    h.run_steps(4);
    let (a, b) = (rect(&h, 0).expect("page 1"), rect(&h, 1).expect("page 2"));
    assert!((a.min.y - b.min.y).abs() < 1.0, "same row: {a:?} {b:?}");
    assert!(b.min.x > a.max.x, "page 2 right of page 1");
    let c = rect(&h, 2).expect("page 3 on the next row");
    assert!(c.min.y > a.max.y);
}

#[test]
fn two_page_view_shows_one_spread_at_a_time_until_continuous() {
    use pdfcraft_ui_egui::canvas::PageLayout;
    // Two-page view without continuous scrolling shows just the current spread; the continuous
    // checkbox reveals the rest (#752).
    let mut h = harness(&[("layout", "two-page"), ("zoom", "50")]);
    h.run_steps(4);
    assert_eq!(h.state().views[0].layout, PageLayout::TwoUp);
    assert!(!h.state().views[0].continuous(), "`two-page` is the non-scrolling two-page display");
    assert!(rect(&h, 0).is_some() && rect(&h, 1).is_some(), "the first spread is on screen");
    assert!(rect(&h, 2).is_none(), "the next spread is not shown until paged or scrolled");
    // Turning on continuous scrolling lays every spread out in one column.
    h.state_mut().set_option("continuous", "on").unwrap();
    h.run_steps(4);
    assert!(rect(&h, 2).is_some(), "continuous scrolling reveals the later spreads");
    assert_eq!(h.state().views[0].layout, PageLayout::TwoUp, "the base layout is unchanged");
}

#[test]
fn single_page_layout_shows_one_page_at_a_time() {
    let mut h = harness(&[("layout", "single"), ("zoom", "50")]);
    h.run_steps(4);
    assert!(rect(&h, 0).is_some() && rect(&h, 1).is_none(), "only page 1");
    h.state_mut().set_option("page", "2").unwrap();
    h.run_steps(4);
    assert!(rect(&h, 0).is_none() && rect(&h, 1).is_some(), "only page 2");
}

#[test]
fn single_page_wheel_pans_instead_when_zoomed_in() {
    // The zoomed-in guard: with room to pan, the wheel pans and must not turn pages.
    let mut h = harness(&[("layout", "single"), ("zoom", "200")]);
    h.run_steps(4);
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(1);
    h.event(wheel(MouseWheelUnit::Point, -60.0, TouchPhase::Move));
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 0, "zoomed-in wheel pans, it does not turn the page");
}

#[test]
fn wheel_turns_one_page_per_notch_and_per_trackpad_swipe() {
    // At a real frame rate: brisk notches each turn a page, and one trackpad swipe turns one
    // page, its momentum included (macOS sends the momentum as a second Start…End).
    let mut h = harness_stepping(1.0 / 60.0, &[("layout", "single"), ("zoom", "50")]);
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(2);
    for _ in 0..2 {
        h.event(wheel(MouseWheelUnit::Line, -1.0, TouchPhase::Move));
        h.run_steps(5);
    }
    h.run_steps(30);
    assert_eq!(h.state().views[0].current, 2, "two notches 80 ms apart turn two pages");
    let mut send = |phase, dy| {
        h.event(wheel(MouseWheelUnit::Point, dy, phase));
        h.step();
    };
    send(TouchPhase::Start, 0.0);
    for _ in 0..6 {
        send(TouchPhase::Move, 25.0);
    }
    send(TouchPhase::End, 0.0);
    send(TouchPhase::Start, 0.0);
    let mut d = 25.0;
    while d > 0.5 {
        send(TouchPhase::Move, d);
        d *= 0.92;
    }
    send(TouchPhase::End, 0.0);
    h.run_steps(30);
    assert_eq!(h.state().views[0].current, 1, "one swipe back turns one page back");
}

#[test]
fn precise_touchpad_wheel_scrolls_one_to_one() {
    // A precision touchpad's pixel delta scrolls the page by that amount in the same frame,
    // instead of egui easing it over several frames so the page lags the finger (#759). A single
    // 200 px precise event is well over egui's 8 px smoothing threshold, so without the fix only
    // a fraction would land on the first frame.
    let mut h = harness_stepping(1.0 / 60.0, &[("layout", "continuous"), ("zoom", "100")]);
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(2);
    let before = rect(&h, 0).expect("page 1 on screen").min.y;
    h.event(wheel(MouseWheelUnit::Point, -200.0, TouchPhase::Move));
    // A few frames for the event to reach the ScrollArea and the new offset to be drawn. The
    // page jumps the full 200 px at once; an eased path would ramp there over ~0.1 s (6 frames).
    h.run_steps(4);
    let moved = before - rect(&h, 0).expect("page 1 still laid out").min.y;
    assert!(moved > 190.0, "a 200 px precise swipe scrolls ~200 px at once, not an eased fraction: moved {moved}");
    // And it stops with the finger: idle frames must not drift on a leftover easing tail.
    let settled = rect(&h, 0).unwrap().min.y;
    h.run_steps(20);
    let drift = settled - rect(&h, 0).unwrap().min.y;
    assert!(drift.abs() < 2.0, "no easing tail after the swipe: drifted {drift}");
}

#[test]
fn page_display_commands_switch_layouts() {
    let mut h = harness(&[]);
    h.state_mut().execute("view.layout.single");
    assert_eq!(h.state().views[0].layout, pdfcraft_ui_egui::canvas::PageLayout::Single);
    // The cover toggle refuses outside two-page view instead of arming a hidden flag.
    assert!(!h.state().views[0].cover);
    assert!(!h.state_mut().execute("view.layout.cover"));
    assert!(!h.state().views[0].cover);
    h.state_mut().execute("view.layout.two_up");
    assert_eq!(h.state().views[0].layout, pdfcraft_ui_egui::canvas::PageLayout::TwoUp);
    assert!(h.state_mut().execute("view.layout.cover"));
    assert!(h.state().views[0].cover, "the cover toggle flips");
    assert!(h.state_mut().execute("view.layout.cover"));
    assert!(!h.state().views[0].cover, "toggling twice restores");
    // Continuous scrolling is an independent toggle over the base layout (#752).
    assert!(h.state().views[0].continuous(), "the default view scrolls continuously");
    h.state_mut().execute("view.continuous");
    h.run_steps(3);
    assert!(!h.state().views[0].continuous(), "the checkbox turns continuous scrolling off");
    assert_eq!(h.state().views[0].layout, pdfcraft_ui_egui::canvas::PageLayout::TwoUp, "toggling scrolling leaves the base layout");
    h.state_mut().execute("view.layout.single");
    assert_eq!(h.state().views[0].layout, pdfcraft_ui_egui::canvas::PageLayout::Single);
    assert!(!h.state().views[0].continuous(), "switching the base layout leaves scrolling off");
}

#[test]
fn layout_option_rejects_typos() {
    let mut h = harness(&[]);
    assert!(h.state_mut().set_option("layout", "singel").is_err());
    assert!(h.state_mut().set_option("default-layout", "bogus").is_err());
    assert!(h.state_mut().set_option("cover", "maybe").is_err());
    assert_eq!(h.state().views[0].layout, pdfcraft_ui_egui::canvas::PageLayout::Single);
    assert!(h.state().views[0].continuous(), "a rejected typo leaves the default display unchanged");
    assert!(h.state_mut().set_option("cover", "on").is_err(), "cover needs two-page view first");
    h.state_mut().set_option("layout", "two-up").unwrap();
    h.state_mut().set_option("cover", "on").unwrap();
    assert!(h.state().views[0].cover);
    h.state_mut().set_option("cover", "off").unwrap();
    assert!(!h.state().views[0].cover);
}

#[test]
fn view_options_without_a_document() {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    assert!(app.set_option("default-layout", "single").is_ok(), "the default needs no document");
    assert_eq!(app.view_defaults.layout, pdfcraft_ui_egui::canvas::PageLayout::Single);
    assert!(app.set_option("cover", "on").is_err(), "cover needs an open document");
    assert!(app.set_option("layout", "single").is_err(), "layout needs an open document");
}

#[test]
fn default_page_display_is_used_for_new_documents() {
    use pdfcraft_ui_egui::canvas::PageLayout;
    // The factory default is single page, scrolling continuously, without snap jumps.
    assert_eq!(PdfCraftApp::new().view_defaults.layout, PageLayout::Single);
    assert!(PdfCraftApp::new().view_defaults.continuous);
    let mut h = harness(&[]);
    h.state_mut().set_option("default-layout", "single").unwrap();
    h.state_mut().open_bytes("other.pdf", None, PAGES.to_vec()).expect("opens");
    assert_eq!(h.state().views[1].layout, PageLayout::Single);
    // So do documents the app creates itself (blank, combined, extracted, from images…).
    assert!(h.state_mut().execute("create.blank"));
    assert_eq!(h.state().views[2].layout, PageLayout::Single);
    // Persisted preferences survive a restart.
    let saved = h.state().persist();
    let mut fresh = PdfCraftApp::new();
    fresh.set_option("language", "en").unwrap();
    fresh.restore(&saved);
    assert_eq!(fresh.view_defaults.layout, PageLayout::Single);
    // Garbage keeps the previous default; casing is forgiven; legacy files lack the key.
    fresh.restore(r#"{"default_layout":"bogus"}"#);
    assert_eq!(fresh.view_defaults.layout, PageLayout::Single);
    fresh.restore(r#"{"default_layout":"TWO-UP"}"#);
    assert_eq!(fresh.view_defaults.layout, PageLayout::TwoUp);
    let mut legacy = PdfCraftApp::new();
    legacy.set_option("language", "en").unwrap();
    legacy.restore("{}");
    assert_eq!(legacy.view_defaults.layout, PageLayout::Single);
    assert!(legacy.view_defaults.continuous, "a legacy file without the key keeps the factory default");
}

#[test]
fn default_zoom_is_used_for_new_documents() {
    use pdfcraft_ui_egui::canvas::Fit;
    let fit_zoom = |app: &PdfCraftApp, i: usize| (app.views[i].fit, app.views[i].zoom);
    let mut h = harness(&[]);
    // The factory default fits the width.
    assert_eq!(h.state().views[0].fit, Fit::Width);
    h.state_mut().set_option("default-zoom", "fit-page").unwrap();
    assert!(h.state_mut().execute("create.blank"));
    assert_eq!(h.state().views[1].fit, Fit::Page);
    h.state_mut().set_option("default-zoom", "150%").unwrap();
    h.state_mut().open_bytes("other.pdf", None, PAGES.to_vec()).expect("opens");
    assert_eq!(fit_zoom(h.state(), 2), (Fit::None, 1.5));
    // A PDF that asks for a zoom gets it.
    let asks = String::from_utf8_lossy(PAGES).replace("/Pages 2 0 R >>", "/Pages 2 0 R /OpenAction [3 0 R /Fit] >>");
    h.state_mut().open_bytes("asks.pdf", None, asks.into_bytes()).expect("opens");
    assert_eq!(h.state().views[3].fit, Fit::Page);
    // Typos and zooms the view can't show are refused.
    for bad in ["huge", "fit-height", "5", "9000%"] {
        assert!(h.state_mut().set_option("default-zoom", bad).is_err(), "{bad}");
    }
    // Saved settings keep it, and garbage leaves it alone.
    let saved = h.state().persist();
    let mut fresh = PdfCraftApp::new();
    fresh.set_option("language", "en").unwrap();
    fresh.restore(&saved);
    fresh.restore(r#"{"default_zoom":"bogus"}"#);
    fresh.open_bytes("again.pdf", None, PAGES.to_vec()).expect("opens");
    assert_eq!(fit_zoom(&fresh, 0), (Fit::None, 1.5));
}

#[test]
fn preferences_set_the_default_zoom() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(&[]);
    assert!(h.state_mut().execute("app.preferences"));
    h.run_steps(3);
    h.get_by_label("Zoom to page level").click();
    h.run_steps(2);
    assert!(h.state_mut().execute("create.blank"));
    assert_eq!(h.state().views[1].fit, pdfcraft_ui_egui::canvas::Fit::Page);
}

#[test]
fn rail_page_display_menu_offers_acrobats_view_choices() {
    use egui_kittest::kittest::Queryable;
    use pdfcraft_ui_egui::canvas::{Fit, PageLayout};
    let mut h = harness(&[]);
    h.run_steps(4);
    let pick = |h: &mut Harness<'static, PdfCraftApp>, item: &str| {
        h.get_by_label_contains("Page display:").click();
        h.run_steps(3);
        h.get_by_label(item).click();
        h.run_steps(4);
    };
    let shown = |h: &Harness<'static, PdfCraftApp>| (h.state().views[0].layout, h.state().views[0].continuous(), h.state().views[0].fit);
    pick(&mut h, "Fit one full page");
    assert_eq!(shown(&h), (PageLayout::Single, false, Fit::Page), "one full page at a time, not scrolling");
    // The whole page fits at once, so the wheel turns pages without zooming out first.
    let at = h.state().views[0].viewport_rect().center();
    h.hover_at(at);
    h.run_steps(1);
    h.event(wheel(MouseWheelUnit::Line, -1.0, TouchPhase::Move));
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 1);
    pick(&mut h, "Actual size");
    assert_eq!((h.state().views[0].fit, h.state().views[0].zoom), (Fit::None, 1.0));
    pick(&mut h, "Zoom to page level");
    assert_eq!(h.state().views[0].fit, Fit::Page);
    pick(&mut h, "Fit to width scrolling");
    assert_eq!(shown(&h), (PageLayout::Single, true, Fit::Width), "single page, scrolling continuously");
}

#[test]
fn rail_page_display_button_switches_layouts() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(&[]);
    h.run_steps(4);
    h.get_by_label_contains("Page display").click();
    h.run_steps(3);
    h.get_by_label("Single page").click();
    h.run_steps(4);
    assert_eq!(h.state().views[0].layout, pdfcraft_ui_egui::canvas::PageLayout::Single);
}

#[test]
fn view_menu_page_display_uses_the_same_commands() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(&[]);
    h.run_steps(4);
    h.get_by_label("Menu").click();
    h.run_steps(2);
    h.get_by_label("View ⏵").hover();
    h.run_steps(3);
    h.get_by_label("Single page").click();
    h.run_steps(4);
    assert_eq!(h.state().views[0].layout, pdfcraft_ui_egui::canvas::PageLayout::Single);
}

#[test]
fn palette_runs_page_display_commands_from_the_keyboard() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(&[]);
    h.state_mut().set_option("palette", "single page").unwrap();
    h.run_steps(3);
    h.get_by_label("Single page");
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert!(!h.state().palette_open, "Enter runs the top hit and closes the palette");
    assert_eq!(h.state().views[0].layout, pdfcraft_ui_egui::canvas::PageLayout::Single);
}

#[test]
fn actual_size_fit_width_and_fit_page() {
    let mut h = harness(&[]);
    let pt = 96.0 / 72.0;

    h.key_press_modifiers(Modifiers::COMMAND, Key::Num1); // actual size
    h.run_steps(4);
    let r = rect(&h, 0).expect("page 1");
    assert!((r.width() - 300.0 * pt).abs() < 1.0, "100% is 300 pt at 96 dpi: {}", r.width());

    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2); // fit width
    h.run_steps(4);
    let (r, vp) = (rect(&h, 0).expect("page 1"), h.state().views[0].viewport_rect());
    assert!(r.width() > vp.width() * 0.85 && r.width() <= vp.width(), "fit width: page {} in viewport {}", r.width(), vp.width());

    h.key_press_modifiers(Modifiers::COMMAND, Key::Num0); // fit page
    h.run_steps(4);
    let (r, vp) = (rect(&h, 0).expect("page 1"), h.state().views[0].viewport_rect());
    assert!(r.height() <= vp.height() && r.height() > vp.height() * 0.85, "fit page: page {} in viewport {}", r.height(), vp.height());
    assert!(r.width() < vp.width(), "a portrait page fits by height");
}

#[test]
fn fit_page_holds_zoom_across_mixed_page_sizes() {
    // Scrolling views fit their largest page, so scrolling past other sizes must not jump
    // the zoom (#214).
    let mut h = mixed_harness(&[("layout", "continuous")]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num0); // fit page
    h.run_steps(4);
    let z1 = h.state().views[0].zoom;
    h.state_mut().set_option("page", "2").unwrap();
    h.run_steps(4);
    assert_eq!(h.state().views[0].current, 1);
    assert_eq!(h.state().views[0].zoom, z1, "another size scrolls past at the same zoom");
}

#[test]
fn single_page_fit_page_fits_the_shown_page() {
    // Single-page view fits the page it shows: each page fills the viewport on its own.
    let mut h = mixed_harness(&[("layout", "single")]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num0); // fit page
    h.run_steps(4);
    let vp = h.state().views[0].viewport_rect();
    let r1 = rect(&h, 0).expect("page 1");
    assert!(r1.width() <= vp.width() && r1.height() <= vp.height(), "page 1 fits: {r1:?} in {vp:?}");
    let z1 = h.state().views[0].zoom;
    h.state_mut().set_option("page", "2").unwrap();
    h.run_steps(4);
    let r2 = rect(&h, 1).expect("page 2");
    assert!(r2.width() <= vp.width() && r2.height() <= vp.height(), "page 2 fits: {r2:?} in {vp:?}");
    assert_ne!(h.state().views[0].zoom, z1, "the wider page refits instead of keeping page 1's zoom");
}

#[test]
fn clicking_a_link_goes_to_its_destination() {
    // Default fit-width zoom: pages are taller than the window, so the target can scroll to the top.
    let mut h = harness(&[]);
    h.run_steps(4);
    let r = rect(&h, 0).expect("page 1");
    // The link spans x 50..250, y 300..350 in PDF space (y up) on a 300×400 page.
    let at = egui::pos2(r.min.x + r.width() * (150.0 / 300.0), r.min.y + r.height() * (1.0 - 325.0 / 400.0));
    h.hover_at(at);
    h.run_steps(2);
    h.drag_at(at); // press
    h.drop_at(at); // release
    h.run_steps(8);
    assert_eq!(h.state().views[0].current, 2, "the link targets page 3");
    let (target, vp) = (rect(&h, 2).expect("page 3 on screen"), h.state().views[0].viewport_rect());
    assert!((target.min.y - vp.min.y).abs() < 40.0, "page 3 is scrolled to the top: {target:?} in {vp:?}");
}

/// Bookmarks and a link with positioned destinations (ISO 32000-2 §12.3.2.2). Page 2's crop box
/// is away from the origin (x 50..350, y 100..500), so its user-space y 420 is 20% down the page.
const DESTS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /Outlines 10 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [20 0 R] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 400 600] /CropBox [50 100 350 500] >> endobj
5 0 obj << /Type /Page /Parent 2 0 R >> endobj
10 0 obj << /Type /Outlines /First 11 0 R /Last 13 0 R /Count 3 >> endobj
11 0 obj << /Title (Section 1.1 Shapes) /Parent 10 0 R /Next 12 0 R /Dest [4 0 R /XYZ 0 420 0] >> endobj
12 0 obj << /Title (Zoomed in) /Parent 10 0 R /Next 13 0 R /Dest [4 0 R /XYZ 125 300 3] >> endobj
13 0 obj << /Title (Broken) /Parent 10 0 R /Dest [5 0 R /XYZ (left) /top true] >> endobj
20 0 obj << /Type /Annot /Subtype /Link /Rect [50 300 250 350] /Border [0 0 0] /Dest [4 0 R /FitH 420] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn dests_harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("dests.pdf", None, DESTS.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "bookmarks").unwrap();
        app
    });
    // The Bookmarks panel widens to fit its titles over many frames; fit-width zoom follows it.
    // Positions are checked once the window's layout holds still.
    let mut width = 0.0;
    for _ in 0..400 {
        h.run_steps(1);
        let w = h.state().views[0].viewport_rect().width();
        if w == width {
            break;
        }
        width = w;
    }
    h
}

/// Where the point a fraction (`fx`, `fy`) across and down `page` is on screen, relative to the
/// viewport's top-left corner.
fn from_viewport_corner(h: &Harness<'static, PdfCraftApp>, page: usize, fx: f32, fy: f32) -> egui::Vec2 {
    let (r, vp) = (rect(h, page).expect("page on screen"), h.state().views[0].viewport_rect());
    egui::pos2(r.min.x + fx * r.width(), r.min.y + fy * r.height()) - vp.min
}

#[test]
fn bookmark_destinations_scroll_to_their_position() {
    use egui_kittest::kittest::Queryable;
    use pdfcraft_ui_egui::canvas::Fit;
    let mut h = dests_harness();
    // Fit width: the pages are taller than the window, so any point of them can reach its top.
    h.get_by_label("Section 1.1 Shapes").click();
    h.run_steps(6);
    let v = &h.state().views[0];
    assert_eq!(v.current, 1, "the bookmark targets page 2");
    assert_eq!(v.fit, Fit::Width, "/XYZ with zoom 0 keeps the zoom");
    let d = from_viewport_corner(&h, 1, 0.0, 0.2);
    assert!(d.y.abs() < 2.0, "y 420 on page 2 is at the top of the window, not the page top: {d:?}");

    // /XYZ left top zoom: 300%, with (125, 300), a quarter across and half way down, at the
    // window's top-left (the page is now wider than the window): its left is the edge of the
    // gutter that keeps pages clear of the quick-action bar.
    h.get_by_label("Zoomed in").click();
    h.run_steps(6);
    let v = &h.state().views[0];
    assert_eq!((v.current, v.fit, v.zoom), (1, Fit::None, 3.0));
    let d = from_viewport_corner(&h, 1, 0.25, 0.5);
    assert!((d.x - 70.0).abs() < 2.0 && d.y.abs() < 2.0, "the destination point is at the window's top-left: {d:?}");

    // Malformed operands fall back to the top of the page at the current zoom.
    h.get_by_label("Broken").click();
    h.run_steps(6);
    let v = &h.state().views[0];
    assert_eq!((v.current, v.zoom), (2, 3.0));
    let d = from_viewport_corner(&h, 2, 0.0, 0.0);
    assert!(d.y > 0.0 && d.y < 40.0, "page 3 is scrolled to its top: {d:?}");
}

#[test]
fn link_destinations_scroll_to_their_position() {
    use pdfcraft_ui_egui::canvas::Fit;
    let mut h = dests_harness();
    h.state_mut().set_option("zoom", "100%").unwrap();
    h.run_steps(4);
    let r = rect(&h, 0).expect("page 1");
    // The link spans x 50..250, y 300..350 in PDF space on the 300×400 page 1.
    let at = egui::pos2(r.min.x + r.width() * (150.0 / 300.0), r.min.y + r.height() * (1.0 - 325.0 / 400.0));
    h.hover_at(at);
    h.run_steps(2);
    h.drag_at(at); // press
    h.drop_at(at); // release
    h.run_steps(8);
    let v = &h.state().views[0];
    assert_eq!((v.current, v.fit), (1, Fit::Width), "/FitH goes to page 2 and fits its width");
    let d = from_viewport_corner(&h, 1, 0.0, 0.2);
    assert!(d.y.abs() < 2.0, "y 420 on page 2 is at the top of the window: {d:?}");
}

/// Two pages with a text field on each and a non-embedded Helvetica.
const FORM: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R 7 0 R] >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [6 0 R] /Contents 5 0 R /Resources << /Font << /F1 8 0 R >> >> >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /Annots [7 0 R] >> endobj
5 0 obj << /Length 37 >> stream
BT /F1 12 Tf 20 20 Td (Form) Tj ET
endstream endobj
6 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (fullname) /V (Ada) /Rect [50 300 250 330] /P 3 0 R >> endobj
7 0 obj << /Type /Annot /Subtype /Widget /FT /Tx /T (postcode) /Rect [50 300 250 330] /P 4 0 R >> endobj
8 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn form_harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, FORM.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app
    });
    h.run_steps(8);
    h
}

#[test]
fn field_list_jumps_to_a_fields_page() {
    use egui_kittest::kittest::Queryable;
    let mut h = form_harness();
    h.state_mut().set_option("panel", "fields").unwrap();
    h.run_steps(4);
    h.get_by_label("fullname");
    h.get_by_label("postcode").click();
    h.run_steps(8);
    assert_eq!(h.state().views[0].current, 1, "postcode is on page 2");
}

#[test]
fn highlight_fields_tints_the_field_area() {
    use egui_kittest::kittest::Queryable;
    let _gpu = gpu();
    let mut h = form_harness();
    h.state_mut().set_option("panel", "none").unwrap();
    for _ in 0..100 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let r = rect(&h, 0).expect("page 1");
    // The field spans x 50..250, y 300..330 (PDF space, y up) on a 300×400 page; sample inside it.
    let at = egui::pos2(r.min.x + r.width() * (60.0 / 300.0), r.min.y + r.height() * (1.0 - 305.0 / 400.0));
    let pixel = |h: &mut Harness<'static, PdfCraftApp>| {
        let img = h.render().expect("renders");
        let ppp = h.ctx.pixels_per_point();
        *img.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32)
    };
    let before = pixel(&mut h);
    h.get_by_label("Highlight fields").click();
    h.run_steps(4);
    assert!(h.state().views[0].highlight_fields);
    h.get_by_label("Hide field highlights");
    let after = pixel(&mut h);
    assert_ne!(before, after, "the field area is tinted when highlighting is on");
}

/// A required radio group (`/Ff` 32768 radio + 2 required) with two 40 pt round buttons.
const RADIOS: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Annots [5 0 R 6 0 R] >> endobj
4 0 obj << /FT /Btn /Ff 32770 /T (choice) /V /Off /Kids [5 0 R 6 0 R] >> endobj
5 0 obj << /Type /Annot /Subtype /Widget /Parent 4 0 R /Rect [50 300 90 340] /AS /Off /P 3 0 R >> endobj
6 0 obj << /Type /Annot /Subtype /Widget /Parent 4 0 R /Rect [150 300 190 340] /AS /Off /P 3 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// #260 (page 8): with fields highlighted, a required radio button gets a round red border, as
/// Acrobat draws it, not a square one around its corners.
#[test]
fn required_radio_buttons_get_a_round_red_border() {
    let _gpu = gpu();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("radios.pdf", None, RADIOS.to_vec()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app.set_option("fields", "on").unwrap();
        app
    });
    for _ in 0..100 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let r = rect(&h, 0).expect("page 1");
    let ppp = h.ctx.pixels_per_point();
    let img = h.render().expect("renders");
    // The first button spans x 50..90, y 300..340 on the 300×400 pt page (y up).
    let at = |x: f32, y: f32| {
        let p = egui::pos2(r.min.x + r.width() * (x / 300.0), r.min.y + r.height() * (1.0 - y / 400.0));
        *img.get_pixel((p.x * ppp) as u32, (p.y * ppp) as u32)
    };
    let red = |p: image::Rgba<u8>| p[0] > 180 && p[1] < 100 && p[2] < 100;
    // Fit width puts about 4 px in a point: 0.2 to 0.3 pt from an edge is inside a 2 px border.
    assert!(red(at(50.2, 320.0)), "the left of the circle is red: {:?}", at(50.2, 320.0));
    assert!(!red(at(50.3, 339.7)), "the widget's corner, outside the circle, is not: {:?}", at(50.3, 339.7));
}

/// Acrobat's Highlight existing fields is a preference, not a per-document choice: turned on
/// once, the next document (and the next session) opens with fields highlighted.
#[test]
fn field_highlighting_is_remembered() {
    use egui_kittest::kittest::Queryable;
    let mut h = form_harness();
    h.state_mut().set_option("panel", "none").unwrap();
    h.run_steps(4);
    assert!(!h.state().views[0].highlight_fields, "off by default");
    h.get_by_label("Highlight fields").click();
    h.run_steps(4);
    assert!(h.state().views[0].highlight_fields);
    h.state_mut().open_bytes("radios.pdf", None, RADIOS.to_vec()).expect("opens");
    h.run_steps(4);
    assert!(h.state().views[1].highlight_fields, "the next document opens highlighted");
    let saved = h.state().persist();
    let mut next = PdfCraftApp::new();
    next.set_option("language", "en").unwrap();
    next.restore(&saved);
    next.open_bytes("form.pdf", None, FORM.to_vec()).expect("opens");
    assert!(next.views[0].highlight_fields, "and so does the next session");
}

#[test]
fn fonts_tab_lists_fonts_and_embedding() {
    use egui_kittest::kittest::Queryable;
    let mut h = form_harness();
    h.state_mut().set_option("dialog", "fonts").unwrap();
    h.run_steps(4);
    h.get_by_label("Helvetica");
    h.get_by_label_contains("Not embedded");
}

#[test]
fn the_hand_tool_pans_by_dragging() {
    let mut h = harness(&[("layout", "continuous"), ("zoom", "150"), ("quick", "hand")]);
    h.run_steps(4);
    let before = rect(&h, 0).expect("page 1 on screen");
    let start = egui::pos2(700.0, 700.0);
    h.hover_at(start);
    h.run_steps(1);
    h.drag_at(start);
    h.run_steps(1);
    for k in 1..=5 {
        h.hover_at(start - egui::vec2(0.0, 60.0 * k as f32));
        h.run_steps(1);
    }
    h.drop_at(start - egui::vec2(0.0, 300.0));
    h.run_steps(4);
    let after = rect(&h, 0).expect("still on screen");
    assert!(before.top() - after.top() > 200.0, "dragging up scrolls down: {before:?} → {after:?}");
    assert!(h.state().views[0].selected_text().is_none(), "no text selection with the hand");
}

#[test]
fn required_fields_get_a_red_border_when_highlighting() {
    let _gpu = gpu();
    let mut h = form_harness();
    h.state_mut().set_option("panel", "none").unwrap();
    h.state_mut().set_option("fields", "on").unwrap();
    let props = pdfcraft_engine::FieldProps { required: Some(true), ..Default::default() };
    h.state_mut().apply_edit(pdfcraft_engine::Edit::SetFieldProps { name: "fullname".into(), props: Box::new(props) });
    for _ in 0..100 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let r = rect(&h, 0).expect("page 1");
    // Just inside the left edge of the field (x 50, y 300..330 on a 300×400 page).
    let ppp = h.ctx.pixels_per_point();
    let at = egui::pos2(r.min.x + r.width() * (50.0 / 300.0) + 0.75, r.min.y + r.height() * (1.0 - 315.0 / 400.0));
    let img = h.render().expect("renders");
    let px = *img.get_pixel((at.x * ppp) as u32, (at.y * ppp) as u32);
    assert!(px[0] > 180 && px[1] < 100 && px[2] < 100, "a red border: {px:?}");
}

/// The page raster is shown texel for texel. At a zoom whose device scale isn't on the old 1/64
/// grid (87 % at 1 px per point: 1.16, rendered at 1.15625) the page was rendered at the
/// quantized scale and stretched onto a rectangle that didn't start on a whole pixel, so every
/// line and glyph was resampled and looked soft (#260, page 9).
#[test]
fn page_raster_maps_one_to_one_onto_screen_pixels() {
    let _gpu = gpu();
    // Fine vertical and horizontal lines (0.5 pt every 1.7 pt) fill most of a 300×400 pt page.
    let mut content = String::from("0 g\n");
    for i in 0..140 {
        let v = 30.0 + 1.7 * i as f32;
        content.push_str(&format!("{v} 40 0.5 320 re 30 {v} 240 0.5 re\n"));
    }
    content.push_str("f\n");
    let pdf = format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
        content.len()
    )
    .into_bytes();
    for ppp in [1.0, 2.0] {
        let bytes = pdf.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1000.0, 800.0)).with_pixels_per_point(ppp).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("lines.pdf", None, bytes.clone()).expect("opens");
            app.set_option("left", "closed").unwrap();
            app.set_option("panel", "none").unwrap();
            app.set_option("zoom", "87").unwrap();
            app
        });
        let (mean, worst) = compare_page_with_raster(&mut h, &pdf, 0.87);
        assert!(mean < 1.0 && worst < 16, "{ppp} px/pt: the page on screen differs from its raster: mean {mean:.2}, max {worst}");
    }
}

/// Mean and largest difference between page 1 on screen and the page rendered directly at the
/// view's exact device scale, over the lined area of `page_raster_maps_one_to_one_onto_screen_pixels`.
fn compare_page_with_raster(h: &mut Harness<'static, PdfCraftApp>, pdf: &[u8], zoom: f32) -> (f64, u8) {
    for _ in 0..100 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let ppp = h.ctx.pixels_per_point();
    let r = rect(h, 0).expect("page 1");
    let img = h.render().expect("renders");
    let scale = zoom * 96.0 / 72.0 * ppp;
    let mut renderer = pdfcraft_render::PageRenderer::new(std::sync::Arc::new(pdf.to_vec()), pdfcraft_render::RenderConfig::default());
    let page = renderer.render(pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::Pixels, tile: None, scale, tag: 0 });
    assert!(page.error.is_none(), "{:?}", page.error);
    // Compare the lined area (page x 40..270 pt, y 60..330 pt from the top), screen against raster.
    let (ox, oy) = ((r.min.x * ppp).round() as i64, (r.min.y * ppp).round() as i64);
    let (mut sum, mut worst, mut n) = (0u64, 0u8, 0u64);
    for py in (60.0 * scale) as u32..(330.0 * scale) as u32 {
        for px in (40.0 * scale) as u32..(270.0 * scale) as u32 {
            let want = page.rgba[((py * page.width + px) * 4) as usize];
            let got = img.get_pixel((ox + px as i64) as u32, (oy + py as i64) as u32)[0];
            let d = want.abs_diff(got);
            sum += u64::from(d);
            worst = worst.max(d);
            n += 1;
        }
    }
    (sum as f64 / n as f64, worst)
}

#[test]
fn zooming_a_tiled_page_shows_its_earlier_tiles_until_new_ones_arrive() {
    let _gpu = gpu();
    // A 1200 pt square page of fine lines (0.5 pt every 1.7 pt): tiled at 300 %.
    let mut content = String::from("0 g\n");
    for i in 0..670 {
        let v = 30.0 + 1.7 * i as f32;
        content.push_str(&format!("{v} 30 0.5 1140 re 30 {v} 1140 0.5 re\n"));
    }
    content.push_str("f\n");
    let pdf = format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 1200 1200] /Contents 4 0 R >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
        content.len()
    )
    .into_bytes();
    let mut h = Harness::builder().with_size(egui::vec2(1000.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("grid.pdf", None, pdf.clone()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app.set_option("zoom", "300").unwrap();
        app
    });
    for _ in 0..400 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // Detail in the middle of the window: the summed difference between neighbouring pixels.
    let detail = |h: &mut Harness<'static, PdfCraftApp>| {
        let img = h.render().expect("renders");
        let (cx, cy) = (img.width() / 2, img.height() / 2);
        let mut sum = 0u64;
        for y in cy - 100..cy + 100 {
            for x in cx - 150..cx + 150 {
                sum += u64::from(img.get_pixel(x, y)[0].abs_diff(img.get_pixel(x + 1, y)[0]));
            }
        }
        sum
    };
    let sharp = detail(&mut h);
    // The first frame at a new zoom has no tiles for it yet (they are requested at its end).
    h.state_mut().set_option("zoom", "310").unwrap();
    h.step();
    let first = detail(&mut h);
    assert!(first * 10 > sharp * 7, "the first frame after zooming lost its detail: {first} against {sharp}");
}
