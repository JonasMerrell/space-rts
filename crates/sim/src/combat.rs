//! Deterministic weapon resolution that accounts for range and relative velocity.
//!
//! Energy weapons (lasers) hit at light speed, so they always connect, but the beam spreads and
//! loses punch with range. Kinetic weapons (slugs/missiles) leave the shooter at `KINETIC_SPEED`
//! and must lead the target: the longer the flight and the more the target can dodge, the more
//! they miss, and they cannot reach a target that outruns them. A slug that connects hits with
//! its speed relative to the target, so head-on passes hurt and stern chases tickle.
//!
//! There is no randomness: each shot deals its *expected* damage (attack × hit fraction ×
//! damage factor × class multiplier), which keeps the server and every client bit-identical.
use crate::game::{PlayerId, ShipClass, Weapon};
use crate::v2::{v2, V2};

/// Slug muzzle speed relative to the shooter, km/s.
pub const KINETIC_SPEED: f64 = 25.0;
/// Speed of light, km/s.
pub const LIGHT_SPEED: f64 = 299_792.458;
pub const KINETIC_RANGE: f64 = 4_000.0;
pub const ENERGY_RANGE: f64 = 8_000.0;
/// Phased laser battery reach and falloff distance (km).
pub const BEAM_RANGE: f64 = 40_000.0;
pub const BATTERY_FALLOFF: f64 = 15_000.0;
/// Beam damage halves at this range.
pub const BEAM_FALLOFF: f64 = 3_000.0;
/// Kinetic damage relative to energy when a slug hits at muzzle speed.
pub const KINETIC_BONUS: f64 = 1.25;
/// Ships sharing a parking orbit still fight across this much space.
pub const MIN_RANGE: f64 = 50.0;
/// Fraction of the target's transverse speed that turns into aiming error per second of flight.
const TRACKING_ERROR: f64 = 0.0014;

pub fn range(w: Weapon) -> f64 {
    match w {
        Weapon::None => 0.0,
        Weapon::Kinetic => KINETIC_RANGE,
        Weapon::Energy => ENERGY_RANGE,
        Weapon::Beam => BEAM_RANGE,
    }
}

/// Effective radius (km) of a ship as a target: formation spread plus hull, scaled by signature.
pub fn size(class: ShipClass) -> f64 {
    0.4 * class.stats().signature
}

/// How hard a ship can jink while slugs are in flight, km/s².
pub fn evasion(class: ShipClass) -> f64 {
    0.0003 / class.stats().signature.sqrt()
}

/// Anything that shoots or can be shot at.
#[derive(Clone, Copy, Debug)]
pub struct Combatant {
    /// stable, unique sort key (ships: id, defenses: 1<<40 | body) so results don't depend on order
    pub key: u64,
    pub owner: PlayerId,
    /// None for planetary defenses and stations
    pub class: Option<ShipClass>,
    pub weapon: Weapon,
    /// damage per hour against a neutral target at point-blank
    pub attack: f64,
    pub hp: f64,
    /// absolute position (km) and velocity (km/s)
    pub pos: V2,
    pub vel: V2,
    /// target radius (km)
    pub size: f64,
    /// evasive acceleration (km/s²)
    pub evade: f64,
    /// research multipliers
    pub dmg_mult: f64,
    pub acc_mult: f64,
}

impl Combatant {
    pub fn is_defense(&self) -> bool {
        self.class.is_none()
    }
}

/// Geometry of one shot: time of flight, hit fraction (0..1) and damage factor (1 = nominal).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shot {
    pub tof: f64,
    pub hit: f64,
    pub factor: f64,
}

