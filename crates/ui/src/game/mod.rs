//! In-game screen: map + HUD + time scrubber + selection sheet + planner/node editor + chat + toasts.

pub mod map;
mod panels;

use crate::backend::{Backend, UiAction};
use crate::input::Input;
use crate::model::*;
use crate::theme::*;
use macroquad::prelude::*;
use map::Camera;

/// How far ahead the scrubber can predict (seconds).
pub const SCRUB_SPAN: f64 = 2.0 * 86400.0;

#[derive(Clone, Debug)]
pub struct NodeEdit {
    pub ship: ShipId,
    pub nodes: Vec<ManeuverNode>,
    pub sel: usize,
    pub preview: Option<Trajectory>,
    pub dirty: bool,
    pub drag: Option<Handle>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Handle {
    Prograde,
    Retrograde,
    RadialOut,
    RadialIn,
    /// Sliding the node along the path.
    Time,
}

#[derive(Clone, Debug)]
pub enum Mode {
    Idle,
    /// A ship is picked; next tap on a body plans a transfer.
    Target(ShipId),
    Plan(TransferPlan),
    Nodes(NodeEdit),
}

pub struct Toast {
    pub notice: Notice,
    pub born: f64,
}

pub struct GameScreen {
    pub cam: Option<Camera>,
    /// Seconds ahead of `now` the map is showing.
    pub scrub: f64,
    pub selected: Option<Focus>,
    pub mode: Mode,
    pub chat_open: bool,
    pub chat_seen: usize,
    pub chat_field: crate::widgets::TextField,
    pub toasts: Vec<Toast>,
    pub hint: Option<(String, f64)>,
    out: Vec<UiAction>,
}

impl Default for GameScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl GameScreen {
    pub fn new() -> Self {
        GameScreen {
            cam: None,
            scrub: 0.0,
            selected: None,
            mode: Mode::Idle,
            chat_open: false,
            chat_seen: 0,
            chat_field: crate::widgets::TextField::new(false),
            toasts: vec![],
            hint: None,
            out: vec![],
        }
    }

    pub fn push_notice(&mut self, n: Notice) {
        self.toasts.push(Toast { notice: n, born: get_time() });
    }

    fn say(&mut self, s: &str) {
        self.hint = Some((s.to_string(), get_time()));
    }

    pub fn draw(&mut self, inp: &mut Input, be: &mut dyn Backend) {
        {
            let be_ref: &dyn Backend = be;
            let Some(view) = be_ref.view() else {
                inked("LOADING SYSTEM...", screen_width() / 2.0, screen_height() / 2.0, 28.0, PAPER, Align::Center);
                return;
            };
            if self.cam.is_none() {
                self.cam = Some(Camera::fit(view));
            }
            let t = view.now + self.scrub;

            // Overlays are drawn after the map but must win the input, so they register their
            // rects (Input::block) and the map only handles gestures that started elsewhere.
            self.handle_map_input(inp, be_ref, view, t);

            let cam = self.cam.unwrap();
            map::draw_orbits(be_ref, view, &cam, t);
            map::draw_trajectories(be_ref, view, &cam, t, self.selected_ship());
            self.draw_mode_world(be_ref, view, &cam, t);
            map::draw_bodies(be_ref, view, &cam, t, self.selected);
            map::draw_ships(be_ref, view, &cam, t, self.selected);
            self.draw_mode_overlay(be_ref, view, &cam, t);
            if self.scrub > 1.0 {
                panels::prediction_banner(self.scrub);
            }

            panels::hud(self, inp, view);
            panels::scrubber(self, inp, view);
            panels::sheet(self, inp, be_ref, view);
            panels::toasts(self, inp, be_ref, view);
            panels::hint(self);
            if self.chat_open {
                panels::chat(self, inp, view);
            }
        }
        for a in self.out.drain(..) {
            be.act(a);
        }
    }

    fn selected_ship(&self) -> Option<ShipId> {
        match (&self.mode, self.selected) {
            (Mode::Plan(p), _) => Some(p.ship),
            (Mode::Nodes(n), _) => Some(n.ship),
            (Mode::Target(s), _) => Some(*s),
            (_, Some(Focus::Ship(s))) => Some(s),
            _ => None,
        }
    }

    fn handle_map_input(&mut self, inp: &mut Input, be: &dyn Backend, view: &GameView, t: f64) {
        let mut cam = self.cam.unwrap();
        let fit = Camera::fit(view);
        let on_map = inp.for_map() && !self.chat_open;

        // Node handles get first dibs on drags.
        let node_dragging = if let Mode::Nodes(ne) = &mut self.mode {
            on_map && panels::node_drag(ne, inp, be, view, &cam, t)
        } else {
            false
        };

        if on_map && !node_dragging {
            if let Some((c, k)) = inp.zoom {
                cam.zoom_at(c, k, fit.zoom * 0.5, fit.zoom * 400.0);
            }
            if let Some(d) = inp.drag {
                cam.pan_px(d);
            }
        }
        self.cam = Some(cam);
        if !on_map || node_dragging {
            return;
        }

        if let Some(p) = inp.long_press {
            self.on_long_press(be, view, &cam, t, p);
        }
        if let Some(p) = inp.tap {
            inp.consume_tap();
            self.on_tap(be, view, &cam, t, p);
        }
    }

