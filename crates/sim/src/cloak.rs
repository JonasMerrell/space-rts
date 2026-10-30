//! Heat-sink cloaks (advanced tier, Shadow branch). A hot ship can't be hidden, but it can
//! store its waste heat for a while: a dark ship's signature drops to `DARK_SIG` until its heat
//! sink fills, then it has to vent (signature × `VENT_SIG`) for as long as it was dark.
//! Any engine burn or weapon fire ends the dark spell at once. Dark ships don't count toward
//! orbit control, can't be shot by anyone whose sensors don't see them, and hold their fire;
//! a ship that decloaks on purpose gets a first-strike bonus for `STRIKE_TIME`.
//!
//! Decoy drones (also Shadow): an unarmed `ShipClass::Decoy` shows to enemy sensors as its
//! `mimic` class, signature and burn flares included, until a scanning station, the viewer's
//! Deep-field interferometry, or a fight (they die fast) gives it away.
//!
//! Counters (Sight branch): gravimetric arrays (fog.rs, `SensorKind::Gravimetric`) see mass, and
//! a lidar sweep turns a scanning station into a 3× range scanner for an hour, at the price of
//! lighting itself up for everyone it illuminates.
use crate::fog::{Sensor, LIDAR_COOLDOWN, LIDAR_RANGE, LIDAR_TIME};
use crate::game::{EventKind, Game, PlayerId, Ship, ShipClass, ShipId};
use crate::station::{StationId, StationKind};
use crate::system::BodyId;
use crate::tech::Tech;
use serde::{Deserialize, Serialize};

/// How long a heat sink lasts (× `Mods::cloak_time`).
pub const DARK_TIME: f64 = 12.0 * 3600.0;
/// Signature multiplier while dark.
/// Signature multiplier while dark: 100× fainter, so a dark gunship shows to a ship's sensors
/// only within ~2,000 km, but a planet with a telescope still sees its own orbit.
pub const DARK_SIG: f64 = 0.01;
/// Signature multiplier while venting (× `Mods::vent_sig`).
pub const VENT_SIG: f64 = 3.0;
/// First-strike window after a chosen decloak, and its damage multiplier.
pub const STRIKE_TIME: f64 = 3600.0;
pub const STRIKE_BONUS: f64 = 1.5;
/// Energy to chill a ship's heat sink, per ship.
pub const CLOAK_ENERGY: f64 = 30.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cloak {
    /// went dark at
    pub since: f64,
    /// dark until then (the heat sink fills), unless broken earlier
    pub dark_until: f64,
    /// venting until
    pub vent_until: f64,
    /// first-strike bonus until (after a chosen decloak)
    #[serde(default)]
    pub strike_until: f64,
}

impl Cloak {
    pub fn dark(&self, t: f64) -> bool {
        t >= self.since && t < self.dark_until
    }
    pub fn venting(&self, t: f64) -> bool {
        t >= self.dark_until && t < self.vent_until
    }
}

/// Dreadnoughts make too much heat to hide.
pub fn can_cloak(class: ShipClass) -> bool {
    !matches!(class, ShipClass::Dreadnought)
}

impl Ship {
    pub fn dark(&self, t: f64) -> bool {
        self.cloak.is_some_and(|c| c.dark(t))
    }
}

impl Game {
    /// Signature multiplier a ship's cloak applies now (`vent_sig` = the owner's research mod).
    pub fn cloak_sig(s: &Ship, t: f64, vent_sig: f64) -> f64 {
        match s.cloak {
            Some(c) if c.dark(t) => DARK_SIG,
            Some(c) if c.venting(t) => VENT_SIG * vent_sig,
            _ => 1.0,
        }
    }

    /// Damage multiplier from a first strike out of the dark.
    pub fn strike_mult(s: &Ship, t: f64) -> f64 {
        if s.cloak.is_some_and(|c| t < c.strike_until) {
            STRIKE_BONUS
        } else {
            1.0
        }
    }

