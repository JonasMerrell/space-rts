//! Live battles (rules 6, `BATTLE_RULES`): when enemies come within weapon range, the fight plays out
//! second by second in a small 2D arena that players can watch and steer (Jonas 2026-10-29).
//!
//! Each fighter moves as it naturally would (parked in orbit, or along its trajectory) plus a
//! deviation `dp`/`dv` built up by burns made in the battle, in straight lines with no gravity.
//! Lasers are instant and always hit, weaker with range. Slugs are real salvos that fly at
//! `KINETIC_SPEED` towards where the target will be, with a little random spread, and burst when they
//! pass close to an enemy, so a ship that sees one coming can burn out of the way. Burns spend the
//! ship's real fuel and are limited by its thrust. A ship nobody is steering follows its `Stance`.
//!
//! Randomness comes from each battle's own seeded generator, so the server and every client that
//! knows the same orders see the same shots.
use serde::{Deserialize, Serialize};

use crate::combat::{expected_damage, lead_time, range, Combatant, Who, KINETIC_BONUS, KINETIC_SPEED};
use crate::game::{EventKind, Game, PlayerId, Ship, ShipClass, ShipId, ShipState, Weapon, TICK};
use crate::station::StationId;
use crate::system::{BodyId, Rng, System};
use crate::traj::{End, State};
use crate::v2::V2;

/// First rules revision with live battles (older wars keep `Game::combat_tick`).
pub const BATTLE_RULES: u32 = 6;
/// Battle step, game seconds.
pub const BATTLE_DT: f64 = 1.0;
/// Damage multiplier against the per-hour attack numbers, so fights take minutes instead of hours.
pub const DMG_SCALE: f64 = 10.0;
/// A salvo bursts when it passes this close to an enemy (km, plus the target's size).
pub const BURST: f64 = 5.0;
/// Aiming spread of a salvo (radians, one sigma) before accuracy research.
pub const SPREAD: f64 = 0.0015;
/// Salvos that hit nothing are gone after this long (s).
pub const SALVO_LIFE: f64 = 300.0;
/// Fighters this much beyond weapon reach of every enemy leave the battle (km).
pub const DISENGAGE: f64 = 3_000.0;
/// Ships parked at a body stay within this distance of their parking orbit while they fight (km).
pub const PARK_LEASH: f64 = 3_000.0;
/// Players hear about enemy flights heading their way this long before they arrive (real seconds).
pub const INBOUND_WARN: f64 = 3600.0;
/// The autopilot starts dodging a salvo that will pass close within this many seconds.
pub const REACT: f64 = 60.0;
/// The autopilot only spends fuel on positioning while the ship has more than this share left.
pub const FUEL_RESERVE: f64 = 0.25;
/// Δv (km/s) the autopilot spends on dodging in one battle (Evasive: 2.5×); a player can do better.
pub const DODGE_BUDGET: f64 = 1.5;
/// Hits and beams are kept this long for drawing (s).
pub const HIT_MEMORY: f64 = 3.0;

/// What a fighter does when nobody is steering it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stance {
    /// the class default: slug ships attack, laser ships keep their distance, unarmed ships evade
    #[default]
    Auto,
    /// close to the best range for its weapon and keep shooting
    Aggressive,
    /// hold where its lasers out-range enemy slugs, back off when they close
    Distance,
    /// dodge first, keep clear of enemies, shoot only when safe
    Evasive,
}

impl Stance {
    pub const ALL: [Stance; 4] = [Stance::Auto, Stance::Aggressive, Stance::Distance, Stance::Evasive];

    pub fn label(self) -> &'static str {
        match self {
            Stance::Auto => "Auto",
            Stance::Aggressive => "Aggressive",
            Stance::Distance => "Keep distance",
            Stance::Evasive => "Evasive",
        }
    }

    /// `Auto` resolved for a ship class.
    pub fn resolve(self, class: ShipClass) -> Stance {
        if self != Stance::Auto {
            return self;
        }
        match class.stats().weapon {
            Weapon::Kinetic => Stance::Aggressive,
            Weapon::Energy | Weapon::Beam => Stance::Distance,
            Weapon::None => Stance::Evasive,
        }
    }
}

/// Engine acceleration a ship can use to manoeuvre in battle, km/s².
pub fn thrust(class: ShipClass) -> f64 {
    use ShipClass::*;
    match class {
        Scout | Decoy => 0.03,
        Interceptor => 0.02,
        Bomber => 0.01,
        Gunship => 0.008,
        Constructor => 0.006,
        Tanker => 0.004,
        Dreadnought => 0.003,
        Torpedo => 0.05,
        Slug => 0.0,
    }
}

/// Seconds between shots.
pub fn reload(class: Option<ShipClass>) -> f64 {
    match class {
        Some(ShipClass::Interceptor) => 20.0,
        Some(ShipClass::Bomber) => 30.0,
        _ => 10.0,
    }
}

/// Something that takes part in a battle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Unit {
    Ship(ShipId),
    /// a body's planetary defenses
    Defense(BodyId),
    Station(StationId),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fighter {
    pub unit: Unit,
    pub owner: PlayerId,
    /// position and velocity relative to the fighter's natural motion, from burns made in battle
    pub dp: V2,
    pub dv: V2,
    /// game time it can fire again
    pub reload_t: f64,
    /// the player's pick of target (None = the stance picks)
    #[serde(default)]
    pub target: Option<Unit>,
    /// a player is steering: the autopilot doesn't move it (it still shoots)
    #[serde(default)]
    pub manual: bool,
    /// Δv the player ordered that is still to be burned (km/s, burned as fast as thrust allows)
    #[serde(default)]
    pub burn: V2,
    /// Δv the autopilot has spent dodging in this battle
    #[serde(default)]
    pub dodged: f64,
}

/// Slugs in flight. Position and velocity are relative to the battle's frame (`Battle::frame`), in
/// which they fly straight.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Salvo {
    pub id: u32,
    pub owner: PlayerId,
    pub from: Unit,
    pub aimed_at: Unit,
    pub pos: V2,
    pub vel: V2,
    pub fired_t: f64,
    /// damage before the impact-speed factor and class matchup
    pub base: f64,
    pub class: Option<ShipClass>,
}

/// A hit (or a laser shot) recently, for drawing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hit {
    pub t: f64,
    pub from: Unit,
    pub to: Unit,
    pub dmg: f64,
    /// laser (drawn as a beam) rather than a salvo burst
    pub beam: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Battle {
    pub id: u32,
    /// the body it's fought at (None = deep space)
    pub body: Option<BodyId>,
    /// deep space: a point falling freely around the star, (t, position, velocity) relative to it
    #[serde(default)]
    pub free: Option<(f64, V2, V2)>,
    pub start_t: f64,
    pub fighters: Vec<Fighter>,
    #[serde(default)]
    pub salvos: Vec<Salvo>,
    #[serde(default)]
    pub hits: Vec<Hit>,
    pub next_salvo: u32,
    pub rng: u64,
    /// every player who had something in it
    pub sides: Vec<PlayerId>,
    /// ships each side lost (by player id)
    pub lost: Vec<u32>,
}

