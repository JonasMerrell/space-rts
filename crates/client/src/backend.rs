//! `ui::Backend` over the network connection and a local copy of the authoritative `sim::Game`.
//! Between server snapshots the local copy is advanced with the same deterministic sim, so the
//! map keeps moving smoothly; every snapshot replaces it.
use crate::forecast::{self, Forecaster, PLANNED_BASE};
use crate::net::{Net, NetEvent};
use crate::notify::Secret;
use crate::unix_now;
use proto::{ClientMsg, GameSummary as WireSummary, ServerMsg};
use sim::game::{self as g, Command, Game, ShipState, Status};
use sim::station::StationKind;
use sim::tech::{Branch, Tech};
use sim::system::{BodyKind as SimKind, SystemSize};
use sim::traj::{Burn, Trajectory as SimTraj};
use sim::waitlist::Wish;
use sim::{BodyId as SimBody, V2};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use ui::model::{self as m, BodyId, ShipId};
use ui::{Backend, BackendEvent, UiAction};

/// Slow work (course planning, impact previews) kept off the render loop: a worker thread natively.
/// The browser has no threads, so there it runs inside a later `try_recv`, after the UI has had a
/// couple of frames to show its "plotting course" state.
#[cfg(not(target_arch = "wasm32"))]
struct Job<T>(std::sync::mpsc::Receiver<T>);

#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + 'static> Job<T> {
    fn spawn(f: impl FnOnce() -> T + Send + 'static) -> Job<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        Job(rx)
    }
    fn try_recv(&self) -> Result<T, std::sync::mpsc::TryRecvError> {
        self.0.try_recv()
    }
}

#[cfg(target_arch = "wasm32")]
struct Job<T> {
    f: RefCell<Option<Box<dyn FnOnce() -> T>>>,
    waits: std::cell::Cell<u8>,
}

#[cfg(target_arch = "wasm32")]
impl<T: 'static> Job<T> {
    fn spawn(f: impl FnOnce() -> T + 'static) -> Job<T> {
        Job { f: RefCell::new(Some(Box::new(f))), waits: std::cell::Cell::new(2) }
    }
    fn try_recv(&self) -> Result<T, std::sync::mpsc::TryRecvError> {
        if self.waits.get() > 0 {
            self.waits.set(self.waits.get() - 1);
            return Err(std::sync::mpsc::TryRecvError::Empty);
        }
        self.f.borrow_mut().take().map(|f| f()).ok_or(std::sync::mpsc::TryRecvError::Disconnected)
    }
}

pub struct ClientBackend {
    server: String,
    /// who we log in as, and with what (a token once the server handed one out)
    creds: Option<(String, Secret)>,
    /// invite code typed with Register (invite-only servers)
    invite: Option<String>,
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
    plan_jobs: RefCell<HashMap<(ShipId, BodyId, i64), Job<Option<m::TransferPlan>>>>,
    /// fuel ↔ time slider: exact courses between two options, keyed by both tokens + fraction
    between: RefCell<HashMap<String, Option<m::TransferPlan>>>,
    between_jobs: RefCell<HashMap<String, Job<Option<m::TransferPlan>>>>,
    /// transfer-window tool, keyed by ship and quarter hour
    windows: RefCell<HashMap<(ShipId, i64), Option<m::WindowsView>>>,
    windows_jobs: RefCell<HashMap<(ShipId, i64), Job<Option<m::WindowsView>>>>,
    /// asteroid impact previews keyed by (asteroid, target), and their background jobs
    impact_cache: RefCell<HashMap<(BodyId, BodyId), Option<m::ImpactPreview>>>,
    impact_jobs: RefCell<HashMap<(BodyId, BodyId), Job<Option<m::ImpactPreview>>>>,
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
    /// Sensors report for the open game, rebuilt with the view.
    sensors: Option<m::SensorsReport>,
    /// (game code, game time) up to which the player has seen the report's log: the server's
    /// last-active time on open, then whenever the report is closed.
    sensors_seen: Option<(String, f64)>,
}

impl ClientBackend {
    /// Log back in with the saved session, if there is one (stay logged in across launches).
    pub fn resume(&mut self) {
        if let Some((server, user, secret)) = crate::notify::load() {
            if crate::env_server().is_err() {
                self.server = crate::migrate_server(server);
            }
            self.auto_user = Some(user.clone());
            self.creds = Some((user, secret));
            self.logged_in = false;
            self.connect(false);
        }
    }

    pub fn new(server: String) -> ClientBackend {
        ClientBackend {
            server,
            creds: None,
            invite: None,
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
            between: RefCell::new(HashMap::new()),
            between_jobs: RefCell::new(HashMap::new()),
            windows: RefCell::new(HashMap::new()),
            windows_jobs: RefCell::new(HashMap::new()),
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
            sensors: None,
            sensors_seen: None,
        }
    }

    fn send(&self, msg: ClientMsg) {
        match &self.net {
            Some(n) if self.online => n.send(msg),
            // queued until we're back: taps made while reconnecting still go through
            _ => self.pending.borrow_mut().push(msg),
        }
    }

