//! `ui::Backend` over the network connection and a local copy of the authoritative `sim::Game`.
//! Between server snapshots the local copy is advanced with the same deterministic sim, so the
//! map keeps moving smoothly; every snapshot replaces it.
use crate::forecast::{self, Forecaster, PLANNED_BASE};
use crate::net::{Net, NetEvent};
use proto::{unix_now, ClientMsg, GameSummary as WireSummary, ServerMsg};
use sim::game::{self as g, Command, Game, ShipState, Status};
use sim::station::StationKind;
use sim::tech::{Branch, Tech};
use sim::system::{BodyKind as SimKind, SystemSize};
use sim::traj::{Burn, Trajectory as SimTraj};
use sim::{BodyId as SimBody, V2};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use ui::model::{self as m, BodyId, ShipId};
use ui::{Backend, BackendEvent, UiAction};

pub struct ClientBackend {
    server: String,
    creds: Option<(String, String)>,
    net: Option<Net>,
    logged_in: bool,
    events: Vec<BackendEvent>,
    games: Vec<WireSummary>,
    game: Option<Game>,
    me: g::PlayerId,
    /// server unix time of the snapshot and the game time then
    anchor: (f64, f64),
    view: Option<m::GameView>,
    /// cached absolute paths of flying ships
    paths: HashMap<u32, m::Trajectory>,
    last_event_seq: u64,
    was_running: bool,
    plan_cache: RefCell<HashMap<(ShipId, BodyId, i64), Option<m::TransferPlan>>>,
    /// background planner jobs keyed like `plan_cache`
    plan_jobs: RefCell<HashMap<(ShipId, BodyId, i64), std::sync::mpsc::Receiver<Option<m::TransferPlan>>>>,
    /// asteroid impact previews keyed by (asteroid, target), and their background jobs
    impact_cache: RefCell<HashMap<(BodyId, BodyId), Option<m::ImpactPreview>>>,
    impact_jobs: RefCell<HashMap<(BodyId, BodyId), std::sync::mpsc::Receiver<Option<m::ImpactPreview>>>>,
    reconnect_at: f64,
    /// Signing in from the saved session (shown by the login screen instead of the form).
    auto_user: Option<String>,
    /// Welcome received on the current connection.
    online: bool,
    /// Commands issued while offline, sent after the next Welcome.
    pending: RefCell<Vec<ClientMsg>>,
    /// Reconnect backoff (s): 1, 2, 4 ... 30, reset on Welcome.
    retry_delay: f64,
    last_rx: f64,
    last_ping: f64,
    last_error: Option<String>,
    forecast: Forecaster,
    /// last `predicted` answer: (run gen, frame index, view)
    predicted: RefCell<Option<(u64, usize, Rc<m::GameView>)>>,
    /// "While you were away" report waiting to be shown / dismissed.
    report: Option<m::AwayReport>,
}

impl ClientBackend {
    /// Log back in with the saved session, if there is one (stay logged in across launches).
    pub fn resume(&mut self) {
        if let Some((server, user, pass)) = crate::notify::load() {
            if std::env::var("SPACE_RTS_SERVER").is_err() {
                self.server = server;
            }
            self.auto_user = Some(user.clone());
            self.creds = Some((user, pass));
            self.logged_in = false;
            self.connect(false);
        }
    }

    pub fn new(server: String) -> ClientBackend {
        ClientBackend {
            server,
            creds: None,
            net: None,
            logged_in: false,
            events: vec![],
            games: vec![],
            game: None,
            me: 0,
            anchor: (0.0, 0.0),
            view: None,
            paths: HashMap::new(),
            last_event_seq: 0,
            was_running: false,
            plan_cache: RefCell::new(HashMap::new()),
            plan_jobs: RefCell::new(HashMap::new()),
            impact_cache: RefCell::new(HashMap::new()),
            impact_jobs: RefCell::new(HashMap::new()),
            reconnect_at: 0.0,
            auto_user: None,
            online: false,
            pending: RefCell::new(vec![]),
            retry_delay: 1.0,
            last_rx: 0.0,
            last_ping: 0.0,
            last_error: None,
            forecast: Forecaster::default(),
            predicted: RefCell::new(None),
            report: None,
        }
    }

    fn send(&self, msg: ClientMsg) {
        match &self.net {
            Some(n) if self.online => n.send(msg),
            // queued until we're back: taps made while reconnecting still go through
            _ => self.pending.borrow_mut().push(msg),
        }
    }

    /// Should we be holding a connection open (logged in, or resuming a saved session)?
    fn want_online(&self) -> bool {
        self.creds.is_some() && (self.logged_in || self.auto_user.is_some())
    }

    fn drop_connection(&mut self, why: Option<String>) {
        self.net = None;
        self.online = false;
        self.last_error = why;
        self.reconnect_at = unix_now() + self.retry_delay;
        self.retry_delay = (self.retry_delay * 2.0).min(30.0);
    }

    fn code(&self) -> Option<String> {
        self.game.as_ref().map(|g| g.code.clone())
    }

    fn connect(&mut self, register: bool) {
        if let Some((u, p)) = &self.creds {
            self.net = Some(Net::connect(self.server.clone(), ClientMsg::Hello { user: u.clone(), pass: p.clone(), register }));
        }
    }

    /// Game time "now" according to the server clock.
    fn now(&self) -> f64 {
        match &self.game {
            Some(g) if g.status == Status::Running => {
                let t = self.anchor.1 + (unix_now() - self.anchor.0) * g.settings.speed;
                t.min(g.end_t)
            }
            Some(g) => g.t,
            None => 0.0,
        }
    }