    /// Why `s` can't go dark now (None = it can).
    pub fn cloak_blocker(&self, p: PlayerId, s: &Ship) -> Option<String> {
        if !self.has_tech(p, Tech::HeatSinkCloaks) {
            return Some(format!("needs {}", Tech::HeatSinkCloaks.label()));
        }
        if !can_cloak(s.class) {
            return Some(format!("a {} runs too hot to hide", s.class.stats().name));
        }
        match s.cloak {
            Some(c) if c.dark(self.t) => Some("already dark".into()),
            Some(c) if c.venting(self.t) => Some("still venting heat".into()),
            _ => None,
        }
    }

    /// `Command::Cloak`: take ships dark, or bring them out of the dark (with a first strike).
    pub(crate) fn set_cloak(&mut self, p: PlayerId, ships: &[ShipId], on: bool) -> Result<(), String> {
        let idx: Vec<usize> = ships.iter().map(|id| self.ships.iter().position(|s| s.id == *id && s.owner == p).ok_or("not your ship")).collect::<Result<_, _>>()?;
        let t = self.t;
        if on {
            for &i in &idx {
                if let Some(why) = self.cloak_blocker(p, &self.ships[i]) {
                    return Err(format!("{}: {why}", self.ships[i].name));
                }
            }
            self.pay(p, 0.0, CLOAK_ENERGY * idx.len() as f64)?;
            let len = DARK_TIME * self.mods(p).cloak_time;
            for i in idx {
                self.ships[i].cloak = Some(Cloak { since: t, dark_until: t + len, vent_until: t + len + len / self.mods(p).cloak_time, strike_until: 0.0 });
            }
        } else {
            let k = self.mods(p).cloak_time;
            for i in idx {
                if let Some(c) = self.ships[i].cloak.as_mut().filter(|c| c.dark(t)) {
                    end_dark(c, t, k, true);
                }
            }
        }
        Ok(())
    }

    /// Burns break the dark; full heat sinks start venting (once per tick).
    pub(crate) fn cloak_tick(&mut self) {
        let t = self.t;
        let mut news: Vec<(PlayerId, String, &'static str)> = vec![];
        for pi in 0..self.players.len() {
            let k = self.mods(pi as PlayerId).cloak_time;
            for s in self.ships.iter_mut().filter(|s| s.owner == pi as PlayerId) {
                let Some(c) = s.cloak.as_mut() else { continue };
                if c.dark(t) && s.last_burn_t >= c.since && s.last_burn_t <= t {
                    end_dark(c, t, k, false);
                    news.push((s.owner, s.name.clone(), "engine burn: out of the dark"));
                } else if c.dark_until > t - crate::game::TICK && c.dark_until <= t && c.since < c.dark_until {
                    news.push((s.owner, s.name.clone(), "heat sink full, venting"));
                }
                if t >= c.vent_until && t >= c.strike_until {
                    s.cloak = None;
                }
            }
        }
        for (o, name, what) in news {
            self.event(Some(o), EventKind::Info, "Cloak", &format!("{name}: {what}"), None, None);
        }
        self.sweeps_tick();
    }

    /// A dark ship that got into a fight (fired or was fired on) is dark no longer.
    pub(crate) fn break_dark(&mut self, ship: usize) {
        let t = self.t;
        let k = self.mods(self.ships[ship].owner).cloak_time;
        if let Some(c) = self.ships[ship].cloak.as_mut().filter(|c| c.dark(t)) {
            end_dark(c, t, k, false);
        }
    }
}

impl Game {
    /// `Command::SetDecoy`.
    pub(crate) fn set_decoy(&mut self, p: PlayerId, ship: ShipId, looks_like: ShipClass) -> Result<(), String> {
        if matches!(looks_like, ShipClass::Decoy) {
            return Err("pick a class to look like".into());
        }
        let s = self.ships.iter_mut().find(|s| s.id == ship && s.owner == p).ok_or("not your ship")?;
        if s.class != ShipClass::Decoy {
            return Err("only decoys can do that".into());
        }
        s.mimic = Some(looks_like);
        Ok(())
    }

