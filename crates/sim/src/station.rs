//! Space stations: built in orbit of a planet, moon or asteroid. Unlike planet upgrades they
//! don't need the body itself — a ship parked there is enough — so you can put a fuel depot on
//! an asteroid or a telescope next to an enemy world. Stations are separate targets in combat.
use crate::game::{EventKind, Game, PlayerId, ShipClass, Upgrade};
use crate::system::BodyId;
use crate::tech::Tech;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StationId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StationKind {
    Shipyard,
    FuelDepot,
    Telescope,
    Scanner,
    Defense,
    Relay,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Station {
    pub id: StationId,
    pub owner: PlayerId,
    pub kind: StationKind,
    /// in orbit of this body
    pub body: BodyId,
    pub hp: f64,
    /// fuel stock (fuel depots only)
    #[serde(default)]
    pub fuel: f64,
    /// when construction finished, or the estimated finish while building (INFINITY while paused)
    pub done_t: f64,
    /// construction work still to do, in single-worker seconds (0 = finished)
    #[serde(default)]
    pub work_left: f64,
}

impl Station {
    pub fn active(&self, t: f64) -> bool {
        t >= self.done_t && self.hp > 0.0
    }
    pub fn building(&self) -> bool {
        self.work_left > 0.0
    }
}

/// Most workers that can usefully crowd one construction site.
pub const MAX_WORKERS: usize = 6;

/// Fuel a depot holds, tonnes.
pub const DEPOT_CAP: f64 = 1500.0;
/// Fuel a depot refines per hour (from the body's matter, paid in energy like a refinery).
pub const DEPOT_RATE: f64 = 80.0;
/// Shipyard level a shipyard station counts as.
pub const STATION_YARD_LEVEL: u8 = 2;

impl StationKind {
    pub const ALL: [StationKind; 6] =
        [StationKind::Shipyard, StationKind::FuelDepot, StationKind::Telescope, StationKind::Scanner, StationKind::Defense, StationKind::Relay];

    pub fn label(self) -> &'static str {
        match self {
            StationKind::Shipyard => "Shipyard",
            StationKind::FuelDepot => "Fuel depot",
            StationKind::Telescope => "Telescope",
            StationKind::Scanner => "Scanning station",
            StationKind::Defense => "Defense platform",
            StationKind::Relay => "Sensor relay",
        }
    }
    pub fn describe(self) -> &'static str {
        match self {
            StationKind::Shipyard => "Builds ships here, even if you don't hold the body. Counts as shipyard level 2.",
            StationKind::FuelDepot => "Refines and stores fuel here and refuels your ships, even on asteroids.",
            StationKind::Telescope => "Long-range passive sensor.",
            StationKind::Scanner => "Short range, but sees every ship in range and reads enemy flight plans.",
            StationKind::Defense => "Armed platform. Fights for the orbit and counts toward control.",
            StationKind::Relay => "Your other sensors within its range reach 50% farther.",
        }
    }
    /// (matter, energy, build seconds for one worker)
    pub fn cost(self) -> (f64, f64, f64) {
        match self {
            StationKind::Shipyard => (300.0, 150.0, 4.0 * 3600.0),
            StationKind::FuelDepot => (150.0, 80.0, 2.0 * 3600.0),
            StationKind::Telescope => (120.0, 120.0, 2.0 * 3600.0),
            StationKind::Scanner => (200.0, 200.0, 3.0 * 3600.0),
            StationKind::Defense => (250.0, 150.0, 3.0 * 3600.0),
            StationKind::Relay => (150.0, 150.0, 2.0 * 3600.0),
        }
    }
    pub fn max_hp(self) -> f64 {
        match self {
            StationKind::Defense => 400.0,
            StationKind::Shipyard => 300.0,
            _ => 150.0,
        }
    }
    /// Damage per hour (armed stations only).
    pub fn attack(self) -> f64 {
        match self {
            StationKind::Defense => 60.0,
            _ => 0.0,
        }
    }
    pub fn needs(self) -> Option<Tech> {
        match self {
            StationKind::Scanner | StationKind::Relay => Some(Tech::ScanningStations),
            StationKind::Defense => Some(Tech::DefensePlatforms),
            _ => None,
        }
    }
}