    fn on_msg(&mut self, msg: ServerMsg) {
        match msg {
            ServerMsg::Welcome { user } => {
                self.logged_in = true;
                self.auto_user = None;
                self.online = true;
                self.retry_delay = 1.0;
                self.last_error = None;
                if let Some((u, p)) = &self.creds {
                    crate::notify::save(&self.server, u, p);
                }
                self.events.push(BackendEvent::LoggedIn { user });
                // re-open the game we were looking at after a reconnect
                if let Some(code) = self.code() {
                    self.send(ClientMsg::Open { code });
                }
                self.send(ClientMsg::ListGames);
                let queued: Vec<ClientMsg> = self.pending.borrow_mut().drain(..).collect();
                for m in queued {
                    self.send(m);
                }
            }
            ServerMsg::Error { msg } => {
                if !self.logged_in {
                    if self.auto_user.take().is_some() {
                        // saved session no longer valid (password changed / account gone)
                        crate::notify::clear();
                    }
                    self.events.push(BackendEvent::LoginFailed(msg));
                    self.net = None;
                    self.creds = None;
                } else if self.game.as_ref().is_none_or(|g| g.status == Status::Lobby) {
                    self.events.push(BackendEvent::LobbyFailed(msg));
                } else {
                    self.notice(m::NoticeKind::Bad, "Server", &msg, None);
                }
            }
            ServerMsg::Games(list) => self.games = list,
            ServerMsg::State { game, me, real_time } => self.on_state(*game, me, real_time),
            ServerMsg::CmdResult { ok, msg } => {
                if !ok {
                    self.notice(m::NoticeKind::Bad, "Can't do that", &msg, None);
                }
            }
            ServerMsg::Report { code, since, now, events } => {
                // only worth a sheet after a real absence (not a reconnect blip)
                if self.code().as_deref() == Some(code.as_str()) && now - since >= REPORT_MIN_AWAY && !events.is_empty() {
                    self.report = Some(away_report(since, now, &events));
                }
            }
            ServerMsg::Events(_) | ServerMsg::Pong => {}
        }
    }

    fn notice(&mut self, kind: m::NoticeKind, title: &str, body: &str, focus: Option<m::Focus>) {
        self.events.push(BackendEvent::Notice(m::Notice { kind, title: title.into(), body: body.into(), focus }));
    }

    fn on_state(&mut self, game: Game, me: g::PlayerId, real_time: f64) {
        let new_game = self.game.as_ref().is_none_or(|old| old.code != game.code);
        if new_game {
            self.last_event_seq = game.events_for(me).last().map(|e| e.seq).unwrap_or(0);
            self.was_running = false;
            self.plan_cache.borrow_mut().clear();
        self.impact_cache.borrow_mut().clear();
            self.impact_cache.borrow_mut().clear();
        }
        // notices for events we haven't seen
        let fresh: Vec<g::Event> = game.events_for(me).filter(|e| e.seq > self.last_event_seq).cloned().collect();
        for e in &fresh {
            let kind = match e.kind {
                g::EventKind::Info => m::NoticeKind::Info,
                g::EventKind::Good => m::NoticeKind::Good,
                g::EventKind::Bad => m::NoticeKind::Bad,
                g::EventKind::Battle => m::NoticeKind::Battle,
                g::EventKind::Sensor => m::NoticeKind::Info,
            };
            let focus = e.ship_ref.map(|s| m::Focus::Ship(s.0)).or(e.body_ref.map(|b| m::Focus::Body(b.0)));
            self.notice(kind, &e.title, &e.body, focus);
            self.last_event_seq = self.last_event_seq.max(e.seq);
        }
        self.me = me;
        self.anchor = (real_time, game.t);
        let running = game.status != Status::Lobby;
        if !running {
            self.events.push(BackendEvent::Lobby(lobby_info(&game)));
        } else if !self.was_running {
            self.events.push(BackendEvent::GameStarted);
        }
        self.was_running = running;
        self.game = Some(game);
        self.plan_cache.borrow_mut().clear();
        self.rebuild();
        self.forecast.restart(self.game.as_ref().unwrap(), me, self.now());
    }

    /// How far the forecast for the latest snapshot has got (`forecast_until` also counts the
    /// previous run, which keeps answering until the new one catches up).
    pub fn forecast_fresh_until(&self) -> f64 {
        self.forecast.shared.lock().unwrap().cur.until
    }

    /// Forecast frame in effect at `t` (only for the future).
    fn frame_at(&self, t: f64) -> Option<(u64, usize, Arc<forecast::Frame>)> {
        if t <= self.now() + 1.0 {
            return None;
        }
        self.forecast.shared.lock().unwrap().at(t)
    }

    /// A ship that only exists in the forecast: its first appearance.
    fn planned_ship(&self, id: ShipId) -> Option<forecast::FShip> {
        let sh = self.forecast.shared.lock().unwrap();
        [&sh.cur, &sh.prev].iter().find_map(|r| r.frames.iter().find_map(|f| f.ship(id).cloned()))
    }

    /// Game snapshot + "now" to plan `ship` on: the live game, or for a ship still in a build queue,
    /// the live game with that ship parked at its shipyard as of the time it is ready.
    fn plan_input(&self, ship: ShipId) -> Option<(Game, f64)> {
        let game = self.game.as_ref()?;
        if ship < PLANNED_BASE {
            return Some((game.clone(), self.now()));
        }
        let fs = self.planned_ship(ship)?;
        let (body, ready) = fs.job?;
        let class = sim_class(fs.model.class);
        let mut g2 = game.clone();
        let sid = g2.spawn(self.me, class, SimBody(body));
        if let Some(s) = g2.ships.iter_mut().find(|s| s.id == sid) {
            s.id = g::ShipId(ship);
            s.name = fs.model.name.clone();
        }
        Some((g2, ready.max(self.now())))
    }

    /// Rebuild the view-model and cached ship paths from the local game copy.
    fn rebuild(&mut self) {
        let Some(game) = &mut self.game else {
            self.view = None;
            return;
        };
        let sys = game.system.clone();
        self.paths.clear();
        for s in game.ships.iter_mut() {
            if let ShipState::Flying(f) = &mut s.state {
                let tr = f.trajectory(&sys).clone();
                self.paths.insert(s.id.0, to_model_traj(&sys, &tr));
            }
        }
        let me = self.me;
        for c in game.players.get_mut(me as usize).map(|p| p.contacts.as_mut_slice()).unwrap_or_default() {
            // ghosts coast on their last known arc
            if let ShipState::Flying(f) = &mut c.ship.state {
                let tr = f.trajectory(&sys).clone();
                self.paths.insert(c.ship.id.0, to_model_traj(&sys, &tr));
            }
        }
        for s in &game.ships {
            if let ShipState::Parked { departure: Some(d), .. } = &s.state {
                let tr = sim::traj::predict(&sys, d.state, &[], Some(d.to), d.state.t + g::Flight::HORIZON);
                self.paths.insert(s.id.0, to_model_traj(&sys, &tr));
            }
        }
        let game = self.game.as_ref().unwrap();
        self.view = Some(build_view(game, self.me, self.now(), &self.paths));
    }