/// Solve the lead for a projectile of speed `s` from the origin at a target at `r` moving with
/// `u` (both relative to the shooter). Returns the earliest positive time of flight.
pub fn lead_time(r: V2, u: V2, s: f64) -> Option<f64> {
    // |r + u t| = s t  →  (u·u − s²) t² + 2 (r·u) t + r·r = 0
    let a = u.len2() - s * s;
    let b = 2.0 * r.dot(u);
    let c = r.len2();
    if a.abs() < 1e-12 {
        return (b < 0.0).then(|| -c / b);
    }
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return None;
    }
    let sq = disc.sqrt();
    let (t1, t2) = ((-b - sq) / (2.0 * a), (-b + sq) / (2.0 * a));
    let (lo, hi) = (t1.min(t2), t1.max(t2));
    if lo > 0.0 {
        Some(lo)
    } else if hi > 0.0 {
        Some(hi)
    } else {
        None
    }
}

/// Resolve one shot of `weapon` at a target at relative position `r` with relative velocity `u`
/// (target minus shooter), target radius `size` and evasive acceleration `evade`.
/// None = out of range or the slug can never catch the target.
pub fn shot(weapon: Weapon, r: V2, u: V2, size: f64, evade: f64) -> Option<Shot> {
    let d = r.len();
    if weapon == Weapon::None || d > range(weapon) {
        return None;
    }
    // ships in the same parking orbit are still some way apart
    let r = if d < MIN_RANGE { if d > 1e-9 { r * (MIN_RANGE / d) } else { v2(MIN_RANGE, 0.0) } } else { r };
    let d = r.len();
    match weapon {
        Weapon::Energy => {
            let tof = d / LIGHT_SPEED;
            Some(Shot { tof, hit: 1.0, factor: 1.0 / (1.0 + (d / BEAM_FALLOFF).powi(2)) })
        }
        Weapon::Beam => {
            let tof = d / LIGHT_SPEED;
            Some(Shot { tof, hit: 1.0, factor: 1.0 / (1.0 + (d / BATTERY_FALLOFF).powi(2)) })
        }
        Weapon::Kinetic => {
            let tof = lead_time(r, u, KINETIC_SPEED)?;
            // the slug's speed relative to the target on impact is |r| / tof
            let impact = d / tof;
            let u_perp = r.norm().cross(u).abs();
            let sigma = 0.5 * evade * tof * tof + TRACKING_ERROR * u_perp * tof;
            let hit = 1.0 / (1.0 + (sigma / size.max(1e-6)).powi(2));
            Some(Shot { tof, hit, factor: KINETIC_BONUS * (impact / KINETIC_SPEED).powi(2) })
        }
        Weapon::None => None,
    }
}

/// Class damage multiplier of attacker `a` against target `t`.
fn class_mult(a: &Combatant, t: &Combatant) -> f64 {
    match (a.class, t.class) {
        (Some(ac), Some(tc)) => ac.vs(tc),
        (Some(ac), None) => ac.vs_defense(),
        (None, _) => 1.0,
    }
}

/// Expected damage `a` deals to `t` in `dt` seconds, if `t` is a valid target.
pub fn expected_damage(a: &Combatant, t: &Combatant, dt: f64) -> Option<f64> {
    if a.owner == t.owner || a.attack <= 0.0 {
        return None;
    }
    let s = shot(a.weapon, t.pos - a.pos, t.vel - a.vel, t.size, t.evade)?;
    let hit = (s.hit * a.acc_mult).min(1.0);
    Some(a.attack * dt / 3600.0 * a.dmg_mult * hit * s.factor * class_mult(a, t))
}

