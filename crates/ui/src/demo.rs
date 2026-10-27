//! Offline fake backend: a hand-made star system with Kepler orbits, Hohmann-style auto-planner
//! and a 2-body integrator for node previews. Used by `examples/demo.rs` and for screenshots;
//! the real game goes through `crates/client`.

use crate::backend::{Backend, BackendEvent, UiAction};
use crate::model::*;
use std::f64::consts::{PI, TAU};

/// Demo distances are authored in "units" and scaled to km by K.
const K: f64 = 6000.0;
/// Star GM in km³/s², chosen so a planet at 150 units (900,000 km) has a 1-day year.
const MU: f64 = 4.0 * PI * PI * (150.0 * K) * (150.0 * K) * (150.0 * K) / (86400.0 * 86400.0);

#[derive(Clone, Copy)]
struct Kep {
    parent: Option<BodyId>,
    a: f64,
    e: f64,
    w: f64,
    m0: f64,
    mu: f64,
}

impl Kep {
    fn period(&self) -> f64 {
        TAU * (self.a.powi(3) / self.mu).sqrt()
    }
    fn rel(&self, t: f64) -> [f64; 2] {
        let m = self.m0 + TAU * t / self.period();
        let mut ea = m;
        for _ in 0..8 {
            ea -= (ea - self.e * ea.sin() - m) / (1.0 - self.e * ea.cos());
        }
        let x = self.a * (ea.cos() - self.e);
        let y = self.a * (1.0 - self.e * self.e).sqrt() * ea.sin();
        let (c, s) = (self.w.cos(), self.w.sin());
        [x * c - y * s, x * s + y * c]
    }
}

pub struct DemoBackend {
    pub view: Option<GameView>,
    events: Vec<BackendEvent>,
    kep: Vec<(BodyId, Kep)>,
    t0_real: f64,
    pub speed: f64,
    pub start_t: f64,
    lobby: Option<LobbyInfo>,
    /// Upgrade levels per body (index = model::UPGRADES).
    levels: std::collections::HashMap<BodyId, [u8; 8]>,
    queue: Vec<(BodyId, String, f64, ShipClass)>,
    /// upgrade lane, side by side with the shipyard: (body, index into UPGRADES, start, done)
    up_queue: Vec<(BodyId, usize, f64, f64)>,
    /// waiting for funds: (id, body, label, cost); the demo never funds them
    waiting: Vec<(u32, BodyId, String, Cost)>,
    user: String,
    games: Vec<GameSummary>,
    /// Research: indices into DEMO_TECHS done, and (tech, lab, done_t) running.
    techs_done: Vec<usize>,
    /// ships gone dark (Heat-sink cloaking, DEMO_TECHS[30])
    dark: Vec<ShipId>,
    research_run: Vec<(usize, BodyId, f64)>,
    /// (body, kind, owner, done_t)
    stations: Vec<(BodyId, String, PlayerId, f64)>,
    /// Sensors report (the demo example fills in `demo_report`).
    pub report: Option<SensorsReport>,
    /// Scene `results`: the war has ended.
    pub results: Option<GameResults>,
}

impl Default for DemoBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl DemoBackend {
    pub fn new() -> Self {
        DemoBackend {
            view: None,
            events: vec![],
            kep: vec![],
            t0_real: macroquad::time::get_time(),
            speed: 60.0,
            start_t: 2.0 * 86400.0 + 14.0 * 3600.0,
            lobby: None,
            levels: std::collections::HashMap::new(),
            queue: vec![],
            up_queue: vec![],
            waiting: vec![],
            user: String::new(),
            techs_done: vec![0, 4, 8, 9, 16],
            dark: vec![],
            research_run: vec![],
            stations: vec![],
            report: None,
            results: None,
            games: vec![
                GameSummary { code: "K7QX".into(), name: "Friday Night War".into(), day: 3, length_days: 7, players: 4, unread: 3 },
                GameSummary { code: "ZB2M".into(), name: "Bots practice".into(), day: 1, length_days: 5, players: 3, unread: 0 },
            ],
        }
    }

    /// Demo/test hook: mark these `DEMO_TECHS` indices researched.
    pub fn set_techs_done(&mut self, done: Vec<usize>) {
        self.techs_done = done;
    }

    pub fn now(&self) -> f64 {
        self.start_t + (macroquad::time::get_time() - self.t0_real) * self.speed
    }

    fn kep(&self, id: BodyId) -> Option<&Kep> {
        self.kep.iter().find(|k| k.0 == id).map(|k| &k.1)
    }

    fn pos(&self, id: BodyId, t: f64) -> [f64; 2] {
        match self.kep(id) {
            None => [0.0, 0.0],
            Some(k) => {
                let r = k.rel(t);
                let p = k.parent.map(|p| self.pos(p, t)).unwrap_or([0.0, 0.0]);
                [p[0] + r[0], p[1] + r[1]]
            }
        }
    }

    fn sun_radius(&self, id: BodyId, t: f64) -> f64 {
        // Distance from the star of the planet (or a moon's planet).
        let mut b = id;
        while let Some(k) = self.kep(b) {
            match k.parent {
                Some(p) if p != 0 => b = p,
                _ => break,
            }
        }
        let p = self.pos(b, t);
        p[0].hypot(p[1])
    }

    /// Build the demo system and a mid-game state. Public so screenshots can jump straight in.
    /// Waiting period: the system exists but nobody has ships or planets yet, and the clock is stopped.
    pub fn enter_lobby(&mut self, info: LobbyInfo) {
        self.start_game();
        self.start_t = 0.0;
        self.speed = 0.0;
        if let Some(v) = self.view.as_mut() {
            v.home = v.bodies.iter().find(|b| b.owner == Some(v.me) && b.kind == BodyKind::Planet).map(|b| b.id);
            v.ships.clear();
            for b in v.bodies.iter_mut() {
                b.owner = None;
                b.intel = None;
            }
            v.chat.clear();
            v.resources = Resources::default();
            v.now = 0.0;
        }
        self.sync_lobby(info);
    }

    fn sync_lobby(&mut self, info: LobbyInfo) {
        if let Some(v) = self.view.as_mut() {
            v.players = info.players.clone();
            v.host = info.host;
            v.lobby = Some(info.clone());
        }
        self.lobby = Some(info.clone());
        self.events.push(BackendEvent::Lobby(info));
    }

