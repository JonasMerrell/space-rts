//! Touch-first gesture recogniser (tap, long-press, drag, pinch) with mouse/wheel fallback,
//! plus "hit blocking" so taps on overlay widgets don't fall through to the map.

use crate::theme::s;
use macroquad::prelude::*;

/// Movement allowed before a touch counts as a drag (fingers wobble more than mice).
const TAP_SLOP_DP: f32 = 16.0;
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
    /// Text fields drawn this/last frame, and which one has the keyboard (only ever one).
    fields: Vec<(u32, Rect)>,
    prev_fields: Vec<(u32, Rect)>,
    pub focused_field: Option<u32>,

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
    /// Mouse position when there's a mouse (computer mode / browser); None on touch devices.
    pub hover: Option<Vec2>,
    /// A touch has ever been seen: then the mouse position is just the last finger, not a hover.
    touch_seen: bool,
    /// Test hook: pretend the mouse is here.
    pub script_hover: Option<Vec2>,
    /// Characters typed this frame and whether backspace/enter were hit.
    pub chars: Vec<char>,
    pub backspace: bool,
    pub enter: bool,
    /// Android back key (Escape on desktop) this frame; whoever handles it calls `take_back`.
    pub back: bool,

    /// Scripted input for tests: per-frame pointer states (None = up) and queued text.
    script: std::collections::VecDeque<Option<(Vec2, f64)>>,
    script_text: String,
    script_enter: bool,
    script_back: bool,
    /// Frozen clock for scripted long-presses (seconds added to get_time()).
    script_clock: f64,
}

impl Input {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin_frame(&mut self) {
        self.prev_blockers = std::mem::take(&mut self.blockers);
        self.prev_fields = std::mem::take(&mut self.fields);
        self.prev_pointers = std::mem::take(&mut self.pointers);
        self.tap = None;
        self.long_press = None;
        self.drag = None;
        self.drag_pos = None;
        self.zoom = None;
        self.pressed = None;
        self.released = None;
        self.held = None;

        let scripted = self.script.pop_front();
        let t = if scripted.is_some() { vec![] } else { touches() };
        if let Some(p) = scripted {
            if let Some((p, jump)) = p {
                self.pointers = vec![(0, p)];
                self.script_clock += jump;
            }
        } else if !t.is_empty() {
            self.touch_seen = true;
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

        let now = get_time() + self.script_clock;
        let was_down = !self.prev_pointers.is_empty();
        let is_down = !self.pointers.is_empty();

        // A touch that began and ended between two frames (fast taps, adb input) only shows up
        // as an Ended touch we never saw down: treat it as a complete tap.
        if !is_down && !was_down {
            if let Some(p) = self.released.take() {
                self.pressed = Some(p);
                self.owned_by_ui = self.blocked(p);
                self.tap = Some(p);
                self.released = Some(p);
            }
        }

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

        // Focus follows taps: a tap on a field focuses it (and only it), anywhere else unfocuses.
        if let Some(p) = self.tap {
            let hit = self.prev_fields.iter().find(|(_, r)| r.contains(p)).map(|(id, _)| *id);
            if hit != self.focused_field {
                macroquad::miniquad::window::show_keyboard(hit.is_some());
            }
            self.focused_field = hit;
        }

        // Mouse: wheel zooms around the cursor (notches are ±1 natively; browsers/trackpads send
        // bigger or fractional deltas, so clamp to a few notches per frame).
        // hover only in computer mode: phones (and the browser on a phone) report the last touch as the mouse
        self.hover = self.script_hover.or_else(|| (crate::theme::wide() && !self.touch_seen && self.script.is_empty()).then(|| mouse_position().into()));
        let (_, wy) = mouse_wheel();
        if wy != 0.0 && touches().is_empty() {
            let notches = if wy.abs() > 3.0 { wy.signum() } else { wy };
            self.zoom = Some((mouse_position().into(), 1.15f32.powf(notches)));
        }
        // Right-click = long-press (node editor) for mouse users.
        if is_mouse_button_pressed(MouseButton::Right) && touches().is_empty() {
            let p: Vec2 = mouse_position().into();
            self.owned_by_ui = self.blocked(p);
            self.long_press = Some(p);
        }

        // Keyboard (computer mode). Tab / Shift+Tab move between text fields; with no field focused,
        // +/- zoom about the screen centre and the arrow keys pan.
        let shift = is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift);
        if is_key_pressed(KeyCode::Tab) && !self.prev_fields.is_empty() {
            let n = self.prev_fields.len();
            let i = self.focused_field.and_then(|f| self.prev_fields.iter().position(|(id, _)| *id == f));
            let next = match i {
                Some(i) if shift => (i + n - 1) % n,
                Some(i) => (i + 1) % n,
                None => 0,
            };
            self.focused_field = Some(self.prev_fields[next].0);
        }
        if self.focused_field.is_none() {
            let centre = vec2(screen_width() / 2.0, screen_height() / 2.0);
            if is_key_pressed(KeyCode::Equal) || is_key_pressed(KeyCode::KpAdd) {
                self.zoom = Some((centre, 1.25));
            }
            if is_key_pressed(KeyCode::Minus) || is_key_pressed(KeyCode::KpSubtract) {
                self.zoom = Some((centre, 1.0 / 1.25));
            }
            let mut pan = Vec2::ZERO;
            for (k, d) in [(KeyCode::Left, vec2(1.0, 0.0)), (KeyCode::Right, vec2(-1.0, 0.0)), (KeyCode::Up, vec2(0.0, 1.0)), (KeyCode::Down, vec2(0.0, -1.0))] {
                if is_key_down(k) {
                    pan += d;
                }
            }
            if pan != Vec2::ZERO && self.drag.is_none() {
                self.drag = Some(pan * 9.0 * s());
                self.owned_by_ui = false;
            }
        }

