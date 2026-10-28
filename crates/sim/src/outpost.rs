//! Outposts: free-floating stations. A construction ship coasting on a stable orbit of the sun can
//! drop anchor wherever it is. The spot becomes a tiny body ("Outpost 3") on the ship's orbit, with
//! the ship parked at it, so everything that works at an asteroid works there too: stations (built
//! the usual way), parking, transfers to and from it, the orbit cap, combat and ownership by power.
//! It produces nothing and has no surface for upgrades.
//!
//! Fog: other players don't know an outpost exists until their sensors cover it (or they see a ship
//! parked there); after that it stays on their map like any body they have intel on.
//! An outpost with no station and no ship left at it is abandoned (hidden, no new transfers); a
//! ship that was already on its way re-occupies it on arrival.
use crate::asteroid::orbit_from_state;
use crate::game::{BodyState, EventKind, Flight, Game, PlayerId, ShipClass, ShipId, ShipState, ORBIT_CAP};
use crate::system::{Body, BodyId, BodyKind, STAR_MU};
use crate::traj::State;

/// km. Small enough that nothing passing by crashes into it by accident.
pub const OUTPOST_RADIUS: f64 = 2.0;
/// km³/s²: a few hundred tonnes of station, so arriving is a rendezvous, not a capture.
pub const OUTPOST_MU: f64 = 1.0;
/// km: a ship counts as arriving once it is this close.
pub const OUTPOST_SOI: f64 = 500.0;

impl Game {
    pub fn is_outpost(&self, b: BodyId) -> bool {
        self.system.bodies.get(b.0 as usize).is_some_and(|b| b.kind == BodyKind::Outpost)
    }

    /// An outpost that nothing is at any more (see module doc).
    pub fn abandoned(&self, b: BodyId) -> bool {
        self.is_outpost(b) && self.bodies[b.0 as usize].gone
    }

    /// Ship `id`'s state relative to the sun now, if it is coasting in the sun's sphere of influence.
    fn helio_state(&self, id: ShipId) -> Result<State, String> {
        let s = self.ship(id).ok_or("no such ship")?;
        let ShipState::Flying(f) = &s.state else {
            return Err("already in orbit of a body: build stations there".into());
        };
        let st = f.compute(&self.system).state_at(&self.system, self.t).ok_or("position unknown")?;
        if !self.system.body(st.parent).is_star() {
            let name = &self.system.body(st.parent).name;
            return Err(format!("inside {name}'s sphere of influence: park at {name} to build there"));
        }
        Ok(st)
    }

    /// Why `p` can't drop anchor with construction ship `id` right now (None = it can). Any bound,
    /// prograde orbit of the sun will do, even a transfer arc: the outpost is on rails, so passing
    /// through a planet's sphere of influence later does it no harm.
    pub fn outpost_blocker(&self, p: PlayerId, id: ShipId) -> Option<String> {
        let s = self.ship(id)?;
        if s.owner != p {
            return Some("not your ship".into());
        }
        if s.class != ShipClass::Constructor {
            return Some("only construction ships can found an outpost".into());
        }
        let st = match self.helio_state(id) {
            Ok(st) => st,
            Err(e) => return Some(e),
        };
        if orbit_from_state(STAR_MU, st.r, st.v, st.t).is_none() {
            return Some("not on a stable orbit of the sun (escaping, too eccentric or retrograde)".into());
        }
        let crew = self.with_fleetmates(id);
        if crew.len() > ORBIT_CAP {
            return Some(format!("at most {ORBIT_CAP} ships can stop there"));
        }
        None
    }

    /// Construction ship `id` (and the fleet flying with it) stops where it is: a new outpost body
    /// on its current orbit of the sun, with the ships parked at it. Planned burns are dropped.
    pub(crate) fn found_outpost(&mut self, p: PlayerId, id: ShipId) -> Result<BodyId, String> {
        if let Some(why) = self.outpost_blocker(p, id) {
            return Err(why);
        }
        let st = self.helio_state(id)?;
        let orbit = orbit_from_state(STAR_MU, st.r, st.v, st.t).ok_or("not on a stable orbit of the sun")?;
        let star = self.system.star().id;
        let body = BodyId(self.system.bodies.len() as u32);
        let n = self.system.bodies.iter().filter(|b| b.kind == BodyKind::Outpost).count() + 1;
        let name = format!("Outpost {n}");
        self.system.bodies.push(Body {
            id: body,
            name: name.clone(),
            kind: BodyKind::Outpost,
            parent: Some(star),
            radius: OUTPOST_RADIUS,
            mu: OUTPOST_MU,
            soi: OUTPOST_SOI,
            orbit: Some(orbit),
            look: 0,
        });
        self.bodies.push(BodyState::default());
        for pl in self.players.iter_mut() {
            pl.intel.resize(self.bodies.len(), None);
        }
        // fleetmates fly the same course, so they stop together
        for m in self.with_fleetmates(id) {
            let s = self.ship_mut(m).unwrap();
            if matches!(s.state, ShipState::Flying(_)) {
                s.state = ShipState::Parked { body, departure: None };
                s.route.clear();
                s.intercept = None;
            }
        }
        let who = self.ship(id).map(|s| s.name.clone()).unwrap_or_default();
        let period = fmt_dur(orbit.period());
        self.event(Some(p), EventKind::Good, "Outpost founded", &format!("{who} dropped anchor: {name} circles the sun every {period}. Build a station there to hold it."), Some(body), Some(id));
        Ok(body)
    }