    pub fn start_game(&mut self) {
        let mut bodies = vec![];
        let mut kep = vec![];
        let mut add = |id: BodyId, name: &str, kind: BodyKind, parent: Option<BodyId>, a: f64, e: f64, w: f64, m0: f64, radius: f64, owner: Option<PlayerId>, kep: &mut Vec<(BodyId, Kep)>| {
            let mu = match parent {
                Some(0) => MU,
                Some(_) => MU * 3e-4,
                None => MU,
            };
            let (a, radius) = (a * K, radius * K);
            let k = Kep { parent, a, e, w, m0, mu };
            let orbit_path = if parent.is_some() {
                let per = k.period();
                (0..=96).map(|i| k.rel(per * i as f64 / 96.0)).collect()
            } else {
                vec![]
            };
            if parent.is_some() {
                kep.push((id, k));
            }
            let soi = if parent.is_some() { a * (k.mu / MU).powf(0.4).max(0.04) } else { 0.0 };
            bodies.push(Body { id, name: name.into(), kind, parent, radius, soi, owner, intel: None, art: None, orbit_path });
        };
        add(0, "Helios", BodyKind::Star, None, 0.0, 0.0, 0.0, 0.0, 6.0, None, &mut kep);
        add(1, "Cinder", BodyKind::Planet, Some(0), 62.0, 0.12, 0.4, 2.4, 1.2, Some(2), &mut kep);
        add(2, "Tethys", BodyKind::Planet, Some(0), 108.0, 0.05, 1.9, 4.1, 2.0, Some(1), &mut kep);
        add(3, "Gaia Nova", BodyKind::Planet, Some(0), 150.0, 0.03, 0.0, 0.3, 2.2, None, &mut kep);
        add(4, "Kraken", BodyKind::Planet, Some(0), 232.0, 0.08, 2.6, 5.4, 4.0, Some(3), &mut kep);
        add(5, "Borealis", BodyKind::Planet, Some(0), 330.0, 0.14, 4.0, 1.2, 3.2, Some(4), &mut kep);
        add(6, "Pip", BodyKind::Moon, Some(2), 9.0, 0.0, 0.0, 1.0, 0.5, Some(1), &mut kep);
        add(7, "Hydra", BodyKind::Moon, Some(4), 16.0, 0.05, 0.0, 3.0, 0.8, None, &mut kep);
        add(8, "Scylla", BodyKind::Moon, Some(4), 26.0, 0.02, 1.0, 0.2, 0.6, Some(3), &mut kep);
        add(9, "Rook-12", BodyKind::Asteroid, Some(0), 190.0, 0.32, 3.4, 2.0, 0.3, Some(1), &mut kep);
        add(10, "Vesper", BodyKind::Asteroid, Some(0), 270.0, 0.22, 0.7, 4.4, 0.3, None, &mut kep);
        self.kep = kep;

        let now = self.start_t;
        for b in bodies.iter_mut() {
            b.intel = match b.id {
                2 | 6 => Some(Intel { seen_at: now - 60.0, power: 64.0, upgrades: vec!["Shipyard II".into(), "Solar".into(), "Refinery".into()], stations: vec!["Telescope".into(), "Fuel depot".into()] }),
                1 => Some(Intel { seen_at: now - 4.0 * 3600.0, power: 38.0, upgrades: vec!["Reactor".into()], stations: vec!["Defense platform".into()] }),
                4 => Some(Intel { seen_at: now - 9.0 * 3600.0, power: 81.0, upgrades: vec!["Shields".into(), "Shipyard III".into()], stations: vec![] }),
                3 | 9 => Some(Intel { seen_at: now - 1800.0, power: 0.0, upgrades: vec![], stations: vec![] }),
                _ => None,
            };
        }
        let players = vec![
            Player { id: 1, name: if self.user.is_empty() { "Jonas".into() } else { self.user.clone() }, color: 0, faction: "Vanguard Navy".into(), online: true },
            Player { id: 2, name: "Mara".into(), color: 1, faction: "Hive Brood".into(), online: false },
            Player { id: 3, name: "Ozzy".into(), color: 2, faction: "Prism Covenant".into(), online: true },
            Player { id: 4, name: "Bot (hard)".into(), color: 3, faction: "Junker Clans".into(), online: true },
        ];
        let ship = |id, name: &str, class, owner, count, at: Option<BodyId>| Ship {
            id,
            name: name.into(),
            class,
            owner,
            count,
            fuel: 340.0,
            fuel_max: 400.0,
            parked_at: at,
            trajectory: None,
            nodes: vec![],
            destination: None,
            retreat: false,
            seen_at: None,
            members: vec![],
            planned: None,
        };
        let mut ships = vec![
            ship(1, "Red Fang", ShipClass::Gunship, 1, 3, Some(2)),
            ship(2, "Lucky Star", ShipClass::Interceptor, 1, 5, Some(2)),
            ship(3, "Big Gulp", ShipClass::Tanker, 1, 1, Some(6)),
            ship(4, "Hammerhead", ShipClass::Bomber, 1, 2, Some(2)),
            ship(10, "Corsair pack", ShipClass::Interceptor, 2, 4, Some(1)),
            ship(11, "Choir", ShipClass::Gunship, 3, 2, Some(4)),
            ship(12, "Miner swarm", ShipClass::Bomber, 4, 6, Some(5)),
        ];
        // our multi-ship tokens are fleets; a few loose ships at Borealis to group
        for sh in ships.iter_mut().filter(|s| s.owner == 1 && s.count > 1) {
            sh.members = (0..sh.count).map(|k| FleetMember { id: 100 * sh.id + k, name: format!("{} {}", sh.class.label(), k + 1), class: sh.class, fuel: 300.0 + 20.0 * k as f32, fuel_max: 400.0 }).collect();
        }
        ships.push(ship(5, "Wasp", ShipClass::Interceptor, 1, 1, Some(2)));
        ships.push(ship(6, "Sparrow", ShipClass::Scout, 1, 1, Some(2)));
        // fog of war: an enemy dreadnought last seen 5 h ago (drawn as a fading ghost)
        ships.push(Ship { seen_at: Some(now - 5.0 * 3600.0), ..ship(13, "Dreadnought", ShipClass::Dreadnought, 2, 1, Some(2)) });
        self.view = Some(GameView {
            code: "K7QX".into(),
            me: 1,
            host: 1,
            now,
            ends_at: 7.0 * 86400.0,
            speed: 1.0,
            bodies,
            ships: vec![],
            players,
            resources: Resources { matter: 1840.0, energy: 620.0, fuel: 1275.0, matter_rate: 42.0, energy_rate: 18.0, fuel_rate: -6.0 },
            chat: vec![
                ChatMsg { from: 2, t: now - 5.0 * 3600.0, text: "nice moon you have there. shame if something crashed into it".into() },
                ChatMsg { from: 3, t: now - 3.0 * 3600.0, text: "truce on Kraken until day 5?".into() },
                ChatMsg { from: 1, t: now - 2.0 * 3600.0, text: "deal. Mara is building something big near Cinder".into() },
            ],
            lobby: None,
            home: None,
        });
        // Our Hammerhead is already en route to Gaia Nova; Mara's pack is heading for Tethys.
        let plan = self.plan_for(&ships[3], 3, now - 3.0 * 3600.0);
        let enemy = self.plan_for(&ships[4], 2, now - 2.0 * 3600.0);
        if let Some(p) = plan {
            ships[3].trajectory = Some(p.path);
            ships[3].destination = Some(3);
        }
        if let Some(p) = enemy {
            ships[4].trajectory = Some(p.path);
            ships[4].destination = Some(2);
        }
        self.view.as_mut().unwrap().ships = ships;
        self.t0_real = macroquad::time::get_time();
    }

    fn ship_state(&self, s: &Ship, t: f64) -> Option<[f64; 2]> {
        if let Some(tr) = &s.trajectory {
            if let Some(p) = tr.sample(t) {
                return Some(p);
            }
            if tr.end().is_some_and(|e| t > e) {
                return s.destination.map(|d| self.pos(d, t));
            }
        }
        s.parked_at.map(|b| self.pos(b, t))
    }