impl Game {
    pub fn stations_at(&self, body: BodyId) -> impl Iterator<Item = &Station> {
        let t = self.t;
        self.stations.iter().filter(move |s| s.body == body && s.active(t))
    }

    /// An active station of this kind owned by `p` at `body`.
    pub fn has_station(&self, p: PlayerId, body: BodyId, kind: StationKind) -> bool {
        self.stations_at(body).any(|s| s.owner == p && s.kind == kind)
    }

    /// Why `p` can't build a `kind` station at `body` right now (None = it can).
    pub fn station_blocker(&self, p: PlayerId, body: BodyId, kind: StationKind) -> Option<String> {
        let b = self.bodies.get(body.0 as usize)?;
        if self.system.body(body).is_star() {
            return Some("can't build in orbit of the star".into());
        }
        if let Some(t) = kind.needs() {
            if !self.has_tech(p, t) {
                return Some(format!("needs {}", t.label()));
            }
        }
        if self.workers(p, body) == 0 {
            return Some(if b.owner == Some(p) { "needs a shipyard or a construction ship here" } else { "needs a construction ship in orbit" }.into());
        }
        if self.stations.iter().any(|s| s.owner == p && s.body == body && s.kind == kind) {
            return Some("you already have one there".into());
        }
        None
    }

    pub(crate) fn build_station(&mut self, p: PlayerId, body: BodyId, kind: StationKind) -> Result<StationId, String> {
        if let Some(why) = self.station_blocker(p, body, kind) {
            return Err(why);
        }
        let (m, e, secs) = kind.cost();
        self.pay(p, m, e)?;
        let id = StationId(self.next_station);
        self.next_station += 1;
        let rate = self.work_rate(p, body);
        self.stations.push(Station { id, owner: p, kind, body, hp: kind.max_hp(), fuel: 0.0, done_t: self.t + secs / rate, work_left: secs });
        Ok(id)
    }

    /// Construction ships `p` has parked at `body`.
    pub fn constructors_at(&self, p: PlayerId, body: BodyId) -> usize {
        self.ships.iter().filter(|s| s.owner == p && s.class == ShipClass::Constructor && s.parked_at() == Some(body)).count()
    }

    /// Workers on `p`'s construction sites at `body`: constructors parked there, plus the body's own
    /// shipyard if `p` holds it. Capped at `MAX_WORKERS`.
    pub fn workers(&self, p: PlayerId, body: BodyId) -> usize {
        let b = &self.bodies[body.0 as usize];
        let yard = (b.owner == Some(p) && b.level(Upgrade::Shipyard) > 0) as usize;
        (self.constructors_at(p, body) + yard).min(MAX_WORKERS)
    }

    /// Work done per second at `p`'s sites at `body` (each site there gets the full crew).
    pub fn work_rate(&self, p: PlayerId, body: BodyId) -> f64 {
        self.workers(p, body) as f64 * self.mods(p).build_speed
    }

    /// Advance construction sites; returns stations that finished this tick.
    fn construction(&mut self, dt: f64) {
        for i in 0..self.stations.len() {
            let st = &self.stations[i];
            if !st.building() {
                continue;
            }
            let rate = self.work_rate(st.owner, st.body);
            let st = &mut self.stations[i];
            st.work_left -= rate * dt;
            if st.work_left <= 0.0 {
                // finished part-way through the tick
                st.done_t = self.t + st.work_left / rate;
                st.work_left = 0.0;
                let st = st.clone();
                let bname = self.system.body(st.body).name.clone();
                self.event(Some(st.owner), EventKind::Good, "Station built", &format!("{} online at {bname}", st.kind.label()), Some(st.body), None);
            } else {
                st.done_t = if rate > 0.0 { self.t + st.work_left / rate } else { f64::INFINITY };
            }
        }
    }

