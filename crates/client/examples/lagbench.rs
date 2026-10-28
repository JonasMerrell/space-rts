//! Frame-cost benchmark on a busy mid-game war, no server needed: four bots (we are one of them)
//! play DAYS game days, then the real ClientBackend + Ui draw the map for FRAMES frames at phone size.
//! Splits the frame into UI drawing vs each Backend call.
//!   [DAYS=2] [SIZE=medium|large] [FRAMES=300] [SELECT=ship|body] cargo run --release -p client --example lagbench
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
    Conf { window_title: "lagbench".into(), window_width: 412, window_height: 880, high_dpi: true, ..Default::default() }
}

fn busy_game(days: f64, size: sim::system::SystemSize) -> sim::game::Game {
    use sim::game::{Game, Settings};
    let settings = Settings { name: "bench".into(), size, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0 };
    let mut g = Game::new("BENCH".into(), settings, 12345, "me");
    for (i, d) in ["hard", "hard", "normal"].iter().enumerate() {
        g.add_player(&format!("bot{i}"), Some(d.to_string())).unwrap();
    }
    g.start();
    g.players[0].bot = Some("hard".into());
    let t0 = std::time::Instant::now();
    g.advance_to(days * 86_400.0);
    g.players[0].bot = None;
    eprintln!("simulated {days} days in {:.1} s: {} ships, {} bodies", t0.elapsed().as_secs_f64(), g.ships.len(), g.system.bodies.len());
    g
}

#[macroquad::main(conf)]
async fn main() {
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or(d.into());
    let days: f64 = env("DAYS", "2").parse().unwrap();
    let size = match env("SIZE", "medium").as_str() {
        "small" => sim::system::SystemSize::Small,
        "large" => sim::system::SystemSize::Large,
        _ => sim::system::SystemSize::Medium,
    };
    let mut game = busy_game(days, size);
    {
        // how often would the server send a snapshot, and how many of them change our forecast?
        let mut g = game.clone();
        let (mut sent, mut changed) = (0, 0);
        let mut sig = space_rts::forecast::input_signature(&g.view_for(0), 0);
        let mut ev = g.next_event;
        for _ in 0..120 {
            let t = g.t + 60.0;
            g.advance_to(t);
            if g.next_event != ev {
                ev = g.next_event;
                sent += 1;
                let s2 = space_rts::forecast::input_signature(&g.view_for(0), 0);
                changed += (s2 != sig) as u32;
                sig = s2;
            }
        }
        println!("next 2 game hours: {sent} event snapshots (+ one every 20 s), {changed} change our forecast inputs");
    }
    let view = game.view_for(0);
    ui::art::load("assets").await;
    ui::art::set_muted(true);
    let mut ui = Ui::new();
    let mut be = Timed { inner: ClientBackend::new("127.0.0.1:1".into()), acc: RefCell::new(HashMap::new()) };
    let json = serde_json::to_string(&view).unwrap();
    let t = Instant::now();
    let parsed: sim::game::Game = serde_json::from_str(&json).unwrap();
    let t_parse = t.elapsed().as_secs_f64();
    let t = Instant::now();
    be.inner.inject_state(parsed, 0, space_rts::unix_now());
    println!("snapshot: {} KB JSON, parse {:.1} ms, on_state {:.1} ms", json.len() / 1024, t_parse * 1e3, t.elapsed().as_secs_f64() * 1e3);
    let t = Instant::now();
    while be.inner.forecast_fresh_until() < f64::INFINITY && t.elapsed().as_secs_f64() < 120.0 {
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("forecast (7 days ahead) took {:.2} s on its worker thread", t.elapsed().as_secs_f64());
    // settle: screen switch, forecast thread
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < env("SETTLE", "4").parse::<f64>().unwrap() {
        ui.frame(&mut be);
        next_frame().await;
    }
    // the server's next snapshots (it re-sends the game on every event and every 20 s)
    for _ in 0..3 {
        let t = game.t + 60.0;
        game.advance_to(t);
        let view = game.view_for(0);
        let t = Instant::now();
        be.inner.inject_state(view, 0, space_rts::unix_now());
        println!("next snapshot: on_state {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
        ui.frame(&mut be);
        next_frame().await;
    }
    if let ui::Screen::Game(g) = &mut ui.screen {
        let v = be.view().unwrap();
        match env("SELECT", "").as_str() {
            "ship" => g.selected = v.ships.iter().find(|s| s.owner == v.me && s.trajectory.is_some()).map(|s| Focus::Ship(s.id)),
            "body" => g.selected = v.bodies.iter().find(|b| b.owner == Some(v.me)).map(|b| Focus::Body(b.id)),
            _ => {}
        }
        // ZOOM=k: k times closer, centred on our home world
        if let Ok(k) = env("ZOOM", "").parse::<f64>() {
            let home = v.bodies.iter().find(|b| b.owner == Some(v.me) && b.kind == BodyKind::Planet).map(|b| b.id).unwrap_or(1);
            let mut cam = g.cam.unwrap();
            cam.zoom *= k;
            cam.center = be.body_pos(home, v.now);
            g.cam = Some(cam);
        }
    } else {
        panic!("not on the game screen");
    }
    let (ships, flying, pts) = be.view().map(|v| {
        let fl: Vec<_> = v.ships.iter().filter_map(|s| s.trajectory.as_ref()).collect();
        (v.ships.len(), fl.len(), fl.iter().map(|t| t.points.len()).sum::<usize>())
    }).unwrap_or_default();
    be.acc.borrow_mut().clear();
    let n: usize = env("FRAMES", "300").parse().unwrap();
    let t1 = Instant::now();
    let mut worst = 0.0f64;
    let mut frame_sum = 0.0;
    let mut times = vec![];
    for _ in 0..n {
        let f0 = Instant::now();
        ui.frame(&mut be);
        let ft = f0.elapsed().as_secs_f64();
        times.push(ft);
        frame_sum += ft;
        worst = worst.max(ft);
        next_frame().await;
    }
    times.sort_by(f64::total_cmp);
    let wall = t1.elapsed().as_secs_f64();
    let acc = be.acc.borrow();
    let backend: f64 = acc.values().map(|v| v.0).sum();
    println!("ships in view: {ships} ({flying} with paths, {pts} path points)");
    println!("frames {n}: wall {:.1} ms/frame (incl. GPU/present), ui.frame avg {:.2} ms, p50 {:.2}, p95 {:.2}, worst {:.2} ms", wall / n as f64 * 1e3, frame_sum / n as f64 * 1e3, times[n / 2] * 1e3, times[n * 95 / 100] * 1e3, worst * 1e3);
    println!("  of which backend calls {:.2} ms/frame, UI-side {:.2} ms/frame", backend / n as f64 * 1e3, (frame_sum - backend) / n as f64 * 1e3);
    let mut v: Vec<_> = acc.iter().collect();
    v.sort_by(|a, b| b.1 .0.total_cmp(&a.1 .0));
    for (k, (t, c)) in v.iter().take(12) {
        println!("    {k:<22} {:>7.3} ms/frame  {:>7.1} calls/frame", t / n as f64 * 1e3, *c as f64 / n as f64);
    }
    std::process::exit(0);
}