/// One combat step of `dt` seconds. Every armed combatant fires at the enemy it hurts most
/// (ships before defenses, then the weakest, then the lowest key). Returns damage taken per
/// combatant, in input order. The result does not depend on the order of `cs`.
pub fn resolve(cs: &[Combatant], dt: f64) -> Vec<f64> {
    resolve_filtered(cs, dt, &|_, _| true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ship(key: u64, owner: PlayerId, class: ShipClass, pos: V2, vel: V2) -> Combatant {
        let st = class.stats();
        Combatant { key, owner, class: Some(class), weapon: st.weapon, attack: st.attack, hp: st.hp, pos, vel, size: size(class), evade: evasion(class), dmg_mult: 1.0, acc_mult: 1.0 }
    }

    #[test]
    fn lead_solves_intercept() {
        let r = v2(1000.0, 0.0);
        let u = v2(0.0, 10.0);
        let t = lead_time(r, u, 25.0).unwrap();
        assert!(((r + u * t).len() - 25.0 * t).abs() < 1e-6);
        // a target running away faster than the slug can't be caught
        assert!(lead_time(r, v2(30.0, 0.0), 25.0).is_none());
        // a fast crosser can't be caught either
        assert!(lead_time(r, v2(0.0, 30.0), 25.0).is_none());
    }

    #[test]
    fn kinetic_misses_more_as_relative_speed_rises() {
        let c = ShipClass::Gunship;
        let r = v2(500.0, 0.0);
        let mut last = f64::INFINITY;
        for v in [0.0, 2.0, 5.0, 10.0, 20.0] {
            let s = shot(Weapon::Kinetic, r, v2(0.0, v), size(c), evasion(c)).unwrap();
            assert!(s.hit <= last, "hit {} at {v} km/s", s.hit);
            last = s.hit;
        }
        assert!(last < 0.5);
        // energy doesn't care
        let e0 = shot(Weapon::Energy, r, V2::ZERO, size(c), evasion(c)).unwrap();
        let e1 = shot(Weapon::Energy, r, v2(0.0, 20.0), size(c), evasion(c)).unwrap();
        assert_eq!(e0, e1);
    }

    #[test]
    fn head_on_slugs_hit_harder_than_stern_chases() {
        let c = ShipClass::Gunship;
        let r = v2(500.0, 0.0);
        let head = shot(Weapon::Kinetic, r, v2(-10.0, 0.0), size(c), evasion(c)).unwrap();
        let tail = shot(Weapon::Kinetic, r, v2(10.0, 0.0), size(c), evasion(c)).unwrap();
        assert!(head.factor > 1.5 * KINETIC_BONUS && tail.factor < 0.5 * KINETIC_BONUS);
        assert!(head.hit > tail.hit);
    }

    #[test]
    fn out_of_range_never_fires() {
        let c = ShipClass::Gunship;
        assert!(shot(Weapon::Kinetic, v2(KINETIC_RANGE + 1.0, 0.0), V2::ZERO, size(c), evasion(c)).is_none());
        assert!(shot(Weapon::Energy, v2(ENERGY_RANGE + 1.0, 0.0), V2::ZERO, size(c), evasion(c)).is_none());
        assert!(shot(Weapon::Energy, v2(ENERGY_RANGE - 1.0, 0.0), V2::ZERO, size(c), evasion(c)).is_some());
    }

    #[test]
    fn defense_stations_shoot_and_get_shot() {
        use crate::game::Settings;
        use crate::station::{Station, StationId, StationKind};
        use crate::system::SystemSize;
        let settings = Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 7.0 * 86_400.0, speed: 1.0 };
        let mut g = Game::new("T".into(), settings, 3, "a");
        g.add_player("b", None).unwrap();
        g.start();
        let x = g.system.bodies.iter().find(|b| b.kind == crate::BodyKind::Moon && g.bodies[b.id.0 as usize].owner.is_none()).unwrap().id;
        g.ships.clear();
        g.stations.push(Station { id: StationId(0), owner: 0, kind: StationKind::Defense, body: x, hp: StationKind::Defense.max_hp(), fuel: 0.0, done_t: 0.0, work_left: 0.0 });
        let st = ShipClass::Bomber.stats();
        g.spawn(1, ShipClass::Bomber, x);
        g.advance_to(g.t + 1800.0);
        let ship_hp = g.ships.first().map(|s| s.hp).unwrap_or(0.0);
        assert!(ship_hp < st.hp, "platform didn't fire");
        assert!(g.stations.first().map(|s| s.hp).unwrap_or(0.0) < StationKind::Defense.max_hp(), "bomber didn't fire back");
    }

    #[test]
    fn resolve_is_order_independent() {
        let cs = vec![
            ship(1, 0, ShipClass::Bomber, V2::ZERO, V2::ZERO),
            ship(2, 0, ShipClass::Interceptor, v2(10.0, 0.0), V2::ZERO),
            ship(3, 1, ShipClass::Gunship, v2(300.0, 0.0), v2(0.0, 3.0)),
            ship(4, 1, ShipClass::Gunship, v2(320.0, 0.0), v2(0.0, 3.0)),
            ship(5, 1, ShipClass::Interceptor, v2(2000.0, 0.0), v2(-5.0, 0.0)),
        ];
        let d = resolve(&cs, 60.0);
        let mut rev = cs.clone();
        rev.reverse();
        let mut dr = resolve(&rev, 60.0);
        dr.reverse();
        assert_eq!(d, dr);
        assert!(d.iter().all(|x| x.is_finite() && *x >= 0.0));
        assert!(d[2] + d[3] > 0.0 && d[0] + d[1] > 0.0);
    }
}