    /// Abandon outposts nothing is at any more; re-occupy abandoned ones a ship arrived at.
    pub(crate) fn outposts_tick(&mut self) {
        for i in 0..self.system.bodies.len() {
            if self.system.bodies[i].kind != BodyKind::Outpost {
                continue;
            }
            let b = BodyId(i as u32);
            let used = self.stations.iter().any(|st| st.body == b) || self.ships.iter().any(|s| s.parked_at() == Some(b));
            if used == self.bodies[i].gone {
                self.bodies[i] = BodyState { gone: !used, ..Default::default() };
            }
        }
    }

    /// Player `p` doesn't know this outpost exists (or it's abandoned and they never saw it).
    pub fn outpost_hidden(&self, p: PlayerId, b: BodyId) -> bool {
        if !self.is_outpost(b) {
            return false;
        }
        let Some(pl) = self.players.get(p as usize) else { return false };
        let mine = self.stations.iter().any(|st| st.owner == p && st.body == b) || self.ships.iter().any(|s| s.owner == p && (s.parked_at() == Some(b) || s.dest() == Some(b)));
        !mine && pl.vision.bodies.binary_search(&b).is_err() && pl.intel.get(b.0 as usize).is_none_or(|i| i.is_none())
    }
}

/// "2d 3h" / "3h 20m" / "45m" for messages.
fn fmt_dur(secs: f64) -> String {
    let m = (secs.max(0.0) / 60.0).round() as u64;
    let (d, h, m) = (m / 1440, m / 60 % 24, m % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Command;
    use crate::v2::v2;

    /// A running game with player 0's construction ship coasting on a circular orbit of the sun,
    /// halfway between the first two planets' orbits, well clear of either.
    fn coasting() -> (Game, ShipId) {
        let mut g = crate::tech::tests::game();
        let star = g.system.star().id;
        let planets: Vec<f64> = g.system.bodies.iter().filter(|b| b.parent == Some(star) && matches!(b.kind, BodyKind::Planet | BodyKind::GasGiant)).map(|b| b.orbit.unwrap().a).collect();
        let r = 0.5 * (planets[0] + planets[1]);
        let v = (STAR_MU / r).sqrt();
        // pick a spot on that circle far from every body for the next few days
        let mut best = (0.0, 0.0);
        for k in 0..72 {
            let ang = k as f64 * std::f64::consts::TAU / 72.0;
            let pos = v2(r * ang.cos(), r * ang.sin());
            let clear = g.system.bodies.iter().skip(1).map(|b| g.system.pos(b.id, g.t).dist(pos) - b.soi).fold(f64::INFINITY, f64::min);
            if clear > best.1 {
                best = (ang, clear);
            }
        }
        let (ang, _) = best;
        let start = State { parent: star, t: g.t, r: v2(r * ang.cos(), r * ang.sin()), v: v2(-v * ang.sin(), v * ang.cos()) };
        let home = g.players[0].home.unwrap();
        let id = g.spawn(0, ShipClass::Constructor, home);
        g.ship_mut(id).unwrap().state = ShipState::Flying(Flight { start, burns: vec![], dest: None, cache: None });
        (g, id)
    }

    #[test]
    fn constructor_founds_an_outpost_that_follows_its_orbit() {
        let (mut g, id) = coasting();
        let pos0 = g.ship_pos(g.ship(id).unwrap(), g.t);
        let n = g.system.bodies.len();
        if let Some(why) = g.outpost_blocker(0, id) {
            panic!("blocked: {why}");
        }
        g.apply(0, Command::FoundOutpost { ship: id }).unwrap();
        let b = BodyId(n as u32);
        assert!(g.is_outpost(b));
        assert_eq!(g.bodies.len(), g.system.bodies.len());
        assert_eq!(g.ship(id).unwrap().parked_at(), Some(b));
        assert!(g.system.pos(b, g.t).dist(pos0) < 1.0, "outpost not where the ship was");
        // a station goes up there the usual way, and the outpost stays on the ship's old orbit
        g.players[0].matter = 1e6;
        g.players[0].energy = 1e6;
        g.apply(0, Command::BuildStation { body: b, kind: crate::station::StationKind::FuelDepot }).unwrap();
        let t1 = g.t + 86_400.0;
        g.advance_to(t1);
        assert!(!g.bodies[b.0 as usize].gone);
        assert!(g.has_station(0, b, crate::station::StationKind::FuelDepot));
        let r = g.system.pos(b, t1).len();
        let r0 = pos0.len();
        assert!((r - r0).abs() < r0 * 1e-6, "circular orbit drifted: {r} vs {r0}");
        // other players don't know about it unless their sensors reach it
        assert!(!g.outpost_hidden(0, b));
    }

    #[test]
    fn outpost_needs_a_coasting_constructor() {
        let (mut g, id) = coasting();
        let home = g.players[0].home.unwrap();
        let other = g.spawn(0, ShipClass::Gunship, home);
        assert!(g.outpost_blocker(0, other).is_some(), "a parked gunship can't found one");
        assert!(g.outpost_blocker(1, id).is_some(), "someone else's ship");
        // escaping the sun
        if let ShipState::Flying(f) = &mut g.ship_mut(id).unwrap().state {
            f.start.v = f.start.v * 1.5;
            f.cache = None;
        }
        assert!(g.outpost_blocker(0, id).is_some(), "escape orbit accepted");
        assert!(g.apply(0, Command::FoundOutpost { ship: id }).is_err());
    }

    #[test]
    fn abandoned_outpost_hides_and_old_saves_load() {
        let (mut g, id) = coasting();
        g.apply(0, Command::FoundOutpost { ship: id }).unwrap();
        let b = BodyId(g.system.bodies.len() as u32 - 1);
        // the constructor leaves without building anything
        g.ships.retain(|s| s.id != id);
        g.outposts_tick();
        assert!(g.abandoned(b));
        assert!(g.apply(0, Command::Transfer { ships: vec![], to: b, depart_t: None }).is_err());
        // a save round-trips (the new body kind serialises)
        let js = serde_json::to_string(&g).unwrap();
        let g2: Game = serde_json::from_str(&js).unwrap();
        assert!(g2.abandoned(b));
    }

    #[test]
    fn hidden_from_players_whose_sensors_never_saw_it() {
        let (mut g, id) = coasting();
        g.apply(0, Command::FoundOutpost { ship: id }).unwrap();
        let b = BodyId(g.system.bodies.len() as u32 - 1);
        g.players[1].vision.bodies.clear();
        g.players[1].intel[b.0 as usize] = None;
        assert!(g.outpost_hidden(1, b));
        let v = g.view_for(1);
        assert!(v.bodies[b.0 as usize].gone, "player 1 sees an outpost it never detected");
        assert!(v.system.bodies[b.0 as usize].orbit.is_some(), "orbit must stay for bit-exact prediction");
        assert!(g.apply(1, Command::Transfer { ships: vec![], to: b, depart_t: None }).is_err());
    }

    #[test]
    fn ships_fly_to_an_outpost_and_back() {
        let (mut g, id) = coasting();
        g.apply(0, Command::FoundOutpost { ship: id }).unwrap();
        let b = BodyId(g.system.bodies.len() as u32 - 1);
        let home = g.players[0].home.unwrap();
        let tanker = g.spawn(0, ShipClass::Tanker, home);
        g.ship_mut(tanker).unwrap().fuel = 1e9;
        g.apply(0, Command::Transfer { ships: vec![tanker], to: b, depart_t: None }).unwrap();
        let end = g.t + 20.0 * 86_400.0;
        while g.t < end && g.ship(tanker).unwrap().parked_at() != Some(b) {
            let t = g.t + 3600.0;
            g.advance_to(t);
        }
        assert_eq!(g.ship(tanker).unwrap().parked_at(), Some(b), "never arrived at the outpost");
        // and home again
        g.apply(0, Command::Transfer { ships: vec![tanker], to: home, depart_t: None }).unwrap();
        let end = g.t + 20.0 * 86_400.0;
        while g.t < end && g.ship(tanker).unwrap().parked_at() != Some(home) {
            let t = g.t + 3600.0;
            g.advance_to(t);
        }
        assert_eq!(g.ship(tanker).unwrap().parked_at(), Some(home), "never got back home");
        // the constructor stayed, so the outpost is still there
        assert!(!g.abandoned(b));
    }

    #[test]
    fn constructor_can_stop_partway_through_a_transfer() {
        let mut g = crate::tech::tests::game();
        let home = g.players[0].home.unwrap();
        let star = g.system.star().id;
        let to = g.system.bodies.iter().find(|b| b.parent == Some(star) && b.id != home && b.kind == BodyKind::Planet).unwrap().id;
        let c = g.spawn(0, ShipClass::Constructor, home);
        g.ship_mut(c).unwrap().fuel = 1e9;
        g.apply(0, Command::Transfer { ships: vec![c], to, depart_t: None }).unwrap();
        let end = g.t + 30.0 * 86_400.0;
        while g.t < end && g.outpost_blocker(0, c).is_some() {
            let t = g.t + 1800.0;
            g.advance_to(t);
        }
        g.apply(0, Command::FoundOutpost { ship: c }).unwrap();
        let b = g.ship(c).unwrap().parked_at().unwrap();
        assert!(g.is_outpost(b));
        assert_eq!(g.ship(c).unwrap().dest(), None);
    }
}