    fn ship_ids(&self, id: ShipId) -> Vec<g::ShipId> {
        vec![g::ShipId(id)]
    }
}

/// Plan the cheapest push of `ast` into `target` on a snapshot of the game (worker thread).
/// None if the asteroid isn't ours to push, the tech is missing, or no course hits.
fn compute_impact(g: &Game, me: g::PlayerId, now: f64, ast: BodyId, target: BodyId) -> Option<m::ImpactPreview> {
    let (a, t) = (SimBody(ast), SimBody(target));
    if a.0 as usize >= g.bodies.len() || t.0 as usize >= g.bodies.len() || !g.can_drive_asteroids(me) {
        return None;
    }
    let here = g.ships.iter().any(|s| s.owner == me && s.parked_at() == Some(a));
    if g.bodies[a.0 as usize].owner != Some(me) && !here {
        return None;
    }
    let p = sim::asteroid::plan_impact(g, a, t, now)?;
    let imp = p.impact?;
    let busy = g.drives.iter().any(|d| d.body == a);
    Some(m::ImpactPreview {
        asteroid: ast,
        target,
        burn_t: p.burn.t - p.burn_s / 2.0,
        burn_s: p.burn_s,
        dv: p.dv,
        fuel: p.fuel,
        fuel_available: sim::asteroid::fuel_available(g, me, a),
        drive_cost: if busy {
            m::Cost { matter: 0.0, energy: 0.0, seconds: 0.0 }
        } else {
            m::Cost { matter: sim::asteroid::DRIVE_MATTER, energy: sim::asteroid::DRIVE_ENERGY, seconds: sim::asteroid::INSTALL_S }
        },
        impact_t: Some(imp.t),
        impact_speed: imp.v,
        // GJ → J
        impact_energy: imp.energy * 1e9,
        severity: imp.severity,
        path: to_model_traj(&g.system, &p.traj),
    })
}

/// Auto-plan a transfer on a snapshot of the game (runs on a worker thread for the async path).
fn compute_plan(g: &Game, now: f64, ship: ShipId, target: BodyId, after: f64) -> Option<m::TransferPlan> {
    let s = g.ship(g::ShipId(ship))?;
    let depart = if after > now + 120.0 { Some(after) } else { None };
    let mut game = g.clone();
    game.t = now;
    let (p, _) = game.plan_for(s, SimBody(target), depart).ok()?;
    let from = s.parked_at()?;
    let first = p.to;
    let span = sim::plan::search_span(&g.system, from, first);
    let windows = sim::plan::windows(&g.system, from, first, now, span).into_iter().map(|w| w.depart_t).collect();
    let dep_fuel = s.fuel_for(p.depart_dv);
    Some(m::TransferPlan {
        ship,
        target,
        depart_t: p.depart_t(),
        arrive_t: p.arrive_t,
        dv_depart: p.depart_dv,
        dv_capture: p.capture_dv,
        fuel_needed: (dep_fuel + g::fuel_needed(s.class, (s.fuel - dep_fuel).max(0.0), p.capture_dv)) as f32,
        path: to_model_traj(&g.system, &p.traj),
        windows,
    })
}

fn lobby_info(game: &Game) -> m::LobbyInfo {
    m::LobbyInfo {
        code: game.code.clone(),
        name: game.settings.name.clone(),
        players: game.players.iter().map(|p| player_model(p)).collect(),
        max_players: game.settings.max_players,
        host: game.host,
        length_days: (game.settings.length / 86_400.0).round() as u32,
        speed: game.settings.speed,
        bots: game.players.iter().filter_map(|p| p.bot.clone()).collect(),
    }
}

fn player_model(p: &g::Player) -> m::Player {
    m::Player { id: p.id, name: p.name.clone(), color: p.color, faction: p.faction.clone(), online: p.bot.is_none() }
}

fn ship_class(c: g::ShipClass) -> m::ShipClass {
    match c {
        g::ShipClass::Scout => m::ShipClass::Scout,
        g::ShipClass::Interceptor => m::ShipClass::Interceptor,
        g::ShipClass::Gunship => m::ShipClass::Gunship,
        g::ShipClass::Bomber => m::ShipClass::Bomber,
        g::ShipClass::Dreadnought => m::ShipClass::Dreadnought,
        g::ShipClass::Tanker => m::ShipClass::Tanker,
        g::ShipClass::Constructor => m::ShipClass::Constructor,
    }
}

fn sim_class(c: m::ShipClass) -> g::ShipClass {
    match c {
        m::ShipClass::Scout => g::ShipClass::Scout,
        m::ShipClass::Interceptor => g::ShipClass::Interceptor,
        m::ShipClass::Gunship => g::ShipClass::Gunship,
        m::ShipClass::Bomber => g::ShipClass::Bomber,
        m::ShipClass::Dreadnought => g::ShipClass::Dreadnought,
        m::ShipClass::Tanker => g::ShipClass::Tanker,
        m::ShipClass::Constructor => g::ShipClass::Constructor,
    }
}

fn upgrade_by_name(name: &str) -> Option<g::Upgrade> {
    g::Upgrade::ALL.into_iter().find(|u| format!("{u:?}").eq_ignore_ascii_case(name))
}

fn tech_by_name(name: &str) -> Option<Tech> {
    Tech::ALL.into_iter().find(|t| format!("{t:?}") == name)
}

fn station_by_name(name: &str) -> Option<StationKind> {
    StationKind::ALL.into_iter().find(|k| format!("{k:?}") == name)
}

fn arr(v: V2) -> [f64; 2] {
    [v.x, v.y]
}