    fn on_tap(&mut self, be: &dyn Backend, view: &GameView, cam: &Camera, t: f64, p: Vec2) {
        let hit = map::hit_test(be, view, cam, t, p);
        match (&self.mode, hit) {
            (Mode::Target(ship), Some(Focus::Body(b))) => {
                let ship = *ship;
                self.plan(be, view, ship, b, view.now);
            }
            (Mode::Target(_), Some(Focus::Ship(s))) => self.pick_ship(view, s),
            (Mode::Target(_), None) => {
                self.mode = Mode::Idle;
                self.selected = None;
            }
            (Mode::Plan(_), _) => {
                // Taps on the map while previewing a plan pick a different target or bail out.
                if let (Some(Focus::Body(b)), Mode::Plan(pl)) = (hit, &self.mode) {
                    let ship = pl.ship;
                    self.plan(be, view, ship, b, view.now);
                } else {
                    self.mode = Mode::Idle;
                }
            }
            (Mode::Nodes(_), _) => {}
            (Mode::Idle, Some(Focus::Ship(s))) => self.pick_ship(view, s),
            (Mode::Idle, h) => self.selected = h,
        }
    }

    fn pick_ship(&mut self, view: &GameView, s: ShipId) {
        self.selected = Some(Focus::Ship(s));
        if view.ship(s).is_some_and(|sh| sh.owner == view.me) {
            self.mode = Mode::Target(s);
            self.say("TAP A TARGET!");
        } else {
            self.mode = Mode::Idle;
        }
    }

    pub fn plan(&mut self, be: &dyn Backend, view: &GameView, ship: ShipId, target: BodyId, after: f64) {
        if view.ship(ship).is_some_and(|s| s.parked_at == Some(target) && !map::is_flying(s, view.now)) {
            self.say("ALREADY THERE!");
            return;
        }
        match be.plan_transfer(ship, target, after) {
            Some(plan) => {
                self.selected = Some(Focus::Ship(ship));
                self.mode = Mode::Plan(plan);
            }
            None => self.say("NO ROUTE! (fuel?)"),
        }
    }

    fn on_long_press(&mut self, be: &dyn Backend, view: &GameView, cam: &Camera, t: f64, p: Vec2) {
        // Long-press on one of our ships, or on its path, opens the node editor there.
        let mine: Vec<&Ship> = view.ships.iter().filter(|s| s.owner == view.me).collect();
        let mut pick: Option<(ShipId, f64)> = None;
        if let Some(Focus::Ship(id)) = map::hit_test(be, view, cam, t, p) {
            if mine.iter().any(|s| s.id == id) {
                pick = Some((id, t.max(view.now) + 600.0));
            }
        }
        if pick.is_none() {
            for s in &mine {
                if let Some(tr) = &s.trajectory {
                    let fut: Vec<_> = tr.points.iter().copied().filter(|q| q.0 >= view.now).collect();
                    if let Some(tt) = map::nearest_on(&fut, cam, p, 24.0 * s_dp()) {
                        pick = Some((s.id, tt));
                        break;
                    }
                }
            }
        }
        let Some((ship, nt)) = pick else { return };
        let sh = view.ship(ship).unwrap();
        let mut nodes = sh.nodes.clone();
        let sel = match nodes.iter().position(|n| (n.t - nt).abs() < 1800.0) {
            Some(i) => i,
            None => {
                nodes.push(ManeuverNode { t: nt, prograde: 0.0, radial: 0.0 });
                nodes.sort_by(|a, b| a.t.total_cmp(&b.t));
                nodes.iter().position(|n| n.t == nt).unwrap()
            }
        };
        let preview = be.preview_nodes(ship, &nodes);
        self.selected = Some(Focus::Ship(ship));
        self.mode = Mode::Nodes(NodeEdit { ship, nodes, sel, preview, dirty: false, drag: None });
        self.say("DRAG THE ARROWS!");
    }

    fn draw_mode_world(&mut self, be: &dyn Backend, view: &GameView, cam: &Camera, t: f64) {
        match &self.mode {
            Mode::Plan(plan) => panels::plan_world(plan, be, view, cam, t),
            Mode::Nodes(ne) => panels::nodes_world(ne, be, view, cam, t),
            _ => {}
        }
    }

    fn draw_mode_overlay(&mut self, be: &dyn Backend, view: &GameView, cam: &Camera, t: f64) {
        if let Mode::Nodes(ne) = &self.mode {
            panels::node_handles(ne, be, view, cam, t);
        }
        if let Mode::Plan(plan) = &self.mode {
            panels::plan_bubble(plan, be, view, cam, t);
        }
    }

    /// Centre the camera on a focus target.
    pub fn focus(&mut self, be: &dyn Backend, f: Focus, t: f64) {
        let p = match f {
            Focus::Body(b) => Some(be.body_pos(b, t)),
            Focus::Ship(s) => be.ship_pos(s, t),
        };
        if let (Some(p), Some(cam)) = (p, self.cam.as_mut()) {
            cam.center = p;
        }
        self.selected = Some(f);
    }

    pub(crate) fn emit(&mut self, a: UiAction) {
        self.out.push(a);
    }
}

fn s_dp() -> f32 {
    s()
}
