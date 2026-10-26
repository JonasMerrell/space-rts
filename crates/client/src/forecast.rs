//! Time-scrubber forecast: the real sim run forward on a worker thread (in the browser: a few ms
//! of every frame) from the player's own
//! (fog-filtered) view, without bots, recording a keyframe whenever something changes: a ship is
//! built, departs, burns, arrives or dies, or a body changes hands. Positions between keyframes
//! come from the recorded trajectories, so a frame stays valid until the next one.
use crate::backend::to_model_traj;
use sim::game::{self as g, Game, ShipState, Status};
use sim::{BodyId as SimBody, System};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use ui::model as m;

/// Ships that only exist in the forecast (still in a build queue) get ids from here up.
pub const PLANNED_BASE: u32 = 0x8000_0000;
/// How far ahead we simulate.
pub const HORIZON: f64 = 7.0 * 86_400.0;
/// Record a frame at least this often so stockpiles keep counting up.
const MAX_GAP: f64 = 3600.0;

#[derive(Clone, Debug)]
pub struct FShip {
    pub model: m::Ship,
    /// departure time of a scheduled departure (the path starts there)
    pub depart_t: Option<f64>,
    pub path: Option<Arc<m::Trajectory>>,
    /// for forecast-born ships: (shipyard body, job done_t)
    pub job: Option<(u32, f64)>,
}

#[derive(Debug)]
pub struct Frame {
    pub t: f64,
    pub ships: Vec<FShip>,
    pub owners: Vec<Option<g::PlayerId>>,
    pub matter: f64,
    pub energy: f64,
}

impl Frame {
    pub fn ship(&self, id: u32) -> Option<&FShip> {
        self.ships.iter().find(|s| s.model.id == id)
    }
}

#[derive(Default)]
pub struct Run {
    pub gen: u64,
    pub frames: Vec<Arc<Frame>>,
    /// simulated up to here
    pub until: f64,
    pub done: bool,
}

impl Run {
    /// The frame in effect at `t` (the last one at or before it).
    pub fn at(&self, t: f64) -> Option<(usize, &Arc<Frame>)> {
        if t > self.until + 1.0 {
            return None;
        }
        let i = self.frames.partition_point(|f| f.t <= t);
        i.checked_sub(1).map(|i| (i, &self.frames[i]))
    }
}

/// The run being computed plus the previous one, which answers for times the new run hasn't
/// reached yet (so the far end of the scrubber doesn't blink on every server snapshot).
#[derive(Default)]
pub struct Shared {
    pub cur: Run,
    pub prev: Run,
}

impl Shared {
    pub fn at(&self, t: f64) -> Option<(u64, usize, Arc<Frame>)> {
        for r in [&self.cur, &self.prev] {
            if let Some((i, f)) = r.at(t) {
                return Some((r.gen, i, f.clone()));
            }
        }
        None
    }
    pub fn until(&self) -> f64 {
        if self.cur.done || self.prev.frames.is_empty() {
            self.cur.until
        } else {
            self.cur.until.max(self.prev.until)
        }
    }
}

pub struct Forecaster {
    pub shared: Arc<Mutex<Shared>>,
    gen: Arc<AtomicU64>,
    /// the run in progress (browser build: stepped by `pump` each frame)
    #[cfg(target_arch = "wasm32")]
    work: std::cell::RefCell<Option<Work>>,
}

impl Default for Forecaster {
    fn default() -> Self {
        Forecaster {
            shared: Arc::new(Mutex::new(Shared::default())),
            gen: Arc::new(AtomicU64::new(0)),
            #[cfg(target_arch = "wasm32")]
            work: Default::default(),
        }
    }
}

