//! Interplanetary weapons (advanced tier, Strike branch). Both are real objects on real
//! trajectories, so they can be seen coming, shot down, or dodged.
//!
//! - Hunter-killer torpedo (`ShipClass::Torpedo`): an uncrewed, high-Δv hull built at a shipyard
//!   and sent with the normal intercept order. Within `DETONATE_RANGE` of its target (or, without
//!   one, the nearest enemy ship) it detonates for `TORPEDO_DAMAGE`. It's a ship, so guns and
//!   interdiction grids can kill it on the way in, and a decoy can soak it.
//! - Mass-driver slug (`ShipClass::Slug`): a mass driver station loads a 20 t guided slug that you
//!   send at a body like any ship. Instead of braking into orbit it hits: damage from its arrival
//!   speed (½mv²) spread over the defenses and enemy ships parked there (and stations, with
//!   Fusion warheads). It can't chase ships in flight.
use crate::game::{EventKind, Game, PlayerId, ShipClass, ShipId};
use crate::station::{StationId, StationKind};
use crate::system::BodyId;
use crate::tech::Tech;
use crate::v2::V2;
use serde::{Deserialize, Serialize};

pub const DETONATE_RANGE: f64 = 2_000.0;
pub const TORPEDO_DAMAGE: f64 = 150.0;
/// Slug damage at `SLUG_REF_SPEED` arrival speed; scales with the speed squared, capped.
pub const SLUG_DAMAGE: f64 = 120.0;
pub const SLUG_REF_SPEED: f64 = 25.0;
pub const SLUG_MAX: f64 = 400.0;
/// Mass driver: cost and reload per slug.
pub const SLUG_COST: (f64, f64) = (150.0, 300.0);
pub const RELOAD: f64 = 6.0 * 3600.0;
/// Interdiction grid: reach, and one kill per this many seconds.
pub const GRID_RANGE: f64 = 20_000.0;
pub const GRID_COOLDOWN: f64 = 1800.0;

/// When a station last fired (mass driver reloads, grid kills).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shot {
    pub station: StationId,
    pub t: f64,
}

pub fn is_ordnance(c: ShipClass) -> bool {
    matches!(c, ShipClass::Torpedo | ShipClass::Slug)
}

impl Game {
    fn last_shot(&self, st: StationId) -> f64 {
        self.station_shots.iter().filter(|s| s.station == st).map(|s| s.t).fold(f64::NEG_INFINITY, f64::max)
    }
    fn mark_shot(&mut self, st: StationId) {
        let t = self.t;
        self.station_shots.retain(|s| s.station != st);
        self.station_shots.push(Shot { station: st, t });
    }

    /// Why `p` can't load a slug at its mass driver at `body` (None = it can).
    pub fn slug_blocker(&self, p: PlayerId, body: BodyId) -> Option<String> {
        if !self.has_tech(p, Tech::MassDrivers) {
            return Some(format!("needs {}", Tech::MassDrivers.label()));
        }
        let Some(st) = self.stations_at(body).find(|s| s.owner == p && s.kind == StationKind::MassDriver) else {
            return Some("needs a finished mass driver here".into());
        };
        let ready = self.last_shot(st.id) + RELOAD;
        if self.t < ready {
            return Some(format!("reloading for {:.0} min", (ready - self.t) / 60.0));
        }
        None
    }

    /// `Command::LoadSlug`: a slug appears parked at the driver; send it like any ship.
    pub(crate) fn load_slug(&mut self, p: PlayerId, body: BodyId) -> Result<ShipId, String> {
        if let Some(why) = self.slug_blocker(p, body) {
            return Err(why);
        }
        self.pay(p, SLUG_COST.0, SLUG_COST.1)?;
        let st = self.stations_at(body).find(|s| s.owner == p && s.kind == StationKind::MassDriver).unwrap().id;
        self.mark_shot(st);
        Ok(self.spawn(p, ShipClass::Slug, body))
    }