    fn plan_for(&self, s: &Ship, target: BodyId, after: f64) -> Option<TransferPlan> {
        let from = s.parked_at?;
        let r1 = self.sun_radius(from, after);
        let r2 = self.sun_radius(target, after);
        if (r1 - r2).abs() < K {
            // Same planet system (planet ↔ its moon): short hop.
            let depart_t = after + 300.0;
            let arrive_t = depart_t + 3.0 * 3600.0;
            let path = Trajectory {
                points: (0..=40)
                    .map(|i| {
                        let k = i as f64 / 40.0;
                        let t = depart_t + (arrive_t - depart_t) * k;
                        let (a, b) = (self.pos(from, t), self.pos(target, t));
                        (t, [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k])
                    })
                    .collect(),
                ..Default::default()
            };
            return Some(TransferPlan { ship: s.id, target, depart_t, arrive_t, dv_depart: 0.4, dv_mid: 0.0, dv_capture: 0.2, fuel_needed: 12.0, fuel_have: 0.0, fuel_problem: None, path, via: vec![], label: String::new(), options: vec![], token: String::new(), windows: vec![depart_t], intercept: None });
        }
        let at = (r1 + r2) / 2.0;
        let tof = PI * (at.powi(3) / MU).sqrt();
        // Scan departures for where the target will be ~180° from us at arrival.
        let mut windows = vec![];
        let mut best: Option<(f64, f64)> = None;
        let step = 900.0;
        let mut prev_err = None::<f64>;
        let mut t = after;
        while t < after + 4.0 * 86400.0 {
            let p1 = self.pos(from, t);
            let p2 = self.pos(target, t + tof);
            let a1 = p1[1].atan2(p1[0]);
            let a2 = p2[1].atan2(p2[0]);
            let err = (a2 - a1 - PI).rem_euclid(TAU) - PI;
            if let Some(pe) = prev_err {
                if pe.signum() != err.signum() && pe.abs() < 1.0 {
                    let tw = t - step * err.abs() / (err.abs() + pe.abs());
                    windows.push(tw);
                    if best.is_none() {
                        best = Some((tw, err));
                    }
                }
            }
            prev_err = Some(err);
            t += step;
        }
        let depart_t = best?.0;
        let p1 = self.pos(from, depart_t);
        let a1 = p1[1].atan2(p1[0]);
        let e = (r2 - r1) / (r2 + r1);
        let peri_at_start = r2 > r1;
        let n = 80;
        let points = (0..=n)
            .map(|i| {
                let k = i as f64 / n as f64;
                // True anomaly 0→π along a Hohmann half-ellipse (time spaced via mean anomaly).
                let m = PI * k;
                let mut ea = m;
                for _ in 0..8 {
                    ea -= (ea - e.abs() * ea.sin() - m) / (1.0 - e.abs() * ea.cos());
                }
                let nu = 2.0 * (((1.0 + e.abs()) / (1.0 - e.abs())).sqrt() * (ea / 2.0).tan()).atan();
                let r = at * (1.0 - e * e) / (1.0 + e.abs() * nu.cos());
                let (r, ang) = if peri_at_start { (r, a1 + nu) } else { (at * (1.0 - e * e) / (1.0 + e.abs() * (nu + PI).cos()), a1 + nu) };
                (depart_t + tof * k, [r * ang.cos(), r * ang.sin()])
            })
            .collect();
        let v1 = (MU / r1).sqrt();
        let v2 = (MU / r2).sqrt();
        let vp = (MU * (2.0 / r1 - 1.0 / at)).sqrt();
        let va = (MU * (2.0 / r2 - 1.0 / at)).sqrt();
        let dv1 = (vp - v1).abs();
        let dv2 = (v2 - va).abs();
        let need = ((dv1 + dv2) * 2.0) as f32 * s.count as f32;
        let have = if s.is_fleet() { s.members.iter().map(|m| m.fuel).sum() } else { s.fuel * s.count as f32 };
        Some(TransferPlan {
            ship: s.id,
            target,
            depart_t,
            arrive_t: depart_t + tof,
            dv_depart: dv1,
            dv_mid: 0.0,
            dv_capture: dv2,
            fuel_needed: need,
            fuel_have: have,
            fuel_problem: (need > have).then(|| format!("not enough fuel: needs {need:.0} t, has {have:.0} t")),
            path: Trajectory { points, ..Default::default() },
            via: vec![],
            label: String::new(),
            options: vec![],
            token: String::new(),
            windows,
            intercept: None,
        })
    }

    fn integrate(&self, mut p: [f64; 2], mut v: [f64; 2], t0: f64, t1: f64, out: &mut Vec<(f64, [f64; 2])>) -> ([f64; 2], [f64; 2]) {
        let steps = (((t1 - t0) / 300.0).ceil() as usize).clamp(1, 2000);
        let dt = (t1 - t0) / steps as f64;
        let acc = |p: [f64; 2]| {
            let r2 = p[0] * p[0] + p[1] * p[1];
            let r = r2.sqrt().max(3.0 * K);
            [-MU * p[0] / (r * r * r), -MU * p[1] / (r * r * r)]
        };
        for i in 0..steps {
            // Leapfrog (kick-drift-kick).
            let a = acc(p);
            v = [v[0] + a[0] * dt / 2.0, v[1] + a[1] * dt / 2.0];
            p = [p[0] + v[0] * dt, p[1] + v[1] * dt];
            let a = acc(p);
            v = [v[0] + a[0] * dt / 2.0, v[1] + a[1] * dt / 2.0];
            if i % 4 == 3 || i == steps - 1 {
                out.push((t0 + dt * (i + 1) as f64, p));
            }
        }
        (p, v)
    }
}

fn ship_cost(c: ShipClass) -> Cost {
    let (m, e, min) = match c {
        ShipClass::Scout => (40.0, 20.0, 20.0),
        ShipClass::Interceptor => (80.0, 30.0, 40.0),
        ShipClass::Gunship => (140.0, 60.0, 70.0),
        ShipClass::Bomber => (160.0, 70.0, 80.0),
        ShipClass::Dreadnought => (480.0, 220.0, 240.0),
        ShipClass::Tanker => (100.0, 30.0, 60.0),
        ShipClass::Constructor => (100.0, 50.0, 30.0),
        ShipClass::Decoy => (40.0, 30.0, 10.0),
        ShipClass::Torpedo => (120.0, 80.0, 20.0),
        ShipClass::Slug => (150.0, 300.0, 0.0),
    };
    Cost { matter: m, energy: e, seconds: min * 60.0 }
}

fn upgrade_cost(level: u8) -> Cost {
    let k = level as f64;
    Cost { matter: 120.0 * k, energy: 50.0 * k, seconds: 1800.0 * k }
}

impl Backend for DemoBackend {
    fn poll(&mut self) -> Vec<BackendEvent> {
        let now = self.now();
        if let Some(v) = self.view.as_mut() {
            v.now = now;
            // Fake the arrival of flights.
            for s in v.ships.iter_mut() {
                if let (Some(tr), Some(d)) = (&s.trajectory, s.destination) {
                    if tr.end().is_some_and(|e| now > e) {
                        s.parked_at = Some(d);
                        s.trajectory = None;
                        s.destination = None;
                        s.nodes.clear();
                    }
                }
            }
        }
        let (ups, rest): (Vec<_>, Vec<_>) = self.up_queue.drain(..).partition(|q| q.3 <= now);
        self.up_queue = rest;
        for (body, i, _, _) in ups {
            let lv = self.levels.entry(body).or_insert([1, 1, 0, 1, 1, 0, 0, 2]);
            lv[i] = (lv[i] + 1).min(3);
        }
        let (done, rest): (Vec<_>, Vec<_>) = self.queue.drain(..).partition(|q| q.2 <= now);
        self.queue = rest;
        if let Some(v) = self.view.as_mut() {
            for (body, label, _, class) in done {
                let id = v.ships.iter().map(|s| s.id).max().unwrap_or(0) + 1;
                v.ships.push(Ship { id, name: format!("New {label}"), class, owner: v.me, count: 1, fuel: 200.0, fuel_max: 400.0, parked_at: Some(body), trajectory: None, nodes: vec![], destination: None, retreat: false, seen_at: None, members: vec![], planned: None });
                self.events.push(BackendEvent::Notice(Notice { kind: NoticeKind::Good, title: format!("{label} ready"), body: "Fresh from the shipyard".into(), focus: Some(Focus::Ship(id)) }));
            }
        }
        std::mem::take(&mut self.events)
    }