    /// Does player `viewer` see through decoys without scanning them?
    pub fn sees_decoys(&self, viewer: PlayerId) -> bool {
        self.mods(viewer).see_decoys
    }
}

/// A lidar sweep from a scanning station.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sweep {
    pub station: StationId,
    pub owner: PlayerId,
    pub body: BodyId,
    /// sweeping until
    pub until: f64,
    /// the station can sweep again from
    pub ready: f64,
}

impl Game {
    /// Is this station sweeping right now?
    pub fn sweeping(&self, st: StationId) -> bool {
        self.sweeps.iter().any(|s| s.station == st && self.t < s.until)
    }

    /// Why `p` can't sweep from `station` now (None = it can).
    pub fn sweep_blocker(&self, p: PlayerId, station: StationId) -> Option<String> {
        let Some(st) = self.stations.iter().find(|s| s.id == station && s.owner == p) else { return Some("not your station".into()) };
        if st.kind != StationKind::Scanner || !st.active(self.t) {
            return Some("only a finished scanning station can sweep".into());
        }
        if !self.has_tech(p, Tech::LidarSweep) {
            return Some(format!("needs {}", Tech::LidarSweep.label()));
        }
        match self.sweeps.iter().find(|s| s.station == station) {
            Some(s) if self.t < s.until => Some("already sweeping".into()),
            Some(s) if self.t < s.ready => Some(format!("recharging for {:.0} min", (s.ready - self.t) / 60.0)),
            _ => None,
        }
    }

    /// `Command::LidarSweep`.
    pub(crate) fn lidar_sweep(&mut self, p: PlayerId, station: StationId) -> Result<(), String> {
        if let Some(why) = self.sweep_blocker(p, station) {
            return Err(why);
        }
        let body = self.stations.iter().find(|s| s.id == station).unwrap().body;
        let t = self.t;
        self.sweeps.retain(|s| s.station != station);
        self.sweeps.push(Sweep { station, owner: p, body, until: t + LIDAR_TIME, ready: t + LIDAR_TIME + LIDAR_COOLDOWN });
        let name = self.system.body(body).name.clone();
        self.event(Some(p), EventKind::Info, "Lidar sweep", &format!("Sweeping from {name} for an hour: everything in range shows, cloaked or not"), Some(body), None);
        // everyone in the beam sees where it came from
        let pos = self.system.pos(body, t);
        let reach = crate::fog::station_sensor(StationKind::Scanner).map(|r| r.0).unwrap_or(0.0) * LIDAR_RANGE * self.mods(p).sensor;
        for q in 0..self.players.len() as PlayerId {
            if q != p && self.sensors(q).iter().any(|s| s.pos.dist(pos) <= reach) {
                self.event(Some(q), EventKind::Bad, "Lidar ping", &format!("A lidar sweep from {name} is painting our ships"), Some(body), None);
            }
        }
        Ok(())
    }

    /// Bodies whose sweeping lidar (not `me`'s) illuminates any of `me`'s sensors.
    pub(crate) fn pinged_by_others(&self, me: PlayerId, mine: &[Sensor]) -> Vec<BodyId> {
        let t = self.t;
        let reach = crate::fog::station_sensor(StationKind::Scanner).map(|r| r.0).unwrap_or(0.0) * LIDAR_RANGE;
        self.sweeps
            .iter()
            .filter(|s| s.owner != me && t < s.until)
            .filter(|s| {
                let pos = self.system.pos(s.body, t);
                let r = reach * self.mods(s.owner).sensor;
                mine.iter().any(|x| x.pos.dist(pos) <= r)
            })
            .map(|s| s.body)
            .collect()
    }

    /// Forget sweeps that have recharged.
    pub(crate) fn sweeps_tick(&mut self) {
        let t = self.t;
        self.sweeps.retain(|s| t < s.ready);
    }
}

