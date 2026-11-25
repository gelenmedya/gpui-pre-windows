//! rustyfiles: a finger's swipe scrolls.
//!
//! Windows turns a finger on a touch screen into mouse messages for a window that handles no touch input: a press, moves with the
//! left button down, a release. To the app a swipe was a drag of the mouse, so a swipe over a file list drew a selection box and a
//! swipe over anything else did nothing (rustyfiles issue 336: "RustyFiles instead draws a rectangular selection box").
//!
//! The rule here, over those same messages (which carry a signature saying they came from a finger, [`from_touch`]):
//!
//! * the press of a finger is held back, not given to the app;
//! * lifted before it moved [`SLOP`]: a tap. The press and the release are given to the app then, as a click;
//! * moved, and not clearly sideways: a swipe. The app gets scroll events for the finger's way up and down and never hears of the
//!   press. Lifted while moving, the scroll runs out by itself ([`Fling`]);
//! * moved clearly sideways (more than twice as far across as up or down): a drag, as before. The held press is given to the app
//!   and the moves follow it. This keeps what a finger could already do: a slider, a pane's divider, a column's edge, the
//!   selection box and a drag of files, started sideways.
//!
//! Nothing changes for a mouse, a touchpad or a pen: their presses do not carry the signature.
//!
//! The decision is [`Pan`], which knows no window and no clock of its own, so the tests drive it with numbers. NOT TRIED ON A
//! TOUCH SCREEN by the author (there is none here): `GPUI_TEST_TOUCH=1` makes every left press count as a finger's, and the rule
//! was tried with posted mouse messages that way.

/// How far (logical pixels) a finger may move before the press is a swipe or a drag and no longer a tap.
pub(crate) const SLOP: f32 = 8.0;

/// A finger that rested this long before lifting is not flung: the scroll stops where it is.
const REST_MS: u64 = 90;

/// Slower than this (physical pixels a second) there is no fling.
const FLING_FROM: f32 = 120.0;

/// A fling ends below this speed.
const FLING_UNTIL: f32 = 30.0;

/// Time constant of a fling's decay: it travels `speed * FLING_TAU` in all.
const FLING_TAU: f32 = 0.325;

/// What a move does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum OnMove {
    /// Not a finger's business: the app gets the move.
    Pass,
    /// Still within the slop (or no way made): nobody gets it.
    Swallow,
    /// The swipe moved this far down (physical pixels; negative: up).
    Scroll(f32),
    /// It is a drag: give the app the held press (its `lparam`) first, then this move.
    Release(isize),
}