    fn act(&mut self, action: UiAction) {
        // can't afford it: it waits for funds
        let short = |v: &Option<GameView>, c: &Cost| v.as_ref().is_some_and(|v| v.resources.matter < c.matter || v.resources.energy < c.energy);
        let wish = match &action {
            UiAction::Build { body, class } => Some((*body, class.label().to_string(), ship_cost(*class))),
            UiAction::Upgrade { body, upgrade } => UPGRADES.iter().position(|u| u == upgrade).map(|i| {
                let lv = self.levels.get(body).map(|l| l[i]).unwrap_or(1);
                (*body, upgrade.clone(), upgrade_cost(lv + 1))
            }),
            UiAction::Research { body, tech } => DEMO_TECHS.iter().position(|t| t.0 == tech).map(|i| (*body, tech.clone(), demo_tech_cost(i))),
            UiAction::BuildStation { body, kind } => STATION_KINDS.iter().position(|k| k == kind).map(|i| (*body, kind.clone(), demo_station_cost(i))),
            _ => None,
        };
        if let Some((body, label, cost)) = wish.filter(|w| short(&self.view, &w.2)) {
            let id = self.waiting.iter().map(|w| w.0 + 1).max().unwrap_or(0);
            self.waiting.push((id, body, label, cost));
            return;
        }
        match action {
            UiAction::ShipAbility { ship, key } if key == "cloak" => {
                match self.dark.iter().position(|&d| d == ship) {
                    Some(i) => {
                        self.dark.remove(i);
                    }
                    None => self.dark.push(ship),
                }
            }
            UiAction::ShipAbility { .. } | UiAction::StationAbility { .. } => {}
            UiAction::Unwait { id } => self.waiting.retain(|w| w.0 != id),
            UiAction::MoveWait { id, to } => {
                if let Some(i) = self.waiting.iter().position(|w| w.0 == id) {
                    let w = self.waiting.remove(i);
                    self.waiting.insert(to.min(self.waiting.len()), w);
                }
            }
            UiAction::DeleteGame { .. } => self.view = None,
            UiAction::Feedback { .. } => {}
            UiAction::SensorsSeen => {
                let now = self.now();
                if let Some(r) = self.report.as_mut() {
                    r.seen_until = now;
                }
            }
            UiAction::Login { user, .. } => {
                self.user = user.clone();
                self.events.push(BackendEvent::LoggedIn { user });
            }
            UiAction::Logout => self.events.push(BackendEvent::LoggedOut),
            UiAction::CreateGame { name, length_days, speed, bots } => {
                let mut players = vec![Player { id: 1, name: self.user.clone(), color: 0, faction: "VANGUARD NAVY".into(), online: true }];
                for (i, b) in bots.iter().enumerate() {
                    players.push(Player { id: 10 + i as u32, name: format!("Bot ({b})"), color: i + 1, faction: "JUNKER CLANS".into(), online: false });
                }
                let info = LobbyInfo { code: "K7QX".into(), name, players, max_players: 4, host: 1, length_days, speed, bots };
                self.enter_lobby(info);
            }
            UiAction::JoinGame { code } => {
                if code == "K7QX" || code == "ZB2M" {
                    self.start_game();
                    self.events.push(BackendEvent::GameStarted);
                } else {
                    let info = LobbyInfo {
                        code,
                        name: "Mara's Brawl".into(),
                        players: vec![
                            Player { id: 2, name: "Mara".into(), color: 1, faction: "HIVE BROOD".into(), online: true },
                            Player { id: 1, name: self.user.clone(), color: 0, faction: "VANGUARD NAVY".into(), online: true },
                        ],
                        max_players: 4,
                        host: 2,
                        length_days: 7,
                        speed: 1.0,
                        bots: vec![],
                    };
                    self.enter_lobby(info);
                }
            }
            UiAction::OpenGame { .. } | UiAction::StartGame => {
                // leaving the waiting period: back to the normal running demo clock
                if self.lobby.take().is_some() {
                    *self = DemoBackend { user: std::mem::take(&mut self.user), ..DemoBackend::new() };
                }
                self.start_game();
                self.events.push(BackendEvent::GameStarted);
            }
            UiAction::AddBot { difficulty } => {
                if let Some(l) = self.lobby.as_mut() {
                    let n = l.players.len();
                    l.players.push(Player { id: 20 + n as u32, name: format!("Bot ({difficulty})"), color: n, faction: "PRISM COVENANT".into(), online: false });
                    let l = l.clone();
                    self.sync_lobby(l);
                }
            }
            UiAction::SetFaction(f) => {
                if let Some(l) = self.lobby.as_mut() {
                    if let Some(p) = l.players.iter_mut().find(|p| p.id == 1) {
                        p.faction = f;
                    }
                    let l = l.clone();
                    self.sync_lobby(l);
                }
            }
            UiAction::RemoveBot { player } => {
                if let Some(l) = self.lobby.as_mut() {
                    l.players.retain(|p| p.id != player || p.online);
                    let l = l.clone();
                    self.sync_lobby(l);
                }
            }
            UiAction::LeaveLobby => {
                self.lobby = None;
                self.view = None;
                self.events.push(BackendEvent::LoggedIn { user: self.user.clone() });
            }
            UiAction::ScheduleTransfer(p) => {
                if let Some(v) = self.view.as_mut() {
                    if let Some(s) = v.ships.iter_mut().find(|s| s.id == p.ship) {
                        s.fuel -= p.fuel_needed;
                        s.trajectory = Some(p.path);
                        s.destination = Some(p.target);
                        s.nodes.clear();
                    }
                }
            }
            UiAction::SetNodes { ship, nodes } => {
                let tr = if nodes.is_empty() { None } else { self.preview_nodes(ship, &nodes) };
                if let Some(v) = self.view.as_mut() {
                    if let Some(s) = v.ships.iter_mut().find(|s| s.id == ship) {
                        if nodes.is_empty() {
                            s.nodes.clear();
                        } else {
                            s.trajectory = tr;
                            s.nodes = nodes;
                            s.destination = None;
                        }
                    }
                }
            }
            UiAction::CancelFlight { ship } => {
                let now = self.now();
                let here = self.view.as_ref().and_then(|v| v.ship(ship)).and_then(|s| {
                    // Only possible before departure in this demo.
                    s.trajectory.as_ref().and_then(|t| t.start()).filter(|&t0| t0 > now).map(|_| ())
                });
                if let Some(v) = self.view.as_mut() {
                    if let Some(s) = v.ships.iter_mut().find(|s| s.id == ship) {
                        if here.is_some() || s.parked_at.is_some() && s.trajectory.as_ref().is_some_and(|t| t.start().is_some_and(|t0| t0 > now)) {
                            s.trajectory = None;
                            s.destination = None;
                            s.nodes.clear();
                        } else {
                            self.events.push(BackendEvent::Notice(Notice {
                                kind: NoticeKind::Bad,
                                title: "CAN'T ABORT!".into(),
                                body: "Already in flight. Add a maneuver node instead.".into(),
                                focus: Some(Focus::Ship(ship)),
                            }));
                        }
                    }
                }
            }
            UiAction::Chat(text) => {
                let now = self.now();
                if let Some(v) = self.view.as_mut() {
                    v.chat.push(ChatMsg { from: v.me, t: now, text });
                }
            }
            UiAction::BackToMenu => self.view = None,
            UiAction::Reconnect => {}
            UiAction::MoveJob { body, from, to } => {
                let ids: Vec<usize> = (0..self.queue.len()).filter(|&i| self.queue[i].0 == body).collect();
                if let (Some(&a), Some(&b)) = (ids.get(from), ids.get(to)) {
                    let start = ids.first().map(|&i| self.queue[i].2 - ship_cost(self.queue[i].3).seconds).unwrap_or(self.now());
                    self.queue.swap(a, b);
                    // recompute finish times in the new order
                    let mut t = start;
                    for &i in &ids {
                        t += ship_cost(self.queue[i].3).seconds;
                        self.queue[i].2 = t;
                    }
                }
            }
            UiAction::CancelJob { body, index } => {
                let ids: Vec<usize> = (0..self.queue.len()).filter(|&i| self.queue[i].0 == body).collect();
                // indices past the ships are the upgrade lane
                if index >= ids.len() {
                    let ups: Vec<usize> = (0..self.up_queue.len()).filter(|&i| self.up_queue[i].0 == body).collect();
                    if let Some(&qi) = ups.get(index - ids.len()) {
                        let (_, _, start, done) = self.up_queue.remove(qi);
                        for q in self.up_queue.iter_mut().filter(|q| q.0 == body && q.3 > done) {
                            q.2 -= done - start;
                            q.3 -= done - start;
                        }
                    }
                }
                if let Some(&qi) = ids.get(index) {
                    let (_, _, done, class) = self.queue.remove(qi);
                    let c = ship_cost(class);
                    if let Some(v) = self.view.as_mut() {
                        v.resources.matter += c.matter;
                        v.resources.energy += c.energy;
                    }
                    // later items on this yard move up
                    for q in self.queue.iter_mut().filter(|q| q.0 == body && q.2 > done) {
                        q.2 -= c.seconds;
                    }
                }
            }
            UiAction::AimAsteroid { body, target } => {
                let name = |id| self.view.as_ref().and_then(|v| v.body(id)).map(|b| b.name.clone()).unwrap_or_default();
                let (a, t) = (name(body), name(target));
                self.events.push(BackendEvent::Notice(Notice { kind: NoticeKind::Info, title: "Drive fitted".into(), body: format!("{a} burning toward {t}"), focus: Some(Focus::Body(body)) }));
            }
            UiAction::Build { body, class } => {
                let now = self.now();
                let cost = ship_cost(class);
                if let Some(v) = self.view.as_mut() {
                    v.resources.matter -= cost.matter;
                    v.resources.energy -= cost.energy;
                }
                let done = self.queue.iter().filter(|q| q.0 == body).map(|q| q.2).fold(now, f64::max) + cost.seconds;
                self.queue.push((body, class.label().to_string(), done, class));
            }
            UiAction::Upgrade { body, upgrade } => {
                if let Some(i) = UPGRADES.iter().position(|u| *u == upgrade) {
                    let now = self.now();
                    let lv = self.levels.entry(body).or_insert([1, 1, 0, 1, 1, 0, 0, 2])[i];
                    let pending = self.up_queue.iter().filter(|q| q.0 == body && q.1 == i).count() as u8;
                    let c = upgrade_cost(lv + pending + 1);
                    if lv + pending < 3 {
                        if let Some(v) = self.view.as_mut() {
                            v.resources.matter -= c.matter;
                            v.resources.energy -= c.energy;
                        }
                        let start = self.up_queue.iter().filter(|q| q.0 == body).map(|q| q.3).fold(now, f64::max);
                        self.up_queue.push((body, i, start, start + c.seconds));
                    }
                }
            }
            UiAction::SetOrder { ship, retreat } => {
                if let Some(s) = self.view.as_mut().and_then(|v| v.ships.iter_mut().find(|s| s.id == ship)) {
                    s.retreat = retreat;
                }
            }
            UiAction::Research { body, tech } => {
                let now = self.now();
                if let Some(i) = DEMO_TECHS.iter().position(|t| t.0 == tech) {
                    let c = demo_tech_cost(i);
                    if let Some(v) = self.view.as_mut() {
                        v.resources.matter -= c.matter;
                        v.resources.energy -= c.energy;
                    }
                    self.research_run.push((i, body, now + c.seconds / 1.5));
                }
            }
            UiAction::BuildStation { body, kind } => {
                let now = self.now();
                let workers = self.stations(body).map(|v| v.workers).unwrap_or(1).max(1) as f64;
                if let Some(i) = STATION_KINDS.iter().position(|k| *k == kind) {
                    let c = demo_station_cost(i);
                    if let Some(v) = self.view.as_mut() {
                        v.resources.matter -= c.matter;
                        v.resources.energy -= c.energy;
                        self.stations.push((body, kind, v.me, now + c.seconds / workers));
                    }
                }
            }
            UiAction::ExtendGame { days } => {
                if let Some(v) = self.view.as_mut() {
                    v.ends_at += days as f64 * 86400.0;
                }
            }
            UiAction::FormFleet { ships, name } => demo_regroup(self.view.as_mut(), &ships, &[], Some(name)),
            UiAction::JoinFleet { fleet, ships } => {
                let name = self.view.as_ref().and_then(|v| v.ship(fleet)).map(|s| s.name.clone());
                let all: Vec<ShipId> = std::iter::once(fleet).chain(ships).collect();
                demo_regroup(self.view.as_mut(), &all, &[], name);
            }
            UiAction::MergeFleets { into, from } => {
                let name = self.view.as_ref().and_then(|v| v.ship(into)).map(|s| s.name.clone());
                demo_regroup(self.view.as_mut(), &[into, from], &[], name);
            }
            UiAction::LeaveFleet { ships, split } => demo_regroup(self.view.as_mut(), &[], &ships, split),
            UiAction::RenameFleet { fleet, name } => {
                if let Some(s) = self.view.as_mut().and_then(|v| v.ships.iter_mut().find(|s| s.id == fleet)) {
                    s.name = name;
                }
            }
            UiAction::DisbandFleet { fleet } => {
                if let Some(v) = self.view.as_mut() {
                    if let Some(i) = v.ships.iter().position(|s| s.id == fleet) {
                        let f = v.ships.remove(i);
                        v.ships.extend(demo_split(&f));
                    }
                }
            }
            UiAction::SetSpeed { speed } => {
                if let Some(v) = self.view.as_mut() {
                    v.speed = speed;
                }
            }
        }
    }