/// The class enemy sensors take this ship for.
pub fn apparent_class(s: &Ship) -> ShipClass {
    match (s.class, s.mimic) {
        (ShipClass::Decoy, Some(m)) => m,
        (c, _) => c,
    }
}

/// Leave the dark at t: vent for as long as the sink was filling (research shortens it with
/// `k` = cloak_time), and optionally open the first-strike window.
fn end_dark(c: &mut Cloak, t: f64, k: f64, strike: bool) {
    let used = (t - c.since).max(0.0);
    c.dark_until = t;
    c.vent_until = t + used / k.max(1.0);
    if strike {
        c.strike_until = t + STRIKE_TIME;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Command;

    fn cloaky() -> (Game, ShipId) {
        let mut g = crate::tech::tests::game();
        g.players[0].techs.push(Tech::HeatSinkCloaks);
        let id = g.ships.iter().find(|s| s.owner == 0 && can_cloak(s.class)).unwrap().id;
        (g, id)
    }

    #[test]
    fn dark_then_vent() {
        let (mut g, id) = cloaky();
        let sig0 = g.signatures()[g.ships.iter().position(|s| s.id == id).unwrap()];
        g.apply(0, Command::Cloak { ships: vec![id], on: true }).unwrap();
        let i = g.ships.iter().position(|s| s.id == id).unwrap();
        assert!(g.signatures()[i] / sig0 <= DARK_SIG + 1e-12);
        assert!(g.apply(0, Command::Cloak { ships: vec![id], on: true }).unwrap_err().contains("already dark"));
        g.advance_to(g.t + DARK_TIME + 120.0);
        let s = g.ship(id).unwrap();
        assert!(!s.dark(g.t) && s.cloak.unwrap().venting(g.t));
        assert!(g.apply(0, Command::Cloak { ships: vec![id], on: true }).unwrap_err().contains("venting"));
        g.advance_to(g.t + DARK_TIME);
        assert!(g.ship(id).unwrap().cloak.is_none());
    }

    #[test]
    fn decloak_strikes_and_needs_research() {
        let (mut g, id) = cloaky();
        assert!(g.apply(1, Command::Cloak { ships: vec![id], on: true }).is_err());
        g.players[0].techs.clear();
        assert!(g.apply(0, Command::Cloak { ships: vec![id], on: true }).unwrap_err().contains("Heat-sink"));
        g.players[0].techs.push(Tech::HeatSinkCloaks);
        g.apply(0, Command::Cloak { ships: vec![id], on: true }).unwrap();
        g.advance_to(g.t + 2.0 * 3600.0);
        g.apply(0, Command::Cloak { ships: vec![id], on: false }).unwrap();
        let s = g.ship(id).unwrap().clone();
        assert!(!s.dark(g.t));
        assert_eq!(Game::strike_mult(&s, g.t), STRIKE_BONUS);
        // vented for as long as it was dark
        assert!((s.cloak.unwrap().vent_until - g.t - 2.0 * 3600.0).abs() < 61.0);
    }

    #[test]
    fn decoys_fool_sensors_until_seen_through() {
        let (mut g, _) = cloaky();
        let home = g.players[0].home.unwrap();
        assert!(g.apply(0, Command::Build { body: home, class: ShipClass::Decoy }).unwrap_err().contains("Decoy"));
        g.players[0].techs.push(Tech::DecoyDrones);
        let d = g.spawn(0, ShipClass::Decoy, home);
        g.apply(0, Command::SetDecoy { ship: d, looks_like: ShipClass::Dreadnought }).unwrap();
        let s = g.ship(d).unwrap().clone();
        // as bright as a dreadnought, shown as one
        assert!(crate::fog::signature(&s, g.t) >= ShipClass::Dreadnought.stats().signature);
        let fooled = g.redact(&s, false);
        assert_eq!((fooled.class, fooled.mimic, fooled.hp), (ShipClass::Dreadnought, None, ShipClass::Dreadnought.stats().hp));
        assert_eq!(g.redact(&s, true).class, ShipClass::Decoy);
        assert!(g.power_at(home).iter().all(|&(p, x)| p != 0 || x > 0.0));
    }

    #[test]
    fn gravimetric_arrays_and_lidar_find_dark_ships() {
        let (mut g, _) = cloaky();
        let b = (0..g.bodies.len() as u32).map(crate::BodyId).find(|&b| g.bodies[b.0 as usize].owner.is_none() && !g.system.body(b).is_star()).unwrap();
        let a = g.spawn(0, ShipClass::Gunship, b);
        g.apply(0, Command::Cloak { ships: vec![a], on: true }).unwrap();
        // bob has nothing there (any ship of his would claim the body, and owners see their orbits)
        g.advance_to(g.t + 120.0);
        assert!(g.players[1].vision.ships.binary_search(&a).is_err());
        // bob's gravimetric array in that orbit: the gunship's mass gives it away
        let put = |g: &mut Game, kind| {
            let id = StationId(g.next_station);
            g.next_station += 1;
            g.stations.push(crate::station::Station { id, owner: 1, kind, body: b, hp: kind.max_hp(), fuel: 0.0, done_t: 0.0, work_left: 0.0 });
            id
        };
        let grav = put(&mut g, StationKind::Gravimetric);
        g.advance_to(g.t + 120.0);
        assert!(g.players[1].vision.ships.binary_search(&a).is_ok(), "gravimetric array missed a dark gunship");
        // seen, so in a fight, so out of the dark
        g.advance_to(g.t + 120.0);
        assert!(!g.ship(a).unwrap().dark(g.t));
        g.stations.retain(|s| s.id != grav);
        // (live battles kill an unarmed station in minutes: the gunship goes before the lidar test)
        g.ships.retain(|s| s.id != a);
        g.battles.clear();
        // lidar: needs the tech, then sweeps, then recharges
        let sc = put(&mut g, StationKind::Scanner);
        assert!(g.apply(1, Command::LidarSweep { station: sc }).unwrap_err().contains("Lidar"));
        g.players[1].techs.push(Tech::LidarSweep);
        g.apply(1, Command::LidarSweep { station: sc }).unwrap();
        assert!(g.apply(1, Command::LidarSweep { station: sc }).unwrap_err().contains("already"));
        g.advance_to(g.t + LIDAR_TIME + 120.0);
        assert!(g.apply(1, Command::LidarSweep { station: sc }).unwrap_err().contains("recharging"));
        assert!(g.events.iter().any(|e| e.to == Some(0) && e.title == "Lidar ping"));
    }

    #[test]
    fn dark_ships_lurk_unseen_and_unshot_in_a_neutral_orbit() {
        let (mut g, _) = cloaky();
        // an empty body: alice's interceptor goes dark there, bob's gunship arrives
        let b = (0..g.bodies.len() as u32).map(crate::BodyId).find(|&b| g.bodies[b.0 as usize].owner.is_none() && !g.system.body(b).is_star()).unwrap();
        let a = g.spawn(0, ShipClass::Interceptor, b);
        g.apply(0, Command::Cloak { ships: vec![a], on: true }).unwrap();
        let e = g.spawn(1, ShipClass::Gunship, b);
        g.advance_to(g.t + 600.0);
        assert!(g.players[1].vision.ships.binary_search(&a).is_err(), "bob saw the dark ship");
        assert!(g.power_at(b).iter().all(|&(p, _)| p != 0), "dark ships don't claim orbits");
        let hp = (g.ship(a).unwrap().hp, g.ship(e).unwrap().hp);
        g.advance_to(g.t + 3600.0);
        assert_eq!((g.ship(a).unwrap().hp, g.ship(e).unwrap().hp), hp, "nobody fights while one side is dark and unseen");
        // decloak: first strike, and the fight is on
        g.apply(0, Command::Cloak { ships: vec![a], on: false }).unwrap();
        g.advance_to(g.t + 600.0);
        assert!(g.ship(e).map_or(true, |s| s.hp < hp.1));
    }
}