// ------------------------------------------------------------------ game integration

use crate::game::{EventKind, Game, Ship, ShipId, ShipState};
use crate::station::StationKind;
use crate::system::BodyId;

/// Sub-steps per combat tick: flybys at tens of km/s cross a weapon envelope in a minute or two.
const SUBSTEPS: usize = 4;

/// Something that can fight over a span of time: who, the body it's parked at, and it as a
/// combatant at the start and the end of the span.
pub(crate) type Engager = (Who, Option<BodyId>, Combatant, Combatant);

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Who {
    Ship(usize),
    Defense(BodyId),
    Station(usize),
}

impl Game {
    /// Research multipliers for combat: (kinetic damage, energy damage, accuracy).
    pub fn combat_mods(&self, p: PlayerId) -> (f64, f64, f64) {
        let m = self.mods(p);
        (m.kinetic_dmg, m.energy_dmg, m.accuracy)
    }

    /// "Under attack" / "Battle" notifications, once per battle per body: a battle is on while
    /// two or more sides have armed power in orbit there.
    fn battle_alerts(&mut self) {
        for bi in 0..self.bodies.len() {
            let body = BodyId(bi as u32);
            let powers = self.power_at(body);
            if powers.iter().filter(|p| p.1 > 0.0).count() < 2 {
                self.bodies[bi].in_combat = false;
                continue;
            }
            if self.bodies[bi].in_combat {
                continue;
            }
            self.bodies[bi].in_combat = true;
            let bname = self.system.body(body).name.clone();
            let owner = self.bodies[bi].owner;
            if let Some(o) = owner {
                self.event(Some(o), EventKind::Battle, "Under attack", &format!("Enemy ships are attacking {bname}"), Some(body), None);
            }
            for (p, _) in powers.iter().filter(|p| p.1 > 0.0 && Some(p.0) != owner) {
                self.event(Some(*p), EventKind::Battle, "Battle", &format!("Your ships are fighting at {bname}"), Some(body), None);
            }
        }
    }

    /// Absolute position and velocity of a ship at `t`, if it exists then.
    pub(crate) fn ship_state_abs(sys: &crate::System, ship: &mut Ship, t: f64) -> Option<(V2, V2, Option<BodyId>)> {
        match &mut ship.state {
            ShipState::Parked { body, .. } => Some((sys.pos(*body, t), sys.vel(*body, t), Some(*body))),
            ShipState::Flying(f) => {
                let st = f.trajectory(sys).state_at(sys, t)?;
                Some((sys.pos(st.parent, t) + st.r, sys.vel(st.parent, t) + st.v, None))
            }
        }
    }

    /// Research multipliers (damage, accuracy) for `p` firing `w`.
    pub(crate) fn weapon_mods(&self, p: PlayerId, w: Weapon) -> (f64, f64) {
        let m = self.combat_mods(p);
        (if w == Weapon::Kinetic { m.0 } else { m.1 }, m.2)
    }

