//! Advanced-tier logistics (Transit branch).
//!
//! - Laser launch array (station): a big laser pushes a departing ship's light sail, so the
//!   first `LAUNCH_DV` of every departure burn from that body costs no fuel. The trajectory
//!   physics is unchanged; only the fuel bill is.
//! - Stargates (station): the one exotic piece. A ship parked at one of your gates can jump to
//!   any of your other gates and arrives parked there. A jump costs `JUMP_ENERGY` per tonne, a
//!   gate passes at most `GATE_MASS` t per `GATE_WINDOW`, a gate is blocked while an enemy holds
//!   the orbit at either end, and gates are so bright that every player sees their bodies.
use crate::game::{EventKind, Game, PlayerId, ShipId, ShipState};
use crate::station::{StationId, StationKind};
use crate::system::BodyId;
use serde::{Deserialize, Serialize};

pub const LAUNCH_DV: f64 = 3.0;
pub const JUMP_ENERGY: f64 = 1.0;
pub const GATE_MASS: f64 = 1500.0;
pub const GATE_WINDOW: f64 = 6.0 * 3600.0;

/// Mass a gate passed at time t (for its throughput limit).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GateUse {
    pub gate: StationId,
    pub t: f64,
    pub mass: f64,
}

impl Game {
    /// Free Δv a departure from `body` gets from `p`'s laser launch array there.
    pub fn launch_assist(&self, p: PlayerId, body: BodyId) -> f64 {
        if self.stations_at(body).any(|s| s.owner == p && s.kind == StationKind::LaserLaunch) {
            LAUNCH_DV
        } else {
            0.0
        }
    }

    fn gate_at(&self, p: PlayerId, body: BodyId) -> Option<StationId> {
        self.stations_at(body).find(|s| s.owner == p && s.kind == StationKind::Stargate).map(|s| s.id)
    }

    /// Bodies with one of `p`'s finished gates.
    pub fn gates_of(&self, p: PlayerId) -> Vec<BodyId> {
        let mut out: Vec<BodyId> = self.stations.iter().filter(|s| s.owner == p && s.kind == StationKind::Stargate && s.active(self.t)).map(|s| s.body).collect();
        out.sort();
        out.dedup();
        out
    }

    /// Mass a gate can still pass in the current window.
    pub fn gate_room(&self, gate: StationId) -> f64 {
        let used: f64 = self.gate_uses.iter().filter(|u| u.gate == gate && self.t - u.t < GATE_WINDOW).map(|u| u.mass).sum();
        (GATE_MASS - used).max(0.0)
    }

    /// Does an enemy out-gun `p` in orbit of `body`?
    fn held_against(&self, p: PlayerId, body: BodyId) -> bool {
        let pw = self.power_at(body);
        let mine = pw.iter().filter(|x| self.allied(x.0, p)).map(|x| x.1).sum::<f64>();
        pw.iter().any(|x| !self.allied(x.0, p) && x.1 > mine)
    }

    /// Why these ships can't jump from where they are to `to` (None = they can).
    pub fn jump_blocker(&self, p: PlayerId, ships: &[ShipId], to: BodyId) -> Option<String> {
        let mut from = None;
        let mut mass = 0.0;
        for &id in ships {
            let Some(s) = self.ship(id).filter(|s| s.owner == p) else { return Some("not your ship".into()) };
            let ShipState::Parked { body, .. } = s.state else { return Some(format!("{} must be parked at a gate", s.name)) };
            if from.is_some_and(|f| f != body) {
                return Some("ships must all be at the same gate".into());
            }
            from = Some(body);
            mass += s.mass();
        }
        let from = from?;
        if from == to {
            return Some("already there".into());
        }
        let Some(g0) = self.gate_at(p, from) else { return Some("no finished gate of ours here".into()) };
        let Some(g1) = self.gate_at(p, to) else { return Some(format!("no finished gate of ours at {}", self.system.body(to).name)) };
        if self.held_against(p, from) || self.held_against(p, to) {
            return Some("the enemy holds the orbit at one end".into());
        }
        let room = self.gate_room(g0).min(self.gate_room(g1));
        if mass > room {
            return Some(format!("{mass:.0} t is more than the gates can pass now ({room:.0} t)"));
        }
        let cost = JUMP_ENERGY * mass;
        if self.players[p as usize].energy < cost {
            return Some(format!("needs {cost:.0} energy"));
        }
        None
    }