impl Forecaster {
    /// Start over from `game` (the player's view) at game time `now`; cancels the running worker.
    pub fn restart(&self, game: &Game, me: g::PlayerId, now: f64) {
        let gen = self.gen.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut sh = self.shared.lock().unwrap();
            let old = std::mem::take(&mut sh.cur);
            if old.done || old.until > sh.prev.until {
                sh.prev = old;
            }
            sh.cur = Run { gen, until: now, ..Default::default() };
        }
        if game.status != Status::Running {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (game, shared, cur_gen) = (game.clone(), self.shared.clone(), self.gen.clone());
            std::thread::spawn(move || {
                let mut w = Work::new(game, me, now, gen);
                while w.step(&cur_gen, &shared) {}
            });
        }
        #[cfg(target_arch = "wasm32")]
        {
            *self.work.borrow_mut() = Some(Work::new(game.clone(), me, now, gen));
        }
    }

    /// Browser build: advance the forecast for up to `budget` seconds of this frame.
    #[cfg(target_arch = "wasm32")]
    pub fn pump(&self, budget: f64) {
        let mut slot = self.work.borrow_mut();
        let Some(w) = slot.as_mut() else { return };
        let t0 = macroquad::miniquad::date::now();
        while macroquad::miniquad::date::now() - t0 < budget {
            if !w.step(&self.gen, &self.shared) {
                *slot = None;
                return;
            }
        }
    }

    pub fn clear(&self) {
        self.gen.fetch_add(1, Ordering::SeqCst);
        *self.shared.lock().unwrap() = Shared::default();
    }
}

/// One forecast run, advanced a tick at a time so the browser build (no threads) can spread it
/// over frames; natively a worker thread just steps it to the end.
struct Work {
    game: Game,
    me: g::PlayerId,
    gen: u64,
    end: f64,
    paths: HashMap<(u32, u64, usize), Arc<m::Trajectory>>,
    jobs: HashMap<u32, (u32, f64)>,
    batch: Vec<Arc<Frame>>,
    sig: u64,
    last_t: f64,
    flushed: f64,
}

impl Work {
    fn new(mut game: Game, me: g::PlayerId, now: f64, gen: u64) -> Work {
        let end = (now + HORIZON).min(game.end_t);
        game.next_ship = game.next_ship.max(PLANNED_BASE);
        game.forecast_to(now);
        let mut paths = HashMap::new();
        let jobs = HashMap::new();
        let batch = vec![frame(&mut game, me, &mut paths, &jobs)];
        let sig = signature(&game);
        let t = game.t;
        Work { game, me, gen, end, paths, jobs, batch, sig, last_t: t, flushed: t }
    }

    /// Simulate one tick; false once the run is finished or superseded.
    fn step(&mut self, cur_gen: &AtomicU64, shared: &Mutex<Shared>) -> bool {
        let (game, end) = (&mut self.game, self.end);
        let alive = cur_gen.load(Ordering::Relaxed) == self.gen;
        let finished = game.t >= end || game.status != Status::Running;
        if !alive {
            return false;
        }
        if finished || game.t - self.flushed > 6.0 * 3600.0 {
            let mut sh = shared.lock().unwrap();
            if sh.cur.gen != self.gen {
                return false;
            }
            sh.cur.frames.append(&mut self.batch);
            sh.cur.until = if finished { f64::INFINITY } else { game.t };
            sh.cur.done = finished;
            self.flushed = game.t;
            if finished {
                return false;
            }
        }
        // build jobs that finish this tick, so the new ships can be traced back to them
        let next = (game.last_tick + g::TICK).min(end);
        let due: Vec<(u32, f64, g::ShipClass, g::PlayerId)> = game
            .bodies
            .iter()
            .enumerate()
            .flat_map(|(i, b)| {
                b.queue.iter().filter(|j| j.done_t <= next).filter_map(move |j| match j.kind {
                    g::JobKind::Ship(c) => Some((i as u32, j.done_t, c, j.owner)),
                    _ => None,
                })
            })
            .collect();
        let before = game.next_ship;
        game.forecast_to(next);
        let jobs = &mut self.jobs;
        for s in game.ships.iter().filter(|s| s.id.0 >= before) {
            if let Some(&(b, t, ..)) = due.iter().find(|d| d.2 == s.class && d.3 == s.owner && !jobs.values().any(|j| *j == (d.0, d.1))) {
                jobs.insert(s.id.0, (b, t));
            }
        }
        let s2 = signature(game);
        if s2 != self.sig || game.t - self.last_t >= MAX_GAP {
            self.sig = s2;
            self.last_t = game.t;
            self.batch.push(frame(game, self.me, &mut self.paths, &self.jobs));
        }
        true
    }
}