/// Timed absolute polyline of a sim trajectory (dense near bodies, where paths curve hardest).
pub(crate) fn to_model_traj(sys: &sim::System, tr: &SimTraj) -> m::Trajectory {
    let mut points = vec![];
    for s in &tr.segs {
        let span = s.t1 - s.t0;
        if span <= 0.0 {
            continue;
        }
        let n = if sys.body(s.parent).is_star() { 160 } else { 80 };
        let mu = sys.body(s.parent).mu;
        for i in 0..=n {
            let t = s.t0 + span * i as f64 / n as f64;
            let p = sys.pos(s.parent, t) + sim::kepler::propagate(mu, s.r0, s.v0, t - s.t0).0;
            points.push((t, arr(p)));
        }
    }
    m::Trajectory { points }
}

/// Fog of war: last sightings of enemy ships that are out of sensor range now.
fn ghosts(game: &Game, me: g::PlayerId) -> impl Iterator<Item = &sim::fog::Contact> {
    game.players.get(me as usize).into_iter().flat_map(|p| p.contacts.iter()).filter(move |c| game.ship(c.ship.id).is_none())
}

pub(crate) fn ship_model(s: &g::Ship, trajectory: Option<m::Trajectory>) -> m::Ship {
    let st = s.class.stats();
    let (parked_at, nodes, destination) = match &s.state {
        ShipState::Parked { body, departure } => (Some(body.0), vec![], departure.as_ref().map(|d| d.to.0)),
        ShipState::Flying(f) => (
            None,
            f.burns.iter().map(|b| m::ManeuverNode { t: b.t, prograde: b.prograde, radial: b.radial }).collect(),
            f.dest.map(|d| d.0),
        ),
    };
    m::Ship {
        id: s.id.0,
        name: s.name.clone(),
        class: ship_class(s.class),
        owner: s.owner,
        count: 1,
        fuel: s.fuel as f32,
        fuel_max: st.fuel_cap as f32,
        parked_at,
        trajectory,
        nodes,
        destination,
        retreat: s.order == g::StandingOrder::Retreat,
        seen_at: None,
        planned: None,
    }
}

/// Game seconds away before the report sheet is worth showing.
const REPORT_MIN_AWAY: f64 = 900.0;

fn away_report(since: f64, now: f64, events: &[g::Event]) -> m::AwayReport {
    let items = events
        .iter()
        .map(|e| {
            let territory = e.title.starts_with("Territory") || e.title == "Battle sighted";
            let section = match e.kind {
                g::EventKind::Battle => m::ReportSection::Battles,
                _ if territory => m::ReportSection::Battles,
                g::EventKind::Sensor => m::ReportSection::Contacts,
                _ => m::ReportSection::Empire,
            };
            let tone = match e.kind {
                g::EventKind::Good => 1,
                g::EventKind::Bad | g::EventKind::Battle => -1,
                g::EventKind::Sensor if e.title == "Contact" || e.title == "Engine burn" => -1,
                _ => 0,
            };
            m::ReportItem {
                t: e.t,
                section,
                title: e.title.clone(),
                text: e.body.clone(),
                focus: e.ship_ref.map(|s| m::Focus::Ship(s.0)).or(e.body_ref.map(|b| m::Focus::Body(b.0))),
                fallback: e.body_ref.map(|b| m::Focus::Body(b.0)),
                tone,
            }
        })
        .collect();
    m::AwayReport { since, now, items }
}

fn build_view(game: &Game, me: g::PlayerId, now: f64, paths: &HashMap<u32, m::Trajectory>) -> m::GameView {
    let sys = &game.system;
    let bodies = sys
        .bodies
        .iter()
        // asteroids that crashed into something no longer exist
        .filter(|b| !game.bodies[b.id.0 as usize].gone)
        .map(|b| {
            let st = &game.bodies[b.id.0 as usize];
            let orbit_path = match b.orbit {
                Some(o) => (0..=96).map(|i| arr(o.state(o.period() * i as f64 / 96.0 - o.m0 / o.n).0)).collect(),
                None => vec![],
            };
            let upgrades = g::Upgrade::ALL.iter().filter(|u| st.level(**u) > 0).map(|u| format!("{} {}", u.label(), st.level(*u))).collect();
            m::Body {
                id: b.id.0,
                name: b.name.clone(),
                kind: match b.kind {
                    SimKind::Star => m::BodyKind::Star,
                    SimKind::Planet | SimKind::GasGiant => m::BodyKind::Planet,
                    SimKind::Moon => m::BodyKind::Moon,
                    SimKind::Asteroid => m::BodyKind::Asteroid,
                },
                parent: b.parent.map(|p| p.0),
                radius: b.radius,
                soi: if b.is_star() { 0.0 } else { b.soi },
                owner: st.owner,
                intel: {
                    let seen = game.players.get(me as usize).and_then(|p| p.intel.get(b.id.0 as usize).cloned().flatten());
                    seen.map(|i| m::Intel {
                        seen_at: i.seen_t,
                        power: i.power.iter().filter(|p| p.0 != me).map(|p| p.1).sum::<f64>() as f32,
                        upgrades,
                        stations: game.stations.iter().filter(|st| st.body == b.id && st.owner != me).map(|st| st.kind.label().to_string()).collect(),
                    })
                },
                art: if st.shattered {
                    Some("rubble".into())
                } else if b.kind == SimKind::GasGiant {
                    Some("planet_gas".into())
                } else {
                    None
                },
                orbit_path,
            }
        })
        .collect();
    let ships = game
        .ships
        .iter()
        .map(|s| ship_model(s, paths.get(&s.id.0).cloned()))
        .chain(ghosts(game, me).map(|c| {
            let s = &c.ship;
            m::Ship {
                id: s.id.0,
                name: s.name.clone(),
                class: ship_class(s.class),
                owner: s.owner,
                count: 1,
                fuel: 0.0,
                fuel_max: s.class.stats().fuel_cap as f32,
                parked_at: s.parked_at().map(|b| b.0),
                trajectory: paths.get(&s.id.0).cloned(),
                nodes: vec![],
                destination: match &s.state {
                    ShipState::Flying(f) => f.dest.map(|d| d.0),
                    _ => None,
                },
                retreat: false,
                seen_at: Some(c.seen_t),
                planned: None,
            }
        }))
        .collect();
    let mut res = m::Resources::default();
    if let Some(p) = game.players.get(me as usize) {
        res.matter = p.matter;
        res.energy = p.energy;
    }
    for (i, b) in game.bodies.iter().enumerate() {
        if b.owner == Some(me) {
            let (pm, pe, pf) = game.production(SimBody(i as u32));
            res.matter_rate += pm;
            res.energy_rate += pe;
            res.fuel_rate += pf;
            res.fuel += b.fuel;
        }
    }
    m::GameView {
        host: game.host,
        me,
        now,
        ends_at: game.end_t,
        speed: game.settings.speed,
        bodies,
        ships,
        players: game.players.iter().map(player_model).collect(),
        resources: res,
        chat: game.chat.iter().map(|c| m::ChatMsg { from: c.from, t: c.t, text: c.text.clone() }).collect(),
    }
}