    /// A ship as a combatant at absolute `pos`/`vel` (time `t` decides a first strike out of the dark).
    pub(crate) fn ship_combatant(&self, s: &Ship, pos: V2, vel: V2, t: f64) -> Combatant {
        let st = s.class.stats();
        let (dm, am) = self.weapon_mods(s.owner, st.weapon);
        Combatant {
            key: s.id.0 as u64,
            owner: s.owner,
            class: Some(s.class),
            weapon: st.weapon,
            attack: st.attack,
            hp: s.hp,
            pos,
            vel,
            size: size(s.class),
            evade: evasion(s.class),
            dmg_mult: dm * Game::strike_mult(s, t),
            acc_mult: am,
        }
    }

    /// A body's planetary defenses as a combatant at `t` (None = unowned or knocked out).
    pub(crate) fn defense_combatant(&self, body: BodyId, t: f64) -> Option<Combatant> {
        let bs = &self.bodies[body.0 as usize];
        let o = bs.owner?;
        if bs.def_hp <= 0.0 {
            return None;
        }
        let (dm, am) = self.weapon_mods(o, Weapon::Energy);
        Some(Combatant {
            key: 1 << 40 | body.0 as u64,
            owner: o,
            class: None,
            weapon: Weapon::Energy,
            attack: self.def_attack(body),
            hp: bs.def_hp,
            pos: self.system.pos(body, t),
            vel: self.system.vel(body, t),
            size: self.system.body(body).radius.max(20.0),
            evade: 0.0,
            dmg_mult: dm,
            acc_mult: am,
        })
    }

    /// Station `si` as a combatant at `t` (None while it isn't built); defense platforms shoot back.
    pub(crate) fn station_combatant(&self, si: usize, t: f64) -> Option<Combatant> {
        let st = &self.stations[si];
        if !st.active(t) {
            return None;
        }
        let (dm, am) = self.weapon_mods(st.owner, Weapon::Energy);
        let armed = st.kind.attack() > 0.0;
        let weapon = match st.kind {
            StationKind::LaserBattery => Weapon::Beam,
            _ if armed => Weapon::Energy,
            _ => Weapon::None,
        };
        Some(Combatant {
            key: 2 << 40 | st.id.0 as u64,
            owner: st.owner,
            class: None,
            weapon,
            attack: st.kind.attack(),
            hp: st.hp,
            pos: self.system.pos(st.body, t),
            vel: self.system.vel(st.body, t),
            size: 2.0,
            evade: 0.0,
            dmg_mult: dm,
            acc_mult: am,
        })
    }

    /// Everything that can fight, as (who, parked-at, combatant at t0, combatant at t1), and the ids
    /// of dark (cloaked) ships, by the same index.
    pub(crate) fn gather(&mut self, t0: f64, t1: f64) -> (Vec<Engager>, Vec<Option<ShipId>>) {
        let mut all: Vec<Engager> = vec![];
        // dark (cloaked) ships: only targetable by players whose sensors see them, and they hold
        // fire unless fired on; whoever is seen and engaged comes out of the dark
        let mut dark: Vec<Option<ShipId>> = vec![];
        for i in 0..self.ships.len() {
            let Some(b) = Self::ship_state_abs(&self.system, &mut self.ships[i], t1) else { continue };
            let a = Self::ship_state_abs(&self.system, &mut self.ships[i], t0).unwrap_or(b);
            let s = &self.ships[i];
            dark.push(s.dark(t1).then_some(s.id));
            all.push((Who::Ship(i), b.2, self.ship_combatant(s, a.0, a.1, t1), self.ship_combatant(s, b.0, b.1, t1)));
        }
        for bi in 0..self.bodies.len() {
            let body = BodyId(bi as u32);
            if let (Some(a), Some(b)) = (self.defense_combatant(body, t0), self.defense_combatant(body, t1)) {
                all.push((Who::Defense(body), Some(body), a, b));
            }
        }
        for si in 0..self.stations.len() {
            let Some(b) = self.station_combatant(si, t1) else { continue };
            let a = self.station_combatant(si, t0).unwrap_or(b);
            all.push((Who::Station(si), Some(self.stations[si].body), a, b));
        }
        dark.resize(all.len(), None);
        (all, dark)
    }