    /// Per-tick station upkeep: construction events, depots refine and refuel, platforms repair.
    pub(crate) fn stations_tick(&mut self, dt: f64) {
        let h = dt / 3600.0;
        self.construction(dt);
        for i in 0..self.stations.len() {
            let st = self.stations[i].clone();
            if !st.active(self.t) {
                continue;
            }
            self.stations[i].hp = (st.hp + st.kind.max_hp() * 0.05 * h).min(st.kind.max_hp());
            if st.kind == StationKind::FuelDepot {
                let mods = self.mods(st.owner);
                let p = &mut self.players[st.owner as usize];
                let want = (DEPOT_RATE * mods.fuel * h).min(DEPOT_CAP - st.fuel).max(0.0);
                let made = want.min(p.energy / 0.5);
                p.energy -= made * 0.5;
                self.stations[i].fuel += made;
                for s in self.ships.iter_mut().filter(|s| s.owner == st.owner && s.parked_at() == Some(st.body)) {
                    let room = (s.stats().fuel_cap - s.fuel).max(0.0);
                    let take = room.min(self.stations[i].fuel).min(200.0 * h);
                    s.fuel += take;
                    self.stations[i].fuel -= take;
                }
            }
        }
    }

    /// Shipyard level `p` gets at `body` from a shipyard station (0 if none).
    pub fn station_yard_level(&self, p: PlayerId, body: BodyId) -> u8 {
        if self.has_station(p, body, StationKind::Shipyard) {
            STATION_YARD_LEVEL
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Command, ShipClass, Upgrade};
    use crate::tech::tests::game;

    fn park(g: &mut Game, owner: PlayerId, class: ShipClass, body: BodyId) -> usize {
        g.spawn(owner, class, body);
        g.ships.len() - 1
    }

    #[test]
    fn stations_need_workers_tech_and_build_time() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        let away = g.players[1].home.unwrap();
        let b = |g: &mut Game, body, kind| g.apply(0, Command::BuildStation { body, kind });
        assert!(b(&mut g, away, StationKind::Telescope).unwrap_err().contains("construction ship"));
        assert!(b(&mut g, home, StationKind::Scanner).unwrap_err().contains("Scanning"));
        // home has a shipyard: one worker, so the listed single-worker time
        b(&mut g, home, StationKind::Telescope).unwrap();
        assert!(b(&mut g, home, StationKind::Telescope).unwrap_err().contains("already"));
        // a warship parked at someone else's body isn't enough any more; a constructor is
        let scout = g.ships.iter().position(|s| s.owner == 0 && s.class == ShipClass::Scout).unwrap();
        g.ships[scout].state = crate::game::ShipState::Parked { body: away, departure: None };
        assert!(b(&mut g, away, StationKind::FuelDepot).is_err());
        park(&mut g, 0, ShipClass::Constructor, away);
        b(&mut g, away, StationKind::FuelDepot).unwrap();
        assert!(!g.has_station(0, home, StationKind::Telescope));
        g.advance_to(2.0 * 3600.0 + 60.0);
        assert!(g.has_station(0, home, StationKind::Telescope));
        assert!(g.events.iter().any(|e| e.title == "Station built"));
    }