impl Battle {
    pub fn fighter(&self, u: Unit) -> Option<&Fighter> {
        self.fighters.iter().find(|f| f.unit == u)
    }

    /// The arena's frame at `t`: absolute position and velocity of its centre. It falls freely (with
    /// the body it's fought at, or around the star), so salvos fly straight in it the way the ships
    /// parked there sit still. Arena coordinates are absolute ones minus this.
    pub fn frame(&self, sys: &System, t: f64) -> (V2, V2) {
        match (self.body, self.free) {
            (Some(b), _) => (sys.pos(b, t), sys.vel(b, t)),
            (None, Some((t0, r, v))) => {
                let star = sys.star();
                let (r, v) = crate::kepler::propagate(star.mu, r, v, t - t0);
                (sys.pos(star.id, t) + r, sys.vel(star.id, t) + v)
            }
            (None, None) => (V2::ZERO, V2::ZERO),
        }
    }
}

/// A fighter's state this step.
struct Now {
    c: Combatant,
    parked: Option<BodyId>,
    dark: Option<ShipId>,
    /// index into `Game::ships`
    ship: Option<usize>,
}

/// "3 Gunships, Scout"
fn class_list(classes: &[ShipClass]) -> String {
    let mut n: Vec<(&str, usize)> = vec![];
    for c in classes {
        let name = c.stats().name;
        match n.iter_mut().find(|x| x.0 == name) {
            Some(x) => x.1 += 1,
            None => n.push((name, 1)),
        }
    }
    n.iter().map(|(c, k)| if *k > 1 { format!("{k} {c}s") } else { c.to_string() }).collect::<Vec<_>>().join(", ")
}

fn class_mult(a: Option<ShipClass>, t: Option<ShipClass>) -> f64 {
    match (a, t) {
        (Some(ac), Some(tc)) => ac.vs(tc),
        (Some(ac), None) => ac.vs_defense(),
        (None, _) => 1.0,
    }
}

