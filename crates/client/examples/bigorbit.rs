//! Profile a big orbit stack on the real client: a 1600x900 window on an existing war (log in, open
//! CODE), time 300 frames, and split the frame into UI drawing vs each Backend call.
//!   SERVER=127.0.0.1:7791 USER=wideuser PASS=pw123456 CODE=H8LMG [SELECT=body id] cargo run --release -p client --example bigorbit
use macroquad::prelude::*;
use space_rts::backend::ClientBackend;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use ui::model::*;
use ui::{Backend, BackendEvent, ImpactPoll, PlanPoll, Ui, UiAction, WindowsPoll};

struct Timed {
    inner: ClientBackend,
    acc: RefCell<HashMap<&'static str, (f64, u64)>>,
}

impl Timed {
    fn add(&self, k: &'static str, t0: Instant) {
        let mut a = self.acc.borrow_mut();
        let e = a.entry(k).or_insert((0.0, 0));
        e.0 += t0.elapsed().as_secs_f64();
        e.1 += 1;
    }
}

impl Backend for Timed {
    fn act(&mut self, action: UiAction) {
        let t0 = Instant::now();
        self.inner.act(action);
        self.add("act", t0);
    }
    fn poll(&mut self) -> Vec<BackendEvent> {
        let t0 = Instant::now();
        let r = self.inner.poll();
        self.add("poll", t0);
        r
    }
    fn my_games(&self) -> Vec<GameSummary> {
        let t0 = Instant::now();
        let r = self.inner.my_games();
        self.add("my_games", t0);
        r
    }
    fn view(&self) -> Option<&GameView> {
        let t0 = Instant::now();
        let r = self.inner.view();
        self.add("view", t0);
        r
    }
    fn body_pos(&self, id: BodyId, t: f64) -> [f64; 2] {
        let t0 = Instant::now();
        let r = self.inner.body_pos(id, t);
        self.add("body_pos", t0);
        r
    }
    fn ship_pos(&self, id: ShipId, t: f64) -> Option<[f64; 2]> {
        let t0 = Instant::now();
        let r = self.inner.ship_pos(id, t);
        self.add("ship_pos", t0);
        r
    }
    fn plan_transfer(&self, ship: ShipId, target: BodyId, after: f64) -> Option<TransferPlan> {
        let t0 = Instant::now();
        let r = self.inner.plan_transfer(ship, target, after);
        self.add("plan_transfer", t0);
        r
    }
    fn plan_transfer_async(&self, ship: ShipId, target: BodyId, after: f64) -> PlanPoll {
        let t0 = Instant::now();
        let r = self.inner.plan_transfer_async(ship, target, after);
        self.add("plan_transfer_async", t0);
        r
    }
    fn plan_between_async(&self, a: &TransferPlan, b: &TransferPlan, f: f32) -> PlanPoll {
        let t0 = Instant::now();
        let r = self.inner.plan_between_async(a, b, f);
        self.add("plan_between_async", t0);
        r
    }
    fn plan_intercept_async(&self, ship: ShipId, target: ShipId, matched: bool) -> PlanPoll {
        let t0 = Instant::now();
        let r = self.inner.plan_intercept_async(ship, target, matched);
        self.add("plan_intercept_async", t0);
        r
    }
    fn windows_async(&self, ship: ShipId) -> WindowsPoll {
        let t0 = Instant::now();
        let r = self.inner.windows_async(ship);
        self.add("windows_async", t0);
        r
    }
    fn plan_impact_async(&self, asteroid: BodyId, target: BodyId) -> ImpactPoll {
        let t0 = Instant::now();
        let r = self.inner.plan_impact_async(asteroid, target);
        self.add("plan_impact_async", t0);
        r
    }
    fn node_outcome(&self, ship: ShipId, nodes: &[ManeuverNode]) -> Option<NodeOutcome> {
        let t0 = Instant::now();
        let r = self.inner.node_outcome(ship, nodes);
        self.add("node_outcome", t0);
        r
    }
    fn preview_nodes(&self, ship: ShipId, nodes: &[ManeuverNode]) -> Option<Trajectory> {
        let t0 = Instant::now();
        let r = self.inner.preview_nodes(ship, nodes);
        self.add("preview_nodes", t0);
        r
    }
    fn describe(&self, key: &str) -> Option<String> {
        let t0 = Instant::now();
        let r = self.inner.describe(key);
        self.add("describe", t0);
        r
    }
    fn orbit_status(&self, body: BodyId) -> Option<String> {
        let t0 = Instant::now();
        let r = self.inner.orbit_status(body);
        self.add("orbit_status", t0);
        r
    }
    fn results(&self) -> Option<GameResults> {
        let t0 = Instant::now();
        let r = self.inner.results();
        self.add("results", t0);
        r
    }
    fn construction_done(&self, body: BodyId) -> Option<f64> {
        let t0 = Instant::now();
        let r = self.inner.construction_done(body);
        self.add("construction_done", t0);
        r
    }
    fn body_info(&self, id: BodyId) -> Option<BodyInfo> {
        let t0 = Instant::now();
        let r = self.inner.body_info(id);
        self.add("body_info", t0);
        r
    }
    fn research(&self) -> Option<ResearchView> {
        let t0 = Instant::now();
        let r = self.inner.research();
        self.add("research", t0);
        r
    }
    fn ship_abilities(&self, ship: ShipId) -> Vec<Ability> {
        let t0 = Instant::now();
        let r = self.inner.ship_abilities(ship);
        self.add("ship_abilities", t0);
        r
    }
    fn stations(&self, id: BodyId) -> Option<StationsView> {
        let t0 = Instant::now();
        let r = self.inner.stations(id);
        self.add("stations", t0);
        r
    }
    fn sensors(&self) -> Option<&SensorsReport> {
        let t0 = Instant::now();
        let r = self.inner.sensors();
        self.add("sensors", t0);
        r
    }
    fn auto_login(&self) -> Option<String> {
        let t0 = Instant::now();
        let r = self.inner.auto_login();
        self.add("auto_login", t0);
        r
    }
    fn connection_status(&self) -> Option<String> {
        let t0 = Instant::now();
        let r = self.inner.connection_status();
        self.add("connection_status", t0);
        r
    }
    fn predicted(&self, t: f64) -> Option<std::rc::Rc<GameView>> {
        let t0 = Instant::now();
        let r = self.inner.predicted(t);
        self.add("predicted", t0);
        r
    }
    fn forecast_until(&self) -> f64 {
        let t0 = Instant::now();
        let r = self.inner.forecast_until();
        self.add("forecast_until", t0);
        r
    }
    fn dark_until(&self, ship: ShipId, t: f64) -> Option<f64> {
        let t0 = Instant::now();
        let r = self.inner.dark_until(ship, t);
        self.add("dark_until", t0);
        r
    }
    fn orbit_load(&self, body: BodyId) -> Option<(usize, usize)> {
        let t0 = Instant::now();
        let r = self.inner.orbit_load(body);
        self.add("orbit_load", t0);
        r
    }
    fn default_server(&self) -> String {
        let t0 = Instant::now();
        let r = self.inner.default_server();
        self.add("default_server", t0);
        r
    }
}