    /// A slug arrived at `body` at `v` km/s: it hits instead of braking.
    pub(crate) fn slug_impact(&mut self, ship: ShipId, body: BodyId, v: f64) {
        let Some(si) = self.ships.iter().position(|s| s.id == ship) else { return };
        let owner = self.ships[si].owner;
        self.ships.remove(si);
        let m = self.mods(owner);
        let dmg = (SLUG_DAMAGE * (v / SLUG_REF_SPEED).powi(2)).min(SLUG_MAX) * m.ordnance_dmg;
        let stations_too = self.has_tech(owner, Tech::FusionWarheads);
        // spread over everything hostile in that orbit, by share
        let bi = body.0 as usize;
        let defense = self.bodies[bi].owner.filter(|&o| !self.allied(o, owner) && self.bodies[bi].def_hp > 0.0).is_some();
        let ships: Vec<usize> = (0..self.ships.len()).filter(|&i| !self.allied(self.ships[i].owner, owner) && self.ships[i].parked_at() == Some(body)).collect();
        let stations: Vec<usize> = if stations_too { (0..self.stations.len()).filter(|&i| !self.allied(self.stations[i].owner, owner) && self.stations[i].body == body).collect() } else { vec![] };
        let n = (ships.len() + stations.len() + defense as usize).max(1) as f64;
        let each = dmg / n;
        if defense {
            let b = &mut self.bodies[bi];
            b.def_hp = (b.def_hp - each).max(0.0);
        }
        for &i in &ships {
            self.ships[i].hp -= each;
        }
        for &i in &stations {
            self.stations[i].hp -= each;
        }
        let bname = self.system.body(body).name.clone();
        let mut hurt: Vec<PlayerId> = ships.iter().map(|&i| self.ships[i].owner).chain(stations.iter().map(|&i| self.stations[i].owner)).collect();
        if defense {
            hurt.extend(self.bodies[bi].owner);
        }
        hurt.sort();
        hurt.dedup();
        let lost: Vec<(PlayerId, ShipId, String)> = self.ships.iter().filter(|s| s.hp <= 0.0).map(|s| (s.owner, s.id, s.name.clone())).collect();
        self.ships.retain(|s| s.hp > 0.0);
        self.stations.retain(|s| s.hp > 0.0);
        self.event(Some(owner), EventKind::Battle, "Slug impact", &format!("Our slug hit {bname} at {v:.1} km/s ({dmg:.0} damage)"), Some(body), None);
        for q in hurt {
            self.event(Some(q), EventKind::Battle, "Slug impact", &format!("A mass-driver slug hit {bname} at {v:.1} km/s"), Some(body), None);
        }
        for (q, id, name) in lost {
            self.ship_note(crate::shiplog::ShipNote { to: q, kind: EventKind::Battle, title: ("Ship destroyed", "Ships destroyed"), ship: id, name: &name, verb: ("was", "were"), rest: format!("destroyed by a slug at {bname}"), body_ref: Some(body), ship_ref: None });
        }
    }