/// What a release does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum OnUp {
    /// Not a finger's business.
    Pass,
    /// A tap: give the app the held press (its `lparam`), then this release.
    Tap(isize),
    /// The swipe ended at this speed (physical pixels a second, down positive; 0: no fling).
    Swiped(f32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum State {
    #[default]
    Idle,
    Held {
        x: i32,
        y: i32,
        lparam: isize,
    },
    Panning {
        y: i32,
        speed: f32,
        at_ms: u64,
    },
}

/// One window's finger.
#[derive(Debug, Default)]
pub(crate) struct Pan {
    state: State,
    /// Where the finger came down (client, physical): the scroll events of the swipe are for what is there.
    origin: (i32, i32),
}

impl Pan {
    /// A left press at (`x`, `y`). True: it is a finger's and is held back.
    pub(crate) fn press(&mut self, x: i32, y: i32, lparam: isize, touch: bool) -> bool {
        // (a press that was still held, or a swipe whose release never came, is forgotten: the app heard of neither)
        self.state = State::Idle;
        if !touch {
            return false;
        }
        self.state = State::Held { x, y, lparam };
        self.origin = (x, y);
        true
    }

    /// Whether a finger's press is held or a swipe runs: only then are the moves and the release looked at here.
    pub(crate) fn busy(&self) -> bool {
        self.state != State::Idle
    }

    pub(crate) fn origin(&self) -> (i32, i32) {
        self.origin
    }

    /// A move to (`x`, `y`) with the left button `down` or not, `slop` in physical pixels.
    pub(crate) fn moved(&mut self, x: i32, y: i32, down: bool, slop: f32, now_ms: u64) -> OnMove {
        if !down && self.state != State::Idle {
            // the release was lost (another window took the mouse): nothing is owed to the app, which never got the press
            self.state = State::Idle;
            return OnMove::Pass;
        }
        match self.state {
            State::Idle => OnMove::Pass,
            State::Held { x: x0, y: y0, lparam } => {
                let (dx, dy) = ((x - x0) as f32, (y - y0) as f32);
                if dx.abs().max(dy.abs()) < slop {
                    OnMove::Swallow
                } else if dx.abs() > 2.0 * dy.abs() {
                    self.state = State::Idle;
                    OnMove::Release(lparam)
                } else {
                    self.state = State::Panning { y, speed: 0.0, at_ms: now_ms };
                    OnMove::Scroll(dy)
                }
            }
            State::Panning { y: y0, speed, at_ms } => {
                let d = (y - y0) as f32;
                if d == 0.0 {
                    // (no way made up or down: the time of the last real move stays, a finger that rests is not flung)
                    return OnMove::Swallow;
                }
                // (the speed of the last few moves, the newest counting most: one late message must not decide the fling)
                let dt = now_ms.saturating_sub(at_ms);
                let speed = if dt == 0 { speed } else { 0.6 * (d * 1000.0 / dt as f32) + 0.4 * speed };
                self.state = State::Panning { y, speed, at_ms: now_ms };
                OnMove::Scroll(d)
            }
        }
    }

    /// The left button's release.
    pub(crate) fn release(&mut self, now_ms: u64) -> OnUp {
        let was = std::mem::take(&mut self.state);
        match was {
            State::Idle => OnUp::Pass,
            State::Held { lparam, .. } => OnUp::Tap(lparam),
            State::Panning { speed, at_ms, .. } => {
                let rested = now_ms.saturating_sub(at_ms) >= REST_MS;
                OnUp::Swiped(if rested || speed.abs() < FLING_FROM { 0.0 } else { speed })
            }
        }
    }
}

/// A scroll running out after the finger left.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Fling {
    /// Physical pixels a second, down positive.
    pub(crate) speed: f32,
}

impl Fling {
    /// The way made in the next `dt` seconds (physical pixels); `None` when the fling is over.
    pub(crate) fn step(&mut self, dt: f32) -> Option<f32> {
        if self.speed.abs() < FLING_UNTIL {
            return None;
        }
        let keep = (-dt / FLING_TAU).exp();
        let way = self.speed * FLING_TAU * (1.0 - keep);
        self.speed *= keep;
        Some(way)
    }
}

/// Whether the mouse message being handled was made by Windows from a finger on a touch screen (not from a pen, which has the same
/// signature without the touch bit). `GPUI_TEST_TOUCH=1`: every message counts as one, for tests on a machine without a touch screen.
pub(crate) fn from_touch() -> bool {
    static TEST: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *TEST.get_or_init(|| std::env::var("GPUI_TEST_TOUCH").is_ok_and(|v| v.trim() == "1")) {
        return true;
    }
    let extra = unsafe { windows::Win32::UI::WindowsAndMessaging::GetMessageExtraInfo() }.0 as u32;
    is_touch_signature(extra)
}