fn gauss(rng: &mut Rng) -> f64 {
    let u = rng.f().max(1e-12);
    let v = rng.f();
    (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
}

impl Game {
    /// The battle `u` is fighting in.
    pub fn battle_of(&self, u: Unit) -> Option<&Battle> {
        self.battles.iter().find(|b| b.fighters.iter().any(|f| f.unit == u))
    }

    fn who_unit(&self, w: Who) -> Unit {
        match w {
            Who::Ship(i) => Unit::Ship(self.ships[i].id),
            Who::Defense(b) => Unit::Defense(b),
            Who::Station(i) => Unit::Station(self.stations[i].id),
        }
    }

    /// Where `f` is and what it can do at `t` (None = it no longer exists).
    fn fighter_now(&mut self, f: &Fighter, t: f64) -> Option<Now> {
        match f.unit {
            Unit::Ship(id) => {
                let i = self.ships.iter().position(|s| s.id == id)?;
                let (pos, vel, parked) = Self::ship_state_abs(&self.system, &mut self.ships[i], t)?;
                let s = &self.ships[i];
                let c = self.ship_combatant(s, pos + f.dp, vel + f.dv, t);
                Some(Now { c, parked, dark: s.dark(t).then_some(id), ship: Some(i) })
            }
            Unit::Defense(b) => {
                let c = self.defense_combatant(b, t)?;
                (c.owner == f.owner).then_some(Now { c, parked: Some(b), dark: None, ship: None })
            }
            Unit::Station(id) => {
                let si = self.stations.iter().position(|s| s.id == id)?;
                let mut c = self.station_combatant(si, t)?;
                c.pos += f.dp;
                Some(Now { c, parked: Some(self.stations[si].body), dark: None, ship: None })
            }
        }
    }

    /// Can `a` shoot at `b`? (Different sides, not parked at different bodies, dark ships only by
    /// those who see them, defenses don't shoot defenses.)
    fn may_target(&self, a: &Now, b: &Now) -> bool {
        a.c.side != b.c.side
            && !(a.parked.is_some() && b.parked.is_some() && a.parked != b.parked)
            && !b.dark.is_some_and(|id| !self.seen_by(a.c.owner, id))
            && !(a.c.is_defense() && b.c.is_defense())
    }

    /// Every TICK (rules 6): start battles where enemies are about to come within weapon range, and
    /// pull newcomers into battles already running.
    pub(crate) fn battle_scan(&mut self) {
        let t = self.t;
        let (mut all, dark) = self.gather(t, t + TICK);
        let units: Vec<Unit> = all.iter().map(|a| self.who_unit(a.0)).collect();
        let member: Vec<Option<usize>> = units.iter().map(|u| self.battles.iter().position(|b| b.fighters.iter().any(|f| f.unit == *u))).collect();
        for (k, m) in member.iter().enumerate() {
            if let Some(bi) = m {
                let f = self.battles[*bi].fighter(units[k]).unwrap();
                let (dp, dv) = (f.dp, f.dv);
                all[k].2.pos += dp;
                all[k].2.vel += dv;
                all[k].3.pos += dp + dv * TICK;
                all[k].3.vel += dv;
            }
        }
        let (_, mut pairs) = self.engagements(&all, &dark);
        // mass-driver slugs are projectiles, not ships: only interdiction grids stop them (ordnance.rs)
        pairs.retain(|&(i, j)| all[i].2.class != Some(ShipClass::Slug) && all[j].2.class != Some(ShipClass::Slug));
        // group what will fight into connected sets, with existing battles as one piece each
        let n = all.len();
        let mut root: Vec<usize> = (0..n).collect();
        fn find(r: &mut [usize], mut x: usize) -> usize {
            while r[x] != x {
                r[x] = r[r[x]];
                x = r[x];
            }
            x
        }
        let join = |r: &mut Vec<usize>, a: usize, b: usize| {
            let (ra, rb) = (find(r, a), find(r, b));
            if ra != rb {
                r[ra.max(rb)] = ra.min(rb);
            }
        };
        for &(i, j) in &pairs {
            join(&mut root, i, j);
        }
        for bi in 0..self.battles.len() {
            let ks: Vec<usize> = (0..n).filter(|&k| member[k] == Some(bi)).collect();
            for w in ks.windows(2) {
                join(&mut root, w[0], w[1]);
            }
        }
        let mut fights: Vec<bool> = vec![false; n];
        for &(i, j) in &pairs {
            fights[i] = true;
            fights[j] = true;
        }
        let mut groups: Vec<(usize, Vec<usize>)> = vec![];
        for (k, _) in fights.iter().enumerate().filter(|f| *f.1) {
            let r = find(&mut root, k);
            match groups.iter_mut().find(|g| g.0 == r) {
                Some(g) => g.1.push(k),
                None => groups.push((r, vec![k])),
            }
        }
        // the same fight comes out the same whatever order the ships are listed in
        for g in groups.iter_mut() {
            g.1.sort_by_key(|&k| units[k]);
        }
        groups.sort_by_key(|g| units[g.1[0]]);
        for (_, ks) in groups {
            // battles this group touches: merge them into the oldest
            let mut olds: Vec<usize> = ks.iter().filter_map(|&k| member[k]).collect();
            olds.sort();
            olds.dedup();
            let newcomers: Vec<usize> = ks.iter().copied().filter(|&k| member[k].is_none()).collect();
            if newcomers.is_empty() && olds.len() < 2 {
                continue;
            }
            let id = match olds.first() {
                Some(&bi) => self.battles[bi].id,
                None => {
                    let body = {
                        let mut bodies: Vec<BodyId> = ks.iter().filter_map(|&k| all[k].1).collect();
                        bodies.sort();
                        // the body most of them are at
                        bodies.chunk_by(|a, b| a == b).max_by_key(|c| c.len()).map(|c| c[0])
                    };
                    let id = self.next_battle;
                    self.next_battle += 1;
                    let seed = (id as u64) << 32 ^ t.to_bits() ^ self.code.bytes().fold(0u64, |h, b| h.rotate_left(5) ^ b as u64);
                    // deep space: the frame starts at the middle of the fight, moving with it
                    let free = body.is_none().then(|| {
                        let k = ks.len() as f64;
                        let p = ks.iter().fold(V2::ZERO, |a, &i| a + all[i].2.pos) / k;
                        let v = ks.iter().fold(V2::ZERO, |a, &i| a + all[i].2.vel) / k;
                        let star = self.system.star().id;
                        (t, p - self.system.pos(star, t), v - self.system.vel(star, t))
                    });
                    self.battles.push(Battle { id, body, free, start_t: t, fighters: vec![], salvos: vec![], hits: vec![], next_salvo: 0, rng: Rng::new(seed).0, sides: vec![], lost: vec![] });
                    id
                }
            };
            // merge the younger battles in (their indices shift as we remove, so go by id)
            let ids: Vec<u32> = olds.iter().skip(1).map(|&bi| self.battles[bi].id).collect();
            for oid in ids {
                let oi = self.battles.iter().position(|b| b.id == oid).unwrap();
                let old = self.battles.remove(oi);
                let b = self.battles.iter_mut().find(|b| b.id == id).unwrap();
                b.fighters.extend(old.fighters);
                b.salvos.extend(old.salvos.into_iter().map(|mut s| {
                    s.id += 1 << 20;
                    s
                }));
                for p in old.sides {
                    if !b.sides.contains(&p) {
                        b.sides.push(p);
                    }
                }
                for (p, l) in old.lost.iter().enumerate() {
                    if b.lost.len() <= p {
                        b.lost.resize(p + 1, 0);
                    }
                    b.lost[p] += l;
                }
            }
            let bi = self.battles.iter().position(|b| b.id == id).unwrap();
            let mut fresh_sides: Vec<PlayerId> = vec![];
            for &k in &newcomers {
                let owner = all[k].2.owner;
                let unit = units[k];
                // fleets parked at the battle's body spread out around their parking orbit, each side
                // in its own stretch of it about 900 km along from the next
                let dp = match (unit, all[k].1) {
                    (Unit::Ship(_), Some(at)) => {
                        let park = self.system.body(at).park_radius();
                        let step = (900.0 / park).min(std::f64::consts::FRAC_PI_2);
                        let b = &mut self.battles[bi];
                        let mut rng = Rng(b.rng);
                        let a = (owner as f64 + rng.range(-0.15, 0.15)) * step;
                        let r = park + rng.range(0.0, 300.0);
                        b.rng = rng.0;
                        V2::from_angle(a) * r
                    }
                    // stations sit at their own spot on the parking orbit
                    (Unit::Station(sid), Some(at)) => V2::from_angle(sid.0 as f64 * 2.4) * self.system.body(at).park_radius(),
                    _ => V2::ZERO,
                };
                if let Who::Ship(si) = all[k].0 {
                    if dark[k].is_some() {
                        self.break_dark(si);
                    }
                }
                let b = &mut self.battles[bi];
                b.fighters.push(Fighter { unit, owner, dp, dv: V2::ZERO, reload_t: t, target: None, manual: false, burn: V2::ZERO, dodged: 0.0 });
                if !b.sides.contains(&owner) {
                    b.sides.push(owner);
                    fresh_sides.push(owner);
                }
            }
            let b = &mut self.battles[bi];
            b.fighters.sort_by_key(|f| f.unit);
            b.fighters.dedup_by_key(|f| f.unit);
            let body = b.body;
            for p in fresh_sides {
                self.battle_joined(p, body);
            }
        }
        // bodies with a battle on: guns aren't repaired, and the map shows it
        for bs in self.bodies.iter_mut() {
            bs.in_combat = false;
        }
        let at: Vec<BodyId> = self.battles.iter().filter_map(|b| b.body).collect();
        for b in at {
            self.bodies[b.0 as usize].in_combat = true;
            self.retreats(b);
        }
    }

    /// `Command::SetStance`.
    pub(crate) fn set_stance(&mut self, p: PlayerId, ships: &[ShipId], stance: Stance) -> Result<(), String> {
        if ships.iter().any(|id| self.ship(*id).is_none_or(|s| s.owner != p)) {
            return Err("not your ship".into());
        }
        for s in self.ships.iter_mut().filter(|s| ships.contains(&s.id)) {
            s.stance = stance;
        }
        Ok(())
    }

    /// Battle orders (`Command::BattleBurn` and friends): `f` applied to each ship's fighter.
    pub(crate) fn battle_order(&mut self, p: PlayerId, ships: &[ShipId], f: impl Fn(&mut Fighter)) -> Result<(), String> {
        if ships.iter().any(|id| self.ship(*id).is_none_or(|s| s.owner != p)) {
            return Err("not your ship".into());
        }
        let mut any = false;
        for b in self.battles.iter_mut() {
            for fi in b.fighters.iter_mut().filter(|fi| matches!(fi.unit, Unit::Ship(id) if ships.contains(&id))) {
                f(fi);
                any = true;
            }
        }
        if any {
            Ok(())
        } else {
            Err("not in a battle".into())
        }
    }

    fn battle_joined(&mut self, p: PlayerId, body: Option<BodyId>) {
        let (title, text) = match body {
            Some(b) => {
                let name = self.system.body(b).name.clone();
                if self.bodies[b.0 as usize].owner == Some(p) {
                    ("Under attack", format!("Enemy ships are attacking {name}"))
                } else {
                    ("Battle", format!("Your ships are fighting at {name}"))
                }
            }
            None => ("Battle", "Your ships are fighting in deep space".to_string()),
        };
        self.event(Some(p), EventKind::Battle, title, &text, body, None);
    }

    /// Once per tick: warn each player about enemy flights they can see heading for a body they own
    /// or have ships at, arriving within `INBOUND_WARN` of real time. One warning per flight and
    /// destination, grouped per enemy and body.
    pub(crate) fn inbound_warnings(&mut self) {
        let t = self.t;
        let window = INBOUND_WARN * self.settings.speed.max(1e-9);
        for p in 0..self.players.len() {
            let me = p as PlayerId;
            if self.players[p].bot.is_some() {
                continue;
            }
            // forget flights that landed, died or changed course
            let ships = &self.ships;
            self.players[p].warned.retain(|&(id, b)| ships.iter().any(|s| s.id == id && matches!(&s.state, ShipState::Flying(f) if f.dest == Some(b))));
            let mut news: Vec<(PlayerId, BodyId, f64, Vec<ShipClass>)> = vec![];
            for i in 0..self.ships.len() {
                let s = &self.ships[i];
                if s.owner == me || self.players[p].vision.ships.binary_search(&s.id).is_err() {
                    continue;
                }
                let ShipState::Flying(f) = &s.state else { continue };
                let Some(dest) = f.dest else { continue };
                let mine = self.bodies[dest.0 as usize].owner == Some(me) || self.ships.iter().any(|o| o.owner == me && o.parked_at() == Some(dest));
                if !mine || self.players[p].warned.contains(&(s.id, dest)) {
                    continue;
                }
                let (owner, id, class) = (s.owner, s.id, crate::cloak::apparent_class(s));
                let ShipState::Flying(f) = &mut self.ships[i].state else { continue };
                let eta = match f.trajectory(&self.system).end {
                    End::Capture { body, t: te, .. } | End::Impact { body, t: te } if body == dest => te,
                    _ => continue,
                };
                if eta - t > window {
                    continue;
                }
                self.players[p].warned.push((id, dest));
                match news.iter_mut().find(|n| n.0 == owner && n.1 == dest) {
                    Some(n) => {
                        n.2 = n.2.min(eta);
                        n.3.push(class);
                    }
                    None => news.push((owner, dest, eta, vec![class])),
                }
            }
            for (owner, dest, eta, classes) in news {
                let who = self.players[owner as usize].name.clone();
                let bname = self.system.body(dest).name.clone();
                let mins = ((eta - t) / self.settings.speed.max(1e-9) / 60.0).ceil().max(1.0);
                // the apps count "in about N min" down from the event's time when they show it later
                let text = format!("Battle at {bname} in about {mins:.0} min: {who}'s {} on the way", class_list(&classes));
                self.event(Some(me), EventKind::Battle, "Enemies inbound", &text, Some(dest), None);
            }
        }
    }

    /// One `BATTLE_DT` step of every battle, ending at `self.t`.
    pub(crate) fn battle_steps(&mut self) {
        let ids: Vec<u32> = self.battles.iter().map(|b| b.id).collect();
        for id in ids {
            if let Some(bi) = self.battles.iter().position(|b| b.id == id) {
                self.battle_step(bi);
            }
        }
    }

    fn battle_step(&mut self, bi: usize) {
        let t = self.t;
        // allies (team wars) never shoot each other's salvos down or dodge them
        let side_of: Vec<PlayerId> = (0..self.players.len() as PlayerId).map(|p| self.side(p)).collect();
        let sd = |p: PlayerId| side_of.get(p as usize).copied().unwrap_or(p);
        let dt = BATTLE_DT;
        let mut b = std::mem::replace(&mut self.battles[bi], Battle { id: 0, body: None, free: None, start_t: 0.0, fighters: vec![], salvos: vec![], hits: vec![], next_salvo: 0, rng: 0, sides: vec![], lost: vec![] });
        let mut rng = Rng(b.rng);
        let (fp, fv) = b.frame(&self.system, t);

        // where everyone is (fighters whose ship or station is gone drop out)
        let mut now: Vec<Now> = vec![];
        let mut keep: Vec<Fighter> = vec![];
        for f in b.fighters.drain(..) {
            if let Some(n) = self.fighter_now(&f, t) {
                now.push(n);
                keep.push(f);
            }
        }
        b.fighters = keep;
        let n = now.len();
        let mut dmg = vec![0.0; n];
        let mut broke: Vec<usize> = vec![];

        // fire
        for i in 0..n {
            let a = &now[i];
            if a.c.attack <= 0.0 || a.c.weapon == Weapon::None || b.fighters[i].reload_t > t {
                continue;
            }
            let rl = reload(a.c.class);
            let pick = b.fighters[i].target.and_then(|u| b.fighters.iter().position(|f| f.unit == u));
            let score = |j: usize| -> Option<f64> {
                if i == j || now[j].c.hp - dmg[j] <= 0.0 || !self.may_target(a, &now[j]) {
                    return None;
                }
                expected_damage(&a.c, &now[j].c, rl).filter(|x| *x > 0.0)
            };
            let mut best: Option<(usize, f64)> = pick.and_then(|j| score(j).map(|x| (j, x)));
            if best.is_none() {
                for j in 0..n {
                    let Some(x) = score(j) else { continue };
                    let better = match best {
                        None => true,
                        Some((k, bx)) => {
                            let (tk, tj) = (&now[k].c, &now[j].c);
                            (tk.is_defense() && !tj.is_defense())
                                || (tk.is_defense() == tj.is_defense() && (x > bx * (1.0 + 1e-9) || ((x - bx).abs() <= bx * 1e-9 && (tj.hp - dmg[j]) < (tk.hp - dmg[k]))))
                        }
                    };
                    if better {
                        best = Some((j, x));
                    }
                }
            }
            let Some((j, x)) = best else { continue };
            // evasive ships only shoot when nothing is coming at them
            if let Some(si) = a.ship {
                let st = self.ships[si].stance.resolve(self.ships[si].class);
                if st == Stance::Evasive && threat(&b.salvos, &side_of, (fp, fv), a).is_some() {
                    continue;
                }
            }
            b.fighters[i].reload_t = t + rl;
            broke.push(i);
            broke.push(j);
            let (tgt, from, to) = (&now[j].c, b.fighters[i].unit, b.fighters[j].unit);
            match a.c.weapon {
                Weapon::Kinetic => {
                    let r = tgt.pos - a.c.pos;
                    let u = tgt.vel - a.c.vel;
                    let Some(tof) = lead_time(r, u, KINETIC_SPEED) else { continue };
                    let aim = (r + u * tof).norm();
                    let dir = aim.rotate(gauss(&mut rng) * SPREAD / a.c.acc_mult.max(0.1));
                    let base = a.c.attack * rl / 3600.0 * DMG_SCALE * a.c.dmg_mult;
                    b.salvos.push(Salvo { id: b.next_salvo, owner: a.c.owner, from, aimed_at: to, pos: a.c.pos - fp, vel: a.c.vel - fv + dir * KINETIC_SPEED, fired_t: t, base, class: a.c.class });
                    b.next_salvo += 1;
                }
                _ => {
                    let d = x * DMG_SCALE;
                    dmg[j] += d;
                    b.hits.push(Hit { t, from, to, dmg: d, beam: true });
                }
            }
        }

        // salvos fly; the first enemy one passes close to takes the burst, and the body the battle is
        // at stops them (the guns on it are hit if that's what they were aimed at)
        let ground = b.body.map(|x| (x, self.system.body(x).radius));
        let mut flying: Vec<Salvo> = vec![];
        for mut s in b.salvos.drain(..) {
            let mut hit: Option<(f64, usize, f64)> = None;
            if let Some((x, radius)) = ground {
                let tc = if s.vel.len2() > 0.0 { (-s.pos.dot(s.vel) / s.vel.len2()).clamp(0.0, dt) } else { 0.0 };
                if (s.pos + s.vel * tc).len() <= radius {
                    let guns = (0..n).find(|&j| b.fighters[j].unit == Unit::Defense(x) && now[j].c.side != sd(s.owner) && s.aimed_at == Unit::Defense(x));
                    if let Some(j) = guns {
                        hit = Some((tc, j, (s.vel + fv - now[j].c.vel).len()));
                    } else {
                        continue;
                    }
                }
            }
            for j in 0..n {
                let c = &now[j].c;
                if c.side == sd(s.owner) || c.hp - dmg[j] <= 0.0 || matches!(b.fighters[j].unit, Unit::Defense(_)) {
                    continue;
                }
                let r0 = s.pos + fp - c.pos;
                let rv = s.vel + fv - c.vel;
                let tc = if rv.len2() > 0.0 { (-r0.dot(rv) / rv.len2()).clamp(0.0, dt) } else { 0.0 };
                if (r0 + rv * tc).len() <= BURST + c.size && hit.is_none_or(|h| tc < h.0) {
                    hit = Some((tc, j, rv.len()));
                }
            }
            match hit {
                Some((_, j, speed)) => {
                    let factor = (KINETIC_BONUS * (speed / KINETIC_SPEED).powi(2)).clamp(0.2, 3.0);
                    let d = s.base * factor * class_mult(s.class, now[j].c.class);
                    dmg[j] += d;
                    b.hits.push(Hit { t, from: s.from, to: b.fighters[j].unit, dmg: d, beam: false });
                    broke.push(j);
                }
                None => {
                    s.pos += s.vel * dt;
                    if t + dt - s.fired_t < SALVO_LIFE {
                        flying.push(s);
                    }
                }
            }
        }
        b.salvos = flying;
        b.hits.retain(|h| t - h.t < HIT_MEMORY);

        // whoever fought comes out of the dark
        broke.sort();
        broke.dedup();
        for &i in &broke {
            if let (Some(si), Some(_)) = (now[i].ship, now[i].dark) {
                self.break_dark(si);
            }
        }

        // damage
        let mut lost: Vec<(Ship, Option<BodyId>)> = vec![];
        for i in 0..n {
            if dmg[i] <= 0.0 {
                continue;
            }
            match b.fighters[i].unit {
                Unit::Ship(_) => {
                    let si = now[i].ship.unwrap();
                    self.ships[si].hp -= dmg[i];
                    if self.ships[si].hp <= 0.0 {
                        let s = self.ships[si].clone();
                        lost.push((s, now[i].parked.or(b.body)));
                        let o = b.fighters[i].owner as usize;
                        if b.lost.len() <= o {
                            b.lost.resize(o + 1, 0);
                        }
                        b.lost[o] += 1;
                    }
                }
                Unit::Defense(body) => {
                    let bs = &mut self.bodies[body.0 as usize];
                    bs.def_hp = (bs.def_hp - dmg[i]).max(0.0);
                }
                Unit::Station(id) => {
                    if let Some(st) = self.stations.iter_mut().find(|s| s.id == id) {
                        st.hp -= dmg[i];
                    }
                }
            }
        }

        // manoeuvre: player burns first, the stance's autopilot for everyone else
        for i in 0..n {
            let Some(si) = now[i].ship else { continue };
            if self.ships[si].hp <= 0.0 {
                continue;
            }
            let ship = &self.ships[si];
            let amax = thrust(ship.class) * dt;
            let f = &b.fighters[i];
            let mut dodging = false;
            let want = if f.burn.len() > 1e-9 {
                f.burn
            } else if f.manual {
                V2::ZERO
            } else {
                let enemies: Vec<&Now> = (0..n).filter(|&j| j != i && now[j].c.hp - dmg[j] > 0.0 && self.may_target(&now[j], &now[i])).map(|j| &now[j]).collect();
                let (w, dodge) = autopilot(ship, f, &now[i], &enemies, &b.salvos, &side_of, (fp, fv));
                dodging = dodge;
                w
            };
            let mut step = if want.len() > amax { want.norm() * amax } else { want };
            if step.len() < 1e-9 {
                continue;
            }
            let ship = &mut self.ships[si];
            let need = ship.fuel_for(step.len());
            if need > ship.fuel {
                let k = if step.len() > 0.0 { ship.dv_left() / step.len() } else { 0.0 };
                step = step * k.clamp(0.0, 1.0);
                ship.fuel = 0.0;
            } else {
                ship.fuel -= need;
            }
            if step.len() < 1e-9 {
                continue;
            }
            ship.last_burn_t = t;
            let f = &mut b.fighters[i];
            if f.burn.len() > 1e-9 {
                f.burn = if f.burn.len() <= step.len() + 1e-9 { V2::ZERO } else { f.burn - step };
            }
            f.dv += step;
            if dodging {
                f.dodged += step.len();
            }
        }

        // coast
        for (i, f) in b.fighters.iter_mut().enumerate() {
            f.dp += f.dv * dt;
            // a parked ship's orbit keeps it near its body
            let Some(at) = now[i].parked.filter(|_| matches!(f.unit, Unit::Ship(_))) else { continue };
            let leash = self.system.body(at).park_radius() + PARK_LEASH;
            if f.dp.len() > leash {
                let out = f.dp.norm();
                let away = f.dv.dot(out);
                if away > 0.0 {
                    f.dv -= out * away;
                }
                f.dp = out * leash;
            }
        }

        // who is out of reach of every enemy leaves; when nobody is left fighting, it's over
        let mut stay = vec![false; n];
        for i in 0..n {
            for j in 0..n {
                if i == j || !(self.may_target(&now[i], &now[j]) || self.may_target(&now[j], &now[i])) {
                    continue;
                }
                let (a, c) = (&now[i].c, &now[j].c);
                if a.attack <= 0.0 && c.attack <= 0.0 {
                    continue;
                }
                let reach = range(a.weapon).max(range(c.weapon)) + DISENGAGE;
                if a.pos.dist(c.pos) <= reach {
                    stay[i] = true;
                }
            }
            if !stay[i] && b.salvos.iter().any(|s| sd(s.owner) != now[i].c.side && (s.pos + fp).dist(now[i].c.pos) < 2.0 * DISENGAGE) {
                stay[i] = true;
            }
        }
        let dead: Vec<bool> = (0..n).map(|i| matches!(b.fighters[i].unit, Unit::Ship(_)) && now[i].ship.is_some_and(|si| self.ships[si].hp <= 0.0)).collect();
        let fighting = (0..n).any(|i| stay[i] && !dead[i]);
        let mut left: Vec<Fighter> = vec![];
        let mut kept: Vec<Fighter> = vec![];
        for (i, f) in b.fighters.drain(..).enumerate() {
            if dead[i] {
                continue;
            }
            if fighting && stay[i] {
                kept.push(f);
            } else {
                left.push(f);
            }
        }
        b.fighters = kept;
        b.rng = rng.0;
        for f in &left {
            self.leave_battle(f);
        }
        self.battles[bi] = b;
        if !fighting {
            let b = self.battles.remove(bi);
            self.battle_over(&b);
        }
        if !lost.is_empty() {
            self.bury(lost);
        }
    }

    /// A fighter leaves its battle: a ship in flight keeps the course its battle burns gave it; a
    /// parked ship settles back into its parking orbit.
    fn leave_battle(&mut self, f: &Fighter) {
        let Unit::Ship(id) = f.unit else { return };
        if f.dp == V2::ZERO && f.dv == V2::ZERO {
            return;
        }
        let t = self.t;
        let sys = &self.system;
        let Some(s) = self.ships.iter_mut().find(|s| s.id == id) else { return };
        if let ShipState::Flying(fl) = &mut s.state {
            let Some(st) = fl.trajectory(sys).state_at(sys, t) else { return };
            fl.start = State { r: st.r + f.dp, v: st.v + f.dv, ..st };
            fl.burns.retain(|b| b.t > t);
            fl.cache = None;
        }
    }

    fn battle_over(&mut self, b: &Battle) {
        let place = match b.body {
            Some(body) => format!("at {}", self.system.body(body).name),
            None => "in deep space".to_string(),
        };
        for &p in &b.sides {
            let mine = b.lost.get(p as usize).copied().unwrap_or(0);
            let theirs: u32 = b.lost.iter().enumerate().filter(|(q, _)| *q != p as usize).map(|(_, l)| l).sum();
            let ships = |n: u32| if n == 1 { "1 ship".to_string() } else { format!("{n} ships") };
            let text = format!("The battle {place} is over: you lost {}, the enemy lost {}", ships(mine), ships(theirs));
            self.event(Some(p), EventKind::Battle, "Battle over", &text, b.body, None);
        }
        if let Some(body) = b.body {
            self.bodies[body.0 as usize].in_combat = self.battles.iter().any(|o| o.body == Some(body));
        }
    }
}

/// The enemy salvo that will pass closest to `me` soonest, if any comes near enough to burst:
/// (seconds to closest approach, where it will be relative to `me` then, relative velocity).
fn threat(salvos: &[Salvo], sides: &[PlayerId], frame: (V2, V2), me: &Now) -> Option<(f64, V2, V2)> {
    let mut best: Option<(f64, V2, V2)> = None;
    for s in salvos.iter().filter(|s| sides.get(s.owner as usize).copied().unwrap_or(s.owner) != me.c.side) {
        let r = s.pos + frame.0 - me.c.pos;
        let rv = s.vel + frame.1 - me.c.vel;
        if rv.len2() <= 0.0 {
            continue;
        }
        let tc = -r.dot(rv) / rv.len2();
        if !(0.0..=REACT).contains(&tc) {
            continue;
        }
        let miss = r + rv * tc;
        if miss.len() <= BURST + me.c.size + 2.0 && best.is_none_or(|b| tc < b.0) {
            best = Some((tc, miss, rv));
        }
    }
    best
}

/// The Δv a ship's stance wants this step (km/s; the caller limits it to the ship's thrust).
fn autopilot(ship: &Ship, f: &Fighter, me: &Now, enemies: &[&Now], salvos: &[Salvo], sides: &[PlayerId], frame: (V2, V2)) -> (V2, bool) {
    let stance = ship.stance.resolve(ship.class);
    let full = ship.stats().fuel_cap.max(1e-9);
    // nimble ships can afford to dodge more (an interceptor's thrust gets the whole budget)
    let budget = DODGE_BUDGET * (thrust(ship.class) / thrust(ShipClass::Interceptor)).min(1.5) * if stance == Stance::Evasive { 2.5 } else { 1.0 };
    // dodge: burn sideways out of the salvo's path
    if let Some((tc, miss, rv)) = threat(salvos, sides, frame, me) {
        if f.dodged < budget && (ship.fuel > FUEL_RESERVE * full || (stance == Stance::Evasive && ship.fuel > 0.1 * full)) {
            let across = rv.perp().norm();
            // moving along the salvo's path doesn't help: get clear sideways, away from where it'll pass
            let off = miss.dot(across);
            let dir = if off.abs() > 1e-6 { across * -off.signum() } else if ship.id.0.is_multiple_of(2) { across } else { -across };
            let gap = BURST + me.c.size + 3.0 - off.abs();
            // just enough sideways speed to be clear when it passes (a burn takes a few steps)
            return (dir * (gap / (tc - 3.0).max(1.0)), true);
        }
    }
    if ship.fuel <= FUEL_RESERVE * full {
        return (V2::ZERO, false);
    }
    // positioning: only in a stand-up fight (fast flybys just pass through)
    let Some(e) = enemies.iter().filter(|e| e.c.attack > 0.0 || stance == Stance::Aggressive).min_by(|a, b| a.c.pos.dist(me.c.pos).total_cmp(&b.c.pos.dist(me.c.pos))) else {
        return (V2::ZERO, false);
    };
    let rel = me.c.vel - e.c.vel;
    if rel.len() > 1.5 {
        return (V2::ZERO, false);
    }
    let to = e.c.pos - me.c.pos;
    let d = to.len();
    if d < 1e-6 {
        return (V2::ZERO, false);
    }
    let los = to / d;
    let slugs = e.c.weapon == Weapon::Kinetic;
    let want_r = match stance {
        Stance::Aggressive if me.c.weapon == Weapon::Kinetic => 400.0,
        Stance::Aggressive if me.c.weapon != Weapon::None => 800.0,
        Stance::Distance if slugs && me.c.weapon != Weapon::None => 2_500.0,
        Stance::Distance => 1_500.0,
        _ => (range(e.c.weapon) * 1.1).min(9_000.0),
    };
    // out of the band and not already heading back into it: close (or open) at 0.1 km/s. Inside it,
    // and for sideways drift, nothing: every burn costs fuel the ship needs to get home.
    let speed = rel.dot(los);
    let pace = ((d - want_r).abs() / 900.0).clamp(0.1, 0.4);
    let want_speed = if d > want_r * 1.5 && speed < pace * 0.5 {
        pace
    } else if d < want_r * 0.6 && speed > -pace * 0.5 {
        -pace
    } else {
        return (V2::ZERO, false);
    };
    (los * (want_speed - speed), false)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::game::{Command, Flight, Settings};
    use crate::system::SystemSize;
    use crate::BodyKind;

    /// Three players, everything cleared, and a free moon to fight over.
    pub(crate) fn arena() -> (Game, BodyId) {
        let settings = Settings { name: "t".into(), size: SystemSize::Medium, max_players: 4, length: 14.0 * 86_400.0, speed: 1.0, teams: 0 };
        let mut g = Game::new("T".into(), settings, 7, "a");
        g.add_player("b", None).unwrap();
        g.add_player("c", None).unwrap();
        g.start();
        assert!(g.rules >= BATTLE_RULES);
        let x = g.system.bodies.iter().find(|b| b.kind == BodyKind::Moon && g.bodies[b.id.0 as usize].owner.is_none()).unwrap().id;
        g.ships.clear();
        g.fleets.clear();
        for q in &mut g.bodies {
            q.queue.clear();
        }
        (g, x)
    }

    /// Run second by second until the first battle is over (or `secs` pass): (its length, salvos fired).
    fn run(g: &mut Game, secs: f64) -> (f64, u32) {
        let t0 = g.t;
        let mut started = None;
        let mut salvos = 0;
        let end = g.t + secs;
        while g.t < end {
            let t = g.t + 1.0;
            g.advance_to(t);
            if let Some(b) = g.battles.first() {
                started.get_or_insert(g.t);
                salvos = salvos.max(b.next_salvo);
            } else if let Some(s) = started {
                return (g.t - s, salvos);
            }
        }
        (g.t - started.unwrap_or(t0), salvos)
    }

    #[test]
    fn parked_fleets_fight_it_out_in_minutes() {
        let (mut g, x) = arena();
        for _ in 0..3 {
            g.spawn(0, ShipClass::Interceptor, x);
            g.spawn(1, ShipClass::Interceptor, x);
        }
        let fuel0: f64 = g.ships.iter().map(|s| s.fuel).sum();
        let (len, salvos) = run(&mut g, 4.0 * 3600.0);
        let fuel1: f64 = g.ships.iter().map(|s| s.fuel).sum();
        assert!(g.battles.is_empty(), "battle never ended");
        assert!(salvos > 0);
        assert!(g.ships.iter().map(|s| s.owner).collect::<std::collections::BTreeSet<_>>().len() <= 1, "both sides still there");
        // 5–15 real minutes at 1× (game time runs 2× real)
        assert!((600.0..=1800.0).contains(&len), "battle took {len} s");
        // dodging and positioning cost fuel, but nobody is left stranded
        assert!(fuel1 < fuel0);
        assert!(g.ships.iter().all(|s| s.fuel > 0.2 * s.stats().fuel_cap), "{:?}", g.ships.iter().map(|s| s.fuel).collect::<Vec<_>>());
        for p in [0, 1] {
            assert!(g.events_for(p).any(|e| e.title == "Battle"));
            assert!(g.events_for(p).any(|e| e.title == "Battle over"));
        }
    }

    #[test]
    fn battles_are_deterministic_and_survive_a_save() {
        let (mut g, x) = arena();
        for c in [ShipClass::Interceptor, ShipClass::Bomber, ShipClass::Gunship] {
            g.spawn(0, c, x);
            g.spawn(1, c, x);
        }
        g.advance_to(g.t + 300.0);
        assert!(!g.battles.is_empty() && !g.battles[0].salvos.is_empty());
        let mut saved: Game = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        let mut copy = g.clone();
        let t = g.t + 900.0;
        for h in [&mut g, &mut saved, &mut copy] {
            h.advance_to(t);
        }
        let key = |h: &Game| serde_json::to_string(&(&h.ships, &h.battles)).unwrap();
        assert_eq!(key(&g), key(&copy));
        assert_eq!(key(&g), key(&saved));
    }

    #[test]
    fn fight_order_does_not_change_the_outcome() {
        let fleet = |rev: bool| {
            let (mut g, x) = arena();
            let mut v = vec![(0, ShipClass::Gunship), (0, ShipClass::Interceptor), (1, ShipClass::Bomber), (1, ShipClass::Interceptor)];
            if rev {
                v.reverse();
            }
            for (p, c) in v {
                g.spawn(p, c, x);
            }
            // same ids whatever order they were built in
            let mut ids: Vec<u32> = g.ships.iter().map(|s| s.id.0).collect();
            ids.sort();
            for (s, id) in g.ships.iter_mut().zip(ids) {
                s.id = ShipId(id);
            }
            g.ships.sort_by_key(|s| (s.owner, s.class as u8));
            for (k, s) in g.ships.iter_mut().enumerate() {
                s.id = ShipId(100 + k as u32);
            }
            if rev {
                g.ships.reverse();
            }
            g.advance_to(g.t + 1200.0);
            let mut out: Vec<(u32, u64)> = g.ships.iter().map(|s| (s.id.0, s.hp.to_bits())).collect();
            out.sort();
            out
        };
        assert_eq!(fleet(false), fleet(true));
    }

    /// One bomber on bob's lone gunship, which bob steers by hand: sitting still vs burning sideways
    /// whenever a salvo is on its way.
    fn steered(dodge: bool) -> f64 {
        let (mut g, x) = arena();
        g.spawn(0, ShipClass::Bomber, x);
        let gs = g.spawn(1, ShipClass::Gunship, x);
        g.advance_to(g.t + 60.0);
        g.apply(1, Command::BattleSteer { ships: vec![gs], manual: true }).unwrap();
        let mut seen = 0;
        for _ in 0..600 {
            g.advance_to(g.t + 1.0);
            let Some(b) = g.battles.first() else { break };
            if dodge && b.next_salvo > seen {
                seen = b.next_salvo;
                let s = b.salvos.last().unwrap();
                g.apply(1, Command::BattleBurn { ships: vec![gs], dv: s.vel.perp().norm() * 0.4 }).unwrap();
            }
        }
        g.ship(gs).map_or(0.0, |s| s.hp)
    }

    #[test]
    fn a_player_can_dodge_slugs_with_burns() {
        let (still, dodged) = (steered(false), steered(true));
        assert!(dodged > still + 20.0, "hp left: sitting still {still}, dodging {dodged}");
    }

    #[test]
    fn battle_orders_need_your_ship_in_a_battle() {
        let (mut g, x) = arena();
        let a = g.spawn(0, ShipClass::Gunship, x);
        assert!(g.apply(0, Command::BattleBurn { ships: vec![a], dv: V2::ZERO }).unwrap_err().contains("battle"));
        assert!(g.apply(1, Command::SetStance { ships: vec![a], stance: Stance::Evasive }).is_err());
        g.apply(0, Command::SetStance { ships: vec![a], stance: Stance::Evasive }).unwrap();
        assert_eq!(g.ship(a).unwrap().stance, Stance::Evasive);
        let e = g.spawn(1, ShipClass::Interceptor, x);
        g.advance_to(g.t + 120.0);
        g.apply(0, Command::BattleTarget { ships: vec![a], target: Some(Unit::Ship(e)) }).unwrap();
        assert_eq!(g.battle_of(Unit::Ship(a)).unwrap().fighter(Unit::Ship(a)).unwrap().target, Some(Unit::Ship(e)));
        assert!(g.apply(1, Command::BattleBurn { ships: vec![a], dv: V2::ZERO }).is_err(), "not bob's ship");
    }

    #[test]
    fn only_the_sides_in_a_battle_see_it() {
        let (mut g, x) = arena();
        g.spawn(0, ShipClass::Gunship, x);
        g.spawn(1, ShipClass::Gunship, x);
        g.advance_to(g.t + 180.0);
        assert_eq!(g.battles.len(), 1);
        assert_eq!(g.view_for(0).battles.len(), 1);
        assert_eq!(g.view_for(1).battles.len(), 1);
        assert!(g.view_for(2).battles.is_empty());
    }

    #[test]
    fn old_wars_keep_the_old_combat() {
        let (mut g, x) = arena();
        g.rules = BATTLE_RULES - 1;
        g.spawn(0, ShipClass::Gunship, x);
        let e = g.spawn(1, ShipClass::Gunship, x);
        g.advance_to(g.t + 1800.0);
        assert!(g.battles.is_empty());
        assert!(g.ship(e).unwrap().hp < ShipClass::Gunship.stats().hp);
    }

    #[test]
    fn a_ship_flying_past_fights_and_flies_on() {
        let (mut g, x) = arena();
        for _ in 0..2 {
            g.spawn(1, ShipClass::Interceptor, x);
        }
        // alice's gunship coasts past the moon at 4 km/s, 300 km off
        let gs = g.spawn(0, ShipClass::Gunship, x);
        let parent = g.system.body(x).parent.unwrap();
        let (r, v) = g.system.rel_state(x, g.t);
        let along = v.norm();
        let start = crate::traj::State { parent, t: g.t, r: r - along * 9_000.0 + along.perp() * 300.0, v: v + along * 4.0 };
        g.ship_mut(gs).unwrap().state = ShipState::Flying(Flight { start, burns: vec![], dest: None, cache: None });
        let mut joined = false;
        let mut last: Vec<V2> = vec![];
        for _ in 0..3600 {
            g.advance_to(g.t + 1.0);
            joined |= g.battle_of(Unit::Ship(gs)).is_some();
            let Some(s) = g.ship(gs) else { break };
            let dp = g.battle_of(Unit::Ship(gs)).and_then(|b| b.fighter(Unit::Ship(gs))).map_or(V2::ZERO, |f| f.dp);
            // its course relative to the moon stays smooth, in battle and after leaving it (no jumps)
            let p = g.ship_pos(s, g.t) + dp - g.system.pos(x, g.t);
            if let [.., a, b] = last[..] {
                let kink = (p - b) - (b - a);
                assert!(kink.len() < 0.5, "jumped {} km at t {}", kink.len(), g.t);
            }
            last.push(p);
            if joined && g.battles.is_empty() {
                break;
            }
        }
        assert!(joined, "the flyby never became a battle");
        assert!(g.battles.is_empty(), "the battle never ended");
        if let Some(s) = g.ship(gs) {
            assert!(matches!(s.state, ShipState::Flying(_)), "it should still be on its way");
        }
    }

    #[test]
    fn players_hear_about_enemies_heading_their_way_once() {
        let (mut g, x) = arena();
        g.spawn(0, ShipClass::Scout, x);
        // bob's gunship flies over from the moon's planet
        let planet = g.system.body(x).parent.unwrap();
        let gs = g.spawn(1, ShipClass::Gunship, planet);
        g.apply(1, Command::Transfer { ships: vec![gs], to: x, depart_t: None }).unwrap();
        let warnings = |g: &Game, p: PlayerId| g.events_for(p).filter(|e| e.title == "Enemies inbound").count();
        let eta = |g: &mut Game| match &mut g.ship_mut(gs).unwrap().state {
            ShipState::Flying(f) if f.dest == Some(x) => {
                let mut f = f.clone();
                Some(f.trajectory(&g.system).end.t())
            }
            _ => None,
        };
        let mut warned = false;
        let end = g.t + 4.0 * 86_400.0;
        let mut left = false;
        loop {
            g.advance_to(g.t + TICK);
            assert!(g.t < end, "the gunship never got there");
            let Some(eta) = eta(&mut g) else {
                if left {
                    break;
                }
                continue;
            };
            left = true;
            let seen = g.players[0].vision.ships.contains(&gs);
            if !warned && seen && eta - g.t <= INBOUND_WARN {
                // the first tick she can see it coming within the window
                assert_eq!(warnings(&g, 0), 1, "not warned {} s before arrival", eta - g.t);
                warned = true;
            }
        }
        assert!(warned, "alice has a scout there and should have been warned");
        assert_eq!(warnings(&g, 0), 1, "one warning per flight");
        assert_eq!(warnings(&g, 1), 0);
        assert_eq!(warnings(&g, 2), 0, "carol has nothing at the moon");
        let e = g.events_for(0).find(|e| e.title == "Enemies inbound").unwrap();
        assert!(e.body.contains("Gunship") && e.body.contains(" in about "), "{}", e.body);
        assert!(g.players[0].warned.is_empty(), "forgotten once it landed");
    }
}
#[cfg(test)]
mod balance {
    use super::*;
    use ShipClass::*;

    /// Equal-cost fleets of two classes at a neutral moon: (survivors a, survivors b, battle length, average fuel share before, after).
    fn duel(a: ShipClass, b: ShipClass, budget: f64) -> (usize, usize, f64, f64, f64) {
        let (mut g, x) = super::tests::arena();
        let n = |c: ShipClass| (budget / (c.stats().matter + c.stats().energy)).round().max(1.0) as usize;
        for _ in 0..n(a) {
            g.spawn(0, a, x);
        }
        for _ in 0..n(b) {
            g.spawn(1, b, x);
        }
        let f0: f64 = g.ships.iter().map(|s| s.fuel / s.stats().fuel_cap).sum::<f64>() / g.ships.len() as f64;
        let mut start = None;
        let mut end = 0.0;
        for _ in 0..(6 * 3600) {
            let t = g.t + 1.0;
            g.advance_to(t);
            if !g.battles.is_empty() {
                start.get_or_insert(g.t);
                end = g.t;
            } else if start.is_some() {
                break;
            }
        }
        let f1: f64 = g.ships.iter().map(|s| s.fuel / s.stats().fuel_cap).sum::<f64>() / g.ships.len().max(1) as f64;
        let na = g.ships.iter().filter(|s| s.owner == 0).count();
        let nb = g.ships.iter().filter(|s| s.owner == 1).count();
        (na, nb, end - start.unwrap_or(end), f0, f1)
    }

    #[test]
    #[ignore]
    fn table() {
        for (a, b) in [(Interceptor, Bomber), (Gunship, Interceptor), (Bomber, Gunship), (Interceptor, Interceptor), (Gunship, Gunship), (Bomber, Bomber), (Dreadnought, Gunship)] {
            let (na, nb, len, f0, f1) = duel(a, b, 1200.0);
            eprintln!("{a:?} vs {b:?}: {na} vs {nb} after {:.0} min, fuel {:.0}% -> {:.0}%", len / 60.0, f0 * 100.0, f1 * 100.0);
        }
    }
}