    /// Build/upgrade/station/research: start it now if we can, otherwise put it on the
    /// waiting-for-funds list (it starts by itself once income covers it).
    fn send_wish(&self, code: String, wish: Wish) {
        let now = self.game.as_ref().is_some_and(|g| {
            let p = &g.players[self.me as usize];
            let (m, e) = g.wish_cost(self.me, wish, 0);
            g.wish_blocker(self.me, wish).is_none() && p.matter >= m && p.energy >= e
        });
        let cmd = if now { wish.command() } else { Command::Wait { wish } };
        self.send(ClientMsg::Cmd { code, cmd });
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
        if let Some((u, secret)) = &self.creds {
            let (pass, token) = match secret {
                Secret::Pass(p) => (p.clone(), None),
                Secret::Token(t) => (String::new(), Some(t.clone())),
            };
            let invite = if register { self.invite.clone() } else { None };
            // stay logged in: a password login asks for a session token to keep instead of the password
            let hello = ClientMsg::Hello { user: u.clone(), pass, register, invite, token, stay: true };
            self.net = Some(Net::connect(self.server.clone(), hello));
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
            ServerMsg::Welcome { user, token } => {
                // a reconnect after a dropped connection: keep the player where they were
                let reconnect = self.logged_in;
                self.logged_in = true;
                self.auto_user = None;
                self.online = true;
                self.retry_delay = 1.0;
                self.last_error = None;
                if let Some((u, secret)) = &mut self.creds {
                    // from now on the token stands in for the password, here and in the saved session
                    if let Some(t) = token {
                        *secret = Secret::Token(t);
                    }
                    crate::notify::save(&self.server, u, secret);
                }
                if !reconnect {
                    self.events.push(BackendEvent::LoggedIn { user });
                }
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
                } else if self.game.is_none() {
                    // e.g. a bad join code: back to the menu with the error
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
            ServerMsg::Report { code, since, .. } => {
                // sent on Open, before the State: what happened since is "new" in the sensors
                // report (a reconnect keeps what the player already looked at)
                if self.sensors_seen.as_ref().is_none_or(|s| s.0 != code) {
                    self.sensors_seen = Some((code, since));
                }
            }
            ServerMsg::Events(_) | ServerMsg::Pong => {}
            ServerMsg::GameDeleted { code, name } => {
                if self.code().as_deref() == Some(code.as_str()) {
                    self.game = None;
                    self.view = None;
                    self.was_running = false;
                }
                self.notice(m::NoticeKind::Bad, "War deleted", &format!("The host deleted \"{name}\""), None);
            }
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
            self.windows.borrow_mut().clear();
            self.between.borrow_mut().clear();
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
        self.between.borrow_mut().clear();
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
                let tr = sim::traj::predict(&sys, d.state, &d.burns, Some(d.to), d.state.t + g::Flight::HORIZON);
                self.paths.insert(s.id.0, to_model_traj(&sys, &tr));
            }
        }
        let game = self.game.as_ref().unwrap();
        let now = self.now();
        self.view = Some(build_view(game, self.me, now, &self.paths));
        let seen = match &self.sensors_seen {
            Some((code, t)) if *code == game.code => *t,
            // no Report for this game (lobby, or just created): nothing counts as new
            _ => {
                self.sensors_seen = Some((game.code.clone(), now));
                now
            }
        };
        self.sensors = (game.status != Status::Lobby).then(|| crate::sensors::report(game, self.me, now, seen));
    }

    fn ship_ids(&self, id: ShipId) -> Vec<g::ShipId> {
        vec![self.lead(id)]
    }

    /// The command for a ship ability button (`ship_abilities` keys).
    fn ability_cmd(&self, ship: ShipId, key: &str) -> Option<Command> {
        let g = self.game.as_ref()?;
        let ships = real_ships(g, &[ship]);
        match key {
            "cloak_on" | "cloak_off" => Some(Command::Cloak { ships, on: key == "cloak_on" }),
            "mines" => ships.iter().filter_map(|&id| g.ship(id)).find(|s| s.class == g::ShipClass::Constructor).and_then(|s| s.parked_at()).map(|body| Command::LayMines { body }),
            "decoy" => {
                let s = g.ship(*ships.first()?)?;
                // cycle through the warship looks
                const LOOKS: [g::ShipClass; 5] = [g::ShipClass::Gunship, g::ShipClass::Dreadnought, g::ShipClass::Bomber, g::ShipClass::Interceptor, g::ShipClass::Tanker];
                let i = LOOKS.iter().position(|&c| Some(c) == s.mimic).map_or(0, |i| (i + 1) % LOOKS.len());
                Some(Command::SetDecoy { ship: s.id, looks_like: LOOKS[i] })
            }
            _ => None,
        }
    }

    fn lead(&self, id: ShipId) -> g::ShipId {
        self.game.as_ref().map(|g| lead(g, id)).unwrap_or(g::ShipId(id))
    }

    fn fleet_cmd(&self, a: UiAction) -> Option<sim::fleet::FleetCommand> {
        use sim::fleet::FleetCommand as F;
        let g = self.game.as_ref()?;
        let fleet = |id: ShipId| token_fleet(g, id).map(|f| f.id);
        Some(match a {
            UiAction::FormFleet { ships, name } => F::Form { ships: real_ships(g, &ships), name },
            UiAction::JoinFleet { fleet: f, ships } => F::Join { fleet: fleet(f)?, ships: real_ships(g, &ships) },
            UiAction::LeaveFleet { ships, split } => F::Leave { ships: real_ships(g, &ships), split },
            UiAction::MergeFleets { into, from } => F::Merge { into: fleet(into)?, from: fleet(from)? },
            UiAction::RenameFleet { fleet: f, name } => F::Rename { fleet: fleet(f)?, name },
            UiAction::DisbandFleet { fleet: f } => F::Disband { fleet: fleet(f)? },
            _ => return None,
        })
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

/// Fleets are shown as one map token with id `FLEET_TOKEN + fleet id`, so they never collide
/// with ship ids (those count up from 1) or forecast-only ships (from `PLANNED_BASE` up).
const FLEET_TOKEN: u32 = 0x4000_0000;

pub(crate) fn fleet_token(f: &sim::fleet::Fleet) -> ShipId {
    FLEET_TOKEN + f.id.0
}

fn token_fleet(g: &Game, id: ShipId) -> Option<&sim::fleet::Fleet> {
    if !(FLEET_TOKEN..PLANNED_BASE).contains(&id) {
        return None;
    }
    g.fleet(sim::fleet::FleetId(id - FLEET_TOKEN))
}

/// The real ship behind a token: the fleet's first ship for a fleet token. The sim applies
/// orders on one member to the whole fleet.
fn lead(g: &Game, id: ShipId) -> g::ShipId {
    match token_fleet(g, id) {
        Some(f) => f.ships.iter().copied().find(|&s| g.ship(s).is_some()).unwrap_or(g::ShipId(id)),
        None => g::ShipId(id),
    }
}

/// Real ship ids for a list of tokens (fleet tokens expand to all their ships).
fn real_ships(g: &Game, ids: &[ShipId]) -> Vec<g::ShipId> {
    ids.iter()
        .flat_map(|&id| match token_fleet(g, id) {
            Some(f) => f.ships.clone(),
            None => vec![g::ShipId(id)],
        })
        .collect()
}

/// Auto-plan a transfer on a snapshot of the game (runs on a worker thread for the async path).
/// Course options for the fuel ↔ time slider (cheapest … fastest, gravity assists included) for
/// the first hop of the route; the returned plan is the cheapest, with all options in `options`.
fn compute_plan(g: &Game, now: f64, ship: ShipId, target: BodyId, after: f64) -> Option<m::TransferPlan> {
    let s = g.ship(lead(g, ship))?;
    if s.parked_at().is_none() {
        return compute_reroute(g, now, ship, target, after);
    }
    let depart = if after > now + 120.0 { Some(after) } else { None };
    let from = s.parked_at()?;
    let first = *sim::plan::route(&g.system, from, SimBody(target)).first()?;
    // the fastest option must fit the thirstiest ship of a fleet
    let budget = g.with_fleetmates(s.id).iter().filter_map(|&id| g.ship(id)).map(|m| m.dv_left()).fold(s.dv_left(), f64::min);
    let opts = sim::plan::tradeoff(&g.system, from, first, now, depart, budget);
    if opts.is_empty() {
        return None;
    }
    let span = sim::plan::search_span(&g.system, from, first);
    let windows: Vec<f64> = sim::plan::windows(&g.system, from, first, now, span).into_iter().map(|w| w.depart_t).collect();
    let mut all: Vec<m::TransferPlan> = opts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut mp = model_plan(g, s, ship, target, p, windows.clone());
            if opts.len() > 1 && (i == 0 || i + 1 == opts.len()) {
                mp.label = if i == 0 { "CHEAPEST" } else { "FASTEST" }.into();
            }
            mp
        })
        .collect();
    let list = all.clone();
    for p in &mut all {
        p.options = list.clone();
    }
    all.into_iter().next()
}