    /// Torpedoes detonate on targets in reach; interdiction grids shoot ordnance down (per tick).
    pub(crate) fn ordnance_tick(&mut self) {
        if !self.ships.iter().any(|s| is_ordnance(s.class)) {
            return;
        }
        let t = self.t;
        let pos: Vec<V2> = self.ships.iter().map(|s| self.ship_pos(s, t)).collect();
        // grids first: each kills one incoming enemy torpedo/slug within reach per cooldown
        let grids: Vec<(StationId, PlayerId, V2)> =
            self.stations.iter().filter(|s| s.kind == StationKind::Interdiction && s.active(t)).map(|s| (s.id, s.owner, self.system.pos(s.body, t))).collect();
        let mut dead: Vec<usize> = vec![];
        for (gid, owner, gp) in grids {
            if t < self.last_shot(gid) + GRID_COOLDOWN {
                continue;
            }
            let near = (0..self.ships.len())
                .filter(|&i| is_ordnance(self.ships[i].class) && self.ships[i].owner != owner && !dead.contains(&i))
                .filter(|&i| pos[i].dist(gp) <= GRID_RANGE)
                .min_by(|&a, &b| pos[a].dist(gp).total_cmp(&pos[b].dist(gp)));
            if let Some(i) = near {
                dead.push(i);
                self.mark_shot(gid);
                let (o, name) = (self.ships[i].owner, self.ships[i].name.clone());
                self.event(Some(o), EventKind::Battle, "Shot down", &format!("{name} was shot down by an interdiction grid"), None, None);
                self.event(Some(owner), EventKind::Good, "Shot down", "Our interdiction grid destroyed incoming ordnance", None, None);
            }
        }
        // torpedoes in reach of their target (or any enemy ship) detonate
        let mut hits: Vec<(usize, usize)> = vec![];
        for (i, s) in self.ships.iter().enumerate() {
            if s.class != ShipClass::Torpedo || dead.contains(&i) {
                continue;
            }
            let target = s.intercept.as_ref().map(|o| o.target);
            let pick = (0..self.ships.len())
                .filter(|&j| j != i && !self.allied(self.ships[j].owner, s.owner) && !is_ordnance(self.ships[j].class) && !dead.contains(&j))
                .filter(|&j| pos[j].dist(pos[i]) <= DETONATE_RANGE)
                .min_by(|&a, &b| {
                    let pa = (Some(self.ships[a].id) != target, pos[a].dist(pos[i]));
                    let pb = (Some(self.ships[b].id) != target, pos[b].dist(pos[i]));
                    pa.0.cmp(&pb.0).then(pa.1.total_cmp(&pb.1))
                });
            if let Some(j) = pick {
                hits.push((i, j));
            }
        }
        for &(i, j) in &hits {
            let mult = self.mods(self.ships[i].owner).ordnance_dmg * if self.ships[j].class == ShipClass::Dreadnought { 2.0 } else { 1.0 };
            self.ships[j].hp -= TORPEDO_DAMAGE * mult;
            dead.push(i);
            let (o, name, victim, vo) = (self.ships[i].owner, self.ships[i].name.clone(), self.ships[j].name.clone(), self.ships[j].owner);
            let killed = self.ships[j].hp <= 0.0;
            let what = if killed { "destroyed" } else { "hit" };
            self.event(Some(o), EventKind::Battle, "Torpedo hit", &format!("{name} {what} an enemy {}", self.ships[j].class.stats().name), self.ships[j].parked_at(), None);
            self.event(Some(vo), EventKind::Battle, if killed { "Ship destroyed" } else { "Torpedo hit" }, &format!("{victim} was {what} by a torpedo"), self.ships[j].parked_at(), None);
        }
        for &(_, j) in &hits {
            if self.ships[j].hp <= 0.0 {
                dead.push(j);
            }
        }
        dead.sort();
        dead.dedup();
        for i in dead.into_iter().rev() {
            self.ships.remove(i);
        }
    }

