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

use crate::game::{EventKind, Game, Ship, ShipState};
use crate::system::BodyId;

/// Sub-steps per combat tick: flybys at tens of km/s cross a weapon envelope in a minute or two.
const SUBSTEPS: usize = 4;

#[derive(Clone, Copy, PartialEq)]
enum Who {
    Ship(usize),
    Defense(BodyId),
}

impl Game {
    /// Research multipliers for combat: (kinetic damage, energy damage, accuracy).
    /// The research thread wires this to `Game::mods`.
    pub fn combat_mods(&self, _p: PlayerId) -> (f64, f64, f64) {
        (1.0, 1.0, 1.0)
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
    fn ship_state_abs(sys: &crate::System, ship: &mut Ship, t: f64) -> Option<(V2, V2, Option<BodyId>)> {
        match &mut ship.state {
            ShipState::Parked { body, .. } => Some((sys.pos(*body, t), sys.vel(*body, t), Some(*body))),
            ShipState::Flying(f) => {
                let st = f.trajectory(sys).state_at(sys, t)?;
                Some((sys.pos(st.parent, t) + st.r, sys.vel(st.parent, t) + st.v, None))
            }
        }
    }

    /// One combat tick ending at `self.t`: everything armed shoots whatever enemy is in range,
    /// parked fleets, ships in flight and planetary defenses alike. Two things parked at
    /// *different* bodies never fight; ships in transit can be shot by (and shoot) anything.
    pub(crate) fn combat_tick(&mut self, dt: f64) {
        self.battle_alerts();
        let (t0, t1) = (self.t - dt, self.t);
        // (who, parked-at, combatant at t0, combatant at t1)
        let mut all: Vec<(Who, Option<BodyId>, Combatant, Combatant)> = vec![];
        let sys = &self.system;
        let mut mods = vec![];
        for p in 0..self.players.len() {
            mods.push(self.combat_mods(p as PlayerId));
        }
        let modf = |p: PlayerId, w: Weapon| {
            let m = mods.get(p as usize).copied().unwrap_or((1.0, 1.0, 1.0));
            (if w == Weapon::Kinetic { m.0 } else { m.1 }, m.2)
        };
        for (i, s) in self.ships.iter_mut().enumerate() {
            let Some(b) = Self::ship_state_abs(sys, s, t1) else { continue };
            let a = Self::ship_state_abs(sys, s, t0).unwrap_or(b);
            let st = s.class.stats();
            let (dm, am) = modf(s.owner, st.weapon);
            let c = |pos, vel| Combatant {
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
                dmg_mult: dm,
                acc_mult: am,
            };
            all.push((Who::Ship(i), b.2, c(a.0, a.1), c(b.0, b.1)));
        }
        for (bi, bs) in self.bodies.iter().enumerate() {
            let Some(o) = bs.owner else { continue };
            if bs.def_hp <= 0.0 {
                continue;
            }
            let body = BodyId(bi as u32);
            let (dm, am) = modf(o, Weapon::Energy);
            let c = |t| Combatant {
                key: 1 << 40 | bi as u64,
                owner: o,
                class: None,
                weapon: Weapon::Energy,
                attack: bs.def_attack(),
                hp: bs.def_hp,
                pos: sys.pos(body, t),
                vel: sys.vel(body, t),
                size: sys.body(body).radius.max(20.0),
                evade: 0.0,
                dmg_mult: dm,
                acc_mult: am,
            };
            all.push((Who::Defense(body), Some(body), c(t0), c(t1)));
        }
        // TODO(research): active Defense stations at body b join here as Who::Station.

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
        if idx.is_empty() {
            return;
        }
        // separate battlefields (parked at different bodies) stay separate even if a flyby
        // links them: a combatant may only target partners it was paired with
        let mut dmg = vec![0.0; n];
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
        self.ships.retain(|s| s.hp > 0.0);
        for (s, near) in lost {
            let place = near.map(|b| format!("at {}", self.system.body(b).name)).unwrap_or_else(|| "in deep space".into());
            self.event(Some(s.owner), EventKind::Battle, "Ship destroyed", &format!("{} was destroyed {place}", s.name), near, None);
        }
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
