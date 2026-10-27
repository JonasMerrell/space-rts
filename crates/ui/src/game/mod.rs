//! In-game screen: map + HUD + time scrubber + selection sheet + planner/node editor + chat + toasts.

pub mod details;
pub mod fleet;
mod lobby;
pub mod map;
mod panels;
mod report;
mod research;

use crate::backend::{Backend, UiAction};
use crate::input::Input;
use crate::model::*;
use crate::theme::*;
use macroquad::prelude::*;
use map::Camera;

/// Scrubber ranges the player can pick (seconds ahead, label).
pub const SCRUB_RANGES: [(f64, &str); 5] = [(3600.0, "1H"), (36_000.0, "10H"), (86_400.0, "1D"), (3.0 * 86_400.0, "3D"), (7.0 * 86_400.0, "1W")];

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
    /// Waiting on `Backend::plan_transfer_async` ("plotting course...").
    Planning { ship: ShipId, target: BodyId, after: f64 },
    Nodes(NodeEdit),
    /// Our asteroid is picked; next tap on a planet/moon previews an impact.
    Aim(BodyId),
    /// Waiting on `Backend::plan_impact_async`.
    Impacting { asteroid: BodyId, target: BodyId },
    Impact(ImpactPreview),
}

pub struct Toast {
    pub notice: Notice,
    pub born: f64,
}