    /// Whether player `q`'s sensors see ship `id` (dark ships can only be fought by those who do).
    pub(crate) fn seen_by(&self, q: PlayerId, id: ShipId) -> bool {
        self.players.get(q as usize).is_some_and(|p| p.vision.ships.binary_search(&id).is_ok())
    }

    /// Which of `all` can engage each other over the span they cover (closest approach of linear
    /// relative motion): the engaged indices and the pairs (i < j), sorted.
    pub(crate) fn engagements(&self, all: &[Engager], dark: &[Option<ShipId>]) -> (Vec<usize>, Vec<(usize, usize)>) {
        let seen_by = |q: PlayerId, id: ShipId| self.seen_by(q, id);
        // who can possibly engage whom this tick (closest approach of linear relative motion)
        let n = all.len();
        let mut engaged = vec![false; n];
        let mut pairs: Vec<(usize, usize)> = vec![];
        for i in 0..n {
            for j in i + 1..n {
                let (a, b) = (&all[i], &all[j]);
                if a.2.owner == b.2.owner || (a.1.is_some() && b.1.is_some() && a.1 != b.1) {
                    continue;
                }
                if dark[i].is_some_and(|id| !seen_by(b.2.owner, id)) || dark[j].is_some_and(|id| !seen_by(a.2.owner, id)) {
                    continue;
                }
                let reach = range(a.2.weapon).max(range(b.2.weapon)) * if a.2.attack > 0.0 || b.2.attack > 0.0 { 1.0 } else { 0.0 };
                if reach <= 0.0 {
                    continue;
                }
                let d0 = b.2.pos - a.2.pos;
                let dd = (b.3.pos - a.3.pos) - d0;
                let s = if dd.len2() > 0.0 { (-d0.dot(dd) / dd.len2()).clamp(0.0, 1.0) } else { 0.0 };
                if (d0 + dd * s).len() <= reach {
                    engaged[i] = true;
                    engaged[j] = true;
                    pairs.push((i, j));
                }
            }
        }
        let idx: Vec<usize> = (0..n).filter(|&i| engaged[i]).collect();
        (idx, pairs)
    }

    /// Remove destroyed ships (`lost`, with the body they died near) and stations, and log it.
    pub(crate) fn bury(&mut self, lost: Vec<(Ship, Option<BodyId>)>) {
        self.ships.retain(|s| s.hp > 0.0);
        let dead: Vec<(PlayerId, String, BodyId)> =
            self.stations.iter().filter(|s| s.hp <= 0.0).map(|s| (s.owner, s.kind.label().to_string(), s.body)).collect();
        self.stations.retain(|s| s.hp > 0.0);
        for (o, label, b) in dead {
            let bn = self.system.body(b).name.clone();
            self.event(Some(o), EventKind::Battle, "Station destroyed", &format!("{label} at {bn} was destroyed"), Some(b), None);
        }
        for (s, near) in lost {
            let place = near.map(|b| format!("at {}", self.system.body(b).name)).unwrap_or_else(|| "in deep space".into());
            self.ship_note(crate::shiplog::ShipNote { to: s.owner, kind: EventKind::Battle, title: ("Ship destroyed", "Ships destroyed"), ship: s.id, name: &s.name, verb: ("was", "were"), rest: format!("destroyed {place}"), body_ref: near, ship_ref: None });
        }
    }