fn conf() -> Conf {
    Conf { window_title: "bigorbit".into(), window_width: 1600, window_height: 900, high_dpi: true, ..Default::default() }
}

#[macroquad::main(conf)]
async fn main() {
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or(d.into());
    ui::art::load("assets").await;
    ui::art::set_muted(true);
    let mut ui = Ui::new();
    let mut be = Timed { inner: ClientBackend::new(env("SERVER", "127.0.0.1:7791")), acc: RefCell::new(HashMap::new()) };
    be.act(UiAction::Login { server: String::new(), user: env("USER", "wideuser"), pass: env("PASS", "pw123456"), register: false, invite: String::new() });
    let t0 = Instant::now();
    let mut opened = false;
    // let it log in, open the war and settle (forecast etc.)
    while t0.elapsed().as_secs_f64() < 25.0 {
        if !opened && matches!(ui.screen, ui::Screen::Menu(_)) {
            opened = true;
            be.act(UiAction::OpenGame { code: env("CODE", "H8LMG") });
        }
        ui.frame(&mut be);
        next_frame().await;
    }
    if let (Ok(b), ui::Screen::Game(g)) = (env("SELECT", "").parse::<u32>(), &mut ui.screen) {
        g.selected = Some(Focus::Body(b));
    }
    let ships = be.view().map(|v| v.ships.len()).unwrap_or(0);
    be.acc.borrow_mut().clear();
    let n = 300;
    let t1 = Instant::now();
    let mut worst = 0.0f64;
    let mut frame_sum = 0.0;
    for _ in 0..n {
        let f0 = Instant::now();
        ui.frame(&mut be);
        let ft = f0.elapsed().as_secs_f64();
        frame_sum += ft;
        worst = worst.max(ft);
        next_frame().await;
    }
    let wall = t1.elapsed().as_secs_f64();
    let acc = be.acc.borrow();
    let backend: f64 = acc.values().map(|v| v.0).sum();
    println!("ships in view: {ships}");
    println!("frames {n}: wall {:.1} ms/frame (incl. GPU/present), ui.frame avg {:.2} ms, worst {:.2} ms", wall / n as f64 * 1e3, frame_sum / n as f64 * 1e3, worst * 1e3);
    println!("  of which backend calls {:.2} ms/frame, UI-side {:.2} ms/frame", backend / n as f64 * 1e3, (frame_sum - backend) / n as f64 * 1e3);
    let mut v: Vec<_> = acc.iter().collect();
    v.sort_by(|a, b| b.1 .0.total_cmp(&a.1 .0));
    for (k, (t, c)) in v.iter().take(12) {
        println!("    {k:<22} {:>7.3} ms/frame  {:>7.1} calls/frame", t / n as f64 * 1e3, *c as f64 / n as f64);
    }
    let _ = Duration::from_millis(0);
}