    fn body_info(&self, id: BodyId) -> Option<BodyInfo> {
        let v = self.view.as_ref()?;
        if v.body(id)?.owner != Some(v.me) {
            return None;
        }
        let lv = self.levels.get(&id).copied().unwrap_or([1, 1, 0, 1, 1, 0, 0, 2]);
        let upgrades = UPGRADES
            .iter()
            .zip(lv)
            .map(|(n, l)| UpgradeInfo { name: n.to_string(), level: l, max: 3, next: (l < 3).then(|| upgrade_cost(l + 1)) })
            .collect();
        let yard = lv[4];
        let buildable = ShipClass::ALL.iter().filter(|c| yard >= 1 && !matches!(**c, ShipClass::Decoy | ShipClass::Torpedo | ShipClass::Slug) && (**c != ShipClass::Dreadnought || yard >= 3)).map(|&c| (c, ship_cost(c))).collect();
        let mut power: Vec<(PlayerId, f64)> = vec![];
        for s in v.ships.iter().filter(|s| s.parked_at == Some(id) && s.trajectory.is_none()) {
            match power.iter_mut().find(|p| p.0 == s.owner) {
                Some(p) => p.1 += 20.0 * s.count as f64,
                None => power.push((s.owner, 20.0 * s.count as f64)),
            }
        }
        Some(BodyInfo {
            upgrades,
            buildable,
            queue: {
                let mut prev: Option<f64> = None;
                let ships = self.queue.iter().filter(|q| q.0 == id).map(|q| {
                    let start = prev.unwrap_or((q.2 - ship_cost(q.3).seconds).min(self.now()));
                    prev = Some(q.2);
                    QueueItem { label: q.1.clone(), ship: Some(q.3), start_t: start, done_t: q.2 }
                });
                let ups = self.up_queue.iter().filter(|q| q.0 == id).map(|q| QueueItem { label: UPGRADES[q.1].to_string(), ship: None, start_t: q.2, done_t: q.3 });
                ships.collect::<Vec<_>>().into_iter().chain(ups).collect()
            },
            waiting: self
                .waiting
                .iter()
                .enumerate()
                .filter(|(_, w)| w.1 == id)
                .map(|(index, w)| WaitingItem {
                    id: w.0,
                    label: w.2.clone(),
                    cost: w.3.clone(),
                    start_t: Some(self.now() + 3600.0 * (index + 1) as f64),
                    blocked: None,
                    index,
                    of: self.waiting.len(),
                })
                .collect(),
            fuel: 420.0,
            power,
            matter_rate: 60.0 * (1.0 + lv[0] as f64),
            energy_rate: 20.0 + 60.0 * (lv[1] + lv[2]) as f64,
            fuel_rate: 120.0 * lv[3] as f64,
        })
    }