/// `MI_WP_SIGNATURE` in the upper 24 bits, and the bit that tells a finger from a pen.
fn is_touch_signature(extra: u32) -> bool {
    extra & 0xFFFF_FF00 == 0xFF51_5700 && extra & 0x80 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLOP_PX: f32 = 8.0;

    #[test]
    fn only_a_fingers_message_carries_the_signature() {
        assert!(is_touch_signature(0xFF51_5780));
        assert!(is_touch_signature(0xFF51_57A3));
        // a pen
        assert!(!is_touch_signature(0xFF51_5700));
        // a mouse
        assert!(!is_touch_signature(0));
        assert!(!is_touch_signature(0x80));
    }

    #[test]
    fn a_mouse_press_is_never_held() {
        let mut p = Pan::default();
        assert!(!p.press(100, 100, 7, false));
        assert!(!p.busy());
        assert_eq!(p.moved(100, 160, true, SLOP_PX, 10), OnMove::Pass);
        assert_eq!(p.release(20), OnUp::Pass);
    }

    #[test]
    fn a_finger_lifted_where_it_came_down_is_a_tap() {
        let mut p = Pan::default();
        assert!(p.press(100, 100, 7, true));
        assert_eq!(p.moved(103, 102, true, SLOP_PX, 10), OnMove::Swallow);
        assert_eq!(p.release(60), OnUp::Tap(7));
        assert!(!p.busy());
    }

    #[test]
    fn a_swipe_up_or_down_scrolls_and_the_app_never_hears_of_the_press() {
        let mut p = Pan::default();
        assert!(p.press(100, 300, 7, true));
        assert_eq!(p.moved(101, 296, true, SLOP_PX, 8), OnMove::Swallow);
        // past the slop: the whole way so far is scrolled at once, so the list does not lag behind the finger
        assert_eq!(p.moved(102, 280, true, SLOP_PX, 16), OnMove::Scroll(-20.0));
        assert_eq!(p.moved(104, 250, true, SLOP_PX, 32), OnMove::Scroll(-30.0));
        // sideways now: still the same swipe
        assert_eq!(p.moved(160, 250, true, SLOP_PX, 48), OnMove::Swallow);
        assert_eq!(p.origin(), (100, 300));
        assert!(matches!(p.release(56), OnUp::Swiped(_)));
        assert!(!p.busy());
    }

    #[test]
    fn a_diagonal_swipe_scrolls_and_a_clearly_sideways_one_is_the_drag_it_was() {
        let mut p = Pan::default();
        p.press(100, 100, 7, true);
        assert_eq!(p.moved(110, 110, true, SLOP_PX, 10), OnMove::Scroll(10.0));

        let mut p = Pan::default();
        p.press(100, 100, 9, true);
        assert_eq!(p.moved(112, 103, true, SLOP_PX, 10), OnMove::Release(9));
        // from here on the moves and the release are the app's
        assert!(!p.busy());
        assert_eq!(p.moved(140, 104, true, SLOP_PX, 20), OnMove::Pass);
        assert_eq!(p.release(30), OnUp::Pass);
    }

    #[test]
    fn a_swipe_lifted_while_moving_is_flung_and_one_that_rested_is_not() {
        let mut p = Pan::default();
        p.press(100, 400, 7, true);
        for (i, y) in [380, 350, 320, 290].into_iter().enumerate() {
            p.moved(100, y, true, SLOP_PX, 16 * (i as u64 + 1));
        }
        let OnUp::Swiped(speed) = p.release(70) else { panic!("a swipe") };
        assert!(speed < -1000.0, "about 30 px every 16 ms upward, got {speed}");

        let mut p = Pan::default();
        p.press(100, 400, 7, true);
        p.moved(100, 380, true, SLOP_PX, 16);
        p.moved(100, 350, true, SLOP_PX, 32);
        assert_eq!(p.release(32 + REST_MS), OnUp::Swiped(0.0));
    }

    #[test]
    fn a_swipe_whose_release_was_lost_ends_at_the_next_plain_move() {
        let mut p = Pan::default();
        p.press(100, 400, 7, true);
        p.moved(100, 350, true, SLOP_PX, 16);
        assert_eq!(p.moved(100, 340, false, SLOP_PX, 32), OnMove::Pass);
        assert!(!p.busy());
        // and a held press that never moved is dropped by the next press
        p.press(10, 10, 1, true);
        assert!(!p.press(20, 20, 2, false));
        assert!(!p.busy());
    }

    #[test]
    fn a_fling_runs_out_and_travels_its_speed_times_the_time_constant() {
        let mut f = Fling { speed: 2000.0 };
        let mut way = 0.0;
        let mut frames = 0;
        while let Some(d) = f.step(1.0 / 60.0) {
            way += d;
            frames += 1;
            assert!(frames < 600, "it ends");
        }
        let all = 2000.0 * FLING_TAU;
        assert!((way - all).abs() < FLING_UNTIL * FLING_TAU + 1.0, "{way} of {all}");
        assert!(frames > 30, "over a second or so, not at once: {frames}");
    }
}