        self.chars.clear();
        while let Some(c) = get_char_pressed() {
            if !c.is_control() {
                self.chars.push(c);
            }
        }
        self.chars.extend(self.script_text.drain(..));
        self.back = is_key_pressed(KeyCode::Back) || is_key_pressed(KeyCode::Escape) || std::mem::take(&mut self.script_back);
        self.backspace = is_key_pressed(KeyCode::Backspace);
        self.enter = is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::KpEnter) || std::mem::take(&mut self.script_enter);
    }

    /// Consume the back key: true once per press, for the topmost thing that closes.
    pub fn take_back(&mut self) -> bool {
        std::mem::take(&mut self.back)
    }

    /// Test hook: back key on the next frame.
    pub fn inject_back(&mut self) {
        self.script_back = true;
    }

    /// Test hook: a tap at `p` (down next frame, up the frame after).
    pub fn inject_tap(&mut self, p: Vec2) {
        self.script.extend([Some((p, 0.0)), None]);
    }

    /// Test hook: hold at `p` long enough to fire a long-press, then release.
    pub fn inject_long_press(&mut self, p: Vec2) {
        self.script.extend([Some((p, 0.0)), Some((p, 1.0)), Some((p, 0.0)), None]);
    }

    /// Test hook: drag from `a` to `b` over `steps` frames.
    pub fn inject_drag(&mut self, a: Vec2, b: Vec2, steps: usize) {
        let n = steps.max(2);
        for i in 0..=n {
            self.script.push_back(Some((a + (b - a) * (i as f32 / n as f32), 0.0)));
        }
        self.script.push_back(None);
    }

    /// Test hook: type text into the focused field (and optionally press Enter).
    pub fn inject_text(&mut self, s: &str, enter: bool) {
        self.script_text.push_str(s);
        self.script_enter |= enter;
    }

    /// True while scripted input is still queued.
    pub fn scripted(&self) -> bool {
        !self.script.is_empty()
    }

    /// Register a text field for focus handling (call every frame it's drawn).
    pub fn field(&mut self, id: u32, r: Rect) {
        self.fields.push((id, r));
        self.blockers.push(r);
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
        self.tap.is_some_and(|p| r.contains(p))
    }

    /// The finger is currently down inside `r` (for pressed-button visuals).
    /// `p` is over a widget drawn last frame (so the map shouldn't react to it).
    pub fn over_ui(&self, p: Vec2) -> bool {
        self.blocked(p)
    }

    /// Mouse is over `r` and no button is held (hover highlight).
    pub fn hovering(&self, r: Rect) -> bool {
        self.held.is_none() && self.hover.is_some_and(|p| r.contains(p))
    }

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