/// New course for a ship in flight (one burn on its current path), slider stops cheapest first.
fn compute_reroute(g: &Game, now: f64, ship: ShipId, target: BodyId, after: f64) -> Option<m::TransferPlan> {
    let s = g.ship(lead(g, ship))?;
    let opts = g.reroute_options(s.id, SimBody(target), now, after);
    let mut all: Vec<m::TransferPlan> = opts.iter().map(|p| model_reroute(g, s, ship, p)).collect();
    let n = all.len();
    for (i, p) in all.iter_mut().enumerate() {
        if n > 1 && (i == 0 || i + 1 == n) {
            p.label = if i == 0 { "CHEAPEST" } else { "FASTEST" }.into();
        }
    }
    let list = all.clone();
    for p in &mut all {
        p.options = list.clone();
    }
    all.into_iter().next()
}

fn model_reroute(g: &Game, s: &g::Ship, ship: ShipId, p: &sim::reroute::CoursePlan) -> m::TransferPlan {
    let flying = g.with_fleetmates(s.id);
    let fuel_have: f64 = flying.iter().filter_map(|&id| g.ship(id)).map(|m| m.fuel).sum();
    let fuel_needed: f64 = flying
        .iter()
        .filter_map(|&id| g.ship(id))
        .map(|m| {
            let dep = m.fuel_for(p.depart_dv);
            dep + g::fuel_needed(m.class, (m.fuel - dep).max(0.0), p.capture_dv)
        })
        .sum();
    // the drawn path starts at the burn; before it the ship keeps its current path
    m::TransferPlan {
        ship,
        target: p.hint.to.0,
        depart_t: p.hint.depart_t,
        arrive_t: p.arrive_t,
        dv_depart: p.depart_dv,
        dv_mid: 0.0,
        dv_capture: p.capture_dv,
        fuel_needed: fuel_needed as f32,
        fuel_have: fuel_have as f32,
        fuel_problem: g.fleet_fuel_check(&flying, p.depart_dv, p.capture_dv).err(),
        path: to_model_traj(&g.system, &p.traj),
        via: vec![],
        label: String::new(),
        options: vec![],
        token: serde_json::to_string(&p.hint).unwrap_or_default(),
        windows: vec![],
        intercept: None,
    }
}

/// Intercept courses (cheapest … fastest) for our `ship` token on enemy ship `target`, on what our
/// sensors know of it. A target sitting in orbit gets an ordinary transfer to that body instead.
fn compute_intercept(g: &Game, now: f64, ship: ShipId, target: ShipId, matched: bool) -> Option<m::TransferPlan> {
    let s = g.ship(lead(g, ship))?;
    let t = g.ship(g::ShipId(target))?;
    if let Some(body) = t.parked_at() {
        let mut p = compute_plan(g, now, ship, body.0, now)?;
        let meet = arr(g.system.pos(body, p.arrive_t));
        let tag = |p: &mut m::TransferPlan| p.intercept = Some(m::Intercept { target, closing: 0.0, matched: true, meet });
        tag(&mut p);
        p.options.iter_mut().for_each(tag);
        return Some(p);
    }
    let tgt = g.view_path(t.id)?;
    let opts = g.intercept_options(s.id, t.id, &tgt, now, matched);
    let mut all: Vec<m::TransferPlan> = opts.iter().map(|p| model_intercept(g, s, ship, p, &tgt)).collect();
    let n = all.len();
    for (i, p) in all.iter_mut().enumerate() {
        if n > 1 && (i == 0 || i + 1 == n) {
            p.label = if i == 0 { "CHEAPEST" } else { "FASTEST" }.into();
        }
    }
    let list = all.clone();
    for p in &mut all {
        p.options = list.clone();
    }
    all.into_iter().next()
}

fn model_intercept(g: &Game, s: &g::Ship, ship: ShipId, p: &sim::intercept::InterceptPlan, tgt: &SimTraj) -> m::TransferPlan {
    let flying = g.with_fleetmates(s.id);
    let fuel_have: f64 = flying.iter().filter_map(|&id| g.ship(id)).map(|m| m.fuel).sum();
    let fuel_needed: f64 = flying
        .iter()
        .filter_map(|&id| g.ship(id))
        .map(|m| {
            let dep = m.fuel_for(p.depart_dv);
            dep + g::fuel_needed(m.class, (m.fuel - dep).max(0.0), p.match_dv)
        })
        .sum();
    let meet = tgt.abs_pos(&g.system, p.arrive_t()).map(arr).unwrap_or([0.0, 0.0]);
    // the body the meeting happens around (titles fall back on it)
    let near = tgt.state_at(&g.system, p.arrive_t()).map(|st| st.parent.0).unwrap_or(0);
    m::TransferPlan {
        ship,
        target: near,
        depart_t: p.depart_t(),
        arrive_t: p.arrive_t(),
        dv_depart: p.depart_dv,
        dv_mid: 0.0,
        dv_capture: p.match_dv,
        fuel_needed: fuel_needed as f32,
        fuel_have: fuel_have as f32,
        fuel_problem: g.fleet_fuel_check(&flying, p.depart_dv, p.match_dv).err(),
        path: to_model_traj(&g.system, &p.traj),
        via: vec![],
        label: String::new(),
        options: vec![],
        token: serde_json::to_string(&p.hint).unwrap_or_default(),
        windows: vec![],
        intercept: Some(m::Intercept { target: p.hint.target.0, closing: p.closing, matched: p.hint.matched, meet }),
    }
}

/// Transfer-window tool: opportunities from where the ship is parked over the next 3 days.
fn compute_windows(g: &Game, now: f64, ship: ShipId) -> Option<m::WindowsView> {
    const HORIZON: f64 = 3.0 * 86_400.0;
    let s = g.ship(lead(g, ship))?;
    let from = s.parked_at()?;
    let budget = g.with_fleetmates(s.id).iter().filter_map(|&id| g.ship(id)).map(|m| m.dv_left()).fold(s.dv_left(), f64::min);
    let est = |w: &sim::plan::Window| m::WindowEst { depart_t: w.depart_t, arrive_t: w.arrive_t, dv: w.dv };
    let list = sim::plan::opportunities(&g.system, from, now, HORIZON, budget)
        .into_iter()
        .map(|o| m::Opportunity {
            target: o.to.0,
            via: o.via.iter().map(|b| b.0).collect(),
            windows: o.windows.iter().map(est).collect(),
            fastest: o.fastest.as_ref().map(est),
            strip: o.strip.iter().map(|&d| d as f32).collect(),
        })
        .collect();
    Some(m::WindowsView { ship, from: from.0, start: now + 60.0, horizon: HORIZON, budget, list })
}