impl Backend for ClientBackend {
    fn poll(&mut self) -> Vec<BackendEvent> {
        let incoming = self.net.as_ref().map(|n| n.poll()).unwrap_or_default();
        let wall = unix_now();
        for e in incoming {
            match e {
                NetEvent::Connected => self.last_rx = wall,
                NetEvent::Msg(m) => {
                    self.last_rx = wall;
                    self.on_msg(m);
                }
                NetEvent::Failed(err) => {
                    if self.want_online() {
                        self.drop_connection(Some(err));
                    } else {
                        self.net = None;
                        self.events.push(BackendEvent::LoginFailed(err));
                        self.creds = None;
                    }
                }
                NetEvent::Closed => {
                    if self.want_online() {
                        self.drop_connection(Some("connection lost".into()));
                    } else {
                        self.net = None;
                    }
                }
            }
        }
        // Heartbeat: phones drop sockets silently (sleep, wifi handover), so ping and give up on a
        // connection that has been quiet for 40 s.
        if self.net.is_some() && self.online {
            if wall - self.last_ping > 15.0 {
                self.last_ping = wall;
                self.send(ClientMsg::Ping);
            }
            if wall - self.last_rx > 40.0 {
                self.drop_connection(Some("server not responding".into()));
            }
        }
        if self.net.is_none() && self.want_online() && wall > self.reconnect_at {
            self.last_rx = wall;
            self.connect(false);
        }
        // the server re-sends state on every event and every 20 s; between snapshots ship and body
        // positions come from their (deterministic) trajectories, so only the clock moves here
        let now = self.now();
        if let Some(v) = &mut self.view {
            v.now = now;
        }
        std::mem::take(&mut self.events)
    }

    fn act(&mut self, action: UiAction) {
        let code = self.code();
        match action {
            UiAction::Login { server, user, pass, register } => {
                if !server.trim().is_empty() {
                    self.server = server.trim().to_string();
                    crate::save_server(&self.server);
                }
                self.creds = Some((user, pass));
                self.logged_in = false;
                self.connect(register);
            }
            UiAction::AimAsteroid { body, target } => {
                if let Some(code) = code {
                    self.send(ClientMsg::Cmd { code, cmd: Command::AimAsteroid { body: SimBody(body), target: SimBody(target) } });
                }
            }
            UiAction::Reconnect => {
                self.reconnect_at = 0.0;
                self.retry_delay = 1.0;
            }
            UiAction::Logout => {
                crate::notify::clear();
                self.auto_user = None;
                self.online = false;
                self.pending.borrow_mut().clear();
                self.was_running = false;
                self.net = None;
                self.creds = None;
                self.logged_in = false;
                self.game = None;
                self.view = None;
                self.forecast.clear();
                self.events.push(BackendEvent::LoggedOut);
            }
            UiAction::CreateGame { name, length_days, speed, bots } => self.send(ClientMsg::CreateGame {
                name,
                size: SystemSize::Medium,
                length_days: length_days as f64,
                speed,
                max_players: 4,
                bots,
            }),
            UiAction::JoinGame { code } => self.send(ClientMsg::JoinGame { code }),
            UiAction::OpenGame { code } => self.send(ClientMsg::Open { code }),
            UiAction::AddBot { difficulty } => {
                if let Some(code) = code {
                    self.send(ClientMsg::AddBot { code, difficulty });
                }
            }
            UiAction::SetFaction(faction) => {
                if let Some(code) = code {
                    self.send(ClientMsg::SetFaction { code, faction });
                }
            }
            UiAction::StartGame => {
                if let Some(code) = code {
                    self.send(ClientMsg::StartGame { code });
                }
            }
            UiAction::LeaveLobby | UiAction::BackToMenu => {
                // so re-opening the same game reports GameStarted again
                self.was_running = false;
                self.game = None;
                self.view = None;
                self.forecast.clear();
                self.send(ClientMsg::ListGames);
            }
            UiAction::ScheduleTransfer(p) if p.ship >= PLANNED_BASE => match (code, self.planned_ship(p.ship).and_then(|s| s.job)) {
                (Some(code), Some((body, done_t))) => self.send(ClientMsg::Cmd {
                    code,
                    cmd: Command::QueueOrder { body: SimBody(body), done_t, to: Some(SimBody(p.target)), depart_t: Some(p.depart_t) },
                }),
                _ => self.notice(m::NoticeKind::Bad, "Can't do that", "That ship is no longer in the build queue", None),
            },
            UiAction::CancelFlight { ship } if ship >= PLANNED_BASE => {
                if let (Some(code), Some((body, done_t))) = (code, self.planned_ship(ship).and_then(|s| s.job)) {
                    self.send(ClientMsg::Cmd { code, cmd: Command::QueueOrder { body: SimBody(body), done_t, to: None, depart_t: None } });
                }
            }
            UiAction::SetOrder { ship, .. } | UiAction::SetNodes { ship, .. } if ship >= PLANNED_BASE => {
                self.notice(m::NoticeKind::Info, "Not built yet", "Standing orders and manual burns need the ship to exist", None)
            }
            UiAction::ScheduleTransfer(p) => {
                if let Some(code) = code {
                    let ships = self.ship_ids(p.ship);
                    self.send(ClientMsg::Cmd { code, cmd: Command::Transfer { ships, to: SimBody(p.target), depart_t: Some(p.depart_t) } });
                }
            }
            UiAction::SetNodes { ship, nodes } => {
                if let Some(code) = code {
                    let burns = nodes.iter().map(|n| Burn { t: n.t, prograde: n.prograde, radial: n.radial }).collect();
                    self.send(ClientMsg::Cmd { code, cmd: Command::SetBurns { ship: g::ShipId(ship), burns } });
                }
            }
            UiAction::CancelFlight { ship } => {
                if let Some(code) = code {
                    self.send(ClientMsg::Cmd { code, cmd: Command::CancelDeparture { ship: g::ShipId(ship) } });
                }
            }
            UiAction::DismissReport => self.report = None,
            UiAction::Chat(text) => {
                if let Some(code) = code {
                    self.send(ClientMsg::Cmd { code, cmd: Command::Chat { text } });
                }
            }
            UiAction::Build { body, class } => {
                if let Some(code) = code {
                    self.send(ClientMsg::Cmd { code, cmd: Command::Build { body: SimBody(body), class: sim_class(class) } });
                }
            }
            UiAction::Upgrade { body, upgrade } => match (code, upgrade_by_name(&upgrade)) {
                (Some(code), Some(upgrade)) => self.send(ClientMsg::Cmd { code, cmd: Command::Upgrade { body: SimBody(body), upgrade } }),
                _ => self.notice(m::NoticeKind::Bad, "Unknown upgrade", &upgrade, None),
            },
            UiAction::SetOrder { ship, retreat } => {
                if let Some(code) = code {
                    let order = if retreat { g::StandingOrder::Retreat } else { g::StandingOrder::Hold };
                    self.send(ClientMsg::Cmd { code, cmd: Command::SetOrder { ship: g::ShipId(ship), order } });
                }
            }
            UiAction::SetSpeed { speed } => {
                if let Some(code) = code {
                    self.send(ClientMsg::Cmd { code, cmd: Command::SetSpeed { speed } });
                }
            }
            UiAction::ExtendGame { days } => {
                if let Some(code) = code {
                    self.send(ClientMsg::Cmd { code, cmd: Command::Extend { seconds: days as f64 * 86_400.0 } });
                }
            }
            UiAction::Research { body, tech } => match (code, tech_by_name(&tech)) {
                (Some(code), Some(tech)) => self.send(ClientMsg::Cmd { code, cmd: Command::Research { body: SimBody(body), tech } }),
                _ => self.notice(m::NoticeKind::Bad, "Unknown tech", &tech, None),
            },
            UiAction::BuildStation { body, kind } => match (code, station_by_name(&kind)) {
                (Some(code), Some(kind)) => self.send(ClientMsg::Cmd { code, cmd: Command::BuildStation { body: SimBody(body), kind } }),
                _ => self.notice(m::NoticeKind::Bad, "Unknown station", &kind, None),
            },
        }
    }

