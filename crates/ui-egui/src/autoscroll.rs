//! Middle-button auto-scroll for the document viewport, the same on every platform. A short
//! press latches it on until the next middle press; holding the button scrolls until the release.
//! Either way the page tools never see the middle button, because egui's drag responses accept
//! every pointer button. While latched, a text tool keeps the left button: a drag selects or
//! highlights as the page scrolls, and a click stops scrolling.

use egui::{Context, CursorIcon, Event, Key, PointerButton, Pos2, Stroke, Vec2, vec2};

const DEAD_ZONE: f32 = 15.0;
/// Seconds the starting press may stay down and still latch auto-scroll on. Held this long or
/// longer, it is a hold: scrolling stops when the button comes up, as in Acrobat on Windows.
const HOLD: f64 = 0.5;
// Chromium's autoscroll_controller.cc uses distance^2.2 * 0.000008; its
// ui/events/gestures/fixed_velocity_curve.cc multiplies elapsed seconds by 5000.
const SPEED_EXPONENT: f32 = 2.2;
const SPEED_MULTIPLIER: f32 = 0.04;
// Bound hostile coordinates before exponentiation, far beyond ordinary screen distances.
const MAX_DISPLACEMENT: f32 = 1_000_000.0;

#[derive(Default)]
pub(crate) struct AutoScroll {
    anchor: Option<Pos2>,
    organize: bool,
    /// Own a cancelling click through its release, so it cannot also edit page content.
    cancel_button: Option<PointerButton>,
    block_input: bool,
    /// When the press that started scrolling went down (egui input time), until its release
    /// decides between latching and stopping. `None` once latched.
    pressed_at: Option<f64>,
    /// A left press that a text tool took while latched, until its release: a click stops
    /// scrolling, a drag (a selection) leaves it on.
    selecting: bool,
}

impl AutoScroll {
    pub(crate) fn active(&self) -> bool {
        self.anchor.is_some()
    }

    pub(crate) fn cancel(&mut self) {
        self.anchor = None;
        self.pressed_at = None;
        self.selecting = false;
        self.cancel_button = None;
        self.block_input = false;
    }

    pub(crate) fn blocks_input(&self) -> bool {
        self.block_input
    }

    /// Run before the document's widgets. Starting is restricted to the unobstructed viewport;
    /// once started, moving outside that viewport still controls the speed. `select_text` says
    /// the current tool selects text, so latched scrolling leaves it the left button.
    pub(crate) fn update(&mut self, ui: &egui::Ui, viewport: egui::Rect, organize: bool, select_text: bool) -> Vec2 {
        let ctx = ui.ctx();
        let press = |i: &egui::InputState, button: PointerButton| {
            // Read the press event itself: later movement in this frame must not move its position.
            i.events.iter().find_map(|event| match event {
                Event::PointerButton { pos, button: b, pressed: true, .. } if *b == button && pos.is_finite() => Some(*pos),
                _ => None,
            })
        };
        let (pointer, middle_press, middle_down, middle_released, left_press, left_down, left_clicked, interrupted, pressed, dt, now) =
            ctx.input(|i| {
                (
                    i.pointer.hover_pos(),
                    press(i, PointerButton::Middle),
                    i.pointer.button_down(PointerButton::Middle),
                    i.pointer.button_released(PointerButton::Middle),
                    press(i, PointerButton::Primary),
                    i.pointer.button_down(PointerButton::Primary),
                    i.pointer.button_clicked(PointerButton::Primary),
                    !i.focused || i.key_pressed(Key::Escape) || i.events.iter().any(|e| matches!(e, Event::MouseWheel { .. } | Event::Zoom(_))),
                    [PointerButton::Primary, PointerButton::Secondary, PointerButton::Extra1, PointerButton::Extra2]
                        .map(|button| i.pointer.button_pressed(button).then_some(button)),
                    i.stable_dt,
                    i.time,
                )
            });
        let starts_here = |p: Pos2| viewport.intersect(ui.clip_rect()).contains(p) && ctx.layer_id_at(p) == Some(ui.layer_id());
        let latched = self.active() && self.pressed_at.is_none();
        // Latched, a text tool keeps the left button on the page: it selects while scrolling goes on.
        let selects = select_text && latched;
        if selects && left_press.is_some_and(starts_here) {
            self.selecting = true;
        }
        let cancel_button = pressed.into_iter().flatten().find(|&button| !(button == PointerButton::Primary && self.selecting));
        let cancel = interrupted || cancel_button.is_some();
        self.block_input = (self.active() && !selects) || self.cancel_button.is_some() || middle_press.is_some() || middle_down || middle_released;
        if let Some(button) = self.cancel_button {
            if !ctx.input(|i| i.pointer.button_down(button)) {
                self.cancel_button = None;
            }
            return Vec2::ZERO;
        }
        if self.active() && (cancel || pointer.is_none() || self.organize != organize || ctx.egui_wants_keyboard_input()) {
            self.cancel();
            self.block_input = true;
            self.cancel_button = cancel_button;
            return Vec2::ZERO;
        }
        if let Some(pressed_at) = middle_press {
            if self.active() {
                self.cancel();
                self.block_input = true;
                self.cancel_button = Some(PointerButton::Middle);
                return Vec2::ZERO;
            } else if !cancel && !ctx.egui_wants_keyboard_input() && starts_here(pressed_at) {
                self.anchor = Some(pressed_at);
                self.pressed_at = Some(now);
                self.organize = organize;
            }
        }
        if self.selecting && !left_down {
            self.selecting = false;
            if left_clicked {
                // A click stops scrolling and stays an ordinary click on the page.
                self.cancel();
                return Vec2::ZERO;
            }
        }
        if middle_released && let Some(start) = self.pressed_at.take() {
            let held = now - start;
            if held.is_finite() && held >= HOLD {
                // A hold ends with its release; the release frame still belongs to the gesture.
                self.cancel();
                self.block_input = true;
                return Vec2::ZERO;
            }
        }
        let (Some(anchor), Some(pointer)) = (self.anchor, pointer) else { return Vec2::ZERO };
        let displacement = pointer.y - anchor.y;
        // Cap the elapsed time too: returning from an idle/hidden window must never jump pages.
        let delta = scroll_delta(displacement, dt);
        if displacement.is_finite() && displacement.abs() > DEAD_ZONE {
            // Continuous redraws let stable_dt use measured frame time. Delayed redraws
            // instead use predicted_dt, which can make speed depend on the actual frame rate.
            ctx.request_repaint();
        }
        vec2(0.0, delta)
    }