/// Changes whenever a frame would look different (apart from positions along known paths).
fn signature(game: &Game) -> u64 {
    let mut h = DefaultHasher::new();
    for s in &game.ships {
        s.id.0.hash(&mut h);
        s.owner.hash(&mut h);
        match &s.state {
            ShipState::Parked { body, departure } => {
                (0u8, body.0).hash(&mut h);
                departure.as_ref().map(|d| d.state.t.to_bits()).hash(&mut h);
            }
            ShipState::Flying(f) => {
                (1u8, f.start.t.to_bits(), f.burns.len(), f.dest.map(|d| d.0)).hash(&mut h);
            }
        }
    }
    for b in &game.bodies {
        b.owner.hash(&mut h);
        b.queue.len().hash(&mut h);
    }
    h.finish()
}

fn frame(game: &mut Game, me: g::PlayerId, paths: &mut HashMap<(u32, u64, usize), Arc<m::Trajectory>>, jobs: &HashMap<u32, (u32, f64)>) -> Arc<Frame> {
    let sys: System = game.system.clone();
    let ships = game
        .ships
        .iter_mut()
        .map(|s| {
            let (path, depart_t) = match &mut s.state {
                ShipState::Flying(f) => {
                    let key = (s.id.0, f.start.t.to_bits(), f.burns.len());
                    let p = paths.entry(key).or_insert_with(|| Arc::new(to_model_traj(&sys, f.trajectory(&sys)))).clone();
                    (Some(p), None)
                }
                ShipState::Parked { departure: Some(d), .. } => {
                    let key = (s.id.0, d.state.t.to_bits(), usize::MAX);
                    let p = paths
                        .entry(key)
                        .or_insert_with(|| {
                            let tr = sim::traj::predict(&sys, d.state, &[], Some(d.to), d.state.t + g::Flight::HORIZON);
                            Arc::new(to_model_traj(&sys, &tr))
                        })
                        .clone();
                    (Some(p), Some(d.state.t))
                }
                ShipState::Parked { .. } => (None, None),
            };
            let mut model = crate::backend::ship_model(s, None);
            model.planned = jobs.get(&s.id.0).map(|j| j.1);
            FShip { model, depart_t, path, job: jobs.get(&s.id.0).copied() }
        })
        .collect();
    let (matter, energy) = game.players.get(me as usize).map(|p| (p.matter, p.energy)).unwrap_or_default();
    Arc::new(Frame { t: game.t, ships, owners: game.bodies.iter().map(|b| b.owner).collect(), matter, energy })
}

/// Where a ship from a frame is at `t`.
pub fn ship_pos(sys: &System, s: &FShip, t: f64) -> Option<[f64; 2]> {
    let arr = |v: sim::V2| [v.x, v.y];
    match (s.model.parked_at, &s.path) {
        (Some(_), Some(p)) if s.depart_t.is_some_and(|d| t >= d) => p.sample(t).or_else(|| p.points.last().map(|q| q.1)),
        (Some(b), _) => Some(arr(sys.pos(SimBody(b), t))),
        (None, Some(p)) => p.sample(t).or_else(|| if t < p.start()? { p.points.first().map(|q| q.1) } else { p.points.last().map(|q| q.1) }),
        (None, None) => None,
    }
}