    fn my_games(&self) -> Vec<m::GameSummary> {
        self.games
            .iter()
            .map(|s| m::GameSummary {
                code: s.code.clone(),
                name: s.name.clone(),
                day: (s.t / 86_400.0) as u32 + 1,
                length_days: (s.length / 86_400.0).round() as u32,
                players: s.players.len(),
                unread: 0,
            })
            .collect()
    }

    fn view(&self) -> Option<&m::GameView> {
        self.view.as_ref().filter(|_| self.game.as_ref().is_some_and(|g| g.status != Status::Lobby))
    }

    fn body_pos(&self, id: BodyId, t: f64) -> [f64; 2] {
        match &self.game {
            Some(g) if (id as usize) < g.system.bodies.len() => arr(g.system.pos(SimBody(id), t)),
            _ => [0.0, 0.0],
        }
    }

    fn ship_pos(&self, id: ShipId, t: f64) -> Option<[f64; 2]> {
        let g = self.game.as_ref()?;
        if let Some((_, _, f)) = self.frame_at(t) {
            // ghosts (last sightings) aren't in the forecast; they keep coasting below
            if !self.view.as_ref().is_some_and(|v| v.ship(id).is_some_and(|s| s.seen_at.is_some())) {
                return forecast::ship_pos(&g.system, f.ship(id)?, t);
            }
        }
        let s = g.ship(g::ShipId(id)).or_else(|| ghosts(g, self.me).find(|c| c.ship.id.0 == id).map(|c| &c.ship))?;
        match &s.state {
            ShipState::Parked { body, departure } => {
                if let (Some(d), Some(p)) = (departure, self.paths.get(&id)) {
                    if t >= d.state.t {
                        return p.sample(t).or_else(|| p.points.last().map(|q| q.1));
                    }
                }
                Some(arr(g.system.pos(*body, t)))
            }
            ShipState::Flying(_) => {
                let p = self.paths.get(&id)?;
                p.sample(t).or_else(|| if t < p.start()? { p.points.first().map(|q| q.1) } else { p.points.last().map(|q| q.1) })
            }
        }
    }

    fn plan_transfer(&self, ship: ShipId, target: BodyId, after: f64) -> Option<m::TransferPlan> {
        let key = (ship, target, (after / 60.0) as i64);
        if let Some(p) = self.plan_cache.borrow().get(&key) {
            return p.clone();
        }
        let (game, now) = self.plan_input(ship)?;
        let res = compute_plan(&game, now, ship, target, after.max(now));
        self.plan_cache.borrow_mut().insert(key, res.clone());
        res
    }