    /// `Command::Jump`.
    pub(crate) fn jump(&mut self, p: PlayerId, ships: &[ShipId], to: BodyId) -> Result<(), String> {
        let ships = self.expand_fleets(ships);
        if let Some(why) = self.jump_blocker(p, &ships, to) {
            return Err(why);
        }
        let from = self.ship(ships[0]).and_then(|s| s.parked_at()).unwrap();
        let mass: f64 = ships.iter().filter_map(|&id| self.ship(id)).map(|s| s.mass()).sum();
        self.pay(p, 0.0, JUMP_ENERGY * mass)?;
        let t = self.t;
        for g in [self.gate_at(p, from).unwrap(), self.gate_at(p, to).unwrap()] {
            self.gate_uses.push(GateUse { gate: g, t, mass });
        }
        self.gate_uses.retain(|u| t - u.t < GATE_WINDOW);
        for &id in &ships {
            let s = self.ship_mut(id).unwrap();
            s.state = ShipState::Parked { body: to, departure: None };
            s.route.clear();
            s.intercept = None;
            // the jump lights the ship up like a burn
            s.last_burn_t = t;
        }
        let (a, b) = (self.system.body(from).name.clone(), self.system.body(to).name.clone());
        self.event(Some(p), EventKind::Good, "Gate jump", &format!("{} ship{} jumped from {a} to {b}", ships.len(), if ships.len() == 1 { "" } else { "s" }), Some(to), None);
        for &id in &ships {
            self.trip_mines(id, to, 0.0);
        }
        Ok(())
    }

    /// Bodies with an active stargate: everyone sees them.
    pub(crate) fn gate_bodies(&self) -> Vec<BodyId> {
        let mut out: Vec<BodyId> = self.stations.iter().filter(|s| s.kind == StationKind::Stargate && s.active(self.t)).map(|s| s.body).collect();
        out.sort();
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Command, ShipClass};
    use crate::station::Station;

    fn put(g: &mut Game, owner: PlayerId, kind: StationKind, body: BodyId) {
        let id = StationId(g.next_station);
        g.next_station += 1;
        g.stations.push(Station { id, owner, kind, body, hp: kind.max_hp(), fuel: 0.0, done_t: 0.0, work_left: 0.0 });
    }

    #[test]
    fn gates_jump_ships_and_limit_mass() {
        let mut g = crate::tech::tests::game();
        let home = g.players[0].home.unwrap();
        let far = (0..g.bodies.len() as u32).map(BodyId).find(|&b| g.bodies[b.0 as usize].owner.is_none() && !g.system.body(b).is_star()).unwrap();
        let ship = g.ships.iter().find(|s| s.owner == 0 && s.class == ShipClass::Gunship).unwrap().id;
        assert!(g.apply(0, Command::Jump { ships: vec![ship], to: far }).unwrap_err().contains("gate"));
        put(&mut g, 0, StationKind::Stargate, home);
        put(&mut g, 0, StationKind::Stargate, far);
        let e0 = g.players[0].energy;
        g.apply(0, Command::Jump { ships: vec![ship], to: far }).unwrap();
        let s = g.ship(ship).unwrap();
        assert_eq!(s.parked_at(), Some(far));
        assert!((e0 - g.players[0].energy - s.mass() * JUMP_ENERGY).abs() < 1e-6);
        // everyone sees a gate's body
        g.advance_to(g.t + 120.0);
        assert!(g.players[1].vision.bodies.binary_search(&far).is_ok());
        // a dreadnought (1000 t) plus the gunship's earlier trip overflows the window
        let d = g.spawn(0, ShipClass::Dreadnought, home);
        let d2 = g.spawn(0, ShipClass::Dreadnought, home);
        assert!(g.jump_blocker(0, &[d, d2], far).unwrap().contains("more than"));
        // an enemy holding the far end blocks the gate
        for _ in 0..3 {
            g.spawn(1, ShipClass::Dreadnought, far);
        }
        assert!(g.jump_blocker(0, &[d], far).unwrap().contains("holds the orbit"));
    }

    #[test]
    fn launch_array_saves_departure_fuel() {
        let mut g = crate::tech::tests::game();
        let home = g.players[0].home.unwrap();
        let to = g.players[1].home.unwrap();
        let ship = g.ships.iter().find(|s| s.owner == 0 && s.class == ShipClass::Gunship).unwrap().id;
        let twin = g.ships.iter().filter(|s| s.owner == 0 && s.class == ShipClass::Gunship).nth(1).unwrap().id;
        // same course, one with the array
        g.apply(0, Command::Transfer { ships: vec![twin], to, depart_t: None }).unwrap();
        let dep = |g: &Game, id| match &g.ship(id).unwrap().state {
            ShipState::Parked { departure: Some(d), .. } => d.state.t,
            _ => panic!("no departure"),
        };
        let t_dep = dep(&g, twin);
        g.advance_to(t_dep + 60.0);
        let burned = g.ship(twin).unwrap().stats().fuel_cap - g.ship(twin).unwrap().fuel;
        put(&mut g, 0, StationKind::LaserLaunch, home);
        g.apply(0, Command::Transfer { ships: vec![ship], to, depart_t: None }).unwrap();
        let t_dep2 = dep(&g, ship);
        let f0 = g.ship(ship).unwrap().fuel;
        g.advance_to(t_dep2 + 60.0);
        let burned2 = f0 - g.ship(ship).unwrap().fuel;
        assert!(burned2 < burned * 0.95, "array saved nothing: {burned2} vs {burned}");
    }
}