pub struct GameScreen {
    pub cam: Option<Camera>,
    /// Seconds ahead of `now` the map is showing.
    pub scrub: f64,
    /// Full length of the scrubber bar (one of `SCRUB_RANGES`).
    pub scrub_span: f64,
    /// Range picker above the scrubber is open.
    pub ranges_open: bool,
    /// Host's game-speed slider value while dragging / until the server confirms it.
    pub speed_drag: Option<f64>,
    /// `scrub` last frame: when it changes the camera follows the selection.
    last_scrub: f64,
    pub selected: Option<Focus>,
    pub mode: Mode,
    pub chat_open: bool,
    pub chat_seen: usize,
    pub chat_field: crate::widgets::TextField,
    pub toasts: Vec<Toast>,
    pub hint: Option<(String, f64)>,
    /// Owned-body sheet tab: 0 upgrades, 1 shipyard.
    pub body_tab: usize,
    /// Upgrade/ship-class detail sheet shown over the planet panel.
    pub detail: Option<details::Detail>,
    /// Node-editor projection for (ship, nodes) — recomputed when the nodes change.
    pub node_outcome: Option<(ShipId, Vec<ManeuverNode>, NodeOutcome)>,
    /// Camera follows the selection; a manual pan turns it off, a new selection or RE-CENTRE turns it on.
    pub follow: bool,
    /// Camera animation: zoom level to ease to (ZOOM TO / SYSTEM VIEW), and whether the centre is
    /// easing to the star (system view) instead of following the selection. Manual pan/zoom cancels.
    pub zoom_goal: Option<f64>,
    pub system_view: bool,
    /// Chat list scroll, px up from the newest message (0 = pinned to the newest).
    pub chat_scroll: f32,
    /// Resource breakdown sheet (0 matter, 1 energy, 2 fuel) opened from the top bar.
    pub res_open: Option<usize>,
    /// In-game menu popover open.
    pub menu_open: bool,
    /// Research overlay open, and which branch tab it shows.
    pub research_open: bool,
    pub research_branch: usize,
    /// Station picker open for this body, and the highlighted kind.
    pub stations_for: Option<BodyId>,
    pub station_pick: Option<String>,
    /// "While you were away" sheet state.
    pub report: report::ReportState,
    /// Fleet sheet (group / split / merge ships parked together).
    pub fleet: Option<fleet::FleetSheet>,
    /// For sound cues: chat length and bodies we owned last frame.
    /// Lobby: pilot roster expanded.
    pub lobby_roster: bool,
    /// Lobby: pilot count last seen, for the join sound.
    pub lobby_pilots: usize,
    seen_chat: usize,
    owned: Vec<BodyId>,
    last_selected: Option<Focus>,
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
            scrub_span: 86_400.0,
            ranges_open: false,
            speed_drag: None,
            last_scrub: 0.0,
            selected: None,
            mode: Mode::Idle,
            chat_open: false,
            chat_seen: 0,
            chat_field: crate::widgets::TextField::new(false),
            toasts: vec![],
            hint: None,
            body_tab: 0,
            menu_open: false,
            detail: None,
            res_open: None,
            chat_scroll: 0.0,
            zoom_goal: None,
            system_view: false,
            follow: true,
            node_outcome: None,
            research_open: false,
            research_branch: 0,
            stations_for: None,
            station_pick: None,
            report: Default::default(),
            fleet: None,
            lobby_roster: true,
            lobby_pilots: 0,
            seen_chat: usize::MAX,
            owned: vec![],
            last_selected: None,
            out: vec![],
        }
    }

    pub fn push_notice(&mut self, n: Notice) {
        crate::art::sfx(match n.kind {
            NoticeKind::Battle | NoticeKind::Bad => "alert_enemy",
            _ => "notification",
        });
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
                // Start zoomed on the inner system; pinch out for the whole map.
                let mut cam = Camera::fit(view);
                cam.zoom *= 1.8;
                // ...centred on our home world when we have one (in the lobby: where we'll start).
                let owned = view.bodies.iter().find(|b| b.owner == Some(view.me) && b.kind == BodyKind::Planet);
                if let Some(home) = owned.or_else(|| view.home.and_then(|h| view.body(h))) {
                    let p = be_ref.body_pos(home.id, view.now);
                    cam.center = [p[0] * 0.5, p[1] * 0.5];
                    if view.lobby.is_some() {
                        // the pilot roster covers the bottom half: put home in the open band under the banner
                        let at = cam.to_screen(p);
                        cam.pan_px(vec2(screen_width() / 2.0 - at.x, screen_height() * 0.36 - at.y));
                    }
                }
                self.cam = Some(cam);
            }
            if let Some(info) = &view.lobby {
                // Waiting for the host: just the map (pan, zoom, tap a body) plus the lobby overlay.
                self.scrub = 0.0;
                let t = view.now;
                self.handle_map_input(inp, be_ref, view, t);
                let cam = self.cam.unwrap();
                map::draw_orbits(be_ref, view, &cam, t);
                map::draw_bodies(be_ref, view, &cam, t, self.selected);
                lobby::draw_home(be_ref, view, &cam, t);
                map::draw_labels(be_ref, view, &cam, t);
                lobby::overlay(self, inp, view, info);
                panels::toasts(self, inp, be_ref, view);
                if self.chat_open {
                    panels::chat(self, inp, view);
                }
            } else {
            let t = view.now + self.scrub;
            self.sound_cues(view);
            self.run_pending_plan(be_ref, view.now);
            self.run_pending_impact(be_ref);
            // Scrubbed ahead: draw the forecast (sim run forward from what we know) instead of the
            // live view. `now` stays the real now.
            let predicted = if self.scrub > 1.0 { be_ref.predicted(t) } else { None };
            let simulating = self.scrub > 1.0 && predicted.is_none() && t > be_ref.forecast_until();
            let view: &GameView = predicted.as_deref().unwrap_or(view);
            // Keep whatever is selected in the middle while scrubbing.
            if self.scrub != self.last_scrub {
                if let Some(f) = self.selected {
                    self.focus(be_ref, f, t);
                }
                self.last_scrub = self.scrub;
            }

            // Overlays are drawn after the map but must win the input, so they register their
            // rects (Input::block) and the map only handles gestures that started elsewhere.
            self.handle_map_input(inp, be_ref, view, t);
            self.follow_selection(be_ref, t);

            let cam = self.cam.unwrap();
            map::draw_orbits(be_ref, view, &cam, t);
            map::draw_trajectories(be_ref, view, &cam, t, self.selected_ship());
            self.draw_mode_world(be_ref, view, &cam, t);
            map::draw_bodies(be_ref, view, &cam, t, self.selected);
            map::draw_ships(be_ref, view, &cam, t, self.selected);
            // Mouse hover (computer mode): outline what a click would select.
            if let Some(hp) = inp.hover.filter(|p| !inp.over_ui(*p) && inp.held.is_none()) {
                if let Some(f) = map::hit_test(be_ref, view, &cam, t, hp, self.selected).filter(|f| Some(*f) != self.selected) {
                    map::draw_hover(be_ref, view, &cam, t, f);
                }
            }
            map::draw_labels(be_ref, view, &cam, t);
            self.draw_mode_overlay(be_ref, view, &cam, t);
            if self.scrub > 1.0 {
                panels::prediction_banner(self.scrub, simulating);
            }

            // Full-screen overlays (away report, research) cover everything and must get the input
            // first, so the HUD/sheets underneath aren't drawn or tapped while they're open. Before this,
            // the HUD menu button under the research X ate the tap and the screen wouldn't close.
            if be_ref.away_report().is_some() {
                report::away_report(self, inp, be_ref, view);
            } else if self.research_open {
                if inp.take_back() {
                    self.research_open = false;
                    crate::art::sfx("menu_close");
                } else {
                    research::research_screen(self, inp, be_ref, view);
                }
            } else {
                self.handle_back(inp);
                panels::hud(self, inp, view);
                // phone: the chat covers the sheet area, so don't run (or hit-test) what's under it
                if !(self.chat_open && !wide()) {
                    panels::sheet(self, inp, be_ref, view);
                }
                panels::scrubber(self, inp, be_ref, view);
                if !self.chat_open {
                    panels::camera_buttons(self, inp, be_ref, view);
                }
                // Toasts sit over the chat panel's corner; hold them while chatting.
                if !self.chat_open {
                    panels::toasts(self, inp, be_ref, view);
                }
                panels::hint(self);
                if self.chat_open {
                    panels::chat(self, inp, view);
                }
            }
            }
        }
        for a in self.out.drain(..) {
            be.act(a);
        }
    }

    /// Back key closes the topmost open thing: chat, menu, detail/breakdown sheets, the current
    /// mode, then the selection.
    fn handle_back(&mut self, inp: &mut Input) {
        if !inp.take_back() {
            return;
        }
        crate::art::sfx("menu_close");
        if self.chat_open {
            self.chat_open = false;
            macroquad::miniquad::window::show_keyboard(false);
        } else if self.menu_open {
            self.menu_open = false;
        } else if self.detail.is_some() {
            self.detail = None;
        } else if self.res_open.is_some() {
            self.res_open = None;
        } else if self.stations_for.is_some() {
            self.stations_for = None;
        } else if !matches!(self.mode, Mode::Idle) {
            self.mode = Mode::Idle;
        } else {
            self.selected = None;
        }
    }

    fn selected_ship(&self) -> Option<ShipId> {
        match (&self.mode, self.selected) {
            (Mode::Plan(p), _) => Some(p.ship),
            (Mode::Planning { ship, .. }, _) => Some(*ship),
            (Mode::Nodes(n), _) => Some(n.ship),
            (Mode::Target(s), _) => Some(*s),
            (_, Some(Focus::Ship(s))) => Some(s),
            _ => None,
        }
    }

    fn handle_map_input(&mut self, inp: &mut Input, be: &dyn Backend, view: &GameView, t: f64) {
        let mut cam = self.cam.unwrap();
        let fit = Camera::fit(view);
        let on_map = inp.for_map() && !self.chat_open && !self.research_open && be.away_report().is_none();

        // Node handles get first dibs on drags.
        let node_dragging = if let Mode::Nodes(ne) = &mut self.mode {
            on_map && panels::node_drag(ne, inp, be, view, &cam, t)
        } else {
            false
        };

        if on_map && !node_dragging {
            let following = self.follow && self.selected.is_some();
            if let Some((c, k)) = inp.zoom {
                // While following, zoom about the screen centre so the selection stays put.
                let c = if following { follow_point() } else { c };
                cam.zoom_at(c, k, fit.zoom * 0.5, fit.zoom * 400.0);
                self.zoom_goal = None;
                self.system_view = false;
            }
            if let Some(d) = inp.drag {
                // a deliberate pan (not pinch jitter) breaks the follow
                if inp.zoom.is_none() && d.length() > 0.5 {
                    self.follow = false;
                    self.zoom_goal = None;
                    self.system_view = false;
                }
                if !(following && inp.zoom.is_some()) {
                    cam.pan_px(d);
                }
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
        let hit = map::hit_test(be, view, cam, t, p, self.selected);
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
            (Mode::Aim(a), Some(Focus::Body(b))) | (Mode::Impact(ImpactPreview { asteroid: a, .. }), Some(Focus::Body(b)))
                if *a != b && view.body(b).is_some_and(|x| matches!(x.kind, BodyKind::Planet | BodyKind::Moon)) =>
            {
                let asteroid = *a;
                self.selected = Some(Focus::Body(asteroid));
                self.mode = Mode::Impacting { asteroid, target: b };
            }
            (Mode::Aim(_), Some(_)) => self.say("Pick a planet or moon"),
            (Mode::Aim(_), None) | (Mode::Impact(_), _) => self.mode = Mode::Idle,
            (Mode::Nodes(_), _) | (Mode::Planning { .. }, _) | (Mode::Impacting { .. }, _) => {}
            (Mode::Idle, Some(Focus::Ship(s))) => self.pick_ship(view, s),
            (Mode::Idle, h) => self.selected = h,
        }
    }

    fn pick_ship(&mut self, view: &GameView, s: ShipId) {
        self.selected = Some(Focus::Ship(s));
        if view.ship(s).is_some_and(|sh| sh.owner == view.me) {
            self.mode = Mode::Target(s);
            crate::art::sfx("ui_click");
            self.say("Tap a target");
        } else {
            self.mode = Mode::Idle;
        }
    }

    pub fn plan(&mut self, _be: &dyn Backend, view: &GameView, ship: ShipId, target: BodyId, after: f64) {
        if view.ship(ship).is_some_and(|s| s.parked_at == Some(target) && !map::is_flying(s, view.now)) {
            self.say("Already there");
            return;
        }
        self.selected = Some(Focus::Ship(ship));
        self.mode = Mode::Planning { ship, target, after };
    }

    fn run_pending_impact(&mut self, be: &dyn Backend) {
        let Mode::Impacting { asteroid, target } = self.mode else { return };
        match be.plan_impact_async(asteroid, target) {
            crate::ImpactPoll::Pending => {}
            crate::ImpactPoll::Done(Some(p)) => self.mode = Mode::Impact(p),
            crate::ImpactPoll::Done(None) => {
                crate::art::sfx("ui_error");
                self.mode = Mode::Aim(asteroid);
                self.say("Can't reach that target");
            }
        }
    }

    /// Picks up the pending plan once the backend has it.
    fn run_pending_plan(&mut self, be: &dyn Backend, now: f64) {
        let Mode::Planning { ship, target, after, .. } = self.mode else { return };
        // Polled every frame; the client plans on a worker thread so the map keeps animating.
        let res = match be.plan_transfer_async(ship, target, after) {
            crate::PlanPoll::Pending => return,
            crate::PlanPoll::Done(r) => r,
        };
        match res {
            Some(plan) => {
                self.selected = Some(Focus::Ship(ship));
                // "Next window" asks for a later departure: scrub ahead to show it.
                if after > now + 60.0 {
                    self.scrub = (plan.depart_t - now).clamp(0.0, SCRUB_RANGES[SCRUB_RANGES.len() - 1].0);
                    self.scrub_span = SCRUB_RANGES.iter().map(|r| r.0).find(|&r| r >= self.scrub).unwrap_or(self.scrub_span).max(self.scrub_span);
                }
                self.mode = Mode::Plan(plan);
            }
            None => {
                crate::art::sfx("ui_error");
                self.mode = Mode::Target(ship);
                self.say("No route (check fuel)")
            }
        }
    }

    fn on_long_press(&mut self, be: &dyn Backend, view: &GameView, cam: &Camera, t: f64, p: Vec2) {
        // Long-press on one of our ships, or on its path, opens the node editor there.
        let mine: Vec<&Ship> = view.ships.iter().filter(|s| s.owner == view.me).collect();
        let mut pick: Option<(ShipId, f64)> = None;
        if let Some(Focus::Ship(id)) = map::hit_test(be, view, cam, t, p, self.selected) {
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
        self.say("Drag the arrows to plan a burn");
    }

    fn draw_mode_world(&mut self, be: &dyn Backend, view: &GameView, cam: &Camera, t: f64) {
        match &self.mode {
            Mode::Plan(plan) => panels::plan_world(plan, be, view, cam, t),
            Mode::Nodes(ne) => panels::nodes_world(ne, self.node_outcome.as_ref().map(|o| &o.2), be, view, cam, t),
            Mode::Impact(ip) => panels::impact_world(ip, be, view, cam, t),
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
    /// Ease the camera toward the selected body/ship every frame (planets keep moving in real time).
    fn follow_selection(&mut self, be: &dyn Backend, t: f64) {
        let k = 1.0 - (-(get_frame_time() as f64) * 6.0).exp();
        if let (Some(goal), Some(cam)) = (self.zoom_goal, self.cam.as_mut()) {
            // ease in log space so zooming in and out feel the same speed
            let z = (cam.zoom.ln() + (goal.ln() - cam.zoom.ln()) * k).exp();
            cam.zoom = z;
            if (z / goal - 1.0).abs() < 0.01 {
                cam.zoom = goal;
                self.zoom_goal = None;
            }
        }
        if self.system_view {
            if let Some(cam) = self.cam.as_mut() {
                cam.center[0] -= cam.center[0] * k;
                cam.center[1] -= cam.center[1] * k;
                if self.zoom_goal.is_none() && cam.center[0].hypot(cam.center[1]) * cam.zoom < 1.0 {
                    self.system_view = false;
                }
            }
            return;
        }
        if !self.follow {
            return;
        }
        let target = match self.selected {
            Some(Focus::Body(b)) => Some(be.body_pos(b, t)),
            Some(Focus::Ship(s)) => be.ship_pos(s, t),
            None => None,
        };
        let (Some(p), Some(cam)) = (target, self.cam.as_mut()) else { return };
        // Aim for the middle of the map band left above the selection's bottom sheet (~1/3 down),
        // not the true screen centre, which the sheet covers.
        let fp = follow_point();
        let dx = (fp.x - screen_width() / 2.0) as f64 / cam.zoom;
        let dy = (fp.y - screen_height() / 2.0) as f64 / cam.zoom;
        let goal = [p[0] - dx, p[1] - dy];
        let k = 1.0 - (-(get_frame_time() as f64) * 8.0).exp();
        cam.center[0] += (goal[0] - cam.center[0]) * k;
        cam.center[1] += (goal[1] - cam.center[1]) * k;
    }

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

    fn sound_cues(&mut self, view: &GameView) {
        use crate::art::sfx;
        if self.seen_chat != usize::MAX && view.chat.len() > self.seen_chat && view.chat.last().is_some_and(|m| m.from != view.me) {
            sfx("chat_message");
        }
        self.seen_chat = view.chat.len();
        let owned: Vec<BodyId> = view.bodies.iter().filter(|b| b.owner == Some(view.me)).map(|b| b.id).collect();
        if !self.owned.is_empty() && owned.iter().any(|b| !self.owned.contains(b)) {
            sfx("capture");
        }
        self.owned = owned;
        if self.selected != self.last_selected {
            // something new picked: centre on it again
            self.follow = true;
            if self.last_selected.is_some() {
                self.detail = None;
            }
            match (self.last_selected, self.selected) {
                (None, Some(_)) => sfx("menu_open"),
                (Some(_), None) => sfx("menu_close"),
                _ => sfx("menu_select"),
            }
            self.last_selected = self.selected;
        }
    }

    pub(crate) fn emit(&mut self, a: UiAction) {
        self.out.push(a);
    }
}

fn s_dp() -> f32 {
    s()
}

/// Where the followed selection sits on screen: the middle of the map that isn't covered by panels
/// (phone: a third down, above the bottom sheet; computer: left of the side panel).
fn follow_point() -> Vec2 {
    if wide() {
        vec2((screen_width() - side_w()) / 2.0, screen_height() * 0.5)
    } else {
        vec2(screen_width() / 2.0, screen_height() * 0.33)
    }
}