/// The exact course `f` of the way between two neighbouring slider options (see `plan::between`).
fn compute_between(g: &Game, a: &m::TransferPlan, b: &m::TransferPlan, f: f64) -> Option<m::TransferPlan> {
    if let (Ok(ha), Ok(hb)) = (serde_json::from_str::<sim::reroute::Hint>(&a.token), serde_json::from_str::<sim::reroute::Hint>(&b.token)) {
        let s = g.ship(lead(g, a.ship))?;
        let path = g.coasting_path(s.id)?;
        let p = sim::reroute::between(&g.system, &path, ha, hb, f)?;
        return Some(model_reroute(g, s, a.ship, &p));
    }
    if let (Ok(ha), Ok(hb)) = (serde_json::from_str::<sim::intercept::Hint>(&a.token), serde_json::from_str::<sim::intercept::Hint>(&b.token)) {
        let s = g.ship(lead(g, a.ship))?;
        let tgt = g.view_path(ha.target)?;
        let ch = sim::intercept::Chaser::of(&g.system, s);
        let p = sim::intercept::between(&g.system, &ch, &tgt, ha, hb, f)?;
        return Some(model_intercept(g, s, a.ship, &p, &tgt));
    }
    let s = g.ship(lead(g, a.ship))?;
    let from = s.parked_at()?;
    let first = *sim::plan::route(&g.system, from, SimBody(a.target)).first()?;
    let rung = |p: &m::TransferPlan| {
        let hint = serde_json::from_str(&p.token).ok()?;
        Some(sim::plan::Rung { depart_t: p.depart_t, arrive_t: p.arrive_t, dv: p.dv_depart + p.dv_mid + p.dv_capture, hint })
    };
    let p = sim::plan::between(&g.system, from, first, rung(a)?, rung(b)?, f)?;
    Some(model_plan(g, s, a.ship, a.target, &p, a.windows.clone()))
}

fn model_plan(g: &Game, s: &g::Ship, ship: ShipId, target: BodyId, p: &sim::plan::TransferPlan, windows: Vec<f64>) -> m::TransferPlan {
    // fuel for every ship that flies (the whole fleet for a fleet token), judged like the server does
    let flying = g.with_fleetmates(s.id);
    let fuel_have: f64 = flying.iter().filter_map(|&id| g.ship(id)).map(|m| m.fuel).sum();
    let fuel_needed: f64 = flying
        .iter()
        .filter_map(|&id| g.ship(id))
        .map(|m| {
            let dep = m.fuel_for(p.depart_dv + p.mid_dv());
            dep + g::fuel_needed(m.class, (m.fuel - dep).max(0.0), p.capture_dv)
        })
        .sum();
    m::TransferPlan {
        ship,
        target,
        depart_t: p.depart_t(),
        arrive_t: p.arrive_t,
        dv_depart: p.depart_dv,
        dv_mid: p.mid_dv(),
        dv_capture: p.capture_dv,
        fuel_needed: fuel_needed as f32,
        fuel_have: fuel_have as f32,
        fuel_problem: if flying.len() > 1 {
            g.fleet_fuel_check(&flying, p.depart_dv, p.capture_dv).err()
        } else {
            (fuel_needed > fuel_have).then(|| format!("not enough fuel: needs {fuel_needed:.0} t, has {fuel_have:.0} t"))
        },
        path: to_model_traj(&g.system, &p.traj),
        via: p.flybys.iter().map(|b| b.0).collect(),
        label: String::new(),
        options: vec![],
        token: p.hint.and_then(|h| serde_json::to_string(&h).ok()).unwrap_or_default(),
        windows,
        intercept: None,
    }
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
        g::ShipClass::Decoy => m::ShipClass::Decoy,
        g::ShipClass::Torpedo => m::ShipClass::Torpedo,
        g::ShipClass::Slug => m::ShipClass::Slug,
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
        m::ShipClass::Decoy => g::ShipClass::Decoy,
        m::ShipClass::Torpedo => g::ShipClass::Torpedo,
        m::ShipClass::Slug => g::ShipClass::Slug,
    }
}

fn upgrade_by_name(name: &str) -> Option<g::Upgrade> {
    g::Upgrade::ALL.into_iter().find(|u| format!("{u:?}").eq_ignore_ascii_case(name))
}

fn tech_by_name(name: &str) -> Option<Tech> {
    Tech::every().find(|t| format!("{t:?}") == name)
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
    let flybys = tr
        .flybys(sys)
        .into_iter()
        .map(|f| m::FlybyMark { body: f.body.0, t: f.t, pos: arr(tr.abs_pos(sys, f.t).unwrap_or_default()), rp: f.rp, gain: f.gain })
        .collect();
    m::Trajectory { points, flybys }
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
        members: vec![],
    }
}

/// Replace our fleets' ship models with one token per fleet (models of other ships pass through).
fn group_fleets(game: &Game, me: g::PlayerId, ships: Vec<m::Ship>) -> Vec<m::Ship> {
    let fleets: Vec<&sim::fleet::Fleet> = game.fleets.iter().filter(|f| f.owner == me).collect();
    let mut out = vec![];
    let mut groups: Vec<Vec<m::Ship>> = vec![vec![]; fleets.len()];
    for s in ships {
        match fleets.iter().position(|f| f.ships.contains(&g::ShipId(s.id))) {
            Some(k) => groups[k].push(s),
            None => out.push(s),
        }
    }
    for (f, mut ms) in fleets.into_iter().zip(groups) {
        if ms.is_empty() {
            continue;
        }
        // members in fleet order; the first one leads (position, course, orders)
        ms.sort_by_key(|s| f.ships.iter().position(|x| x.0 == s.id));
        let weakest = ms.iter().min_by(|a, b| (a.fuel / a.fuel_max.max(1.0)).total_cmp(&(b.fuel / b.fuel_max.max(1.0)))).unwrap();
        // the most common class names the token's sprite
        let class = ms.iter().map(|s| s.class).max_by_key(|c| ms.iter().filter(|s| s.class == *c).count()).unwrap();
        out.push(m::Ship {
            id: fleet_token(f),
            name: f.name.clone(),
            class,
            count: ms.len() as u32,
            fuel: weakest.fuel,
            fuel_max: weakest.fuel_max,
            members: ms.iter().map(|s| m::FleetMember { id: s.id, name: s.name.clone(), class: s.class, fuel: s.fuel, fuel_max: s.fuel_max }).collect(),
            ..ms[0].clone()
        });
    }
    out
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
        .collect::<Vec<_>>();
    let ships = group_fleets(game, me, ships)
        .into_iter()
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
                members: vec![],
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
        code: game.code.clone(),
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
        lobby: (game.status == Status::Lobby).then(|| lobby_info(game)),
        home: game.players.get(me as usize).and_then(|p| p.home).map(|b| b.0),
    }
}