    fn research(&self) -> Option<ResearchView> {
        let v = self.view.as_ref()?;
        let now = self.now();
        let labs: Vec<LabInfo> = v
            .bodies
            .iter()
            .filter(|b| b.owner == Some(v.me))
            .filter_map(|b| {
                let level = self.levels.get(&b.id).copied().unwrap_or([1, 1, 0, 1, 1, 0, 0, 2])[7];
                let busy = self.research_run.iter().filter(|r| r.1 == b.id && r.2 > now).map(|r| (DEMO_TECHS[r.0].1.to_string(), r.2)).collect();
                (level > 0).then(|| LabInfo { body: b.id, name: b.name.clone(), level, busy })
            })
            .collect();
        let done = |i: usize| self.techs_done.contains(&i) || self.research_run.iter().any(|r| r.0 == i && r.2 <= now);
        let techs = DEMO_TECHS
            .iter()
            .enumerate()
            .map(|(i, (id, name, desc))| {
                let (advanced, tier, branch, min_lab, has_pre) = demo_tech_meta(i);
                let base_done = (0..20).filter(|&j| done(j)).count();
                let state = if done(i) {
                    TechState::Done
                } else if let Some(r) = self.research_run.iter().find(|r| r.0 == i) {
                    TechState::Running { lab: r.1, done_t: r.2 }
                } else if advanced && base_done < DEMO_ADV_UNLOCK {
                    TechState::Locked(format!("needs {DEMO_ADV_UNLOCK} base techs ({base_done} so far)"))
                } else if has_pre && !done(i - 1) {
                    TechState::Locked(format!("needs {}", DEMO_TECHS[i - 1].1))
                } else {
                    // a lab with a free slot, else the best busy one (it queues there)
                    match labs.iter().filter(|l| l.has_free_slot() && l.level >= min_lab).max_by_key(|l| l.level).or_else(|| labs.iter().filter(|l| l.level >= min_lab).max_by_key(|l| l.level)) {
                        Some(l) => TechState::Ready { lab: l.body },
                        None => TechState::Locked(format!("needs lab level {min_lab}")),
                    }
                };
                TechInfo { id: id.to_string(), name: name.to_string(), branch: branch.to_string(), tier, desc: desc.to_string(), cost: demo_tech_cost(i), min_lab, advanced, state }
            })
            .collect();
        Some(ResearchView { techs, labs })
    }

    fn sensors(&self) -> Option<&SensorsReport> {
        self.report.as_ref()
    }

    fn ship_abilities(&self, ship: ShipId) -> Vec<Ability> {
        let Some(v) = self.view.as_ref() else { return vec![] };
        let Some(sh) = v.ships.iter().find(|s| s.id == ship && s.owner == v.me) else { return vec![] };
        if !self.techs_done.contains(&30) {
            return vec![];
        }
        let dark = self.dark.contains(&ship);
        let blocker = (sh.class == ShipClass::Dreadnought).then(|| "a Dreadnought runs too hot to hide".to_string());
        vec![Ability {
            key: "cloak".into(),
            label: if dark { "DECLOAK".into() } else { "GO DARK".into() },
            on: dark,
            blocker,
            note: if dark { "dark for 11h 40m · first strike on decloak".into() } else { "30 energy · 12 h heat sink".into() },
        }]
    }

    fn stations(&self, id: BodyId) -> Option<StationsView> {
        let v = self.view.as_ref()?;
        let now = self.now();
        let constructors: usize = v.ships.iter().filter(|s| s.owner == v.me && s.class == ShipClass::Constructor && s.parked_at == Some(id) && s.trajectory.is_none()).map(|s| s.count as usize).sum();
        let workers = constructors.min(6);
        let present = workers > 0;
        let here = self
            .stations
            .iter()
            .filter(|s| s.0 == id)
            .map(|s| {
                let total = demo_station_cost(STATION_KINDS.iter().position(|k| *k == s.1).unwrap_or(0)).seconds / workers.max(1) as f64;
                let progress = (1.0 - (s.3 - now) / total).clamp(0.0, 1.0) as f32;
                StationInfo { kind: s.1.clone(), owner: s.2, building_until: (s.3 > now).then_some(s.3), progress, hp: 1.0 }
            })
            .collect();
        let scanning = self.techs_done.contains(&17);
        let platforms = self.techs_done.contains(&15);
        // the demo has no advanced-tier stations
        let options = STATION_KINDS
            .iter()
            .take(DEMO_STATIONS.len())
            .enumerate()
            .map(|(i, k)| {
                let blocker = if !present {
                    Some(format!("needs a construction ship in orbit of {}", v.body(id).map(|b| b.name.as_str()).unwrap_or("here")))
                } else if (*k == "Scanner" || *k == "Relay") && !scanning {
                    Some("needs Scanning stations".to_string())
                } else if *k == "Defense" && !platforms {
                    Some("needs Defense platforms".to_string())
                } else if self.stations.iter().any(|s| s.0 == id && s.1 == *k && s.2 == v.me) {
                    Some("already built here".to_string())
                } else {
                    None
                };
                StationOption { kind: k.to_string(), name: DEMO_STATIONS[i].0.to_string(), desc: DEMO_STATIONS[i].1.to_string(), cost: demo_station_cost(i), queueable: blocker.as_deref() != Some("already built here"), blocker }
            })
            .collect();
        Some(StationsView { here, options, present, constructors, workers, actions: vec![] })
    }

    fn plan_impact_async(&self, asteroid: BodyId, target: BodyId) -> crate::ImpactPoll {
        let v = match self.view.as_ref() {
            Some(v) => v,
            None => return crate::ImpactPoll::Done(None),
        };
        if v.body(asteroid).map(|b| b.owner) != Some(Some(v.me)) {
            return crate::ImpactPoll::Done(None);
        }
        // Pretend the rock is a parked ship and reuse the Hohmann planner for the path.
        let rock = Ship {
            seen_at: None,
            members: vec![],
            id: u32::MAX,
            name: String::new(),
            class: ShipClass::Tanker,
            owner: v.me,
            count: 1,
            fuel: 0.0,
            fuel_max: 0.0,
            parked_at: Some(asteroid),
            trajectory: None,
            nodes: vec![],
            destination: None,
            retreat: false,
            planned: None,
        };
        let res = self.plan_for(&rock, target, v.now).map(|p| {
            let speed = p.dv_capture + 9.0;
            let mass = 4.0e15;
            ImpactPreview {
                asteroid,
                target,
                burn_t: p.depart_t,
                burn_s: 10.0 * 3600.0,
                dv: p.dv_depart,
                fuel: p.dv_depart * 40.0,
                fuel_available: 600.0,
                drive_cost: Cost { matter: 200.0, energy: 150.0, seconds: 7200.0 },
                impact_t: Some(p.arrive_t),
                impact_speed: speed,
                impact_energy: 0.5 * mass * (speed * 1000.0).powi(2),
                severity: 1.6,
                path: p.path,
            }
        });
        crate::ImpactPoll::Done(res)
    }

    fn orbit_status(&self, body: BodyId) -> Option<String> {
        match body {
            1 => Some("Standoff: no armed ships here, so nobody can shoot or take it".into()),
            2 => Some("Battle: Jonas vs Mara".into()),
            _ => None,
        }
    }

    fn results(&self) -> Option<GameResults> {
        self.results.clone()
    }

    fn construction_done(&self, body: BodyId) -> Option<f64> {
        let v = self.view.as_ref()?;
        let now = self.now();
        self.stations.iter().filter(|s| s.0 == body && s.2 == v.me && s.3 > now).map(|s| s.3).reduce(f64::max)
    }

    fn default_server(&self) -> String {
        "demo (offline)".into()
    }

    fn my_games(&self) -> Vec<GameSummary> {
        self.games.clone()
    }

    fn view(&self) -> Option<&GameView> {
        self.view.as_ref()
    }

    fn body_pos(&self, id: BodyId, t: f64) -> [f64; 2] {
        self.pos(id, t)
    }