    fn plan_impact_async(&self, asteroid: BodyId, target: BodyId) -> ui::ImpactPoll {
        let key = (asteroid, target);
        if let Some(p) = self.impact_cache.borrow().get(&key) {
            return ui::ImpactPoll::Done(p.clone());
        }
        let mut jobs = self.impact_jobs.borrow_mut();
        if let Some(rx) = jobs.get(&key) {
            return match rx.try_recv() {
                Ok(res) => {
                    jobs.remove(&key);
                    self.impact_cache.borrow_mut().insert(key, res.clone());
                    ui::ImpactPoll::Done(res)
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ui::ImpactPoll::Pending,
                Err(_) => {
                    jobs.remove(&key);
                    ui::ImpactPoll::Done(None)
                }
            };
        }
        let Some(game) = self.game.clone() else { return ui::ImpactPoll::Done(None) };
        let (now, me) = (self.now(), self.me);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(compute_impact(&game, me, now, asteroid, target));
        });
        jobs.insert(key, rx);
        ui::ImpactPoll::Pending
    }

    fn plan_transfer_async(&self, ship: ShipId, target: BodyId, after: f64) -> ui::PlanPoll {
        let key = (ship, target, (after / 60.0) as i64);
        if let Some(p) = self.plan_cache.borrow().get(&key) {
            return ui::PlanPoll::Done(p.clone());
        }
        let mut jobs = self.plan_jobs.borrow_mut();
        if let Some(rx) = jobs.get(&key) {
            return match rx.try_recv() {
                Ok(res) => {
                    jobs.remove(&key);
                    self.plan_cache.borrow_mut().insert(key, res.clone());
                    ui::PlanPoll::Done(res)
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ui::PlanPoll::Pending,
                Err(_) => {
                    jobs.remove(&key);
                    ui::PlanPoll::Done(None)
                }
            };
        }
        let Some((game, now)) = self.plan_input(ship) else { return ui::PlanPoll::Done(None) };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(compute_plan(&game, now, ship, target, after.max(now)));
        });
        jobs.insert(key, rx);
        ui::PlanPoll::Pending
    }

    fn describe(&self, key: &str) -> Option<String> {
        let mut parts = key.split(':');
        match (parts.next()?, parts.next()?) {
            ("ship", name) => g::ShipClass::ALL.into_iter().find(|c| c.stats().name.eq_ignore_ascii_case(name)).map(|c| c.describe()),
            ("upgrade", name) => {
                let level = parts.next().and_then(|l| l.parse().ok()).unwrap_or(1);
                upgrade_by_name(name).map(|u| u.describe(level))
            }
            _ => None,
        }
    }

    fn predicted(&self, t: f64) -> Option<Rc<m::GameView>> {
        let (gen, idx, f) = self.frame_at(t)?;
        if let Some((g0, i0, v)) = &*self.predicted.borrow() {
            if (*g0, *i0) == (gen, idx) {
                return Some(v.clone());
            }
        }
        let mut v = self.view.clone()?;
        v.ships = f
            .ships
            .iter()
            .map(|s| m::Ship { trajectory: s.path.as_deref().cloned(), ..s.model.clone() })
            .chain(v.ships.iter().filter(|s| s.seen_at.is_some()).cloned())
            .collect();
        for b in v.bodies.iter_mut() {
            b.owner = f.owners.get(b.id as usize).copied().flatten();
        }
        v.resources.matter = f.matter;
        v.resources.energy = f.energy;
        let v = Rc::new(v);
        *self.predicted.borrow_mut() = Some((gen, idx, v.clone()));
        Some(v)
    }

    fn forecast_until(&self) -> f64 {
        self.forecast.shared.lock().unwrap().until()
    }

    fn default_server(&self) -> String {
        self.server.clone()
    }

    fn auto_login(&self) -> Option<String> {
        self.auto_user.clone().filter(|_| !self.logged_in)
    }

    fn connection_status(&self) -> Option<String> {
        if !self.want_online() || self.online {
            return None;
        }
        let wait = (self.reconnect_at - unix_now()).ceil();
        let why = self.last_error.clone().unwrap_or_else(|| "connecting".into());
        Some(if self.net.is_some() || wait <= 0.0 { format!("Connecting to {}...", self.server) } else { format!("Offline ({why}). Retrying in {wait:.0}s") })
    }

    fn body_info(&self, id: BodyId) -> Option<m::BodyInfo> {
        let g = self.game.as_ref()?;
        let b = SimBody(id);
        let st = g.bodies.get(id as usize)?;
        let mine = st.owner == Some(self.me);
        let pending = |u: g::Upgrade| st.queue.iter().filter(|j| matches!(j.kind, g::JobKind::Upgrade(x) if x == u)).count() as u8;
        let upgrades = g::Upgrade::ALL
            .iter()
            .map(|&u| {
                let next_level = st.level(u) + pending(u) + 1;
                m::UpgradeInfo {
                    name: format!("{u:?}"),
                    level: st.level(u),
                    max: g::Upgrade::MAX_LEVEL,
                    next: (mine && next_level <= g::Upgrade::MAX_LEVEL).then(|| {
                        let (matter, energy, seconds) = u.cost(next_level);
                        m::Cost { matter, energy, seconds }
                    }),
                }
            })
            .collect();
        let buildable = if mine {
            g::ShipClass::ALL
                .iter()
                .filter(|c| g.yard_level(self.me, b) >= c.stats().min_shipyard)
                .map(|&c| {
                    let s = c.stats();
                    (ship_class(c), m::Cost { matter: s.matter, energy: s.energy, seconds: s.build_time })
                })
                .collect()
        } else {
            vec![]
        };
        let queue = st
            .queue
            .iter()
            .map(|j| {
                let label = match &j.kind {
                    g::JobKind::Ship(c) => c.stats().name.to_string(),
                    g::JobKind::Upgrade(u) => u.label().to_string(),
                };
                (label, j.done_t)
            })
            .collect();
        let (matter_rate, energy_rate, fuel_rate) = if mine { g.production(b) } else { (0.0, 0.0, 0.0) };
        Some(m::BodyInfo { upgrades, buildable, queue, fuel: st.fuel, power: g.power_at(b), matter_rate, energy_rate, fuel_rate })
    }

    fn research(&self) -> Option<m::ResearchView> {
        let g = self.game.as_ref()?;
        let me = self.me;
        let p = g.players.get(me as usize)?;
        let labs: Vec<m::LabInfo> = (0..g.bodies.len())
            .filter(|&i| g.bodies[i].owner == Some(me) && g.bodies[i].level(g::Upgrade::Research) > 0)
            .map(|i| {
                let b = SimBody(i as u32);
                let busy = p.research.iter().find(|r| r.body == b).map(|r| (r.tech.label().to_string(), r.done_t));
                m::LabInfo { body: i as u32, name: g.system.body(b).name.clone(), level: g.bodies[i].level(g::Upgrade::Research), busy }
            })
            .collect();
        let techs = Tech::ALL
            .iter()
            .map(|&t| {
                let state = if p.techs.contains(&t) {
                    m::TechState::Done
                } else if let Some(r) = p.research.iter().find(|r| r.tech == t) {
                    m::TechState::Running { lab: r.body.0, done_t: r.done_t }
                } else {
                    // the best idle lab that can take it, else the reason the first lab gives
                    let mut best: Option<(u8, u32)> = None;
                    let mut why = if labs.is_empty() { "needs a research lab".to_string() } else { String::new() };
                    for l in &labs {
                        match g.research_blocker(me, SimBody(l.body), t) {
                            None if best.is_none_or(|b| l.level > b.0) => best = Some((l.level, l.body)),
                            None => {}
                            Some(w) if why.is_empty() || w.starts_with("needs ") => why = w,
                            Some(_) => {}
                        }
                    }
                    match best {
                        Some((_, lab)) => m::TechState::Ready { lab },
                        None => m::TechState::Locked(why),
                    }
                };
                let (matter, energy, seconds) = t.cost();
                m::TechInfo {
                    id: format!("{t:?}"),
                    name: t.label().to_string(),
                    branch: t.branch().label().to_string(),
                    tier: t.tier(),
                    desc: t.describe().to_string(),
                    cost: m::Cost { matter, energy, seconds },
                    min_lab: t.min_lab(),
                    state,
                }
            })
            .collect();
        debug_assert!(Branch::ALL.iter().zip(m::TECH_BRANCHES).all(|(b, n)| b.label() == n));
        Some(m::ResearchView { techs, labs })
    }

    fn away_report(&self) -> Option<&m::AwayReport> {
        self.report.as_ref()
    }

    fn stations(&self, id: BodyId) -> Option<m::StationsView> {
        let g = self.game.as_ref()?;
        let me = self.me;
        let b = SimBody(id);
        g.bodies.get(id as usize)?;
        if g.system.body(b).is_star() {
            return None;
        }
        let workers = g.workers(me, b);
        let present = workers > 0;
        let here = g
            .stations
            .iter()
            .filter(|s| s.body == b)
            .map(|s| m::StationInfo {
                kind: format!("{:?}", s.kind),
                owner: s.owner,
                building_until: s.building().then_some(s.done_t),
                progress: if s.building() { (1.0 - s.work_left / s.kind.cost().2).clamp(0.0, 1.0) as f32 } else { 1.0 },
                hp: (s.hp / s.kind.max_hp()) as f32,
            })
            .collect();
        let options = StationKind::ALL
            .iter()
            .map(|&k| {
                let (matter, energy, seconds) = k.cost();
                m::StationOption {
                    kind: format!("{k:?}"),
                    name: k.label().to_string(),
                    desc: k.describe().to_string(),
                    cost: m::Cost { matter, energy, seconds },
                    blocker: g.station_blocker(me, b, k),
                }
            })
            .collect();
        Some(m::StationsView { here, options, present, constructors: g.constructors_at(me, b), workers })
    }

    fn node_outcome(&self, ship: ShipId, nodes: &[m::ManeuverNode]) -> Option<m::NodeOutcome> {
        let g = self.game.as_ref()?;
        let s = g.ship(g::ShipId(ship))?;
        let ShipState::Flying(f) = &s.state else { return None };
        let sys = &g.system;
        let mut burns: Vec<Burn> = nodes.iter().map(|n| Burn { t: n.t, prograde: n.prograde, radial: n.radial }).collect();
        burns.sort_by(|a, b| a.t.total_cmp(&b.t));
        let tr = sim::traj::predict(sys, f.start, &burns, f.dest, f.start.t + g::Flight::HORIZON);
        let end_t = tr.end.t();
        let end_pos = tr.abs_pos(sys, end_t).map(arr).unwrap_or([0.0, 0.0]);
        // fuel: every applied burn, then the capture burn if the path ends captured
        let mut fuel = s.fuel;
        for b in &tr.burns {
            fuel -= s.fuel_for_with(fuel.max(0.0), b.dv());
        }
        let captured = match tr.end {
            sim::traj::End::Capture { body, rp, vp, .. } => {
                let dv = vp - sim::kepler::circular_speed(sys.body(body).mu, rp);
                fuel -= s.fuel_for_with(fuel.max(0.0), dv);
                Some(body.0)
            }
            _ => None,
        };
        // encounter: first SOI entered after the first burn, else the closest pass to any body
        let after = burns.first().map(|b| b.t).unwrap_or(f.start.t);
        let start_parent = tr.seg_at(after).map(|seg| seg.parent);
        let entered = tr.segs.iter().find(|seg| seg.t0 >= after && Some(seg.parent) != start_parent && !sys.body(seg.parent).is_star());
        let encounter = match entered {
            Some(seg) => {
                let rp = sim::kepler::Conic::from_state(sys.body(seg.parent).mu, seg.r0, seg.v0).rp;
                Some(m::Encounter { body: seg.parent.0, t: seg.t0, distance: rp, inside_soi: true })
            }
            None => sys
                .bodies
                .iter()
                .filter(|b| !b.is_star() && Some(b.id) != start_parent)
                .map(|b| (b, tr.closest_approach(sys, b.id)))
                .filter(|(_, (_, t))| *t >= after)
                .min_by(|a, b| (a.1 .0 / a.0.soi).total_cmp(&(b.1 .0 / b.0.soi)))
                .map(|(b, (d, t))| m::Encounter { body: b.id.0, t, distance: d, inside_soi: d < b.soi }),
        };
        Some(m::NodeOutcome { end_t, end_pos, captured, encounter, fuel_left: Some(fuel) })
    }

    fn preview_nodes(&self, ship: ShipId, nodes: &[m::ManeuverNode]) -> Option<m::Trajectory> {
        let g = self.game.as_ref()?;
        let s = g.ship(g::ShipId(ship))?;
        let ShipState::Flying(f) = &s.state else { return None };
        let mut burns: Vec<Burn> = nodes.iter().map(|n| Burn { t: n.t, prograde: n.prograde, radial: n.radial }).collect();
        burns.sort_by(|a, b| a.t.total_cmp(&b.t));
        let tr = sim::traj::predict(&g.system, f.start, &burns, f.dest, f.start.t + g::Flight::HORIZON);
        Some(to_model_traj(&g.system, &tr))
    }
}