    /// Draw an original geometric marker at the activation point, above page content.
    pub(crate) fn paint(&self, ui: &egui::Ui, viewport: egui::Rect) {
        let Some(anchor) = self.anchor else { return };
        let painter = ui.painter().with_clip_rect(viewport);
        let ink = ui.visuals().text_color();
        painter.circle(anchor, 13.0, ui.visuals().window_fill(), Stroke::new(1.0, ink));
        painter.circle_filled(anchor, 2.0, ink);
        for direction in [-1.0, 1.0] {
            painter.add(egui::Shape::convex_polygon(
                vec![anchor + vec2(-4.0, direction * 6.0), anchor + vec2(4.0, direction * 6.0), anchor + vec2(0.0, direction * 10.0)],
                ink,
                Stroke::NONE,
            ));
        }
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }

    /// Escape belongs to autoscroll first, leaving selection/find/full-screen intact.
    pub(crate) fn escape(&mut self, ctx: &Context) -> bool {
        if self.active() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            self.cancel();
            true
        } else {
            false
        }
    }
}

fn scroll_delta(displacement: f32, dt: f32) -> f32 {
    if !displacement.is_finite() || !dt.is_finite() {
        return 0.0;
    }
    let distance = displacement.abs();
    if distance <= DEAD_ZONE {
        return 0.0;
    }
    // Chromium uses the full distance outside the dead zone, without subtracting its radius.
    let speed = distance.min(MAX_DISPLACEMENT).powf(SPEED_EXPONENT) * SPEED_MULTIPLIER;
    // egui's delta moves content, the opposite of the scroll offset.
    -displacement.signum() * speed * dt.clamp(0.0, 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn middle(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos, button: PointerButton::Middle, pressed, modifiers: egui::Modifiers::NONE }
    }

    fn left(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }
    }

    /// Run one frame at `time` seconds; `check` sees the scroll delta inside the frame.
    fn frame(ctx: &Context, scroll: &mut AutoScroll, time: f64, events: Vec<Event>, focused: bool, check: impl Fn(&AutoScroll, Vec2)) {
        frame_with(ctx, scroll, time, events, focused, false, check);
    }

    /// [`frame`] with `select_text`: whether the current tool selects text.
    fn frame_with(
        ctx: &Context,
        scroll: &mut AutoScroll,
        time: f64,
        events: Vec<Event>,
        focused: bool,
        select_text: bool,
        check: impl Fn(&AutoScroll, Vec2),
    ) {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(400.0, 400.0))),
                events,
                focused,
                time: Some(time),
                ..Default::default()
            },
            |ui| {
                let delta = scroll.update(ui, ui.max_rect(), false, select_text);
                check(scroll, delta);
            },
        );
        // This input-only test has no renderer to apply the generated font texture.
        output.textures_delta.clear();
    }

    #[test]
    fn a_short_press_latches_until_the_next_press() {
        let ctx = Context::default();
        let mut scroll = AutoScroll::default();
        let p = egui::pos2(100.0, 100.0);
        let below = p + vec2(0.0, 80.0);
        // The first frame lays out the layer that later presses are tested against.
        frame(&ctx, &mut scroll, 0.0, vec![], true, |_, _| {});
        frame(&ctx, &mut scroll, 0.1, vec![Event::PointerMoved(p), middle(p, true)], true, |s, _| {
            assert!(s.active() && s.blocks_input(), "the press itself must not reach the page tools");
        });
        frame(&ctx, &mut scroll, 0.3, vec![middle(p, false)], true, |s, _| {
            assert!(s.active(), "a short press keeps scrolling on after the release");
            assert!(s.blocks_input(), "the release must not finish a drawing drag");
        });
        frame(&ctx, &mut scroll, 0.5, vec![Event::PointerMoved(below)], true, |s, d| {
            assert!(s.active());
            assert!(d.y < 0.0, "below the anchor scrolls down with no button held: {d:?}");
        });
        frame(&ctx, &mut scroll, 2.0, vec![middle(below, true)], true, |s, d| {
            assert!(!s.active(), "the next press stops it");
            assert_eq!(d, Vec2::ZERO);
            assert!(s.blocks_input(), "the stopping press owns its release");
        });
        frame(&ctx, &mut scroll, 2.1, vec![middle(below, false)], true, |s, _| assert!(s.blocks_input()));
        frame(&ctx, &mut scroll, 2.2, vec![], true, |s, _| assert!(!s.active() && !s.blocks_input()));
    }

    #[test]
    fn holding_scrolls_until_the_release() {
        let ctx = Context::default();
        let mut scroll = AutoScroll::default();
        let p = egui::pos2(100.0, 100.0);
        let below = p + vec2(0.0, 80.0);
        frame(&ctx, &mut scroll, 0.0, vec![], true, |_, _| {});
        frame(&ctx, &mut scroll, 0.1, vec![Event::PointerMoved(p), middle(p, true)], true, |s, _| assert!(s.active() && s.blocks_input()));
        frame(&ctx, &mut scroll, 0.4, vec![Event::PointerMoved(below)], true, |s, d| {
            assert!(s.active() && s.blocks_input());
            assert!(d.y < 0.0, "a held button scrolls with distance from the press: {d:?}");
        });
        frame(&ctx, &mut scroll, 0.7, vec![middle(below, false)], true, |s, d| {
            assert!(!s.active(), "releasing a hold stops scrolling");
            assert_eq!(d, Vec2::ZERO);
            assert!(s.blocks_input(), "the release must not finish a drawing drag");
        });
        frame(&ctx, &mut scroll, 0.8, vec![Event::PointerMoved(p)], true, |s, d| {
            assert_eq!(d, Vec2::ZERO, "nothing moves once the button is up");
            assert!(!s.active() && !s.blocks_input());
        });
    }

    #[test]
    fn half_a_second_separates_a_press_from_a_hold() {
        let p = egui::pos2(100.0, 100.0);
        for (held, latched) in [(0.0, true), (0.2, true), (0.49, true), (0.5, false), (3.0, false)] {
            let ctx = Context::default();
            let mut scroll = AutoScroll::default();
            frame(&ctx, &mut scroll, 0.0, vec![], true, |_, _| {});
            frame(&ctx, &mut scroll, 1.0, vec![Event::PointerMoved(p), middle(p, true)], true, |_, _| {});
            frame(&ctx, &mut scroll, 1.0 + held, vec![middle(p, false)], true, |s, _| {
                assert_eq!(s.active(), latched, "held for {held} s");
            });
        }
        // A press and release that arrive in one frame are a click.
        let ctx = Context::default();
        let mut scroll = AutoScroll::default();
        frame(&ctx, &mut scroll, 0.0, vec![], true, |_, _| {});
        frame(&ctx, &mut scroll, 1.0, vec![Event::PointerMoved(p), middle(p, true), middle(p, false)], true, |s, _| assert!(s.active()));
    }

    #[test]
    fn presses_outside_the_viewport_are_ignored_and_focus_loss_stops_scrolling() {
        let ctx = Context::default();
        let mut scroll = AutoScroll::default();
        let outside = egui::pos2(500.0, 100.0);
        frame(&ctx, &mut scroll, 0.0, vec![], true, |_, _| {});
        frame(&ctx, &mut scroll, 0.1, vec![Event::PointerMoved(outside), middle(outside, true)], true, |s, _| assert!(!s.active()));
        frame(&ctx, &mut scroll, 0.2, vec![Event::PointerMoved(outside + vec2(0.0, 50.0))], true, |_, d| assert_eq!(d, Vec2::ZERO));
        frame(&ctx, &mut scroll, 0.3, vec![middle(outside, false)], true, |s, _| assert!(!s.active()));

        let p = egui::pos2(100.0, 100.0);
        frame(&ctx, &mut scroll, 1.0, vec![Event::PointerMoved(p), middle(p, true)], true, |s, _| assert!(s.active()));
        frame(&ctx, &mut scroll, 1.1, vec![Event::PointerMoved(p + vec2(0.0, 50.0))], false, |s, d| {
            assert_eq!(d, Vec2::ZERO);
            assert!(!s.active(), "losing focus stops scrolling");
        });
    }

    /// Latch auto-scroll with a short middle click at `p`, ending at 0.2 s.
    fn latch(ctx: &Context, scroll: &mut AutoScroll, p: Pos2, select_text: bool) {
        frame_with(ctx, scroll, 0.0, vec![], true, select_text, |_, _| {});
        frame_with(ctx, scroll, 0.1, vec![Event::PointerMoved(p), middle(p, true)], true, select_text, |_, _| {});
        frame_with(ctx, scroll, 0.2, vec![middle(p, false)], true, select_text, |s, _| assert!(s.active()));
    }

    #[test]
    fn a_left_drag_with_a_text_tool_selects_while_scrolling_continues() {
        let ctx = Context::default();
        let mut scroll = AutoScroll::default();
        let p = egui::pos2(100.0, 100.0);
        let q = p + vec2(0.0, 80.0);
        latch(&ctx, &mut scroll, p, true);
        frame_with(&ctx, &mut scroll, 0.5, vec![Event::PointerMoved(q)], true, true, |s, d| {
            assert!(s.active() && !s.blocks_input(), "latched with a text tool, the page takes input");
            assert!(d.y < 0.0);
        });
        frame_with(&ctx, &mut scroll, 1.0, vec![left(q, true)], true, true, |s, _| {
            assert!(s.active(), "a left press on the page selects instead of stopping");
            assert!(!s.blocks_input(), "the text tool sees the press");
        });
        frame_with(&ctx, &mut scroll, 1.1, vec![Event::PointerMoved(q + vec2(0.0, 30.0))], true, true, |s, d| {
            assert!(s.active() && !s.blocks_input());
            assert!(d.y < 0.0, "scrolling continues while the left button drags: {d:?}");
        });
        frame_with(&ctx, &mut scroll, 1.2, vec![left(q + vec2(0.0, 30.0), false)], true, true, |s, _| {
            assert!(s.active(), "the end of a drag leaves scrolling on");
            assert!(!s.blocks_input());
        });
        frame_with(&ctx, &mut scroll, 1.3, vec![], true, true, |s, _| assert!(s.active()));
    }

    #[test]
    fn a_left_click_with_a_text_tool_stops_scrolling_and_reaches_the_page() {
        let ctx = Context::default();
        let mut scroll = AutoScroll::default();
        let p = egui::pos2(100.0, 100.0);
        let q = p + vec2(0.0, 80.0);
        latch(&ctx, &mut scroll, p, true);
        frame_with(&ctx, &mut scroll, 1.0, vec![Event::PointerMoved(q), left(q, true)], true, true, |s, _| {
            assert!(s.active() && !s.blocks_input());
        });
        frame_with(&ctx, &mut scroll, 1.1, vec![left(q, false)], true, true, |s, d| {
            assert!(!s.active(), "a left click stops scrolling");
            assert_eq!(d, Vec2::ZERO);
            assert!(!s.blocks_input(), "the click is an ordinary click on the page");
        });
    }

    #[test]
    fn a_left_press_with_another_tool_or_off_the_page_stops_scrolling_and_is_owned() {
        let p = egui::pos2(100.0, 100.0);
        let outside = egui::pos2(500.0, 100.0);
        // Another tool (drawing, cropping…), and a text tool pressed outside the viewport.
        for (select_text, at) in [(false, p + vec2(0.0, 80.0)), (true, outside)] {
            let ctx = Context::default();
            let mut scroll = AutoScroll::default();
            latch(&ctx, &mut scroll, p, select_text);
            frame_with(&ctx, &mut scroll, 1.0, vec![Event::PointerMoved(at), left(at, true)], true, select_text, |s, _| {
                assert!(!s.active(), "select_text={select_text}: the press stops scrolling");
                assert!(s.blocks_input(), "select_text={select_text}: the stopping press must not reach a tool");
            });
            frame_with(&ctx, &mut scroll, 1.1, vec![left(at, false)], true, select_text, |s, _| assert!(s.blocks_input()));
            frame_with(&ctx, &mut scroll, 1.2, vec![], true, select_text, |s, _| assert!(!s.blocks_input()));
        }
    }

    #[test]
    fn a_left_press_while_the_middle_button_is_held_stays_blocked() {
        let ctx = Context::default();
        let mut scroll = AutoScroll::default();
        let p = egui::pos2(100.0, 100.0);
        let q = p + vec2(0.0, 80.0);
        frame_with(&ctx, &mut scroll, 0.0, vec![], true, true, |_, _| {});
        frame_with(&ctx, &mut scroll, 0.1, vec![Event::PointerMoved(p), middle(p, true)], true, true, |s, _| assert!(s.active()));
        frame_with(&ctx, &mut scroll, 0.3, vec![Event::PointerMoved(q), left(q, true)], true, true, |s, _| {
            assert!(!s.active(), "a left press during a hold stops scrolling");
            assert!(s.blocks_input(), "nothing reaches the page while the middle button is down");
        });
    }

    #[test]
    fn chromium_curve_is_gentle_near_the_anchor_and_accelerates_farther_away() {
        // Reference speeds in screen points/second from Chromium's distance exponent (2.2),
        // controller multiplier (0.000008), and fixed-velocity animation multiplier (5000).
        for (distance, expected_speed) in [(25.0, 48.0), (50.0, 219.0), (100.0, 1005.0), (200.0, 4617.0)] {
            let speed = -scroll_delta(distance, 0.01) / 0.01;
            assert!((speed - expected_speed).abs() < 1.0, "distance={distance}, speed={speed}, expected={expected_speed}");
        }
        for distance in [-15.0, 0.0, 15.0] {
            assert_eq!(scroll_delta(distance, 0.01), 0.0);
        }
        assert!(scroll_delta(15.1, 0.01) < 0.0);
    }

    #[test]
    fn fractional_motion_covers_the_same_distance_at_different_frame_rates() {
        for distance in [16.0, 50.0, 200.0] {
            let expected = scroll_delta(distance, 0.01) * 100.0;
            for frames in [30, 60, 120, 144] {
                let delta = scroll_delta(distance, 1.0 / frames as f32);
                let travelled: f32 = (0..frames).map(|_| delta).sum();
                assert!((travelled - expected).abs() < expected.abs() * 0.00001);
            }
        }
        assert!(scroll_delta(16.0, 1.0 / 144.0).abs() < 1.0);
    }

    #[test]
    fn speed_has_a_dead_zone_is_symmetric_and_rejects_invalid_input() {
        for y in [-15.0, -1.0, 0.0, 1.0, 15.0] {
            assert_eq!(scroll_delta(y, 0.016), 0.0);
        }
        assert!(scroll_delta(30.0, 0.016) < 0.0);
        for (near, far) in [(16.0, 30.0), (30.0, 100.0), (100.0, 200.0)] {
            assert!(scroll_delta(far, 0.016).abs() > scroll_delta(near, 0.016).abs());
        }
        assert_eq!(scroll_delta(30.0, 0.016), -scroll_delta(-30.0, 0.016));
        assert!(scroll_delta(200.0, 0.01).abs() / 0.01 > 3200.0, "ordinary distances have no linear-curve speed ceiling");
        assert_eq!(scroll_delta(1000.0, 10.0), scroll_delta(1000.0, 0.05));
        assert_eq!(scroll_delta(1000.0, -1.0), 0.0);
        assert_eq!(scroll_delta(f32::MAX, 0.016), scroll_delta(MAX_DISPLACEMENT, 0.016));
        assert!(scroll_delta(f32::MAX, 0.016).is_finite());
        assert_eq!(scroll_delta(-f32::MAX, 0.016), -scroll_delta(f32::MAX, 0.016));
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(scroll_delta(invalid, 0.016), 0.0);
            assert_eq!(scroll_delta(1000.0, invalid), 0.0);
        }
    }
}