    #[test]
    fn more_constructors_build_faster_and_none_pauses() {
        let mut g = game();
        let ast = g.system.bodies.iter().find(|b| b.kind == crate::system::BodyKind::Asteroid).map(|b| b.id).unwrap();
        let (_, _, secs) = StationKind::Shipyard.cost();
        let c1 = park(&mut g, 0, ShipClass::Constructor, ast);
        for _ in 0..3 {
            park(&mut g, 0, ShipClass::Constructor, ast);
        }
        assert_eq!(g.workers(0, ast), 4);
        g.apply(0, Command::BuildStation { body: ast, kind: StationKind::Shipyard }).unwrap();
        let st = g.stations.last().unwrap();
        assert!((st.done_t - (g.t + secs / 4.0)).abs() < 1.0, "4 workers → a quarter of the time");
        // half done, then everyone leaves: construction pauses
        g.advance_to(secs / 8.0);
        let ids: Vec<_> = g.ships.iter().filter(|s| s.class == ShipClass::Constructor).map(|s| s.id).collect();
        g.ships.retain(|s| !ids.contains(&s.id));
        g.advance_to(secs);
        let st = g.stations.last().unwrap();
        assert!(st.building() && st.done_t.is_infinite(), "{st:?}");
        assert!((st.work_left - secs / 2.0).abs() < 4.0 * 60.0 + 1.0, "{}", st.work_left);
        // one constructor comes back and finishes the other half alone
        let _ = c1;
        park(&mut g, 0, ShipClass::Constructor, ast);
        g.advance_to(secs + secs / 2.0 + 120.0);
        assert!(g.has_station(0, ast, StationKind::Shipyard));
        assert_eq!(g.workers(0, ast), 1);
    }

    #[test]
    fn workers_are_capped() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        for _ in 0..10 {
            park(&mut g, 0, ShipClass::Constructor, home);
        }
        assert_eq!(g.workers(0, home), MAX_WORKERS);
        assert_eq!(g.workers(1, home), 0, "bob has nothing at alice's home");
    }

    #[test]
    fn shipyard_station_builds_at_a_body_you_dont_own() {
        let mut g = game();
        let away = g.players[1].home.unwrap();
        g.stations.push(Station { id: StationId(0), owner: 0, kind: StationKind::Shipyard, body: away, hp: 300.0, fuel: 0.0, done_t: 0.0 , work_left: 0.0 });
        assert_eq!(g.yard_level(0, away), STATION_YARD_LEVEL);
        g.apply(0, Command::Build { body: away, class: ShipClass::Dreadnought }).unwrap();
        g.advance_to(4.0 * 3600.0);
        assert!(g.events.iter().any(|e| e.to == Some(0) && e.title == "Ship built" && e.body.starts_with("Dreadnought")));
        // can't upgrade a body you don't own through a station
        assert!(g.apply(0, Command::Upgrade { body: away, upgrade: Upgrade::Mining }).is_err());
    }

    #[test]
    fn defense_platform_counts_as_power_and_depot_refuels() {
        let mut g = game();
        let home = g.players[0].home.unwrap();
        g.stations.push(Station { id: StationId(0), owner: 0, kind: StationKind::Defense, body: home, hp: 400.0, fuel: 0.0, done_t: 0.0 , work_left: 0.0 });
        let before = g.power_at(home).iter().find(|p| p.0 == 0).unwrap().1;
        g.stations.push(Station { id: StationId(1), owner: 0, kind: StationKind::Defense, body: home, hp: 400.0, fuel: 0.0, done_t: 1e9 , work_left: 0.0 });
        assert!(g.power_at(home).iter().find(|p| p.0 == 0).unwrap().1 == before, "unfinished stations don't count");
        g.stations.clear();
        let base = g.power_at(home).iter().find(|p| p.0 == 0).unwrap().1;
        assert!((before - base - StationKind::Defense.attack()).abs() < 1e-9);

        let ast = g.system.bodies.iter().find(|b| b.kind == crate::system::BodyKind::Asteroid).map(|b| b.id);
        let Some(ast) = ast else { return };
        let i = g.ships.iter().position(|s| s.owner == 0 && s.class == ShipClass::Gunship).unwrap();
        g.ships[i].state = crate::game::ShipState::Parked { body: ast, departure: None };
        g.ships[i].fuel = 10.0;
        g.stations.push(Station { id: StationId(2), owner: 0, kind: StationKind::FuelDepot, body: ast, hp: 150.0, fuel: 0.0, done_t: 0.0 , work_left: 0.0 });
        g.advance_to(3.0 * 3600.0);
        assert!(g.ships[i].fuel > 10.0 + 100.0, "fuel {}", g.ships[i].fuel);
    }
}
