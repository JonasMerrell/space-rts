//! Touch-first gesture recogniser (tap, long-press, drag, pinch) with mouse/wheel fallback,
//! plus "hit blocking" so taps on overlay widgets don't fall through to the map.

use crate::theme::s;
use macroquad::prelude::*;

const TAP_SLOP_DP: f32 = 10.0;
const LONG_PRESS_S: f64 = 0.5;

#[derive(Default)]
pub struct Input {
    /// Pointer positions this frame (touch ids, or 0 for the mouse).
    pointers: Vec<(u64, Vec2)>,
    prev_pointers: Vec<(u64, Vec2)>,
    start: Option<(Vec2, f64)>,
    moved: bool,
    long_fired: bool,
    pinch_prev: Option<(Vec2, f32)>,
    /// Gesture began on a widget; the map must ignore it.
    owned_by_ui: bool,
    blockers: Vec<Rect>,
    prev_blockers: Vec<Rect>,

    // Results for this frame.
    pub tap: Option<Vec2>,
    pub long_press: Option<Vec2>,
    /// Single-finger drag delta in px.
    pub drag: Option<Vec2>,
    pub drag_pos: Option<Vec2>,
    /// (centre, scale factor) for pinch / wheel zoom.
    pub zoom: Option<(Vec2, f32)>,
    /// Pointer went down this frame.
    pub pressed: Option<Vec2>,
    pub released: Option<Vec2>,
    /// Primary pointer position while down.
    pub held: Option<Vec2>,
    /// Characters typed this frame and whether backspace/enter were hit.
    pub chars: Vec<char>,
    pub backspace: bool,
    pub enter: bool,
}

impl Input {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin_frame(&mut self) {
        self.prev_blockers = std::mem::take(&mut self.blockers);
        self.prev_pointers = std::mem::take(&mut self.pointers);
        self.tap = None;
        self.long_press = None;
        self.drag = None;
        self.drag_pos = None;
        self.zoom = None;
        self.pressed = None;
        self.released = None;
        self.held = None;

        let t = touches();
        if !t.is_empty() {
            self.pointers = t
                .iter()
                .filter(|t| !matches!(t.phase, TouchPhase::Ended | TouchPhase::Cancelled))
                .map(|t| (t.id, t.position))
                .collect();
            if self.pointers.is_empty() {
                // Every touch ended this frame: report a release at the last spot.
                self.released = t.first().map(|t| t.position);
            }
        } else if is_mouse_button_down(MouseButton::Left) {
            self.pointers = vec![(0, mouse_position().into())];
        }

        let now = get_time();
        let was_down = !self.prev_pointers.is_empty();
        let is_down = !self.pointers.is_empty();

        if is_down && !was_down {
            let p = self.pointers[0].1;
            self.start = Some((p, now));
            self.moved = false;
            self.long_fired = false;
            self.pressed = Some(p);
            self.owned_by_ui = self.blocked(p);
        }
        if !is_down && was_down {
            let p = self.released.unwrap_or(self.prev_pointers[0].1);
            self.released = Some(p);
            if !self.moved && !self.long_fired {
                self.tap = Some(p);
            }
            self.start = None;
            self.pinch_prev = None;
        }

        if self.pointers.len() >= 2 {
            let (a, b) = (self.pointers[0].1, self.pointers[1].1);
            let (c, d) = ((a + b) / 2.0, a.distance(b).max(1.0));
            if let Some((pc, pd)) = self.pinch_prev {
                if !self.owned_by_ui {
                    self.zoom = Some((c, d / pd));
                    self.drag = Some(c - pc);
                }
            }
            self.pinch_prev = Some((c, d));
            self.moved = true;
        } else if self.pointers.len() == 1 {
            self.pinch_prev = None;
            let p = self.pointers[0].1;
            self.held = Some(p);
            if let Some((sp, st)) = self.start {
                if p.distance(sp) > TAP_SLOP_DP * s() {
                    self.moved = true;
                }
                if !self.moved && !self.long_fired && now - st > LONG_PRESS_S {
                    self.long_fired = true;
                    self.long_press = Some(p);
                }
            }
            if self.moved {
                if let Some(&(id, pp)) = self.prev_pointers.first() {
                    if id == self.pointers[0].0 && self.prev_pointers.len() == 1 {
                        self.drag = Some(p - pp);
                        self.drag_pos = Some(p);
                    }
                }
            }
        }

        let (_, wy) = mouse_wheel();
        if wy != 0.0 && touches().is_empty() {
            self.zoom = Some((mouse_position().into(), if wy > 0.0 { 1.15 } else { 1.0 / 1.15 }));
        }

        self.chars.clear();
        while let Some(c) = get_char_pressed() {
            if !c.is_control() {
                self.chars.push(c);
            }
        }
        self.backspace = is_key_pressed(KeyCode::Backspace);
        self.enter = is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::KpEnter);
    }

    /// Declare an on-screen widget rect; gestures starting there next frame belong to the UI.
    pub fn block(&mut self, r: Rect) {
        self.blockers.push(r);
    }

    fn blocked(&self, p: Vec2) -> bool {
        self.prev_blockers.iter().chain(self.blockers.iter()).any(|r| r.contains(p))
    }

    /// True if this frame's gesture belongs to the map (did not start on a widget).
    pub fn for_map(&self) -> bool {
        !self.owned_by_ui
    }

    /// A tap landed in `r`.
    pub fn tapped(&self, r: Rect) -> bool {
        self.tap.is_some_and(|p| r.contains(p)) && self.start_in(r)
    }

    fn start_in(&self, _r: Rect) -> bool {
        // Taps are only reported for gestures that didn't move, so start ≈ end.
        true
    }

    /// The finger is currently down inside `r` (for pressed-button visuals).
    pub fn holding(&self, r: Rect) -> bool {
        self.held.is_some_and(|p| r.contains(p))
    }

    /// Eat the tap so nothing else reacts to it.
    pub fn consume_tap(&mut self) {
        self.tap = None;
    }

    pub fn gesture_on_ui(&self) -> bool {
        self.owned_by_ui
    }
}