    /// Slugs fly ballistic: they never take intercept orders.
    pub(crate) fn no_slug(&self, ships: &[ShipId]) -> Result<(), String> {
        if ships.iter().filter_map(|&id| self.ship(id)).any(|s| s.class == ShipClass::Slug) {
            return Err("slugs can only be sent at a body".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Command;

    #[test]
    fn torpedo_detonates_on_a_nearby_enemy() {
        let mut g = crate::tech::tests::game();
        let home = g.players[0].home.unwrap();
        let b = (0..g.bodies.len() as u32).map(BodyId).find(|&b| g.bodies[b.0 as usize].owner.is_none() && !g.system.body(b).is_star()).unwrap();
        assert!(g.apply(0, Command::Build { body: home, class: ShipClass::Torpedo }).unwrap_err().contains("torpedoes"));
        let tp = g.spawn(0, ShipClass::Torpedo, b);
        let d = g.spawn(1, ShipClass::Dreadnought, b);
        let hp = g.ship(d).unwrap().hp;
        g.ordnance_tick();
        assert!(g.ship(tp).is_none(), "torpedo is spent");
        assert!((g.ship(d).unwrap().hp - (hp - 2.0 * TORPEDO_DAMAGE)).abs() < 1e-9);
    }

    #[test]
    fn grid_shoots_down_one_torpedo_per_cooldown() {
        let mut g = crate::tech::tests::game();
        let b = (0..g.bodies.len() as u32).map(BodyId).find(|&b| g.bodies[b.0 as usize].owner.is_none() && !g.system.body(b).is_star()).unwrap();
        let id = StationId(g.next_station);
        g.next_station += 1;
        g.stations.push(crate::station::Station { id, owner: 1, kind: StationKind::Interdiction, body: b, hp: 250.0, fuel: 0.0, done_t: 0.0, work_left: 0.0 });
        let t1 = g.spawn(0, ShipClass::Torpedo, b);
        let t2 = g.spawn(0, ShipClass::Torpedo, b);
        g.ordnance_tick();
        assert_eq!([t1, t2].iter().filter(|&&t| g.ship(t).is_some()).count(), 1);
        g.ordnance_tick();
        assert_eq!([t1, t2].iter().filter(|&&t| g.ship(t).is_some()).count(), 1, "grid still cooling down");
    }

    #[test]
    fn slugs_load_reload_and_hit() {
        let mut g = crate::tech::tests::game();
        let home = g.players[0].home.unwrap();
        assert!(g.apply(0, Command::LoadSlug { body: home }).unwrap_err().contains("Mass"));
        g.players[0].techs.push(Tech::MassDrivers);
        assert!(g.apply(0, Command::LoadSlug { body: home }).unwrap_err().contains("mass driver"));
        let id = StationId(g.next_station);
        g.next_station += 1;
        g.stations.push(crate::station::Station { id, owner: 0, kind: StationKind::MassDriver, body: home, hp: 300.0, fuel: 0.0, done_t: 0.0, work_left: 0.0 });
        g.apply(0, Command::LoadSlug { body: home }).unwrap();
        assert!(g.apply(0, Command::LoadSlug { body: home }).unwrap_err().contains("reloading"));
        let slug = g.ships.iter().find(|s| s.class == ShipClass::Slug).unwrap().id;
        assert!(g.no_slug(&[slug]).is_err());
        // it hits bob's home: defenses and parked ships share the damage
        let bob = g.players[1].home.unwrap();
        let def0 = g.bodies[bob.0 as usize].def_hp;
        let n = g.ships.iter().filter(|s| s.owner == 1 && s.parked_at() == Some(bob)).count() + 1;
        g.slug_impact(slug, bob, 1.5 * SLUG_REF_SPEED);
        // 1.5× the reference speed: 2.25× the damage, one share for the defenses
        let share = def0 - g.bodies[bob.0 as usize].def_hp;
        assert!((share - SLUG_DAMAGE * 2.25 / n as f64).abs() < 1e-6, "{share}");
        assert!(g.ship(slug).is_none());
    }

    #[test]
    fn a_slug_flies_to_its_target_and_hits() {
        let mut g = crate::tech::tests::game();
        let home = g.players[0].home.unwrap();
        let bob = g.players[1].home.unwrap();
        g.players[0].techs.push(Tech::MassDrivers);
        g.stations.push(crate::station::Station { id: StationId(99), owner: 0, kind: StationKind::MassDriver, body: home, hp: 300.0, fuel: 0.0, done_t: 0.0, work_left: 0.0 });
        let slug = g.load_slug(0, home).unwrap();
        g.apply(0, Command::Transfer { ships: vec![slug], to: bob, depart_t: None }).unwrap();
        let def0 = g.bodies[bob.0 as usize].def_hp;
        let t0 = g.t;
        while g.ship(slug).is_some() && g.t < t0 + 4.0 * 86_400.0 {
            g.advance_to(g.t + 3600.0);
        }
        assert!(g.ship(slug).is_none(), "slug never arrived");
        assert!(g.events.iter().any(|e| e.title == "Slug impact" && e.to == Some(1)));
        assert!(g.bodies[bob.0 as usize].def_hp < def0 || g.bodies[bob.0 as usize].owner != Some(1));
    }
}