    fn ship_pos(&self, id: ShipId, t: f64) -> Option<[f64; 2]> {
        let s = self.view.as_ref()?.ship(id)?;
        self.ship_state(s, t)
    }

    fn plan_transfer(&self, ship: ShipId, target: BodyId, after: f64) -> Option<TransferPlan> {
        let s = self.view.as_ref()?.ship(ship)?;
        let mut cheap = self.plan_for(s, target, after)?;
        // fake course options: a slingshot marker on the cheapest path and two pricier, quicker ones
        if let Some(&(t, pos)) = cheap.path.points.get(cheap.path.points.len() / 2) {
            let body = self.view.as_ref()?.bodies.iter().find(|b| b.kind == BodyKind::Planet && b.id != target)?.id;
            cheap.path.flybys.push(FlybyMark { body, t, pos, rp: 2000.0, gain: 3.4 });
            cheap.via = vec![body];
        }
        let mut opts = vec![];
        for (label, k) in [("CHEAPEST", 1.0), ("BALANCED", 1.45), ("FASTEST", 2.6)] {
            let mut o = cheap.clone();
            o.label = label.into();
            o.token = label.into();
            if k > 1.0 {
                o.via.clear();
                o.path.flybys.clear();
                o.dv_depart *= k;
                o.dv_capture *= k;
                o.arrive_t = o.depart_t + (o.arrive_t - o.depart_t) / k;
            }
            opts.push(o);
        }
        for o in opts.iter_mut() {
            o.options = vec![];
        }
        let list = opts.clone();
        for o in opts.iter_mut() {
            o.options = list.clone();
        }
        opts.into_iter().next()
    }

    fn preview_nodes(&self, ship: ShipId, nodes: &[ManeuverNode]) -> Option<Trajectory> {
        let v = self.view.as_ref()?;
        let s = v.ship(ship)?;
        let first = nodes.first()?;
        let mut out = vec![];
        // Coast on the committed path up to the first node.
        let mut t = v.now;
        while t < first.t {
            if let Some(p) = self.ship_state(s, t) {
                out.push((t, p));
            }
            t += 600.0;
        }
        let p = self.ship_state(s, first.t)?;
        let q = self.ship_state(s, first.t - 60.0)?;
        let mut pos = p;
        let mut vel = [(p[0] - q[0]) / 60.0, (p[1] - q[1]) / 60.0];
        let mut t = first.t;
        out.push((t, pos));
        for (i, n) in nodes.iter().enumerate() {
            if n.t > t {
                let (p2, v2) = self.integrate(pos, vel, t, n.t, &mut out);
                pos = p2;
                vel = v2;
                t = n.t;
            }
            let sp = vel[0].hypot(vel[1]).max(1e-12);
            let pro = [vel[0] / sp, vel[1] / sp];
            let mut rad = [-pro[1], pro[0]];
            if rad[0] * pos[0] + rad[1] * pos[1] < 0.0 {
                rad = [-rad[0], -rad[1]];
            }
            // km/s → Gm/s.
            vel[0] += pro[0] * n.prograde + rad[0] * n.radial;
            vel[1] += pro[1] * n.prograde + rad[1] * n.radial;
            let end = nodes.get(i + 1).map(|n| n.t).unwrap_or(t + 86400.0);
            if i + 1 == nodes.len() {
                let (p2, v2) = self.integrate(pos, vel, t, end, &mut out);
                pos = p2;
                vel = v2;
                t = end;
            }
        }
        Some(Trajectory { points: out, ..Default::default() })
    }
}

/// Mirror of sim's tech tree for offline demo (id, name, description), branch-major.
/// The last 15 are the advanced tier (3 per branch, opens after `DEMO_ADV_UNLOCK` base techs).
const DEMO_TECHS: [(&str, &str, &str); 35] = [
    ("HardenedSlugs", "Hardened slugs", "Kinetic weapons deal 15% more damage."),
    ("FocusedLasers", "Focused lasers", "Energy weapons deal 15% more damage."),
    ("FireControl", "Fire control", "Kinetic slugs lead fast targets better: +25% accuracy."),
    ("SpinalLance", "Spinal lance", "All weapons deal 20% more damage."),
    ("EfficientNozzles", "Efficient nozzles", "Engine exhaust velocity +10%: more Δv per tonne of fuel."),
    ("ExpandedTanks", "Expanded tanks", "Ship fuel tanks hold 25% more."),
    ("IonDrives", "Ion drives", "Exhaust velocity +20% more."),
    ("AsteroidDrives", "Asteroid drives", "Attach drives to asteroids to push them onto new orbits."),
    ("DeepCoreMining", "Deep-core mining", "Matter production +25%."),
    ("FusionPlants", "Fusion plants", "Energy production +25%."),
    ("ModularYards", "Modular yards", "Ships build 30% faster."),
    ("AutoRefineries", "Auto refineries", "Fuel refineries produce 50% more."),
    ("ArmorPlating", "Armor plating", "Ships have 20% more hull."),
    ("PlanetaryShields", "Planetary shields", "Planetary defenses have 50% more hit points."),
    ("PointDefense", "Point defense", "Planetary defenses fire 50% harder."),
    ("DefensePlatforms", "Defense platforms", "Unlocks armed defense platform stations."),
    ("BetterOptics", "Better optics", "Telescope and sensor range +30%."),
    ("ScanningStations", "Scanning stations", "Unlocks scanning stations and sensor relays."),
    ("SignatureDamping", "Signature damping", "Your ships are 30% harder to see."),
    ("DeepSpaceArray", "Deep-space array", "Sensor range +40% more."),
    ("LaserLaunch", "Laser launch array", "Station that beams departing ships: the first 3 km/s of a departure burn costs no fuel."),
    ("AntimatterCatalysis", "Antimatter catalysis", "Engine exhaust velocity +50%."),
    ("Stargates", "Stargates", "Paired gate stations: ships parked at one gate jump to the other in 10 minutes."),
    ("Torpedoes", "Hunter-killer torpedoes", "Uncrewed torpedoes that chase one enemy ship across the system and detonate."),
    ("MassDrivers", "Mass drivers", "Station that fires guided slugs on transfer orbits at another body's orbit."),
    ("FusionWarheads", "Fusion warheads", "Torpedo and slug damage +60%; slugs also hit stations."),
    ("Minefields", "Orbital minefields", "Constructors seed mines that hit enemy ships capturing into that orbit."),
    ("InterdictionGrid", "Interdiction grid", "Station that shoots down incoming torpedoes and slugs and fragments asteroids."),
    ("PhasedLasers", "Phased laser battery", "Planet-based laser battery that hits ships out to 40,000 km."),
    ("DecoyDrones", "Decoy drones", "Cheap drones that look like a warship to enemy sensors."),
    ("HeatSinkCloaks", "Heat-sink cloaking", "Ships can go dark for 12 h by storing their heat, then must vent."),
    ("ThermalBaffles", "Thermal baffles", "Cloaks last twice as long; venting is half as bright."),
    ("GravimetricArray", "Gravimetric array", "Station that detects ships by mass, cloaked or not."),
    ("LidarSweep", "Lidar sweep", "Scanning stations can ping: everything in 3× range shows for 1 h, the pinger too."),
    ("DeepFieldInterferometry", "Deep-field interferometry", "Sensor range +50%, and passive sensors see through decoys."),
];
const DEMO_ADV_UNLOCK: usize = 15;

/// (advanced, tier, branch, min lab, has a prereq) of `DEMO_TECHS[i]`, like sim's tech.rs.
fn demo_tech_meta(i: usize) -> (bool, u8, &'static str, u8, bool) {
    match i.checked_sub(20) {
        Some(a) => (true, (a % 3) as u8 + 1, ADV_BRANCHES[a / 3], if a % 3 == 0 { 2 } else { 3 }, a % 3 != 0),
        None => (false, (i % 4) as u8 + 1, TECH_BRANCHES[i / 4], [1, 1, 2, 3][i % 4], i % 4 != 0),
    }
}