impl ClientBackend {
    /// Poll a course computed on a worker thread (slider blends, intercepts), cached by `key`
    /// until the next game state.
    fn poll_course(&self, key: String, ship: ShipId, job: impl FnOnce(Game, f64) -> Option<m::TransferPlan> + Send + 'static) -> ui::PlanPoll {
        if let Some(p) = self.between.borrow().get(&key) {
            return ui::PlanPoll::Done(p.clone());
        }
        let mut jobs = self.between_jobs.borrow_mut();
        if let Some(rx) = jobs.get(&key) {
            return match rx.try_recv() {
                Ok(res) => {
                    jobs.remove(&key);
                    let mut cache = self.between.borrow_mut();
                    if cache.len() > 64 {
                        cache.clear();
                    }
                    cache.insert(key, res.clone());
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
        jobs.insert(key, Job::spawn(move || job(game, now)));
        ui::PlanPoll::Pending
    }
}

impl Backend for ClientBackend {
    fn poll(&mut self) -> Vec<BackendEvent> {
        let incoming = self.net.as_ref().map(|n| n.poll()).unwrap_or_default();
        #[cfg(target_arch = "wasm32")]
        self.forecast.pump(0.006);
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
            UiAction::Login { server, user, pass, register, invite } => {
                self.invite = Some(invite).filter(|c| !c.is_empty());
                if !server.trim().is_empty() {
                    self.server = server.trim().to_string();
                    crate::save_server(&self.server);
                }
                self.creds = Some((user, Secret::Pass(pass)));
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
                // revoke this device's session on the server (best effort: offline it just expires)
                if let (Some(n), Some((_, Secret::Token(t)))) = (&self.net, &self.creds) {
                    if self.online {
                        n.send(ClientMsg::Logout { token: t.clone() });
                    }
                }
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
            // opening a game always ends on the game screen, even if it's the one we had before
            UiAction::JoinGame { code } => {
                self.was_running = false;
                self.send(ClientMsg::JoinGame { code })
            }
            UiAction::DeleteGame { code } => self.send(ClientMsg::DeleteGame { code }),
            UiAction::Feedback { bug, text } => {
                let platform = if cfg!(target_arch = "wasm32") { "web" } else if cfg!(target_os = "android") { "android" } else { "desktop" };
                let client = format!("{} {platform}", env!("CARGO_PKG_VERSION"));
                self.send(ClientMsg::Feedback { bug, text, game: code, client });
            }
            UiAction::OpenGame { code } => {
                self.was_running = false;
                self.send(ClientMsg::Open { code })
            }
            UiAction::AddBot { difficulty } => {
                if let Some(code) = code {
                    self.send(ClientMsg::AddBot { code, difficulty });
                }
            }
            UiAction::RemoveBot { player } => {
                if let Some(code) = code {
                    self.send(ClientMsg::RemoveBot { code, player });
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
                    let to = SimBody(p.target);
                    let icp = serde_json::from_str::<sim::intercept::Hint>(&p.token).ok();
                    let rr = serde_json::from_str::<sim::reroute::Hint>(&p.token).ok();
                    let cmd = match serde_json::from_str::<sim::plan::PlanHint>(&p.token) {
                        _ if rr.is_some() => {
                            let h = rr.unwrap();
                            Command::Reroute { ships, to: h.to, depart_t: h.depart_t, aim_t: h.aim_t }
                        }
                        _ if icp.is_some() => {
                            let h = icp.unwrap();
                            Command::Intercept { ships, target: h.target, depart_t: h.depart_t, arrive_t: h.arrive_t, matched: h.matched }
                        }
                        Ok(hint) => Command::TransferPlanned { ships, to, depart_t: p.depart_t, hint },
                        Err(_) => Command::Transfer { ships, to, depart_t: Some(p.depart_t) },
                    };
                    self.send(ClientMsg::Cmd { code, cmd });
                }
            }
            UiAction::SetNodes { ship, nodes } => {
                if let Some(code) = code {
                    let burns = nodes.iter().map(|n| Burn { t: n.t, prograde: n.prograde, radial: n.radial }).collect();
                    self.send(ClientMsg::Cmd { code, cmd: Command::SetBurns { ship: self.lead(ship), burns } });
                }
            }
            UiAction::CancelFlight { ship } => {
                if let Some(code) = code {
                    self.send(ClientMsg::Cmd { code, cmd: Command::CancelDeparture { ship: self.lead(ship) } });
                }
            }
            UiAction::SensorsSeen => {
                if let Some(code) = self.code() {
                    self.sensors_seen = Some((code, self.now()));
                    self.rebuild();
                }
            }
            UiAction::MoveJob { body, from, to } => {
                if let Some(code) = code.clone() {
                    self.send(ClientMsg::Cmd { code, cmd: Command::MoveJob { body: SimBody(body), from, to } });
                }
            }
            UiAction::CancelJob { body, index } => {
                if let Some(code) = code {
                    self.send(ClientMsg::Cmd { code, cmd: Command::CancelJob { body: SimBody(body), index } });
                }
            }
            UiAction::Chat(text) => {
                if let Some(code) = code {
                    self.send(ClientMsg::Cmd { code, cmd: Command::Chat { text } });
                }
            }
            UiAction::Build { body, class } => {
                if let Some(code) = code {
                    self.send_wish(code, Wish::Build { body: SimBody(body), class: sim_class(class) });
                }
            }
            UiAction::Upgrade { body, upgrade } => match (code, upgrade_by_name(&upgrade)) {
                (Some(code), Some(upgrade)) => self.send_wish(code, Wish::Upgrade { body: SimBody(body), upgrade }),
                _ => self.notice(m::NoticeKind::Bad, "Unknown upgrade", &upgrade, None),
            },
            UiAction::Unwait { id } => {
                if let Some(code) = code {
                    self.send(ClientMsg::Cmd { code, cmd: Command::Unwait { id } });
                }
            }
            UiAction::MoveWait { id, to } => {
                if let Some(code) = code {
                    self.send(ClientMsg::Cmd { code, cmd: Command::MoveWait { id, to } });
                }
            }
            UiAction::ShipAbility { ship, key } => {
                if let (Some(code), Some(cmd)) = (code, self.ability_cmd(ship, &key)) {
                    self.send(ClientMsg::Cmd { code, cmd });
                }
            }
            UiAction::StationAbility { body, key } => {
                let station = self.game.as_ref().and_then(|g| g.stations.iter().find(|st| st.owner == self.me && st.body == SimBody(body) && st.kind == StationKind::Scanner).map(|st| st.id));
                match (code, station, key.as_str()) {
                    (Some(code), Some(station), "lidar") => self.send(ClientMsg::Cmd { code, cmd: Command::LidarSweep { station } }),
                    (Some(code), _, "slug") => self.send(ClientMsg::Cmd { code, cmd: Command::LoadSlug { body: SimBody(body) } }),
                    _ => {}
                }
            }
            UiAction::SetOrder { ship, retreat } => {
                if let Some(code) = code {
                    let order = if retreat { g::StandingOrder::Retreat } else { g::StandingOrder::Hold };
                    self.send(ClientMsg::Cmd { code, cmd: Command::SetOrder { ship: self.lead(ship), order } });
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
                (Some(code), Some(tech)) => self.send_wish(code, Wish::Research { body: SimBody(body), tech }),
                _ => self.notice(m::NoticeKind::Bad, "Unknown tech", &tech, None),
            },
            UiAction::FormFleet { .. }
            | UiAction::JoinFleet { .. }
            | UiAction::LeaveFleet { .. }
            | UiAction::MergeFleets { .. }
            | UiAction::RenameFleet { .. }
            | UiAction::DisbandFleet { .. } => {
                if let (Some(code), Some(cmd)) = (code, self.fleet_cmd(action)) {
                    self.send(ClientMsg::Cmd { code, cmd: Command::Fleet(cmd) });
                }
            }
            UiAction::BuildStation { body, kind } => match (code, station_by_name(&kind)) {
                (Some(code), Some(kind)) => self.send_wish(code, Wish::Station { body: SimBody(body), kind }),
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
        // also while waiting in the lobby: the map shows the generated system (view.lobby is Some)
        self.view.as_ref()
    }

    fn body_pos(&self, id: BodyId, t: f64) -> [f64; 2] {
        match &self.game {
            Some(g) if (id as usize) < g.system.bodies.len() => arr(g.system.pos(SimBody(id), t)),
            _ => [0.0, 0.0],
        }
    }

    fn ship_pos(&self, id: ShipId, t: f64) -> Option<[f64; 2]> {
        let g = self.game.as_ref()?;
        let id = lead(g, id).0;
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
        jobs.insert(key, Job::spawn(move || compute_impact(&game, me, now, asteroid, target)));
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
        jobs.insert(key, Job::spawn(move || compute_plan(&game, now, ship, target, after.max(now))));
        ui::PlanPoll::Pending
    }

    fn windows_async(&self, ship: ShipId) -> ui::WindowsPoll {
        let key = (ship, (self.now() / 900.0) as i64);
        if let Some(v) = self.windows.borrow().get(&key) {
            return ui::WindowsPoll::Done(v.clone());
        }
        let mut jobs = self.windows_jobs.borrow_mut();
        if let Some(rx) = jobs.get(&key) {
            return match rx.try_recv() {
                Ok(res) => {
                    jobs.remove(&key);
                    let mut cache = self.windows.borrow_mut();
                    cache.retain(|k, _| k.1 == key.1);
                    cache.insert(key, res.clone());
                    ui::WindowsPoll::Done(res)
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ui::WindowsPoll::Pending,
                Err(_) => {
                    jobs.remove(&key);
                    ui::WindowsPoll::Done(None)
                }
            };
        }
        // a new quarter hour: keep showing the last list while the fresh one is computed
        let stale = self.windows.borrow().iter().find(|(k, _)| k.0 == ship).map(|(_, v)| v.clone());
        let Some((game, now)) = self.plan_input(ship) else { return ui::WindowsPoll::Done(None) };
        jobs.insert(key, Job::spawn(move || compute_windows(&game, now, ship)));
        match stale {
            Some(v) => ui::WindowsPoll::Done(v),
            None => ui::WindowsPoll::Pending,
        }
    }

    fn plan_between_async(&self, a: &m::TransferPlan, b: &m::TransferPlan, f: f32) -> ui::PlanPoll {
        let key = format!("{}|{}|{}|{}|{}", a.token, a.depart_t.to_bits(), b.token, b.depart_t.to_bits(), f.to_bits());
        let (a, b) = (a.clone(), b.clone());
        self.poll_course(key, a.ship, move |game, _| compute_between(&game, &a, &b, f as f64))
    }

    fn plan_intercept_async(&self, ship: ShipId, target: ShipId, matched: bool) -> ui::PlanPoll {
        if ship >= PLANNED_BASE {
            return ui::PlanPoll::Done(None);
        }
        self.poll_course(format!("icp|{ship}|{target}|{matched}"), ship, move |game, now| compute_intercept(&game, now, ship, target, matched))
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
            .collect();
        v.ships = group_fleets(self.game.as_ref()?, self.me, v.ships);
        v.ships.extend(self.view.as_ref()?.ships.iter().filter(|s| s.seen_at.is_some()).cloned());
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

    fn orbit_status(&self, body: BodyId) -> Option<String> {
        let g = self.game.as_ref()?;
        let b = SimBody(body);
        let here: Vec<&g::Ship> = g.ships.iter().filter(|s| s.parked_at() == Some(b)).collect();
        let mut owners: Vec<g::PlayerId> = here.iter().map(|s| s.owner).collect();
        owners.sort();
        owners.dedup();
        if owners.len() < 2 {
            return None;
        }
        let armed: Vec<g::PlayerId> = here.iter().filter(|s| s.class.stats().attack > 0.0).map(|s| s.owner).collect();
        let name = |p: g::PlayerId| g.players.get(p as usize).map(|p| p.name.clone()).unwrap_or_else(|| "?".into());
        Some(if armed.is_empty() {
            "Standoff: no armed ships here, so nobody can shoot or take it".into()
        } else if g.bodies.get(body as usize).is_some_and(|s| s.in_combat) {
            let mut sides: Vec<String> = owners.iter().map(|&p| name(p)).collect();
            sides.sort();
            format!("Battle: {}", sides.join(" vs "))
        } else {
            "Ships of several players in orbit".into()
        })
    }

    fn results(&self) -> Option<m::GameResults> {
        let g = self.game.as_ref().filter(|g| g.status == Status::Ended)?;
        let mut scores: Vec<(m::PlayerId, u32)> = g.players.iter().map(|p| (p.id, g.score(p.id))).collect();
        scores.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        Some(m::GameResults { winner: g.winner, scores, ended_at: g.end_t })
    }

    fn construction_done(&self, body: BodyId) -> Option<f64> {
        let g = self.game.as_ref()?;
        g.stations
            .iter()
            .filter(|st| st.owner == self.me && st.body == SimBody(body) && st.building())
            .map(|st| st.done_t)
            .fold(None, |a: Option<f64>, t| Some(a.map_or(t, |x| x.max(t))))
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
        let waiting = g.players.get(self.me as usize).map(|p| p.waiting.as_slice()).unwrap_or(&[]);
        // upgrades already queued or waiting for funds raise the level the next one buys
        let pending = |u: g::Upgrade| {
            let w = Wish::Upgrade { body: b, upgrade: u };
            (st.queue.iter().filter(|j| matches!(j.kind, g::JobKind::Upgrade(x) if x == u)).count() + waiting.iter().filter(|x| x.wish == w).count()) as u8
        };
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
        // ships can be built at your own bodies and wherever you have a shipyard station (e.g. an asteroid)
        let buildable = if mine || g.station_yard_level(self.me, b) > 0 {
            g::ShipClass::ALL
                .iter()
                .filter(|c| g.yard_level(self.me, b) >= c.stats().min_shipyard)
                .filter(|c| **c != g::ShipClass::Slug && c.needs().is_none_or(|t| g.has_tech(self.me, t)))
                .map(|&c| {
                    let s = c.stats();
                    (ship_class(c), m::Cost { matter: s.matter, energy: s.energy, seconds: s.build_time })
                })
                .collect()
        } else {
            vec![]
        };
        // each lane (ships / upgrades) runs its jobs back to back; the first one is running now
        let now = g.t;
        let yard_speed = (1.0 + 0.5 * (g.yard_level(self.me, b).max(1) as f64 - 1.0)) * g.mods(self.me).build_speed;
        let mut ups_before: HashMap<usize, u8> = HashMap::new();
        let queue = st
            .queue
            .iter()
            .enumerate()
            .map(|(i, j)| {
                let lane = j.lane();
                let prev = st.queue[..i].iter().rev().find(|p| p.lane() == lane).map(|p| p.done_t);
                let (label, ship, dur) = match &j.kind {
                    g::JobKind::Ship(c) => (c.stats().name.to_string(), Some(ship_class(*c)), c.stats().build_time / yard_speed),
                    g::JobKind::Upgrade(u) => {
                        let k = ups_before.entry(u.index()).or_insert(0);
                        *k += 1;
                        (u.label().to_string(), None, u.cost(st.level(*u) + *k).2)
                    }
                };
                let start_t = prev.unwrap_or((j.done_t - dur).min(now));
                m::QueueItem { label, ship, start_t, done_t: j.done_t }
            })
            .collect();
        let (matter_rate, energy_rate, fuel_rate) = if mine { g.production(b) } else { (0.0, 0.0, 0.0) };
        let etas = g.waiting_eta(self.me);
        let waiting = waiting
            .iter()
            .zip(&etas)
            .enumerate()
            .filter(|(_, (w, _))| w.wish.body() == b)
            .map(|(index, (w, e))| m::WaitingItem {
                id: w.id,
                label: w.wish.label(),
                cost: m::Cost { matter: e.matter, energy: e.energy, seconds: 0.0 },
                start_t: e.start_t,
                blocked: e.blocked.clone(),
                index,
                of: etas.len(),
            })
            .collect();
        Some(m::BodyInfo { upgrades, buildable, queue, waiting, fuel: st.fuel, power: g.power_at(b), matter_rate, energy_rate, fuel_rate })
    }

    fn research(&self) -> Option<m::ResearchView> {
        let g = self.game.as_ref()?;
        let me = self.me;
        let p = g.players.get(me as usize)?;
        let labs: Vec<m::LabInfo> = (0..g.bodies.len())
            .filter(|&i| g.bodies[i].owner == Some(me) && g.bodies[i].level(g::Upgrade::Research) > 0)
            .map(|i| {
                let b = SimBody(i as u32);
                let busy = p.research.iter().filter(|r| r.body == b).map(|r| (r.tech.label().to_string(), r.done_t)).collect();
                m::LabInfo { body: i as u32, name: g.system.body(b).name.clone(), level: g.bodies[i].level(g::Upgrade::Research), busy }
            })
            .collect();
        // the advanced tier only exists in wars created with it
        let techs = Tech::every()
            .filter(|t| !t.advanced() || g.rules >= sim::tech::ADVANCED_RULES)
            .map(|t| {
                let state = if p.techs.contains(&t) {
                    m::TechState::Done
                } else if let Some(r) = p.research.iter().find(|r| r.tech == t) {
                    m::TechState::Running { lab: r.body.0, done_t: r.done_t }
                } else {
                    // the best lab with a free slot; if they're all full, the best busy one (it waits
                    // there for a slot); else the reason the first lab gives
                    let mut best: Option<(u8, u32)> = None;
                    let mut full: Option<(u8, u32)> = None;
                    let mut why = if labs.is_empty() { "needs a research lab".to_string() } else { String::new() };
                    for l in &labs {
                        match g.research_blocker(me, SimBody(l.body), t) {
                            None if best.is_none_or(|b| l.level > b.0) => best = Some((l.level, l.body)),
                            None => {}
                            Some(w) if w.contains("busy") => {
                                if full.is_none_or(|b| l.level > b.0) {
                                    full = Some((l.level, l.body));
                                }
                            }
                            Some(w) if why.is_empty() || w.starts_with("needs ") => why = w,
                            Some(_) => {}
                        }
                    }
                    match best.or(full) {
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
                    advanced: t.advanced(),
                    state,
                }
            })
            .collect();
        debug_assert!(Branch::ALL.iter().zip(m::TECH_BRANCHES).all(|(b, n)| b.label() == n));
        debug_assert!(Branch::ADVANCED.iter().zip(m::ADV_BRANCHES).all(|(b, n)| b.label() == n));
        Some(m::ResearchView { techs, labs })
    }

    fn sensors(&self) -> Option<&m::SensorsReport> {
        self.sensors.as_ref()
    }

    fn ship_abilities(&self, ship: ShipId) -> Vec<m::Ability> {
        let Some(g) = self.game.as_ref() else { return vec![] };
        let me = self.me;
        let ships: Vec<&g::Ship> = real_ships(g, &[ship]).into_iter().filter_map(|id| g.ship(id)).filter(|s| s.owner == me).collect();
        let mut out = vec![];
        if ships.is_empty() || g.rules < sim::tech::ADVANCED_RULES {
            return out;
        }
        let t = g.t;
        if g.has_tech(me, Tech::HeatSinkCloaks) {
            let dark = ships.iter().filter(|s| s.dark(t)).count();
            if dark > 0 {
                let left = ships.iter().filter_map(|s| s.cloak).filter(|c| c.dark(t)).map(|c| c.dark_until - t).fold(f64::INFINITY, f64::min);
                out.push(m::Ability { key: "cloak_off".into(), label: "DECLOAK".into(), on: true, blocker: None, note: format!("dark for {} · first strike on decloak", ui::theme::fmt_dur(left)) });
            } else {
                let blocker = ships.iter().find_map(|s| g.cloak_blocker(me, s));
                let note = format!("{:.0} energy · {} heat sink", sim::cloak::CLOAK_ENERGY * ships.len() as f64, ui::theme::fmt_dur(sim::cloak::DARK_TIME * g.mods(me).cloak_time));
                out.push(m::Ability { key: "cloak_on".into(), label: "GO DARK".into(), on: false, blocker, note });
            }
        }
        // construction ships seed minefields where they're parked
        if g.has_tech(me, Tech::Minefields) {
            if let Some(b) = ships.iter().find(|s| s.class == g::ShipClass::Constructor).and_then(|s| s.parked_at()) {
                let n = g.mines_at(me, b);
                let (mm, me_) = sim::bastion::MINE_COST;
                let note = if n > 0 { format!("our field here: {n}/{} charges", sim::bastion::MINE_CHARGES) } else { format!("{mm:.0}M {me_:.0}E · {} charges", sim::bastion::MINE_CHARGES) };
                out.push(m::Ability { key: "mines".into(), label: if n > 0 { "RESTOCK MINES".into() } else { "LAY MINES".into() }, on: n > 0, blocker: g.mines_blocker(me, b), note });
            }
        }
        if let [s] = ships.as_slice() {
            if s.class == g::ShipClass::Decoy {
                let look = sim::cloak::apparent_class(s).stats().name.to_uppercase();
                out.push(m::Ability { key: "decoy".into(), label: format!("LOOKS LIKE {look}"), on: false, blocker: None, note: "tap to change what enemies see".into() });
            }
        }
        out
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
            // advanced-tier stations only show once researched
            .filter(|k| k.needs().is_none_or(|t| !t.advanced() || g.has_tech(me, t)))
            .map(|&k| {
                let (matter, energy, seconds) = k.cost();
                m::StationOption {
                    kind: format!("{k:?}"),
                    name: k.label().to_string(),
                    desc: k.describe().to_string(),
                    cost: m::Cost { matter, energy, seconds },
                    blocker: g.station_blocker(me, b, k),
                    queueable: g.wish_obsolete(me, Wish::Station { body: b, kind: k }, 0).is_none(),
                }
            })
            .collect();
        let mut actions = vec![];
        if let Some(sc) = g.stations.iter().find(|st| st.owner == me && st.body == b && st.kind == StationKind::Scanner && st.active(g.t)) {
            if g.has_tech(me, Tech::LidarSweep) {
                let sweeping = g.sweeping(sc.id);
                let note = if sweeping { "sweeping: everything in 3× range shows".to_string() } else { "3× range for 1 h, sees cloaks · lights us up".to_string() };
                actions.push(m::Ability { key: "lidar".into(), label: "LIDAR SWEEP".into(), on: sweeping, blocker: g.sweep_blocker(me, sc.id).filter(|_| !sweeping), note });
            }
        }
        if g.stations.iter().any(|st| st.owner == me && st.body == b && st.kind == StationKind::MassDriver && st.active(g.t)) {
            let (mm, ee) = sim::ordnance::SLUG_COST;
            actions.push(m::Ability {
                key: "slug".into(),
                label: "LOAD SLUG".into(),
                on: false,
                blocker: g.slug_blocker(me, b),
                note: format!("{mm:.0}M {ee:.0}E · then tap it and pick a target body"),
            });
        }
        Some(m::StationsView { here, options, present, constructors: g.constructors_at(me, b), workers, actions })
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
        let s = g.ship(lead(g, ship))?;
        let ShipState::Flying(f) = &s.state else { return None };
        let mut burns: Vec<Burn> = nodes.iter().map(|n| Burn { t: n.t, prograde: n.prograde, radial: n.radial }).collect();
        burns.sort_by(|a, b| a.t.total_cmp(&b.t));
        let tr = sim::traj::predict(&g.system, f.start, &burns, f.dest, f.start.t + g::Flight::HORIZON);
        Some(to_model_traj(&g.system, &tr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fleets_show_as_one_token_that_maps_back_to_its_ships() {
        let settings = g::Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 86_400.0, speed: 1.0 };
        let mut game = Game::new("FLT2".into(), settings, 5, "ann");
        game.add_player("bob", None).unwrap();
        game.start();
        let gs: Vec<g::ShipId> = game.ships.iter().filter(|s| s.owner == 0 && s.class == g::ShipClass::Gunship).map(|s| s.id).collect();
        game.apply(0, Command::Fleet(sim::fleet::FleetCommand::Form { ships: gs.clone(), name: "Hammer".into() })).unwrap();
        let v = build_view(&game, 0, game.t, &HashMap::new());
        let tok = v.ships.iter().find(|s| s.is_fleet()).expect("fleet token");
        assert_eq!((tok.name.as_str(), tok.count, tok.members.len()), ("Hammer", gs.len() as u32, gs.len()));
        assert!(tok.id >= FLEET_TOKEN && tok.id < PLANNED_BASE);
        assert!(v.ships.iter().all(|s| gs.iter().all(|g| g.0 != s.id)), "members aren't drawn on their own");
        assert_eq!(lead(&game, tok.id), gs[0]);
        assert_eq!(real_ships(&game, &[tok.id]), gs);
        // the plan sheet compares the fleet's total need with the fleet's total fuel, and agrees
        // with the server's verdict (it used to compare the sum against one ship's tank)
        let home = game.players[0].home.unwrap();
        let target = game.system.bodies.iter().find(|b| b.kind == SimKind::Planet && b.id != home && game.bodies[b.id.0 as usize].owner.is_none()).unwrap().id;
        let plan = compute_plan(&game, game.t, tok.id, target.0, game.t).expect("fleet plan");
        let total: f64 = gs.iter().map(|&id| game.ship(id).unwrap().fuel).sum();
        assert!((plan.fuel_have as f64 - total).abs() < 1.0);
        assert!(plan.fuel_needed > tok.fuel, "a fleet needs more than one tank");
        let server = game.clone().apply(0, Command::Transfer { ships: vec![gs[0]], to: target, depart_t: Some(plan.depart_t) });
        assert_eq!(plan.fuel_problem.is_none(), server.is_ok(), "{:?} vs {:?}", plan.fuel_problem, server);
        // other players see neither the fleet nor its name
        let bob = game.view_for(1);
        assert!(build_view(&bob, 1, game.t, &HashMap::new()).ships.iter().all(|s| !s.is_fleet()));
    }
}