    /// One combat tick ending at `self.t`: everything armed shoots whatever enemy is in range,
    /// parked fleets, ships in flight and planetary defenses alike. Two things parked at
    /// *different* bodies never fight; ships in transit can be shot by (and shoot) anything.
    pub(crate) fn combat_tick(&mut self, dt: f64) {
        self.battle_alerts();
        let (t0, t1) = (self.t - dt, self.t);
        let (all, dark) = self.gather(t0, t1);
        let (idx, pairs) = self.engagements(&all, &dark);
        if idx.is_empty() {
            return;
        }
        for &i in &idx {
            if let (Who::Ship(si), Some(_)) = (all[i].0, dark[i]) {
                self.break_dark(si);
            }
        }
        // separate battlefields (parked at different bodies) stay separate even if a flyby
        // links them: a combatant may only target partners it was paired with
        let mut dmg = vec![0.0; all.len()];
        let h = dt / SUBSTEPS as f64;
        for k in 0..SUBSTEPS {
            let f = (k as f64 + 0.5) / SUBSTEPS as f64;
            let cs: Vec<Combatant> = idx
                .iter()
                .map(|&i| {
                    let (_, _, a, b) = &all[i];
                    Combatant { pos: a.pos + (b.pos - a.pos) * f, vel: a.vel + (b.vel - a.vel) * f, hp: a.hp - dmg[i], ..*a }
                })
                .collect();
            let allowed = |x: usize, y: usize| {
                let (i, j) = (idx[x].min(idx[y]), idx[x].max(idx[y]));
                pairs.binary_search(&(i, j)).is_ok()
            };
            let d = resolve_filtered(&cs, h, &allowed);
            for (x, &i) in idx.iter().enumerate() {
                dmg[i] += d[x];
            }
        }
        // apply
        let mut fought: Vec<BodyId> = vec![];
        for &i in &idx {
            if dmg[i] <= 0.0 {
                continue;
            }
            if let Some(b) = all[i].1 {
                if !fought.contains(&b) {
                    fought.push(b);
                }
            }
            match all[i].0 {
                Who::Ship(si) => self.ships[si].hp -= dmg[i],
                Who::Defense(b) => {
                    let bs = &mut self.bodies[b.0 as usize];
                    bs.def_hp = (bs.def_hp - dmg[i]).max(0.0);
                }
                Who::Station(si) => self.stations[si].hp -= dmg[i],
            }
        }
        let mut lost: Vec<(Ship, Option<BodyId>)> = vec![];
        for &i in &idx {
            if let Who::Ship(si) = all[i].0 {
                if self.ships[si].hp <= 0.0 {
                    let s = self.ships[si].clone();
                    let near = all[i].1.or_else(|| match &s.state {
                        ShipState::Flying(f) => f.cache.as_ref().and_then(|tr| tr.seg_at(t1)).map(|sg| sg.parent).filter(|p| !self.system.body(*p).is_star()),
                        _ => None,
                    });
                    lost.push((s, near));
                }
            }
        }
        self.bury(lost);
        fought.sort();
        for b in fought {
            self.retreats(b);
        }
    }
}

/// `resolve` restricted to target pairs for which `allowed(i, j)` holds.
fn resolve_filtered(cs: &[Combatant], dt: f64, allowed: &dyn Fn(usize, usize) -> bool) -> Vec<f64> {
    let mut order: Vec<usize> = (0..cs.len()).collect();
    order.sort_by_key(|&i| cs[i].key);
    let mut dmg = vec![0.0; cs.len()];
    for &i in &order {
        let a = &cs[i];
        if a.attack <= 0.0 || a.weapon == Weapon::None {
            continue;
        }
        let mut best: Option<(usize, f64)> = None;
        for &j in &order {
            let t = &cs[j];
            if i == j || !allowed(i, j) || t.hp - dmg[j] <= 0.0 || (a.is_defense() && t.is_defense()) {
                continue;
            }
            let Some(x) = expected_damage(a, t, dt) else { continue };
            if x <= 0.0 {
                continue;
            }
            let better = match best {
                None => true,
                Some((b, bx)) => {
                    let tb = &cs[b];
                    // ships before defenses, then more damage, then the weakest
                    (tb.is_defense() && !t.is_defense())
                        || (tb.is_defense() == t.is_defense()
                            && (x > bx * (1.0 + 1e-9) || ((x - bx).abs() <= bx * 1e-9 && (t.hp - dmg[j]) < (tb.hp - dmg[b]))))
                }
            };
            if better {
                best = Some((j, x));
            }
        }
        if let Some((j, x)) = best {
            dmg[j] += x;
        }
    }
    dmg
}