/// Fake fleet bookkeeping: `tokens` (whole tokens) merge into one fleet named `name`;
/// `leavers` (member ids) come out of their fleets, as a new fleet if `name` is set.
fn demo_regroup(view: Option<&mut GameView>, tokens: &[ShipId], leavers: &[ShipId], name: Option<String>) {
    let Some(v) = view else { return };
    let mut loose: Vec<Ship> = vec![];
    for &t in tokens {
        if let Some(i) = v.ships.iter().position(|s| s.id == t) {
            loose.extend(demo_split(&v.ships.remove(i)));
        }
    }
    for sh in v.ships.iter_mut().filter(|s| s.is_fleet()) {
        let (out, keep): (Vec<FleetMember>, Vec<FleetMember>) = sh.members.drain(..).partition(|m| leavers.contains(&m.id));
        sh.members = keep;
        sh.count = sh.members.len() as u32;
        let base = sh.clone();
        loose.extend(out.into_iter().map(|m| Ship { id: m.id, name: m.name, class: m.class, count: 1, fuel: m.fuel, fuel_max: m.fuel_max, members: vec![], ..base.clone() }));
    }
    // a one-ship fleet dissolves
    let mut ones = vec![];
    v.ships.retain(|s| {
        if s.is_fleet() && s.members.len() < 2 {
            ones.push(s.clone());
            false
        } else {
            true
        }
    });
    for o in ones {
        v.ships.extend(demo_split(&o));
    }
    match name {
        Some(n) if loose.len() >= 2 => {
            let id = 0x4000_0000 + v.ships.iter().chain(&loose).map(|s| s.id & 0xffff).max().unwrap_or(0) + 1;
            let members: Vec<FleetMember> =
                loose.iter().map(|s| FleetMember { id: s.id, name: s.name.clone(), class: s.class, fuel: s.fuel, fuel_max: s.fuel_max }).collect();
            let weakest = members.iter().min_by(|a, b| (a.fuel / a.fuel_max).total_cmp(&(b.fuel / b.fuel_max))).unwrap();
            let name = if n.trim().is_empty() { format!("Fleet {}", id & 0xffff) } else { n.trim().to_string() };
            v.ships.push(Ship { id, name, count: members.len() as u32, fuel: weakest.fuel, fuel_max: weakest.fuel_max, members, ..loose[0].clone() });
        }
        _ => v.ships.extend(loose),
    }
}

/// A token's ships as single ships (just itself when it isn't a fleet).
fn demo_split(f: &Ship) -> Vec<Ship> {
    if !f.is_fleet() {
        return vec![f.clone()];
    }
    f.members.iter().map(|m| Ship { id: m.id, name: m.name.clone(), class: m.class, count: 1, fuel: m.fuel, fuel_max: m.fuel_max, members: vec![], ..f.clone() }).collect()
}

fn demo_tech_cost(i: usize) -> Cost {
    let (adv, tier, ..) = demo_tech_meta(i);
    let k = tier as f64;
    if adv {
        return Cost { matter: 500.0 + 500.0 * k, energy: 600.0 + 600.0 * k, seconds: [20.0, 32.0, 48.0][tier as usize - 1] * 3600.0 };
    }
    Cost { matter: 150.0 * k, energy: 200.0 * k, seconds: [2.0, 4.0, 8.0, 12.0][tier as usize - 1] * 3600.0 }
}

const DEMO_STATIONS: [(&str, &str); 6] = [
    ("Shipyard", "Builds ships here, even if you don't hold the body."),
    ("Fuel depot", "Refines and stores fuel here and refuels your ships, even on asteroids."),
    ("Telescope", "Long-range passive sensor."),
    ("Scanning station", "Sees every ship in range and reads enemy flight plans."),
    ("Defense platform", "Armed platform. Fights for the orbit and counts toward control."),
    ("Sensor relay", "Your other sensors within its range reach 50% farther."),
];

fn demo_station_cost(i: usize) -> Cost {
    let (m, e, h) = [(300.0, 150.0, 4.0), (150.0, 80.0, 2.0), (120.0, 120.0, 2.0), (200.0, 200.0, 3.0), (250.0, 150.0, 3.0), (150.0, 150.0, 2.0)][i];
    Cost { matter: m, energy: e, seconds: h * 3600.0 }
}

/// A sample sensors report (back after 14 h away) for the demo scenes `report*`.
pub fn demo_report(now: f64) -> SensorsReport {
    let h = 3600.0;
    let it = |dt: f64, when, section, title: &str, text: &str, focus, tone| ReportItem { t: now - dt * h, when, section, title: title.into(), text: text.into(), focus, fallback: None, tone };
    use ReportSection::*;
    use When::*;
    SensorsReport {
        seen_until: now - 14.0 * h,
        items: vec![
            it(-3.4, Eta, Threats, "Incoming", "Mara's 3 Interceptors heading for Tethys", Some(Focus::Ship(10)), -1),
            it(-9.0, Eta, Threats, "Asteroid strike", "Vesta-7 is on course to hit Gaia Nova", Some(Focus::Body(9)), -1),
            it(0.0, Live, Battles, "Fighting", "Fighting at Kraken (Ozzy, Bot (hard))", Some(Focus::Body(4)), 0),
            it(-1.2, Eta, Transfers, "Under way", "Big Gulp heading for Kraken", Some(Focus::Ship(3)), 1),
            it(-5.5, Eta, Transfers, "Departs", "Red Fang leaves Tethys for Gaia Nova", Some(Focus::Ship(1)), 1),
            it(0.0, Live, Ships, "Mara", "3 Interceptors in flight near Gaia Nova, heading for Tethys", Some(Focus::Ship(10)), -1),
            it(0.0, Live, Ships, "Ozzy", "Gunship, Scout in orbit of Kraken", Some(Focus::Body(4)), -1),
            it(6.0, Seen, Ghosts, "Mara", "Dreadnought, last seen near Tethys", Some(Focus::Ship(13)), -1),
            it(0.0, Live, Bodies, "Kraken", "Ozzy's · Mining 2, Defense 1 · 2 ships in orbit", Some(Focus::Body(4)), -1),
            it(3.0, Seen, Bodies, "Gaia Nova", "Unclaimed · nothing seen in orbit", Some(Focus::Body(3)), 0),
            it(0.0, Never, Bodies, "Never scanned", "Ymir, Ixvo, Rhae", None, 0),
            it(13.2, At, Log, "Ship built", "Interceptor 3 at Tethys", Some(Focus::Body(2)), 1),
            it(11.5, At, Log, "Contact", "Mara: 3 Interceptors, Scout near Gaia Nova", Some(Focus::Ship(10)), -1),
            it(10.9, At, Log, "Engine burn", "Mara's 3 Interceptors burned near Gaia Nova, heading for Tethys", Some(Focus::Ship(10)), -1),
            it(9.0, At, Log, "Battle sighted", "Fighting at Kraken", Some(Focus::Body(4)), 0),
            it(7.4, At, Log, "Territory", "Kraken changed hands from Ozzy to Bot (hard)", Some(Focus::Body(4)), 0),
            it(6.0, At, Log, "Contact lost", "Lost track of Mara's Dreadnought near Tethys", Some(Focus::Ship(13)), -1),
            it(4.2, At, Log, "Under attack", "Tethys: enemy ships in orbit", Some(Focus::Body(2)), -1),
            it(3.9, At, Log, "Territory gained", "Gaia Nova is yours", Some(Focus::Body(3)), 1),
            it(2.0, At, Log, "Research done", "Better optics", None, 1),
            it(0.5, At, Log, "Arrived", "Big Gulp is in orbit of Gaia Nova", Some(Focus::Ship(3)), 1),
            it(16.0, At, Log, "Ship built", "Scout 2 at Tethys", Some(Focus::Body(2)), 1),
        ],
    }
}
